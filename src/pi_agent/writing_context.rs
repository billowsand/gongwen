//! 写作时的日期基准和标准组织背景。只读，不改表单，也不判断正文事实真伪。
use crate::ai_services::vocabulary as vocab;
use crate::models::{VocabularyCategory as Category, VocabularyEntry as Entry};
use serde_json::{Value, json};

fn clip(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

pub(super) fn date_context() -> Value {
    let time = crate::prompt::TimeContext::now();
    json!({
        "now": time.now, "timezone": time.timezone, "today": time.today,
        "tomorrow": time.tomorrow, "this_week": time.this_week, "next_week": time.next_week,
        "note": "本机当前日期，用于理解今年、本月、明天、下周等相对时间；材料中的明确日期和已经约定的期限不自动改动。成文日期等表单授权要素仍由用户填写。",
    })
}

fn row(entries: &[Entry], entry: &Entry) -> Value {
    let owner = crate::units::identity::parent(entries, entry);
    json!({
        "identity": entry.key(), "name": clip(&entry.canonical, 200),
        "category": entry.category.label(), "code": clip(&entry.code, 100),
        "external_name": clip(&entry.external_name, 200), "abbr": clip(&entry.abbr, 100),
        "aliases": entry.aliases.iter().take(10).map(|name| clip(name, 100)).collect::<Vec<_>>(),
        "unit": owner.map(|unit| clip(&unit.canonical, 200)),
        "unit_identity": owner.map(Entry::key),
        "position": clip(&entry.position, 200), "phone": clip(&entry.phone, 200), "note": clip(&entry.note, 500),
        "duties": clip(&entry.duties, 2000), "duties_truncated": entry.duties.chars().count() > 2000,
        "profile": clip(&entry.profile, 1000), "profile_truncated": entry.profile.chars().count() > 1000,
    })
}

/// 小词库完整提供单位；大词库优先名称命中、职能候选及其上级。明确报告未提供的条目。
pub(super) fn organization_context(entries: &[Entry], request: &str) -> Value {
    let units: Vec<_> = entries
        .iter()
        .filter(|e| e.category == Category::Unit)
        .collect();
    let suggested = vocab::suggest(entries, Category::Unit, request, 16);
    let named = |entry: &&Entry| {
        [&entry.canonical, &entry.external_name, &entry.abbr]
            .into_iter()
            .chain(&entry.aliases)
            .any(|name| !name.trim().is_empty() && request.contains(name.as_str()))
    };
    let mut ranked: Vec<&Entry> = entries
        .iter()
        .filter(|entry| {
            named(entry)
                || (entry.category == Category::Unit
                    && (units.len() <= 32 || suggested.contains(entry)))
        })
        .collect();
    ranked.sort_by_key(|entry| !named(entry));
    let mut chosen: Vec<&Entry> = ranked.into_iter().take(40).collect();
    for entry in chosen.clone() {
        let mut owner = crate::units::identity::parent(entries, entry);
        for _ in 0..16 {
            let Some(unit) = owner else {
                break;
            };
            if chosen.contains(&unit) {
                break;
            }
            if chosen.len() >= 48 {
                break;
            }
            chosen.push(unit);
            owner = crate::units::identity::parent(entries, unit);
        }
    }
    let mut rows: Vec<Value> = chosen.iter().map(|entry| row(entries, entry)).collect();
    // 上下文不无界增长；剩余条目可用分页工具继续查。
    while serde_json::to_vec(&rows).is_ok_and(|bytes| bytes.len() > 32 * 1024) {
        rows.pop();
    }
    let provided = rows.len();
    json!({
        "results": rows, "total": entries.len(), "provided": provided,
        "truncated": provided < entries.len(),
        "note": "标准词库提供规范名称、所属关系、职能任务及人员简介。安排分工时结合职责判断牵头与配合；职责未写清不猜。人员优先查词库，同名按所属单位区分；查不到时保留用户材料中的明确人名。未列出不表示不存在，使用 vocab_search 按名称、职能或所属单位继续分页查找。不会自动修改表单。",
    })
}

pub(super) fn search(
    entries: &[Entry],
    args: &serde_json::Map<String, Value>,
) -> Result<Value, String> {
    for field in ["query", "category", "unit"] {
        if args.get(field).is_some_and(|value| !value.is_string()) {
            return Err(format!("词库参数 {field} 必须是文字。"));
        }
    }
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if query.chars().count() > 200 {
        return Err("词库查询词不能超过 200 字。".into());
    }
    let category = match args.get("category").and_then(Value::as_str) {
        Some("unit" | "单位") => Some(Category::Unit),
        Some("person" | "人员") => Some(Category::Person),
        None => None,
        Some(value) => return Err(format!("不认识的词库类别：{value}")),
    };
    let unit = args
        .get("unit")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| vocab::unique_unit(entries, value))
        .transpose()?;
    let matches: Vec<_> = entries
        .iter()
        .filter(|entry| {
            category.is_none_or(|value| value == entry.category)
                && vocab::matches_with_duties(entry, query)
                && unit
                    .is_none_or(|unit| crate::units::identity::parent(entries, entry) == Some(unit))
        })
        .collect();
    let offset = args
        .get("offset")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(usize::MAX as u64) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 40) as usize;
    let results: Vec<_> = matches
        .iter()
        .skip(offset)
        .take(limit)
        .map(|entry| row(entries, entry))
        .collect();
    let next = offset.saturating_add(results.len());
    Ok(
        json!({"results": results, "total": matches.len(), "offset": offset,
        "next_offset": (next < matches.len()).then_some(next),
        "note": "按名称、职能任务和人员简介检索；unit 可筛所属单位。结果只提供标准组织背景，不证明本次事项发生，也不自动写表单。"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Entry {
                canonical: "市数据局".into(),
                code: "A".into(),
                duties: "统筹公共数据归集、共享开放".into(),
                ..Default::default()
            },
            Entry {
                canonical: "技术中心".into(),
                code: "B".into(),
                parent: "A".into(),
                duties: "负责共享平台技术维护".into(),
                ..Default::default()
            },
            Entry {
                category: Category::Person,
                canonical: "张三".into(),
                unit: "A".into(),
                position: "科长".into(),
                profile: "负责数据共享协调".into(),
                ..Default::default()
            },
            Entry {
                category: Category::Person,
                canonical: "张三".into(),
                unit: "B".into(),
                position: "工程师".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn duties_and_profiles_are_searchable_and_same_name_people_can_be_scoped() {
        let entries = entries();
        let query = json!({"query":"共享", "category":"unit"});
        let result = search(&entries, query.as_object().unwrap()).unwrap();
        assert_eq!(result["total"], 2);
        assert!(
            result["results"][0]["duties"]
                .as_str()
                .unwrap()
                .contains("统筹")
        );
        let query = json!({"query":"张三", "category":"person", "unit":"技术中心"});
        let result = search(&entries, query.as_object().unwrap()).unwrap();
        assert_eq!(result["total"], 1);
        assert_eq!(result["results"][0]["position"], "工程师");
        assert_eq!(result["results"][0]["unit"], "技术中心");
        let query = json!({"query":"协调", "category":"person"});
        assert_eq!(
            search(&entries, query.as_object().unwrap()).unwrap()["total"],
            1
        );
    }

    #[test]
    fn organization_context_and_paging_keep_names_duties_and_parent_relationships() {
        let entries = entries();
        let context = organization_context(&entries, "由技术中心配合，请张三联系");
        let rows = context["results"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1]["unit"], "市数据局");
        assert!(rows[0]["duties"].as_str().unwrap().contains("共享"));
        let first = search(
            &entries,
            json!({"category":"person","limit":1}).as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(first["next_offset"], 1);
        let second = search(
            &entries,
            json!({"category":"person","offset":1,"limit":1})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        assert!(second["next_offset"].is_null());
        assert_ne!(first["results"][0]["unit"], second["results"][0]["unit"]);
    }

    #[test]
    fn date_context_comes_from_the_current_local_clock_with_week_and_zone() {
        let context = date_context();
        assert_eq!(context["today"], crate::prompt::TimeContext::now().today);
        assert!(context["now"].as_str().unwrap().contains("星期"));
        assert!(context["timezone"].as_str().unwrap().starts_with("UTC"));
        assert!(!context["next_week"].as_str().unwrap().is_empty());
    }

    #[test]
    fn large_organization_background_is_bounded_and_does_not_invent_a_match() {
        let entries: Vec<_> = (0..300)
            .map(|i| Entry {
                canonical: format!("测试单位{i}"),
                duties: "技术职责".repeat(1000),
                ..Default::default()
            })
            .collect();
        let context = organization_context(&entries, "未知事项");
        assert_eq!(context["provided"], 0);
        assert_eq!(context["truncated"], true);
        assert!(serde_json::to_vec(&context).unwrap().len() < 48 * 1024);
    }
}
