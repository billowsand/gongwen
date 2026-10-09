//! 证据包与引用标记。
//!
//! 检索到的片段按到达顺序编号 [K1]、[K2]……起草与补全时让模型在用到证据的句子末尾标注
//! 编号；交付前由程序剥掉标记（公文正文里不能有），同时记下「句子 → 来源」供核实清单
//! 与核验使用。

use super::gaps::sentence_spans;
use crate::rag::RetrievedChunk;
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

/// 一段证据。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Evidence {
    /// K 编号，从 1 起。
    pub(crate) id: usize,
    /// 去重键：`kb:<片段 id>`、`kbdoc:<文档 id>`、`ms:<稿件 id>:<版本>`……同一份资料只编一个号。
    pub(crate) key: String,
    pub(crate) doc_title: String,
    pub(crate) section: String,
    pub(crate) kind_label: String,
    pub(crate) text: String,
    /// 是哪个检索词把它找回来的。
    pub(crate) query: String,
}

impl Evidence {
    /// 「《甲》· 第二部分」。
    pub(crate) fn source_label(&self) -> String {
        if self.section.trim().is_empty() {
            format!("《{}》", self.doc_title)
        } else {
            format!("《{}》· {}", self.doc_title, self.section.trim())
        }
    }
}

/// 一份待并入证据包的资料（工具产出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EvidenceDoc {
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) section: String,
    pub(crate) kind_label: String,
    pub(crate) text: String,
}

impl EvidenceDoc {
    pub(crate) fn from_chunk(chunk: &RetrievedChunk) -> Self {
        Self {
            key: format!("kb:{}", chunk.chunk_id),
            title: chunk.doc_title.clone(),
            section: chunk.section.clone(),
            kind_label: chunk.kind.label().to_string(),
            text: chunk.text.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct EvidencePack {
    items: Vec<Evidence>,
    /// 用户在证据取舍里剔掉的资料（按文件题名，同一份文件的各段一起算）：之后再检索到也不
    /// 并入（`docs/decision-modules.md` 9.5）。
    #[serde(default)]
    excluded: Vec<String>,
    /// 用过的最大编号。编号只增不减：剔掉或截掉之后再并入的不会和已有编号撞上。
    #[serde(default)]
    next: usize,
}

impl EvidencePack {
    /// 并入一批检索结果，按原顺序返回每个片段的编号；已经在包里的片段沿用旧编号。
    pub(crate) fn absorb(&mut self, query: &str, chunks: &[RetrievedChunk]) -> Vec<usize> {
        let docs: Vec<EvidenceDoc> = chunks.iter().map(EvidenceDoc::from_chunk).collect();
        self.absorb_docs(query, &docs)
    }

    /// 并入任意来源的资料，按原顺序返回编号；同一个键沿用旧编号。用户剔掉的文件跳过、不占
    /// 编号——要按「资料与编号一一对应」用返回值的，先用 [`Self::is_excluded`] 滤掉再并入。
    pub(crate) fn absorb_docs(&mut self, query: &str, docs: &[EvidenceDoc]) -> Vec<usize> {
        let docs: Vec<&EvidenceDoc> = docs.iter().filter(|doc| !self.is_excluded(doc)).collect();
        docs.into_iter()
            .map(|doc| {
                if let Some(existing) = self.items.iter().find(|e| e.key == doc.key) {
                    return existing.id;
                }
                // 旧检查点里没有 `next`：从现有最大编号接着编。
                let largest = self.items.iter().map(|e| e.id).max().unwrap_or(0);
                let id = self.next.max(largest) + 1;
                self.next = id;
                self.items.push(Evidence {
                    id,
                    key: doc.key.clone(),
                    doc_title: doc.title.clone(),
                    section: doc.section.clone(),
                    kind_label: doc.kind_label.clone(),
                    text: doc.text.clone(),
                    query: query.to_string(),
                });
                id
            })
            .collect()
    }

    pub(crate) fn items(&self) -> &[Evidence] {
        &self.items
    }

    /// 用户在证据取舍里剔掉过这份文件没有。
    pub(crate) fn is_excluded(&self, doc: &EvidenceDoc) -> bool {
        self.excluded.contains(&doc.title)
    }

    /// 证据取舍：只留题名在 `keep` 里的文件（编号在 `pinned` 里的——用户 `@` 引用的——一律留），
    /// 其余剔出去并记下题名（之后不再并入），返回剔掉几份文件。
    pub(crate) fn exclude_except(&mut self, keep: &[String], pinned: &[usize]) -> usize {
        let mut dropped: Vec<String> = Vec::new();
        self.items.retain(|item| {
            let kept = keep.contains(&item.doc_title) || pinned.contains(&item.id);
            if !kept && !dropped.contains(&item.doc_title) {
                dropped.push(item.doc_title.clone());
            }
            kept
        });
        let count = dropped.len();
        for title in dropped {
            if !self.excluded.contains(&title) {
                self.excluded.push(title);
            }
        }
        count
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 只留最近 `keep` 条（检查点超尺寸上限时截证据包；编号保持原样，允许断号）。
    pub(crate) fn keep_last(&mut self, keep: usize) {
        if self.items.len() > keep {
            self.items.drain(..self.items.len() - keep);
        }
    }

    pub(crate) fn get(&self, id: usize) -> Option<&Evidence> {
        self.items.iter().find(|item| item.id == id)
    }

    pub(crate) fn all_ids(&self) -> Vec<usize> {
        self.items.iter().map(|item| item.id).collect()
    }

    /// 拼成给模型看的证据节，超过 `max_chars` 就截断，最后一段截在半截也照样标明。
    pub(crate) fn format(&self, ids: &[usize], max_chars: usize) -> String {
        let mut out = String::new();
        let mut used = 0usize;
        for id in ids {
            let Some(item) = self.get(*id) else {
                continue;
            };
            let head = format!(
                "--- [K{}] {}{} ---\n",
                item.id,
                item.kind_label,
                item.source_label()
            );
            let budget = max_chars.saturating_sub(used);
            if budget == 0 {
                break;
            }
            let body: String = item.text.chars().take(budget).collect();
            let cut = body.chars().count() < item.text.chars().count();
            used += body.chars().count();
            out.push_str(&head);
            out.push_str(&body);
            if cut {
                out.push_str("……（截断）");
            }
            out.push('\n');
        }
        out
    }

    /// 这些编号的证据原文连在一起，用来判断某个事实在不在证据里。
    pub(crate) fn text_of(&self, ids: &[usize]) -> String {
        ids.iter()
            .filter_map(|id| self.get(*id))
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
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

/// 测试用的片段构造（工具层的测试也要用）。
#[cfg(test)]
pub(crate) mod tests_support {
    use crate::models::TemplateKind;
    use crate::rag::RetrievedChunk;

    pub(crate) fn chunk(id: i64, title: &str, text: &str) -> RetrievedChunk {
        RetrievedChunk {
            chunk_id: id,
            doc_id: 1,
            doc_title: title.into(),
            kind: TemplateKind::PlainDocument,
            section: String::new(),
            text: text.into(),
            vector_score: 0.0,
            bm25_score: 0.0,
            fused_score: 0.0,
            rerank_score: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateKind;

    pub(crate) fn chunk(id: i64, title: &str, text: &str) -> RetrievedChunk {
        RetrievedChunk {
            chunk_id: id,
            doc_id: 1,
            doc_title: title.into(),
            kind: TemplateKind::PlainDocument,
            section: String::new(),
            text: text.into(),
            vector_score: 0.0,
            bm25_score: 0.0,
            fused_score: 0.0,
            rerank_score: None,
        }
    }

    #[test]
    fn absorbing_numbers_new_chunks_and_reuses_old_numbers() {
        let mut pack = EvidencePack::default();
        assert_eq!(
            pack.absorb("甲", &[chunk(10, "A", "a"), chunk(11, "B", "b")]),
            [1, 2]
        );
        assert_eq!(
            pack.absorb("乙", &[chunk(11, "B", "b"), chunk(12, "C", "c")]),
            [2, 3]
        );
        assert_eq!(pack.items().len(), 3);
        assert_eq!(pack.get(3).unwrap().query, "乙");
    }

    #[test]
    fn format_labels_sources_and_respects_the_budget() {
        let mut pack = EvidencePack::default();
        pack.absorb(
            "q",
            &[chunk(1, "条例", "一二三四五六"), chunk(2, "办法", "七八九")],
        );
        let text = pack.format(&[1, 2], 8);
        assert!(
            text.contains("--- [K1] 普通公文《条例》 ---\n一二三四五六\n"),
            "{text}"
        );
        assert!(text.contains("[K2]"));
        assert!(text.contains("七八……（截断）"), "{text}");
    }

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
