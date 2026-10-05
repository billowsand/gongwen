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

/// 按名称找，也按职能任务 / 个人简介找：「谁管数据共享」查「数据共享」就能找到。
fn matches_with_duties(entry: &VocabularyEntry, query: &str) -> bool {
    matches(entry, query) || entry.duties.contains(query) || entry.profile.contains(query)
}

/// 职能对号时不算数的常见字对：哪个单位的职能里都有。
const COMMON_PAIRS: [&str; 16] = [
    "工作", "负责", "有关", "相关", "开展", "组织", "落实", "加强", "推进", "做好", "承担", "协调",
    "指导", "管理", "建设", "单位",
];

/// 汉字两两相邻的字对。
fn pairs(text: &str) -> std::collections::HashSet<String> {
    let chars: Vec<char> = text
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .collect();
    chars
        .windows(2)
        .map(|pair| pair.iter().collect::<String>())
        .filter(|pair| !COMMON_PAIRS.contains(&pair.as_str()))
        .collect()
}

/// 按职能任务（单位）或个人简介（人员）给一件事挑候选：字对重合两组以上才算对得上，按重合多少排，
/// 最多 `max` 个。只读标准词库，候选由用户点选（红线 2：描述类要素的值来自标准词库并注明出处）。
pub(crate) fn suggest<'a>(
    vocabulary: &'a [VocabularyEntry],
    category: VocabularyCategory,
    task: &str,
    max: usize,
) -> Vec<&'a VocabularyEntry> {
    let wanted = pairs(task);
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &VocabularyEntry)> = vocabulary
        .iter()
        .filter(|entry| entry.category == category && !entry.canonical.trim().is_empty())
        .filter_map(|entry| {
            let about = match category {
                VocabularyCategory::Unit => &entry.duties,
                _ => &entry.profile,
            };
            let score = pairs(about).intersection(&wanted).count();
            (score >= 2).then_some((score, entry))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .take(max)
        .map(|(_, entry)| entry)
        .collect()
}

/// 人员所属单位的名称（候选的说明里用）。
pub(crate) fn unit_of<'a>(vocabulary: &'a [VocabularyEntry], person: &VocabularyEntry) -> &'a str {
    // 未归属的人员：编码为空，别和编码同样为空的单位对上。
    if person.unit.trim().is_empty() {
        return "";
    }
    unit_name(vocabulary, &person.unit)
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
        "查标准词库里的单位：规范名称、对外名称、简称、别名、层级编码、上级单位与职能任务。\
         分配任务、定牵头或责任单位时用 query 写事项关键词，按职能任务对号"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("query", "按名称、简称、别名或职能任务里的关键词找"),
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
            .filter(|e| {
                e.category == VocabularyCategory::Unit && matches_with_duties(e, query.trim())
            })
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
                    "duties": e.duties,
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
        "查标准词库里的人员：姓名、职务、所属单位、个人简介。推荐联系人、负责人时用 query 写事项关键词，\
         按个人简介对号。电话默认不给，技能声明 vocab.persons.phone 才返回"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("query", "按姓名或个人简介里的关键词找"),
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
            .filter(|e| {
                e.category == VocabularyCategory::Person && matches_with_duties(e, query.trim())
            })
            .filter(|e| unit_code.as_ref().is_none_or(|code| &e.unit == code))
            .take(MAX_ROWS)
            .map(|e| {
                let mut row = json!({
                    "name": e.canonical,
                    "position": e.position,
                    "unit": unit_name(vocabulary, &e.unit),
                    "profile": e.profile,
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
