//! 选择题：拿不准的地方不让模型猜，变成选择题交给用户（`docs/ai-agent-workbench.md` 14.11）。
//!
//! 两个时机：
//! - 动笔前：只问会让整篇写偏的事（文种、受文对象、目的、篇幅），最多 3 题；
//! - 第一稿出来后：需要用户提供的、知识库里找不到的、来源不明的，一批最多 4 题。
//!
//! 选项尽量由程序给：文种来自固定列表，事实类只给「自己填写 / 保留待核实」，只有措辞、
//! 方向这类没有标准答案的才用模型出的选项。用户的回答**按确定性规则落到工作稿**
//! （占位直接替换成答案），不再交给模型改写。

use super::gaps::{Gap, GapKind, GapStatus, Ledger};
use crate::models::TemplateKind;

/// 选项被选中后做什么。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Action {
    /// 作为已确认的补充信息交给起草（动笔前的题）。
    Note(String),
    /// 切换文种。由界面线程执行，流程本身不碰 `DraftInput`。
    SwitchKind(TemplateKind),
    /// 保持当前文种。
    KeepKind,
    /// 来源不明的事实：用户确认原文无误。
    KeepOriginal,
    /// 改成「【待核实：…】」占位。
    MarkPending,
    /// 通用选择题（`ask.choice`）：选中项的值存进步骤的 `save_as` 变量。
    Pick(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Choice {
    pub(crate) label: String,
    /// 一句话说明：出处或推荐理由。
    pub(crate) detail: String,
    pub(crate) recommended: bool,
    pub(crate) action: Action,
}

/// 这道题问的是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Target {
    PreDraft,
    Gap(usize),
    /// 流程中途的通用选择题（`ask.choice`），答案存进变量后流程接着跑。
    Pick,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Question {
    pub(crate) id: usize,
    pub(crate) text: String,
    pub(crate) choices: Vec<Choice>,
    /// `Some(提示)` 表示可以自己填写。
    pub(crate) custom_hint: Option<String>,
    /// 自己填写框里预先填好的内容（如待确认的大纲），用户在它上面改；多行时显示成多行框。
    pub(crate) prefill: String,
    /// 可以跳过：动笔前的题跳过就按现有信息写；缺口题跳过就保留待核实。
    pub(crate) skippable: bool,
    pub(crate) target: Target,
}

/// 用户对一道题的回答。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Reply {
    Choice(usize),
    Custom(String),
    Skip,
}

/// 问题不超过这么多字，长了多半是模型在解释而不是在出题。
const MAX_QUESTION_CHARS: usize = 40;

/// 解析模型出的澄清题：每行「问题｜选项1｜选项2…」，2–4 个选项。
///
/// 闸门：问题过长、选项数不对、选项为空或重复的整题丢弃；模型说「无」就是没有题。
pub(crate) fn parse_model_questions(reply: &str, max: usize) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for line in reply.lines() {
        let line = strip_numbering(line.trim());
        if line.is_empty() || line == "无" {
            continue;
        }
        let parts: Vec<String> = line
            .split(['｜', '|'])
            .map(|part| part.trim().to_string())
            .collect();
        let Some((question, options)) = parts.split_first() else {
            continue;
        };
        let question = question.trim_end_matches(['?', '？']).to_string() + "？";
        let unique = options
            .iter()
            .enumerate()
            .all(|(i, option)| !options[..i].contains(option));
        if question.chars().count() > MAX_QUESTION_CHARS + 1
            || !(2..=4).contains(&options.len())
            || options.iter().any(|option| option.is_empty())
            || !unique
            || echoes_format(&question, options)
        {
            continue;
        }
        out.push((question, options.to_vec()));
        if out.len() == max {
            break;
        }
    }
    out
}

/// 模型把提示词里的格式示例「问题｜选项1｜选项2」原样抄了出来（实测遇到过）。
fn echoes_format(question: &str, options: &[String]) -> bool {
    let is_placeholder = |text: &str| {
        text.strip_prefix("选项")
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
    };
    question.trim_end_matches('？') == "问题" || options.iter().any(|o| is_placeholder(o))
}

/// 要求里写的文种 → 可接受的文种。同一位置上长词优先：「研究报告」不算成「报告」。
const KIND_WORDS: [(&str, &[TemplateKind]); 12] = [
    ("红头呈批件", &[TemplateKind::RedHeadApproval]),
    ("电话通知", &[TemplateKind::PhoneNotice]),
    ("会议议程", &[TemplateKind::MeetingAgenda]),
    ("研究报告", &[TemplateKind::ResearchReport]),
    ("调研报告", &[TemplateKind::ResearchReport]),
    (
        "呈批件",
        &[TemplateKind::WhitePaper, TemplateKind::RedHeadApproval],
    ),
    ("白头件", &[TemplateKind::WhitePaper]),
    ("议程", &[TemplateKind::MeetingAgenda]),
    (
        "通知",
        &[TemplateKind::PlainDocument, TemplateKind::PhoneNotice],
    ),
    ("请示", &[TemplateKind::PlainDocument]),
    ("报告", &[TemplateKind::PlainDocument]),
    ("函", &[TemplateKind::OfficialLetter]),
];

/// 要求里点名的文种与当前文种对不上时，返回建议切换到的文种。
///
/// 取**最先出现**的文种词：「起草一份通知……要求各单位报送调研报告」要写的是通知，
/// 后面的调研报告是让别人交的东西。只看开头 120 字。
pub(crate) fn kind_mismatch(request: &str, current: TemplateKind) -> Option<TemplateKind> {
    let head: String = request.chars().take(120).collect();
    let (_, _, kinds) = KIND_WORDS
        .iter()
        .filter_map(|(word, kinds)| head.find(word).map(|pos| (pos, word.len(), *kinds)))
        .min_by_key(|(pos, len, _)| (*pos, std::cmp::Reverse(*len)))?;
    (!kinds.contains(&current)).then(|| kinds[0])
}

/// 动笔前的题：文种不一致（程序判断）在前，模型出的澄清题在后，总数不超过 `max`。
pub(crate) fn predraft_questions(
    request: &str,
    current: TemplateKind,
    model_questions: Vec<(String, Vec<String>)>,
    max: usize,
) -> Vec<Question> {
    let mut out = Vec::new();
    if let Some(kind) = kind_mismatch(request, current) {
        out.push(Question {
            id: 1,
            text: format!(
                "要求里写的是「{}」，当前文种是「{}」，按哪个写？",
                kind.label(),
                current.label()
            ),
            choices: vec![
                Choice {
                    label: format!("切换为{}", kind.label()),
                    detail: "版式、行文规则随文种变化".into(),
                    recommended: true,
                    action: Action::SwitchKind(kind),
                },
                Choice {
                    label: format!("保持{}", current.label()),
                    detail: String::new(),
                    recommended: false,
                    action: Action::KeepKind,
                },
            ],
            custom_hint: None,
            prefill: String::new(),
            skippable: false,
            target: Target::PreDraft,
        });
    }
    let asked_kind = !out.is_empty();
    for (question, options) in model_questions {
        if out.len() >= max {
            break;
        }
        // 实测：程序已经问了文种，模型又问「是否按通知（下行）处理」。同一件事不问两遍。
        if asked_kind && (question.contains("文种") || question.contains("行文方向")) {
            continue;
        }
        let id = out.len() + 1;
        out.push(Question {
            id,
            choices: options
                .iter()
                .map(|option| Choice {
                    label: option.clone(),
                    detail: String::new(),
                    recommended: false,
                    action: Action::Note(format!("{question}{option}")),
                })
                .collect(),
            text: question,
            custom_hint: Some("其他，自己写".into()),
            prefill: String::new(),
            skippable: true,
            target: Target::PreDraft,
        });
    }
    out
}

/// 一个缺口对应的选择题。
pub(crate) fn gap_question(id: usize, gap: &Gap) -> Question {
    let context = around(&gap.sentence, &gap.literal, 16);
    let (text, choices, custom_hint) = match gap.kind {
        GapKind::Untraced => (
            format!(
                "「{}」在材料和知识库里都找不到出处，怎么处理？（原句：{context}）",
                gap.hint
            ),
            vec![
                Choice {
                    label: "改为待核实".into(),
                    detail: "先占位，核实后再填".into(),
                    recommended: true,
                    action: Action::MarkPending,
                },
                Choice {
                    label: "保留原文".into(),
                    detail: "我确认这个内容无误".into(),
                    recommended: false,
                    action: Action::KeepOriginal,
                },
            ],
            Some("改成……".to_string()),
        ),
        GapKind::NeedsUser | GapKind::Retrievable => {
            let why = if gap.status == GapStatus::NoAnswer {
                "知识库里没找到"
            } else {
                "只有你知道"
            };
            (
                format!("「{}」是什么？（{why}；原句：{context}）", gap.hint),
                Vec::new(),
                Some(format!("填写{}", gap.hint)),
            )
        }
    };
    Question {
        id,
        text,
        choices,
        custom_hint,
        prefill: String::new(),
        skippable: true,
        target: Target::Gap(gap.id),
    }
}

/// 第一稿之后要问的题，最多 `max` 道。
pub(crate) fn gap_questions(ledger: &Ledger, max: usize) -> Vec<Question> {
    ledger
        .needs_user()
        .into_iter()
        .filter_map(|id| ledger.get(id))
        .take(max)
        .enumerate()
        .map(|(index, gap)| gap_question(index + 1, gap))
        .collect()
}

/// 动笔前的回答 → (要切换到的文种, 补充给起草的信息)。
pub(crate) fn resolve_predraft(
    questions: &[Question],
    replies: &[(usize, Reply)],
) -> (Option<TemplateKind>, Vec<String>) {
    let mut kind = None;
    let mut notes = Vec::new();
    for (question_id, reply) in replies {
        let Some(question) = questions.iter().find(|q| q.id == *question_id) else {
            continue;
        };
        match reply {
            Reply::Choice(index) => match question.choices.get(*index).map(|c| &c.action) {
                Some(Action::SwitchKind(target)) => kind = Some(*target),
                Some(Action::Note(note)) => notes.push(note.clone()),
                _ => {}
            },
            Reply::Custom(text) if !text.trim().is_empty() => {
                notes.push(format!("{}{}", question.text, text.trim()));
            }
            _ => {}
        }
    }
    (kind, notes)
}

/// 把缺口题的回答落到工作稿上，返回改后的正文。台账里对应缺口的状态一并更新。
///
/// 全是确定性替换：占位换成答案；来源不明的事实换成答案、换成占位或原样保留。
/// 一个都不交给模型——用户亲口给的值，原样写进去最可靠。
pub(crate) fn apply_gap_replies(
    markdown: &str,
    ledger: &mut Ledger,
    questions: &[Question],
    replies: &[(usize, Reply)],
) -> String {
    let mut text = markdown.to_string();
    for (question_id, reply) in replies {
        let Some(question) = questions.iter().find(|q| q.id == *question_id) else {
            continue;
        };
        let Target::Gap(gap_id) = question.target else {
            continue;
        };
        let Some(gap) = ledger.get_mut(gap_id) else {
            continue;
        };
        match reply {
            Reply::Custom(value) if !value.trim().is_empty() => {
                let value = value.trim();
                text = text.replace(&gap.literal, value);
                gap.status = GapStatus::Answered(value.to_string());
            }
            Reply::Choice(index) => match question.choices.get(*index).map(|c| &c.action) {
                Some(Action::MarkPending) => {
                    let placeholder = format!("【待核实：{}】", gap.hint);
                    text = text.replace(&gap.literal, &placeholder);
                    gap.status = GapStatus::Skipped;
                }
                Some(Action::KeepOriginal) => gap.status = GapStatus::Kept,
                _ => {}
            },
            Reply::Skip | Reply::Custom(_) => gap.status = GapStatus::Skipped,
        }
    }
    text
}

/// 去掉行首的「1.」「2、」「3)」编号；「2026年……」这种以数字开头的正文不动。
pub(crate) fn strip_numbering(line: &str) -> &str {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return line;
    }
    match line[digits..].chars().next() {
        Some(mark @ ('.' | '、' | ')' | '）' | '．')) => {
            line[digits + mark.len_utf8()..].trim_start()
        }
        _ => line,
    }
}

/// 取 `needle` 前后各 `side` 个字作上下文：长句里要看到的是缺口本身，不是句首。
fn around(text: &str, needle: &str, side: usize) -> String {
    let Some(pos) = text.find(needle) else {
        return text.chars().take(side * 2).collect();
    };
    let before: Vec<char> = text[..pos].chars().collect();
    let after: Vec<char> = text[pos + needle.len()..].chars().collect();
    let head: String = before[before.len().saturating_sub(side)..].iter().collect();
    let tail: String = after[..after.len().min(side)].iter().collect();
    format!(
        "{}{head}{needle}{tail}{}",
        if before.len() > side { "…" } else { "" },
        if after.len() > side { "…" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_questions_pass_the_gate_or_are_dropped() {
        let reply = "1. 受文对象是谁｜各区县政府｜市直各部门｜两者都发\n\
                     篇幅多长？｜500 字以内｜1000 字左右\n\
                     这是一道特别特别特别特别特别特别特别特别特别特别特别特别特别特别特别特别特别特别长的问题吗｜是｜否\n\
                     只有一个选项｜甲\n\
                     选项重复｜甲｜甲\n";
        let parsed = parse_model_questions(reply, 3);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "受文对象是谁？");
        assert_eq!(parsed[0].1, ["各区县政府", "市直各部门", "两者都发"]);
        assert_eq!(parsed[1].0, "篇幅多长？");
        assert!(parse_model_questions("无", 3).is_empty());
        // 抄了格式示例的不算题。
        assert!(parse_model_questions("问题｜选项1｜选项2｜选项3", 3).is_empty());
        assert!(parse_model_questions("受文对象？｜选项1｜选项2", 3).is_empty());
    }

    #[test]
    fn kind_mismatch_reads_the_longest_word_first() {
        assert_eq!(
            kind_mismatch("写一份调研报告", TemplateKind::OfficialLetter),
            Some(TemplateKind::ResearchReport)
        );
        assert_eq!(
            kind_mismatch("起草一份通知", TemplateKind::OfficialLetter),
            Some(TemplateKind::PlainDocument)
        );
        // 电话通知也算通知，不打扰。
        assert_eq!(
            kind_mismatch("起草一份通知", TemplateKind::PhoneNotice),
            None
        );
        assert_eq!(
            kind_mismatch("帮我写一份商洽函", TemplateKind::OfficialLetter),
            None
        );
        assert_eq!(
            kind_mismatch("根据材料起草", TemplateKind::OfficialLetter),
            None
        );
        // 实测踩过：后面的「调研报告」是让各单位交的，要写的是通知。
        assert_eq!(
            kind_mismatch(
                "起草一份通知，部署开展调研，要求各单位按时报送调研报告。",
                TemplateKind::PlainDocument
            ),
            None
        );
    }

    #[test]
    fn a_model_question_about_the_kind_is_dropped_when_the_program_already_asked() {
        let model = vec![
            (
                "文种与行文方向是否按通知（下行）处理？".to_string(),
                vec!["是".into(), "否".into()],
            ),
            ("篇幅多长？".to_string(), vec!["短".into(), "长".into()]),
        ];
        let questions = predraft_questions(
            "起草一份通知",
            TemplateKind::OfficialLetter,
            model.clone(),
            3,
        );
        let texts: Vec<_> = questions.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[1].contains("篇幅"));
        // 程序没问文种时，模型的文种题照常保留。
        let questions = predraft_questions("起草一份通知", TemplateKind::PlainDocument, model, 3);
        assert_eq!(questions.len(), 2);
    }

    #[test]
    fn predraft_puts_the_kind_question_first_and_caps_the_count() {
        let model = vec![
            ("受文对象是谁？".to_string(), vec!["甲".into(), "乙".into()]),
            ("篇幅多长？".to_string(), vec!["短".into(), "长".into()]),
            ("语气？".to_string(), vec!["严".into(), "缓".into()]),
        ];
        let questions = predraft_questions("起草一份通知", TemplateKind::OfficialLetter, model, 3);
        assert_eq!(questions.len(), 3);
        assert!(questions[0].text.contains("普通公文"));
        assert!(questions[0].choices[0].recommended);
        let (kind, notes) = resolve_predraft(
            &questions,
            &[
                (1, Reply::Choice(0)),
                (2, Reply::Choice(1)),
                (3, Reply::Custom("各乡镇".into())),
            ],
        );
        assert_eq!(kind, Some(TemplateKind::PlainDocument));
        assert_eq!(notes, ["受文对象是谁？乙", "篇幅多长？各乡镇"]);
    }

    #[test]
    fn the_context_shows_the_gap_not_just_the_start_of_a_long_sentence() {
        let sentence = "据相关研究，美军梅文项目自启动以来，系统已在美国军方各分支中运行，截至2024年已汇集179条数据流。";
        let context = around(sentence, "2024年", 6);
        assert_eq!(context, "…中运行，截至2024年已汇集179…");
        assert_eq!(around("短句2024年。", "2024年", 6), "短句2024年。");
    }

    #[test]
    fn gap_replies_are_applied_deterministically() {
        let mut ledger = Ledger::default();
        let text = "请于【待核实：排查完成时限】前完成。会议定于【待核实：会议地点】召开。依据《防火办法》执行。";
        ledger.sync(text, "", &[]);
        // 第三个是来源不明的文件名；检索过、找不到出处才出题。
        assert_eq!(ledger.gaps[2].kind, GapKind::Untraced);
        assert_eq!(gap_questions(&ledger, 4).len(), 2, "来源不明的先去查");
        ledger.gaps[2].status = GapStatus::NoAnswer;
        let questions = gap_questions(&ledger, 4);
        assert_eq!(questions.len(), 3);
        assert!(questions[0].text.contains("只有你知道"));
        assert_eq!(questions[2].choices[0].action, Action::MarkPending);

        let out = apply_gap_replies(
            text,
            &mut ledger,
            &questions,
            &[
                (1, Reply::Custom("12月1日".into())),
                (2, Reply::Skip),
                (3, Reply::Choice(0)),
            ],
        );
        assert_eq!(
            out,
            "请于12月1日前完成。会议定于【待核实：会议地点】召开。依据【待核实：《防火办法》】执行。"
        );
        assert_eq!(ledger.gaps[0].status, GapStatus::Answered("12月1日".into()));
        assert_eq!(ledger.gaps[1].status, GapStatus::Skipped);
        assert_eq!(ledger.gaps[2].status, GapStatus::Skipped);
        // 处理过的不再出题。
        assert!(gap_questions(&ledger, 4).is_empty());
    }
}
