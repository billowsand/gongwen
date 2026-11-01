//! 独立的本地全文索引，不改写向量切块与 embedding。

use super::{KnowledgeStore, kind_to_str, rag, str_to_kind};
use crate::models::TemplateKind;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use std::ops::Range;

/// 本地全文结果：摘要及其原文字节起点；高亮范围相对摘要。
#[derive(Debug, Clone)]
pub struct LocalSearchHit {
    pub doc_id: i64,
    pub title: String,
    pub kind: TemplateKind,
    pub excerpt: String,
    pub highlights: Vec<Range<usize>>,
    pub source_offset: usize,
}

impl KnowledgeStore {
    /// 全文索引独立存储；读、分词、写处于同一事务，避免改名与后台补建交错。
    pub fn index_local_document(&mut self, doc_id: i64) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let document = tx
            .query_row(
                "SELECT title, content_markdown FROM knowledge_docs WHERE id = ?1",
                params![doc_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((title, content)) = document else {
            return Ok(false);
        };
        let tokens = index_tokens(&format!("{title}\n{content}"));
        tx.execute(
            "DELETE FROM knowledge_local_fts WHERE rowid = ?1",
            params![doc_id],
        )?;
        tx.execute(
            "INSERT INTO knowledge_local_fts(rowid, tokens) VALUES (?1, ?2)",
            params![doc_id, tokens],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// 找出缺失或因正文/标题变动而失效的全文索引。
    pub fn unindexed_local_doc_ids(&mut self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.id FROM knowledge_docs d LEFT JOIN knowledge_local_fts f ON f.rowid = d.id WHERE f.rowid IS NULL ORDER BY d.id",
        )?;
        Ok(stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 普通关键词安全转义；空格分隔的关键词同时匹配，同一关键词内允许部分词命中。
    /// 双引号短语在 SQL 中按原文核验后再 LIMIT，避免先截断候选导致漏搜。
    pub fn local_search(
        &mut self,
        query: &str,
        kind: Option<TemplateKind>,
        limit: usize,
    ) -> Result<Vec<LocalSearchHit>> {
        let trimmed = query.trim();
        if trimmed.chars().count() > 256 {
            bail!("检索词不能超过256个字符");
        }
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let phrase = trimmed
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .filter(|s| !s.is_empty());
        let groups: Vec<&str> = if let Some(phrase) = phrase {
            vec![phrase]
        } else {
            trimmed.split_whitespace().collect()
        };
        if groups.len() > 32 {
            bail!("一次最多检索32组关键词");
        }
        let mut expressions = Vec::new();
        let mut highlights = Vec::new();
        for group in groups {
            let mut terms = query_terms(group);
            if phrase.is_some() {
                // 短语召回只依赖原文二元组，避免上下文或用户词典改变 jieba 切法。
                terms.retain(|term| !term.chars().all(is_han) || term.chars().count() <= 2);
            }
            if terms.is_empty() {
                continue;
            }
            let quoted: Vec<_> = terms
                .iter()
                .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
                .collect();
            expressions.push(format!(
                "({})",
                quoted.join(if phrase.is_some() || !group.chars().any(is_han) {
                    " AND "
                } else {
                    " OR "
                })
            ));
            highlights.extend(terms);
        }
        if expressions.is_empty() {
            return Ok(Vec::new());
        }
        let expression = expressions.join(" AND ");
        let mut stmt = self.conn.prepare(
            "SELECT d.id, d.title, d.kind
             FROM knowledge_local_fts f JOIN knowledge_docs d ON d.id = f.rowid
             WHERE knowledge_local_fts MATCH ?1 AND (?2 IS NULL OR d.kind = ?2)
               AND (?3 IS NULL OR instr(lower(d.content_markdown), lower(?3)) > 0 OR instr(lower(d.title), lower(?3)) > 0)
             ORDER BY bm25(knowledge_local_fts), d.id LIMIT ?4",
        )?;
        let candidates = stmt
            .query_map(
                params![
                    expression,
                    kind.map(kind_to_str),
                    phrase,
                    limit.clamp(1, 100) as i64
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut hits = Vec::new();
        for (doc_id, title, kind) in candidates {
            let content = self
                .conn
                .query_row(
                    "SELECT content_markdown FROM knowledge_docs WHERE id = ?1",
                    params![doc_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(content) = content else {
                continue;
            };
            let terms = if let Some(phrase) = phrase {
                vec![phrase.to_string()]
            } else {
                highlights.clone()
            };
            let (excerpt, ranges, source_offset) = excerpt(&content, &terms);
            hits.push(LocalSearchHit {
                doc_id,
                title,
                kind: str_to_kind(&kind).unwrap_or_default(),
                excerpt,
                highlights: ranges,
                source_offset,
            });
        }
        Ok(hits)
    }
}

fn is_han(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0x20000..=0x323af)
}

/// jieba 主词项保留词频排序；连续汉字另补单字/二元组，支持专名内部与跨分词边界查询。
fn index_tokens(text: &str) -> String {
    let mut tokens = rag::tokenize(text);
    let mut previous = None;
    for ch in text.chars() {
        if is_han(ch) {
            tokens.push(' ');
            tokens.push(ch);
            if let Some(previous) = previous {
                tokens.push(' ');
                tokens.push(previous);
                tokens.push(ch);
            }
            previous = Some(ch);
        } else {
            previous = None;
        }
    }
    tokens
}

fn query_terms(text: &str) -> Vec<String> {
    let mut terms: Vec<String> = rag::tokenize(text)
        .split_whitespace()
        .flat_map(|word| word.split(|ch: char| !ch.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut run = Vec::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if is_han(ch) {
            run.push(ch);
        } else {
            if run.len() == 1 {
                terms.push(run[0].to_string());
            }
            for pair in run.windows(2) {
                terms.push(pair.iter().collect());
            }
            run.clear();
        }
    }
    terms.sort();
    terms.dedup();
    // 多字关键词不把“的、在”等单字当作独立 OR 条件，否则会召回无关材料。
    if text.chars().filter(|ch| is_han(*ch)).count() > 1 {
        terms.retain(|term| !(term.chars().count() == 1 && term.chars().all(is_han)));
    }
    terms.truncate(128);
    terms
}

/// 范围从原文计算并合并重叠，绝不使用 FTS 分词字符串的偏移。
fn excerpt(content: &str, terms: &[String]) -> (String, Vec<Range<usize>>, usize) {
    let folded = content.to_ascii_lowercase();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for term in terms {
        for (start, matched) in folded.match_indices(&term.to_ascii_lowercase()).take(256) {
            ranges.push(start..start + matched.len());
        }
    }
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    let first = merged.first().cloned().unwrap_or(0..0);
    let start = content[..first.start]
        .char_indices()
        .rev()
        .nth(80)
        .map_or(0, |(index, _)| index);
    let end = content[first.end..]
        .char_indices()
        .nth(160)
        .map_or(content.len(), |(index, _)| first.end + index);
    let excerpt = content[start..end].to_string();
    let visible = merged
        .into_iter()
        .filter(|range| range.start >= start && range.end <= end)
        .map(|range| range.start - start..range.end - start)
        .collect();
    (excerpt, visible, start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgeImportItem, KnowledgeSource};

    #[test]
    fn local_fulltext_rebuild_preserves_existing_vectors() {
        use crate::knowledge::{NewKnowledgeChunk, NewKnowledgeDoc};
        let directory = tempfile::tempdir().unwrap();
        let mut store = KnowledgeStore::open(&directory.path().join("vectors.db")).unwrap();
        let meta = NewKnowledgeDoc {
            source: KnowledgeSource::Markdown,
            source_manuscript_id: None,
            source_path: "旧材料.md".into(),
            kind: TemplateKind::OfficialLetter,
            title: "原题名".into(),
            content_markdown: "原正文".into(),
            embed_model: "existing-model".into(),
            content_hash: String::new(),
        };
        let chunks = [NewKnowledgeChunk {
            ord: 0,
            section: "正文".into(),
            text: "原正文".into(),
            tokens: "原 正文".into(),
            embedding: Some(crate::rag::vector::encode(&[0.25, 0.75])),
            dims: 2,
        }];
        let id = store.replace_document(&meta, &chunks).unwrap();
        let before = store.all_embeddings(None).unwrap();
        store.index_local_document(id).unwrap();
        store.index_local_document(id).unwrap();
        assert_eq!(store.all_embeddings(None).unwrap(), before);
        assert_eq!(store.count_chunks().unwrap(), 1);
        store.replace_chunks(id, &chunks, "existing-model").unwrap();
        assert_eq!(store.local_search("\"原正文\"", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn local_fulltext_phrase_limit_title_and_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("local.db");
        let mut store = KnowledgeStore::open(&path).unwrap();
        for n in 0..12 {
            let item = KnowledgeImportItem {
                source: KnowledgeSource::Markdown,
                source_manuscript_id: None,
                source_path: format!("{n}.md"),
                kind: TemplateKind::OfficialLetter,
                title: if n == 11 {
                    "仅标题存在稀有词".into()
                } else {
                    format!("通知{n}")
                },
                content_markdown: if n == 11 {
                    "安全检查。".into()
                } else {
                    "检查后落实安全措施。".into()
                },
            };
            let id = store.import_document(&item).unwrap();
            store.index_local_document(id).unwrap();
        }
        assert_eq!(
            store.local_search("\"安全检查\"", None, 1).unwrap().len(),
            1
        );
        let hits = store.local_search("\"稀有词\"", None, 1).unwrap();
        assert_eq!(hits.len(), 1, "只有标题命中也应返回原文摘要");
        assert!(hits[0].highlights.is_empty());
        drop(store);
        let mut store = KnowledgeStore::open(&path).unwrap();
        assert!(store.unindexed_local_doc_ids().unwrap().is_empty());
        assert_eq!(
            store.local_search("\"安全检查\"", None, 1).unwrap().len(),
            1
        );
        assert!(
            store
                .local_search("\"安全检查\"", Some(TemplateKind::WhitePaper), 1)
                .unwrap()
                .is_empty()
        );
        assert!(store.local_search(&"中".repeat(257), None, 1).is_err());
    }

    #[test]
    fn local_fulltext_excerpt_merges_overlaps_and_keeps_original_offsets() {
        let text = format!("{}原文安全生产检查 GW-2026。", "正文。".repeat(100));
        let (snippet, ranges, offset) = excerpt(
            &text,
            &["安全".into(), "安全生产".into(), "生产".into(), "gw".into()],
        );
        assert!(offset > 0);
        assert_eq!(&text[offset..offset + snippet.len()], snippet);
        assert!(ranges.windows(2).all(|pair| pair[0].end < pair[1].start));
        assert!(
            ranges
                .iter()
                .any(|range| &snippet[range.clone()] == "安全生产")
        );
        assert!(ranges.iter().any(|range| &snippet[range.clone()] == "GW"));
    }
}
