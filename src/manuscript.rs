//! 稿件库：SQLite 存储与 CRUD。
//!
//! 生命周期状态（`ManuscriptStatus`）决定可编辑性：归档是唯一终态，归档后
//! `update`/`set_status`/`delete` 一律拒绝，保证标题、日期、正文等关键信息不可再变；
//! PDF 附件是独立表，归档后仍可增删（附件不属于“稿件信息”）。
//! 时间戳统一 RFC3339，`updated_at` 可直接按字典序排序。
//!
//! `New`（新建）是历史遗留状态：旧版首次保存落为“新建”，现已在创建与 v2 迁移时
//! 统一升为“草稿”，不再产生新的“新建”记录（枚举保留仅为解析旧库与旧 ZIP 包）。

use crate::export::extract_title;
use crate::models::{AppConfig, DraftInput, ManuscriptStatus, TemplateKind};
use anyhow::{Context, Result, bail};
use chrono::Local;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// schema 版本 1：稿件表 + PDF 附件表。
const DDL_V1: &str = r#"
CREATE TABLE IF NOT EXISTS manuscripts (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 全量 DraftInput 快照（JSON），打开续编/归档查阅时整体还原
    snapshot_json    TEXT    NOT NULL,
    -- 审校稿正文（即起草页右侧的 markdown）
    content_markdown TEXT    NOT NULL DEFAULT '',
    notes            TEXT    NOT NULL DEFAULT '',
    -- 反规范化列，用于列表与过滤（由 snapshot + content 派生，见 create/update）
    title            TEXT    NOT NULL DEFAULT '',
    kind             TEXT    NOT NULL DEFAULT 'OfficialLetter',
    status           TEXT    NOT NULL DEFAULT 'New',
    doc_number       TEXT    NOT NULL DEFAULT '',
    doc_date         TEXT    NOT NULL DEFAULT '',
    doc_date_iso     TEXT    NOT NULL DEFAULT '',
    -- 导入时保留的源库 id，用于“跳过已有记录”的去重
    source_id        INTEGER,
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL,
    published_at     TEXT,
    archived_at      TEXT
);
CREATE INDEX IF NOT EXISTS idx_manuscripts_status  ON manuscripts(status);
CREATE INDEX IF NOT EXISTS idx_manuscripts_kind    ON manuscripts(kind);
CREATE INDEX IF NOT EXISTS idx_manuscripts_date    ON manuscripts(doc_date_iso);
CREATE INDEX IF NOT EXISTS idx_manuscripts_updated ON manuscripts(updated_at);
CREATE INDEX IF NOT EXISTS idx_manuscripts_source  ON manuscripts(source_id) WHERE source_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS pdf_attachments (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    manuscript_id INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    file_name     TEXT    NOT NULL,
    bytes         BLOB    NOT NULL,
    added_at      TEXT    NOT NULL,
    sort_order    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_pdf_attachments_manuscript ON pdf_attachments(manuscript_id);
"#;

/// schema 版本 3：稿件版本链 + 全局配置版本链。两份快照都是不可变检查点，
/// 线性编号；删除稿件时版本随外键级联清除。
const DDL_V3: &str = r#"
CREATE TABLE IF NOT EXISTS manuscript_versions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    manuscript_id    INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    version_number   INTEGER NOT NULL,
    name             TEXT    NOT NULL,
    comment          TEXT    NOT NULL DEFAULT '',
    snapshot_json    TEXT    NOT NULL,
    content_markdown TEXT    NOT NULL,
    notes            TEXT    NOT NULL DEFAULT '',
    title            TEXT    NOT NULL DEFAULT '',
    doc_number       TEXT    NOT NULL DEFAULT '',
    doc_date         TEXT    NOT NULL DEFAULT '',
    created_at       TEXT    NOT NULL,
    UNIQUE (manuscript_id, version_number)
);
CREATE INDEX IF NOT EXISTS idx_manuscript_versions_manuscript
    ON manuscript_versions(manuscript_id, version_number);

CREATE TABLE IF NOT EXISTS config_versions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    version_number INTEGER NOT NULL,
    name           TEXT    NOT NULL,
    comment        TEXT    NOT NULL DEFAULT '',
    config_json    TEXT    NOT NULL,
    created_at     TEXT    NOT NULL,
    UNIQUE (version_number)
);
"#;

/// schema 版本 4：上次退出时打开着的标签，用于下次启动恢复现场。
/// 只记标签的身份，内容照常从 manuscripts / 配置里读。
const DDL_V4: &str = r#"
CREATE TABLE IF NOT EXISTS open_tabs (
    ord           INTEGER PRIMARY KEY,
    kind          TEXT    NOT NULL,
    manuscript_id INTEGER,
    page          TEXT,
    is_active     INTEGER NOT NULL DEFAULT 0
);
"#;

/// schema 版本 5：知识库。独立于稿件库保存副本，按块建向量与全文索引，
/// 供 RAG 检索增强起草。FTS5 采用 external-content 模式：只索引 jieba
/// 预分词后的 tokens（unicode61 会把整段中文当成一个词，必须先分词），
/// 由 ai/ad/au 三个触发器保持与 knowledge_chunks 同步。
pub(crate) const DDL_V5: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_docs (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 'markdown' = 外部 md 文件；'manuscript' = 从稿件库勾选
    source               TEXT    NOT NULL,
    -- source='manuscript' 时记稿件库 id，仅用于展示与去重，不做外键（稿件可删）
    source_manuscript_id INTEGER,
    source_path          TEXT    NOT NULL DEFAULT '',
    kind                 TEXT    NOT NULL DEFAULT 'PlainDocument',
    title                TEXT    NOT NULL DEFAULT '',
    content_markdown     TEXT    NOT NULL DEFAULT '',
    -- 正文归一化后的哈希，用于**跨来源**去重：同一篇内容既从稿件库导入、
    -- 又从 md 文件导入时，按 source 各自去重是拦不住的，会整篇重复入库
    content_hash         TEXT    NOT NULL DEFAULT '',
    chunk_count          INTEGER NOT NULL DEFAULT 0,
    -- 嵌入所用模型名；换模型后重建索引时据此判断是否需要重算
    embed_model          TEXT    NOT NULL DEFAULT '',
    created_at           TEXT    NOT NULL,
    updated_at           TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_knowledge_docs_kind   ON knowledge_docs(kind);
CREATE INDEX IF NOT EXISTS idx_knowledge_docs_source ON knowledge_docs(source, source_manuscript_id)
    WHERE source_manuscript_id IS NOT NULL;
-- 注意：content_hash 的索引不放这里。老库的 knowledge_docs 已经存在（IF NOT
-- EXISTS 不会补列），此处引用该列会报 no such column，统一交给
-- ensure_knowledge_schema 在补列之后建。

CREATE TABLE IF NOT EXISTS knowledge_chunks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id     INTEGER NOT NULL REFERENCES knowledge_docs(id) ON DELETE CASCADE,
    ord        INTEGER NOT NULL,
    -- 块所在小节的标题链，如“正文 / 一、工作背景”；用于注入时标注出处
    section    TEXT    NOT NULL DEFAULT '',
    text       TEXT    NOT NULL,
    -- 块文本的 jieba 分词结果（空格分隔），仅供 FTS5 索引用，不用于展示
    tokens     TEXT    NOT NULL DEFAULT '',
    -- 原始向量，小端 f32 字节流（rag::vector::encode/decode），未嵌入时为 NULL
    embedding  BLOB,
    dims       INTEGER NOT NULL DEFAULT 0,
    UNIQUE (doc_id, ord)
);
CREATE INDEX IF NOT EXISTS idx_knowledge_chunks_doc ON knowledge_chunks(doc_id);

CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_chunks_fts USING fts5(
    tokens,
    content='knowledge_chunks',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS knowledge_chunks_ai AFTER INSERT ON knowledge_chunks BEGIN
    INSERT INTO knowledge_chunks_fts(rowid, tokens) VALUES (new.id, new.tokens);
END;
CREATE TRIGGER IF NOT EXISTS knowledge_chunks_ad AFTER DELETE ON knowledge_chunks BEGIN
    INSERT INTO knowledge_chunks_fts(knowledge_chunks_fts, rowid, tokens)
        VALUES('delete', old.id, old.tokens);
END;
CREATE TRIGGER IF NOT EXISTS knowledge_chunks_au AFTER UPDATE ON knowledge_chunks BEGIN
    INSERT INTO knowledge_chunks_fts(knowledge_chunks_fts, rowid, tokens)
        VALUES('delete', old.id, old.tokens);
    INSERT INTO knowledge_chunks_fts(rowid, tokens) VALUES (new.id, new.tokens);
END;
"#;

/// 幂等地建齐/补齐知识库表。`ManuscriptStore` 与 `KnowledgeStore` 各自打开
/// 同一个库文件时都调它，谁先到谁建。
///
/// `DDL_V5` 里的 `CREATE TABLE IF NOT EXISTS` 对**已存在**的旧表只会跳过、
/// 不会补列，所以后加的列必须单独探测再 `ALTER`——DDL 里直接引用新列会在
/// 老库上报 `no such column`。
pub(crate) fn ensure_knowledge_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL_V5)?;
    if !column_exists(conn, "knowledge_docs", "content_hash")? {
        conn.execute_batch(
            "ALTER TABLE knowledge_docs ADD COLUMN content_hash TEXT NOT NULL DEFAULT ''",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_knowledge_docs_hash ON knowledge_docs(content_hash)",
    )?;
    Ok(())
}

/// 表里是否已有该列（`PRAGMA table_info` 的第 2 列是列名）。
fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 上次退出时的一格标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTab {
    /// 稿件库里的一篇稿子。
    Manuscript(i64),
    /// 一个常驻页面，名称由调用方自己解释。
    Page(String),
}

/// 列表行：只取列表/过滤需要的轻量字段。
#[derive(Debug, Clone)]
pub struct ManuscriptRow {
    pub id: i64,
    pub title: String,
    pub kind: TemplateKind,
    pub status: ManuscriptStatus,
    /// 密级来自稿件快照；空串表示不标注密级。
    pub security_level: String,
    pub doc_number: String,
    pub doc_date: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

/// 完整稿件：详情页与导出/导入用。
#[derive(Debug, Clone)]
pub struct ManuscriptRecord {
    pub id: i64,
    pub snapshot: DraftInput,
    pub content_markdown: String,
    pub notes: String,
    pub title: String,
    pub kind: TemplateKind,
    pub status: ManuscriptStatus,
    pub doc_number: String,
    pub doc_date: String,
    pub source_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub published_at: Option<String>,
    pub archived_at: Option<String>,
    pub pdfs: Vec<PdfAttachment>,
}

/// 稿件下的 PDF 附件（扫描盖章件等）。
#[derive(Debug, Clone)]
pub struct PdfAttachment {
    pub id: i64,
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub added_at: String,
}

/// 稿件版本列表行：版本历史列表展示用。
#[derive(Debug, Clone)]
pub struct VersionRow {
    pub version_number: i64,
    pub name: String,
    pub comment: String,
    pub title: String,
    pub doc_number: String,
    pub doc_date: String,
    pub created_at: String,
    pub is_latest: bool,
}

/// 稿件版本完整记录：载入编辑 / 对照时整体还原。
#[derive(Debug, Clone)]
pub struct VersionRecord {
    pub version_number: i64,
    pub name: String,
    pub comment: String,
    pub snapshot: DraftInput,
    pub content_markdown: String,
    pub notes: String,
    pub created_at: String,
}

/// 配置版本列表行。
#[derive(Debug, Clone)]
pub struct ConfigVersionRow {
    pub version_number: i64,
    pub name: String,
    pub comment: String,
    pub created_at: String,
    pub is_latest: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NewManuscript {
    pub snapshot: DraftInput,
    pub content_markdown: String,
    pub notes: String,
    pub status: ManuscriptStatus,
    /// 导入时保留的原始时间戳；为 None 时取当前时间。普通保存不填。
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub published_at: Option<String>,
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ManuscriptUpdate {
    pub snapshot: DraftInput,
    pub content_markdown: String,
    pub notes: String,
}

/// 列表与导出的过滤条件。空值 = 不过滤。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManuscriptFilter {
    /// 匹配 标题 / 文号 / 备注。
    pub keyword: String,
    pub status: Option<ManuscriptStatus>,
    pub kind: Option<TemplateKind>,
    /// 成文日期范围，YYYY-MM-DD，空 = 不限。
    pub date_from: String,
    pub date_to: String,
}

pub struct ManuscriptStore {
    conn: Connection,
}

impl ManuscriptStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).context("创建稿件库目录失败")?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("无法打开稿件库 {}", path.display()))?;
        conn.busy_timeout(Duration::from_millis(2000))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 1 {
            self.conn.execute_batch(DDL_V1)?;
            self.conn.execute_batch("PRAGMA user_version = 1")?;
        }
        // v2：取消“新建”状态，历史“新建”一律升为草稿。
        if version < 2 {
            self.conn
                .execute_batch("UPDATE manuscripts SET status='Draft' WHERE status='New'")?;
            self.conn.execute_batch("PRAGMA user_version = 2")?;
        }
        // v3：稿件版本链与全局配置版本链。
        if version < 3 {
            self.conn.execute_batch(DDL_V3)?;
            self.conn.execute_batch("PRAGMA user_version = 3")?;
        }
        // v4：记住上次打开的标签。
        if version < 4 {
            self.conn.execute_batch(DDL_V4)?;
            self.conn.execute_batch("PRAGMA user_version = 4")?;
        }
        // v5：知识库文档/块/全文索引。建表与补列都幂等，v5 之后的知识库
        // 列变更统一走 ensure_knowledge_schema，不再单开 user_version 档位。
        ensure_knowledge_schema(&self.conn)?;
        if version < 5 {
            self.conn.execute_batch("PRAGMA user_version = 5")?;
        }
        Ok(())
    }

    /// 覆盖记录当前打开的标签。整表重写：标签不多，增量维护不划算。
    pub fn save_open_tabs(&mut self, tabs: &[OpenTab], active: usize) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM open_tabs", [])?;
        for (ord, tab) in tabs.iter().enumerate() {
            let is_active = i64::from(ord == active);
            match tab {
                OpenTab::Manuscript(id) => tx.execute(
                    "INSERT INTO open_tabs (ord, kind, manuscript_id, page, is_active)
                     VALUES (?1, 'doc', ?2, NULL, ?3)",
                    params![ord as i64, id, is_active],
                )?,
                OpenTab::Page(name) => tx.execute(
                    "INSERT INTO open_tabs (ord, kind, manuscript_id, page, is_active)
                     VALUES (?1, 'page', NULL, ?2, ?3)",
                    params![ord as i64, name, is_active],
                )?,
            };
        }
        tx.commit()?;
        Ok(())
    }

    /// 读回上次的标签与当时选中的那一格。稿件已被删除的行由调用方跳过。
    pub fn load_open_tabs(&mut self) -> Result<(Vec<OpenTab>, usize)> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, manuscript_id, page, is_active FROM open_tabs ORDER BY ord")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut tabs = Vec::new();
        let mut active = 0;
        for row in rows {
            let (kind, manuscript_id, page, is_active) = row?;
            let tab = match (kind.as_str(), manuscript_id, page) {
                ("doc", Some(id), _) => OpenTab::Manuscript(id),
                ("page", _, Some(name)) => OpenTab::Page(name),
                _ => continue,
            };
            if is_active != 0 {
                active = tabs.len();
            }
            tabs.push(tab);
        }
        Ok((tabs, active))
    }

    /// 由 snapshot + 正文派生标题/文种/文号/成文日期。新建记录，返回新 id。
    pub fn create(&mut self, new: &NewManuscript, source_id: Option<i64>) -> Result<i64> {
        let snapshot_json = serde_json::to_string(&new.snapshot).context("序列化稿件快照失败")?;
        let title = extract_title(&new.content_markdown, &new.snapshot.title_hint);
        let doc_number = new.snapshot.profile.document_number.trim().to_string();
        let doc_date = new.snapshot.date.trim().to_string();
        let doc_date_iso = parse_doc_date_iso(&doc_date);
        let now = Local::now().to_rfc3339();
        // “新建”是历史遗留状态：任何新建一律直接落为草稿。
        let status = if new.status == ManuscriptStatus::New {
            ManuscriptStatus::Draft
        } else {
            new.status
        };
        // 导入时保留源库时间；普通保存按状态补 published_at/archived_at。
        let created_at = new.created_at.clone().unwrap_or_else(|| now.clone());
        let updated_at = new.updated_at.clone().unwrap_or_else(|| now.clone());
        let published_at = new
            .published_at
            .clone()
            .or_else(|| (status == ManuscriptStatus::Published).then(|| now.clone()));
        let archived_at = new
            .archived_at
            .clone()
            .or_else(|| (status == ManuscriptStatus::Archived).then(|| now.clone()));
        self.conn.execute(
            "INSERT INTO manuscripts (snapshot_json, content_markdown, notes, title, kind, status, doc_number, doc_date, doc_date_iso, source_id, created_at, updated_at, published_at, archived_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                snapshot_json,
                new.content_markdown,
                new.notes,
                title,
                kind_to_str(new.snapshot.kind),
                status_to_str(status),
                doc_number,
                doc_date,
                doc_date_iso,
                source_id,
                created_at,
                updated_at,
                published_at,
                archived_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 更新快照/正文/备注。归档稿件拒绝；历史遗留的“新建”记录在首次修改时兜底提升为草稿。
    pub fn update(&mut self, id: i64, upd: &ManuscriptUpdate) -> Result<()> {
        let current = self.status_of(id)?.context("稿件不存在")?;
        if current == ManuscriptStatus::Archived {
            bail!("归档稿件不可修改");
        }
        let status = if current == ManuscriptStatus::New {
            ManuscriptStatus::Draft
        } else {
            current
        };
        let snapshot_json = serde_json::to_string(&upd.snapshot).context("序列化稿件快照失败")?;
        let title = extract_title(&upd.content_markdown, &upd.snapshot.title_hint);
        let doc_number = upd.snapshot.profile.document_number.trim().to_string();
        let doc_date = upd.snapshot.date.trim().to_string();
        let doc_date_iso = parse_doc_date_iso(&doc_date);
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "UPDATE manuscripts SET snapshot_json=?1, content_markdown=?2, notes=?3, title=?4, kind=?5, status=?6, doc_number=?7, doc_date=?8, doc_date_iso=?9, updated_at=?10 WHERE id=?11",
            params![
                snapshot_json,
                upd.content_markdown,
                upd.notes,
                title,
                kind_to_str(upd.snapshot.kind),
                status_to_str(status),
                doc_number,
                doc_date,
                doc_date_iso,
                now,
                id,
            ],
        )?;
        Ok(())
    }

    /// 状态流转。校验 `can_transition`；归档行拒绝任何改动；记录 published_at/archived_at。
    pub fn set_status(&mut self, id: i64, target: ManuscriptStatus) -> Result<()> {
        let current = self.status_of(id)?.context("稿件不存在")?;
        if current == ManuscriptStatus::Archived {
            bail!("归档稿件不可修改");
        }
        if !current.can_transition(target) {
            bail!("不允许从{}转为{}", current.label(), target.label());
        }
        let now = Local::now().to_rfc3339();
        let published_at = (target == ManuscriptStatus::Published).then(|| now.clone());
        let archived_at = (target == ManuscriptStatus::Archived).then(|| now.clone());
        self.conn.execute(
            "UPDATE manuscripts SET status=?1, published_at=?2, archived_at=?3, updated_at=?4 WHERE id=?5",
            params![status_to_str(target), published_at, archived_at, now, id],
        )?;
        Ok(())
    }

    /// 删除稿件（连同附件级联）。归档行拒绝删除。
    pub fn delete(&mut self, id: i64) -> Result<()> {
        let current = self.status_of(id)?.context("稿件不存在")?;
        if current == ManuscriptStatus::Archived {
            bail!("归档稿件不可删除");
        }
        self.conn
            .execute("DELETE FROM manuscripts WHERE id=?1", [id])?;
        Ok(())
    }

    /// 原子批量删除：任一稿件不存在或已归档时整批不落库。
    pub fn delete_many(&mut self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for &id in ids {
            let status = tx
                .query_row("SELECT status FROM manuscripts WHERE id=?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .and_then(|value| str_to_status(&value))
                .context("稿件不存在")?;
            if status == ManuscriptStatus::Archived {
                bail!("归档稿件不可删除");
            }
            tx.execute("DELETE FROM manuscripts WHERE id=?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 完整读取（含 PDF 附件）。
    pub fn get(&mut self, id: i64) -> Result<Option<ManuscriptRecord>> {
        let data = self
            .conn
            .query_row(
                "SELECT snapshot_json, content_markdown, notes, title, kind, status, doc_number, doc_date, source_id, created_at, updated_at, published_at, archived_at
                 FROM manuscripts WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, Option<i64>>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, Option<String>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            snapshot_json,
            content,
            notes,
            title,
            kind,
            status,
            doc_number,
            doc_date,
            source_id,
            created,
            updated,
            published_at,
            archived_at,
        )) = data
        else {
            return Ok(None);
        };
        let snapshot = serde_json::from_str(&snapshot_json).context("稿件快照数据损坏")?;
        let mut record = ManuscriptRecord {
            id,
            snapshot,
            content_markdown: content,
            notes,
            title,
            kind: str_to_kind(&kind).unwrap_or(TemplateKind::OfficialLetter),
            status: str_to_status(&status).unwrap_or(ManuscriptStatus::Draft),
            doc_number,
            doc_date,
            source_id,
            created_at: created,
            updated_at: updated,
            published_at,
            archived_at,
            pdfs: Vec::new(),
        };
        record.pdfs = self.pdfs_for(id)?;
        Ok(Some(record))
    }

    /// 按过滤条件列出稿件（轻量行）。
    pub fn list(&mut self, filter: &ManuscriptFilter) -> Result<Vec<ManuscriptRow>> {
        let mut sql = String::from(
            "SELECT id, title, kind, status, doc_number, doc_date, updated_at, archived_at, snapshot_json FROM manuscripts WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let keyword = filter.keyword.trim();
        if !keyword.is_empty() {
            sql.push_str(
                " AND instr(lower(title || ' ' || doc_number || ' ' || notes), lower(?)) > 0",
            );
            params.push(Box::new(keyword.to_lowercase()));
        }
        if let Some(status) = filter.status {
            sql.push_str(" AND status = ?");
            params.push(Box::new(status_to_str(status).to_string()));
        }
        if let Some(kind) = filter.kind {
            sql.push_str(" AND kind = ?");
            params.push(Box::new(kind_to_str(kind).to_string()));
        }
        let from = filter.date_from.trim();
        if !from.is_empty() {
            sql.push_str(" AND doc_date_iso >= ?");
            params.push(Box::new(from.to_string()));
        }
        let to = filter.date_to.trim();
        if !to.is_empty() {
            sql.push_str(" AND doc_date_iso <= ?");
            params.push(Box::new(to.to_string()));
        }
        sql.push_str(" ORDER BY updated_at DESC, id DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let mapped = stmt.query_map(
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            |row| {
                let snapshot_json: String = row.get(8)?;
                let security_level = serde_json::from_str::<DraftInput>(&snapshot_json)
                    .map(|snapshot| snapshot.profile.security_level)
                    .unwrap_or_default();
                Ok(ManuscriptRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    kind: str_to_kind(&row.get::<_, String>(2)?)
                        .unwrap_or(TemplateKind::OfficialLetter),
                    status: str_to_status(&row.get::<_, String>(3)?)
                        .unwrap_or(ManuscriptStatus::Draft),
                    security_level,
                    doc_number: row.get(4)?,
                    doc_date: row.get(5)?,
                    updated_at: row.get(6)?,
                    archived_at: row.get(7)?,
                })
            },
        )?;
        let mut out = Vec::new();
        for row in mapped {
            out.push(row?);
        }
        Ok(out)
    }

    /// 各状态稿件数，`[新建, 草稿, 发布, 归档]`。新建恒为 0（历史遗留，创建与迁移时统一升为草稿）。
    pub fn count_by_status(&mut self) -> Result<[i64; 4]> {
        let mut counts = [0i64; 4];
        let mut stmt = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM manuscripts GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (status, count) = row?;
            if let Some(status) = str_to_status(&status) {
                counts[status as usize] = count;
            }
        }
        Ok(counts)
    }

    /// 同一机关代字、同一发文年度已经使用过的最高函号流水号。
    /// `excluding_id` 用于编辑已有稿件时排除自身，避免把当前值当作历史上限。
    pub fn highest_letter_serial(
        &self,
        year: &str,
        department_code: &str,
        excluding_id: Option<i64>,
    ) -> Result<Option<u64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, snapshot_json FROM manuscripts WHERE kind='OfficialLetter'")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut highest = None;
        for row in rows {
            let (id, json) = row?;
            if excluding_id == Some(id) {
                continue;
            }
            let Ok(snapshot) = serde_json::from_str::<DraftInput>(&json) else {
                continue;
            };
            if snapshot.document_year() != year
                || snapshot.profile.department_code.trim() != department_code.trim()
            {
                continue;
            }
            let Ok(serial) = snapshot.profile.document_number.trim().parse::<u64>() else {
                continue;
            };
            highest = Some(highest.map_or(serial, |current: u64| current.max(serial)));
        }
        Ok(highest)
    }

    /// 已用过的 `source_id` 集合，供导入预览判断“与本地同源”。
    pub fn source_ids(&mut self) -> Result<HashSet<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source_id FROM manuscripts WHERE source_id IS NOT NULL")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        let mut set = HashSet::new();
        for row in rows {
            set.insert(row?);
        }
        Ok(set)
    }

    /// 为稿件追加一个 PDF 附件（任何状态下都允许）。
    pub fn add_pdf(&mut self, manuscript_id: i64, file_name: &str, bytes: &[u8]) -> Result<()> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM manuscripts WHERE id=?1)",
            [manuscript_id],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("稿件不存在，无法添加附件");
        }
        let now = Local::now().to_rfc3339();
        let sort: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM pdf_attachments WHERE manuscript_id=?1",
            [manuscript_id],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO pdf_attachments (manuscript_id, file_name, bytes, added_at, sort_order) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![manuscript_id, file_name, bytes, now, sort],
        )?;
        Ok(())
    }

    /// 删除一个 PDF 附件。
    pub fn remove_pdf(&mut self, attachment_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM pdf_attachments WHERE id=?1", [attachment_id])?;
        Ok(())
    }

    // ---- 版本链：稿件版本 ----

    /// 稿件相对最新版本是否有内容变更（提交前的预览检查）。
    /// 首个版本要求有正文或要素；之后要求与最新版不同。
    pub fn manuscript_version_changed(
        &mut self,
        manuscript_id: i64,
        snapshot: &DraftInput,
        content_markdown: &str,
        notes: &str,
    ) -> Result<bool> {
        let snapshot_json = serde_json::to_string(snapshot).context("序列化稿件快照失败")?;
        if let Some((json, content, old_notes)) = self.latest_manuscript_content(manuscript_id)? {
            return Ok(json != snapshot_json || content != content_markdown || old_notes != notes);
        }
        Ok(!content_markdown.trim().is_empty() || !snapshot.title_hint.trim().is_empty())
    }

    /// 提交一个新稿件版本：内容必须相对最新版本有变更（否则报错）。返回新版本行。
    pub fn commit_manuscript_version(
        &mut self,
        manuscript_id: i64,
        name: &str,
        comment: &str,
        snapshot: &DraftInput,
        content_markdown: &str,
        notes: &str,
    ) -> Result<VersionRow> {
        let current = self
            .status_of(manuscript_id)?
            .context("稿件不存在，无法提交版本")?;
        if current == ManuscriptStatus::Archived {
            bail!("归档稿件不可提交版本");
        }
        if !self.manuscript_version_changed(manuscript_id, snapshot, content_markdown, notes)? {
            bail!("相对上一版本没有内容变更，不能提交");
        }
        let snapshot_json = serde_json::to_string(snapshot).context("序列化稿件快照失败")?;
        let title = extract_title(content_markdown, &snapshot.title_hint);
        let doc_number = snapshot.profile.document_number.trim().to_string();
        let doc_date = snapshot.date.trim().to_string();
        let number = self
            .latest_manuscript_number(manuscript_id)?
            .map_or(1, |n| n + 1);
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO manuscript_versions (manuscript_id, version_number, name, comment, snapshot_json, content_markdown, notes, title, doc_number, doc_date, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                manuscript_id,
                number,
                name.trim(),
                comment.trim(),
                snapshot_json,
                content_markdown,
                notes,
                title,
                doc_number,
                doc_date,
                now,
            ],
        )?;
        Ok(VersionRow {
            version_number: number,
            name: name.trim().to_string(),
            comment: comment.trim().to_string(),
            title,
            doc_number,
            doc_date,
            created_at: now,
            is_latest: true,
        })
    }

    /// 列出一篇稿件的版本历史（升序，最新在最后）。
    pub fn list_manuscript_versions(&mut self, manuscript_id: i64) -> Result<Vec<VersionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT version_number, name, comment, title, doc_number, doc_date, created_at
             FROM manuscript_versions WHERE manuscript_id=?1 ORDER BY version_number ASC",
        )?;
        let rows = stmt.query_map([manuscript_id], |row| {
            Ok(VersionRow {
                version_number: row.get(0)?,
                name: row.get(1)?,
                comment: row.get(2)?,
                title: row.get(3)?,
                doc_number: row.get(4)?,
                doc_date: row.get(5)?,
                created_at: row.get(6)?,
                is_latest: false,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        if let Some(last) = out.last_mut() {
            last.is_latest = true;
        }
        Ok(out)
    }

    /// 读取稿件某一版本（载入编辑 / 对照用）。
    pub fn get_manuscript_version(
        &mut self,
        manuscript_id: i64,
        version_number: i64,
    ) -> Result<Option<VersionRecord>> {
        let data = self
            .conn
            .query_row(
                "SELECT version_number, name, comment, snapshot_json, content_markdown, notes, created_at
                 FROM manuscript_versions WHERE manuscript_id=?1 AND version_number=?2",
                params![manuscript_id, version_number],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((version_number, name, comment, snapshot_json, content, notes, created_at)) = data
        else {
            return Ok(None);
        };
        let snapshot = serde_json::from_str(&snapshot_json).context("版本快照数据损坏")?;
        Ok(Some(VersionRecord {
            version_number,
            name,
            comment,
            snapshot,
            content_markdown: content,
            notes,
            created_at,
        }))
    }

    /// 读取活稿行的快照与正文（轻量，不带 PDF 附件），供起草页判断"有没有未提交修改"
    /// 与切回未提交内容使用。`get` 会连同附件 BLOB 一起读出来，这两个场景用不上。
    pub fn snapshot_of(&self, id: i64) -> Result<Option<(DraftInput, String)>> {
        let data = self
            .conn
            .query_row(
                "SELECT snapshot_json, content_markdown FROM manuscripts WHERE id=?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((snapshot_json, content)) = data else {
            return Ok(None);
        };
        let snapshot = serde_json::from_str(&snapshot_json).context("稿件快照数据损坏")?;
        Ok(Some((snapshot, content)))
    }

    /// 读取稿件备注（轻量），供起草页提交版本预览使用。
    pub fn notes_of(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT notes FROM manuscripts WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?)
    }

    fn latest_manuscript_number(&self, manuscript_id: i64) -> Result<Option<i64>> {
        let number = self
            .conn
            .query_row(
                "SELECT version_number FROM manuscript_versions WHERE manuscript_id=?1 ORDER BY version_number DESC LIMIT 1",
                [manuscript_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(number)
    }

    /// 最新稿件版本的 (snapshot_json, content_markdown, notes)，供变更检查。
    /// 最新已提交版本的 `(要素 JSON, 正文, 备注)`；一版都没有时为 None。
    pub(crate) fn latest_manuscript_content(
        &self,
        manuscript_id: i64,
    ) -> Result<Option<(String, String, String)>> {
        let data = self
            .conn
            .query_row(
                "SELECT snapshot_json, content_markdown, notes FROM manuscript_versions
                 WHERE manuscript_id=?1 ORDER BY version_number DESC LIMIT 1",
                [manuscript_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(data)
    }

    // ---- 版本链：全局配置 ----

    /// 当前配置相对最新配置版本是否有变更（提交前的预览检查）。首个版本恒为可提交。
    pub fn config_version_changed(&self, config: &AppConfig) -> Result<bool> {
        let json = serde_json::to_string(config).context("序列化配置失败")?;
        Ok(match self.latest_config()? {
            Some((latest_json, _)) => latest_json != json,
            None => true,
        })
    }

    /// 提交一个新配置版本：内容必须相对最新版本有变更（否则报错）。返回新版本行。
    pub fn commit_config_version(
        &mut self,
        name: &str,
        comment: &str,
        config: &AppConfig,
    ) -> Result<ConfigVersionRow> {
        let json = serde_json::to_string(config).context("序列化配置版本失败")?;
        let latest = self.latest_config()?;
        if let Some((latest_json, _)) = &latest
            && latest_json == &json
        {
            bail!("相对上一配置版本没有变更，不能提交");
        }
        let number = latest.map(|(_, n)| n + 1).unwrap_or(1);
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO config_versions (version_number, name, comment, config_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![number, name.trim(), comment.trim(), json, now],
        )?;
        Ok(ConfigVersionRow {
            version_number: number,
            name: name.trim().to_string(),
            comment: comment.trim().to_string(),
            created_at: now,
            is_latest: true,
        })
    }

    /// 列出配置版本历史（升序，最新在最后）。
    pub fn list_config_versions(&mut self) -> Result<Vec<ConfigVersionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT version_number, name, comment, created_at FROM config_versions ORDER BY version_number ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ConfigVersionRow {
                version_number: row.get(0)?,
                name: row.get(1)?,
                comment: row.get(2)?,
                created_at: row.get(3)?,
                is_latest: false,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        if let Some(last) = out.last_mut() {
            last.is_latest = true;
        }
        Ok(out)
    }

    /// 读取配置某一版本，反序列化为 AppConfig（应用回滚用）。
    pub fn get_config_version(&mut self, version_number: i64) -> Result<Option<AppConfig>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT config_json FROM config_versions WHERE version_number=?1",
                [version_number],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(json) => Ok(Some(
                serde_json::from_str(&json).context("配置版本数据损坏")?,
            )),
            None => Ok(None),
        }
    }

    /// 最新配置版本的 (config_json, version_number)。
    fn latest_config(&self) -> Result<Option<(String, i64)>> {
        let data = self
            .conn
            .query_row(
                "SELECT config_json, version_number FROM config_versions ORDER BY version_number DESC LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(data)
    }

    fn status_of(&self, id: i64) -> Result<Option<ManuscriptStatus>> {
        let raw: Option<String> = self
            .conn
            .query_row("SELECT status FROM manuscripts WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(raw.and_then(|s| str_to_status(&s)))
    }

    fn pdfs_for(&self, manuscript_id: i64) -> Result<Vec<PdfAttachment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_name, bytes, added_at FROM pdf_attachments WHERE manuscript_id=?1 ORDER BY sort_order, id",
        )?;
        let rows = stmt.query_map([manuscript_id], |row| {
            Ok(PdfAttachment {
                id: row.get(0)?,
                file_name: row.get(1)?,
                bytes: row.get(2)?,
                added_at: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

fn status_to_str(status: ManuscriptStatus) -> &'static str {
    match status {
        ManuscriptStatus::New => "New",
        ManuscriptStatus::Draft => "Draft",
        ManuscriptStatus::Published => "Published",
        ManuscriptStatus::Archived => "Archived",
    }
}

fn str_to_status(s: &str) -> Option<ManuscriptStatus> {
    match s {
        "New" => Some(ManuscriptStatus::New),
        "Draft" => Some(ManuscriptStatus::Draft),
        "Published" => Some(ManuscriptStatus::Published),
        "Archived" => Some(ManuscriptStatus::Archived),
        _ => None,
    }
}

pub(crate) fn kind_to_str(kind: TemplateKind) -> &'static str {
    match kind {
        TemplateKind::OfficialLetter => "OfficialLetter",
        TemplateKind::PhoneNotice => "PhoneNotice",
        TemplateKind::PlainDocument => "PlainDocument",
        TemplateKind::MeetingAgenda => "MeetingAgenda",
        TemplateKind::WhitePaper => "WhitePaper",
    }
}

pub(crate) fn str_to_kind(s: &str) -> Option<TemplateKind> {
    match s {
        "OfficialLetter" => Some(TemplateKind::OfficialLetter),
        "PhoneNotice" => Some(TemplateKind::PhoneNotice),
        "PlainDocument" => Some(TemplateKind::PlainDocument),
        "MeetingAgenda" => Some(TemplateKind::MeetingAgenda),
        "WhitePaper" => Some(TemplateKind::WhitePaper),
        _ => None,
    }
}

/// 成文日期显示串 → YYYY-MM-DD，用于范围过滤。
/// 支持 `2026年8月6日`、`2026-08-06`、`2026/8/6` 等；缺月/日时补 `01`；无法解析返回空串。
fn parse_doc_date_iso(value: &str) -> String {
    let parts: Vec<u32> = value
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse::<u32>().ok())
        .collect();
    let Some(&year) = parts.first() else {
        return String::new();
    };
    if !(1900..=2200).contains(&year) {
        return String::new();
    }
    let month = parts.get(1).copied().unwrap_or(1);
    let day = parts.get(2).copied().unwrap_or(1);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return String::new();
    }
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateProfile;

    fn sample_snapshot(kind: TemplateKind) -> DraftInput {
        DraftInput {
            kind,
            title_hint: "关于召开会议的通知".into(),
            date: "2026年8月6日".into(),
            date_is_auto: false,
            meeting_time: String::new(),
            attendees: String::new(),
            profile: TemplateProfile {
                document_number: "某教函〔2026〕12号".into(),
                ..TemplateProfile::for_kind(kind)
            },
        }
    }

    fn new_manuscript(kind: TemplateKind, markdown: &str) -> NewManuscript {
        NewManuscript {
            snapshot: sample_snapshot(kind),
            content_markdown: markdown.to_string(),
            notes: String::new(),
            status: ManuscriptStatus::Draft,
            ..Default::default()
        }
    }

    fn numbered_letter(year: &str, code: &str, serial: &str) -> NewManuscript {
        let mut manuscript = new_manuscript(TemplateKind::OfficialLetter, "# 函稿");
        manuscript.snapshot.profile.document_year = year.into();
        manuscript.snapshot.profile.department_code = code.into();
        manuscript.snapshot.profile.document_number = serial.into();
        manuscript
    }

    fn mem_store() -> ManuscriptStore {
        ManuscriptStore::open(Path::new(":memory:")).expect("内存库应能打开")
    }

    #[test]
    fn highest_letter_serial_is_scoped_by_year_and_department_code() {
        let mut store = mem_store();
        let first = store
            .create(&numbered_letter("2026", "某政函", "12"), None)
            .unwrap();
        store
            .create(&numbered_letter("2026", "某政函", "18"), None)
            .unwrap();
        store
            .create(&numbered_letter("2025", "某政函", "99"), None)
            .unwrap();
        store
            .create(&numbered_letter("2026", "另一函", "88"), None)
            .unwrap();

        assert_eq!(
            store.highest_letter_serial("2026", "某政函", None).unwrap(),
            Some(18)
        );
        assert_eq!(
            store
                .highest_letter_serial("2026", "某政函", Some(first))
                .unwrap(),
            Some(18)
        );
    }

    #[test]
    fn delete_many_is_atomic_when_an_archived_record_is_included() {
        let mut store = mem_store();
        let draft = store
            .create(
                &new_manuscript(TemplateKind::OfficialLetter, "# 草稿"),
                None,
            )
            .unwrap();
        let archived = store
            .create(
                &new_manuscript(TemplateKind::OfficialLetter, "# 归档"),
                None,
            )
            .unwrap();
        store
            .set_status(archived, ManuscriptStatus::Archived)
            .unwrap();

        assert!(store.delete_many(&[draft, archived]).is_err());
        assert!(store.get(draft).unwrap().is_some());
        assert!(store.get(archived).unwrap().is_some());
    }

    #[test]
    fn crud_round_trip_and_lifecycle() {
        let mut store = mem_store();
        let id = store
            .create(
                &new_manuscript(TemplateKind::OfficialLetter, "# 关于召开会议的通知\n\n正文"),
                None,
            )
            .expect("创建应成功");

        // 首次保存即草稿，无需二次保存或 AI 生成。
        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.status, ManuscriptStatus::Draft);
        assert_eq!(record.title, "关于召开会议的通知");
        assert_eq!(record.doc_number, "某教函〔2026〕12号");

        let mut updated = record.snapshot.clone();
        updated.title_hint = "改过的标题提示".into();
        store
            .update(
                id,
                &ManuscriptUpdate {
                    snapshot: updated,
                    content_markdown: "# 关于召开会议的通知\n\n改过的正文".into(),
                    notes: "备注".into(),
                },
            )
            .unwrap();
        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.status, ManuscriptStatus::Draft);
        assert_eq!(record.notes, "备注");
        assert_eq!(record.snapshot.title_hint, "改过的标题提示");

        // 草稿 → 发布 → 归档。
        store.set_status(id, ManuscriptStatus::Published).unwrap();
        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.status, ManuscriptStatus::Published);
        assert!(record.published_at.is_some());
        assert!(record.archived_at.is_none());

        store.set_status(id, ManuscriptStatus::Draft).unwrap(); // 发布可退回草稿
        store.set_status(id, ManuscriptStatus::Archived).unwrap();
        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.status, ManuscriptStatus::Archived);
        assert!(record.archived_at.is_some());
    }

    #[test]
    fn archived_rows_reject_mutation() {
        let mut store = mem_store();
        let id = store
            .create(&new_manuscript(TemplateKind::WhitePaper, "# 呈批件"), None)
            .unwrap();
        store.set_status(id, ManuscriptStatus::Published).unwrap();
        store.set_status(id, ManuscriptStatus::Archived).unwrap();

        let upd = ManuscriptUpdate {
            snapshot: sample_snapshot(TemplateKind::WhitePaper),
            content_markdown: "# 试图改标题".into(),
            notes: String::new(),
        };
        assert!(store.update(id, &upd).is_err());
        assert!(store.set_status(id, ManuscriptStatus::Draft).is_err());
        assert!(store.delete(id).is_err());
        // 附件仍可增删（归档不锁附件）。
        store.add_pdf(id, "扫描件.pdf", b"%PDF-1.4 fake").unwrap();
        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.pdfs.len(), 1);
        store.remove_pdf(record.pdfs[0].id).unwrap();
        assert!(store.get(id).unwrap().unwrap().pdfs.is_empty());
        // 归档内容未被改动。
        assert_eq!(record.title, "呈批件");
    }

    #[test]
    fn transition_rules_are_enforced() {
        let mut store = mem_store();
        let id = store
            .create(
                &new_manuscript(TemplateKind::PhoneNotice, "# 电话通知"),
                None,
            )
            .unwrap();
        // 创建即草稿：不能退回“新建”，发布可退回草稿，归档是终态。
        assert!(store.set_status(id, ManuscriptStatus::New).is_err());
        store.set_status(id, ManuscriptStatus::Published).unwrap();
        store.set_status(id, ManuscriptStatus::Draft).unwrap();
        store.set_status(id, ManuscriptStatus::Archived).unwrap();
        assert!(store.set_status(id, ManuscriptStatus::Published).is_err());
        assert!(store.set_status(id, ManuscriptStatus::Draft).is_err());
    }

    #[test]
    fn filter_queries_match() {
        let mut store = mem_store();
        let mut secret_manuscript =
            new_manuscript(TemplateKind::OfficialLetter, "# 甲文件\n\n正文");
        secret_manuscript.snapshot.profile.security_level = "机密".into();
        let a = store.create(&secret_manuscript, None).unwrap();
        let b = store
            .create(
                &new_manuscript(TemplateKind::MeetingAgenda, "# 乙议程\n\n议程"),
                None,
            )
            .unwrap();
        let c = store
            .create(
                &new_manuscript(TemplateKind::WhitePaper, "# 丙呈批件"),
                None,
            )
            .unwrap();
        store.set_status(c, ManuscriptStatus::Published).unwrap();

        let all = store.list(&ManuscriptFilter::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(
            all.iter().find(|row| row.id == a).unwrap().security_level,
            "机密"
        );

        let by_kind = store
            .list(&ManuscriptFilter {
                kind: Some(TemplateKind::MeetingAgenda),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].id, b);

        let by_status = store
            .list(&ManuscriptFilter {
                status: Some(ManuscriptStatus::Published),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_status.len(), 1);
        assert_eq!(by_status[0].id, c);

        let by_keyword = store
            .list(&ManuscriptFilter {
                keyword: "乙议程".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_keyword.len(), 1);
        assert_eq!(by_keyword[0].id, b);

        let by_date = store
            .list(&ManuscriptFilter {
                date_from: "2026-08-01".into(),
                date_to: "2026-08-31".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_date.len(), 3);

        // 范围外日期被过滤掉。
        store
            .update(
                a,
                &ManuscriptUpdate {
                    snapshot: DraftInput {
                        date: "2025年1月1日".into(),
                        ..sample_snapshot(TemplateKind::OfficialLetter)
                    },
                    content_markdown: "# 甲文件\n\n正文".into(),
                    notes: String::new(),
                },
            )
            .unwrap();
        let by_date = store
            .list(&ManuscriptFilter {
                date_from: "2026-08-01".into(),
                date_to: "2026-08-31".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_date.len(), 2);

        // 计数（创建即草稿，c 已发布）。
        let counts = store.count_by_status().unwrap();
        assert_eq!(counts[0], 0); // 新建恒为 0
        assert_eq!(counts[1], 2); // 草稿（a、b）
        assert_eq!(counts[2], 1); // 发布

        // source_ids 去重。
        assert!(store.source_ids().unwrap().is_empty());
    }

    #[test]
    fn source_id_dedup_surfaces() {
        let mut store = mem_store();
        let id = store
            .create(
                &new_manuscript(TemplateKind::OfficialLetter, "# 甲文件"),
                Some(42),
            )
            .unwrap();
        assert_eq!(id, 1);
        assert!(store.source_ids().unwrap().contains(&42));
    }

    #[test]
    fn pdf_attachments_round_trip() {
        let mut store = mem_store();
        let id = store
            .create(&new_manuscript(TemplateKind::OfficialLetter, "# 甲"), None)
            .unwrap();
        store.add_pdf(id, "扫描件.pdf", b"%PDF-1.4 first").unwrap();
        store.add_pdf(id, "盖章件.pdf", b"%PDF-1.4 second").unwrap();
        let record = store.get(id).unwrap().unwrap();
        assert_eq!(record.pdfs.len(), 2);
        assert_eq!(record.pdfs[0].file_name, "扫描件.pdf");
        assert_eq!(record.pdfs[0].bytes, b"%PDF-1.4 first");
        assert_eq!(record.pdfs[1].file_name, "盖章件.pdf");
        // 不存在的稿件不能加附件。
        assert!(store.add_pdf(999, "x.pdf", b"x").is_err());
    }

    #[test]
    fn parse_doc_date_iso_variants() {
        assert_eq!(parse_doc_date_iso("2026年8月6日"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026年08月06日"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026-08-06"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026/8/6"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026年8月"), "2026-08-01");
        assert_eq!(parse_doc_date_iso("2026年"), "2026-01-01");
        assert_eq!(parse_doc_date_iso(""), "");
        assert_eq!(parse_doc_date_iso("另行通知"), "");
        assert_eq!(parse_doc_date_iso("9999年8月6日"), "");
    }

    #[test]
    fn status_and_kind_string_conversions() {
        for status in ManuscriptStatus::ALL {
            assert_eq!(str_to_status(status_to_str(status)), Some(status));
        }
        for kind in TemplateKind::ALL {
            assert_eq!(str_to_kind(kind_to_str(kind)), Some(kind));
        }
        assert_eq!(str_to_status("nope"), None);
        assert_eq!(str_to_kind("nope"), None);
    }

    #[test]
    fn open_tabs_round_trip_and_overwrite() {
        let mut store = mem_store();
        let id = store
            .create(
                &new_manuscript(
                    TemplateKind::OfficialLetter,
                    "# 甲

正文",
                ),
                None,
            )
            .unwrap();
        store
            .save_open_tabs(
                &[OpenTab::Manuscript(id), OpenTab::Page("settings".into())],
                1,
            )
            .unwrap();
        let (tabs, active) = store.load_open_tabs().unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0], OpenTab::Manuscript(id));
        assert_eq!(tabs[1], OpenTab::Page("settings".into()));
        assert_eq!(active, 1);

        // 整表重写：第二次保存不应该留下上一次的行。
        store
            .save_open_tabs(&[OpenTab::Page("vocabulary".into())], 0)
            .unwrap();
        let (tabs, active) = store.load_open_tabs().unwrap();
        assert_eq!(tabs, vec![OpenTab::Page("vocabulary".into())]);
        assert_eq!(active, 0);
    }

    #[test]
    fn manuscript_versions_commit_list_and_load() {
        let mut store = mem_store();
        let id = store
            .create(
                &new_manuscript(TemplateKind::OfficialLetter, "# 甲\n\n正文v1"),
                None,
            )
            .unwrap();
        let rec = store.get(id).unwrap().unwrap();

        // 首个版本：有正文即可提交。
        let v1 = store
            .commit_manuscript_version(
                id,
                "2026-08-07 09:00",
                "初稿",
                &rec.snapshot,
                "# 甲\n\n正文v1",
                &rec.notes,
            )
            .unwrap();
        assert_eq!(v1.version_number, 1);
        assert!(v1.is_latest);

        // 完全相同的内容 → 拒绝。
        assert!(
            store
                .commit_manuscript_version(
                    id,
                    "2026-08-07 09:30",
                    "",
                    &rec.snapshot,
                    "# 甲\n\n正文v1",
                    &rec.notes
                )
                .is_err()
        );

        // 改要素 → v2。
        let mut snapshot = rec.snapshot.clone();
        snapshot.title_hint = "改动".into();
        let v2 = store
            .commit_manuscript_version(
                id,
                "2026-08-07 10:00",
                "第二版",
                &snapshot,
                "# 甲\n\n正文v2",
                &rec.notes,
            )
            .unwrap();
        assert_eq!(v2.version_number, 2);

        let list = store.list_manuscript_versions(id).unwrap();
        assert_eq!(list.len(), 2);
        assert!(!list[0].is_latest && list[0].version_number == 1);
        assert!(list[1].is_latest && list[1].version_number == 2);
        assert_eq!(list[1].comment, "第二版");
        assert_eq!(list[1].title, "甲");

        // 载入旧版。
        let old = store.get_manuscript_version(id, 1).unwrap().unwrap();
        assert_eq!(old.content_markdown, "# 甲\n\n正文v1");
        assert_eq!(old.snapshot.title_hint, "关于召开会议的通知");
        assert!(store.get_manuscript_version(id, 99).unwrap().is_none());

        // 删除稿件 → 版本级联清除。
        store.delete(id).unwrap();
        assert!(store.list_manuscript_versions(id).unwrap().is_empty());
    }

    #[test]
    fn first_version_requires_content() {
        let mut store = mem_store();
        let id = store
            .create(
                &NewManuscript {
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        // 空正文 + 空要素 → 首个版本不可提交。
        assert!(
            store
                .commit_manuscript_version(id, "x", "", &DraftInput::default(), "", "")
                .is_err()
        );
    }

    #[test]
    fn archived_manuscript_rejects_version_commit() {
        let mut store = mem_store();
        let id = store
            .create(&new_manuscript(TemplateKind::WhitePaper, "# 呈批件"), None)
            .unwrap();
        store.set_status(id, ManuscriptStatus::Published).unwrap();
        store.set_status(id, ManuscriptStatus::Archived).unwrap();
        let rec = store.get(id).unwrap().unwrap();
        assert!(
            store
                .commit_manuscript_version(id, "v", "", &rec.snapshot, "# 呈批件", "")
                .is_err()
        );
    }

    #[test]
    fn config_versions_commit_and_apply() {
        let mut store = mem_store();
        let mut config = AppConfig::default();
        assert!(store.config_version_changed(&config).unwrap());
        let v1 = store
            .commit_config_version("2026-08-07 09:00", "初始", &config)
            .unwrap();
        assert_eq!(v1.version_number, 1);

        // 相同配置 → 拒绝。
        assert!(!store.config_version_changed(&config).unwrap());
        assert!(
            store
                .commit_config_version("2026-08-07 09:30", "", &config)
                .is_err()
        );

        config.allow_free_text = false;
        assert!(store.config_version_changed(&config).unwrap());
        let v2 = store
            .commit_config_version("2026-08-07 10:00", "收紧录入", &config)
            .unwrap();
        assert_eq!(v2.version_number, 2);

        let list = store.list_config_versions().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[1].is_latest);

        let applied = store.get_config_version(1).unwrap().unwrap();
        assert!(applied.allow_free_text);
        assert!(store.get_config_version(99).unwrap().is_none());
    }
}
