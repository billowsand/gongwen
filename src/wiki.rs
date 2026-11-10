//! 维基知识库存储：分类框架（主题→分类两级）、页面（唯一 ID + 标题）、
//! 全量版本链、链接边（含红链）与资料文档「已用于维基」确认标记。
//!
//! 表结构方案见 `docs/wiki-knowledge-base-design.md` §8。与稿件库同一个
//! SQLite 文件（SQLCipher 自动覆盖），建表走 `ensure_schema`，由
//! `ManuscriptStore::migrate` 统一调用，任何开库方先到先建。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{Connection, OptionalExtension, params};

/// 框架节点：主题（kind=theme）或分类（kind=category），两级，程序保证。
#[derive(Debug, Clone)]
pub(crate) struct WikiNode {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub kind: &'static str,
}

/// 主题 → 其下分类，供界面直接渲染框架树。
#[derive(Debug, Clone)]
pub(crate) struct WikiTree {
    pub theme: WikiNode,
    pub categories: Vec<WikiNode>,
    /// 每个节点挂的导览页 UUID（没有则为 None）。
    pub theme_guide: Option<String>,
    pub category_guides: HashMap<i64, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct WikiPageMeta {
    pub uuid: String,
    pub title: String,
    pub node_id: i64,
    pub kind: &'static str,
    pub current_version: i64,
    pub updated_at: String,
}

/// 页面连同当前版本正文。
#[derive(Debug, Clone)]
pub(crate) struct WikiPage {
    pub meta: WikiPageMeta,
    pub body: String,
}

#[derive(Debug, Clone)]
pub(crate) struct WikiVersionMeta {
    pub version: i64,
    pub source_run_id: String,
    pub review_edited: bool,
    pub created_at: String,
}

/// 正文里解析出的一条 `[[目标]]` / `[[目标|显示]]`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawLink {
    /// 目标标题（不含别名）。
    pub target: String,
    /// 显示文字；无别名时等于 target。
    pub text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct WikiLink {
    /// 出链目标页 UUID；红链为 None。
    pub to_page: Option<String>,
    /// 目标节点（导览页链接指向主题/分类节点）；页面链接为 None。
    pub to_node: Option<i64>,
    /// 红链的目标标题（未建页）。
    pub to_title: String,
    pub text: String,
    pub resolved: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Backlink {
    pub from_uuid: String,
    pub from_title: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct WikiSearchHit {
    pub uuid: String,
    pub title: String,
    pub node_id: i64,
}

/// 资料文档的维基状态：doc_id → 引用它的页面数（0 = 未整编）。
pub(crate) type DocWikiStatus = HashMap<i64, i64>;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS wiki_nodes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id  INTEGER REFERENCES wiki_nodes(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL DEFAULT '',
    kind       TEXT    NOT NULL DEFAULT 'category',
    ord        INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wiki_nodes_parent ON wiki_nodes(parent_id);

-- 页面：seq 是库内整数主键（FTS rowid 用），uuid 是稳定身份（链接按它解析，
-- 标题改名不影响链接）。标题全局唯一，供 [[目标]] 按标题解析。
CREATE TABLE IF NOT EXISTS wiki_pages (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid            TEXT    NOT NULL UNIQUE,
    title           TEXT    NOT NULL UNIQUE,
    node_id         INTEGER NOT NULL REFERENCES wiki_nodes(id),
    kind            TEXT    NOT NULL DEFAULT 'normal',
    current_version INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wiki_pages_node ON wiki_pages(node_id);

CREATE TABLE IF NOT EXISTS wiki_page_versions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    page_id       TEXT    NOT NULL REFERENCES wiki_pages(uuid) ON DELETE CASCADE,
    version       INTEGER NOT NULL,
    body          TEXT    NOT NULL,
    source_run_id TEXT    NOT NULL DEFAULT '',
    review_edited INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    UNIQUE (page_id, version)
);

-- 链接边：入库时从正文确定性解析。to_page 指向页面、to_node 指向框架节点
-- （导览页），两者都为空且 resolved=0 时是红链（待建页，to_title 记目标标题）。
CREATE TABLE IF NOT EXISTS wiki_links (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    from_page TEXT    NOT NULL REFERENCES wiki_pages(uuid) ON DELETE CASCADE,
    to_page   TEXT    REFERENCES wiki_pages(uuid) ON DELETE CASCADE,
    to_node   INTEGER REFERENCES wiki_nodes(id) ON DELETE CASCADE,
    to_title  TEXT    NOT NULL DEFAULT '',
    link_text TEXT    NOT NULL DEFAULT '',
    resolved  INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_wiki_links_from ON wiki_links(from_page);
CREATE INDEX IF NOT EXISTS idx_wiki_links_to_page ON wiki_links(to_page);

-- 资料「已用于维基」确认标记，挂在版本上（哪一版用了哪些资料、各支撑什么）。
CREATE TABLE IF NOT EXISTS wiki_doc_refs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    page_id    TEXT    NOT NULL REFERENCES wiki_pages(uuid) ON DELETE CASCADE,
    version_id INTEGER NOT NULL REFERENCES wiki_page_versions(id) ON DELETE CASCADE,
    doc_id     INTEGER NOT NULL,
    note       TEXT    NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_wiki_doc_refs_doc ON wiki_doc_refs(doc_id);

-- 整页全文索引（jieba 分词后写入，与 knowledge_local_fts 同模式）。
CREATE VIRTUAL TABLE IF NOT EXISTS wiki_fts USING fts5(tokens, tokenize='unicode61');
"#;

/// 建页/更新的入参包（字段多，成组传）。
#[derive(Debug, Clone, Default)]
pub(crate) struct PageInput {
    pub title: String,
    pub node_id: i64,
    pub kind: &'static str,
    pub body: String,
    pub source_run_id: String,
    pub review_edited: bool,
    /// 依据清单：资料库 doc_id + 支撑说明。
    pub doc_refs: Vec<(i64, String)>,
}

pub(crate) struct WikiStore {
    conn: Connection,
}

impl WikiStore {
    /// 应用层打开持久库（与稿件库同一文件，SQLCipher 自动覆盖）；测试走内存库。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("无法打开维基知识库 {}", path.display()))?;
        crate::storage::apply_db_key(&conn)?;
        // 与知识库连接一致：写作侧可能正持有写事务，界面连接要肯等。
        conn.busy_timeout(std::time::Duration::from_millis(10_000))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL")?;
        ensure_schema(&conn)?;
        Ok(Self { conn })
    }

    #[cfg(test)]
    fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        ensure_schema(&conn)?;
        Ok(Self { conn })
    }

    fn now() -> String {
        Local::now().to_rfc3339()
    }

    // ── 框架节点 ─────────────────────────────────────────────────────────

    pub fn create_theme(&self, name: &str) -> Result<i64> {
        let now = Self::now();
        self.conn.execute(
            "INSERT INTO wiki_nodes (parent_id, name, kind, created_at, updated_at)
             VALUES (NULL, ?1, 'theme', ?2, ?2)",
            params![name.trim(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 新建分类，必须挂在某个主题下（两级框架，分类下不再分层）。
    pub fn create_category(&self, theme_id: i64, name: &str) -> Result<i64> {
        let kind: Option<String> = self
            .conn
            .query_row(
                "SELECT kind FROM wiki_nodes WHERE id = ?1",
                params![theme_id],
                |r| r.get(0),
            )
            .optional()?;
        match kind.as_deref() {
            Some("theme") => {}
            Some(_) => anyhow::bail!("分类只能挂在主题下"),
            None => anyhow::bail!("主题不存在"),
        }
        let now = Self::now();
        self.conn.execute(
            "INSERT INTO wiki_nodes (parent_id, name, kind, created_at, updated_at)
             VALUES (?1, ?2, 'category', ?3, ?3)",
            params![theme_id, name.trim(), now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_node(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE wiki_nodes SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name.trim(), Self::now(), id],
        )?;
        Ok(())
    }

    /// 把分类移到另一个主题下。主题不能移动（总在顶层）。
    pub fn move_category(&self, id: i64, new_theme_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE wiki_nodes SET parent_id = ?1, updated_at = ?2
             WHERE id = ?3 AND kind = 'category'",
            params![new_theme_id, Self::now(), id],
        )?;
        Ok(())
    }

    /// 删除节点：分类下无页面、主题下无分类且无页面时才允许；
    /// 页面必须先移走或删除（程序把关，防止页面悬空）。
    pub fn delete_node(&self, id: i64) -> Result<()> {
        let kind: String = self.conn.query_row(
            "SELECT kind FROM wiki_nodes WHERE id=?1",
            params![id],
            |r| r.get(0),
        )?;
        if kind == "theme" {
            let cats: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM wiki_nodes WHERE parent_id=?1",
                params![id],
                |r| r.get(0),
            )?;
            if cats > 0 {
                anyhow::bail!("主题下还有分类，先删除或移走分类");
            }
        }
        let pages: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM wiki_pages WHERE node_id=?1",
            params![id],
            |r| r.get(0),
        )?;
        if pages > 0 {
            anyhow::bail!("节点下还有页面，先移走或删除页面");
        }
        self.conn
            .execute("DELETE FROM wiki_nodes WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn list_tree(&self) -> Result<Vec<WikiTree>> {
        let mut themes = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, parent_id, name, kind FROM wiki_nodes WHERE kind='theme' ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(WikiNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                kind: "theme",
            })
        })?;
        for t in rows {
            themes.push(t?);
        }
        let mut cats_stmt = self.conn.prepare(
            "SELECT id, parent_id, name, kind FROM wiki_nodes WHERE kind='category' AND parent_id=?1 ORDER BY id",
        )?;
        let mut guide_stmt = self
            .conn
            .prepare("SELECT uuid FROM wiki_pages WHERE node_id=?1 AND kind='guide'")?;
        let mut out = Vec::new();
        for theme in themes {
            let categories = cats_stmt
                .query_map(params![theme.id], |r| {
                    Ok(WikiNode {
                        id: r.get(0)?,
                        parent_id: r.get(1)?,
                        name: r.get(2)?,
                        kind: "category",
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let theme_guide: Option<String> = guide_stmt
                .query_row(params![theme.id], |r| r.get(0))
                .optional()?;
            let mut category_guides = HashMap::new();
            for c in &categories {
                if let Some(g) = guide_stmt
                    .query_row(params![c.id], |r| r.get(0))
                    .optional()?
                {
                    category_guides.insert(c.id, g);
                }
            }
            out.push(WikiTree {
                theme,
                categories,
                theme_guide,
                category_guides,
            });
        }
        Ok(out)
    }

    // ── 页面与版本 ───────────────────────────────────────────────────────

    /// 建页并落 v1：解析正文链接、登记依据、写全文索引，一个事务完成。
    pub fn create_page(&self, input: &PageInput) -> Result<String> {
        if self.page_by_title(&input.title)?.is_some() {
            anyhow::bail!("已存在同名页面「{}」", input.title);
        }
        if input.kind == "guide" && self.guide_of_node(input.node_id)?.is_some() {
            anyhow::bail!("该节点已有导览页");
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        let now = Self::now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO wiki_pages (uuid, title, node_id, kind, current_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
            params![uuid, input.title.trim(), input.node_id, input.kind, now],
        )?;
        let seq = tx.last_insert_rowid();
        let version_id = insert_version(
            &tx,
            &uuid,
            1,
            &input.body,
            &input.source_run_id,
            input.review_edited,
        )?;
        insert_doc_refs(&tx, &uuid, version_id, &input.doc_refs)?;
        write_fts(&tx, seq, &input.title, &input.body)?;
        write_links(&tx, &uuid, &input.body)?;
        tx.commit()?;
        Ok(uuid)
    }

    /// 更新页面：落一个新版本（全量快照），链接边与全文索引重建。
    pub fn update_page(&self, uuid: &str, input: &PageInput) -> Result<i64> {
        let old = self
            .get_page(uuid)?
            .with_context(|| format!("页面不存在：{uuid}"))?;
        if old.meta.title != input.title.trim()
            && self
                .page_by_title(&input.title)?
                .is_some_and(|o| o.uuid != uuid)
        {
            anyhow::bail!("已存在同名页面「{}」", input.title);
        }
        let version = old.meta.current_version + 1;
        let now = Self::now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE wiki_pages SET title=?1, node_id=?2, current_version=?3, updated_at=?4 WHERE uuid=?5",
            params![input.title.trim(), input.node_id, version, now, uuid],
        )?;
        let version_id = insert_version(
            &tx,
            uuid,
            version,
            &input.body,
            &input.source_run_id,
            input.review_edited,
        )?;
        tx.execute("DELETE FROM wiki_doc_refs WHERE page_id=?1", params![uuid])?;
        insert_doc_refs(&tx, uuid, version_id, &input.doc_refs)?;
        let seq: i64 = tx.query_row(
            "SELECT seq FROM wiki_pages WHERE uuid=?1",
            params![uuid],
            |r| r.get(0),
        )?;
        write_fts(&tx, seq, &input.title, &input.body)?;
        write_links(&tx, uuid, &input.body)?;
        tx.commit()?;
        Ok(version)
    }

    /// 回滚：取旧版本正文生成一个新版本（版本链只前进）。
    pub fn rollback(&self, uuid: &str, version: i64) -> Result<i64> {
        let old_body = self
            .get_version_body(uuid, version)?
            .with_context(|| format!("版本不存在：v{version}"))?;
        let cur = self.get_page(uuid)?.context("页面不存在")?;
        self.update_page(
            uuid,
            &PageInput {
                title: cur.meta.title,
                node_id: cur.meta.node_id,
                kind: cur.meta.kind,
                body: old_body,
                source_run_id: format!("rollback-from-v{version}"),
                review_edited: true,
                doc_refs: Vec::new(),
            },
        )
    }

    pub fn get_page(&self, uuid: &str) -> Result<Option<WikiPage>> {
        let meta = self
            .conn
            .query_row(
                "SELECT uuid, title, node_id, kind, current_version, updated_at
                 FROM wiki_pages WHERE uuid=?1",
                params![uuid],
                |r| {
                    Ok(WikiPageMeta {
                        uuid: r.get(0)?,
                        title: r.get(1)?,
                        node_id: r.get(2)?,
                        kind: box_kind(&r.get::<_, String>(3)?),
                        current_version: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .optional()?;
        let Some(meta) = meta else { return Ok(None) };
        let body = self
            .get_version_body(uuid, meta.current_version)?
            .unwrap_or_default();
        Ok(Some(WikiPage { meta, body }))
    }

    pub fn page_by_title(&self, title: &str) -> Result<Option<WikiPageMeta>> {
        let meta = self
            .conn
            .query_row(
                "SELECT uuid, title, node_id, kind, current_version, updated_at
                 FROM wiki_pages WHERE title=?1",
                params![title.trim()],
                |r| {
                    Ok(WikiPageMeta {
                        uuid: r.get(0)?,
                        title: r.get(1)?,
                        node_id: r.get(2)?,
                        kind: box_kind(&r.get::<_, String>(3)?),
                        current_version: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(meta)
    }

    pub fn list_pages(&self, node_id: i64) -> Result<Vec<WikiPageMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT uuid, title, node_id, kind, current_version, updated_at
             FROM wiki_pages WHERE node_id=?1 ORDER BY kind, seq",
        )?;
        let rows = stmt.query_map(params![node_id], |r| {
            Ok(WikiPageMeta {
                uuid: r.get(0)?,
                title: r.get(1)?,
                node_id: r.get(2)?,
                kind: box_kind(&r.get::<_, String>(3)?),
                current_version: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn list_versions(&self, uuid: &str) -> Result<Vec<WikiVersionMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT version, source_run_id, review_edited, created_at
             FROM wiki_page_versions WHERE page_id=?1 ORDER BY version DESC",
        )?;
        let rows = stmt.query_map(params![uuid], |r| {
            Ok(WikiVersionMeta {
                version: r.get(0)?,
                source_run_id: r.get(1)?,
                review_edited: r.get::<_, i64>(2)? != 0,
                created_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get_version_body(&self, uuid: &str, version: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT body FROM wiki_page_versions WHERE page_id=?1 AND version=?2",
                params![uuid, version],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 删除页面：被其他页面引用时拒绝（删除保护）。
    pub fn delete_page(&self, uuid: &str) -> Result<()> {
        let refs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM wiki_links WHERE to_page=?1",
            params![uuid],
            |r| r.get(0),
        )?;
        if refs > 0 {
            anyhow::bail!("该页面被 {refs} 个页面引用，先解除引用才能删除");
        }
        let seq: Option<i64> = self
            .conn
            .query_row(
                "SELECT seq FROM wiki_pages WHERE uuid=?1",
                params![uuid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(seq) = seq {
            self.conn
                .execute("DELETE FROM wiki_fts WHERE rowid=?1", params![seq])?;
        }
        self.conn
            .execute("DELETE FROM wiki_pages WHERE uuid=?1", params![uuid])?;
        Ok(())
    }

    /// 页面的入链（backlinks）。
    pub fn backlinks(&self, uuid: &str) -> Result<Vec<Backlink>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.uuid, p.title, l.link_text FROM wiki_links l
             JOIN wiki_pages p ON p.uuid = l.from_page
             WHERE l.to_page=?1 ORDER BY p.title",
        )?;
        let rows = stmt.query_map(params![uuid], |r| {
            Ok(Backlink {
                from_uuid: r.get(0)?,
                from_title: r.get(1)?,
                text: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// 页面的出链。
    pub fn outgoing_links(&self, uuid: &str) -> Result<Vec<WikiLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT to_page, to_node, to_title, link_text, resolved
             FROM wiki_links WHERE from_page=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![uuid], |r| {
            Ok(WikiLink {
                to_page: r.get(0)?,
                to_node: r.get(1)?,
                to_title: r.get(2)?,
                text: r.get(3)?,
                resolved: r.get::<_, i64>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// 语义检索：jieba 分词后 FTS 匹配（标题 + 正文），空查询返回空。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<WikiSearchHit>> {
        let tokens = crate::rag::tokenize(query);
        let words: Vec<&str> = tokens.split_whitespace().collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let phrase = words
            .iter()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = "SELECT p.uuid, p.title, p.node_id FROM wiki_fts f
             JOIN wiki_pages p ON p.seq = f.rowid
             WHERE wiki_fts MATCH ?1 ORDER BY rank LIMIT ?2"
            .to_string();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![phrase, limit as i64], |r| {
            Ok(WikiSearchHit {
                uuid: r.get(0)?,
                title: r.get(1)?,
                node_id: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// 资料文档的维基状态：doc_id → 引用它的页面数（按各页当前版本计）。
    pub fn doc_wiki_status(&self) -> Result<DocWikiStatus> {
        let mut stmt = self.conn.prepare(
            "SELECT r.doc_id, COUNT(DISTINCT r.page_id) FROM wiki_doc_refs r
             JOIN wiki_pages p ON p.uuid = r.page_id
             JOIN wiki_page_versions v ON v.id = r.version_id
             WHERE v.version = p.current_version
             GROUP BY r.doc_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// 某页当前版本的依据清单。
    pub fn page_doc_refs(&self, uuid: &str) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.doc_id, r.note FROM wiki_doc_refs r
             JOIN wiki_pages p ON p.uuid = r.page_id
             JOIN wiki_page_versions v ON v.id = r.version_id
             WHERE r.page_id=?1 AND v.version = p.current_version
             ORDER BY r.id",
        )?;
        let rows = stmt.query_map(params![uuid], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn guide_of_node(&self, node_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT uuid FROM wiki_pages WHERE node_id=?1 AND kind='guide'",
                params![node_id],
                |r| r.get(0),
            )
            .optional()?)
    }
}

/// 幂等建表。`ManuscriptStore::migrate` 调用，任何开库方先到先建。
pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)
}

fn box_kind(kind: &str) -> &'static str {
    match kind {
        "guide" => "guide",
        _ => "normal",
    }
}

fn insert_version(
    tx: &rusqlite::Transaction,
    uuid: &str,
    version: i64,
    body: &str,
    source_run_id: &str,
    review_edited: bool,
) -> Result<i64> {
    tx.execute(
        "INSERT INTO wiki_page_versions (page_id, version, body, source_run_id, review_edited, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            uuid,
            version,
            body,
            source_run_id,
            i64::from(review_edited),
            WikiStore::now()
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn insert_doc_refs(
    tx: &rusqlite::Transaction,
    uuid: &str,
    version_id: i64,
    doc_refs: &[(i64, String)],
) -> Result<()> {
    for (doc_id, note) in doc_refs {
        tx.execute(
            "INSERT INTO wiki_doc_refs (page_id, version_id, doc_id, note) VALUES (?1, ?2, ?3, ?4)",
            params![uuid, version_id, doc_id, note],
        )?;
    }
    Ok(())
}

/// 写全文索引：先删旧行再插（更新路径同样调用）。
fn write_fts(tx: &rusqlite::Transaction, seq: i64, title: &str, body: &str) -> Result<()> {
    tx.execute("DELETE FROM wiki_fts WHERE rowid=?1", params![seq])?;
    tx.execute(
        "INSERT INTO wiki_fts (rowid, tokens) VALUES (?1, ?2)",
        params![seq, crate::rag::tokenize(&format!("{title}\n{body}"))],
    )?;
    Ok(())
}

/// 从正文确定性解析 `[[链接]]`，重建该页的出链边。
/// 目标解析顺序：页面标题 → 框架节点名（主题/分类，导览页链接）→ 红链。
fn write_links(tx: &rusqlite::Transaction, from_uuid: &str, body: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM wiki_links WHERE from_page=?1",
        params![from_uuid],
    )?;
    for raw in parse_wikilinks(body) {
        let target = raw.target.trim();
        if target.is_empty() {
            continue;
        }
        let page: Option<String> = tx
            .query_row(
                "SELECT uuid FROM wiki_pages WHERE title=?1",
                params![target],
                |r| r.get(0),
            )
            .optional()?;
        let (to_page, to_node, resolved) = if let Some(p) = page {
            (Some(p), None, 1)
        } else {
            let node: Option<i64> = tx
                .query_row(
                    "SELECT id FROM wiki_nodes WHERE name=?1",
                    params![target],
                    |r| r.get(0),
                )
                .optional()?;
            match node {
                Some(n) => (None, Some(n), 1),
                None => (None, None, 0),
            }
        };
        tx.execute(
            "INSERT INTO wiki_links (from_page, to_page, to_node, to_title, link_text, resolved)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                from_uuid,
                to_page,
                to_node,
                if resolved == 1 { "" } else { target },
                raw.text,
                resolved
            ],
        )?;
    }
    Ok(())
}

/// 框架导入大纲：一个主题与它名下的分类（两级，不更深）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutlineTheme {
    pub name: String,
    pub categories: Vec<String>,
}

/// 把用户粘贴的 Markdown 大纲解析成两级框架。
///
/// 支持两种写法、可混排：标题式（`# 主题` / `## 分类`）与列表式
/// （`- 主题` / 缩进一格的 `- 分类`）。第三级直接报错；完全解析不出
/// 任何主题也报错。重名主题、主题内重名分类在这里就拦下。
pub(crate) fn parse_frame_outline(text: &str) -> Result<Vec<OutlineTheme>, String> {
    let mut themes: Vec<OutlineTheme> = Vec::new();
    let mut list_theme: Option<usize> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let name = trimmed[level..].trim().trim_matches('*').trim().to_string();
            if name.is_empty() {
                continue;
            }
            match level {
                1 => {
                    themes.push(OutlineTheme {
                        name,
                        categories: Vec::new(),
                    });
                    // 混排时允许标题主题后面接列表式分类。
                    list_theme = Some(themes.len() - 1);
                }
                2 => {
                    let Some(theme) = themes.last_mut() else {
                        return Err(format!("「{name}」前面没有一级主题（# 主题）"));
                    };
                    if theme.categories.contains(&name) {
                        return Err(format!("主题「{}」下分类「{name}」重复", theme.name));
                    }
                    theme.categories.push(name);
                }
                _ => return Err("框架只分两级（主题 → 分类），不支持三级标题".into()),
            }
            continue;
        }
        // 列表式：按缩进深度定层级（0 = 主题，1 = 分类，更深拒绝）。
        let body = line.trim_start();
        let Some(item) = body.strip_prefix("- ") else {
            return Err(format!("认不出这一行（要用 # / ## 或 - 列表）：{line}"));
        };
        let depth = (line.len() - body.len()) / 2;
        let name = item.trim().trim_matches('*').trim().to_string();
        if name.is_empty() {
            continue;
        }
        match depth {
            0 => {
                themes.push(OutlineTheme {
                    name,
                    categories: Vec::new(),
                });
                list_theme = Some(themes.len() - 1);
            }
            1 => {
                let Some(index) = list_theme else {
                    return Err(format!("「{name}」前面没有一级主题（- 主题）"));
                };
                if themes[index].categories.contains(&name) {
                    return Err(format!(
                        "主题「{}」下分类「{name}」重复",
                        themes[index].name
                    ));
                }
                themes[index].categories.push(name);
            }
            _ => return Err("框架只分两级（主题 → 分类），列表缩进不要深于一层".into()),
        }
    }
    if themes.is_empty() {
        return Err(
            "没有解析出任何主题；用 `# 主题` / `## 分类` 或 `- 主题` / 缩进 `- 分类` 写".into(),
        );
    }
    let mut seen = std::collections::HashSet::new();
    for theme in &themes {
        if theme.name.chars().count() > 60 {
            return Err(format!("主题名超过 60 字：{}", theme.name));
        }
        if !seen.insert(theme.name.clone()) {
            return Err(format!("主题「{}」重复", theme.name));
        }
        for category in &theme.categories {
            if category.chars().count() > 60 {
                return Err(format!("分类名超过 60 字：{category}"));
            }
        }
    }
    Ok(themes)
}

/// 解析正文里的 `[[目标]]` 与 `[[目标|显示]]`（支持嵌套 `[` 的标题不做转义，
/// 公文标题不含 `]`，扫描即可）。
pub(crate) fn parse_wikilinks(body: &str) -> Vec<RawLink> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < bytes.len() && !(bytes[j] == b']' && bytes[j + 1] == b']') {
                j += 1;
            }
            if j + 1 < bytes.len() {
                let inner = &body[start..j];
                let (target, text) = match inner.split_once('|') {
                    Some((t, x)) => (t.trim(), x.trim()),
                    None => (inner.trim(), inner.trim()),
                };
                if !target.is_empty() && !text.is_empty() {
                    out.push(RawLink {
                        target: target.to_string(),
                        text: text.to_string(),
                    });
                }
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests;
