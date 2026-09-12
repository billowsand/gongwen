//! 导出小鹤双拼用户码表。
//!
//! 码表文件形如：
//!
//! ```text
//! ---config@码表分类=主码-用户码表
//! ---config@码表别名=用户
//! 专项整治<TAB>vjvv
//! ```
//!
//! `<TAB>` 是真正的制表符；UTF-8 带 BOM——小鹤原生输入法所在的 Windows
//! 工具链按 BOM 认编码。
//!
//! ## 为什么要有规模预算
//!
//! 用户码表和主码表合进**同一个编码空间**：多收一个词就是多一条重码、多一次
//! 翻页。所以这里的默认取向是「小而精」——按 `省键数 × 篇数` 排序后截断，
//! 而不是扫到多少导出多少。同码词按同一权重排序落盘，让常用的排在前面。

use super::{LexiconTerm, TermOrigin, TermState};
use std::collections::BTreeMap;

/// 小鹤原生输入法的用户码表头。写在文件最前面，`encode` 时可选。
const FLYPY_HEADER: &str = "---config@码表分类=主码-用户码表\n---config@码表别名=用户\n";
const BOM: &str = "\u{feff}";

/// 导出口径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    /// 最少字数。默认 3——二字词在小鹤里本来就是四码，进码表省 0 键。
    pub min_chars: usize,
    /// 语料词至少要出现在几篇里。权威词源不受此限。
    pub min_doc_count: i64,
    /// 码表条数上限。超出的按权重截断。
    pub budget: usize,
    /// 连同尚未人工确认的候选词一起导出。
    pub include_candidates: bool,
    /// 排除 jieba 自带词典里就有的通用词。默认关：公文套语多半也在自带词典里，
    /// 而那恰恰是最该收的一类。
    pub exclude_common: bool,
    /// 写入小鹤码表头。导进「主码-用户码表」时需要。
    pub with_header: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            min_chars: 3,
            min_doc_count: 2,
            budget: 1500,
            include_candidates: true,
            exclude_common: false,
            with_header: true,
        }
    }
}

/// 一条被排除的词及原因，显示在导出摘要里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub term: String,
    pub reason: String,
}

/// 一次导出的产物。
#[derive(Debug, Clone, Default)]
pub struct FlypyExport {
    /// 码表正文（含 BOM，可直接写文件）。
    pub table: String,
    /// 重码报告（含 BOM）；没有重码时为空串。
    pub conflicts: String,
    pub written: usize,
    /// 因为预算截断而没进码表的条数。
    pub truncated: usize,
    /// 编码失败的词（多音字标错、字数对不上）。
    pub failed: Vec<Dropped>,
    /// 参与重码的编码数与词数。
    pub conflict_codes: usize,
    pub conflict_terms: usize,
    /// 实际省下的总键数（按每个词出现一次算的下限）。
    pub saved_keys: i64,
}

impl FlypyExport {
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("导出 {} 条", self.written)];
        if self.truncated > 0 {
            parts.push(format!("按预算截断 {} 条", self.truncated));
        }
        if self.conflict_codes > 0 {
            parts.push(format!(
                "{} 个编码上有重码，共 {} 词",
                self.conflict_codes, self.conflict_terms
            ));
        }
        if !self.failed.is_empty() {
            parts.push(format!("{} 条编码失败", self.failed.len()));
        }
        parts.join("，")
    }
}

/// 按口径挑词、排序、出码。
///
/// 排序是 `省键数 × 篇数` 降序：同一个四码下的候选按这个顺序落盘，最常用的
/// 排在最前面，翻页次数最少。
pub fn build(terms: &[LexiconTerm], options: &ExportOptions) -> FlypyExport {
    let mut chosen: Vec<&LexiconTerm> = terms.iter().filter(|term| keep(term, options)).collect();
    chosen.sort_by(|left, right| {
        right
            .weight()
            .cmp(&left.weight())
            .then_with(|| right.doc_count.cmp(&left.doc_count))
            .then_with(|| right.freq_total.cmp(&left.freq_total))
            .then_with(|| left.term.cmp(&right.term))
    });

    let mut export = FlypyExport::default();
    if chosen.len() > options.budget {
        export.truncated = chosen.len() - options.budget;
        chosen.truncate(options.budget);
    }

    let mut table = String::from(BOM);
    if options.with_header {
        table.push_str(FLYPY_HEADER);
    }
    let mut by_code: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for term in chosen {
        match term.code() {
            Ok(code) => {
                table.push_str(&term.term);
                table.push('\t');
                table.push_str(&code);
                table.push('\n');
                by_code.entry(code).or_default().push(term.term.clone());
                export.written += 1;
                export.saved_keys += term.saved_keys();
            }
            Err(error) => export.failed.push(Dropped {
                term: term.term.clone(),
                reason: error.to_string(),
            }),
        }
    }
    export.table = table;

    by_code.retain(|_, words| words.len() > 1);
    if !by_code.is_empty() {
        let mut report = String::from(BOM);
        report.push_str("# 编码\t重码词（按导出顺序，靠前的少翻页）\n");
        for (code, words) in &by_code {
            export.conflict_codes += 1;
            export.conflict_terms += words.len();
            report.push_str(code);
            report.push('\t');
            report.push_str(&words.join(" "));
            report.push('\n');
        }
        export.conflicts = report;
    }
    export
}

fn keep(term: &LexiconTerm, options: &ExportOptions) -> bool {
    if term.state == TermState::Rejected {
        return false;
    }
    if term.state == TermState::Candidate && !options.include_candidates {
        return false;
    }
    // 权威词源（标准词库、校对词表、手工）不设门槛：输入法里打出来的单位名
    // 必须和公文落款里的一致，这正是把词表做进公文助手的理由。
    if term.origin.auto_accepted() {
        return !term.term.is_empty();
    }
    if term.char_count() < options.min_chars {
        return false;
    }
    if term.doc_count < options.min_doc_count {
        return false;
    }
    if options.exclude_common && term.in_base_dict {
        return false;
    }
    true
}

/// 码表文件的建议文件名。小鹤的码表管理按文件名区分来源。
pub fn suggested_file_name() -> String {
    format!(
        "公文助手-小鹤用户码表-{}.txt",
        chrono::Local::now().format("%Y%m%d")
    )
}

pub fn suggested_conflict_file_name() -> String {
    format!(
        "公文助手-重码报告-{}.txt",
        chrono::Local::now().format("%Y%m%d")
    )
}

/// 各词源在本次导出里各占多少条，显示在导出摘要里。
pub fn origin_breakdown(
    terms: &[LexiconTerm],
    options: &ExportOptions,
) -> Vec<(TermOrigin, usize)> {
    let mut counts: BTreeMap<TermOrigin, usize> = BTreeMap::new();
    for term in terms.iter().filter(|term| keep(term, options)) {
        *counts.entry(term.origin).or_default() += 1;
    }
    counts.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(text: &str, doc_count: i64, origin: TermOrigin) -> LexiconTerm {
        LexiconTerm {
            id: 0,
            term: text.to_string(),
            pinyin: String::new(),
            code_override: String::new(),
            freq_total: doc_count * 2,
            doc_count,
            origin,
            state: if origin.auto_accepted() {
                TermState::Accepted
            } else {
                TermState::Candidate
            },
            locked: false,
            in_base_dict: false,
            group_name: String::new(),
            note: String::new(),
            first_seen: String::new(),
            last_seen: String::new(),
        }
    }

    #[test]
    fn two_char_corpus_terms_are_dropped_by_default() {
        // 二字词在小鹤里本来就是四码，收进用户码表只会多一条重码。
        let terms = vec![
            term("公文", 20, TermOrigin::Corpus),
            term("专项整治", 3, TermOrigin::Corpus),
        ];
        let export = build(&terms, &ExportOptions::default());
        assert_eq!(export.written, 1);
        assert!(export.table.contains("专项整治"));
        assert!(!export.table.contains("公文\t"));
    }

    #[test]
    fn authoritative_terms_ignore_the_thresholds() {
        // 标准词库里的两字简称没有篇数、也不够三字，但必须进码表：
        // 输入法打出来的要和公文落款里的是同一个字符串。
        let terms = vec![term("市府", 0, TermOrigin::Vocabulary)];
        let export = build(&terms, &ExportOptions::default());
        assert_eq!(export.written, 1);
    }

    #[test]
    fn budget_truncates_by_value_not_by_insertion_order() {
        let terms = vec![
            term("专项整治行动方案", 2, TermOrigin::Corpus),
            term("计算机", 9, TermOrigin::Corpus),
        ];
        let options = ExportOptions {
            budget: 1,
            ..ExportOptions::default()
        };
        let export = build(&terms, &options);
        assert_eq!(export.written, 1);
        assert_eq!(export.truncated, 1);
        // 8 字词省 12 键 × 2 篇 = 24；三字词省 2 键 × 9 篇 = 18。长词胜出。
        assert!(
            export.table.contains("专项整治行动方案"),
            "预算该留给省键数更大的长词：{}",
            export.table
        );
    }

    #[test]
    fn conflicting_codes_are_reported_in_export_order() {
        let mut first = term("甲乙丙", 9, TermOrigin::Corpus);
        first.code_override = "abcd".into();
        let mut second = term("丁戊己", 2, TermOrigin::Corpus);
        second.code_override = "abcd".into();
        let export = build(&[first, second], &ExportOptions::default());
        assert_eq!(export.conflict_codes, 1);
        assert_eq!(export.conflict_terms, 2);
        assert!(
            export.conflicts.contains("abcd\t甲乙丙 丁戊己"),
            "{}",
            export.conflicts
        );
    }

    #[test]
    fn rejected_terms_never_reach_the_table() {
        let mut rejected = term("套话连篇", 9, TermOrigin::Corpus);
        rejected.state = TermState::Rejected;
        let export = build(&[rejected], &ExportOptions::default());
        assert_eq!(export.written, 0);
    }

    #[test]
    fn encoding_failures_are_listed_instead_of_silently_dropped() {
        let mut bad = term("专项整治", 5, TermOrigin::Corpus);
        bad.pinyin = "zhuan xiang".into(); // 四个字只标了两个音节
        let export = build(&[bad], &ExportOptions::default());
        assert_eq!(export.written, 0);
        assert_eq!(export.failed.len(), 1);
        assert!(export.failed[0].reason.contains("音节"));
    }

    #[test]
    fn table_carries_bom_and_flypy_header() {
        let export = build(
            &[term("专项整治", 5, TermOrigin::Corpus)],
            &ExportOptions::default(),
        );
        assert!(export.table.starts_with('\u{feff}'));
        assert!(export.table.contains("---config@码表分类=主码-用户码表"));
        let bare = ExportOptions {
            with_header: false,
            ..ExportOptions::default()
        };
        let export = build(&[term("专项整治", 5, TermOrigin::Corpus)], &bare);
        assert!(!export.table.contains("---config@"));
    }
}
