//! 追问用的会话历史（`docs/ai-agent-workbench.md` 16.15 B.7）与会话压缩（A.5）。
//!
//! 每一轮发出前，把本会话之前的各轮压成几行摘要（你说了什么、按什么技能处理、结果如何），连同
//! 压缩出的会话摘要交给流程（黑板 `history`）：起草模型的系统提示自动带上，「再短一点」
//! 「第二条展开说」才知道指的是什么。历史估算超过上下文窗口的四分之一时，较早的轮次交辅助模型
//! 压成一段摘要，最近 3 轮保留原样；也可以手动压（`/compact`、「•••」→「压缩会话」）。原始轮次
//! 仍在会话里可翻看，只是不再整段带进提示词。

use super::{AiPanel, AiTurn, TurnState};
use crate::agent::tools::short;
use crate::lmstudio::context::estimate_tokens;

/// 压缩时最近几轮保留原样。
pub(crate) const KEEP_RECENT: usize = 3;

/// 一轮在历史里的样子；没有可说的（还在跑）返回 None。
pub(crate) fn digest(index: usize, turn: &AiTurn) -> Option<String> {
    if turn.state.running() {
        return None;
    }
    let mut text = format!(
        "【第 {index} 轮】你说：{}\n处理：{}",
        short(turn.prompt.trim(), 200),
        turn.title
    );
    let head = || short(turn.content.trim(), 300);
    let result = match &turn.state {
        TurnState::Proposed(_) => format!("交了提案，还没处理。提案开头：{}", head()),
        TurnState::Accepted => format!("交了提案，你已接受写进正文。开头：{}", head()),
        TurnState::Discarded => format!("交了提案，你放弃了。开头：{}", head()),
        TurnState::Superseded | TurnState::Expired => {
            format!("交了提案，后来作废了。开头：{}", head())
        }
        TurnState::Reported { .. } => {
            let items: Vec<String> = turn
                .findings
                .iter()
                .take(6)
                .map(|f| format!("{}：{}", f.group, short(&f.text, 60)))
                .collect();
            if items.is_empty() {
                "问题清单：没有发现问题".to_string()
            } else {
                format!(
                    "问题清单 {} 条，前几条：{}",
                    turn.findings.len(),
                    items.join("；")
                )
            }
        }
        TurnState::Asking => {
            let asked: Vec<&str> = turn.questions.iter().map(|q| q.text.as_str()).collect();
            format!("停下来问你：{}（还没答）", asked.join("；"))
        }
        TurnState::Stopped | TurnState::Interrupted => "没有做完".to_string(),
        TurnState::Failed(error) => format!("出错了：{}", short(error, 80)),
        TurnState::Waiting | TurnState::Streaming | TurnState::Checking => return None,
    };
    text.push_str("；结果：");
    text.push_str(&result);
    if let Some(request) = &turn.request
        && !request.notes.is_empty()
    {
        text.push_str("\n你确认过：");
        text.push_str(&request.notes.join("；"));
    }
    Some(text)
}

/// 交给后台压缩的那部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Compaction {
    /// 已有的会话摘要（压缩会把它与新的几轮合成一段）。
    pub(crate) previous: String,
    pub(crate) lines: Vec<String>,
    /// 压到哪一轮为止。
    pub(crate) upto: u64,
}

/// 这一轮要带的历史。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryPlan {
    pub(crate) summary: String,
    pub(crate) recent: Vec<String>,
    /// 要先压缩的话：压哪些。压完用新摘要 + `recent`。
    pub(crate) compact: Option<Compaction>,
}

impl HistoryPlan {
    /// 不压缩（或压缩没做成）时带进提示词的历史。
    pub(crate) fn text(&self) -> String {
        render(&self.summary, &self.recent)
    }
}

/// 会话摘要 + 最近几轮 → 黑板上的 `history`。
pub(crate) fn render(summary: &str, recent: &[String]) -> String {
    let mut parts = Vec::new();
    if !summary.trim().is_empty() {
        parts.push(format!("【会话摘要（较早的轮次）】\n{}", summary.trim()));
    }
    parts.extend(recent.iter().cloned());
    parts.join("\n\n")
}

/// 算这一轮的历史。`window` 是起草模型的上下文窗口；`force` 是手动压缩（不看比例，只要有
/// 可压的就压）。
pub(crate) fn plan(panel: &AiPanel, window: usize, force: bool) -> HistoryPlan {
    let upto = panel.session.compacted_upto;
    let candidates: Vec<(u64, String)> = panel
        .turns
        .iter()
        .enumerate()
        .filter(|(_, turn)| turn.id > upto)
        .filter_map(|(index, turn)| digest(index + 1, turn).map(|text| (turn.id, text)))
        .collect();
    let summary = panel.session.summary.clone();
    let total = estimate_tokens(&summary)
        + candidates
            .iter()
            .map(|(_, text)| estimate_tokens(text))
            .sum::<usize>();
    let over = total > window / 4;
    if (force || over) && candidates.len() > KEEP_RECENT {
        let split = candidates.len() - KEEP_RECENT;
        let (older, recent) = candidates.split_at(split);
        return HistoryPlan {
            summary: summary.clone(),
            recent: recent.iter().map(|(_, text)| text.clone()).collect(),
            compact: Some(Compaction {
                previous: summary,
                lines: older.iter().map(|(_, text)| text.clone()).collect(),
                upto: older.last().map(|(id, _)| *id).unwrap_or(upto),
            }),
        };
    }
    let mut recent: Vec<String> = candidates.into_iter().map(|(_, text)| text).collect();
    // 压不了（轮数太少）又超了预算：只带最近的，先丢最早的。
    while recent.len() > 1 && estimate_tokens(&render(&summary, &recent)) > window / 4 {
        recent.remove(0);
    }
    HistoryPlan {
        summary,
        recent,
        compact: None,
    }
}

/// 让辅助模型压缩会话的提示词。
pub(crate) fn compact_prompt(compaction: &Compaction) -> String {
    let previous = if compaction.previous.trim().is_empty() {
        "（没有）".to_string()
    } else {
        compaction.previous.trim().to_string()
    };
    format!(
        "下面是公文写作软件里一段 AI 会话较早的部分。把它压成一段会话摘要，供后面的轮次理解上下文。\n\
         要写清：做过哪些事（按什么要求、交了什么结果、用户接受还是放弃）；定下来的要求与偏好；\
         用户答过的问题和答案（原样保留时间、数字、名称）；还没解决的事。\n\
         只写事实，不加评论，不超过 400 字，不要用 Markdown 标题。\n\n\
         【已有的会话摘要】\n{previous}\n\n【要并进去的轮次】\n{}",
        compaction.lines.join("\n\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_panel::ProposalSummary;

    fn panel(turns: usize) -> AiPanel {
        let mut panel = AiPanel::default();
        for index in 0..turns {
            panel.push_turn(
                "润色".into(),
                format!("第{index}次要求：{}", "压缩篇幅".repeat(10)),
                vec![],
                None,
            );
            panel.turns.last_mut().unwrap().content = "提案正文".repeat(100);
            panel.finish(TurnState::Proposed(ProposalSummary::default()));
        }
        panel
    }

    #[test]
    fn digests_say_what_was_asked_and_what_came_back() {
        let panel = panel(1);
        let text = digest(1, &panel.turns[0]).unwrap();
        assert!(text.starts_with("【第 1 轮】你说：第0次要求"), "{text}");
        assert!(text.contains("处理：润色"));
        assert!(text.contains("交了提案，还没处理"));
        assert!(text.chars().count() < 600, "提案只取开头");
    }

    #[test]
    fn short_sessions_are_carried_whole() {
        let panel = panel(2);
        let plan = plan(&panel, 32_768, false);
        assert!(plan.compact.is_none());
        assert_eq!(plan.recent.len(), 2);
        assert!(plan.text().contains("【第 2 轮】"));
    }

    #[test]
    fn long_sessions_compact_all_but_the_last_three() {
        let panel = panel(10);
        let plan = plan(&panel, 8192, false);
        let compaction = plan.compact.expect("超过窗口四分之一就压");
        assert_eq!(compaction.lines.len(), 7);
        assert_eq!(compaction.upto, 7);
        assert_eq!(plan.recent.len(), KEEP_RECENT);
        assert!(compact_prompt(&compaction).contains("【第 7 轮】"));
        // 手动压缩：不看比例。
        let forced = plan_forced(&panel);
        assert!(forced.compact.is_some());
        // 轮数不超过 3 时没什么可压。
        assert!(super::plan(&panel_of(3), 1024, true).compact.is_none());
    }

    fn plan_forced(panel: &AiPanel) -> HistoryPlan {
        plan(panel, 1_000_000, true)
    }

    fn panel_of(turns: usize) -> AiPanel {
        panel(turns)
    }

    #[test]
    fn compacted_turns_are_replaced_by_the_summary() {
        let mut panel = panel(5);
        panel.session.summary = "前面润色过两次。".into();
        panel.session.compacted_upto = 2;
        let plan = plan(&panel, 32_768, false);
        let text = plan.text();
        assert!(text.starts_with("【会话摘要（较早的轮次）】\n前面润色过两次。"));
        assert!(!text.contains("【第 1 轮】"));
        assert!(text.contains("【第 3 轮】"));
    }
}
