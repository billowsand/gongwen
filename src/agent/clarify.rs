//! 选择题：拿不准的地方不让模型猜，变成选择题交给用户（`docs/ai-agent-workbench.md` 14.11）。
//!
//! 两个时机：
//! - 动笔前：只问会让整篇写偏的事（文种、受文对象、目的、篇幅），最多 3 题；
//! - 第一稿出来后：需要用户提供的、知识库里找不到的、来源不明的，一批最多 4 题。
//!
//! 选项尽量由程序给：文种来自固定列表，事实类只给「自己填写 / 另行通知 / 删去这项 / 保留待核实」，
//! 只有措辞、方向这类没有标准答案的才用模型出的选项。缺口题的回答怎么落到工作稿见
//! `apply_gap_replies`：这里是确定性的兜底，界面上先交模型把答案写进所在段落、过闸门。

use super::gaps::{Gap, GapKind, GapStatus, Ledger, find_placeholders};
use crate::models::{TemplateKind, VocabularyCategory, VocabularyEntry};

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
    /// 缺口填成这个值（「另行通知」这类程序给的写法）。
    Fill(String),
    /// 删去这一项：连同只为它服务的说法一起去掉。
    Drop,
    /// AI 建议的写法：点了填进自己填写框，用户可以接着改；交上去时按自己填的算。
    Suggest(String),
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
    /// 动笔前的六要素题（`elements.rs`）：答案记作已确认信息，跳过就留固定占位。
    Element(super::elements::Element),
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

impl Target {
    /// 动笔前问的题（方向题与六要素题）：答案作为已确认信息交给起草。
    pub(crate) fn is_predraft(self) -> bool {
        matches!(self, Self::PreDraft | Self::Element(_))
    }
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

/// 缺口提示 → 像人问话的问句：「研究方案反馈时限定到什么时候？」。
fn gap_ask(hint: &str) -> String {
    let has = |words: &[&str]| words.iter().any(|word| hint.contains(word));
    let tail = if has(&["时限", "截止", "期限"]) {
        "定到什么时候"
    } else if has(&["时间", "日期", "几月", "几日"]) {
        "定在什么时候"
    } else if has(&["地点", "会场", "地址"]) {
        "在哪里"
    } else if has(&["电话", "手机", "邮箱"]) {
        "是多少"
    } else if has(&["联系人", "负责人", "人员", "名单"]) {
        "是谁"
    } else if has(&["单位"]) {
        "是哪个单位"
    } else if has(&["金额", "经费", "预算", "资金", "数额"]) || hint.ends_with('数') {
        "是多少"
    } else {
        "怎么写"
    };
    format!("「{hint}」{tail}？")
}

/// 时间类缺口：可以「另行通知」。
fn is_time(hint: &str) -> bool {
    ["时限", "截止", "期限", "时间", "日期"]
        .iter()
        .any(|word| hint.contains(word))
}

/// 缺口在问哪类人或单位：按职能任务 / 个人简介从标准词库里给候选。
fn who_kind(hint: &str) -> Option<VocabularyCategory> {
    let has = |words: &[&str]| words.iter().any(|word| hint.contains(word));
    if has(&["联系人", "负责人", "经办人", "人员"]) {
        Some(VocabularyCategory::Person)
    } else if has(&["单位", "部门", "牵头", "责任", "承办", "主办", "配合"]) {
        Some(VocabularyCategory::Unit)
    } else {
        None
    }
}

/// 标准词库里职能对得上的单位或人员，做成选项（最多 3 个）。选中就把名称写进去。
pub(crate) fn vocabulary_choices(
    vocabulary: &[VocabularyEntry],
    category: VocabularyCategory,
    task: &str,
    with_phone: bool,
) -> Vec<Choice> {
    let clip = |text: &str| -> String {
        let text = text.trim().replace('\n', "；");
        if text.chars().count() > 40 {
            text.chars().take(40).collect::<String>() + "…"
        } else {
            text
        }
    };
    super::tools::vocab::suggest(vocabulary, category, task, 3)
        .into_iter()
        .map(|entry| {
            let name = entry.canonical.trim().to_string();
            let (label, detail, value) = match category {
                VocabularyCategory::Unit => (
                    name.clone(),
                    format!("标准词库 · 职能：{}", clip(&entry.duties)),
                    name,
                ),
                _ => {
                    let unit = super::tools::vocab::unit_of(vocabulary, entry);
                    let role: Vec<&str> = [entry.position.trim(), unit]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect();
                    let value = if with_phone && !entry.phone.trim().is_empty() {
                        format!("{name}，联系电话：{}", entry.phone.trim())
                    } else {
                        name.clone()
                    };
                    (
                        if role.is_empty() {
                            name
                        } else {
                            format!("{name}（{}）", role.join(" · "))
                        },
                        format!("标准词库 · 简介：{}", clip(&entry.profile)),
                        value,
                    )
                }
            };
            Choice {
                label,
                detail,
                recommended: false,
                action: Action::Fill(value),
            }
        })
        .collect()
}

/// 一个缺口对应的选择题。题面只问一件事；所在小节与整句由侧栏按 `Target::Gap` 回查台账显示。
/// 问的是单位或人员时，按标准词库里的职能任务 / 个人简介给候选。
pub(crate) fn gap_question(id: usize, gap: &Gap, vocabulary: &[VocabularyEntry]) -> Question {
    let choice = |label: &str, detail: &str, recommended: bool, action: Action| Choice {
        label: label.into(),
        detail: detail.into(),
        recommended,
        action,
    };
    let (text, choices, custom_hint) = match gap.kind {
        GapKind::Untraced => (
            format!("「{}」在材料和知识库里都找不到出处，怎么处理？", gap.hint),
            vec![
                choice(
                    "改为待核实",
                    "先占位，核实后再填",
                    true,
                    Action::MarkPending,
                ),
                choice(
                    "保留原文",
                    "我确认这个内容无误",
                    false,
                    Action::KeepOriginal,
                ),
                choice(
                    "删去这个说法",
                    "连同只为它服务的话一起去掉",
                    false,
                    Action::Drop,
                ),
            ],
            Some("改成……".to_string()),
        ),
        GapKind::NeedsUser | GapKind::Retrievable => {
            let mut choices = Vec::new();
            if let Some(category) = who_kind(&gap.hint) {
                let task = find_placeholders(&gap.sentence)
                    .iter()
                    .fold(gap.sentence.clone(), |text, p| text.replace(&p.literal, ""));
                let with_phone = gap.hint.contains("联系人") || gap.hint.contains("电话");
                choices.extend(vocabulary_choices(vocabulary, category, &task, with_phone));
            }
            if is_time(&gap.hint) {
                choices.push(choice(
                    "另行通知",
                    "写成「具体时间另行通知」",
                    false,
                    Action::Fill("另行通知".into()),
                ));
            }
            choices.push(choice(
                "删去这项",
                "这项不必写，连同相关说法一起去掉",
                false,
                Action::Drop,
            ));
            (
                gap_ask(&gap.hint),
                choices,
                Some("直接写，大白话也行，AI 会把它写进句子".to_string()),
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

/// 只有起草人知道、AI 不该猜的：金额、数量、联系方式、编号。这类缺口不出建议。
fn secret(hint: &str) -> bool {
    const WORDS: [&str; 11] = [
        "金额", "经费", "预算", "资金", "数额", "人数", "电话", "手机", "邮箱", "编号", "文号",
    ];
    (hint.chars().count() >= 2 && hint.ends_with('数')) || WORDS.iter().any(|w| hint.contains(w))
}

/// 一道题能不能交 AI 出建议写法：缺口题里不是来源不明的事实（那是核对真假，不是选写法）、
/// 不是单位人员（候选来自标准词库）、不是金额电话这类只有起草人知道的。
fn suggestable(question: &Question, ledger: &Ledger) -> Option<(usize, String, String)> {
    let Target::Gap(id) = question.target else {
        return None;
    };
    let gap = ledger.get(id)?;
    if gap.kind == GapKind::Untraced || who_kind(&gap.hint).is_some() || secret(&gap.hint) {
        return None;
    }
    let sentence = super::evidence::strip_citations(&gap.sentence)
        .trim()
        .to_string();
    Some((question.id, gap.hint.clone(), sentence))
}

/// 给起草后的确认题要建议写法的提示词；一道能出建议的都没有就是 `None`。
pub(crate) fn suggestion_prompt(
    questions: &[Question],
    ledger: &Ledger,
    request: &str,
    today: &str,
) -> Option<String> {
    let items: Vec<String> = questions
        .iter()
        .filter_map(|question| suggestable(question, ledger))
        .map(|(id, hint, sentence)| format!("{id}. 「{hint}」　原句：{sentence}"))
        .collect();
    if items.is_empty() {
        return None;
    }
    let request = request.trim();
    let request: String = if request.chars().count() > 600 {
        request.chars().take(600).collect::<String>() + "…"
    } else {
        request.to_string()
    };
    Some(format!(
        "下面是一份公文稿里还没定下来的几处，起草人要逐一确认。为每一处给 2 到 3 个建议写法，\
         供起草人参考挑选、修改。

【背景】
{request}
【今天】{today}

{}

要求：
1. 建议要具体、能直接写进这句话，例如时限写「收到本函后15个工作日内」「{today}起一个月内」，\
方式写「书面函复」「通过电子邮件报送」；
2. 不要编单位、人名、电话、金额；拿不准的这一处写「无」；
3. 每处一行，格式严格如下：题号｜建议1｜建议2｜建议3",
        items.join("\n")
    ))
}

/// 模型的建议 → 过闸门后挂到题上：每条不超过 30 字、不重复、不含占位，每题最多 3 条。
pub(crate) fn add_suggestions(questions: &mut [Question], reply: &str) {
    for line in reply.lines() {
        // 「1｜甲｜乙」，模型也常写成「1. 甲｜乙」。
        let line = line.trim();
        let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let Ok(id) = line[..digits].parse::<usize>() else {
            continue;
        };
        let rest = line[digits..].trim_start_matches(['.', '、', ')', '）', '．', '｜', '|', ' ']);
        let Some(question) = questions.iter_mut().find(|q| q.id == id) else {
            continue;
        };
        let values: Vec<String> = rest.split(['｜', '|']).map(str::to_string).collect();
        question
            .choices
            .extend(suggestion_choices(&question.choices, &values));
    }
}

/// 一组建议写法 → 选项。`existing` 里已有的（程序给的选项）不重复出。
pub(crate) fn suggestion_choices(existing: &[Choice], values: &[String]) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    for value in values {
        let value = value
            .trim()
            .trim_matches(['「', '」', '“', '”', '"', '。'])
            .trim();
        let count = value.chars().count();
        let junk = value.is_empty()
            || count > 30
            || matches!(value, "无" | "建议1" | "建议2" | "建议3")
            || value.contains("待核实");
        let seen = existing.iter().chain(&out).any(|c| c.label == value);
        if junk || seen || out.len() >= 3 {
            continue;
        }
        out.push(Choice {
            label: value.to_string(),
            detail: "AI 建议，点了填进下面的框，可以接着改".into(),
            recommended: false,
            action: Action::Suggest(value.to_string()),
        });
    }
    out
}

/// 第一稿之后要问的题，最多 `max` 道。
pub(crate) fn gap_questions(
    ledger: &Ledger,
    max: usize,
    vocabulary: &[VocabularyEntry],
) -> Vec<Question> {
    ledger
        .needs_user()
        .into_iter()
        .filter_map(|id| ledger.get(id))
        .take(max)
        .enumerate()
        .map(|(index, gap)| gap_question(index + 1, gap, vocabulary))
        .collect()
}

/// 动笔前的回答 → (要切换到的文种, 补充给起草的信息)。`kind` 是出题时的文种，六要素题按它
/// 回查要素的说法。
pub(crate) fn resolve_predraft(
    questions: &[Question],
    replies: &[(usize, Reply)],
    kind: TemplateKind,
) -> (Option<TemplateKind>, Vec<String>) {
    let mut switch = None;
    let mut notes = Vec::new();
    for (question_id, reply) in replies {
        let Some(question) = questions.iter().find(|q| q.id == *question_id) else {
            continue;
        };
        match reply {
            Reply::Choice(index) => match question.choices.get(*index).map(|c| &c.action) {
                Some(Action::SwitchKind(target)) => switch = Some(*target),
                Some(Action::Note(note)) => notes.push(note.clone()),
                Some(Action::Suggest(value)) => notes.push(match question.target {
                    Target::Element(element) => {
                        super::elements::note_for(kind, element, Some(value))
                    }
                    _ => format!("{}{value}", question.text),
                }),
                _ => {}
            },
            Reply::Custom(text) if !text.trim().is_empty() => {
                notes.push(match question.target {
                    Target::Element(element) => {
                        super::elements::note_for(kind, element, Some(text.trim()))
                    }
                    _ => format!("{}{}", question.text, text.trim()),
                });
            }
            Reply::Skip | Reply::Custom(_) => {
                if let Target::Element(element) = question.target {
                    notes.push(super::elements::note_for(kind, element, None));
                }
            }
        }
    }
    (switch, notes)
}

/// 把缺口题的回答落到工作稿上，返回改后的正文。台账里对应缺口的状态一并更新。
///
/// 全是确定性改法：占位换成答案；删去就去掉所在句；来源不明的事实换成占位或原样保留。
/// 这是兜底——界面上先交模型把答案写进所在段落（`gap_revise`），过不了闸门的才这样落。
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
        match gap_edit(question, reply) {
            GapEdit::Fill(value) => {
                text = text.replace(&gap.literal, &value);
                gap.status = GapStatus::Answered(value);
            }
            GapEdit::Drop => {
                text = drop_sentence(&text, &gap.literal);
                gap.status = GapStatus::Dropped;
            }
            GapEdit::MarkPending => {
                let placeholder = format!("【待核实：{}】", gap.hint);
                text = text.replace(&gap.literal, &placeholder);
                gap.status = GapStatus::Skipped;
            }
            GapEdit::Keep => gap.status = GapStatus::Kept,
            GapEdit::Skip => gap.status = GapStatus::Skipped,
        }
    }
    text
}

/// 一道缺口题的回答要对正文做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GapEdit {
    Fill(String),
    Drop,
    MarkPending,
    Keep,
    Skip,
}

pub(crate) fn gap_edit(question: &Question, reply: &Reply) -> GapEdit {
    match reply {
        Reply::Custom(value) if !value.trim().is_empty() => GapEdit::Fill(value.trim().into()),
        Reply::Choice(index) => match question.choices.get(*index).map(|c| &c.action) {
            Some(Action::Fill(value) | Action::Suggest(value)) => GapEdit::Fill(value.clone()),
            Some(Action::Drop) => GapEdit::Drop,
            Some(Action::MarkPending) => GapEdit::MarkPending,
            Some(Action::KeepOriginal) => GapEdit::Keep,
            _ => GapEdit::Skip,
        },
        Reply::Skip | Reply::Custom(_) => GapEdit::Skip,
    }
}

/// 去掉 `literal` 所在的那一句；整行只有这一句就连行一起去掉。
pub(crate) fn drop_sentence(text: &str, literal: &str) -> String {
    let Some(pos) = text.find(literal) else {
        return text.to_string();
    };
    let span = super::gaps::sentence_at(text, pos);
    let line_start = text[..span.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[span.end..]
        .find('\n')
        .map_or(text.len(), |i| span.end + i);
    if text[line_start..span.start].trim().is_empty() && text[span.end..line_end].trim().is_empty()
    {
        let end = (line_end + 1).min(text.len());
        return format!("{}{}", &text[..line_start], &text[end..]);
    }
    format!("{}{}", &text[..span.start], &text[span.end..])
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
            TemplateKind::OfficialLetter,
        );
        assert_eq!(kind, Some(TemplateKind::PlainDocument));
        assert_eq!(notes, ["受文对象是谁？乙", "篇幅多长？各乡镇"]);
    }

    #[test]
    fn gap_replies_are_applied_deterministically() {
        let mut ledger = Ledger::default();
        let text = "请于【待核实：排查完成时限】前完成。会议定于【待核实：会议地点】召开。\
                    依据《防火办法》执行。\n\n联系人：【待核实：联系人】。\n";
        ledger.sync(text, "", &[]);
        // 第四个是来源不明的文件名；检索过、找不到出处才出题。
        assert_eq!(ledger.gaps[3].kind, GapKind::Untraced);
        assert_eq!(gap_questions(&ledger, 4, &[]).len(), 3, "来源不明的先去查");
        ledger.gaps[3].status = GapStatus::NoAnswer;
        let questions = gap_questions(&ledger, 4, &[]);
        let texts: Vec<_> = questions.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(
            texts[..3],
            [
                "「排查完成时限」定到什么时候？",
                "「会议地点」在哪里？",
                "「联系人」是谁？"
            ]
        );
        assert_eq!(questions[0].choices[0].label, "另行通知");
        assert_eq!(questions[2].choices[0].action, Action::Drop);
        assert_eq!(questions[3].choices[0].action, Action::MarkPending);

        let out = apply_gap_replies(
            text,
            &mut ledger,
            &questions,
            &[
                (1, Reply::Custom("12月1日".into())),
                (2, Reply::Skip),
                (3, Reply::Choice(0)),
                (4, Reply::Choice(0)),
            ],
        );
        assert_eq!(
            out,
            "请于12月1日前完成。会议定于【待核实：会议地点】召开。依据【待核实：《防火办法》】执行。\n\n"
        );
        assert_eq!(ledger.gaps[0].status, GapStatus::Answered("12月1日".into()));
        assert_eq!(ledger.gaps[1].status, GapStatus::Skipped);
        // 删去这项：整行只有这一句，连行去掉。
        assert_eq!(ledger.gaps[2].status, GapStatus::Dropped);
        assert_eq!(ledger.gaps[3].status, GapStatus::Skipped);
        // 处理过的不再出题。
        assert!(gap_questions(&ledger, 4, &[]).is_empty());
    }

    #[test]
    fn dropping_a_sentence_keeps_the_rest_of_the_line() {
        let text = "一、加强巡查。请于【待核实：时限】前报送。其余照旧。\n";
        assert_eq!(
            drop_sentence(text, "【待核实：时限】"),
            "一、加强巡查。其余照旧。\n"
        );
    }

    #[test]
    fn unit_and_person_gaps_offer_matches_from_the_vocabulary() {
        let entry = |category, name: &str, about: &str| VocabularyEntry {
            category,
            canonical: name.into(),
            duties: if category == VocabularyCategory::Unit {
                about.into()
            } else {
                String::new()
            },
            profile: if category == VocabularyCategory::Person {
                about.into()
            } else {
                String::new()
            },
            phone: "12345678".into(),
            position: "科长".into(),
            ..VocabularyEntry::default()
        };
        let vocabulary = [
            entry(
                VocabularyCategory::Unit,
                "市数据局",
                "负责公共数据共享与开放",
            ),
            entry(VocabularyCategory::Unit, "市应急局", "负责应急救援"),
            entry(
                VocabularyCategory::Person,
                "李四",
                "长期从事数据共享平台运维",
            ),
        ];
        let text = "由【待核实：牵头单位】负责数据共享平台建设。联系人：【待核实：联系人】，负责数据共享平台对接。";
        let mut ledger = Ledger::default();
        ledger.sync(text, "", &[]);
        let questions = gap_questions(&ledger, 4, &vocabulary);
        let labels = |q: &Question| {
            q.choices
                .iter()
                .map(|c| c.label.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(&questions[0]), ["市数据局", "删去这项"]);
        assert_eq!(labels(&questions[1]), ["李四（科长）", "删去这项"]);
        assert_eq!(
            questions[1].choices[0].action,
            Action::Fill("李四，联系电话：12345678".into()),
            "联系人带上词库里的电话"
        );
    }

    #[test]
    fn suggestions_are_asked_only_where_ai_may_guess_and_gated() {
        let text = "请于【待核实：反馈时限】前以【待核实：报送方式】报送，经费【待核实：经费数额】万元。\
                    联系人：【待核实：联系人】。";
        let mut ledger = Ledger::default();
        ledger.sync(text, "", &[]);
        let mut questions = gap_questions(&ledger, 4, &[]);
        let prompt = suggestion_prompt(&questions, &ledger, "起草通知", "2026年10月5日").unwrap();
        assert!(prompt.contains("1. 「反馈时限」"), "{prompt}");
        assert!(prompt.contains("2. 「报送方式」"), "{prompt}");
        assert!(!prompt.contains("「经费数额」"), "金额不让 AI 猜");
        assert!(!prompt.contains("「联系人」"), "人员的候选来自标准词库");
        add_suggestions(
            &mut questions,
            "1｜收到本通知后10个工作日内｜另行通知｜【待核实：时限】\n\
             2. 书面形式｜无\n\
             9｜不存在的题",
        );
        let suggested = |q: &Question| {
            q.choices
                .iter()
                .filter(|c| matches!(c.action, Action::Suggest(_)))
                .map(|c| c.label.clone())
                .collect::<Vec<_>>()
        };
        // 「另行通知」程序已经给了，不重复；带占位的丢掉。
        assert_eq!(suggested(&questions[0]), ["收到本通知后10个工作日内"]);
        assert_eq!(suggested(&questions[1]), ["书面形式"]);
        let edit = gap_edit(
            &questions[0],
            &Reply::Choice(questions[0].choices.len() - 1),
        );
        assert_eq!(edit, GapEdit::Fill("收到本通知后10个工作日内".into()));
    }
}
