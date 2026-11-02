//! 证据包与引用标记。
//!
//! 检索到的片段按到达顺序编号 [K1]、[K2]……起草与补全时让模型在用到证据的句子末尾标注
//! 编号；交付前由程序剥掉标记（公文正文里不能有），同时记下「句子 → 来源」供核实清单
//! 与核验使用。

use crate::rag::RetrievedChunk;

/// 引用标记的识别、编号抽取与交付前剥离在共享层（`ai_services::citations`）：
/// 正则只有一份，句切也用它。这里单向引用，旧流程与 Pi 共用同一套判断。
pub(crate) use crate::ai_services::citations::{
    CITATION, citation_ids, cited_sentences, normalize_citations, strip_citations,
};

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
}
