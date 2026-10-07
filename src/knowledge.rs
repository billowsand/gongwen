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

/// 外部文件导入时的标题：取正文首个 `# ` 标题；它以冒号结尾、下一行又是标题时，
/// 是 PDF 转换把「主标题：副标题」拆成了两行，拼回去；识别得不像标题（见
/// [`is_poor_title`]）或没有标题时用文件名。
///
/// 长标题不算坏标题：论文的完整题目常有一百多字符，而文件名多是
/// `ssrn-6447919.pdf` 这种编号，换过去反而看不懂。
pub fn import_title(markdown: &str, file_stem: &str) -> String {
    let stem = file_stem.trim();
    let mut lines = markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let Some(heading) = lines
        .by_ref()
        .find_map(|line| line.strip_prefix("# ").map(clean_heading))
        .filter(|heading| !heading.is_empty())
    else {
        return if stem.is_empty() {
            "未命名公文".to_string()
        } else {
            stem.to_string()
        };
    };
    if heading.ends_with(['：', ':'])
        && let Some(subtitle) = lines
            .next()
            .filter(|line| line.starts_with('#'))
            .map(|line| clean_heading(line.trim_start_matches('#')))
        && !subtitle.is_empty()
        && subtitle.chars().count() <= 60
        && !is_poor_title(&subtitle)
    {
        return format!("{heading}{subtitle}");
    }
    if is_poor_title(&heading) && !stem.is_empty() {
        stem.to_string()
    } else {
        heading
    }
}

/// 去掉标题两端空白与 PDF 转换带出的脚注记号（`￥`、`*`、`†` 等）。
fn clean_heading(line: &str) -> String {
    line.trim()
        .trim_end_matches(['￥', '*', '†', '‡', '#'])
        .trim()
        .to_string()
}

/// 从转换稿里取出的首个 `# ` 标题是否不像一篇文档的标题。电子书、网页转出来的
/// Markdown 常把目录链接（`[](#toc.xhtml…)`）、`{.copyright_}` 这类属性块、
/// 孤立的「•」「1」排在最前面，拿来当标题整库都看不懂。
pub fn is_poor_title(title: &str) -> bool {
    let title = title.trim();
    // 链接、HTML / 属性残留。
    const MARKUP: [&str; 6] = ["](", "[[", "{", "}", "=\"", "<"];
    if MARKUP.iter().any(|mark| title.contains(mark)) {
        return true;
    }
    // 几乎没有文字（只剩标点、符号或一个字）。
    title.chars().filter(|c| c.is_alphanumeric()).count() < 2
}

/// 文档来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeSource {
    /// 外部文件：Markdown 原文，或经 `doc_import` 转换的 Word / PDF 等文档。
    /// 库里仍记作 `markdown`，与旧数据保持一致。
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

/// 待导入的一篇知识库文档（外部文档转换后或从稿件库勾选归一化而来）。
#[derive(Debug, Clone)]
pub struct KnowledgeImportItem {
    pub source: KnowledgeSource,
    pub source_manuscript_id: Option<i64>,
    pub source_path: String,
    pub kind: TemplateKind,
    pub title: String,
    pub content_markdown: String,
}

/// 索引流水线：对库内每篇指定文档切块 → jieba 分词 → 批量嵌入 → 原地替换切块。
/// 导入与索引是两步：导入只存正文（见 [`KnowledgeStore::import_document`]），这里
/// 按 id 回库取正文，文档 id 不变。每篇独立处理，单篇失败不中断整批。
/// 返回 (成功数, [(标题, 错误)])。
pub fn run_index_pipeline_with<F: Fn(usize, usize, String)>(
    db_path: PathBuf,
    config: RagConfig,
    doc_ids: Vec<i64>,
    progress: F,
) -> (usize, Vec<(String, String)>) {
    let total = doc_ids.len();
    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    let params = rag::ChunkParams::from(&config);

    let mut store = match KnowledgeStore::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            return (
                0,
                vec![("知识库".to_string(), format!("打开知识库失败：{error:#}"))],
            );
        }
    };

    for (index, id) in doc_ids.into_iter().enumerate() {
        let (title, content) = match store.get_doc_content(id) {
            Ok(Some(doc)) => doc,
            // 索引期间被删掉的文档直接跳过，不算失败。
            Ok(None) => continue,
            Err(error) => {
                failed.push((format!("#{id}"), format!("{error:#}")));
                continue;
            }
        };
        progress(index, total, title.clone());
        match index_one(&mut store, &config, &params, id, &title, &content) {
            Ok(()) => ok += 1,
            Err(error) => failed.push((title, format!("{error:#}"))),
        }
    }
    progress(total, total, String::new());
    (ok, failed)
}

/// 索引单篇：切块、分词、批量嵌入（失败重试一次），最后原地替换该篇的切块。
fn index_one(
    store: &mut KnowledgeStore,
    config: &RagConfig,
    params: &rag::ChunkParams,
    doc_id: i64,
    title: &str,
    content: &str,
) -> Result<()> {
    let chunks = rag::chunk_markdown(content, params);
    if chunks.is_empty() {
        anyhow::bail!("正文为空，没有可索引的内容");
    }
    let title = if title.trim().is_empty() {
        "未命名公文"
    } else {
        title.trim()
    };
    // 检索文本 = 标题 + 小节 + 正文。切块时 `#`/`##` 标题行只留在 section 里、
    // 不进正文，若照原样索引，按标题检索就一条都命中不了——而公文标题恰恰是
    // 信息密度最高的一句。向量与 FTS 都用这个复合文本。
    let index_texts: Vec<String> = chunks
        .iter()
        .map(|chunk| index_text(title, &chunk.section, &chunk.text))
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
    store.replace_chunks(doc_id, &new_chunks, &config.embedding.model)
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

    /// 导入一篇文档：只存正文，不切块、不嵌入（`chunk_count` 为 0、`embed_model`
    /// 为空即"待索引"）。判重同 [`Self::replace_document`]：同来源或同正文的旧文档
    /// 连同其索引一并替换，因为正文可能改过，旧索引已经对不上。
    pub fn import_document(&mut self, item: &KnowledgeImportItem) -> Result<i64> {
        let title = if item.title.trim().is_empty() {
            "未命名公文".to_string()
        } else {
            item.title.trim().to_string()
        };
        let meta = NewKnowledgeDoc {
            source: item.source,
            source_manuscript_id: item.source_manuscript_id,
            source_path: item.source_path.clone(),
            kind: item.kind,
            title,
            content_hash: content_hash(&item.content_markdown),
            content_markdown: item.content_markdown.clone(),
            embed_model: String::new(),
        };
        self.replace_document(&meta, &[])
    }

    /// 原地替换一篇文档的全部切块，并记下切块数与嵌入模型。文档 id 与创建时间不变。
    pub fn replace_chunks(
        &mut self,
        doc_id: i64,
        chunks: &[NewKnowledgeChunk],
        embed_model: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM knowledge_chunks WHERE doc_id = ?1",
            params![doc_id],
        )?;
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
        let updated = tx.execute(
            "UPDATE knowledge_docs SET chunk_count = ?1, embed_model = ?2, updated_at = ?3 WHERE id = ?4",
            params![chunks.len() as i64, embed_model, Self::now(), doc_id],
        )?;
        if updated == 0 {
            anyhow::bail!("文档已不在知识库中");
        }
        tx.commit()?;
        Ok(())
    }

    /// 尚未建立索引的文档 id（导入后还没嵌入过的）。
    pub fn unindexed_doc_ids(&mut self) -> Result<Vec<i64>> {
        self.doc_ids("SELECT id FROM knowledge_docs WHERE embed_model = '' ORDER BY id")
    }

    /// 全部文档 id（重建索引用）。
    pub fn all_doc_ids(&mut self) -> Result<Vec<i64>> {
        self.doc_ids("SELECT id FROM knowledge_docs ORDER BY id")
    }

    fn doc_ids(&mut self, sql: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 删除一批文档（chunks 级联清除，FTS 由触发器同步）。一个事务，要么全删要么不删。
    pub fn delete_documents(&mut self, ids: &[i64]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut removed = 0;
        {
            let mut stmt = tx.prepare("DELETE FROM knowledge_docs WHERE id = ?1")?;
            for id in ids {
                removed += stmt.execute(params![id])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// 用户改标题。标题拼在每个切块的检索文本里（见 `index_text`），旧索引还带着
    /// 旧标题，所以有索引的文档改名后标成待索引（`embed_model` 清空），由「建立索引」
    /// 重做；重做前旧切块照常参与检索。记下 `title_manual`，导入标题修正不再覆盖它。
    pub fn rename_document(&mut self, id: i64, title: &str) -> Result<()> {
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("标题不能为空");
        }
        let updated = self.conn.execute(
            "UPDATE knowledge_docs SET title = ?1, title_manual = 1, embed_model = '', updated_at = ?2
             WHERE id = ?3",
            params![title, Self::now(), id],
        )?;
        if updated == 0 {
            anyhow::bail!("文档已不在知识库中");
        }
        Ok(())
    }

    /// 按当前规则（[`import_title`]）重算外部文件的标题，修正早先导入时识别错的
    /// （目录链接、属性块、孤立标点、拆成两行的主副标题）。用户手改过的不动；
    /// 规则不变时重跑不改任何一篇。改了的同样标成待索引，理由见
    /// [`Self::rename_document`]。返回修正篇数。
    pub fn repair_import_titles(&mut self) -> Result<usize> {
        let candidates: Vec<(i64, String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, title, source_path, content_markdown FROM knowledge_docs
                 WHERE source = 'markdown' AND title_manual = 0",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let fixes: Vec<(i64, String)> = candidates
            .into_iter()
            .filter_map(|(id, title, path, content)| {
                let stem = Path::new(&path)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_default();
                let expected = import_title(&content, &stem);
                (expected != title).then_some((id, expected))
            })
            .collect();
        if fixes.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt =
                tx.prepare("UPDATE knowledge_docs SET title = ?1, embed_model = '' WHERE id = ?2")?;
            for (id, title) in &fixes {
                stmt.execute(params![title, id])?;
            }
        }
        tx.commit()?;
        Ok(fixes.len())
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
        s.delete_documents(&[id]).unwrap();
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

    #[test]
    fn import_then_index_keeps_doc_id() {
        let mut s = store();
        let item = KnowledgeImportItem {
            source: KnowledgeSource::Markdown,
            source_manuscript_id: None,
            source_path: "/tmp/甲函.docx".into(),
            kind: TemplateKind::OfficialLetter,
            title: "甲函".into(),
            content_markdown: "## 一、背景\n\n正文。".into(),
        };
        let id = s.import_document(&item).unwrap();
        // 导入后在列表里可见，但处于待索引状态。
        let docs = s.list_docs(None).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].chunk_count, 0);
        assert!(docs[0].embed_model.is_empty());
        assert_eq!(s.unindexed_doc_ids().unwrap(), vec![id]);

        s.replace_chunks(
            id,
            &[chunk(0, "正文。", Some(vec![1.0, 0.0]))],
            "test-embed",
        )
        .unwrap();
        let docs = s.list_docs(None).unwrap();
        assert_eq!(docs[0].id, id);
        assert_eq!(docs[0].chunk_count, 1);
        assert_eq!(docs[0].embed_model, "test-embed");
        assert!(s.unindexed_doc_ids().unwrap().is_empty());
        assert_eq!(s.count_chunks().unwrap(), 1);

        // 再索引一次是替换而不是追加。
        s.replace_chunks(
            id,
            &[chunk(0, "一", None), chunk(1, "二", None)],
            "test-embed",
        )
        .unwrap();
        assert_eq!(s.count_chunks().unwrap(), 2);

        // 重新导入同一文件会把旧索引一并清掉，回到待索引。
        let id2 = s.import_document(&item).unwrap();
        assert_eq!(s.count_chunks().unwrap(), 0);
        assert_eq!(s.unindexed_doc_ids().unwrap(), vec![id2]);
    }

    #[test]
    fn replace_chunks_on_missing_doc_fails() {
        let mut s = store();
        assert!(s.replace_chunks(999, &[], "test-embed").is_err());
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

    #[test]
    fn poor_titles_fall_back_to_file_name() {
        for bad in [
            "[]{#004_toc_Contents.xhtml#page_vii .pagebreak role=\"doc-pagebreak\"}Contents",
            "{.copyright_}",
            "•",
            "1",
            "世",
        ] {
            assert!(is_poor_title(bad), "{bad} 应判为不像标题");
            assert_eq!(
                import_title(&format!("# {bad}\n\n正文"), "文件名"),
                "文件名"
            );
        }
        // 长标题照用：论文全题常过百字符，文件名却多是 `ssrn-6447919.pdf` 这类编号。
        let long = "Information Warfare and the Contest for Strategic Truth: Implications for Security, Diplomacy";
        for good in ["关于开展安全生产检查的通知", "Cognitive Superiority", long] {
            assert!(!is_poor_title(good), "{good} 是正常标题");
            assert_eq!(
                import_title(&format!("# {good}\n\n正文"), "ssrn-6502639.pdf"),
                good
            );
        }
        // 没有 `# ` 标题时照旧用文件名。
        assert_eq!(import_title("正文", "文件名"), "文件名");
        assert_eq!(import_title("正文", ""), "未命名公文");
    }

    #[test]
    fn split_title_and_subtitle_are_joined() {
        // PDF 转换把「主标题：副标题」拆成两个标题行，副标题尾巴还挂着脚注记号。
        let md = "# 话语操控与场景传播：\n# 乌克兰危机中美国主流媒体对俄舆论战 ￥\n任 华\n\n正文";
        assert_eq!(
            import_title(md, "EAST202303004"),
            "话语操控与场景传播：乌克兰危机中美国主流媒体对俄舆论战"
        );
        // 下一行是正文就不拼，冒号标题原样保留。
        let md = "# 背景：\n这里是正文，不是副标题。";
        assert_eq!(import_title(md, "文件名"), "背景：");
    }

    #[test]
    fn rename_marks_reindex_and_survives_repair() {
        let mut s = store();
        let id = s
            .replace_document(&meta("甲函"), &[chunk(0, "一", None)])
            .unwrap();
        assert!(s.rename_document(id, "  ").is_err(), "空标题应拒绝");
        s.rename_document(id, "改过的标题：").unwrap();
        let doc = &s.list_docs(None).unwrap()[0];
        assert_eq!(doc.title, "改过的标题：");
        assert_eq!(doc.embed_model, "", "改名后应转为待索引");
        assert_eq!(doc.chunk_count, 1, "旧切块保留，重建前照常可检索");
        assert_eq!(s.unindexed_doc_ids().unwrap(), vec![id]);
        // 用户手改的标题哪怕“不像标题”，修正也不动它。
        assert_eq!(s.repair_import_titles().unwrap(), 0);
        assert!(s.rename_document(id + 100, "x").is_err());
    }

    #[test]
    fn repair_import_titles_uses_file_stem_once() {
        let mut s = store();
        let mut bad = meta("·");
        bad.source_path = "/tmp/数字冷战再审视.md".into();
        s.replace_document(&bad, &[chunk(0, "一", None)]).unwrap();
        s.replace_document(&meta("甲函"), &[chunk(0, "二", None)])
            .unwrap();
        assert_eq!(s.repair_import_titles().unwrap(), 1);
        let docs = s.list_docs(None).unwrap();
        let fixed = docs
            .iter()
            .find(|doc| doc.title == "数字冷战再审视")
            .unwrap();
        assert_eq!(fixed.embed_model, "", "修正后的标题要重建索引才进检索文本");
        let kept = docs.iter().find(|doc| doc.title == "甲函").unwrap();
        assert_eq!(kept.embed_model, "test-embed");
        assert_eq!(s.repair_import_titles().unwrap(), 0, "修正应幂等");
        // 稿件库来源的标题由稿件维护，不参与重算。
        let mut from_manuscript = meta("·");
        from_manuscript.source = KnowledgeSource::Manuscript;
        from_manuscript.source_manuscript_id = Some(7);
        from_manuscript.content_markdown = "# 稿件正文".into();
        from_manuscript.content_hash = content_hash("# 稿件正文");
        s.replace_document(&from_manuscript, &[]).unwrap();
        assert_eq!(s.repair_import_titles().unwrap(), 0);
    }

    #[test]
    fn delete_documents_removes_batch() {
        let mut s = store();
        let a = s.replace_document(&meta("甲函"), &[]).unwrap();
        let b = s.replace_document(&meta("乙函"), &[]).unwrap();
        s.replace_document(&meta("丙函"), &[]).unwrap();
        assert_eq!(s.delete_documents(&[a, b, 9999]).unwrap(), 2);
        let docs = s.list_docs(None).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "丙函");
    }
}
