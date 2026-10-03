//! 起草页右侧的 AI 侧栏（智能体工作台第 ① 期：流式输出 + 输入框 + 生成卡 / 结果卡）。
//!
//! 设计见 `docs/ai-agent-workbench.md`。本期只接「润色」与「起草」两种用法：
//! 输入框写要求，发送后模型输出逐段流进任务流；流结束、闸门跑完后同一张卡
//! 转成结果卡，由用户点「采用 / 接受」才落入正文（红线 1）。仿写、知识起草与
//! 大纲暂时仍走旧工作台，侧栏右上角留了入口。
//!
//! 本文件只放状态与纯逻辑；界面在 `ai_panel/ui.rs`，挂在 `DraftPage` 上。

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

mod ui;

/// 侧栏的两种用法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PanelMode {
    /// 在事实锁定下修改现有正文（全文或选区）。
    #[default]
    Polish,
    /// 按输入框里的材料与要求起草。
    Draft,
}

impl PanelMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Polish => "润色",
            Self::Draft => "起草",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Polish => "说说怎么改，例如：压缩第二部分，不改任务、责任单位和时限",
            Self::Draft => "粘贴材料、写清要求，例如：根据以下会议纪要起草一份通知……",
        }
    }
}

/// 输入区的状态。随稿件保存，切换标签页互不影响。
#[derive(Debug, Default)]
pub(crate) struct Composer {
    pub(crate) mode: PanelMode,
    pub(crate) text: String,
    /// 打开侧栏时锁定的选区（字节区间）与当时的原文。正文被改动后按原文重新定位。
    pub(crate) selection: Option<(Range<usize>, String)>,
    /// 润色预设（`AppConfig::ai_prompts` 的 id）。None 表示不用预设。
    pub(crate) preset: Option<u32>,
    /// 起草时检索知识库。
    pub(crate) use_rag: bool,
    pub(crate) error: Option<String>,
    /// 首次打开时按设置给过默认值没有。之后以用户的勾选为准，不再覆盖。
    pub(crate) primed: bool,
}

/// 一轮请求的原始参数，「重新生成」照它再发一次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnRequest {
    pub(crate) mode: PanelMode,
    pub(crate) text: String,
    pub(crate) selection: Option<(Range<usize>, String)>,
    pub(crate) preset: Option<u32>,
    pub(crate) use_rag: bool,
}

/// 结果卡上的摘要，在提案到达时算一次，不必每帧重算。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProposalSummary {
    /// 提案正文字数。
    pub(crate) chars: usize,
    /// 提案前正文是否为空：空稿显示「采用为正文」，有稿显示「接受」。
    pub(crate) was_empty: bool,
    pub(crate) fact_changes: usize,
    pub(crate) warnings: usize,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnState {
    /// 已发出，还没收到第一个字。
    Waiting,
    /// 正在接收模型输出。
    Streaming,
    /// 模型说完了，程序在跑清洗、规整与闸门。
    Checking,
    /// 提案已就绪，等用户采用或放弃。
    Proposed(ProposalSummary),
    /// 不经审阅直接写入了正文（旧工作台的空稿起草）。
    Applied,
    Accepted,
    Discarded,
    /// 被同一篇稿件的新任务取代。
    Superseded,
    Stopped,
    Failed(String),
}

impl TurnState {
    pub(crate) fn running(&self) -> bool {
        matches!(self, Self::Waiting | Self::Streaming | Self::Checking)
    }
}

/// 任务流里的一轮。
#[derive(Debug)]
pub(crate) struct AiTurn {
    pub(crate) id: u64,
    /// 卡片抬头：「润色 · 精简篇幅」。
    pub(crate) title: String,
    /// 用户原话；旧工作台发起的任务没有原话，显示任务名。
    pub(crate) prompt: String,
    /// 上下文 chip：「选区 128 字」「知识库」。
    pub(crate) context: Vec<String>,
    /// 侧栏发起的才有，旧入口发起的不能「重新生成」。
    pub(crate) request: Option<TurnRequest>,
    pub(crate) state: TurnState,
    /// 模型正文（流式累加，未经清洗）。
    pub(crate) content: String,
    /// 思考过程。
    pub(crate) reasoning: String,
    /// 阶段提示：「正在检索知识库…」「正在校验…」。会被下一阶段冲掉。
    pub(crate) phase: String,
    /// 要一直留在卡片上的说明：知识库命中了哪几篇、为什么没命中。
    pub(crate) notes: Vec<String>,
    pub(crate) started: Instant,
    /// 结束时定格的耗时；运行中为 None，按 `started` 现算。
    pub(crate) elapsed: Option<Duration>,
}

impl AiTurn {
    pub(crate) fn elapsed(&self) -> Duration {
        self.elapsed.unwrap_or_else(|| self.started.elapsed())
    }

    fn settle(&mut self, state: TurnState) {
        if self.elapsed.is_none() {
            self.elapsed = Some(self.started.elapsed());
        }
        self.phase.clear();
        self.state = state;
    }
}

/// 模型输出撞上长度上限时附在审校提示里的文字；结果卡据此亮「输出被截断」。
pub(crate) const TRUNCATED_NOTE: &str = "模型输出达到长度上限被截断，正文可能不完整。";

/// 任务流最多保留的轮数。只存内存，不落盘。
const MAX_TURNS: usize = 20;

/// 一篇稿件的 AI 侧栏。
#[derive(Debug, Default)]
pub(crate) struct AiPanel {
    pub(crate) open: bool,
    pub(crate) composer: Composer,
    pub(crate) turns: Vec<AiTurn>,
    /// 正在跑的那一轮的停止开关。
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    next_id: u64,
}

impl AiPanel {
    /// 新开一轮。之前还挂着的提案随之作废——新任务会把 `ai_proposal` 清掉。
    pub(crate) fn push_turn(
        &mut self,
        title: String,
        prompt: String,
        context: Vec<String>,
        request: Option<TurnRequest>,
    ) -> u64 {
        for turn in &mut self.turns {
            if matches!(turn.state, TurnState::Proposed(_)) {
                turn.state = TurnState::Superseded;
            }
        }
        self.next_id += 1;
        self.turns.push(AiTurn {
            id: self.next_id,
            title,
            prompt,
            context,
            request,
            state: TurnState::Waiting,
            content: String::new(),
            reasoning: String::new(),
            phase: String::new(),
            notes: Vec::new(),
            started: Instant::now(),
            elapsed: None,
        });
        if self.turns.len() > MAX_TURNS {
            let excess = self.turns.len() - MAX_TURNS;
            self.turns.drain(..excess);
        }
        self.next_id
    }

    /// 最后一轮是否刚发出、还在等后台接手。
    pub(crate) fn has_waiting_turn(&self) -> bool {
        self.turns
            .last()
            .is_some_and(|turn| turn.state == TurnState::Waiting)
    }

    /// 正在跑的那一轮（最多一轮：同一篇稿件同时只有一个后台任务）。
    pub(crate) fn running_turn_mut(&mut self) -> Option<&mut AiTurn> {
        self.turns
            .iter_mut()
            .rev()
            .find(|turn| turn.state.running())
    }

    pub(crate) fn running(&self) -> bool {
        self.turns.iter().any(|turn| turn.state.running())
    }

    /// 收到一批增量。`done` 表示模型已经说完，接下来是程序校验。
    pub(crate) fn append(&mut self, content: &str, reasoning: &str, done: bool) {
        let Some(turn) = self.running_turn_mut() else {
            return;
        };
        turn.content.push_str(content);
        turn.reasoning.push_str(reasoning);
        if done {
            turn.state = TurnState::Checking;
            turn.phase = "正在校验…".into();
        } else if !content.is_empty() || !reasoning.is_empty() {
            turn.state = TurnState::Streaming;
            turn.phase.clear();
        }
    }

    pub(crate) fn set_phase(&mut self, phase: &str) {
        if let Some(turn) = self.running_turn_mut() {
            turn.phase = phase.to_string();
        }
    }

    pub(crate) fn note(&mut self, note: String) {
        if let Some(turn) = self.running_turn_mut() {
            turn.notes.push(note);
        }
    }

    pub(crate) fn finish(&mut self, state: TurnState) {
        self.cancel = None;
        if let Some(turn) = self.running_turn_mut() {
            turn.settle(state);
        }
    }

    /// 提案被接受或放弃（无论从结果卡还是旧审阅窗点的）。
    pub(crate) fn resolve_proposal(&mut self, accepted: bool) {
        if let Some(turn) = self
            .turns
            .iter_mut()
            .rev()
            .find(|turn| matches!(turn.state, TurnState::Proposed(_)))
        {
            turn.state = if accepted {
                TurnState::Accepted
            } else {
                TurnState::Discarded
            };
        }
    }

    /// 清空任务流。跑着的那一轮保留，否则回来的增量没处落。
    pub(crate) fn clear_history(&mut self) {
        self.turns.retain(|turn| turn.state.running());
    }
}

/// 在当前正文里找回锁定的选区。
///
/// 原位还是那段文字就用原位；否则全篇找，只有一处才认，多处或找不到都放弃——
/// 猜错的代价是把别处改坏（同 `revision::Anchor` 的取舍）。
pub(crate) fn locate_selection(
    markdown: &str,
    selection: &(Range<usize>, String),
) -> Option<String> {
    let (range, text) = selection;
    if text.trim().is_empty() {
        return None;
    }
    if markdown.get(range.clone()) == Some(text.as_str()) {
        return Some(text.clone());
    }
    let mut hits = markdown.match_indices(text.as_str());
    let first = hits.next()?;
    hits.next().is_none().then(|| first.1.to_string())
}

/// 拼润色指令：预设 + 用户的话 + 选区硬约束。
pub(crate) fn polish_instruction(preset: &str, custom: &str, selected: Option<&str>) -> String {
    let mut instruction = [preset.trim(), custom.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if instruction.is_empty() {
        instruction = "只做公文格式规整，不改写事实和措辞。".into();
    }
    if let Some(selected) = selected {
        instruction.push_str(&format!(
            "\n\n【唯一允许修改的原文片段】\n{selected}\n【范围硬约束】只修改上述片段，片段之外的文字和 Markdown 标记必须逐字保持不变；仍须返回完整 Markdown。"
        ));
    }
    instruction
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_with_turn() -> AiPanel {
        let mut panel = AiPanel::default();
        panel.push_turn("润色".into(), "压一压".into(), vec![], None);
        panel
    }

    #[test]
    fn deltas_flow_into_the_running_turn() {
        let mut panel = panel_with_turn();
        assert!(panel.has_waiting_turn());
        panel.append("", "想", false);
        assert_eq!(panel.turns[0].state, TurnState::Streaming);
        panel.append("一、", "", false);
        panel.append("总体要求", "", true);
        let turn = &panel.turns[0];
        assert_eq!(turn.content, "一、总体要求");
        assert_eq!(turn.reasoning, "想");
        assert_eq!(turn.state, TurnState::Checking);
        assert!(panel.running());
    }

    #[test]
    fn notes_stay_after_the_phase_moves_on() {
        let mut panel = panel_with_turn();
        panel.set_phase("正在检索知识库…");
        panel.note("知识库：已注入 3 段知识库参考：《甲》".into());
        panel.append("正文", "", false);
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        let turn = &panel.turns[0];
        assert!(turn.phase.is_empty());
        assert_eq!(turn.notes, ["知识库：已注入 3 段知识库参考：《甲》"]);
        // 结束之后的说明没有归属，丢掉。
        panel.note("迟到".into());
        assert_eq!(panel.turns[0].notes.len(), 1);
    }

    #[test]
    fn empty_batches_do_not_flip_waiting_to_streaming() {
        let mut panel = panel_with_turn();
        panel.append("", "", false);
        assert_eq!(panel.turns[0].state, TurnState::Waiting);
    }

    #[test]
    fn finishing_settles_elapsed_and_clears_phase() {
        let mut panel = panel_with_turn();
        panel.set_phase("正在检索知识库…");
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        let turn = &panel.turns[0];
        assert!(turn.elapsed.is_some());
        assert!(turn.phase.is_empty());
        assert!(!panel.running());
        // 没有在跑的轮次时，迟到的增量直接丢掉。
        panel.append("迟到", "", false);
        assert!(panel.turns[0].content.is_empty());
    }

    #[test]
    fn a_new_turn_supersedes_a_pending_proposal() {
        let mut panel = panel_with_turn();
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        panel.push_turn("起草".into(), "写个通知".into(), vec![], None);
        assert_eq!(panel.turns[0].state, TurnState::Superseded);
        // 已作废的提案不会被后来的接受误标。
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        panel.resolve_proposal(true);
        assert_eq!(panel.turns[0].state, TurnState::Superseded);
        assert_eq!(panel.turns[1].state, TurnState::Accepted);
    }

    #[test]
    fn history_is_capped_and_clearing_keeps_the_running_turn() {
        let mut panel = AiPanel::default();
        for index in 0..(MAX_TURNS + 5) {
            panel.push_turn(format!("第{index}轮"), String::new(), vec![], None);
            if index + 1 < MAX_TURNS + 5 {
                panel.finish(TurnState::Stopped);
            }
        }
        assert_eq!(panel.turns.len(), MAX_TURNS);
        assert_eq!(panel.turns[0].title, "第5轮");
        panel.clear_history();
        assert_eq!(panel.turns.len(), 1);
        assert!(panel.turns[0].state.running());
    }

    #[test]
    fn selection_is_found_in_place_or_by_unique_text() {
        let text = "一、甲\n二、乙\n三、丙";
        let start = text.find("二、乙").unwrap();
        let selection = (start..start + "二、乙".len(), "二、乙".to_string());
        assert_eq!(
            locate_selection(text, &selection).as_deref(),
            Some("二、乙")
        );
        // 前面插了字，原位对不上，但全篇只有一处。
        let edited = format!("前言\n{text}");
        assert_eq!(
            locate_selection(&edited, &selection).as_deref(),
            Some("二、乙")
        );
        // 出现两处：无法判定，放弃。
        let twice = format!("{text}\n二、乙");
        assert_eq!(locate_selection(&twice, &(0..3, "二、乙".into())), None);
        // 已被删掉。
        assert_eq!(locate_selection("一、甲", &selection), None);
    }

    #[test]
    fn polish_instruction_joins_parts_and_pins_the_selection() {
        assert_eq!(
            polish_instruction("", "  ", None),
            "只做公文格式规整，不改写事实和措辞。"
        );
        assert_eq!(
            polish_instruction("精简", "不改时限", None),
            "精简\n\n不改时限"
        );
        let pinned = polish_instruction("", "压缩", Some("二、乙"));
        assert!(pinned.starts_with("压缩"));
        assert!(pinned.contains("【唯一允许修改的原文片段】\n二、乙"));
    }
}
