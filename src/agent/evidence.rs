//! 证据包与引用标记。
//!
//! 检索到的片段按到达顺序编号 [K1]、[K2]……起草与补全时让模型在用到证据的句子末尾标注
//! 编号；交付前由程序剥掉标记（公文正文里不能有），同时记下「句子 → 来源」供核实清单
//! 与核验使用。

use super::gaps::sentence_spans;
use crate::rag::RetrievedChunk;
use regex::Regex;
use std::sync::LazyLock;

/// 引用标记：`[K3]`、`[K3,K5]`、`[K3、K5]`、`【K3】`，前面可以有空白。
pub(crate) static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*[\[【]\s*K\s*\d+(?:\s*[,，、]\s*K?\s*\d+)*\s*[\]】]").expect("引用标记正则")
});

static CITATION_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("编号正则"));

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
}

impl EvidencePack {
    /// 并入一批检索结果，按原顺序返回每个片段的编号；已经在包里的片段沿用旧编号。
    pub(crate) fn absorb(&mut self, query: &str, chunks: &[RetrievedChunk]) -> Vec<usize> {
        let docs: Vec<EvidenceDoc> = chunks.iter().map(EvidenceDoc::from_chunk).collect();
        self.absorb_docs(query, &docs)
    }

    /// 并入任意来源的资料，按原顺序返回编号；同一个键沿用旧编号。
    pub(crate) fn absorb_docs(&mut self, query: &str, docs: &[EvidenceDoc]) -> Vec<usize> {
        docs.iter()
            .map(|doc| {
                if let Some(existing) = self.items.iter().find(|e| e.key == doc.key) {
                    return existing.id;
                }
                let id = self.items.len() + 1;
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

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
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
}
