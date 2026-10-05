//! 按回答修订：用户答完缺口题，交模型把答案写进所在段落，而不是把字硬塞进占位。
//!
//! 「请于【待核实：反馈时限】前反馈」答了「另行通知」，硬替换成「请于另行通知前反馈」就不通；
//! 答了一句大白话，原样塞进去也不像公文。所以按段落交模型改写：答复是确认过的事实，照它的
//! 意思写；为通顺可以调整前后的说法；别处不动。
//!
//! 闸门（确定性，不过就退回直接替换，并在过程里说明）：
//! - 要改的占位都没了，段里别的占位原样还在；
//! - 答复里的日期、数字、文件名原样出现；程序给的写法（「另行通知」）原样出现；
//! - 新出现的事实（日期、数字、单位、人名、文件名）只能来自答复；
//! - 标题层级、表格竖线不变，篇幅不离谱。
//!
//! 产物仍是工作稿，重新定稿成提案，由用户在审阅视图里接受（红线 1）。

use super::backend::{ModelBackend, ModelRole};
use super::clarify::{GapEdit, Question, Reply, Target, drop_sentence, gap_edit};
use super::gaps::{GapKind, GapStatus, Ledger, find_placeholders, section_of, sentence_at};
use super::tools::ASSIST_SYSTEM;
use crate::models::VocabularyEntry;
use std::ops::Range;

/// 整段都该删时模型回这一句。
const WHOLE_PARAGRAPH_DROPPED: &str = "（整段删去）";

const PROMPT: &str = "下面是一份公文里的一段，其中几处要按起草人的答复修改。

【所在小节】{section}
【原段】
{paragraph}

【要改的地方】
{items}

要求：
1. 起草人的答复是确认过的事实，照它的意思写，其中的时间、数字、名称一字不改；答复是大白话的，\
改成公文用语再写进去；
2. 「另行通知」「删去」这类，要顺带调整前后的说法，让句子通顺、不留残句，例如「请于【待核实：\
反馈时限】前反馈」改成「反馈时间另行通知」；
3. 除这几处和为通顺必须调整的字词外，原段其他内容不改，段中其他「【待核实：…】」原样保留；
4. 不得新写答复里没有的时间、数字、单位、人名、文件名；
5. 删去后整段没有内容了，只输出「（整段删去）」；
6. 只输出改后的这一段，不加解释、不加引号。";

/// 一处要交模型改的缺口。
#[derive(Debug, Clone)]
struct Item {
    gap_id: usize,
    literal: String,
    hint: String,
    untraced: bool,
    edit: GapEdit,
}

impl Item {
    fn describe(&self, index: usize) -> String {
        match &self.edit {
            GapEdit::Fill(value) if self.untraced => {
                format!("{index}. 「{}」：起草人要求改成「{value}」", self.literal)
            }
            GapEdit::Fill(value) => {
                format!("{index}. 「{}」：起草人答复「{value}」", self.literal)
            }
            _ => format!(
                "{index}. 「{}」：起草人要求删去这一项，连同只为它服务的说法",
                self.literal
            ),
        }
    }
}

/// 按回答改完的工作稿与台账。
pub(crate) struct Revised {
    pub(crate) raw: String,
    pub(crate) ledger: Ledger,
}

/// 把缺口题的回答落到工作稿：要填的、要删的按段落交模型改写，过闸门才用，不过就直接替换；
/// 改成待核实、保留原文、跳过的不调模型。`emit` 收过程说明（每段一行）。
pub(crate) fn revise(
    model: &dyn ModelBackend,
    raw: &str,
    mut ledger: Ledger,
    questions: &[Question],
    replies: &[(usize, Reply)],
    vocabulary: &[VocabularyEntry],
    emit: &mut dyn FnMut(String),
) -> Revised {
    let mut text = raw.to_string();
    let mut items = Vec::new();
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
            edit @ (GapEdit::Fill(_) | GapEdit::Drop) => items.push(Item {
                gap_id,
                literal: gap.literal.clone(),
                hint: gap.hint.clone(),
                untraced: gap.kind == GapKind::Untraced,
                edit,
            }),
            GapEdit::MarkPending => {
                let placeholder = format!("【待核实：{}】", gap.hint);
                text = text.replace(&gap.literal, &placeholder);
                gap.status = GapStatus::Skipped;
            }
            GapEdit::Keep => gap.status = GapStatus::Kept,
            GapEdit::Skip => gap.status = GapStatus::Skipped,
        }
    }

    // 按所在段落分组：同一段里的几处一起改，改一次就通顺。
    while let Some(first) = items.first().cloned() {
        let Some(range) = text
            .find(&first.literal)
            .map(|pos| paragraph_at(&text, pos))
        else {
            emit(format!("「{}」在正文里找不到了，跳过", first.hint));
            items.remove(0);
            continue;
        };
        let (group, rest): (Vec<Item>, Vec<Item>) = items
            .into_iter()
            .partition(|item| text[range.clone()].contains(&item.literal));
        items = rest;
        let paragraph = text[range.clone()].to_string();
        let section = section_of(&text, range.start);
        let outcome = rewrite(model, &section, &paragraph, &group, vocabulary);
        let replacement = match outcome {
            Ok(revised) => {
                let names: Vec<String> = group
                    .iter()
                    .map(|item| format!("「{}」", item.hint))
                    .collect();
                emit(format!("{}已写进所在段落", names.join("")));
                revised
            }
            Err(why) => {
                emit(format!(
                    "{}：AI 改写没过闸门（{why}），已直接填入",
                    group
                        .iter()
                        .map(|item| format!("「{}」", item.hint))
                        .collect::<String>()
                ));
                fallback(&paragraph, &group)
            }
        };
        text = splice(&text, range, &replacement);
        for item in &group {
            if let Some(gap) = ledger.get_mut(item.gap_id) {
                gap.status = match &item.edit {
                    GapEdit::Fill(value) => GapStatus::Answered(value.clone()),
                    _ => GapStatus::Dropped,
                };
            }
        }
    }

    // 还没处理的缺口，所在句可能随段落改写变了：按字面重新找一遍，侧栏显示与定位要用。
    for gap in &mut ledger.gaps {
        if matches!(gap.status, GapStatus::Open | GapStatus::NoAnswer)
            && let Some(pos) = text.find(&gap.literal)
        {
            gap.sentence = text[sentence_at(&text, pos)].to_string();
        }
    }
    Revised { raw: text, ledger }
}

/// 包含 `pos` 的那一行（不含换行）。
fn paragraph_at(text: &str, pos: usize) -> Range<usize> {
    let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    start..end
}

/// 换掉一段；整段删去时连同后面的一个空行一起去掉。
fn splice(text: &str, range: Range<usize>, replacement: &str) -> String {
    if !replacement.is_empty() {
        return format!(
            "{}{replacement}{}",
            &text[..range.start],
            &text[range.end..]
        );
    }
    let rest = &text[range.end..];
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let rest = if text[..range.start].ends_with("\n\n") {
        rest.strip_prefix('\n').unwrap_or(rest)
    } else {
        rest
    };
    format!("{}{rest}", &text[..range.start])
}

/// 确定性兜底：填的直接替换，删的去掉所在句。
fn fallback(paragraph: &str, group: &[Item]) -> String {
    let mut text = paragraph.to_string();
    for item in group {
        text = match &item.edit {
            GapEdit::Fill(value) => text.replace(&item.literal, value),
            _ => drop_sentence(&text, &item.literal),
        };
    }
    text.trim().to_string()
}

fn rewrite(
    model: &dyn ModelBackend,
    section: &str,
    paragraph: &str,
    group: &[Item],
    vocabulary: &[VocabularyEntry],
) -> Result<String, String> {
    let items = group
        .iter()
        .enumerate()
        .map(|(index, item)| item.describe(index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = PROMPT
        .replace(
            "{section}",
            if section.is_empty() {
                "（文首）"
            } else {
                section
            },
        )
        .replace("{paragraph}", paragraph)
        .replace("{items}", &items);
    let reply = model
        .complete(ModelRole::Draft, ASSIST_SYSTEM, &prompt, &mut |_| {})
        .map_err(|error| format!("模型出错：{error:#}"))?
        .content;
    gate(paragraph, &reply, group, vocabulary)
}

/// 去掉空白再比：「12 月 1 日」与「12月1日」算同一个。
fn squash(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 段落改写的闸门。通过返回改后的段落（整段删去时为空串）。
fn gate(
    paragraph: &str,
    reply: &str,
    group: &[Item],
    vocabulary: &[VocabularyEntry],
) -> Result<String, String> {
    let revised = reply
        .trim()
        .trim_matches(['「', '」', '“', '”', '"'])
        .trim()
        .to_string();
    if revised == WHOLE_PARAGRAPH_DROPPED {
        return if group.iter().all(|item| item.edit == GapEdit::Drop) {
            Ok(String::new())
        } else {
            Err("有要填的内容却把整段删了".into())
        };
    }
    if revised.is_empty() {
        return Err("什么都没写".into());
    }
    if revised.lines().count() > paragraph.lines().count().max(1) {
        return Err("拆成了几段".into());
    }
    if let Some(item) = group.iter().find(|item| revised.contains(&item.literal)) {
        return Err(format!("「{}」还在", item.hint));
    }
    for placeholder in find_placeholders(paragraph) {
        if group.iter().all(|item| item.literal != placeholder.literal)
            && !revised.contains(&placeholder.literal)
        {
            return Err(format!("把别处的「{}」也改掉了", placeholder.literal));
        }
    }
    let heading = |text: &str| text.chars().take_while(|ch| *ch == '#').count();
    if heading(paragraph) != heading(&revised)
        || paragraph.matches('|').count() != revised.matches('|').count()
    {
        return Err("动了标题或表格".into());
    }
    let answers: String = group
        .iter()
        .filter_map(|item| match &item.edit {
            GapEdit::Fill(value) => Some(value.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let squashed = squash(&revised);
    for item in group {
        let GapEdit::Fill(value) = &item.edit else {
            continue;
        };
        let facts = crate::ai_guard::extract_key_facts(value, vocabulary);
        // 程序给的写法（「另行通知」）与答复里的事实都要原样出现；纯大白话的答复交模型转述。
        let must: Vec<String> = if facts.is_empty() && value.chars().count() <= 6 {
            vec![value.clone()]
        } else {
            facts.into_iter().map(|fact| fact.value).collect()
        };
        if let Some(missing) = must.iter().find(|word| !squashed.contains(&squash(word))) {
            return Err(format!("答复里的「{missing}」没写进去"));
        }
    }
    let known = crate::ai_guard::extract_key_facts(paragraph, vocabulary);
    let answer_text = squash(&answers);
    if let Some(fact) = crate::ai_guard::extract_key_facts(&revised, vocabulary)
        .into_iter()
        .find(|fact| !known.contains(fact) && !answer_text.contains(&squash(&fact.value)))
    {
        return Err(format!("新写了答复里没有的「{}」", fact.value));
    }
    let before = paragraph.chars().count();
    let after = revised.chars().count();
    let all_drop = group.iter().all(|item| item.edit == GapEdit::Drop);
    if after > before + answers.chars().count() + 40 || (!all_drop && after * 3 < before) {
        return Err("改动幅度过大".into());
    }
    Ok(revised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::clarify::gap_questions;
    use crate::agent::testkit::ScriptedModel;

    const RAW: &str = "# 关于共建研究平台的函\n\n市数据局：\n\n## 工作安排\n\n\
                       请贵单位于【待核实：研究方案反馈时限】前反馈研究意向。\
                       联系人：【待核实：联系人】。\n\n此函。\n";

    fn setup() -> (Ledger, Vec<Question>) {
        let mut ledger = Ledger::default();
        ledger.sync(RAW, "", &[]);
        let questions = gap_questions(&ledger, 4, &[]);
        (ledger, questions)
    }

    #[test]
    fn answers_are_written_into_the_paragraph_by_the_model() {
        let (ledger, questions) = setup();
        let model = ScriptedModel::new(|_, prompt| {
            assert!(prompt.contains("【所在小节】工作安排"), "{prompt}");
            assert!(prompt.contains("起草人答复「另行通知」"), "{prompt}");
            "反馈研究意向的具体时间另行通知。联系人：【待核实：联系人】。".into()
        });
        let mut lines = Vec::new();
        let revised = revise(
            &model,
            RAW,
            ledger,
            &questions,
            &[(1, Reply::Choice(0)), (2, Reply::Skip)],
            &[],
            &mut |line| lines.push(line),
        );
        assert!(
            revised
                .raw
                .contains("反馈研究意向的具体时间另行通知。联系人：【待核实：联系人】。"),
            "{}",
            revised.raw
        );
        assert_eq!(
            revised.ledger.gaps[0].status,
            GapStatus::Answered("另行通知".into())
        );
        assert_eq!(revised.ledger.gaps[1].status, GapStatus::Skipped);
        assert_eq!(lines, ["「研究方案反馈时限」已写进所在段落"]);
    }

    #[test]
    fn a_rewrite_that_invents_facts_falls_back_to_plain_replacement() {
        let (ledger, questions) = setup();
        // 模型自己编了个电话号码。
        let model = ScriptedModel::new(|_, _| {
            "请贵单位于10月31日前反馈研究意向。联系人：张三，电话12345678。".into()
        });
        let mut lines = Vec::new();
        let revised = revise(
            &model,
            RAW,
            ledger,
            &questions,
            &[
                (1, Reply::Custom("10月31日".into())),
                (2, Reply::Custom("张三".into())),
            ],
            &[],
            &mut |line| lines.push(line),
        );
        assert!(
            revised
                .raw
                .contains("请贵单位于10月31日前反馈研究意向。联系人：张三。"),
            "{}",
            revised.raw
        );
        assert!(lines[0].contains("没过闸门"), "{lines:?}");
    }

    #[test]
    fn dropping_every_sentence_removes_the_paragraph() {
        let raw = "# 函\n\n联系人：【待核实：联系人】。\n\n此函。\n";
        let mut ledger = Ledger::default();
        ledger.sync(raw, "", &[]);
        let questions = gap_questions(&ledger, 4, &[]);
        let drop = questions[0]
            .choices
            .iter()
            .position(|c| c.label == "删去这项")
            .unwrap();
        let model = ScriptedModel::new(|_, _| WHOLE_PARAGRAPH_DROPPED.into());
        let revised = revise(
            &model,
            raw,
            ledger,
            &questions,
            &[(1, Reply::Choice(drop))],
            &[],
            &mut |_| {},
        );
        assert_eq!(revised.raw, "# 函\n\n此函。\n");
        assert_eq!(revised.ledger.gaps[0].status, GapStatus::Dropped);
    }
}
