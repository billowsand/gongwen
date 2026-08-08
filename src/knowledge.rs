//! 知识库：SQLite 存储封装。与稿件库同一个数据库文件（manuscripts.db），
//! 表结构见 `manuscript::DDL_V5`。`KnowledgeStore` 持有自己的连接打开同一文件，
//! 与 `ManuscriptStore` 解耦——知识库页、索引线程、检索线程各拿各的连接。
//!
//! 表：knowledge_docs（整篇副本）→ knowledge_chunks（切块，含向量 BLOB 与 jieba
//! 分词 tokens）→ knowledge_chunks_fts（FTS5 external-content，由触发器同步）。

use crate::manuscript::{ensure_knowledge_schema, kind_to_str, str_to_kind};
use crate::models::{RagConfig, TemplateKind};
use crate::rag::{self, vector};
use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 列表行：知识库文档的轻量展示字段。
#[derive(Debug, Clone)]
pub struct KnowledgeDocRow {
    pub id: i64,
    pub source: String,
    pub source_manuscript_id: Option<i64>,
    pub source_path: String,
    pub kind: TemplateKind,
    pub title: String,
    pub chunk_count: i64,
    pub embed_model: String,
    #[allow(dead_code)] // 预留：详情展示创建时间
    pub created_at: String,
    pub updated_at: String,
}

/// 新建/替换一篇知识库文档的元数据。
#[derive(Debug, Clone)]
pub struct NewKnowledgeDoc {
    pub source: KnowledgeSource,
    pub source_manuscript_id: Option<i64>,
    pub source_path: String,
    pub kind: TemplateKind,
    pub title: String,
    pub content_markdown: String,
    pub embed_model: String,
    /// 正文归一化后的哈希，跨来源去重用（见 `content_hash`）。
    pub content_hash: String,
}

/// 正文内容哈希：去掉所有空白后取 64 位 FNV-1a，十六进制。
///
/// 只为“同一篇内容是否已入库”服务，不做密码学用途。去空白是因为同一篇公文
/// 走稿件库和走 md 文件两条路进来时，缩进和换行常有细微差别。
pub fn content_hash(markdown: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in markdown.bytes().filter(|b| !b.is_ascii_whitespace()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// 文档来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeSource {
    /// 外部 markdown 文件。
    Markdown,
    /// 从稿件库勾选。
    Manuscript,
}

impl KnowledgeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Manuscript => "manuscript",
        }
    }

    #[allow(dead_code)] // 预留：来源展示
    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "外部",
            Self::Manuscript => "稿件",
        }
    }
}

/// 待写入的一个切块。
#[derive(Debug, Clone)]
pub struct NewKnowledgeChunk {
    pub ord: usize,
    pub section: String,
    pub text: String,
    pub tokens: String,
    /// 已编码的小端 f32 字节流；未嵌入为 None。
    pub embedding: Option<Vec<u8>>,
    pub dims: usize,
}

/// 检索回表后的完整块（供注入提示词与测试展示）。
#[derive(Debug, Clone)]
pub struct KnowledgeChunkRow {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub doc_title: String,
    pub kind: TemplateKind,
    pub section: String,
    pub text: String,
}

/// 待索引的一篇知识库文档（导入外部 md 或从稿件库勾选归一化而来）。
#[derive(Debug, Clone)]
pub struct KnowledgeImportItem {
    pub source: KnowledgeSource,
    pub source_manuscript_id: Option<i64>,
    pub source_path: String,
    pub kind: TemplateKind,
    pub title: String,
    pub content_markdown: String,
}

/// 索引流水线：对每篇文档切块 → jieba 分词 → 批量嵌入 → 整篇替换入库。
/// `rebuild` 为 true 时（换模型后）不重删文档，只逐篇重新嵌入（replace 已按来源去重）。
/// 每篇独立处理，单篇失败不中断整批。返回 (成功数, [(标题, 错误)])。
pub fn run_index_pipeline_with<F: Fn(usize, usize, String)>(
    db_path: PathBuf,
    config: RagConfig,
    items: Vec<KnowledgeImportItem>,
    progress: F,
) -> (usize, Vec<(String, String)>) {
    let total = items.len();
    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    let params = rag::ChunkParams::from(&config);

    let mut store = match KnowledgeStore::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            return (
                0,
                items
                    .into_iter()
                    .map(|item| (item.title, format!("打开知识库失败：{error:#}")))
                    .collect(),
            );
        }
    };

    for (index, item) in items.into_iter().enumerate() {
        progress(index, total, item.title.clone());
        match index_one(&mut store, &config, &params, &item) {
            Ok(()) => ok += 1,
            Err(error) => failed.push((item.title.clone(), format!("{error:#}"))),
        }
    }
    progress(total, total, String::new());
    (ok, failed)
}

/// 索引单篇：切块、分词、批量嵌入（失败重试一次），最后整篇替换。
fn index_one(
    store: &mut KnowledgeStore,
    config: &RagConfig,
    params: &rag::ChunkParams,
    item: &KnowledgeImportItem,
) -> Result<()> {
    let chunks = rag::chunk_markdown(&item.content_markdown, params);
    if chunks.is_empty() {
        anyhow::bail!("正文为空，没有可索引的内容");
    }
    let title = if item.title.trim().is_empty() {
        "未命名公文".to_string()
    } else {
        item.title.trim().to_string()
    };
    // 检索文本 = 标题 + 小节 + 正文。切块时 `#`/`##` 标题行只留在 section 里、
    // 不进正文，若照原样索引，按标题检索就一条都命中不了——而公文标题恰恰是
    // 信息密度最高的一句。向量与 FTS 都用这个复合文本。
    let index_texts: Vec<String> = chunks
        .iter()
        .map(|chunk| index_text(&title, &chunk.section, &chunk.text))
        .collect();
    let embeddings = embed_with_retry(config, &index_texts)?;
    let new_chunks: Vec<NewKnowledgeChunk> = chunks
        .into_iter()
        .enumerate()
        .map(|(ord, chunk)| {
            let (embedding, dims) = match embeddings.get(ord).and_then(|e| e.as_ref()) {
                Some(vec) => (Some(vector::encode(vec)), vec.len()),
                None => (None, 0),
            };
            NewKnowledgeChunk {
                ord,
                section: chunk.section,
                tokens: rag::tokenize(&index_texts[ord]),
                // text 保持干净的正文：注入提示词与界面展示都用它，不带标题前缀。
                text: chunk.text,
                embedding,
                dims,
            }
        })
        .collect();
    let meta = NewKnowledgeDoc {
        source: item.source,
        source_manuscript_id: item.source_manuscript_id,
        source_path: item.source_path.clone(),
        kind: item.kind,
        title,
        content_hash: content_hash(&item.content_markdown),
        content_markdown: item.content_markdown.clone(),
        embed_model: config.embedding.model.clone(),
    };
    store.replace_document(&meta, &new_chunks)?;
    Ok(())
}

/// 拼出用于检索的复合文本：标题 / 小节 / 正文。小节与标题重复时不重复拼。
fn index_text(title: &str, section: &str, text: &str) -> String {
    let mut out = String::with_capacity(title.len() + section.len() + text.len() + 8);
    out.push_str(title);
    let section = section.trim();
    if !section.is_empty() && section != title {
        out.push('\n');
        out.push_str(section);
    }
    out.push('\n');
    out.push_str(text);
    out
}

/// 分批嵌入；每批失败重试一次（间隔 2 秒），再失败则报错（该篇标错）。
fn embed_with_retry(config: &RagConfig, texts: &[String]) -> Result<Vec<Option<Vec<f32>>>> {
    let mut out: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
    let batch = config.embedding.batch_size.max(1);
    for (start, chunk_texts) in texts.chunks(batch).enumerate() {
        let offset = start * batch;
        let result = crate::rag_client::embed(&config.embedding, chunk_texts).or_else(|e| {
            std::thread::sleep(Duration::from_secs(2));
            crate::rag_client::embed(&config.embedding, chunk_texts).map_err(|_| e)
        });
        let vecs = result?;
        for (i, vec) in vecs.into_iter().enumerate() {
            out[offset + i] = Some(vec);
        }
    }
    Ok(out)
}

pub struct KnowledgeStore {
    conn: Connection,
}

impl KnowledgeStore {
    /// 打开（必要时创建）知识库所在的同一数据库文件，并确保知识库表已就绪。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).context("创建知识库目录失败")?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("无法打开知识库 {}", path.display()))?;
        // 索引线程可能正持有写事务，读连接要肯等；2 秒对本地库偏短。
        conn.busy_timeout(Duration::from_millis(10_000))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        // 知识库表幂等建立/补列；不依赖 ManuscriptStore 是否已迁移到 v5。
        ensure_knowledge_schema(&conn)?;
        Ok(Self { conn })
    }

    fn now() -> String {
        Local::now().to_rfc3339()
    }

    /// 列出文档，可按文种过滤，按更新时间倒序。
    pub fn list_docs(&mut self, kind: Option<TemplateKind>) -> Result<Vec<KnowledgeDocRow>> {
        let (sql, kind_param) = match kind {
            Some(k) => (
                "SELECT id, source, source_manuscript_id, source_path, kind, title, chunk_count, embed_model, created_at, updated_at
                 FROM knowledge_docs WHERE kind = ?1 ORDER BY updated_at DESC, id DESC",
                Some(kind_to_str(k).to_string()),
            ),
            None => (
                "SELECT id, source, source_manuscript_id, source_path, kind, title, chunk_count, embed_model, created_at, updated_at
                 FROM knowledge_docs ORDER BY updated_at DESC, id DESC",
                None,
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<KnowledgeDocRow> {
            let kind_str: String = row.get(4)?;
            Ok(KnowledgeDocRow {
                id: row.get(0)?,
                source: row.get(1)?,
                source_manuscript_id: row.get(2)?,
                source_path: row.get(3)?,
                kind: str_to_kind(&kind_str).unwrap_or_default(),
                title: row.get(5)?,
                chunk_count: row.get(6)?,
                embed_model: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        };
        let rows = match &kind_param {
            Some(k) => stmt.query_map(params![k], map_row)?,
            None => stmt.query_map([], map_row)?,
        };
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 块总数（统计展示用）。
    pub fn count_chunks(&mut self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM knowledge_chunks", [], |row| {
                row.get(0)
            })?)
    }

    /// 文档总数。
    #[allow(dead_code)] // 预留：统计展示
    pub fn count_docs(&mut self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM knowledge_docs", [], |row| row.get(0))?)
    }

    /// 整篇替换：事务内删除重复的旧文档（chunks 级联）→ 插入新文档与全部块。
    ///
    /// 判重走三条，命中任一即替换，返回新文档 id：
    /// 1. 同稿件 id（source='manuscript'）；
    /// 2. 同文件路径（source='markdown'）；
    /// 3. **同正文哈希，不分来源**——同一篇既走稿件库又走 md 文件进来时，
    ///    只按前两条各自去重是拦不住的，会整篇重复入库，检索时每个片段都出
    ///    现两遍，白白吃掉一半的融合与 rerank 名额。
    pub fn replace_document(
        &mut self,
        meta: &NewKnowledgeDoc,
        chunks: &[NewKnowledgeChunk],
    ) -> Result<i64> {
        let tx = self.conn.transaction()?;
        match meta.source {
            KnowledgeSource::Manuscript => {
                tx.execute(
                    "DELETE FROM knowledge_docs WHERE source='manuscript' AND source_manuscript_id = ?1",
                    params![meta.source_manuscript_id],
                )?;
            }
            KnowledgeSource::Markdown => {
                tx.execute(
                    "DELETE FROM knowledge_docs WHERE source='markdown' AND source_path = ?1",
                    params![meta.source_path],
                )?;
            }
        }
        if !meta.content_hash.is_empty() {
            tx.execute(
                "DELETE FROM knowledge_docs WHERE content_hash = ?1",
                params![meta.content_hash],
            )?;
        }
        let now = Self::now();
        tx.execute(
            "INSERT INTO knowledge_docs
             (source, source_manuscript_id, source_path, kind, title, content_markdown, content_hash, chunk_count, embed_model, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                meta.source.as_str(),
                meta.source_manuscript_id,
                meta.source_path,
                kind_to_str(meta.kind),
                meta.title,
                meta.content_markdown,
                meta.content_hash,
                chunks.len() as i64,
                meta.embed_model,
                now,
                now
            ],
        )?;
        let doc_id = tx.last_insert_rowid();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO knowledge_chunks (doc_id, ord, section, text, tokens, embedding, dims)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for chunk in chunks {
                stmt.execute(params![
                    doc_id,
                    chunk.ord as i64,
                    chunk.section,
                    chunk.text,
                    chunk.tokens,
                    chunk.embedding,
                    chunk.dims as i64
                ])?;
            }
        }
        tx.commit()?;
        Ok(doc_id)
    }

    /// 删除一篇文档（chunks 级联清除，FTS 由触发器同步）。
    pub fn delete_document(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM knowledge_docs WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 清空全部知识库文档（重建索引前用）。
    #[allow(dead_code)] // 预留：清空重建
    pub fn clear_all(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM knowledge_docs", [])?;
        Ok(())
    }

    /// 取全量向量用于内存算分：返回 (chunk_id, doc_id, 向量)。可按文种过滤。
    pub fn all_embeddings(
        &mut self,
        kind: Option<TemplateKind>,
    ) -> Result<Vec<(i64, i64, Vec<f32>)>> {
        let (sql, kind_param) = match kind {
            Some(k) => (
                "SELECT c.id, c.doc_id, c.embedding FROM knowledge_chunks c
                 JOIN knowledge_docs d ON d.id = c.doc_id
                 WHERE c.embedding IS NOT NULL AND d.kind = ?1",
                Some(kind_to_str(k).to_string()),
            ),
            None => (
                "SELECT c.id, c.doc_id, c.embedding FROM knowledge_chunks c
                 WHERE c.embedding IS NOT NULL",
                None,
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<(i64, i64, Vec<u8>)> {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        };
        let rows = match &kind_param {
            Some(k) => stmt.query_map(params![k], map_row)?,
            None => stmt.query_map([], map_row)?,
        };
        let mut out = Vec::new();
        for row in rows {
            let (chunk_id, doc_id, bytes) = row?;
            if let Some(vec) = vector::decode(&bytes) {
                out.push((chunk_id, doc_id, vec));
            }
        }
        Ok(out)
    }

    /// FTS5 关键词召回：query_tokens 为 jieba 分词后的空格词串，内部转成 OR 查询。
    /// 返回 (chunk_id, bm25 归一化分)，按相关度降序，最多 limit 条。
    pub fn fts_search(
        &mut self,
        query_tokens: &str,
        kind: Option<TemplateKind>,
        limit: usize,
    ) -> Result<Vec<(i64, f32)>> {
        // 词间空格在 FTS5 里是 AND，太严格；转成带引号的 OR。
        let or_query = query_tokens
            .split_whitespace()
            .filter(|t| !t.is_empty())
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        if or_query.is_empty() {
            return Ok(Vec::new());
        }
        let (sql, kind_param) = match kind {
            Some(k) => (
                "SELECT f.rowid, bm25(knowledge_chunks_fts) AS rank
                 FROM knowledge_chunks_fts f
                 JOIN knowledge_chunks c ON c.id = f.rowid
                 JOIN knowledge_docs d ON d.id = c.doc_id
                 WHERE knowledge_chunks_fts MATCH ?1 AND d.kind = ?2
                 ORDER BY rank LIMIT ?3",
                Some(kind_to_str(k).to_string()),
            ),
            None => (
                "SELECT f.rowid, bm25(knowledge_chunks_fts) AS rank
                 FROM knowledge_chunks_fts f
                 WHERE knowledge_chunks_fts MATCH ?1
                 ORDER BY rank LIMIT ?2",
                None,
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<(i64, f64)> {
            Ok((row.get(0)?, row.get(1)?))
        };
        let rows = match &kind_param {
            Some(k) => stmt.query_map(params![or_query, k, limit as i64], map_row)?,
            None => stmt.query_map(params![or_query, limit as i64], map_row)?,
        };
        let mut scored: Vec<(i64, f64)> = Vec::new();
        for row in rows {
            scored.push(row?);
        }
        // bm25 得分是负数、越小越相关；归一化成 0..1 越大越相关。
        let min = scored.iter().map(|(_, r)| *r).fold(f64::INFINITY, f64::min);
        let max = scored
            .iter()
            .map(|(_, r)| *r)
            .fold(f64::NEG_INFINITY, f64::max);
        let span = (max - min).abs();
        Ok(scored
            .into_iter()
            .map(|(id, rank)| {
                // 只有一条命中（或全部同分）时无从归一化，按满分算——
                // 此前这里取 norm=1.0 反而算出 0 分，界面上会把明明命中的
                // 片段显示成“关键词 0.000”。
                let score = if span < f64::EPSILON {
                    1.0
                } else {
                    1.0 - (rank - min) / span // rank 越小越相关 → 取反
                };
                (id, score as f32)
            })
            .collect())
    }

    /// 按 chunk_id 批量回表取正文 + 标题 + 文种 + 小节。
    pub fn chunks_by_ids(&mut self, ids: &[i64]) -> Result<Vec<KnowledgeChunkRow>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.doc_id, d.title, d.kind, c.section, c.text
             FROM knowledge_chunks c JOIN knowledge_docs d ON d.id = c.doc_id
             WHERE c.id = ?1",
        )?;
        let mut out = Vec::new();
        for id in ids {
            let row = stmt
                .query_row(params![id], |row| {
                    let kind_str: String = row.get(3)?;
                    Ok(KnowledgeChunkRow {
                        chunk_id: row.get(0)?,
                        doc_id: row.get(1)?,
                        doc_title: row.get(2)?,
                        kind: str_to_kind(&kind_str).unwrap_or_default(),
                        section: row.get(4)?,
                        text: row.get(5)?,
                    })
                })
                .optional()?;
            if let Some(row) = row {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// 取一篇文档的标题与原文（查看详情用）。
    pub fn get_doc_content(&mut self, id: i64) -> Result<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT title, content_markdown FROM knowledge_docs WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// 取一篇文档的标题、文种与原文（公文预览用）。
    pub fn get_doc_for_preview(
        &mut self,
        id: i64,
    ) -> Result<Option<(String, TemplateKind, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT title, kind, content_markdown FROM knowledge_docs WHERE id = ?1",
                params![id],
                |row| {
                    let kind_str: String = row.get(1)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        str_to_kind(&kind_str).unwrap_or_default(),
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?)
    }

    /// 已入库的稿件 id 集合，供稿件管理列表打「已入库」标记，
    /// 让用户在导入前就看得出哪些已经进过知识库。
    pub fn indexed_manuscript_ids(&mut self) -> Result<std::collections::HashSet<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT source_manuscript_id FROM knowledge_docs
             WHERE source='manuscript' AND source_manuscript_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// 库内出现过的 embedding 模型名（去重）。与当前配置比对可提示需重建索引：
    /// 换模型后维度多半不同，旧块的余弦恒为 0，会**静默**退出向量召回。
    pub fn distinct_embed_models(&mut self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT embed_model FROM knowledge_docs WHERE embed_model <> ''")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> KnowledgeStore {
        // 用内存库跑，避免落盘；建表与补列在 ensure_knowledge_schema 里幂等完成。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        ensure_knowledge_schema(&conn).unwrap();
        KnowledgeStore { conn }
    }

    fn meta(title: &str) -> NewKnowledgeDoc {
        let content = format!("# {title}\n\n正文。");
        NewKnowledgeDoc {
            source: KnowledgeSource::Markdown,
            source_manuscript_id: None,
            source_path: format!("/tmp/{title}.md"),
            kind: TemplateKind::OfficialLetter,
            title: title.into(),
            content_hash: content_hash(&content),
            content_markdown: content,
            embed_model: "test-embed".into(),
        }
    }

    fn chunk(ord: usize, text: &str, vec: Option<Vec<f32>>) -> NewKnowledgeChunk {
        let (embedding, dims) = match &vec {
            Some(v) => (Some(vector::encode(v)), v.len()),
            None => (None, 0),
        };
        NewKnowledgeChunk {
            ord,
            section: "正文".into(),
            text: text.into(),
            tokens: crate::rag::tokenize(text),
            embedding,
            dims,
        }
    }

    #[test]
    fn replace_document_roundtrip_and_dedup() {
        let mut s = store();
        let id = s
            .replace_document(
                &meta("甲函"),
                &[chunk(0, "关于开展工作的通知", Some(vec![1.0, 0.0]))],
            )
            .unwrap();
        assert!(id > 0);
        assert_eq!(s.count_docs().unwrap(), 1);
        assert_eq!(s.count_chunks().unwrap(), 1);
        let docs = s.list_docs(None).unwrap();
        assert_eq!(docs[0].title, "甲函");
        assert_eq!(docs[0].chunk_count, 1);
        // 同路径再次导入 → 去重替换，不新增。
        s.replace_document(&meta("甲函"), &[chunk(0, "改后的正文", None)])
            .unwrap();
        assert_eq!(s.count_docs().unwrap(), 1, "同路径应去重替换");
    }

    #[test]
    fn delete_cascades_chunks() {
        let mut s = store();
        let id = s
            .replace_document(&meta("乙函"), &[chunk(0, "一", None), chunk(1, "二", None)])
            .unwrap();
        assert_eq!(s.count_chunks().unwrap(), 2);
        s.delete_document(id).unwrap();
        assert_eq!(s.count_chunks().unwrap(), 0, "删文档应级联清块");
    }

    #[test]
    fn fts_trigger_sync_and_search() {
        let mut s = store();
        s.replace_document(
            &meta("丙函"),
            &[
                chunk(0, "关于开展安全生产大检查的通知", None),
                chunk(1, "关于组织义务植树活动的函", None),
            ],
        )
        .unwrap();
        let hits = s
            .fts_search(&crate::rag::tokenize("安全生产"), None, 10)
            .unwrap();
        assert_eq!(hits.len(), 1, "只有一块命中“安全生产”");
        let rows = s.chunks_by_ids(&[hits[0].0]).unwrap();
        assert!(rows[0].text.contains("安全生产"));
    }

    #[test]
    fn all_embeddings_decodes() {
        let mut s = store();
        s.replace_document(
            &meta("丁函"),
            &[chunk(0, "向量化块", Some(vec![0.5, 0.5, 0.5]))],
        )
        .unwrap();
        let embs = s.all_embeddings(None).unwrap();
        assert_eq!(embs.len(), 1);
        assert_eq!(embs[0].2, vec![0.5, 0.5, 0.5]);
    }

    /// 回归：同一篇内容既从稿件库、又从 md 文件进来时必须只留一份。
    /// 此前两种来源各按各的键去重，互不可见，整篇会重复入库。
    #[test]
    fn same_content_from_both_sources_is_deduped() {
        let mut s = store();
        let content = "# 关于报送情况的函\n\n各单位：请于月底前报送。";
        let mut from_file = meta("关于报送情况的函");
        from_file.content_markdown = content.into();
        from_file.content_hash = content_hash(content);
        s.replace_document(&from_file, &[chunk(0, "各单位：请于月底前报送。", None)])
            .unwrap();

        // 同一篇内容改走稿件库导入：路径/稿件 id 都对不上，只有正文哈希能拦住。
        let mut from_manuscript = from_file.clone();
        from_manuscript.source = KnowledgeSource::Manuscript;
        from_manuscript.source_manuscript_id = Some(7);
        from_manuscript.source_path = String::new();
        s.replace_document(
            &from_manuscript,
            &[chunk(0, "各单位：请于月底前报送。", None)],
        )
        .unwrap();

        assert_eq!(s.count_docs().unwrap(), 1, "同正文跨来源应只留一份");
        assert_eq!(s.count_chunks().unwrap(), 1, "重复的块也应被级联清掉");
        assert_eq!(s.list_docs(None).unwrap()[0].source, "manuscript");
    }

    /// 缩进/换行的细微差别不应让同一篇内容被当成两篇。
    #[test]
    fn content_hash_ignores_whitespace() {
        assert_eq!(
            content_hash("# 甲\n\n正文。"),
            content_hash("#   甲\n正文。 ")
        );
        assert_ne!(
            content_hash("# 甲\n\n正文。"),
            content_hash("# 甲\n\n正文!")
        );
    }

    #[test]
    fn indexed_manuscript_ids_reports_imported_manuscripts() {
        let mut s = store();
        let mut m = meta("稿件来的函");
        m.source = KnowledgeSource::Manuscript;
        m.source_manuscript_id = Some(42);
        s.replace_document(&m, &[chunk(0, "正文内容", None)])
            .unwrap();
        let ids = s.indexed_manuscript_ids().unwrap();
        assert!(ids.contains(&42));
        assert!(!ids.contains(&43));
    }

    /// 单条命中时不该被归一化成 0 分（界面会显示成“关键词 0.000”）。
    #[test]
    fn fts_single_hit_scores_full_marks() {
        let mut s = store();
        s.replace_document(
            &meta("戊函"),
            &[chunk(0, "关于开展安全生产大检查的通知", None)],
        )
        .unwrap();
        let hits = s
            .fts_search(&crate::rag::tokenize("安全生产"), None, 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            (hits[0].1 - 1.0).abs() < 1e-6,
            "单条命中应为满分，实际 {}",
            hits[0].1
        );
    }

    #[test]
    fn kind_filter_narrows_results() {
        let mut s = store();
        s.replace_document(&meta("公函"), &[chunk(0, "公函内容", None)])
            .unwrap();
        let mut m2 = meta("白头");
        m2.kind = TemplateKind::WhitePaper;
        m2.source_path = "/tmp/白头.md".into();
        s.replace_document(&m2, &[chunk(0, "白头内容", None)])
            .unwrap();
        let only_letter = s.list_docs(Some(TemplateKind::OfficialLetter)).unwrap();
        assert_eq!(only_letter.len(), 1);
        assert_eq!(only_letter[0].title, "公函");
    }
}
