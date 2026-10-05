//! 情报六要素：动笔之前先把「何时、何地、何人、何事、何因、何法」讲清楚。
//!
//! 写一份标准公文，尤其是发函，时限、对象、联系人这些只有起草人知道的事，应当在动笔前
//! 问清楚、写进第一稿，而不是先写成「【待核实】」再回头补（那样补进去的字前后不接）。
//!
//! 分工：清单与每个文种「必备哪几项」由程序定；模型只逐项判断要求里给没给，给了就原样
//! 摘出那几个字。摘录在要求原文里找不到的，按没给处理——模型说「给了」不算数。
//! 缺的项出成选择题；选项只有程序给的「另行通知」「不写」，具体值一律由起草人填。

use super::clarify::{Action, Choice, Question, Target, suggestion_choices, vocabulary_choices};
use crate::models::{TemplateKind, VocabularyCategory, VocabularyEntry};

/// 六要素。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Element {
    What,
    Why,
    Who,
    When,
    Where,
    How,
}

impl Element {
    /// 出题顺序：先事由，再对象与时间地点，最后办法。
    pub(crate) const ALL: [Self; 6] = [
        Self::What,
        Self::Why,
        Self::Who,
        Self::When,
        Self::Where,
        Self::How,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::What => "何事",
            Self::Why => "何因",
            Self::Who => "何人",
            Self::When => "何时",
            Self::Where => "何地",
            Self::How => "何法",
        }
    }

    fn from_label(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|element| text.contains(element.label()))
    }

    /// 自己填写框里的示例。
    fn example(self) -> &'static str {
        match self {
            Self::What => "一两句话说清要办的事",
            Self::Why => "如：根据《……》要求；为做好……工作",
            Self::Who => "如：各区县政府；联系人张三，电话……",
            Self::When => "如：10月31日前；收到本函后10个工作日内",
            Self::Where => "如：市政府3号楼301会议室",
            Self::How => "如：请于……前书面函复；请指定专人对接",
        }
    }
}

/// 一个要素在某个文种里指什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Scope {
    pub(crate) element: Element,
    /// 给模型和起草人看的说明。
    pub(crate) meaning: &'static str,
    /// 短名：起草人选「先不定」时，正文占位写成「【待核实：短名】」，事后不再问第二遍。
    pub(crate) short: &'static str,
    /// 必备：模型没交代清楚就要问。非必备的只在模型明说「缺」时才问。
    pub(crate) required: bool,
}

const fn scope(
    element: Element,
    meaning: &'static str,
    short: &'static str,
    required: bool,
) -> Scope {
    Scope {
        element,
        meaning,
        short,
        required,
    }
}

/// 文种的要素清单。研究报告不走这一套（它要讲清的是研究问题与范围）。
pub(crate) fn checklist(kind: TemplateKind) -> Vec<Scope> {
    use Element::*;
    match kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => vec![
            scope(What, "商洽、请求、告知或答复的具体事项", "函告事项", true),
            scope(
                Why,
                "发函缘由与依据（文件、会议、工作需要）",
                "发函依据",
                true,
            ),
            scope(
                Who,
                "致函对象，以及我方联系人和联系方式",
                "联系人及电话",
                true,
            ),
            scope(When, "请对方办理或回复的时限", "回复时限", true),
            scope(Where, "涉及的地点（没有可不写）", "地点", false),
            scope(How, "请对方怎么办：办理要求、回复方式", "办理要求", true),
        ],
        TemplateKind::PlainDocument => vec![
            scope(What, "部署、请示或报告的具体事项", "主要事项", true),
            scope(Why, "依据与目的", "依据与目的", true),
            scope(Who, "受文对象与责任单位", "责任单位", true),
            scope(When, "完成时限或时间安排", "完成时限", true),
            scope(Where, "涉及的地点（没有可不写）", "地点", false),
            scope(How, "工作要求与措施、报送方式", "工作要求", false),
        ],
        TemplateKind::MeetingAgenda => vec![
            scope(What, "会议议题", "会议议题", true),
            scope(Why, "会议目的（没有可不写）", "会议目的", false),
            scope(Who, "主持人、参会人员", "参会人员", true),
            scope(When, "会议时间", "会议时间", true),
            scope(Where, "会议地点", "会议地点", true),
            scope(How, "议程安排", "议程安排", false),
        ],
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => vec![
            scope(What, "请示或报批的事项", "报批事项", true),
            scope(Why, "缘由与依据", "缘由与依据", true),
            scope(Who, "呈报对象与经办单位", "经办单位", true),
            scope(When, "需要批复的时限（没有可不写）", "批复时限", false),
            scope(Where, "涉及的地点（没有可不写）", "地点", false),
            scope(How, "拟办意见或建议方案", "拟办意见", true),
        ],
        TemplateKind::ResearchReport => Vec::new(),
    }
}

/// 技能里没写「要素检查」这段提示词时用这一份。
pub(crate) const PROMPT: &str =
    "下面是一份{kind}的写作要求。逐项检查起草所需的六要素是否已经讲清楚。

{checklist}

每个要素输出一行，格式严格如下（全角竖线分隔）：
要素｜已给｜从写作要求里原样摘出的那几个字
要素｜缺｜向起草人提的问题（不超过30字）｜建议写法1｜建议写法2｜建议写法3
本次用不上的要素写：要素｜不适用

只判断给没给；拿不准算不算给了，就写「缺」。
缺的要素附 2 到 3 个建议写法，供起草人参考挑选、修改：要能直接写进公文，时间可按今天（{today}）推算，
如「收到本函后15个工作日内」；单位、人名、电话、金额不要编，这类不附建议。

【写作要求】
{request}";

/// 清单 → 提示词里的「【何时】回复时限（必备）」几行。
pub(crate) fn checklist_text(list: &[Scope]) -> String {
    list.iter()
        .map(|scope| {
            format!(
                "【{}】{}{}",
                scope.element.label(),
                scope.meaning,
                if scope.required { "（必备）" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 模型对一项要素的判断。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Given(String),
    /// (问句, 建议写法)
    Missing(String, Vec<String>),
    NotApplicable,
}

fn parse(reply: &str) -> Vec<(Element, Verdict)> {
    let mut out = Vec::new();
    for line in reply.lines() {
        let line = super::clarify::strip_numbering(line.trim());
        let parts: Vec<&str> = line.split(['｜', '|']).map(str::trim).collect();
        let (Some(head), Some(status)) = (parts.first(), parts.get(1)) else {
            continue;
        };
        let Some(element) = Element::from_label(head) else {
            continue;
        };
        if out.iter().any(|(seen, _)| *seen == element) {
            continue;
        }
        let rest = parts
            .get(2..)
            .map(|rest| rest.join("｜"))
            .unwrap_or_default();
        let verdict = if status.contains("不适用") {
            Verdict::NotApplicable
        } else if status.contains("已给") {
            Verdict::Given(rest)
        } else if status.contains('缺') {
            let question = parts.get(2).map_or("", |q| *q).to_string();
            let suggestions = parts
                .get(3..)
                .unwrap_or_default()
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            Verdict::Missing(question, suggestions)
        } else {
            continue;
        };
        out.push((element, verdict));
    }
    out
}

/// 去掉空白与常见标点再比：模型摘录时常丢个逗号、换个引号。
fn squash(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !"，。、；：,.;:“”\"'‘’「」".contains(*ch))
        .collect()
}

/// 问题最多这么多字；长了多半是模型在解释。
const MAX_QUESTION_CHARS: usize = 30;

/// 按模型的判断出题：缺的、摘录对不上原文的必备项。题号从 `first_id` 起，最多 `max` 道，
/// 必备的排在前面。模型的回复一行都解析不出来时不出题——宁可少问，不要按坏输出乱问。
///
/// 「何人」问的是单位时（致函对象、责任单位、经办单位），按标准词库的职能任务给候选。
pub(crate) fn questions(
    kind: TemplateKind,
    request: &str,
    reply: &str,
    (first_id, max): (usize, usize),
    vocabulary: &[VocabularyEntry],
) -> Vec<Question> {
    let parsed = parse(reply);
    if parsed.is_empty() || max == 0 {
        return Vec::new();
    }
    let haystack = squash(request);
    let mut picked: Vec<(Scope, String, Vec<String>)> = Vec::new();
    for scope in checklist(kind) {
        let verdict = parsed
            .iter()
            .find(|(element, _)| *element == scope.element)
            .map(|(_, verdict)| verdict);
        let (question, suggestions) = match verdict {
            // 摘录在原文里找得到才算给了。
            Some(Verdict::Given(excerpt))
                if !squash(excerpt).is_empty() && haystack.contains(&squash(excerpt)) =>
            {
                continue;
            }
            Some(Verdict::NotApplicable) if !scope.required => continue,
            Some(Verdict::Missing(question, suggestions)) => {
                (question.clone(), suggestions.clone())
            }
            // 必备项：没提、说不适用、摘录对不上，都要问。
            _ if scope.required => (String::new(), Vec::new()),
            _ => continue,
        };
        picked.push((scope, question, suggestions));
    }
    picked.sort_by_key(|(scope, _, _)| !scope.required);
    picked
        .into_iter()
        .take(max)
        .enumerate()
        .map(|(index, (scope, question, suggestions))| {
            let mut asked = question_for(first_id + index, scope, &question);
            // 「何人」的候选只从标准词库来，模型建议的单位人名不要。
            if scope.element != Element::Who {
                let extra = suggestion_choices(&asked.choices, &suggestions);
                asked.choices.extend(extra);
            }
            if scope.element == Element::Who
                && let Some(role) = unit_role(kind)
            {
                let picks =
                    vocabulary_choices(vocabulary, VocabularyCategory::Unit, request, false)
                        .into_iter()
                        .filter_map(|mut choice| {
                            let Action::Fill(name) = &choice.action else {
                                return None;
                            };
                            choice.action =
                                Action::Note(format!("{role}：{name}（起草人从标准词库选定）"));
                            Some(choice)
                        });
                asked.choices.splice(0..0, picks);
            }
            asked
        })
        .collect()
}

/// 「何人」里能从标准词库按职能挑的那个角色。会议的参会人员不在此列。
fn unit_role(kind: TemplateKind) -> Option<&'static str> {
    match kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => Some("致函对象"),
        TemplateKind::PlainDocument => Some("责任单位"),
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => Some("经办单位"),
        TemplateKind::MeetingAgenda | TemplateKind::ResearchReport => None,
    }
}

fn question_for(id: usize, scope: Scope, model_question: &str) -> Question {
    let model_question = model_question.trim();
    let usable = !model_question.is_empty()
        && model_question.chars().count() <= MAX_QUESTION_CHARS
        && !model_question.contains("问题");
    let text = if usable {
        model_question.trim_end_matches(['?', '？']).to_string() + "？"
    } else {
        format!("{}是什么？", scope.meaning)
    };
    let note = |label: &str, detail: &str, note: String| Choice {
        label: label.into(),
        detail: detail.into(),
        recommended: false,
        action: Action::Note(note),
    };
    let mut choices = Vec::new();
    if scope.element == Element::When {
        choices.push(note(
            "另行通知",
            "正文写「具体时间另行通知」",
            format!("{}：另行通知（起草人确认）", scope.short),
        ));
    }
    if !scope.required || matches!(scope.element, Element::Where | Element::When) {
        choices.push(note(
            "不写这项",
            "正文不提这一项",
            format!(
                "{}：不写进正文，也不要留待核实占位（起草人确认）",
                scope.short
            ),
        ));
    }
    Question {
        id,
        text: format!("【{}】{text}", scope.element.label()),
        choices,
        custom_hint: Some(scope.element.example().to_string()),
        prefill: String::new(),
        skippable: true,
        target: Target::Element(scope.element),
    }
}

/// 要素题的回答 → 交给起草的已确认信息。`kind` 用来回查短名。
///
/// - 自己填的：原样记下，标明起草人确认（之后它就是事实出处，不算来源不明）；
/// - 跳过：让起草写成「【待核实：短名】」，事后的缺口题不再问第二遍。
pub(crate) fn note_for(kind: TemplateKind, element: Element, custom: Option<&str>) -> String {
    let scope = checklist(kind)
        .into_iter()
        .find(|scope| scope.element == element);
    let (meaning, short) =
        scope.map_or((element.label(), element.label()), |s| (s.meaning, s.short));
    match custom {
        Some(text) => format!("{short}（{meaning}）：{text}（起草人确认）"),
        None => format!(
            "{short}暂未确定：正文写「{}」，不要自己编",
            pending_literal(short)
        ),
    }
}

/// 起草人选了「先不定」的那一项在正文里的占位。
pub(crate) fn pending_literal(short: &str) -> String {
    format!("【待核实：{short}】")
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &str = "给市数据局发函，商请共建公共数据研究平台。根据市政府第12次常务会议要求。\
                           联系人：李四，电话12345678。";

    #[test]
    fn verified_excerpts_count_as_given_and_required_gaps_are_asked() {
        let reply = "何事｜已给｜商请共建公共数据研究平台\n\
                     何因｜已给｜根据市政府第12次常务会议要求\n\
                     何人｜已给｜联系人：李四，电话12345678\n\
                     何时｜缺｜研究方案请对方什么时候前反馈？\n\
                     何地｜不适用\n\
                     何法｜已给｜请贵局书面函复";
        let questions = questions(TemplateKind::OfficialLetter, REQUEST, reply, (1, 4), &[]);
        let texts: Vec<_> = questions.iter().map(|q| q.text.as_str()).collect();
        // 「请贵局书面函复」原文里没有：模型说给了不算数，必备项照问。
        assert_eq!(
            texts,
            [
                "【何时】研究方案请对方什么时候前反馈？",
                "【何法】请对方怎么办：办理要求、回复方式是什么？"
            ]
        );
        assert_eq!(questions[0].target, Target::Element(Element::When));
        assert_eq!(questions[0].choices[0].label, "另行通知");
    }

    #[test]
    fn unparsable_replies_ask_nothing_and_optional_items_need_an_explicit_gap() {
        assert!(questions(TemplateKind::OfficialLetter, REQUEST, "好的", (1, 4), &[]).is_empty());
        // 只提了一项：没提到的必备项照问，没提到的非必备项（何地）不问。
        let questions = questions(
            TemplateKind::OfficialLetter,
            REQUEST,
            "何事｜已给｜商请共建",
            (1, 9),
            &[],
        );
        assert!(
            questions
                .iter()
                .all(|q| q.target != Target::Element(Element::Where))
        );
        assert_eq!(questions.len(), 4);
    }

    #[test]
    fn research_reports_have_no_checklist_and_the_cap_keeps_required_first() {
        assert!(checklist(TemplateKind::ResearchReport).is_empty());
        let reply = "何事｜缺｜办什么\n何地｜缺｜在哪里\n何时｜缺｜什么时候";
        let questions = questions(TemplateKind::MeetingAgenda, "", reply, (3, 2), &[]);
        assert_eq!(questions[0].id, 3);
        assert!(
            questions
                .iter()
                .all(|q| q.target != Target::Element(Element::Why))
        );
    }

    #[test]
    fn the_who_question_offers_units_whose_duties_match_the_request() {
        use crate::models::VocabularyEntry;
        let unit = |name: &str, duties: &str| VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: name.into(),
            duties: duties.into(),
            ..VocabularyEntry::default()
        };
        let vocabulary = [
            unit(
                "市数据局",
                "负责公共数据归集、共享与开放，统筹数据资源平台建设",
            ),
            unit("市应急局", "负责安全生产综合监督管理和应急救援"),
        ];
        let request = "给有关部门发函，商请共建公共数据研究平台，推动数据共享。";
        let reply = "何人｜缺｜发给谁？";
        let questions = questions(
            TemplateKind::OfficialLetter,
            request,
            reply,
            (1, 9),
            &vocabulary,
        );
        let who = questions
            .iter()
            .find(|q| q.target == Target::Element(Element::Who))
            .expect("何人要问");
        assert_eq!(who.choices.len(), 1, "只有职能对得上的");
        assert_eq!(who.choices[0].label, "市数据局");
        assert!(who.choices[0].detail.contains("职能"));
        assert_eq!(
            who.choices[0].action,
            Action::Note("致函对象：市数据局（起草人从标准词库选定）".into())
        );
    }

    #[test]
    fn missing_items_carry_ai_suggestions_except_for_who() {
        let reply = "何时｜缺｜对方什么时候前反馈？｜收到本函后15个工作日内｜另行通知\n\
                     何人｜缺｜发给谁？｜市数据局";
        let questions = questions(TemplateKind::OfficialLetter, "", reply, (1, 9), &[]);
        let when = questions
            .iter()
            .find(|q| q.target == Target::Element(Element::When))
            .unwrap();
        let labels: Vec<_> = when.choices.iter().map(|c| c.label.as_str()).collect();
        // 「另行通知」程序给过了，模型的同名建议不重复。
        assert_eq!(labels, ["另行通知", "不写这项", "收到本函后15个工作日内"]);
        assert_eq!(
            when.choices[2].action,
            Action::Suggest("收到本函后15个工作日内".into())
        );
        let who = questions
            .iter()
            .find(|q| q.target == Target::Element(Element::Who))
            .unwrap();
        assert!(who.choices.is_empty(), "模型建议的单位不要");
    }

    #[test]
    fn skipped_items_become_a_fixed_placeholder() {
        let note = note_for(TemplateKind::OfficialLetter, Element::When, None);
        assert!(note.contains("【待核实：回复时限】"), "{note}");
        let note = note_for(
            TemplateKind::OfficialLetter,
            Element::When,
            Some("10月31日前"),
        );
        assert_eq!(
            note,
            "回复时限（请对方办理或回复的时限）：10月31日前（起草人确认）"
        );
    }
}
