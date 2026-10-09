//! 选择题：拿不准的地方不让模型猜，变成选择题交给用户（`docs/ai-agent-workbench.md` 14.11）。
//!
//! 决策按依赖分层，**一批只问一层，上层定了再出下层的题**：
//! 1. 定文种（[`Target::Kind`]）：文种决定用哪个技能、按哪份六要素清单出题、按哪种版式写，
//!    是其余一切的前提，单独一批先问；答完回到澄清这一步重做（`Flow::SuspendAgain`），
//!    换了文种还要按新文种重新选技能；
//! 2. 动笔前：只问会让整篇写偏的事（行文方向、受文对象、目的、篇幅），最多 3 题，加上按
//!    已定文种的六要素题；同一件事只问一遍（方向题与要素题话题重叠的，留要素题）；
//! 3. 第一稿出来后：需要用户提供的、知识库里找不到的、来源不明的，一批最多 4 题；同一句里
//!    一处选了「删去」，同句其余几处随之删去（[`follow_drops`]），不再各自落地。
//!
//! 上层的答案变了，以它为前提的下层题与回答一律作废重出，不拿旧前提下的回答去写稿。
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
    /// 定文种：其余题的前提，单独一批先问。
    Kind,
    PreDraft,
    /// 动笔前的六要素题（`elements.rs`）：答案记作已确认信息，跳过就留固定占位。
    Element(super::elements::Element),
    /// 动笔前的表单要素题（`element_fields.rs`）：选中的值进黑板的要素建议清单
    /// （第 3 期建议卡采纳才写表单，红线 2），同时记一句已确认信息。
    Field(crate::element_fields::FieldId),
    Gap(usize),
    /// 流程中途的通用选择题（`ask.choice`），答案存进变量后流程接着跑。
    Pick,
    /// 自主步骤里模型提的题（`docs/decision-modules.md` 第五节）：答案记作已确认信息，
    /// 回到这一步重做。
    Agent,
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
    /// 多选题（主送、抄送等多值要素字段）：界面画复选框，回答用 `Reply::Many`。
    /// 旧会话没有这个字段，读回按单选。
    #[serde(default)]
    pub(crate) multi: bool,
    pub(crate) target: Target,
}

impl Target {
    /// 动笔前问的题（定文种、方向题、六要素题与表单要素题）：答案作为已确认信息交给起草。
    pub(crate) fn is_predraft(self) -> bool {
        matches!(
            self,
            Self::Kind | Self::PreDraft | Self::Element(_) | Self::Field(_)
        )
    }
}

/// 用户对一道题的回答。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Reply {
    Choice(usize),
    /// 勾选模式的清单确认：勾中了哪几条（选项下标）。
    Many(Vec<usize>),
    Custom(String),
    Skip,
}

/// 问题不超过这么多字，长了多半是模型在解释而不是在出题。
const MAX_QUESTION_CHARS: usize = 40;

/// 要 JSON 时附在澄清提示词末尾的一句。端点支持 `response_format` 时格式由服务端保证，这句是给
/// 不支持的端点看的；两种回法 [`parse_model_questions`] 都认。
pub(crate) const JSON_HINT: &str = "也可以按 JSON 输出：{\"questions\": [{\"question\": \"…\", \"options\": [\"…\", \"…\"]}]}，\
     不需要问时 questions 给空数组。";

/// 澄清题的 JSON Schema（`response_format` 用）。
pub(crate) fn questions_schema() -> (&'static str, serde_json::Value) {
    (
        "clarify_questions",
        serde_json::json!({
            "type": "object",
            "properties": {"questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": {"type": "string"},
                        "options": {
                            "type": "array",
                            "items": {"type": "string"},
                            "minItems": 2,
                            "maxItems": 4,
                        },
                    },
                    "required": ["question", "options"],
                },
            }},
            "required": ["questions"],
        }),
    )
}

/// JSON 回法（`{"questions": [{"question", "options"}]}`，可以包在 ```json 代码块里）→ 一行一题的
/// 文本回法，好走同一道闸门。不是这种 JSON 返回 None。
fn json_question_lines(reply: &str) -> Option<Vec<String>> {
    let body = reply.trim();
    let body = body
        .strip_prefix("```json")
        .or_else(|| body.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .unwrap_or(body)
        .trim();
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let items = value.get("questions")?.as_array()?;
    Some(
        items
            .iter()
            .map(|item| {
                let question = item.get("question").and_then(|q| q.as_str()).unwrap_or("");
                let options: Vec<String> = item
                    .get("options")
                    .and_then(|o| o.as_array())
                    .into_iter()
                    .flatten()
                    .map(super::board::value_to_text)
                    .collect();
                std::iter::once(question.to_string())
                    .chain(options)
                    .collect::<Vec<_>>()
                    .join("｜")
            })
            .collect(),
    )
}

/// 解析模型出的澄清题：JSON（[`questions_schema`]），或每行「问题｜选项1｜选项2…」，2–4 个选项。
///
/// 闸门：问题过长、选项数不对、选项为空或重复的整题丢弃；模型说「无」就是没有题。
pub(crate) fn parse_model_questions(reply: &str, max: usize) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let lines =
        json_question_lines(reply).unwrap_or_else(|| reply.lines().map(str::to_string).collect());
    for line in &lines {
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
const KIND_WORDS: [(&str, &[TemplateKind]); 14] = [
    ("红头呈批件", &[TemplateKind::RedHeadApproval]),
    ("电话记录单", &[TemplateKind::PhoneRecord]),
    ("电话记录", &[TemplateKind::PhoneRecord]),
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

/// 一段文字点名的文种：取**最先出现**的文种词，同一位置长词优先。点名的是一组文种（「通知」
/// 可以是普通公文或电话通知）且当前文种在其中时算当前文种。只看开头 120 字。
fn named_kind(text: &str, current: TemplateKind) -> Option<TemplateKind> {
    let head: String = text.chars().take(120).collect();
    let (_, _, kinds) = KIND_WORDS
        .iter()
        .filter_map(|(word, kinds)| head.find(word).map(|pos| (pos, word.len(), *kinds)))
        .min_by_key(|(pos, len, _)| (*pos, std::cmp::Reverse(*len)))?;
    Some(if kinds.contains(&current) {
        current
    } else {
        kinds[0]
    })
}

/// 要求里点名的文种与当前文种对不上时，返回建议切换到的文种。
///
/// 取**最先出现**的文种词：「起草一份通知……要求各单位报送调研报告」要写的是通知，
/// 后面的调研报告是让别人交的东西。
pub(crate) fn kind_mismatch(request: &str, current: TemplateKind) -> Option<TemplateKind> {
    named_kind(request, current).filter(|kind| *kind != current)
}

fn switch_choice(kind: TemplateKind, recommended: bool) -> Choice {
    Choice {
        label: format!("切换为{}", kind.label()),
        detail: "技能、要素清单、版式与行文规则都随文种变化，后面的问题按新文种重新出".into(),
        recommended,
        action: Action::SwitchKind(kind),
    }
}

fn keep_choice(current: TemplateKind) -> Choice {
    Choice {
        label: format!("保持{}", current.label()),
        detail: String::new(),
        recommended: false,
        action: Action::KeepKind,
    }
}

/// 定文种题（程序判断）：要求里点名的文种与当前文种对不上时出这一题。它是其余题的前提，
/// 必须单独一批先问（[`Target::Kind`]）。
pub(crate) fn kind_question(request: &str, current: TemplateKind) -> Option<Question> {
    let kind = kind_mismatch(request, current)?;
    Some(Question {
        id: 1,
        text: format!(
            "要求里写的是「{}」，当前文种是「{}」，按哪个写？",
            kind.label(),
            current.label()
        ),
        choices: vec![switch_choice(kind, true), keep_choice(current)],
        custom_hint: None,
        prefill: String::new(),
        skippable: false,
        multi: false,
        target: Target::Kind,
    })
}

/// 模型出的题里问文种的：文种是上层决策，不能和以它为前提的题混在一批里当普通备注记下
/// （选了「通知」却照旧按函出要素题、按函写）。选项能对上文种的，改成定文种题单独先问；
/// 对不上的（「是 / 否」）返回 None，由 [`predraft_questions`] 丢掉。
pub(crate) fn model_kind_question(
    model_questions: &[(String, Vec<String>)],
    current: TemplateKind,
) -> Option<Question> {
    model_questions.iter().find_map(|(question, options)| {
        if !question.contains("文种") {
            return None;
        }
        let mut kinds: Vec<TemplateKind> = Vec::new();
        for kind in options.iter().filter_map(|o| named_kind(o, current)) {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        if !kinds.iter().any(|kind| *kind != current) {
            return None;
        }
        let mut choices: Vec<Choice> = kinds
            .iter()
            .filter(|kind| **kind != current)
            .map(|kind| switch_choice(*kind, false))
            .collect();
        choices.push(keep_choice(current));
        Some(Question {
            id: 1,
            text: question.clone(),
            choices,
            custom_hint: None,
            prefill: String::new(),
            skippable: false,
            multi: false,
            target: Target::Kind,
        })
    })
}

/// 授权类要素的说法（红线 2）：AI 不写、不给建议，也不就它们出题——这些由起草人在要素区
/// 填、由签发流程定，不是问一句就能定的。
const AUTHORIZED_WORDS: [&str; 8] = [
    "密级",
    "份号",
    "签发人",
    "成文日期",
    "印发机关",
    "印发日期",
    "发文字号",
    "发文机关",
];

/// 题目或选项碰到授权类要素没有。「文号」单独看：问引用的《某文件》的文号是核对依据，
/// 不算；没有书名号时问的就是本文的文号。
fn touches_authorized(text: &str) -> bool {
    AUTHORIZED_WORDS.iter().any(|word| text.contains(word))
        || (text.contains("文号") && !text.contains('《'))
}

/// 自主步骤一批最多问几题。
pub(crate) const AGENT_MAX_QUESTIONS: usize = 3;

/// 自主步骤里模型要问用户的题 → 选择题。过闸门（与澄清题同一套：问题不长、选项 2–4 个不重复、
/// 没照抄格式示例；另外允许不给选项，只让人填），碰到授权类要素、问文种的整批打回，
/// 返回给模型看的原因。
pub(crate) fn agent_questions(raw: &[(String, Vec<String>)]) -> Result<Vec<Question>, String> {
    if raw.is_empty() {
        return Err("没有题目：questions 里至少写一道".into());
    }
    if raw.len() > AGENT_MAX_QUESTIONS {
        return Err(format!(
            "一次最多问 {AGENT_MAX_QUESTIONS} 道，只问最要紧的、只有用户知道的"
        ));
    }
    let mut out = Vec::new();
    for (index, (question, options)) in raw.iter().enumerate() {
        let question = question.trim().trim_end_matches(['?', '？']).to_string() + "？";
        let options: Vec<String> = options.iter().map(|o| o.trim().to_string()).collect();
        let all = std::iter::once(question.as_str())
            .chain(options.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        if touches_authorized(&all) {
            return Err(format!(
                "「{question}」问的是密级、文号、签发人、成文日期这类授权要素：它们由起草人在要素区填，\
                 你不能问也不能建议，不要写进稿子"
            ));
        }
        if question.contains("文种") {
            return Err("文种在要素区定，不在这里问；按当前文种做".into());
        }
        let unique = options
            .iter()
            .enumerate()
            .all(|(i, option)| !options[..i].contains(option));
        if question.chars().count() > MAX_QUESTION_CHARS + 1 {
            return Err(format!(
                "「{question}」太长：问题不超过 {MAX_QUESTION_CHARS} 字，解释放到 progress 里"
            ));
        }
        if options.len() == 1
            || options.len() > 4
            || options.iter().any(String::is_empty)
            || !unique
            || echoes_format(&question, &options)
        {
            return Err(format!(
                "「{question}」的选项不对：要么不给选项（让用户自己填），要么给 2 到 4 个不重复的选项"
            ));
        }
        out.push(Question {
            id: index + 1,
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
            custom_hint: Some(if options.is_empty() {
                "直接写".into()
            } else {
                "其他，自己写".into()
            }),
            prefill: String::new(),
            skippable: true,
            multi: false,
            target: Target::Agent,
        });
    }
    Ok(out)
}

/// 自主步骤提的题的回答 → 已确认信息。跳过的也记一句，免得模型重跑时再问同一件事。
pub(crate) fn resolve_agent(questions: &[Question], replies: &[(usize, Reply)]) -> Vec<String> {
    let mut notes = Vec::new();
    for question in questions.iter().filter(|q| q.target == Target::Agent) {
        let reply = replies
            .iter()
            .find(|(id, _)| *id == question.id)
            .map_or(&Reply::Skip, |(_, reply)| reply);
        let note = match reply {
            Reply::Choice(index) => match question.choices.get(*index).map(|c| &c.action) {
                Some(Action::Note(note)) => Some(note.clone()),
                _ => None,
            },
            Reply::Custom(text) if !text.trim().is_empty() => {
                Some(format!("{}{}", question.text, text.trim()))
            }
            _ => None,
        };
        notes.push(note.unwrap_or_else(|| {
            format!(
                "{}（起草人没有回答，按现有信息处理，不要再问）",
                question.text
            )
        }));
    }
    notes
}

/// 动笔前的方向题：模型出的澄清题，题号从 1 起，最多 `max` 道。问文种的一律不要——
/// 走到这里文种已经定了（[`kind_question`] / [`model_kind_question`]）。
pub(crate) fn predraft_questions(
    model_questions: Vec<(String, Vec<String>)>,
    max: usize,
) -> Vec<Question> {
    let mut out = Vec::new();
    for (question, options) in model_questions {
        if out.len() >= max {
            break;
        }
        if question.contains("文种") {
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
            multi: false,
            target: Target::PreDraft,
        });
    }
    out
}

/// 同一批里同一件事只问一遍：方向题与已出的六要素题话题重叠的（模型问「受文对象是谁」，
/// 要素题也问「何人」），丢掉方向题——要素题的选项来自标准词库，答案按要素记。
/// 出了表单要素题（主送、承办、联系人、呈报领导）时「何人」已由字段题覆盖，同样去掉
/// 与何人重叠的方向题。题号重新从 1 排起。
pub(crate) fn merge_predraft(direction: Vec<Question>, elements: Vec<Question>) -> Vec<Question> {
    let mut asked: Vec<super::elements::Element> = elements
        .iter()
        .filter_map(|q| match q.target {
            Target::Element(element) => Some(element),
            _ => None,
        })
        .collect();
    use crate::element_fields::FieldId;
    if elements.iter().any(|q| {
        matches!(
            q.target,
            Target::Field(
                FieldId::Recipient
                    | FieldId::ResponsibleUnit
                    | FieldId::ContactPerson
                    | FieldId::ReportingLeaders
            )
        )
    }) {
        asked.push(super::elements::Element::Who);
    }
    direction
        .into_iter()
        .filter(|q| !asked.iter().any(|element| element.covers(&q.text)))
        .chain(elements)
        .enumerate()
        .map(|(index, mut question)| {
            question.id = index + 1;
            question
        })
        .collect()
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
        // 事实冲突：材料里同一处写的是别的值。候选是程序按字面找出来的，带出处摘录；只有一个
        // 候选时推荐它（材料是起草人给的），几个候选说不准哪个对，仍推荐先占位。
        GapKind::Untraced if !gap.candidates.is_empty() => {
            let single = gap.candidates.len() == 1;
            let sources: Vec<&str> = gap.candidates.iter().map(|c| c.source.as_str()).collect();
            let mut choices: Vec<Choice> = gap
                .candidates
                .iter()
                .map(|candidate| {
                    choice(
                        &format!("按{}：{}", candidate.source, candidate.value),
                        &candidate.excerpt,
                        single,
                        Action::Fill(candidate.value.clone()),
                    )
                })
                .collect();
            choices.extend([
                choice(
                    "改为待核实",
                    "先占位，核实后再填",
                    !single,
                    Action::MarkPending,
                ),
                choice(
                    "保留原文",
                    "我确认稿里这个值无误",
                    false,
                    Action::KeepOriginal,
                ),
            ]);
            (
                format!(
                    "「{}」与{}里的说法对不上，按哪个写？",
                    gap.hint,
                    dedup_join(&sources)
                ),
                choices,
                Some("改成……".to_string()),
            )
        }
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
        multi: false,
        target: Target::Gap(gap.id),
    }
}

/// 「材料、《甲》」：去重后用顿号连起来。
fn dedup_join(items: &[&str]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for item in items {
        if !seen.contains(item) {
            seen.push(item);
        }
    }
    seen.join("、")
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
        // 表单要素题的回答由 `element_fields::resolve_answers` 落地（建议清单 + 已确认信息）。
        if matches!(question.target, Target::Field(_)) {
            continue;
        }
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
            // 勾选回答只出现在清单确认里，这里按没答算。
            Reply::Skip | Reply::Many(_) | Reply::Custom(_) => {
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
    let replies = follow_drops(questions, replies, ledger);
    for (question_id, reply) in &replies {
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
        Reply::Skip | Reply::Many(_) | Reply::Custom(_) => GapEdit::Skip,
    }
}

/// 同一批缺口题里的依赖：一处选了「删去」，所在那句整句去掉，同一句里的其余几处跟着
/// 没了落点——再按它们各自的回答去填、去改，就是在给已删的句子写字（模型按段改写时还会
/// 收到「这句删掉」与「这句填上」两条打架的指令）。这里把它们的回答统一改成「删去」，
/// 两条落地路径（确定性兜底、按段交模型）都先过这一道。
pub(crate) fn follow_drops(
    questions: &[Question],
    replies: &[(usize, Reply)],
    ledger: &Ledger,
) -> Vec<(usize, Reply)> {
    let sentence_of = |question_id: usize| {
        let question = questions.iter().find(|q| q.id == question_id)?;
        let Target::Gap(gap_id) = question.target else {
            return None;
        };
        Some((question, ledger.get(gap_id)?.sentence.trim().to_string()))
    };
    let dropped: Vec<String> = replies
        .iter()
        .filter_map(|(id, reply)| {
            let (question, sentence) = sentence_of(*id)?;
            (gap_edit(question, reply) == GapEdit::Drop && !sentence.is_empty()).then_some(sentence)
        })
        .collect();
    replies
        .iter()
        .map(|(id, reply)| {
            let follow = sentence_of(*id).and_then(|(question, sentence)| {
                let drop = question
                    .choices
                    .iter()
                    .position(|c| c.action == Action::Drop)?;
                (dropped.contains(&sentence) && gap_edit(question, reply) != GapEdit::Drop)
                    .then_some(Reply::Choice(drop))
            });
            (*id, follow.unwrap_or_else(|| reply.clone()))
        })
        .collect()
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
    fn json_questions_go_through_the_same_gate() {
        let reply = r#"{"questions": [
            {"question": "受文对象是谁", "options": ["各区县政府", "市直各部门"]},
            {"question": "只有一个选项", "options": ["甲"]},
            {"question": "篇幅多长？", "options": ["500 字以内", "1000 字左右"]}
        ]}"#;
        let parsed = parse_model_questions(reply, 3);
        assert_eq!(parsed.len(), 2, "选项数不对的照样丢");
        assert_eq!(parsed[0].0, "受文对象是谁？");
        assert_eq!(parsed[0].1, ["各区县政府", "市直各部门"]);
        let fenced = "```json
{\"questions\": [{\"question\": \"篇幅？\", \"options\": [\"短\", \"长\"]}]}
```";
        assert_eq!(
            parse_model_questions(fenced, 3).len(),
            1,
            "包在代码块里也认"
        );
        assert!(parse_model_questions(r#"{"questions": []}"#, 3).is_empty());
        let schema = questions_schema().1;
        assert_eq!(
            schema["properties"]["questions"]["items"]["required"],
            serde_json::json!(["question", "options"])
        );
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
    fn the_kind_question_stands_alone_and_direction_questions_never_ask_the_kind() {
        // 实测踩过：要求写「研究报告」、当前是公函，定文种题和按公函出的要素题挤在一批，
        // 选了研究报告，后面几题却还在问公函的致函对象。定文种题只出它自己。
        let question = kind_question(
            "根据《梅文项目》总结得失，写一个研究报告",
            TemplateKind::OfficialLetter,
        )
        .expect("文种对不上要问");
        assert_eq!(question.target, Target::Kind);
        assert!(!question.skippable);
        assert!(question.target.is_predraft());
        assert_eq!(
            question.choices[0].action,
            Action::SwitchKind(TemplateKind::ResearchReport)
        );
        assert!(question.choices[0].recommended);
        assert_eq!(question.choices[1].action, Action::KeepKind);
        assert!(kind_question("写一个研究报告", TemplateKind::ResearchReport).is_none());

        // 方向题里问文种的一律不要（走到这里文种已经定了）；行文方向是方向题，照留。
        let model = vec![
            (
                "文种与行文方向是否按通知（下行）处理？".to_string(),
                vec!["是".into(), "否".into()],
            ),
            ("行文方向？".to_string(), vec!["上行".into(), "下行".into()]),
            ("篇幅多长？".to_string(), vec!["短".into(), "长".into()]),
            ("语气？".to_string(), vec!["严".into(), "缓".into()]),
        ];
        let questions = predraft_questions(model, 2);
        let texts: Vec<_> = questions.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(texts, ["行文方向？", "篇幅多长？"]);
        assert!(questions.iter().all(|q| q.target == Target::PreDraft));
        let (kind, notes) = resolve_predraft(
            &questions,
            &[(1, Reply::Choice(1)), (2, Reply::Custom("一千字".into()))],
            TemplateKind::OfficialLetter,
        );
        assert_eq!(kind, None);
        assert_eq!(notes, ["行文方向？下行", "篇幅多长？一千字"]);
    }

    #[test]
    fn a_model_question_about_the_kind_becomes_the_kind_question() {
        let model = vec![
            (
                "文种按通知还是按函写？".to_string(),
                vec!["通知".into(), "公函".into()],
            ),
            ("篇幅多长？".to_string(), vec!["短".into(), "长".into()]),
        ];
        let question =
            model_kind_question(&model, TemplateKind::OfficialLetter).expect("选项对得上文种");
        assert_eq!(question.target, Target::Kind);
        let actions: Vec<_> = question.choices.iter().map(|c| c.action.clone()).collect();
        assert_eq!(
            actions,
            [
                Action::SwitchKind(TemplateKind::PlainDocument),
                Action::KeepKind
            ]
        );
        assert!(
            question.choices.iter().all(|c| !c.recommended),
            "模型的判断不替用户推荐"
        );
        // 选项对不上文种（是 / 否），或都是当前文种：不算定文种题。
        let vague = vec![(
            "文种是否按通知处理？".to_string(),
            vec!["是".into(), "否".into()],
        )];
        assert!(model_kind_question(&vague, TemplateKind::OfficialLetter).is_none());
        assert!(model_kind_question(&model, TemplateKind::PlainDocument).is_some());
        let same = vec![(
            "文种？".to_string(),
            vec!["普通通知".into(), "电话通知".into()],
        )];
        assert!(
            model_kind_question(&same, TemplateKind::PhoneNotice).is_none(),
            "通知也算电话通知，只剩当前文种时不问"
        );
    }

    #[test]
    fn the_same_matter_is_asked_once_and_ids_are_renumbered() {
        let direction = predraft_questions(
            vec![
                ("受文对象是谁？".to_string(), vec!["甲".into(), "乙".into()]),
                ("篇幅多长？".to_string(), vec!["短".into(), "长".into()]),
            ],
            3,
        );
        let elements = super::super::elements::questions(
            TemplateKind::OfficialLetter,
            "",
            "何人｜缺｜发给谁？\n何时｜缺｜什么时候前回复？",
            (1, 4),
            &[],
            false,
        );
        let merged = merge_predraft(direction, elements);
        let texts: Vec<_> = merged.iter().map(|q| q.text.as_str()).collect();
        assert!(!texts.contains(&"受文对象是谁？"), "{texts:?}");
        assert_eq!(texts[0], "篇幅多长？");
        let ids: Vec<_> = merged.iter().map(|q| q.id).collect();
        assert_eq!(ids, (1..=merged.len()).collect::<Vec<_>>());
    }

    /// 出了主送题（Target::Field）时，「何人」已被覆盖：与何人重叠的方向题照样去掉。
    #[test]
    fn a_field_question_covers_who_for_direction_dedup() {
        let direction = predraft_questions(
            vec![
                ("受文对象是谁？".to_string(), vec!["甲".into(), "乙".into()]),
                ("篇幅多长？".to_string(), vec!["短".into(), "长".into()]),
            ],
            3,
        );
        let field = Question {
            id: 1,
            text: "主送单位是哪些？（可多选）".into(),
            choices: vec![],
            custom_hint: None,
            prefill: String::new(),
            skippable: true,
            multi: true,
            target: Target::Field(crate::element_fields::FieldId::Recipient),
        };
        let merged = merge_predraft(direction, vec![field]);
        let texts: Vec<_> = merged.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(texts, ["篇幅多长？", "主送单位是哪些？（可多选）"]);
        assert_eq!(merged[1].id, 2);
    }

    #[test]
    fn answers_in_a_dropped_sentence_follow_the_drop() {
        let mut ledger = Ledger::default();
        let text = "请于【待核实：报送时限】前将材料报送【待核实：报送单位】。其余照旧。\n";
        ledger.sync(text, "", &[]);
        let questions = gap_questions(&ledger, 4, &[]);
        assert_eq!(questions.len(), 2);
        let drop = questions[1]
            .choices
            .iter()
            .position(|c| c.action == Action::Drop)
            .unwrap();
        // 第一处填了时限，第二处却把整句删了：时限跟着删，不往已删的句子里填字。
        let replies = [
            (1, Reply::Custom("10月31日".into())),
            (2, Reply::Choice(drop)),
        ];
        let followed = follow_drops(&questions, &replies, &ledger);
        assert_eq!(gap_edit(&questions[0], &followed[0].1), GapEdit::Drop);
        let out = apply_gap_replies(text, &mut ledger, &questions, &replies);
        assert_eq!(out, "其余照旧。\n");
        assert!(ledger.gaps.iter().all(|g| g.status == GapStatus::Dropped));
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
