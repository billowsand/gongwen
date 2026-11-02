//! 标准词库的纯查询能力：名称匹配、职能建议、所属单位。
//!
//! 从旧 `agent::tools::vocab` 原样搬来（含私有辅助函数），只有一份实现：词库工具、
//! 要素抽取、澄清出题、Pi 词库工具与旧编排都调这里。工具注册与 `ToolCtx` 留在旧工具层。

use crate::models::{VocabularyCategory, VocabularyEntry};

/// 名称对词库：规范名、对外名、简称、别名任一包含查询串即算对上。
/// 「要素抽取」（`element_fields`）把原文摘录解析成词库词条用的也是这一套。
pub(crate) fn matches(entry: &VocabularyEntry, query: &str) -> bool {
    query.is_empty()
        || entry.key() == query
        || entry.code == query
        || entry.canonical.contains(query)
        || entry.external_name.contains(query)
        || entry.abbr.contains(query)
        || entry.aliases.iter().any(|alias| alias.contains(query))
}

/// 按名称找，也按职能任务 / 个人简介找：「谁管数据共享」查「数据共享」就能找到。
pub(crate) fn matches_with_duties(entry: &VocabularyEntry, query: &str) -> bool {
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

/// 单位编码 → 规范名称。
pub(crate) fn unit_name<'a>(vocabulary: &'a [VocabularyEntry], code: &str) -> &'a str {
    vocabulary
        .iter()
        .find(|e| e.category == VocabularyCategory::Unit && e.code == code)
        .map_or("", |e| e.canonical.as_str())
}

/// 模糊检索用于列候选；指定父单位／所属单位必须唯一解析。
pub(crate) fn unique_unit<'a>(
    vocabulary: &'a [VocabularyEntry],
    value: &str,
) -> Result<&'a VocabularyEntry, String> {
    if let Some(entry) =
        crate::units::identity::resolve(vocabulary, VocabularyCategory::Unit, value)
    {
        return Ok(entry);
    }
    let found: Vec<_> = vocabulary
        .iter()
        .filter(|e| {
            e.category == VocabularyCategory::Unit
                && [&e.canonical, &e.external_name, &e.abbr]
                    .into_iter()
                    .chain(&e.aliases)
                    .any(|s| s == value)
        })
        .collect();
    if found.len() == 1 {
        Ok(found[0])
    } else {
        Err(format!(
            "单位不存在或有同名歧义，请使用查询结果的身份编码：{value}"
        ))
    }
}
