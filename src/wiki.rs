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
    pub guidance: String,
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

/// 红链缺口：同一个未建页目标（to_title）被哪些页面引用。
#[derive(Debug, Clone)]
pub(crate) struct RedLinkGap {
    pub title: String,
    pub refs: Vec<Backlink>,
}

#[derive(Debug, Clone)]
pub(crate) struct WikiSearchHit {
    pub uuid: String,
    pub title: String,
    pub node_id: i64,
}

/// 资料文档的维基状态：doc_id → 引用它的页面数（0 = 未整编）。
pub(crate) type DocWikiStatus = HashMap<i64, i64>;

/// 一次检索最多取前多少个分词去匹配 FTS5（问题可以很长，只取开头这些词）。
const MAX_QUERY_TERMS: usize = 32;

/// 与 Pi 作业闸门一致的结构上限；人工建页/改名也走同一口径，避免绕过检查。
const MAX_NODE_NAME_CHARS: usize = 60;
const MAX_PAGE_TITLE_CHARS: usize = 100;
const MAX_PAGE_BODY_CHARS: usize = 20_000;
const MAX_FRAME_THEMES: usize = 30;
const MAX_FRAME_CATEGORIES_PER_THEME: usize = 20;

pub(crate) const MAX_FRAME_GUIDANCE_CHARS: usize = 600;

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
CREATE TABLE IF NOT EXISTS wiki_node_guidance (
    node_id  INTEGER PRIMARY KEY REFERENCES wiki_nodes(id) ON DELETE CASCADE,
    guidance TEXT NOT NULL
);

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
CREATE INDEX IF NOT EXISTS idx_wiki_doc_refs_version ON wiki_doc_refs(version_id);

-- 整页全文索引（jieba 分词后写入，与 knowledge_local_fts 同模式）。
-- 框架确认的资料映射供整编使用；删除原文时自动清掉映射。
CREATE TABLE IF NOT EXISTS wiki_category_docs (
    node_id INTEGER NOT NULL REFERENCES wiki_nodes(id) ON DELETE CASCADE,
    doc_id INTEGER NOT NULL REFERENCES knowledge_docs(id) ON DELETE CASCADE,
    PRIMARY KEY (node_id, doc_id)
);

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
    pub base_version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameTheme {
    pub name: String,
    pub guidance: String,
    pub categories: Vec<FrameCategory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameCategory {
    pub name: String,
    pub guidance: String,
    pub doc_ids: Vec<i64>,
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
        conn.execute_batch(crate::manuscript::DDL_V5)?;
        ensure_schema(&conn)?;
        Ok(Self { conn })
    }

    fn now() -> String {
        Local::now().to_rfc3339()
    }

    // ── 框架节点 ─────────────────────────────────────────────────────────

    /// 将用户确认的一整份框架合并入库。已有同名节点保留身份。
    pub fn apply_frame(&self, themes: &[FrameTheme]) -> Result<(usize, usize)> {
        anyhow::ensure!(
            themes.len() <= MAX_FRAME_THEMES,
            "框架主题超过 {MAX_FRAME_THEMES} 个，请拆分后再确认"
        );
        let tx = self.conn.unchecked_transaction()?;
        let mut created = 0;
        let mut reused = 0;
        for theme in themes {
            let theme_name = theme.name.trim();
            anyhow::ensure!(!theme_name.is_empty(), "主题名不能为空");
            anyhow::ensure!(
                theme_name.chars().count() <= MAX_NODE_NAME_CHARS,
                "主题名超过 {MAX_NODE_NAME_CHARS} 字：{theme_name}"
            );
            anyhow::ensure!(
                theme.categories.len() <= MAX_FRAME_CATEGORIES_PER_THEME,
                "主题「{theme_name}」下分类超过 {MAX_FRAME_CATEGORIES_PER_THEME} 个，请拆分后再确认"
            );
            let existing = unique_node_id(&tx, None, "theme", theme_name)?;
            let theme_id = if let Some(id) = existing {
                reused += 1;
                id
            } else {
                created += 1;
                self.create_theme(theme_name)?
            };
            fill_node_guidance(&tx, theme_id, &theme.guidance)?;
            rebind_unresolved(&tx, theme_name)?;
            for category in &theme.categories {
                let category_name = category.name.trim();
                anyhow::ensure!(!category_name.is_empty(), "分类名不能为空");
                anyhow::ensure!(
                    category_name.chars().count() <= MAX_NODE_NAME_CHARS,
                    "分类名超过 {MAX_NODE_NAME_CHARS} 字：{category_name}"
                );
                let existing = unique_node_id(&tx, Some(theme_id), "category", category_name)?;
                let node = if let Some(id) = existing {
                    reused += 1;
                    id
                } else {
                    created += 1;
                    self.create_category(theme_id, category_name)?
                };
                fill_node_guidance(&tx, node, &category.guidance)?;
                rebind_unresolved(&tx, category_name)?;
                for doc_id in &category.doc_ids {
                    tx.execute(
                        "INSERT OR IGNORE INTO wiki_category_docs(node_id, doc_id) VALUES (?1, ?2)",
                        params![node, doc_id],
                    )
                    .with_context(|| format!("无法为「{}」登记资料 E{doc_id}", category.name))?;
                }
            }
        }
        tx.commit()?;
        Ok((created, reused))
    }

    pub fn frame_doc_ids(&self, node_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT d.doc_id FROM wiki_category_docs d JOIN wiki_nodes n ON n.id=d.node_id
             WHERE n.id=?1 OR n.parent_id=?1 ORDER BY d.doc_id",
        )?;
        let rows = stmt.query_map([node_id], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn frame_documents(&self, node_id: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT k.id, k.title
             FROM wiki_category_docs d
             JOIN wiki_nodes n ON n.id=d.node_id
             JOIN knowledge_docs k ON k.id=d.doc_id
             WHERE n.id=?1 OR n.parent_id=?1 ORDER BY k.id",
        )?;
        let rows = stmt.query_map([node_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn create_theme(&self, name: &str) -> Result<i64> {
        let name = name.trim();
        validate_node_name(name)?;
        let now = Self::now();
        self.conn.execute(
            "INSERT INTO wiki_nodes (parent_id, name, kind, created_at, updated_at)
             VALUES (NULL, ?1, 'theme', ?2, ?2)",
            params![name, now],
        )?;
        let id = self.conn.last_insert_rowid();
        // 先有红链、后建同名主题：未解析链接立刻指向这个节点（与建页同一条规则）。
        rebind_unresolved(&self.conn, name)?;
        Ok(id)
    }

    /// 新建分类，必须挂在某个主题下（两级框架，分类下不再分层）。
    pub fn create_category(&self, theme_id: i64, name: &str) -> Result<i64> {
        let name = name.trim();
        validate_node_name(name)?;
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
            params![theme_id, name, now],
        )?;
        let id = self.conn.last_insert_rowid();
        rebind_unresolved(&self.conn, name)?;
        Ok(id)
    }

    /// 保存名称和分类口径；与链接重绑同一事务。
    pub fn save_node_definition(&self, id: i64, name: &str, guidance: &str) -> Result<()> {
        let name = name.trim();
        let guidance = guidance.trim();
        validate_node_name(name)?;
        validate_frame_guidance(guidance)?;
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE wiki_nodes SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, Self::now(), id],
        )?;
        anyhow::ensure!(changed == 1, "节点已不存在");
        if guidance.is_empty() {
            tx.execute("DELETE FROM wiki_node_guidance WHERE node_id=?1", [id])?;
        } else {
            tx.execute(
                "INSERT INTO wiki_node_guidance(node_id, guidance) VALUES (?1, ?2)
                 ON CONFLICT(node_id) DO UPDATE SET guidance=excluded.guidance",
                params![id, guidance],
            )?;
        }
        rebind_unresolved(&tx, name)?;
        tx.commit()?;
        Ok(())
    }

    /// 把分类移到另一个主题下。主题不能移动（总在顶层）。
    pub fn move_category(&self, id: i64, new_theme_id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE wiki_nodes SET parent_id = ?1, updated_at = ?2
             WHERE id = ?3 AND kind = 'category'
               AND EXISTS (SELECT 1 FROM wiki_nodes WHERE id=?1 AND kind='theme')",
            params![new_theme_id, Self::now(), id],
        )?;
        anyhow::ensure!(changed == 1, "只能把现有分类移到现有主题下");
        Ok(())
    }

    /// 删除节点：分类下无页面、主题下无分类且无页面时才允许；
    /// 页面必须先移走或删除（程序把关，防止页面悬空）。
    pub fn delete_node(&self, id: i64) -> Result<()> {
        let (kind, name): (String, String) = self
            .conn
            .query_row(
                "SELECT kind, name FROM wiki_nodes WHERE id=?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .context("节点已不存在")?;
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
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE wiki_links SET to_node=NULL, to_page=NULL, resolved=0,
               to_title=COALESCE(NULLIF(to_title,''), ?2)
             WHERE to_node=?1",
            params![id, name],
        )?;
        tx.execute("DELETE FROM wiki_nodes WHERE id=?1", [id])?;
        // 节点没了，指向它的链接退回按标题解析：有同名页面就落到页面（与 write_links
        // 的「页面优先」一致），没有就保持红链。
        rebind_unresolved(&tx, &name)?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_tree(&self) -> Result<Vec<WikiTree>> {
        let mut themes = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.parent_id, n.name, COALESCE(g.guidance, '')
             FROM wiki_nodes n LEFT JOIN wiki_node_guidance g ON g.node_id=n.id
             WHERE n.kind='theme' ORDER BY n.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(WikiNode {
                id: r.get(0)?,
                parent_id: r.get(1)?,
                name: r.get(2)?,
                kind: "theme",
                guidance: r.get(3)?,
            })
        })?;
        for t in rows {
            themes.push(t?);
        }
        let mut cats_stmt = self.conn.prepare(
            "SELECT n.id, n.parent_id, n.name, COALESCE(g.guidance, '')
             FROM wiki_nodes n LEFT JOIN wiki_node_guidance g ON g.node_id=n.id
             WHERE n.kind='category' AND n.parent_id=?1 ORDER BY n.id",
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
                        guidance: r.get(3)?,
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
        validate_page(&tx, input, true)?;
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
        write_links(&tx, &uuid, &input.body, &HashMap::new())?;
        // 缺口补齐：既有正文里指向本页标题的红链，随建页解析为页面链接。
        tx.execute(
            "UPDATE wiki_links SET to_page=?1, to_node=NULL, resolved=1
             WHERE resolved=0 AND to_title=?2",
            params![uuid, input.title.trim()],
        )?;
        tx.commit()?;
        Ok(uuid)
    }

    /// 更新页面：落一个新版本（全量快照），链接边与全文索引重建。
    pub fn update_page(&self, uuid: &str, input: &PageInput) -> Result<i64> {
        self.write_new_version(uuid, input, true)
    }

    /// 页面改名：只改显示标题（页面身份是 UUID，链接按 UUID 稳定跟随），
    /// 不产生新版本；全文索引重建，未解析的 `[[新标题]]` 红链随解析为页面。
    pub fn rename_page(&self, uuid: &str, title: &str) -> Result<()> {
        let title = title.trim();
        anyhow::ensure!(!title.is_empty(), "页面标题不能为空");
        anyhow::ensure!(
            title.chars().count() <= MAX_PAGE_TITLE_CHARS,
            "页面标题超过 {MAX_PAGE_TITLE_CHARS} 字"
        );
        let tx = self.conn.unchecked_transaction()?;
        let old = self
            .get_page(uuid)?
            .with_context(|| format!("页面不存在：{uuid}"))?;
        if old.meta.title != title {
            if self
                .page_by_title(title)?
                .is_some_and(|other| other.uuid != uuid)
            {
                anyhow::bail!("已存在同名页面「{title}」");
            }
            tx.execute(
                "UPDATE wiki_pages SET title=?1, updated_at=?2 WHERE uuid=?3",
                params![title, Self::now(), uuid],
            )?;
            let seq: i64 = tx.query_row(
                "SELECT seq FROM wiki_pages WHERE uuid=?1",
                params![uuid],
                |r| r.get(0),
            )?;
            write_fts(&tx, seq, title, &old.body)?;
            // 与建页 / 更新页同一条补齐规则：指向新标题的红链现在有落点了。
            tx.execute(
                "UPDATE wiki_links SET to_page=?1, to_node=NULL, resolved=1
                 WHERE resolved=0 AND to_title=?2",
                params![uuid, title],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn write_new_version(&self, uuid: &str, input: &PageInput, check_docs: bool) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
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
        if let Some(base) = input.base_version {
            anyhow::ensure!(
                old.meta.current_version == base,
                "页面已由 v{base} 变为 v{}，请按当前版本重新发起更新",
                old.meta.current_version
            );
        }
        anyhow::ensure!(old.meta.kind == input.kind, "不能改变页面类型");
        anyhow::ensure!(
            input.kind != "guide" || old.meta.node_id == input.node_id,
            "导览页不能移到其他节点"
        );
        validate_page(&tx, input, check_docs)?;
        let previous_links = self.link_bindings(uuid, &old.body)?;
        let version = old.meta.current_version + 1;
        let now = Self::now();
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
        insert_doc_refs(&tx, uuid, version_id, &input.doc_refs)?;
        let seq: i64 = tx.query_row(
            "SELECT seq FROM wiki_pages WHERE uuid=?1",
            params![uuid],
            |r| r.get(0),
        )?;
        write_fts(&tx, seq, &input.title, &input.body)?;
        write_links(&tx, uuid, &input.body, &previous_links)?;
        tx.execute(
            "UPDATE wiki_links SET to_page=?1, to_node=NULL, resolved=1
             WHERE resolved=0 AND to_title=?2",
            params![uuid, input.title.trim()],
        )?;
        tx.commit()?;
        Ok(version)
    }

    /// 回滚：取旧版本正文生成一个新版本（版本链只前进）。
    pub fn rollback(&self, uuid: &str, version: i64) -> Result<i64> {
        let old_body = self
            .get_version_body(uuid, version)?
            .with_context(|| format!("版本不存在：v{version}"))?;
        let cur = self.get_page(uuid)?.context("页面不存在")?;
        self.write_new_version(
            uuid,
            &PageInput {
                title: cur.meta.title,
                node_id: cur.meta.node_id,
                kind: cur.meta.kind,
                body: old_body,
                source_run_id: format!("rollback-from-v{version}"),
                review_edited: true,
                doc_refs: self.version_doc_refs(uuid, version)?,
                base_version: Some(cur.meta.current_version),
            },
            false,
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
        let tx = self.conn.unchecked_transaction()?;
        let refs: i64 = tx.query_row(
            "SELECT COUNT(DISTINCT l.from_page) FROM wiki_links l
             JOIN wiki_pages p ON p.uuid=?1
             WHERE l.from_page<>?1 AND (l.to_page=?1 OR (p.kind='guide' AND l.to_node=p.node_id))",
            [uuid],
            |r| r.get(0),
        )?;
        anyhow::ensure!(refs == 0, "该页面被 {refs} 个页面引用，先解除引用才能删除");
        tx.execute(
            "DELETE FROM wiki_fts WHERE rowid=(SELECT seq FROM wiki_pages WHERE uuid=?1)",
            [uuid],
        )?;
        tx.execute("DELETE FROM wiki_pages WHERE uuid=?1", [uuid])?;
        tx.commit()?;
        Ok(())
    }

    /// 页面的入链（backlinks）。同一来源页重复引用同一目标只算一条。
    pub fn backlinks(&self, uuid: &str) -> Result<Vec<Backlink>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.uuid, p.title, MIN(l.link_text) FROM wiki_links l
             JOIN wiki_pages p ON p.uuid = l.from_page
             JOIN wiki_pages target ON target.uuid=?1
             WHERE l.to_page=?1 OR (target.kind='guide' AND l.to_node=target.node_id)
             GROUP BY p.uuid, p.title
             ORDER BY p.title",
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

    /// 红链缺口：未建页目标按标题聚合（缺口清单的数据源）。
    /// 同一来源页重复引用同一目标只算一处，免得「被 N 页引用」虚高。
    pub fn red_link_gaps(&self) -> Result<Vec<RedLinkGap>> {
        let mut stmt = self.conn.prepare(
            "SELECT l.to_title, p.uuid, p.title, MIN(l.link_text) FROM wiki_links l
             JOIN wiki_pages p ON p.uuid = l.from_page
             WHERE l.resolved = 0
             GROUP BY l.to_title, p.uuid, p.title
             ORDER BY l.to_title, p.title",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Backlink {
                    from_uuid: r.get(1)?,
                    from_title: r.get(2)?,
                    text: r.get(3)?,
                },
            ))
        })?;
        let mut gaps: Vec<RedLinkGap> = Vec::new();
        for row in rows {
            let (title, link) = row?;
            match gaps.last_mut() {
                Some(gap) if gap.title == title => gap.refs.push(link),
                _ => gaps.push(RedLinkGap {
                    title,
                    refs: vec![link],
                }),
            }
        }
        Ok(gaps)
    }

    /// 关键词检索：jieba 分词后 FTS 匹配（标题 + 正文），空查询返回空。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<WikiSearchHit>> {
        self.search_pages(query, limit, false)
    }

    pub fn search_for_evidence(&self, query: &str, limit: usize) -> Result<Vec<WikiSearchHit>> {
        self.search_pages(query, limit, true)
    }

    fn search_pages(
        &self,
        query: &str,
        limit: usize,
        evidence_only: bool,
    ) -> Result<Vec<WikiSearchHit>> {
        let tokens = crate::rag::tokenize(query);
        let words: Vec<&str> = tokens.split_whitespace().collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        // FTS5 的字符串字面量里双引号要写成两个；分词结果可能带引号（用户输入
        // `"四不两直"` 这类），不转义会让整条 MATCH 变成语法错误，检索直接失败。
        // 同时只取前若干个词：问答问题可以很长，词太多既无助于命中，也会把
        // FTS5 表达式撑得很大。
        let phrase = words
            .iter()
            .take(MAX_QUERY_TERMS)
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = "SELECT p.uuid, p.title, p.node_id FROM wiki_fts f
             JOIN wiki_pages p ON p.seq = f.rowid
             WHERE wiki_fts MATCH ?1 AND (NOT ?3 OR (
                EXISTS (
                    SELECT 1 FROM wiki_page_versions ev
                    JOIN wiki_doc_refs er ON er.version_id=ev.id
                    WHERE ev.page_id=p.uuid AND ev.version=p.current_version
                )
                AND NOT EXISTS (
                    SELECT 1 FROM wiki_page_versions v
                    JOIN wiki_doc_refs r ON r.version_id=v.id
                    LEFT JOIN knowledge_docs d ON d.id=r.doc_id
                    WHERE v.page_id=p.uuid AND v.version=p.current_version AND d.id IS NULL
                )
             )) ORDER BY rank LIMIT ?2";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![phrase, limit as i64, evidence_only], |r| {
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

    /// 资料表的关联条目来自已入库页面的当前版本。
    pub fn source_pages(&self) -> Result<HashMap<i64, Vec<(String, String)>>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT r.doc_id, p.uuid, p.title FROM wiki_doc_refs r
             JOIN wiki_pages p ON p.uuid = r.page_id
             JOIN wiki_page_versions v ON v.id = r.version_id
             WHERE v.version = p.current_version ORDER BY p.title",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
            ))
        })?;
        let mut pages: HashMap<_, Vec<_>> = HashMap::new();
        for row in rows {
            let (doc, page) = row?;
            pages.entry(doc).or_default().push(page);
        }
        Ok(pages)
    }

    /// 某页当前版本的依据清单。
    pub fn page_doc_refs(&self, uuid: &str) -> Result<Vec<(i64, String)>> {
        let version = self
            .conn
            .query_row(
                "SELECT current_version FROM wiki_pages WHERE uuid=?1",
                [uuid],
                |r| r.get(0),
            )
            .optional()?;
        match version {
            Some(version) => self.version_doc_refs(uuid, version),
            None => Ok(Vec::new()),
        }
    }

    pub(crate) fn version_doc_refs(&self, uuid: &str, version: i64) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.doc_id, r.note FROM wiki_doc_refs r
             JOIN wiki_page_versions v ON v.id = r.version_id
             WHERE r.page_id=?1 AND v.version=?2
             ORDER BY r.id",
        )?;
        let rows = stmt.query_map(params![uuid, version], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// 当前正文里的原始链接名与已保存的稳定目标。旧库按边的入库顺序补读目标名。
    pub fn link_bindings(&self, uuid: &str, body: &str) -> Result<HashMap<String, WikiLink>> {
        let raw = parse_wikilinks(body);
        let edges = self.outgoing_links(uuid)?;
        let matching_order = raw.len() == edges.len();
        Ok(edges
            .into_iter()
            .enumerate()
            .filter_map(|(index, edge)| {
                let title = if !edge.to_title.is_empty() {
                    edge.to_title.clone()
                } else if matching_order {
                    raw[index].target.clone()
                } else {
                    return None;
                };
                Some((title, edge))
            })
            .collect())
    }

    pub fn page_has_missing_sources(&self, uuid: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
               SELECT 1 FROM wiki_pages p
               JOIN wiki_page_versions v ON v.page_id=p.uuid AND v.version=p.current_version
               JOIN wiki_doc_refs r ON r.version_id=v.id
               LEFT JOIN knowledge_docs d ON d.id=r.doc_id
               WHERE p.uuid=?1 AND d.id IS NULL
             )",
            [uuid],
            |r| r.get(0),
        )?)
    }

    /// 能否作为自动取证来源：当前版本至少登记了一条依据，且这些资料都还在。
    /// 人工建的无依据页只能人工阅读；补上依据（例如走更新作业）后才进入自动取证，
    /// 否则「每页有依据」的溯源口径会被一条手工正文绕开。
    pub fn page_evidence_ready(&self, uuid: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
               SELECT 1 FROM wiki_pages p
               JOIN wiki_page_versions v ON v.page_id=p.uuid AND v.version=p.current_version
               JOIN wiki_doc_refs r ON r.version_id=v.id
               LEFT JOIN knowledge_docs d ON d.id=r.doc_id
               WHERE p.uuid=?1
               GROUP BY p.uuid
               HAVING COUNT(r.id) > 0 AND COUNT(d.id) = COUNT(r.id)
             )",
            [uuid],
            |r| r.get(0),
        )?)
    }

    /// 当前依据已被移除的页面。保留历史引用，以便人工复核。
    pub fn pages_needing_review(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT p.uuid, p.title FROM wiki_pages p
             JOIN wiki_page_versions v ON v.page_id=p.uuid AND v.version=p.current_version
             JOIN wiki_doc_refs r ON r.version_id=v.id
             LEFT JOIN knowledge_docs d ON d.id=r.doc_id
             WHERE d.id IS NULL ORDER BY p.title",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
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

pub(crate) fn validate_frame_guidance(guidance: &str) -> Result<()> {
    anyhow::ensure!(
        guidance.chars().count() <= MAX_FRAME_GUIDANCE_CHARS,
        "分类口径最多 {MAX_FRAME_GUIDANCE_CHARS} 字"
    );
    Ok(())
}

fn fill_node_guidance(conn: &Connection, id: i64, guidance: &str) -> Result<()> {
    let guidance = guidance.trim();
    validate_frame_guidance(guidance)?;
    if !guidance.is_empty() {
        conn.execute(
            "INSERT OR IGNORE INTO wiki_node_guidance(node_id, guidance) VALUES (?1, ?2)",
            params![id, guidance],
        )?;
    }
    Ok(())
}

/// 节点名与 Pi 作业、导入大纲同一口径：非空、不超过 60 字。
fn validate_node_name(name: &str) -> Result<()> {
    anyhow::ensure!(!name.is_empty(), "名称不能为空");
    anyhow::ensure!(
        name.chars().count() <= MAX_NODE_NAME_CHARS,
        "名称超过 {MAX_NODE_NAME_CHARS} 字：{name}"
    );
    Ok(())
}

/// 按名字找一个唯一节点：同名节点有多个时明确报错，不按 id 顺序静默挑一个。
/// `parent` 为 None 表示找主题（主题总在顶层）。
fn unique_node_id(
    conn: &Connection,
    parent: Option<i64>,
    kind: &str,
    name: &str,
) -> Result<Option<i64>> {
    let mut ids = Vec::new();
    match parent {
        Some(parent) => {
            let mut stmt = conn.prepare(
                "SELECT id FROM wiki_nodes WHERE parent_id=?1 AND kind=?2 AND name=?3 ORDER BY id",
            )?;
            let rows = stmt.query_map(params![parent, kind, name], |r| r.get::<_, i64>(0))?;
            for row in rows {
                ids.push(row?);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT id FROM wiki_nodes WHERE parent_id IS NULL AND kind=?1 AND name=?2 ORDER BY id",
            )?;
            let rows = stmt.query_map(params![kind, name], |r| r.get::<_, i64>(0))?;
            for row in rows {
                ids.push(row?);
            }
        }
    }
    anyhow::ensure!(
        ids.len() <= 1,
        "已有多个同名{}「{name}」，先在知识库页改名后再确认",
        if kind == "theme" { "主题" } else { "分类" }
    );
    Ok(ids.into_iter().next())
}

/// 把仍未解析的 `[[标题]]` 边重新落点：页面优先，其次是唯一同名框架节点。
/// 与 `write_links` 的解析顺序一致，用于节点建立 / 改名 / 删除后的回填。
fn rebind_unresolved(conn: &Connection, title: &str) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Ok(());
    }
    let page: Option<String> = conn
        .query_row("SELECT uuid FROM wiki_pages WHERE title=?1", [title], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(uuid) = page {
        conn.execute(
            "UPDATE wiki_links SET to_page=?1, to_node=NULL, resolved=1
             WHERE resolved=0 AND to_title=?2",
            params![uuid, title],
        )?;
        return Ok(());
    }
    let node: Option<i64> = conn
        .query_row(
            "SELECT MIN(id) FROM wiki_nodes WHERE name=?1 HAVING COUNT(*)=1",
            [title],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = node {
        conn.execute(
            "UPDATE wiki_links SET to_page=NULL, to_node=?1, resolved=1
             WHERE resolved=0 AND to_title=?2",
            params![id, title],
        )?;
    }
    Ok(())
}

fn validate_page(tx: &rusqlite::Transaction, input: &PageInput, check_docs: bool) -> Result<()> {
    anyhow::ensure!(!input.title.trim().is_empty(), "页面标题不能为空");
    anyhow::ensure!(
        input.title.trim().chars().count() <= MAX_PAGE_TITLE_CHARS,
        "页面标题超过 {MAX_PAGE_TITLE_CHARS} 字"
    );
    anyhow::ensure!(!input.body.trim().is_empty(), "页面正文不能为空");
    anyhow::ensure!(
        input.body.chars().count() <= MAX_PAGE_BODY_CHARS,
        "页面正文超过 {MAX_PAGE_BODY_CHARS} 字，请拆成多页"
    );
    let node: String = tx
        .query_row(
            "SELECT kind FROM wiki_nodes WHERE id=?1",
            [input.node_id],
            |r| r.get(0),
        )
        .context("页面所属节点已不存在")?;
    anyhow::ensure!(
        input.kind == "guide" || (input.kind == "normal" && node == "category"),
        "普通页面必须归属一个分类"
    );
    if check_docs && !input.doc_refs.is_empty() {
        let mut exists = tx.prepare("SELECT EXISTS(SELECT 1 FROM knowledge_docs WHERE id=?1)")?;
        for (id, _) in &input.doc_refs {
            anyhow::ensure!(
                exists.query_row([id], |r| r.get::<_, bool>(0))?,
                "依据 E{id} 已不存在"
            );
        }
    }
    Ok(())
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
fn write_links(
    tx: &rusqlite::Transaction,
    from_uuid: &str,
    body: &str,
    previous: &HashMap<String, WikiLink>,
) -> Result<()> {
    tx.execute(
        "DELETE FROM wiki_links WHERE from_page=?1",
        params![from_uuid],
    )?;
    for raw in parse_wikilinks(body) {
        let target = raw.target.trim();
        if target.is_empty() {
            continue;
        }
        let (to_page, to_node) = if let Some(link) = previous.get(target) {
            (link.to_page.clone(), link.to_node)
        } else {
            let page: Option<String> = tx
                .query_row(
                    "SELECT uuid FROM wiki_pages WHERE title=?1",
                    [target],
                    |r| r.get(0),
                )
                .optional()?;
            let node = if page.is_none() {
                tx.query_row(
                    "SELECT MIN(id) FROM wiki_nodes WHERE name=?1 HAVING COUNT(*)=1",
                    [target],
                    |r| r.get(0),
                )
                .optional()?
            } else {
                None
            };
            (page, node)
        };
        let resolved = to_page.is_some() || to_node.is_some();
        tx.execute(
            "INSERT INTO wiki_links (from_page, to_page, to_node, to_title, link_text, resolved)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![from_uuid, to_page, to_node, target, raw.text, resolved],
        )?;
    }
    Ok(())
}

/// 从两级 Markdown 标题或列表导入框架。节点后的 `>` 行是它的分类口径。
pub(crate) fn parse_frame_outline(text: &str) -> Result<Vec<FrameTheme>, String> {
    let mut themes: Vec<FrameTheme> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rule) = trimmed.strip_prefix('>') {
            let theme = themes.last_mut().ok_or("分类口径前需要一个主题或分类")?;
            let guidance = theme
                .categories
                .last_mut()
                .map_or(&mut theme.guidance, |category| &mut category.guidance);
            let rule = rule.trim();
            if !rule.is_empty() {
                if !guidance.is_empty() {
                    guidance.push('\n');
                }
                guidance.push_str(rule);
                validate_frame_guidance(guidance).map_err(|error| error.to_string())?;
            }
            continue;
        }
        let (depth, item) = if trimmed.starts_with('#') {
            let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
            if level > 2 {
                return Err("框架只分两级（主题 → 分类），不支持三级标题".into());
            }
            (level - 1, &trimmed[level..])
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            let indent = &line[..line.len() - trimmed.len()];
            if indent.contains('\t') || indent.chars().count() % 2 != 0 {
                return Err(format!("列表缩进用两个空格一级，不要用 Tab：{line}"));
            }
            let depth = indent.chars().count() / 2;
            if depth > 1 {
                return Err("框架只分两级（主题 → 分类），列表缩进不要深于一层".into());
            }
            (depth, item)
        } else {
            return Err(format!(
                "认不出这一行（使用 # / ##、- 列表或 > 分类口径）：{line}"
            ));
        };
        let name = item.trim().trim_matches('*').trim().to_owned();
        if name.is_empty() {
            continue;
        }
        if depth == 0 {
            themes.push(FrameTheme {
                name,
                guidance: String::new(),
                categories: Vec::new(),
            });
        } else {
            let theme = themes
                .last_mut()
                .ok_or_else(|| format!("「{name}」前面没有一级主题"))?;
            if theme
                .categories
                .iter()
                .any(|category| category.name == name)
            {
                return Err(format!("主题「{}」下分类「{name}」重复", theme.name));
            }
            theme.categories.push(FrameCategory {
                name,
                guidance: String::new(),
                doc_ids: Vec::new(),
            });
        }
    }
    if themes.is_empty() {
        return Err(
            "没有解析出任何主题；用 `# 主题` / `## 分类` 或 `- 主题` / 缩进 `- 分类` 写".into(),
        );
    }
    if themes.len() > MAX_FRAME_THEMES {
        return Err(format!(
            "主题超过 {MAX_FRAME_THEMES} 个，请拆分后再导入（与 AI 归纳同一上限）"
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for theme in &themes {
        if theme.name.chars().count() > MAX_NODE_NAME_CHARS {
            return Err(format!(
                "主题名超过 {MAX_NODE_NAME_CHARS} 字：{}",
                theme.name
            ));
        }
        if !seen.insert(theme.name.clone()) {
            return Err(format!("主题「{}」重复", theme.name));
        }
        if theme.categories.len() > MAX_FRAME_CATEGORIES_PER_THEME {
            return Err(format!(
                "主题「{}」下分类超过 {MAX_FRAME_CATEGORIES_PER_THEME} 个，请拆分后再导入",
                theme.name
            ));
        }
        for category in &theme.categories {
            if category.name.chars().count() > MAX_NODE_NAME_CHARS {
                return Err(format!(
                    "分类名超过 {MAX_NODE_NAME_CHARS} 字：{}",
                    category.name
                ));
            }
        }
    }
    Ok(themes)
}

/// 解析正文里的 `[[目标]]` 与 `[[目标|显示]]`（支持嵌套 `[` 的标题不做转义，
/// 公文标题不含 `]`，扫描即可）。
pub(crate) fn parse_wikilinks(body: &str) -> Vec<RawLink> {
    wikilink_spans(body)
        .into_iter()
        .map(|(_, link)| link)
        .collect()
}

pub(crate) fn wikilink_spans(body: &str) -> Vec<(std::ops::Range<usize>, RawLink)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            let mut j = start;
            // 链接必须写在同一行内：跨行的 `[[` 会一路扫到文末，既不是合法页面名，
            // 也会让每个未闭合的 `[[` 重复扫描后半篇（阅读器每帧解析，长正文会卡）。
            let mut closed = false;
            while j + 1 < bytes.len() {
                if bytes[j] == b'\n' {
                    break;
                }
                if bytes[j] == b']' && bytes[j + 1] == b']' {
                    closed = true;
                    break;
                }
                j += 1;
            }
            if closed {
                let inner = &body[start..j];
                let (target, text) = match inner.split_once('|') {
                    Some((t, x)) => (t.trim(), x.trim()),
                    None => (inner.trim(), inner.trim()),
                };
                if !target.is_empty() && !text.is_empty() {
                    out.push((
                        i..j + 2,
                        RawLink {
                            target: target.to_string(),
                            text: text.to_string(),
                        },
                    ));
                }
                i = j + 2;
                continue;
            }
            // 本行没闭合：从行尾继续，不再反复重扫同一段未闭合文本。
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests;
