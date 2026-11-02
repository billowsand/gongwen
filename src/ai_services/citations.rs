//! 引用标记：`[K3]`、`【K3】` 的识别、编号抽取与交付前剥离。
//!
//! 从旧 `agent::evidence` 搬来，行为与正则一字未改。旧 `evidence` 单向引用这里，句切
//! （`ai_services::text::sentence_spans`）也用它，共享层不再反向依赖 `agent::evidence`。

use super::text::sentence_spans;
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

/// 引用标记：`[K3]`、`[K3,K5]`、`[K3、K5]`、`[K3; K5]`、`【K3】`，前面可以有空白。
pub(crate) static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*[\[【]\s*K\s*\d+(?:\s*[,，、;；]\s*K?\s*\d+)*\s*[\]】]").expect("引用标记正则")
});

static CITATION_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("编号正则"));

/// 交付正文时识别 AI 工作稿里的 `[@K2]` 来源标识：临时归为证据语法，随后
/// 与 `[K2]` 一起剥离并记录来源编号。仅用于交付边界，不回写工作稿。
/// 文献库中真实存在的键优先按文献保留；其他未知文献键仍交给文献校验。
pub(crate) fn normalize_citations<'a>(text: &'a str, bib: &str) -> Cow<'a, str> {
    if !text.contains("[@") {
        return Cow::Borrowed(text);
    }
    let known = crate::export::crossref::bibtex_keys(bib);
    let mut out = String::new();
    let mut last = 0;
    for mark in crate::export::crossref::citation_marks(text, &|_| false) {
        let (mut keys, mut ids) = (Vec::new(), Vec::new());
        for key in mark.keys {
            let number = key.strip_prefix('K').filter(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            });
            if !known.iter().any(|entry| entry == key)
                && let Some(number) = number
            {
                ids.push(format!("K{number}"));
            } else {
                keys.push(key);
            }
        }
        if ids.is_empty() {
            continue;
        }
        out.push_str(&text[last..mark.range.start]);
        if !keys.is_empty() {
            out.push_str(&format!("[@{}]", keys.join("; @")));
        }
        out.push_str(&format!("[{}]", ids.join(",")));
        last = mark.range.end;
    }
    if last == 0 {
        Cow::Borrowed(text)
    } else {
        out.push_str(&text[last..]);
        Cow::Owned(out)
    }
}

/// 一段文字里引用到的证据编号（去重，保持出现顺序）。
pub(crate) fn citation_ids(text: &str) -> Vec<usize> {
    let mut ids = Vec::new();
    for found in CITATION.find_iter(text) {
        for number in CITATION_NUMBER.find_iter(found.as_str()) {
            if let Ok(id) = number.as_str().parse::<usize>()
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    ids
}

/// 剥掉全部引用标记。
pub(crate) fn strip_citations(text: &str) -> String {
    CITATION.replace_all(text, "").into_owned()
}

/// 所有 AI 正文提案共用的交付清理，不改 AI 工作稿，也不碰真实文献引用。
pub(crate) fn delivery_markdown(text: &str, bib: &str) -> String {
    strip_citations(&normalize_citations(text, bib))
}

/// 带引用标记的句子：(原句，含标记；引用到的编号)。
pub(crate) fn cited_sentences(text: &str) -> Vec<(String, Vec<usize>)> {
    sentence_spans(text)
        .into_iter()
        .filter_map(|span| {
            let sentence = &text[span];
            let ids = citation_ids(sentence);
            (!ids.is_empty()).then(|| (sentence.to_string(), ids))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_are_read_and_stripped_in_every_shape() {
        let text = "已覆盖全部林区[K3]。依据《条例》【K1、K2】 执行 [K3, K5]。";
        assert_eq!(citation_ids(text), [3, 1, 2, 5]);
        assert_eq!(strip_citations(text), "已覆盖全部林区。依据《条例》 执行。");
        let cited = cited_sentences(text);
        assert_eq!(cited.len(), 2);
        assert_eq!(cited[1].1, [1, 2, 3, 5]);
    }

    #[test]
    fn semicolon_evidence_groups_are_removed_at_delivery() {
        let text = "甲[K1; K2]，乙【K3；4】。";
        assert_eq!(citation_ids(text), [1, 2, 3, 4]);
        assert_eq!(delivery_markdown(text, ""), "甲，乙。");
    }

    #[test]
    fn miswritten_evidence_is_normalized_without_losing_real_bibliography_keys() {
        let bib = "@book{K2, title = {真实文献}}";
        let text = "甲[@K1]乙[@K2]丙[@real; @K3; @K2; @missing]丁[@K4; @K5]。";
        let normalized = normalize_citations(text, bib);
        assert_eq!(
            normalized,
            "甲[K1]乙[@K2]丙[@real; @K2; @missing][K3]丁[K4,K5]。"
        );
        assert_eq!(citation_ids(&normalized), [1, 3, 4, 5]);
        assert_eq!(
            strip_citations(&normalized),
            "甲乙[@K2]丙[@real; @K2; @missing]丁。"
        );
        assert!(matches!(
            normalize_citations("真实引用[@K2]和[@unknown]、[@K2a]。", bib),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn literal_citations_in_inline_code_links_and_escapes_are_not_normalized() {
        let text = r"`[@K1]` [@K2](https://example.com) \[@K3] [^n]:(例子[@K4])";
        assert_eq!(normalize_citations(text, ""), text);
    }
}
