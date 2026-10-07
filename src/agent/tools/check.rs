//! E 组：确定性检查。不改任何东西，结果给流程判断或交给用户。
//!
//! 默认检查工作稿（工作稿为空时检查正文）；给了 `text` 就检查那段文字。
//! 排版实测（`check.layout`）要走 Typst 编译，第 ④ 期随管理界面一起加。

use super::{Input, Permission, Tool, ToolCtx, ToolOutput, arg_str, optional, short};
use crate::agent::gaps::{find_placeholders, sentence_at, untraced_facts};
use serde_json::{Map, Value, json};

pub(super) const TOOLS: [&dyn Tool; 5] =
    [&Elements, &Proofread, &Facts, &Placeholders, &References];

const TEXT_DOC: &str = "要检查的文字；不给就检查工作稿";

struct Elements;

impl Tool for Elements {
    fn id(&self) -> &'static str {
        "check.elements"
    }
    fn permission(&self) -> Permission {
        Permission::Check
    }
    fn description(&self) -> &'static str {
        "要素与结构校验：缺不缺主送、标题规不规范、密级与要素是否一致等"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("text", TEXT_DOC)];
        INPUTS
    }
    fn example(&self) -> &'static str {
        r#"{}"#
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = ctx.text_arg(args);
        let issues = crate::validator::validate(
            &ctx.board.draft,
            &text,
            ctx.env.vocabulary,
            &ctx.env.config.security_rules,
        );
        let count = issues.len();
        Ok(ToolOutput::new(
            Value::Array(issues.into_iter().map(Value::String).collect()),
            format!("要素与结构校验 → {count} 条"),
        ))
    }
}

struct Proofread;

impl Tool for Proofread {
    fn id(&self) -> &'static str {
        "check.proofread"
    }
    fn permission(&self) -> Permission {
        Permission::Check
    }
    fn description(&self) -> &'static str {
        "校对词表 + 文档规则 + 语感规则复扫；有确定改法的带出替换文字"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("text", TEXT_DOC)];
        INPUTS
    }
    fn example(&self) -> &'static str {
        r#"{}"#
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = ctx.text_arg(args);
        let lexicon = crate::proofread::Lexicon::resolved(&ctx.env.config.proofread);
        let mut notes = lexicon.check(&text);
        notes.extend(crate::proofread_rules::check(&ctx.board.draft, &text));
        let must = notes
            .iter()
            .filter(|n| n.level == crate::proofread::Level::MustFix)
            .count();
        let rows: Vec<Value> = notes
            .into_iter()
            .map(|note| {
                json!({
                    "found": text.get(note.span.clone()).unwrap_or(""),
                    "message": note.message,
                    "replacement": note.replacement,
                    "level": note.level.label(),
                    "group": note.group,
                    "rule": note.entry_id,
                })
            })
            .collect();
        let count = rows.len();
        Ok(ToolOutput::new(
            Value::Array(rows),
            format!("词表与规则复扫 → {count} 条（必错 {must}）"),
        ))
    }
}

struct Facts;

impl Tool for Facts {
    fn id(&self) -> &'static str {
        "check.facts"
    }
    fn permission(&self) -> Permission {
        Permission::Check
    }
    fn description(&self) -> &'static str {
        "抽取关键事实（单位、人名、日期、数量、文件名）；给了 sources 就只列在来源里找不到的"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("text", TEXT_DOC),
            optional(
                "sources",
                "对照的来源文字；不给就只抽取。写 board 表示用材料、回答、要素与证据包",
            ),
        ];
        INPUTS
    }
    fn example(&self) -> &'static str {
        r#"{"sources": "board"}"#
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = ctx.text_arg(args);
        match arg_str(args, "sources") {
            Some(sources) => {
                let sources = if sources == "board" {
                    ctx.board.sources_text()
                } else {
                    sources
                };
                let rows: Vec<Value> = untraced_facts(&text, &sources, ctx.env.vocabulary)
                    .into_iter()
                    .map(|fact| {
                        let sentence = text
                            .find(&fact.value)
                            .map(|at| text[sentence_at(&text, at)].to_string())
                            .unwrap_or_default();
                        json!({"value": fact.value, "kind": fact.kind.label(), "sentence": sentence})
                    })
                    .collect();
                let count = rows.len();
                Ok(ToolOutput::new(
                    Value::Array(rows),
                    format!("来源不明的事实 → {count} 处"),
                ))
            }
            None => {
                let rows: Vec<Value> =
                    crate::ai_guard::extract_key_facts(&text, ctx.env.vocabulary)
                        .into_iter()
                        .map(|fact| json!({"value": fact.value, "kind": fact.kind.label()}))
                        .collect();
                let count = rows.len();
                Ok(ToolOutput::new(
                    Value::Array(rows),
                    format!("抽取关键事实 → {count} 个"),
                ))
            }
        }
    }
}

struct Placeholders;

impl Tool for Placeholders {
    fn id(&self) -> &'static str {
        "check.placeholders"
    }
    fn permission(&self) -> Permission {
        Permission::Check
    }
    fn description(&self) -> &'static str {
        "列出「【待核实：…】」占位与所在句"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("text", TEXT_DOC)];
        INPUTS
    }
    fn example(&self) -> &'static str {
        r#"{}"#
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = ctx.text_arg(args);
        let rows: Vec<Value> = find_placeholders(&text)
            .into_iter()
            .map(|p| {
                let sentence = text[sentence_at(&text, p.span.start)].to_string();
                json!({"hint": p.hint, "literal": p.literal, "sentence": sentence})
            })
            .collect();
        let count = rows.len();
        Ok(ToolOutput::new(
            Value::Array(rows),
            format!("待核实占位 → {count} 处"),
        ))
    }
}

struct References;

impl Tool for References {
    fn id(&self) -> &'static str {
        "check.references"
    }
    fn permission(&self) -> Permission {
        Permission::Check
    }
    fn description(&self) -> &'static str {
        "识别正文里引用的公文（书名号加文号），给出规范写法"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("text", TEXT_DOC)];
        INPUTS
    }
    fn example(&self) -> &'static str {
        r#"{}"#
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = ctx.text_arg(args);
        let rows: Vec<Value> = crate::document_reference::detect_numbered(&text)
            .into_iter()
            .map(|c| {
                json!({
                    "found": text.get(c.range.clone()).unwrap_or(""),
                    "title": c.title,
                    "number": c.number,
                    "normalized": c.text(),
                })
            })
            .collect();
        let count = rows.len();
        let summary = match rows.first() {
            Some(first) => format!(
                "识别公文引用 → {count} 处（如{}）",
                short(first["normalized"].as_str().unwrap_or(""), 24)
            ),
            None => "识别公文引用 → 0 处".into(),
        };
        Ok(ToolOutput::new(Value::Array(rows), summary))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use serde_json::json;

    const DOC: &str = "# 关于做好冬季森林防火工作的通知\n\n根据《国务院办公厅关于加强森林防火工作的通知》（国办发〔2024〕3号），请于2026年12月1日前完成【待核实：排查范围】排查，布署值班。\n";

    #[test]
    fn placeholders_facts_and_references() {
        let mut fixture = Fixture::new(DOC);
        let holes = fixture.call("check.placeholders", json!({})).unwrap();
        assert_eq!(holes.value[0]["hint"], "排查范围");
        assert!(
            holes.value[0]["sentence"]
                .as_str()
                .unwrap()
                .contains("请于")
        );

        let facts = fixture.call("check.facts", json!({})).unwrap();
        assert!(
            facts
                .value
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["value"] == "2026年12月1日")
        );
        let untraced = fixture
            .call("check.facts", json!({"sources": "12月1日前完成排查"}))
            .unwrap();
        assert!(
            untraced
                .value
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["value"] == "2026年12月1日")
        );

        let refs = fixture.call("check.references", json!({})).unwrap();
        assert_eq!(refs.value[0]["number"], "国办发〔2024〕3号");
    }

    #[test]
    fn proofread_and_element_checks_report_findings() {
        let mut fixture = Fixture::new(DOC);
        let notes = fixture.call("check.proofread", json!({})).unwrap();
        assert!(
            notes
                .value
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["replacement"] == "部署"),
            "{:?}",
            notes.value
        );
        let issues = fixture.call("check.elements", json!({"text": ""})).unwrap();
        assert!(
            !issues.value.as_array().unwrap().is_empty(),
            "空正文一定有要素问题"
        );
    }
}
