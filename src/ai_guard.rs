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
    extract_facts(markdown, vocabulary, true)
}

/// 只认得准的那几类：词库里的单位与人名、日期、数量、书名号文件名。
///
/// **不含**词库外单位的正则兜底。那条兜底是贪婪的：`[汉字]{2,24}` 后接「科」，
/// 在「请于本月底前将落实情况书面报送我办综合科」里会把整个前半句吞成一个
/// 「单位名」——汉字之间没有边界符可以让正则停下，而 Rust 的 regex 不支持
/// 环视，改不出「必须从词的开头起算」。
///
/// 结果是：句首任何改动都会让这个假单位名变样，从而误判成「改动了关键事实」。
/// 用在整篇提案的事实清单上只是多列几行让人核对，尚可接受；用作**逐句改写的
/// 硬闸门就是灾难**——公文句子里到处是局、处、科、中心，绝大多数正常的语病
/// 修改都会被无声丢弃，而且丢得毫无痕迹。
///
/// 所以逐句闸门只认这一版。词库外单位的保护由此让位给「用户逐条过目」：
/// 模型建议本来就是「疑似」档，界面上原文与改法并排显示，人看得见。
pub fn compare_key_facts_precise(
    before: &str,
    after: &str,
    vocabulary: &[VocabularyEntry],
) -> Vec<FactChange> {
    diff_facts(
        extract_facts(before, vocabulary, false),
        extract_facts(after, vocabulary, false),
    )
}

fn extract_facts(
    markdown: &str,
    vocabulary: &[VocabularyEntry],
    heuristic_units: bool,
) -> Vec<FactToken> {
    let mut out = BTreeSet::new();
    // 公文引用按规范写法记一条：名称与文号一起作为事实，括号写法不同不算改动。
    for citation in crate::document_reference::detect_numbered(markdown) {
        out.insert(FactToken {
            kind: FactKind::Document,
            value: citation.text(),
        });
    }
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
    // 贪婪匹配会吞掉前面的普通汉字，见 [`compare_key_facts_precise`] 的说明——
    // 整篇比对容得下这点噪音，逐句闸门容不下。
    if heuristic_units {
        collect_heuristic_units(markdown, &mut out);
    }
    out.into_iter().collect()
}

/// 词库外单位的启发式抽取，用 jieba 词性切分代替正则：
/// 找到以机构后缀结尾的词（「局」「委员会」等），再向前合并紧挨着的名词类词，
/// 遇到非名词（「因」「可能」「的」）就停。这样「中华人民共和国财政部」「共和国」
/// 不会从中间断开，「可能因认知局」只取到「认知局」。
/// 合并结果不足两个字，或者不是以后缀结尾，都丢弃。
fn collect_heuristic_units(text: &str, out: &mut BTreeSet<FactToken>) {
    const UNIT_SUFFIXES: [&str; 17] = [
        "委员会",
        "人民政府",
        "办公室",
        "工作组",
        "领导小组",
        "管理局",
        "分局",
        "厅",
        "局",
        "处",
        "科",
        "中心",
        "公司",
        "集团",
        "学院",
        "学校",
        "部",
    ];
    // 单位名的最长字数，超过就不是一个单位名，不再向前合并。
    const MAX_CHARS: usize = 24;
    let tokens = crate::lexicon::segmenter::tagged(text);
    for (end, (word, tag)) in tokens.iter().enumerate() {
        // 后缀词本身必须是名词，「无处」这类副词性的「…处」不算单位。
        if !tag.starts_with('n') || !UNIT_SUFFIXES.iter().any(|suffix| word.ends_with(suffix)) {
            continue;
        }
        let mut start = end;
        let mut chars = word.chars().count();
        while start > 0 {
            let (prev, tag) = &tokens[start - 1];
            let prev_chars = prev.chars().count();
            // 名词、动词（「认知」「管理」）、代词（「某某局」）、区别词（「开放式」）
            // 可以并进单位名；副词、介词、连词、助词是句子的边界。
            // jieba 对动词与名词的标注常有出入，所以动词也收。
            // 单字动词多半是句首的「请」「须」之类，不并入。
            let joinable = tag.starts_with('n')
                || (tag.starts_with('v') && prev_chars >= 2)
                || tag.starts_with('r')
                || tag.starts_with('b');
            if !joinable || chars + prev_chars > MAX_CHARS {
                break;
            }
            chars += prev_chars;
            start -= 1;
        }
        let value: String = tokens[start..=end]
            .iter()
            .map(|(word, _)| word.as_str())
            .collect();
        if value.chars().count() < 2 {
            continue;
        }
        out.insert(FactToken {
            kind: FactKind::Unit,
            value,
        });
    }
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
    diff_facts(
        extract_key_facts(before, vocabulary),
        extract_key_facts(after, vocabulary),
    )
}

fn diff_facts(before: Vec<FactToken>, after: Vec<FactToken>) -> Vec<FactChange> {
    let before: BTreeSet<_> = before.into_iter().collect();
    let after: BTreeSet<_> = after.into_iter().collect();
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
        "\n\n【锁定事实——除非本次任务明确要求，否则必须逐字保留】\n{rows}\n不得删除、替换、简称或新增同类事实。\n正文里的公文引用《名称》（发文字号）必须逐字保留，不得改写名称或文号，不得新增或删除引用。"
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
    fn heuristic_unit_does_not_swallow_sentence_prefix() {
        let facts = extract_key_facts("人类决策者可能因认知局，并非某某局。", &[]);
        let units: Vec<_> = facts
            .iter()
            .filter(|fact| fact.kind == FactKind::Unit)
            .map(|fact| fact.value.as_str())
            .collect();
        assert_eq!(units, vec!["某某局", "认知局"]);
        // 「无处」里的「处」只剩一个字，不算单位名。
        assert!(
            extract_key_facts("其突出优势在于其无处", &[])
                .iter()
                .all(|fact| fact.kind != FactKind::Unit)
        );
    }

    #[test]
    fn heuristic_unit_keeps_names_containing_function_words() {
        // 「和」「共和国」都是名字的一部分，不能从中间切开。
        let facts = extract_key_facts("请中华人民共和国财政部牵头办理。", &[]);
        assert!(
            facts
                .iter()
                .any(|fact| fact.kind == FactKind::Unit && fact.value == "中华人民共和国财政部"),
            "实际抽取：{facts:?}"
        );
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
