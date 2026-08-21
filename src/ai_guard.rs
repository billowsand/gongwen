//! AI 改写前后的关键事实保护。
//!
//! 提示词只能表达“不得改事实”的意图；这里用确定性提取再比较一遍，保证单位、
//! 人名、日期、数量和文件名称一旦发生变化，必须经过用户显式确认才能落回审校稿。

use crate::models::{VocabularyCategory, VocabularyEntry};
use regex::Regex;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FactKind {
    Unit,
    Person,
    DateTime,
    Number,
    Document,
}

impl FactKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unit => "单位名称",
            Self::Person => "人员姓名",
            Self::DateTime => "日期时间",
            Self::Number => "数字数据",
            Self::Document => "文件依据",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FactToken {
    pub kind: FactKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactChangeKind {
    Removed,
    Added,
}

impl FactChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Removed => "被删除",
            Self::Added => "被新增",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactChange {
    pub kind: FactKind,
    pub value: String,
    pub change: FactChangeKind,
}

/// 提取适合锁定的关键事实。标题编号、Markdown 序号等普通小数字不算事实；
/// 数量只认“数字 + 量词/单位”、百分比和四位年份，降低格式调整产生的误报。
pub fn extract_key_facts(markdown: &str, vocabulary: &[VocabularyEntry]) -> Vec<FactToken> {
    let mut out = BTreeSet::new();
    for entry in vocabulary {
        let canonical = entry.canonical.trim();
        if canonical.is_empty() || !markdown.contains(canonical) {
            continue;
        }
        let kind = match entry.category {
            VocabularyCategory::Unit => FactKind::Unit,
            VocabularyCategory::Person => FactKind::Person,
        };
        out.insert(FactToken {
            kind,
            value: canonical.to_string(),
        });
    }

    collect_matches(
        markdown,
        r"(?:[12]\d{3}年(?:1[0-2]|0?[1-9])月(?:3[01]|[12]\d|0?[1-9])日(?:（星期[一二三四五六日天]）)?|(?:1[0-2]|0?[1-9])月(?:3[01]|[12]\d|0?[1-9])日|(?:[01]?\d|2[0-3])[:：][0-5]\d)",
        FactKind::DateTime,
        &mut out,
    );
    collect_matches(
        markdown,
        r"(?:\d+(?:\.\d+)?%|\d+(?:\.\d+)?(?:万|亿)?(?:元|人|户|家|个|项|件|份|次|场|名|套|公里|千米|平方米|亩|吨|天|日|月|年)|[12]\d{3})",
        FactKind::Number,
        &mut out,
    );
    collect_matches(
        markdown,
        r"《[^》\r\n]{2,80}》",
        FactKind::Document,
        &mut out,
    );

    // 词库外单位也要尽量看住。只认常见机构后缀，且避开跨标点的长串。
    collect_matches(
        markdown,
        r"[\p{Han}]{2,24}(?:委员会|人民政府|办公室|工作组|领导小组|管理局|分局|厅|局|处|科|中心|公司|集团|学院|学校)",
        FactKind::Unit,
        &mut out,
    );
    out.into_iter().collect()
}

fn collect_matches(text: &str, pattern: &str, kind: FactKind, out: &mut BTreeSet<FactToken>) {
    let regex = Regex::new(pattern).expect("关键事实正则必须有效");
    for hit in regex.find_iter(text) {
        out.insert(FactToken {
            kind,
            value: hit.as_str().to_string(),
        });
    }
}

pub fn compare_key_facts(
    before: &str,
    after: &str,
    vocabulary: &[VocabularyEntry],
) -> Vec<FactChange> {
    let before: BTreeSet<_> = extract_key_facts(before, vocabulary).into_iter().collect();
    let after: BTreeSet<_> = extract_key_facts(after, vocabulary).into_iter().collect();
    let mut changes = Vec::new();
    changes.extend(before.difference(&after).map(|fact| FactChange {
        kind: fact.kind,
        value: fact.value.clone(),
        change: FactChangeKind::Removed,
    }));
    changes.extend(after.difference(&before).map(|fact| FactChange {
        kind: fact.kind,
        value: fact.value.clone(),
        change: FactChangeKind::Added,
    }));
    changes
}

/// 给模型看的锁定清单。程序比较仍是最终防线；这段只用于尽量减少模型越界。
pub fn protected_facts_prompt(markdown: &str, vocabulary: &[VocabularyEntry]) -> String {
    let facts = extract_key_facts(markdown, vocabulary);
    if facts.is_empty() {
        return String::new();
    }
    let rows = facts
        .iter()
        .map(|fact| format!("- [{}] {}", fact.kind.label(), fact.value))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\n\n【锁定事实——除非本次任务明确要求，否则必须逐字保留】\n{rows}\n不得删除、替换、简称或新增同类事实。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_changed_dates_numbers_documents_and_units() {
        let old = "请某某市教育局于2026年8月21日拨付10万元，依据《测试办法》办理。";
        let new = "请某某市教育局于2026年8月22日拨付12万元办理。";
        let changes = compare_key_facts(old, new, &[]);
        assert!(changes.iter().any(|item| item.value == "2026年8月21日"));
        assert!(changes.iter().any(|item| item.value == "12万元"));
        assert!(changes.iter().any(|item| item.value == "《测试办法》"));
        assert!(!changes.iter().any(|item| item.value == "某某市教育局"));
    }

    #[test]
    fn vocabulary_people_are_locked() {
        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Person,
            canonical: "张三".into(),
            ..Default::default()
        }];
        let changes = compare_key_facts("张三参加会议。", "李四参加会议。", &vocabulary);
        assert!(changes.iter().any(|item| {
            item.kind == FactKind::Person
                && item.value == "张三"
                && item.change == FactChangeKind::Removed
        }));
    }
}
