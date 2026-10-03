//! D 组：标准词库与规范（只读）。

use super::{Input, Permission, Tool, ToolCtx, ToolOutput, arg_str, kind_arg, optional, required};
use crate::models::{VocabularyCategory, VocabularyEntry};
use serde_json::{Map, Value, json};

pub(super) const TOOLS: [&dyn Tool; 5] = [&Units, &Persons, &Normalize, &Style, &Lexicon];

/// 一次最多返回多少条词库记录。
const MAX_ROWS: usize = 30;

fn matches(entry: &VocabularyEntry, query: &str) -> bool {
    query.is_empty()
        || entry.canonical.contains(query)
        || entry.external_name.contains(query)
        || entry.abbr.contains(query)
        || entry.aliases.iter().any(|alias| alias.contains(query))
}

fn unit_name<'a>(vocabulary: &'a [VocabularyEntry], code: &str) -> &'a str {
    vocabulary
        .iter()
        .find(|e| e.category == VocabularyCategory::Unit && e.code == code)
        .map_or("", |e| e.canonical.as_str())
}

struct Units;

impl Tool for Units {
    fn id(&self) -> &'static str {
        "vocab.units"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "查标准词库里的单位：规范名称、对外名称、简称、别名、层级编码与上级单位"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("query", "按名称、简称或别名找"),
            optional("parent", "只列这个单位的下级（写单位名称）"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let vocabulary = ctx.env.vocabulary;
        let query = arg_str(args, "query").unwrap_or_default();
        let parent_code = match arg_str(args, "parent") {
            Some(name) => Some(
                vocabulary
                    .iter()
                    .find(|e| e.category == VocabularyCategory::Unit && matches(e, &name))
                    .map(|e| e.code.clone())
                    .ok_or_else(|| format!("标准词库里没有单位「{name}」"))?,
            ),
            None => None,
        };
        let rows: Vec<Value> = vocabulary
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit && matches(e, query.trim()))
            .filter(|e| parent_code.as_ref().is_none_or(|code| &e.parent == code))
            .take(MAX_ROWS)
            .map(|e| {
                json!({
                    "canonical": e.canonical,
                    "external_name": if e.external_name.is_empty() { &e.canonical } else { &e.external_name },
                    "abbr": e.abbr,
                    "aliases": e.aliases,
                    "code": e.code,
                    "parent": unit_name(vocabulary, &e.parent),
                })
            })
            .collect();
        let count = rows.len();
        Ok(ToolOutput::new(
            Value::Array(rows),
            format!("查标准词库单位 → {count} 个"),
        ))
    }
}

struct Persons;

impl Tool for Persons {
    fn id(&self) -> &'static str {
        "vocab.persons"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "查标准词库里的人员：姓名、职务、所属单位。电话默认不给，技能声明 vocab.persons.phone 才返回"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("query", "按姓名找"),
            optional("unit", "只列这个单位的人员（写单位名称）"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let vocabulary = ctx.env.vocabulary;
        let with_phone = ctx.env.skill.allows_tool("vocab.persons.phone");
        let query = arg_str(args, "query").unwrap_or_default();
        let unit_code = arg_str(args, "unit").and_then(|name| {
            vocabulary
                .iter()
                .find(|e| e.category == VocabularyCategory::Unit && matches(e, &name))
                .map(|e| e.code.clone())
        });
        let rows: Vec<Value> = vocabulary
            .iter()
            .filter(|e| e.category == VocabularyCategory::Person && matches(e, query.trim()))
            .filter(|e| unit_code.as_ref().is_none_or(|code| &e.unit == code))
            .take(MAX_ROWS)
            .map(|e| {
                let mut row = json!({
                    "name": e.canonical,
                    "position": e.position,
                    "unit": unit_name(vocabulary, &e.unit),
                    "note": e.note,
                });
                if with_phone {
                    row["phone"] = Value::String(e.phone.clone());
                }
                row
            })
            .collect();
        let count = rows.len();
        Ok(ToolOutput::new(
            Value::Array(rows),
            format!("查标准词库人员 → {count} 人"),
        ))
    }
}

struct Normalize;

impl Tool for Normalize {
    fn id(&self) -> &'static str {
        "vocab.normalize"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "把一段文字里的单位、人员简称与别名换成标准词库里的规范名称（只返回结果，不改工作稿）"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required("text", "要规范化的文字")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let mut text = arg_str(args, "text").unwrap_or_default();
        // 别名从长到短替换：「林草局」不能先把「市林草局」拆坏。同时跳过已经是规范名称一部分的位置。
        let mut pairs: Vec<(&str, &str)> = ctx
            .env
            .vocabulary
            .iter()
            .flat_map(|e| {
                e.aliases
                    .iter()
                    .filter(|alias| !alias.is_empty() && **alias != e.canonical)
                    .map(move |alias| (alias.as_str(), e.canonical.as_str()))
            })
            .collect();
        pairs.sort_by_key(|(alias, _)| std::cmp::Reverse(alias.chars().count()));
        let mut changes = Vec::new();
        for (alias, canonical) in pairs {
            // 已经写成规范名称的地方（别名落在某个规范名称内部）不动。
            let inside: Vec<std::ops::Range<usize>> = ctx
                .env
                .vocabulary
                .iter()
                .map(|e| e.canonical.as_str())
                .filter(|name| name.contains(alias))
                .flat_map(|name| {
                    text.match_indices(name)
                        .map(move |(at, _)| at..at + name.len())
                })
                .collect();
            let mut out = String::with_capacity(text.len());
            let mut last = 0usize;
            let mut count = 0;
            for (at, _) in text.match_indices(alias) {
                if inside
                    .iter()
                    .any(|r| r.start <= at && at + alias.len() <= r.end)
                {
                    continue;
                }
                out.push_str(&text[last..at]);
                out.push_str(canonical);
                last = at + alias.len();
                count += 1;
            }
            out.push_str(&text[last..]);
            if count > 0 {
                changes.push(json!({"from": alias, "to": canonical, "count": count}));
                text = out;
            }
        }
        let total: u64 = changes.iter().filter_map(|c| c["count"].as_u64()).sum();
        Ok(ToolOutput::new(
            json!({"text": text, "changes": changes}),
            format!("规范化单位与人员名称 {total} 处"),
        ))
    }
}

struct Style;

impl Tool for Style {
    fn id(&self) -> &'static str {
        "rules.style"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "某个文种的写作规范与语感要求（行文方向、力度词、开头收尾等）"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("kind", "文种；不给用当前文种")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let kind = kind_arg(args, "kind").unwrap_or(ctx.board.draft.kind);
        let text = format!(
            "文种：{}（{}）\n{}",
            kind.label(),
            kind.description(),
            crate::prompt::style_guide(kind)
        );
        Ok(ToolOutput::new(
            Value::String(text),
            format!("读取「{}」写作规范", kind.label()),
        ))
    }
}

struct Lexicon;

impl Tool for Lexicon {
    fn id(&self) -> &'static str {
        "rules.lexicon"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "查校对词表：一个词或一句话有没有规范写法、该怎么改"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required("term", "要查的词或句子")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let term = arg_str(args, "term").unwrap_or_default();
        let lexicon = crate::proofread::Lexicon::resolved(&ctx.env.config.proofread);
        let hits: Vec<Value> = lexicon
            .check(&term)
            .into_iter()
            .map(|note| {
                json!({
                    "found": term.get(note.span.clone()).unwrap_or(""),
                    "message": note.message,
                    "replacement": note.replacement,
                    "level": note.level.label(),
                    "group": note.group,
                })
            })
            .collect();
        let count = hits.len();
        Ok(ToolOutput::new(
            Value::Array(hits),
            format!("查校对词表 → {count} 条"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use serde_json::json;

    #[test]
    fn units_are_found_by_alias_and_listed_under_a_parent() {
        let mut fixture = Fixture::new("");
        let by_alias = fixture
            .call("vocab.units", json!({"query": "林草局"}))
            .unwrap();
        assert_eq!(by_alias.value[0]["canonical"], "市林业和草原局");
        assert_eq!(by_alias.value[0]["external_name"], "市林草局");
        let children = fixture
            .call("vocab.units", json!({"parent": "市应急管理局"}))
            .unwrap();
        assert_eq!(children.value.as_array().unwrap().len(), 1);
        assert_eq!(children.value[0]["parent"], "市应急管理局");
    }

    #[test]
    fn phone_numbers_need_an_explicit_declaration() {
        let mut fixture = Fixture::new("");
        fixture.skill.tools.retain(|t| t != "vocab.persons.phone");
        let row = fixture
            .call("vocab.persons", json!({"unit": "市应急局"}))
            .unwrap()
            .value[0]
            .clone();
        assert_eq!(row["name"], "张三");
        assert_eq!(row["unit"], "市应急管理局");
        assert!(row.get("phone").is_none(), "默认不给电话");
        fixture.skill.tools.push("vocab.persons.phone".into());
        let row = fixture
            .call("vocab.persons", json!({"query": "张"}))
            .unwrap()
            .value[0]
            .clone();
        assert_eq!(row["phone"], "13800000000");
    }

    #[test]
    fn normalize_replaces_aliases_without_breaking_canonical_names() {
        let mut fixture = Fixture::new("");
        let out = fixture
            .call("vocab.normalize", json!({"text": "请市应急局与林草局、市应急管理局办公室会同办理。市应急管理局和市应急局。"}))
            .unwrap();
        assert_eq!(
            out.value["text"],
            "请市应急管理局与市林业和草原局、市应急管理局办公室会同办理。市应急管理局和市应急管理局。"
        );
        assert_eq!(out.value["changes"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn style_and_lexicon_lookups() {
        let mut fixture = Fixture::new("");
        let style = fixture
            .call("rules.style", json!({"kind": "公函"}))
            .unwrap();
        assert!(style.value.as_str().unwrap().contains("公函"));
        let hits = fixture
            .call("rules.lexicon", json!({"term": "布署工作"}))
            .unwrap();
        assert!(
            hits.value
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["replacement"] == "部署"),
            "{:?}",
            hits.value
        );
    }
}
