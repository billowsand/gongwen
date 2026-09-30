//! 送批材料：呈批件（主件）挂随行件的清单、引用规则与各件的版本状态。
//!
//! 方案见 `docs/send-package-design.md`。要点：
//! - 主件用本机 id（随主件删除级联清除）；随行件用稿件全局 UUID，同步到另一台
//!   电脑、或随行件尚未导入时关联都不断，解析时连同别名一起查。
//! - 送批材料只有一层：挂着随行件的稿件不能被当作随行件，被当作随行件的稿件不能再挂。
//! - 版本一律取离线同步版本图里的可见提交版本，不另起一套。
//! - 被已归档主件引用的稿件不能删除；其余被引用的稿件删除时连同关联一起清掉，
//!   删除前的二次确认由界面按 [`ManuscriptStore::send_package_referrers`] 列出。

use super::{ManuscriptStore, kind_to_str, str_to_kind, str_to_status};
use crate::models::{DraftInput, ManuscriptStatus, TemplateKind};
use anyhow::{Context, Result, bail};
use chrono::Local;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::HashSet;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS send_package_items (
    owner_id             INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    -- 随行件的稿件全局 UUID（manuscripts.document_uuid 或其别名），不做外键
    item_uuid            TEXT    NOT NULL,
    sort_order           INTEGER NOT NULL,
    -- 仅主件归档时写入钉住的 sync_revisions.revision_uuid；NULL 表示跟最新提交版
    pinned_revision_uuid TEXT,
    added_at             TEXT    NOT NULL,
    PRIMARY KEY (owner_id, item_uuid)
);
CREATE INDEX IF NOT EXISTS idx_send_package_items_item ON send_package_items(item_uuid);

-- 导出记录：每导出一次合并 PDF 记一条，只记本机，不进同步包。
CREATE TABLE IF NOT EXISTS send_package_exports (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id    INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    exported_at TEXT    NOT NULL,
    output_path TEXT    NOT NULL,
    with_toc    INTEGER NOT NULL DEFAULT 0,
    total_pages INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_send_package_exports_owner ON send_package_exports(owner_id);

-- 每次导出里每个件（主件 sort_order = 0）的身份与版本；标题、文种冗余保存，
-- 稿件以后改名或删除，记录照样读得懂。
CREATE TABLE IF NOT EXISTS send_package_export_items (
    export_id      INTEGER NOT NULL REFERENCES send_package_exports(id) ON DELETE CASCADE,
    sort_order     INTEGER NOT NULL,
    document_uuid  TEXT    NOT NULL,
    revision_uuid  TEXT    NOT NULL,
    payload_hash   TEXT    NOT NULL,
    visible_number INTEGER,
    title          TEXT    NOT NULL,
    kind           TEXT    NOT NULL,
    page_count     INTEGER NOT NULL,
    blank_pages    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (export_id, sort_order)
);
"#;

/// 幂等建表，与候选区、词表一样不单开 `user_version` 档位。
pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)
}

/// 能挂送批材料的文种：白头件与红头呈批件。
pub fn is_owner_kind(kind: TemplateKind) -> bool {
    matches!(
        kind,
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval
    )
}

/// 主件清单里的一个随行件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPackageItem {
    pub item_uuid: String,
    /// 本机对应的稿件；`None` 表示本机未找到（可能还没从同步包导入），关联照旧保留。
    pub manuscript_id: Option<i64>,
    /// 主件归档时钉住的版本；平时为 `None`，表示跟最新提交版。
    pub pinned_revision_uuid: Option<String>,
    pub added_at: String,
}

/// 一篇稿件不能加为某主件随行件的原因。界面据此置灰候选并说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddBlock {
    /// 主件不存在。
    OwnerMissing,
    /// 主件不是呈批件。
    OwnerKind,
    /// 主件已归档，清单只读。
    OwnerArchived,
    /// 主件本身被当作了别处的随行件，不能再挂随行件。
    OwnerIsItem,
    /// 候选稿件不存在。
    CandidateMissing,
    /// 不能挂自己。
    SelfReference,
    /// 已在清单里。
    AlreadyAdded,
    /// 候选稿件自己挂着随行件，不能再被当作随行件。
    CandidateHasItems,
}

impl AddBlock {
    pub fn reason(self) -> &'static str {
        match self {
            Self::OwnerMissing => "主件不存在",
            Self::OwnerKind => "只有白头件、红头呈批件可以挂送批材料",
            Self::OwnerArchived => "主件已归档，送批材料不可再改",
            Self::OwnerIsItem => "本件已是其他呈批件的送批材料，不能再挂送批材料",
            Self::CandidateMissing => "稿件不存在",
            Self::SelfReference => "不能把主件自己加为送批材料",
            Self::AlreadyAdded => "已在送批材料中",
            Self::CandidateHasItems => "该稿件自己挂着送批材料，不能再作为送批材料",
        }
    }
}

/// 一篇稿件最新提交版的身份（离线同步版本图里带显示序号的最新一版）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedRevision {
    pub revision_uuid: String,
    pub payload_hash: String,
    pub visible_number: i64,
    pub name: String,
    pub comment: String,
    pub created_at: String,
}

/// 一篇稿件在送批材料里的版本状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageItemState {
    /// 最新提交版；`None` 表示从未提交，不能进包。
    pub latest: Option<CommittedRevision>,
    /// 活稿行与最新提交版内容不同（从未提交时恒为真）。
    pub has_uncommitted: bool,
    /// 有待处理的同步分支：包里用的是本机这一支。
    pub has_pending_branch: bool,
}

/// 引用某篇稿件作为随行件的主件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPackageReferrer {
    pub owner_id: i64,
    pub title: String,
    pub status: ManuscriptStatus,
    pub pinned_revision_uuid: Option<String>,
}

/// 面板每行要显示的稿件概要：取自反规范化列，不读快照与附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManuscriptBrief {
    pub id: i64,
    pub title: String,
    pub kind: TemplateKind,
    pub status: ManuscriptStatus,
}

/// 导出计划里某一件用的版本：不可变快照，导出与记录都以它为准。
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedRevision {
    pub revision_uuid: String,
    pub payload_hash: String,
    /// 本机显示序号；钉住的版本若是同步检查点则没有序号。
    pub visible_number: Option<i64>,
    pub snapshot: DraftInput,
    pub content_markdown: String,
}

/// 某一件不能进包的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanBlocker {
    /// 从未提交过版本。
    NeverCommitted,
    /// 本机找不到这篇稿件。
    Missing,
    /// 钉住的版本在本机版本图里找不到。
    PinnedRevisionMissing,
}

impl PlanBlocker {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NeverCommitted => "从未提交过版本，请先提交一个版本",
            Self::Missing => "本机未找到该稿件，请先从同步包导入或移出清单",
            Self::PinnedRevisionMissing => "归档时钉住的版本在本机找不到",
        }
    }
}

/// 导出计划里的一件（第 0 件是主件）。
#[derive(Debug, Clone, PartialEq)]
pub struct PlanEntry {
    pub manuscript_id: Option<i64>,
    pub document_uuid: String,
    pub title: String,
    pub kind: TemplateKind,
    /// 要进包的版本；`None` 时 `blocker` 说明原因。
    pub revision: Option<PlannedRevision>,
    pub blocker: Option<PlanBlocker>,
    /// 活稿行与所用版本内容不同：这些修改不会进包。钉版时不提示。
    pub has_uncommitted: bool,
    pub has_pending_branch: bool,
    /// 用的是主件归档时钉住的版本，而不是最新提交版。
    pub pinned: bool,
}

/// 一次合并导出的计划：点导出时生成、给用户确认，确认后整份交给后台线程。
/// 版本快照已取出，之后再有人提交新版本也不会混进这一次导出。
#[derive(Debug, Clone, PartialEq)]
pub struct SendPackagePlan {
    pub owner_id: i64,
    pub entries: Vec<PlanEntry>,
}

impl SendPackagePlan {
    pub fn owner_title(&self) -> &str {
        self.entries
            .first()
            .map(|entry| entry.title.as_str())
            .unwrap_or_default()
    }

    /// 没有拦截项、可以导出。
    pub fn ready(&self) -> bool {
        self.entries.iter().all(|entry| entry.revision.is_some())
    }
}

/// 导出记录里一件的身份与版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportItemRecord {
    /// 主件为 0，随行件从 1 起；目录页不记。
    pub sort_order: i64,
    pub document_uuid: String,
    pub revision_uuid: String,
    pub payload_hash: String,
    pub visible_number: Option<i64>,
    pub title: String,
    pub kind: TemplateKind,
    pub page_count: i64,
    pub blank_pages: i64,
}

/// 一次合并导出的记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRecord {
    pub id: i64,
    pub exported_at: String,
    pub output_path: String,
    pub with_toc: bool,
    pub total_pages: i64,
    pub items: Vec<ExportItemRecord>,
}

/// 稿件的全部身份：当前 UUID 加上合并身份后留下的别名。随行件按其中任一个引用都算。
fn identity_uuids(conn: &Connection, id: i64) -> Result<Vec<String>> {
    let Some(current) = conn
        .query_row(
            "SELECT document_uuid FROM manuscripts WHERE id=?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare("SELECT alias_uuid FROM sync_aliases WHERE manuscript_id=?1")?;
    let mut out = vec![current];
    for alias in stmt.query_map([id], |row| row.get::<_, String>(0))? {
        out.push(alias?);
    }
    out.retain(|uuid| !uuid.is_empty());
    Ok(out)
}

/// `?1, ?2, …` 占位符，给 `IN (...)` 用。
fn placeholders(count: usize, start: usize) -> String {
    (start..start + count)
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 引用这篇稿件的主件（按主件 id 升序）。
fn referrers(conn: &Connection, id: i64) -> Result<Vec<SendPackageReferrer>> {
    let uuids = identity_uuids(conn, id)?;
    if uuids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT m.id, m.title, m.status, i.pinned_revision_uuid
         FROM send_package_items i JOIN manuscripts m ON m.id = i.owner_id
         WHERE i.item_uuid IN ({}) ORDER BY m.id",
        placeholders(uuids.len(), 1)
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(uuids.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (owner_id, title, status, pinned_revision_uuid) = row?;
        out.push(SendPackageReferrer {
            owner_id,
            title,
            status: str_to_status(&status).unwrap_or(ManuscriptStatus::Draft),
            pinned_revision_uuid,
        });
    }
    Ok(out)
}

/// 删除稿件前调用（与删除同一事务）：被已归档主件引用的拒绝删除；否则把它从各
/// 主件清单里摘掉。作为主件的清单与导出记录随外键级联清除，不用这里处理。
pub(super) fn detach_before_delete(conn: &Connection, id: i64) -> Result<()> {
    let refs = referrers(conn, id)?;
    if let Some(archived) = refs
        .iter()
        .find(|owner| owner.status == ManuscriptStatus::Archived)
    {
        bail!(
            "该稿件是已归档呈批件「{}」的送批材料，不能删除",
            archived.title
        );
    }
    let uuids = identity_uuids(conn, id)?;
    if !uuids.is_empty() {
        let sql = format!(
            "DELETE FROM send_package_items WHERE item_uuid IN ({})",
            placeholders(uuids.len(), 1)
        );
        conn.execute(&sql, params_from_iter(uuids.iter()))?;
    }
    Ok(())
}

impl ManuscriptStore {
    /// 主件的随行件清单，按排列顺序。
    pub fn send_package_items(&self, owner_id: i64) -> Result<Vec<SendPackageItem>> {
        let rows = {
            let mut stmt = self.conn.prepare(
                "SELECT item_uuid, pinned_revision_uuid, added_at FROM send_package_items
                 WHERE owner_id=?1 ORDER BY sort_order, added_at",
            )?;
            stmt.query_map([owner_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::with_capacity(rows.len());
        for (item_uuid, pinned_revision_uuid, added_at) in rows {
            out.push(SendPackageItem {
                manuscript_id: self.find_document_uuid(&item_uuid)?,
                item_uuid,
                pinned_revision_uuid,
                added_at,
            });
        }
        Ok(out)
    }

    /// 这篇稿件是否挂着随行件（不论当前文种）。
    pub fn has_send_package_items(&self, owner_id: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM send_package_items WHERE owner_id=?1)",
            [owner_id],
            |row| row.get(0),
        )?)
    }

    /// 引用这篇稿件作为随行件的主件：稿件详情的反向引用、删除前的确认都用它。
    pub fn send_package_referrers(&self, item_id: i64) -> Result<Vec<SendPackageReferrer>> {
        referrers(&self.conn, item_id)
    }

    /// 能否把 `candidate_id` 加为 `owner_id` 的随行件；能加时为 `None`。
    pub fn send_package_add_block(
        &self,
        owner_id: i64,
        candidate_id: i64,
    ) -> Result<Option<AddBlock>> {
        let owner = self
            .conn
            .query_row(
                "SELECT kind, status FROM manuscripts WHERE id=?1",
                [owner_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((kind, status)) = owner else {
            return Ok(Some(AddBlock::OwnerMissing));
        };
        if !str_to_kind(&kind).is_some_and(is_owner_kind) {
            return Ok(Some(AddBlock::OwnerKind));
        }
        if str_to_status(&status) == Some(ManuscriptStatus::Archived) {
            return Ok(Some(AddBlock::OwnerArchived));
        }
        if !referrers(&self.conn, owner_id)?.is_empty() {
            return Ok(Some(AddBlock::OwnerIsItem));
        }
        if candidate_id == owner_id {
            return Ok(Some(AddBlock::SelfReference));
        }
        let candidate_uuids = identity_uuids(&self.conn, candidate_id)?;
        if candidate_uuids.is_empty() {
            return Ok(Some(AddBlock::CandidateMissing));
        }
        let listed: HashSet<String> = self
            .send_package_items(owner_id)?
            .into_iter()
            .map(|item| item.item_uuid)
            .collect();
        if candidate_uuids.iter().any(|uuid| listed.contains(uuid)) {
            return Ok(Some(AddBlock::AlreadyAdded));
        }
        if self.has_send_package_items(candidate_id)? {
            return Ok(Some(AddBlock::CandidateHasItems));
        }
        Ok(None)
    }

    /// 把稿件加到主件清单末尾。不合引用规则时报错，原因同 [`AddBlock::reason`]。
    pub fn add_send_package_item(&mut self, owner_id: i64, candidate_id: i64) -> Result<()> {
        if let Some(block) = self.send_package_add_block(owner_id, candidate_id)? {
            bail!("{}", block.reason());
        }
        let (item_uuid, _) = self.document_identity(candidate_id)?;
        let sort: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM send_package_items WHERE owner_id=?1",
            [owner_id],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO send_package_items (owner_id, item_uuid, sort_order, added_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![owner_id, item_uuid, sort, Local::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// 从主件清单里移除一个随行件（本机找不到的也能移除）。
    pub fn remove_send_package_item(&mut self, owner_id: i64, item_uuid: &str) -> Result<()> {
        self.ensure_package_editable(owner_id)?;
        let removed = self.conn.execute(
            "DELETE FROM send_package_items WHERE owner_id=?1 AND item_uuid=?2",
            params![owner_id, item_uuid],
        )?;
        if removed == 0 {
            bail!("送批材料中没有这一件");
        }
        Ok(())
    }

    /// 按给定顺序重排清单。`order` 必须恰好是现有随行件 UUID 的一个排列。
    pub fn reorder_send_package(&mut self, owner_id: i64, order: &[String]) -> Result<()> {
        self.ensure_package_editable(owner_id)?;
        let current: HashSet<String> = self
            .send_package_items(owner_id)?
            .into_iter()
            .map(|item| item.item_uuid)
            .collect();
        let wanted: HashSet<String> = order.iter().cloned().collect();
        if wanted.len() != order.len() || wanted != current {
            bail!("送批材料顺序与现有清单不一致，请刷新后重试");
        }
        let tx = self.conn.transaction()?;
        for (sort, uuid) in order.iter().enumerate() {
            tx.execute(
                "UPDATE send_package_items SET sort_order=?1 WHERE owner_id=?2 AND item_uuid=?3",
                params![sort as i64, owner_id, uuid],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 稿件概要（标题、文种、状态）；不存在时为 `None`。
    pub fn manuscript_brief(&self, id: i64) -> Result<Option<ManuscriptBrief>> {
        let row = self
            .conn
            .query_row(
                "SELECT title, kind, status FROM manuscripts WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(title, kind, status)| ManuscriptBrief {
            id,
            title,
            kind: str_to_kind(&kind).unwrap_or(TemplateKind::OfficialLetter),
            status: str_to_status(&status).unwrap_or(ManuscriptStatus::Draft),
        }))
    }

    /// 最新提交版：版本图里带显示序号的最新一版，与版本面板的「最新版」一致。
    pub fn latest_committed_revision(&self, id: i64) -> Result<Option<CommittedRevision>> {
        Ok(self
            .conn
            .query_row(
                "SELECT revision_uuid, payload_hash, visible_number, name, comment, created_at
                 FROM sync_revisions WHERE manuscript_id=?1 AND visible_number IS NOT NULL
                 ORDER BY visible_number DESC LIMIT 1",
                [id],
                |row| {
                    Ok(CommittedRevision {
                        revision_uuid: row.get(0)?,
                        payload_hash: row.get(1)?,
                        visible_number: row.get(2)?,
                        name: row.get(3)?,
                        comment: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    /// 一篇稿件在送批材料里的版本状态：最新提交版、有无未提交修改、有无待处理分支。
    pub fn send_package_item_state(&self, id: i64) -> Result<PackageItemState> {
        let (snapshot, markdown) = self.snapshot_of(id)?.context("稿件不存在")?;
        let notes = self.notes_of(id)?.unwrap_or_default();
        let live_hash = super::sync::payload_hash(&snapshot, &markdown, &notes)?;
        let latest = self.latest_committed_revision(id)?;
        let has_uncommitted = latest
            .as_ref()
            .is_none_or(|revision| revision.payload_hash != live_hash);
        Ok(PackageItemState {
            latest,
            has_uncommitted,
            has_pending_branch: !self.pending_sync_heads(id)?.is_empty(),
        })
    }

    /// 生成导出计划：主件在前，随行件按清单顺序；每件取钉住的版本或最新提交版，
    /// 取不到的记下拦截原因。只读库，不改任何东西。
    pub fn send_package_plan(&self, owner_id: i64) -> Result<SendPackagePlan> {
        let (owner_uuid, _) = self.document_identity(owner_id)?;
        let mut targets = vec![(Some(owner_id), owner_uuid, None)];
        for item in self.send_package_items(owner_id)? {
            targets.push((
                item.manuscript_id,
                item.item_uuid,
                item.pinned_revision_uuid,
            ));
        }
        let mut entries = Vec::with_capacity(targets.len());
        for (manuscript_id, document_uuid, pinned) in targets {
            let brief = match manuscript_id {
                Some(id) => self.manuscript_brief(id)?,
                None => None,
            };
            let Some(brief) = brief else {
                entries.push(PlanEntry {
                    manuscript_id: None,
                    document_uuid,
                    title: String::new(),
                    kind: TemplateKind::OfficialLetter,
                    revision: None,
                    blocker: Some(PlanBlocker::Missing),
                    has_uncommitted: false,
                    has_pending_branch: false,
                    pinned: false,
                });
                continue;
            };
            let state = self.send_package_item_state(brief.id)?;
            let (revision, blocker) = match &pinned {
                Some(uuid) => match self.planned_revision(brief.id, uuid)? {
                    Some(revision) => (Some(revision), None),
                    None => (None, Some(PlanBlocker::PinnedRevisionMissing)),
                },
                None => match &state.latest {
                    Some(latest) => (
                        self.planned_revision(brief.id, &latest.revision_uuid)?,
                        None,
                    ),
                    None => (None, Some(PlanBlocker::NeverCommitted)),
                },
            };
            entries.push(PlanEntry {
                manuscript_id: Some(brief.id),
                document_uuid,
                title: brief.title,
                kind: brief.kind,
                revision,
                blocker,
                has_uncommitted: pinned.is_none() && state.has_uncommitted,
                has_pending_branch: state.has_pending_branch,
                pinned: pinned.is_some(),
            });
        }
        Ok(SendPackagePlan { owner_id, entries })
    }

    /// 从版本图里取出某一版的完整快照。
    fn planned_revision(&self, id: i64, revision_uuid: &str) -> Result<Option<PlannedRevision>> {
        let row = self
            .conn
            .query_row(
                "SELECT payload_hash, visible_number, snapshot_json, content_markdown
                 FROM sync_revisions WHERE manuscript_id=?1 AND revision_uuid=?2",
                params![id, revision_uuid],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((payload_hash, visible_number, json, content_markdown)) = row else {
            return Ok(None);
        };
        Ok(Some(PlannedRevision {
            revision_uuid: revision_uuid.to_string(),
            payload_hash,
            visible_number,
            snapshot: serde_json::from_str(&json).context("版本快照数据损坏")?,
            content_markdown,
        }))
    }

    /// 写一条导出记录（连同每一件），返回记录 id。
    pub fn record_send_package_export(
        &mut self,
        owner_id: i64,
        output_path: &str,
        with_toc: bool,
        total_pages: i64,
        items: &[ExportItemRecord],
    ) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO send_package_exports (owner_id, exported_at, output_path, with_toc, total_pages)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                owner_id,
                Local::now().to_rfc3339(),
                output_path,
                with_toc,
                total_pages
            ],
        )?;
        let export_id = tx.last_insert_rowid();
        for item in items {
            tx.execute(
                "INSERT INTO send_package_export_items
                 (export_id, sort_order, document_uuid, revision_uuid, payload_hash,
                  visible_number, title, kind, page_count, blank_pages)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    export_id,
                    item.sort_order,
                    item.document_uuid,
                    item.revision_uuid,
                    item.payload_hash,
                    item.visible_number,
                    item.title,
                    kind_to_str(item.kind),
                    item.page_count,
                    item.blank_pages
                ],
            )?;
        }
        tx.commit()?;
        Ok(export_id)
    }

    /// 主件的历次导出记录，最新的在前。
    pub fn send_package_exports(&self, owner_id: i64) -> Result<Vec<ExportRecord>> {
        let heads = {
            let mut stmt = self.conn.prepare(
                "SELECT id, exported_at, output_path, with_toc, total_pages
                 FROM send_package_exports WHERE owner_id=?1 ORDER BY id DESC",
            )?;
            stmt.query_map([owner_id], |row| {
                Ok(ExportRecord {
                    id: row.get(0)?,
                    exported_at: row.get(1)?,
                    output_path: row.get(2)?,
                    with_toc: row.get(3)?,
                    total_pages: row.get(4)?,
                    items: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut stmt = self.conn.prepare(
            "SELECT sort_order, document_uuid, revision_uuid, payload_hash, visible_number,
                    title, kind, page_count, blank_pages
             FROM send_package_export_items WHERE export_id=?1 ORDER BY sort_order",
        )?;
        let mut out = Vec::with_capacity(heads.len());
        for mut record in heads {
            record.items = stmt
                .query_map([record.id], |row| {
                    Ok(ExportItemRecord {
                        sort_order: row.get(0)?,
                        document_uuid: row.get(1)?,
                        revision_uuid: row.get(2)?,
                        payload_hash: row.get(3)?,
                        visible_number: row.get(4)?,
                        title: row.get(5)?,
                        kind: str_to_kind(&row.get::<_, String>(6)?)
                            .unwrap_or(TemplateKind::OfficialLetter),
                        page_count: row.get(7)?,
                        blank_pages: row.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.push(record);
        }
        Ok(out)
    }

    /// 清单可改：主件存在且未归档。
    fn ensure_package_editable(&self, owner_id: i64) -> Result<()> {
        match self.status_of(owner_id)? {
            None => bail!("{}", AddBlock::OwnerMissing.reason()),
            Some(ManuscriptStatus::Archived) => bail!("{}", AddBlock::OwnerArchived.reason()),
            Some(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::{ManuscriptUpdate, NewManuscript};
    use crate::models::{DraftInput, TemplateProfile};
    use std::path::Path;

    fn store() -> ManuscriptStore {
        ManuscriptStore::open(Path::new(":memory:")).expect("内存库应能打开")
    }

    fn snapshot(kind: TemplateKind, title: &str) -> DraftInput {
        DraftInput {
            kind,
            title_hint: title.into(),
            date: "2026年9月30日".into(),
            profile: TemplateProfile::for_kind(kind),
            ..Default::default()
        }
    }

    fn create(store: &mut ManuscriptStore, kind: TemplateKind, title: &str) -> i64 {
        store
            .create(
                &NewManuscript {
                    snapshot: snapshot(kind, title),
                    content_markdown: format!("# {title}\n\n正文。"),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap()
    }

    fn commit(store: &mut ManuscriptStore, id: i64, name: &str) {
        let (snapshot, markdown) = store.snapshot_of(id).unwrap().unwrap();
        store
            .commit_manuscript_version(id, name, "", &snapshot, &markdown, "")
            .unwrap();
    }

    fn edit(store: &mut ManuscriptStore, id: i64, markdown: &str) {
        let (snapshot, _) = store.snapshot_of(id).unwrap().unwrap();
        store
            .update(
                id,
                &ManuscriptUpdate {
                    snapshot,
                    content_markdown: markdown.into(),
                    notes: String::new(),
                },
            )
            .unwrap();
    }

    fn uuid_of(store: &ManuscriptStore, id: i64) -> String {
        store.document_identity(id).unwrap().0
    }

    fn item_ids(store: &ManuscriptStore, owner: i64) -> Vec<Option<i64>> {
        store
            .send_package_items(owner)
            .unwrap()
            .into_iter()
            .map(|item| item.manuscript_id)
            .collect()
    }

    #[test]
    fn items_are_added_in_order_reordered_and_removed() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::WhitePaper, "呈批件");
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        let report = create(&mut store, TemplateKind::ResearchReport, "研究报告");
        store.add_send_package_item(owner, letter).unwrap();
        store.add_send_package_item(owner, report).unwrap();
        assert_eq!(item_ids(&store, owner), vec![Some(letter), Some(report)]);
        assert!(store.has_send_package_items(owner).unwrap());

        let reversed = vec![uuid_of(&store, report), uuid_of(&store, letter)];
        store.reorder_send_package(owner, &reversed).unwrap();
        assert_eq!(item_ids(&store, owner), vec![Some(report), Some(letter)]);

        store
            .remove_send_package_item(owner, &uuid_of(&store, report))
            .unwrap();
        assert_eq!(item_ids(&store, owner), vec![Some(letter)]);
        assert!(
            store
                .remove_send_package_item(owner, &uuid_of(&store, report))
                .is_err()
        );
    }

    #[test]
    fn reorder_rejects_lists_that_are_not_a_permutation() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::RedHeadApproval, "呈批件");
        let a = create(&mut store, TemplateKind::OfficialLetter, "甲");
        let b = create(&mut store, TemplateKind::PlainDocument, "乙");
        store.add_send_package_item(owner, a).unwrap();
        store.add_send_package_item(owner, b).unwrap();
        let (ua, ub) = (uuid_of(&store, a), uuid_of(&store, b));
        assert!(
            store
                .reorder_send_package(owner, std::slice::from_ref(&ua))
                .is_err()
        );
        assert!(
            store
                .reorder_send_package(owner, &[ua.clone(), ua.clone()])
                .is_err()
        );
        assert!(
            store
                .reorder_send_package(owner, &[ua.clone(), ub.clone(), "x".into()])
                .is_err()
        );
        assert_eq!(item_ids(&store, owner), vec![Some(a), Some(b)]);
    }

    #[test]
    fn reference_rules_are_enforced() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::WhitePaper, "呈批件");
        let other_owner = create(&mut store, TemplateKind::RedHeadApproval, "另一件呈批件");
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        let notice = create(&mut store, TemplateKind::PhoneNotice, "电话通知");

        // 只有呈批件能当主件。
        assert_eq!(
            store.send_package_add_block(letter, notice).unwrap(),
            Some(AddBlock::OwnerKind)
        );
        assert_eq!(
            store.send_package_add_block(owner, owner).unwrap(),
            Some(AddBlock::SelfReference)
        );
        assert_eq!(
            store.send_package_add_block(owner, 9999).unwrap(),
            Some(AddBlock::CandidateMissing)
        );
        assert_eq!(
            store.send_package_add_block(9999, letter).unwrap(),
            Some(AddBlock::OwnerMissing)
        );

        store.add_send_package_item(owner, letter).unwrap();
        assert_eq!(
            store.send_package_add_block(owner, letter).unwrap(),
            Some(AddBlock::AlreadyAdded)
        );
        // 挂着随行件的呈批件不能被当作随行件。
        assert_eq!(
            store.send_package_add_block(other_owner, owner).unwrap(),
            Some(AddBlock::CandidateHasItems)
        );
        // 呈批件可以当别人的随行件，但之后自己就不能再挂。
        let third_owner = create(&mut store, TemplateKind::WhitePaper, "第三件呈批件");
        store
            .add_send_package_item(other_owner, third_owner)
            .unwrap();
        assert_eq!(
            store.send_package_add_block(third_owner, notice).unwrap(),
            Some(AddBlock::OwnerIsItem)
        );
        let error = store
            .add_send_package_item(third_owner, notice)
            .unwrap_err()
            .to_string();
        assert!(error.contains("不能再挂送批材料"), "{error}");
    }

    #[test]
    fn one_manuscript_can_back_several_owners() {
        let mut store = store();
        let a = create(&mut store, TemplateKind::WhitePaper, "甲呈批件");
        let b = create(&mut store, TemplateKind::RedHeadApproval, "乙呈批件");
        let report = create(&mut store, TemplateKind::ResearchReport, "研究报告");
        store.add_send_package_item(a, report).unwrap();
        store.add_send_package_item(b, report).unwrap();
        let owners: Vec<i64> = store
            .send_package_referrers(report)
            .unwrap()
            .into_iter()
            .map(|owner| owner.owner_id)
            .collect();
        assert_eq!(owners, vec![a, b]);
        assert!(store.send_package_referrers(a).unwrap().is_empty());
    }

    #[test]
    fn archived_owner_list_is_read_only() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::WhitePaper, "呈批件");
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        let report = create(&mut store, TemplateKind::ResearchReport, "研究报告");
        store.add_send_package_item(owner, letter).unwrap();
        store.set_status(owner, ManuscriptStatus::Archived).unwrap();

        assert_eq!(
            store.send_package_add_block(owner, report).unwrap(),
            Some(AddBlock::OwnerArchived)
        );
        let letter_uuid = uuid_of(&store, letter);
        assert!(store.remove_send_package_item(owner, &letter_uuid).is_err());
        assert!(store.reorder_send_package(owner, &[letter_uuid]).is_err());
        assert_eq!(item_ids(&store, owner), vec![Some(letter)]);
    }

    #[test]
    fn deleting_an_item_detaches_it_unless_an_archived_owner_uses_it() {
        let mut store = store();
        let draft_owner = create(&mut store, TemplateKind::WhitePaper, "在办呈批件");
        let archived_owner = create(&mut store, TemplateKind::RedHeadApproval, "已归档呈批件");
        let loose = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        let kept = create(&mut store, TemplateKind::ResearchReport, "研究报告");
        store.add_send_package_item(draft_owner, loose).unwrap();
        store.add_send_package_item(draft_owner, kept).unwrap();
        store.add_send_package_item(archived_owner, kept).unwrap();
        store
            .set_status(archived_owner, ManuscriptStatus::Archived)
            .unwrap();

        store.delete(loose).unwrap();
        assert_eq!(item_ids(&store, draft_owner), vec![Some(kept)]);

        let error = store.delete(kept).unwrap_err().to_string();
        assert!(error.contains("已归档呈批件「已归档呈批件」"), "{error}");
        // 批量删除同样整批拒绝，不留下半截结果。
        let other = create(&mut store, TemplateKind::PlainDocument, "普通公文");
        assert!(store.delete_many(&[other, kept]).is_err());
        assert!(store.get(other).unwrap().is_some());
        assert_eq!(item_ids(&store, draft_owner), vec![Some(kept)]);
    }

    #[test]
    fn deleting_an_owner_drops_its_list() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::WhitePaper, "呈批件");
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        store.add_send_package_item(owner, letter).unwrap();
        store.delete(owner).unwrap();
        assert!(store.send_package_items(owner).unwrap().is_empty());
        assert!(store.send_package_referrers(letter).unwrap().is_empty());
    }

    #[test]
    fn items_resolve_through_aliases_and_survive_when_missing() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::WhitePaper, "呈批件");
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        store.add_send_package_item(owner, letter).unwrap();

        // 离线合并身份后，稿件换了 UUID、旧 UUID 成为别名：关联照样解析得到。
        let old = uuid_of(&store, letter);
        let new = uuid::Uuid::new_v4().to_string();
        store.assign_document_uuid(letter, &new).unwrap();
        store.link_document_alias(letter, &old).unwrap();
        assert_eq!(item_ids(&store, owner), vec![Some(letter)]);
        assert_eq!(store.send_package_referrers(letter).unwrap().len(), 1);
        assert_eq!(
            store.send_package_add_block(owner, letter).unwrap(),
            Some(AddBlock::AlreadyAdded)
        );

        // 本机找不到的随行件保留在清单里，也能手动移除。
        let missing = uuid::Uuid::new_v4().to_string();
        store
            .conn
            .execute(
                "INSERT INTO send_package_items (owner_id, item_uuid, sort_order, added_at)
                 VALUES (?1, ?2, 5, '2026-09-30T00:00:00+08:00')",
                params![owner, missing],
            )
            .unwrap();
        assert_eq!(item_ids(&store, owner), vec![Some(letter), None]);
        store.remove_send_package_item(owner, &missing).unwrap();
        assert_eq!(item_ids(&store, owner), vec![Some(letter)]);
    }

    #[test]
    fn item_state_tracks_latest_commit_and_uncommitted_edits() {
        let mut store = store();
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        let state = store.send_package_item_state(letter).unwrap();
        assert_eq!(state.latest, None);
        assert!(state.has_uncommitted);
        assert!(!state.has_pending_branch);

        commit(&mut store, letter, "初稿");
        let state = store.send_package_item_state(letter).unwrap();
        let v1 = state.latest.clone().unwrap();
        assert_eq!((v1.visible_number, v1.name.as_str()), (1, "初稿"));
        assert!(!state.has_uncommitted);

        edit(&mut store, letter, "# 函稿\n\n改过的正文。");
        let state = store.send_package_item_state(letter).unwrap();
        assert_eq!(state.latest, Some(v1.clone()));
        assert!(state.has_uncommitted);

        commit(&mut store, letter, "修改稿");
        let state = store.send_package_item_state(letter).unwrap();
        let v2 = state.latest.unwrap();
        assert_eq!(v2.visible_number, 2);
        assert_ne!(v2.revision_uuid, v1.revision_uuid);
        assert!(!state.has_uncommitted);
    }

    #[test]
    fn plan_uses_latest_commit_and_flags_what_cannot_go_in() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::WhitePaper, "呈批件");
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        let notice = create(&mut store, TemplateKind::PlainDocument, "通知");
        store.add_send_package_item(owner, letter).unwrap();
        store.add_send_package_item(owner, notice).unwrap();
        commit(&mut store, owner, "送审稿");
        commit(&mut store, letter, "初稿");
        edit(&mut store, letter, "# 函稿\n\n还没提交的修改。");

        let plan = store.send_package_plan(owner).unwrap();
        assert_eq!(plan.owner_title(), "呈批件");
        let titles: Vec<&str> = plan.entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["呈批件", "函稿", "通知"]);
        // 函稿进包的是 v1 的内容，不是活稿行里未提交的修改。
        let letter_entry = &plan.entries[1];
        let revision = letter_entry.revision.as_ref().unwrap();
        assert_eq!(revision.visible_number, Some(1));
        assert!(!revision.content_markdown.contains("还没提交"));
        assert!(letter_entry.has_uncommitted);
        assert!(!plan.entries[0].has_uncommitted);
        // 通知从未提交，拦下。
        assert_eq!(plan.entries[2].revision, None);
        assert_eq!(plan.entries[2].blocker, Some(PlanBlocker::NeverCommitted));
        assert!(!plan.ready());

        commit(&mut store, notice, "初稿");
        assert!(store.send_package_plan(owner).unwrap().ready());
    }

    #[test]
    fn plan_honours_pins_and_reports_missing_items() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::RedHeadApproval, "呈批件");
        let letter = create(&mut store, TemplateKind::OfficialLetter, "函稿");
        store.add_send_package_item(owner, letter).unwrap();
        commit(&mut store, owner, "送审稿");
        commit(&mut store, letter, "初稿");
        let v1 = store.latest_committed_revision(letter).unwrap().unwrap();
        edit(&mut store, letter, "# 函稿\n\n第二稿。");
        commit(&mut store, letter, "修改稿");
        store
            .conn
            .execute(
                "UPDATE send_package_items SET pinned_revision_uuid=?1 WHERE owner_id=?2",
                params![v1.revision_uuid, owner],
            )
            .unwrap();
        let missing = uuid::Uuid::new_v4().to_string();
        store
            .conn
            .execute(
                "INSERT INTO send_package_items (owner_id, item_uuid, sort_order, added_at)
                 VALUES (?1, ?2, 9, '2026-09-30T00:00:00+08:00')",
                params![owner, missing],
            )
            .unwrap();

        let plan = store.send_package_plan(owner).unwrap();
        let pinned = &plan.entries[1];
        assert!(pinned.pinned);
        assert_eq!(
            pinned.revision.as_ref().unwrap().revision_uuid,
            v1.revision_uuid
        );
        assert!(!pinned.has_uncommitted, "钉版时不提示未提交修改");
        assert_eq!(plan.entries[2].blocker, Some(PlanBlocker::Missing));
        assert_eq!(plan.entries[2].document_uuid, missing);

        // 钉住的版本不存在时拦下。
        store
            .conn
            .execute(
                "UPDATE send_package_items SET pinned_revision_uuid='nope' WHERE item_uuid=?1",
                [uuid_of(&store, letter)],
            )
            .unwrap();
        let plan = store.send_package_plan(owner).unwrap();
        assert_eq!(
            plan.entries[1].blocker,
            Some(PlanBlocker::PinnedRevisionMissing)
        );
    }

    #[test]
    fn export_records_round_trip_newest_first() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::WhitePaper, "呈批件");
        let item = |order: i64, title: &str, pages: i64| ExportItemRecord {
            sort_order: order,
            document_uuid: format!("doc-{title}"),
            revision_uuid: format!("rev-{title}"),
            payload_hash: format!("hash-{title}"),
            visible_number: Some(order + 1),
            title: title.into(),
            kind: TemplateKind::OfficialLetter,
            page_count: pages,
            blank_pages: pages % 2,
        };
        let first = store
            .record_send_package_export(owner, "/tmp/a.pdf", false, 4, &[item(0, "甲", 3)])
            .unwrap();
        let items = vec![item(0, "甲", 3), item(1, "乙", 2)];
        let second = store
            .record_send_package_export(owner, "/tmp/b.pdf", true, 8, &items)
            .unwrap();
        let records = store.send_package_exports(owner).unwrap();
        assert_eq!(
            records.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![second, first]
        );
        assert!(records[0].with_toc);
        assert_eq!(records[0].total_pages, 8);
        assert_eq!(records[0].items, items);
        // 主件删除时记录一并清掉。
        store.delete(owner).unwrap();
        assert!(store.send_package_exports(owner).unwrap().is_empty());
    }

    #[test]
    fn brief_reads_title_kind_and_status() {
        let mut store = store();
        let owner = create(&mut store, TemplateKind::RedHeadApproval, "关于某事的请示");
        store
            .set_status(owner, ManuscriptStatus::Published)
            .unwrap();
        let brief = store.manuscript_brief(owner).unwrap().unwrap();
        assert_eq!(brief.id, owner);
        assert_eq!(brief.kind, TemplateKind::RedHeadApproval);
        assert_eq!(brief.status, ManuscriptStatus::Published);
        assert!(!brief.title.is_empty());
        assert_eq!(store.manuscript_brief(9999).unwrap(), None);
    }

    #[test]
    fn schema_is_idempotent() {
        let store = store();
        ensure_schema(&store.conn).unwrap();
        ensure_schema(&store.conn).unwrap();
    }
}
