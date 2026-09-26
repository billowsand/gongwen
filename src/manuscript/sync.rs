//! 离线稿件同步的身份与不可变版本图。
use super::{ManuscriptStore, column_exists};
use crate::models::DraftInput;
use anyhow::{Context, Result, bail};
use chrono::Local;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS sync_revisions (
    revision_uuid TEXT PRIMARY KEY,
    manuscript_id INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    parent1_uuid TEXT,
    parent2_uuid TEXT,
    payload_hash TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    content_markdown TEXT NOT NULL,
    notes TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    comment TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    visible_number INTEGER
);
CREATE INDEX IF NOT EXISTS idx_sync_revisions_manuscript ON sync_revisions(manuscript_id);
CREATE TABLE IF NOT EXISTS sync_aliases (
    alias_uuid TEXT PRIMARY KEY,
    manuscript_id INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS legacy_imports (
    fingerprint TEXT PRIMARY KEY,
    manuscript_id INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS pending_sync_heads (
    manuscript_id INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    revision_uuid TEXT NOT NULL REFERENCES sync_revisions(revision_uuid),
    imported_status TEXT NOT NULL,
    added_at TEXT NOT NULL,
    PRIMARY KEY (manuscript_id, revision_uuid)
);
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRevision {
    pub revision_uuid: String,
    pub parents: Vec<String>,
    pub payload_hash: String,
    pub snapshot: DraftInput,
    pub content_markdown: String,
    pub notes: String,
    pub name: String,
    pub comment: String,
    pub created_at: String,
    pub visible_number: Option<i64>,
}

impl SyncRevision {
    pub fn new(
        parents: Vec<String>,
        snapshot: DraftInput,
        content_markdown: String,
        notes: String,
        name: String,
        comment: String,
        visible_number: Option<i64>,
    ) -> Result<Self> {
        let payload_hash = payload_hash(&snapshot, &content_markdown, &notes)?;
        Ok(Self {
            revision_uuid: Uuid::new_v4().to_string(),
            parents,
            payload_hash,
            snapshot,
            content_markdown,
            notes,
            name,
            comment,
            created_at: Local::now().to_rfc3339(),
            visible_number,
        })
    }

    pub fn verify(&self) -> Result<()> {
        if Uuid::parse_str(&self.revision_uuid).is_err()
            || self.parents.len() > 2
            || self
                .parents
                .iter()
                .any(|parent| Uuid::parse_str(parent).is_err())
        {
            bail!("版本身份或父版本身份无效");
        }
        if self
            .parents
            .iter()
            .any(|parent| parent == &self.revision_uuid)
            || self.parents.len() == 2 && self.parents[0] == self.parents[1]
        {
            bail!("版本父子关系无效");
        }
        if payload_hash(&self.snapshot, &self.content_markdown, &self.notes)? != self.payload_hash {
            bail!("版本内容校验失败：{}", self.revision_uuid);
        }
        Ok(())
    }
}

pub fn payload_hash(snapshot: &DraftInput, markdown: &str, notes: &str) -> Result<String> {
    let json = serde_json::to_vec(snapshot).context("序列化稿件要素失败")?;
    let mut hash = Sha256::new();
    for part in [json.as_slice(), markdown.as_bytes(), notes.as_bytes()] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    Ok(hex(&hash.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn bytes_hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(super) fn migrate(conn: &mut Connection) -> Result<()> {
    if column_exists(conn, "manuscripts", "document_uuid")?
        && column_exists(conn, "manuscripts", "head_revision_uuid")?
        && conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? >= 8
    {
        return Ok(());
    }
    let tx = conn.transaction()?;
    if !column_exists(&tx, "manuscripts", "document_uuid")? {
        tx.execute_batch(
            "ALTER TABLE manuscripts ADD COLUMN document_uuid TEXT NOT NULL DEFAULT ''",
        )?;
    }
    if !column_exists(&tx, "manuscripts", "head_revision_uuid")? {
        tx.execute_batch("ALTER TABLE manuscripts ADD COLUMN head_revision_uuid TEXT")?;
    }
    tx.execute_batch(DDL)?;
    let manuscript_rows = {
        let mut stmt = tx.prepare(
            "SELECT id, document_uuid, snapshot_json, content_markdown, notes FROM manuscripts ORDER BY id"
        )?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, old_uuid, snapshot_json, markdown, notes) in manuscript_rows {
        let doc_uuid = if old_uuid.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            old_uuid
        };
        let prior_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sync_revisions WHERE manuscript_id=?1",
            [id],
            |row| row.get(0),
        )?;
        let mut head = None;
        if prior_count == 0 {
            let old_versions = {
                let mut stmt = tx.prepare(
                    "SELECT version_number, name, comment, snapshot_json, content_markdown, notes, created_at
                     FROM manuscript_versions WHERE manuscript_id=?1 ORDER BY version_number"
                )?;
                stmt.query_map([id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (number, name, comment, json, body, note, created_at) in old_versions {
                let snapshot: DraftInput = serde_json::from_str(&json)
                    .context("旧版稿件版本快照损坏，无法迁移同步历史")?;
                let mut revision = SyncRevision::new(
                    head.iter().cloned().collect(),
                    snapshot,
                    body,
                    note,
                    name,
                    comment,
                    Some(number),
                )?;
                revision.created_at = created_at;
                insert_revision(&tx, id, &revision)?;
                head = Some(revision.revision_uuid);
            }
            let live_snapshot: DraftInput =
                serde_json::from_str(&snapshot_json).context("稿件快照损坏，无法迁移同步历史")?;
            let live_hash = payload_hash(&live_snapshot, &markdown, &notes)?;
            let head_hash = if let Some(ref uuid) = head {
                tx.query_row(
                    "SELECT payload_hash FROM sync_revisions WHERE revision_uuid=?1",
                    [uuid],
                    |row| row.get::<_, String>(0),
                )?
            } else {
                String::new()
            };
            if live_hash != head_hash {
                let revision = SyncRevision::new(
                    head.iter().cloned().collect(),
                    live_snapshot,
                    markdown,
                    notes,
                    String::new(),
                    String::new(),
                    None,
                )?;
                insert_revision(&tx, id, &revision)?;
                head = Some(revision.revision_uuid);
            }
        } else {
            head = tx.query_row(
                "SELECT head_revision_uuid FROM manuscripts WHERE id=?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )?;
        }
        tx.execute(
            "UPDATE manuscripts SET document_uuid=?1, head_revision_uuid=?2 WHERE id=?3",
            params![doc_uuid, head, id],
        )?;
    }
    tx.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS idx_manuscripts_uuid ON manuscripts(document_uuid); PRAGMA user_version = 8")?;
    tx.commit()?;
    Ok(())
}

fn insert_revision(conn: &Connection, manuscript_id: i64, revision: &SyncRevision) -> Result<()> {
    revision.verify()?;
    let json = serde_json::to_string(&revision.snapshot)?;
    conn.execute(
        "INSERT INTO sync_revisions
         (revision_uuid, manuscript_id, parent1_uuid, parent2_uuid, payload_hash,
          snapshot_json, content_markdown, notes, name, comment, created_at, visible_number)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            revision.revision_uuid,
            manuscript_id,
            revision.parents.first(),
            revision.parents.get(1),
            revision.payload_hash,
            json,
            revision.content_markdown,
            revision.notes,
            revision.name,
            revision.comment,
            revision.created_at,
            revision.visible_number
        ],
    )?;
    Ok(())
}

/// 一个可见提交版本的载荷：本机显示序号、名称、注释与内容快照。
pub struct VisibleRevision<'a> {
    pub number: i64,
    pub name: &'a str,
    pub comment: &'a str,
    pub snapshot: &'a DraftInput,
    pub markdown: &'a str,
    pub notes: &'a str,
}

impl ManuscriptStore {
    pub fn document_identity(&self, id: i64) -> Result<(String, Option<String>)> {
        self.conn
            .query_row(
                "SELECT document_uuid, head_revision_uuid FROM manuscripts WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("稿件不存在")
    }

    pub fn find_document_uuid(&self, uuid: &str) -> Result<Option<i64>> {
        let direct = self
            .conn
            .query_row(
                "SELECT id FROM manuscripts WHERE document_uuid=?1",
                [uuid],
                |row| row.get(0),
            )
            .optional()?;
        if direct.is_some() {
            return Ok(direct);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT manuscript_id FROM sync_aliases WHERE alias_uuid=?1",
                [uuid],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn link_document_alias(&self, id: i64, alias: &str) -> Result<()> {
        if Uuid::parse_str(alias).is_err() {
            bail!("稿件身份无效");
        }
        if let Some(other) = self.find_document_uuid(alias)? {
            if other != id {
                bail!("该稿件身份已属于另一篇稿件");
            }
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO sync_aliases(alias_uuid, manuscript_id) VALUES (?1, ?2)",
            params![alias, id],
        )?;
        Ok(())
    }

    pub fn document_aliases(&self, id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT alias_uuid FROM sync_aliases WHERE manuscript_id=?1 ORDER BY alias_uuid",
        )?;
        Ok(stmt
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn find_legacy_import(&self, fingerprint: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT manuscript_id FROM legacy_imports WHERE fingerprint=?1",
                [fingerprint],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn record_legacy_import(&self, fingerprint: &str, id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO legacy_imports(fingerprint, manuscript_id) VALUES (?1, ?2)",
            params![fingerprint, id],
        )?;
        Ok(())
    }

    pub fn assign_document_uuid(&self, id: i64, uuid: &str) -> Result<()> {
        if Uuid::parse_str(uuid).is_err() {
            bail!("稿件身份无效");
        }
        if let Some(other) = self.find_document_uuid(uuid)?
            && other != id
        {
            bail!("稿件身份与本机其他记录冲突");
        }
        self.conn.execute(
            "UPDATE manuscripts SET document_uuid=?1 WHERE id=?2",
            params![uuid, id],
        )?;
        Ok(())
    }

    pub fn sync_revisions(&self, id: i64) -> Result<Vec<SyncRevision>> {
        let mut stmt = self.conn.prepare(
            "SELECT revision_uuid, parent1_uuid, parent2_uuid, payload_hash,
                    snapshot_json, content_markdown, notes, name, comment, created_at, visible_number
             FROM sync_revisions WHERE manuscript_id=?1 ORDER BY rowid"
        )?;
        let rows = stmt.query_map([id], |row| {
            let p1: Option<String> = row.get(1)?;
            let p2: Option<String> = row.get(2)?;
            Ok((
                row.get::<_, String>(0)?,
                p1.into_iter().chain(p2).collect::<Vec<_>>(),
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<i64>>(10)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                revision_uuid,
                parents,
                payload_hash,
                json,
                content_markdown,
                notes,
                name,
                comment,
                created_at,
                visible_number,
            ) = row?;
            out.push(SyncRevision {
                revision_uuid,
                parents,
                payload_hash,
                snapshot: serde_json::from_str(&json).context("同步版本快照损坏")?,
                content_markdown,
                notes,
                name,
                comment,
                created_at,
                visible_number,
            });
        }
        Ok(out)
    }

    pub fn sync_checkpoint(&mut self, id: i64) -> Result<SyncRevision> {
        let record = self.get(id)?.context("稿件不存在")?;
        let (_, head) = self.document_identity(id)?;
        let hash = payload_hash(&record.snapshot, &record.content_markdown, &record.notes)?;
        if let Some(ref uuid) = head
            && let Some(revision) = self
                .sync_revisions(id)?
                .into_iter()
                .find(|revision| &revision.revision_uuid == uuid)
            && revision.payload_hash == hash
        {
            return Ok(revision);
        }
        let revision = SyncRevision::new(
            head.into_iter().collect(),
            record.snapshot,
            record.content_markdown,
            record.notes,
            String::new(),
            String::new(),
            None,
        )?;
        insert_revision(&self.conn, id, &revision)?;
        self.set_sync_head(id, &revision.revision_uuid)?;
        Ok(revision)
    }

    pub fn set_sync_head(&self, id: i64, uuid: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE manuscripts SET head_revision_uuid=?1 WHERE id=?2",
            params![uuid, id],
        )?;
        Ok(())
    }

    pub fn append_visible_sync_revision(
        &self,
        id: i64,
        version: &VisibleRevision<'_>,
    ) -> Result<()> {
        let (_, head) = self.document_identity(id)?;
        let revision = SyncRevision::new(
            head.into_iter().collect(),
            version.snapshot.clone(),
            version.markdown.to_string(),
            version.notes.to_string(),
            version.name.to_string(),
            version.comment.to_string(),
            Some(version.number),
        )?;
        insert_revision(&self.conn, id, &revision)?;
        self.set_sync_head(id, &revision.revision_uuid)
    }

    pub fn insert_sync_revisions(&self, id: i64, revisions: &[SyncRevision]) -> Result<()> {
        let mut known: HashSet<String> = self
            .sync_revisions(id)?
            .into_iter()
            .map(|revision| revision.revision_uuid)
            .collect();
        for revision in revisions {
            revision.verify()?;
            if let Some((owner, hash, parent1, parent2)) = self
                .conn
                .query_row(
                    "SELECT manuscript_id, payload_hash, parent1_uuid, parent2_uuid
                     FROM sync_revisions WHERE revision_uuid=?1",
                    [&revision.revision_uuid],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?
            {
                let parents = parent1.into_iter().chain(parent2).collect::<Vec<_>>();
                if owner != id || hash != revision.payload_hash || parents != revision.parents {
                    bail!(
                        "相同版本身份对应不同稿件、内容或父版本：{}",
                        revision.revision_uuid
                    );
                }
                known.insert(revision.revision_uuid.clone());
                continue;
            }
            if revision
                .parents
                .iter()
                .any(|parent| !known.contains(parent))
            {
                bail!("同步包版本祖先缺失或顺序错误：{}", revision.revision_uuid);
            }
            let mut imported = revision.clone();
            // 来源机器的显示编号不能作为本机编号。
            imported.visible_number = None;
            insert_revision(&self.conn, id, &imported)?;
            known.insert(revision.revision_uuid.clone());
        }
        Ok(())
    }

    pub fn join_equivalent_sync_heads(
        &self,
        id: i64,
        local: String,
        incoming: String,
        snapshot: &DraftInput,
        markdown: &str,
        notes: &str,
    ) -> Result<()> {
        if local == incoming {
            return self.set_sync_head(id, &local);
        }
        let revisions = self.sync_revisions(id)?;
        let graph = revisions
            .iter()
            .map(|revision| (revision.revision_uuid.as_str(), revision.parents.as_slice()))
            .collect::<HashMap<_, _>>();
        let reaches = |head: &str, target: &str| {
            let mut seen = HashSet::new();
            let mut stack = vec![head];
            while let Some(uuid) = stack.pop() {
                if uuid == target {
                    return true;
                }
                if seen.insert(uuid)
                    && let Some(parents) = graph.get(uuid)
                {
                    stack.extend(parents.iter().map(String::as_str));
                }
            }
            false
        };
        if reaches(&local, &incoming) {
            return self.set_sync_head(id, &local);
        }
        if reaches(&incoming, &local) {
            return self.set_sync_head(id, &incoming);
        }
        let join = SyncRevision::new(
            vec![local, incoming],
            snapshot.clone(),
            markdown.to_string(),
            notes.to_string(),
            String::new(),
            String::new(),
            None,
        )?;
        insert_revision(&self.conn, id, &join)?;
        self.set_sync_head(id, &join.revision_uuid)
    }

    pub fn materialize_imported_versions(&self, id: i64, revisions: &[SyncRevision]) -> Result<()> {
        for revision in revisions
            .iter()
            .filter(|revision| revision.visible_number.is_some())
        {
            let visible: Option<i64> = self.conn.query_row(
                "SELECT visible_number FROM sync_revisions WHERE revision_uuid=?1 AND manuscript_id=?2",
                params![revision.revision_uuid, id], |row| row.get(0)
            ).optional()?.flatten();
            if visible.is_some() {
                continue;
            }
            let number: i64 = self.conn.query_row(
                "SELECT COALESCE(MAX(version_number), 0) + 1 FROM manuscript_versions WHERE manuscript_id=?1",
                [id], |row| row.get(0)
            )?;
            let json = serde_json::to_string(&revision.snapshot)?;
            let title = crate::export::extract_title(
                &revision.content_markdown,
                &revision.snapshot.title_hint,
            );
            self.conn.execute(
                "INSERT INTO manuscript_versions
                 (manuscript_id, version_number, name, comment, snapshot_json,
                  content_markdown, notes, title, doc_number, doc_date, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id,
                    number,
                    revision.name,
                    revision.comment,
                    json,
                    revision.content_markdown,
                    revision.notes,
                    title,
                    revision.snapshot.profile.document_number,
                    revision.snapshot.date,
                    revision.created_at
                ],
            )?;
            self.conn.execute(
                "UPDATE sync_revisions SET visible_number=?1 WHERE revision_uuid=?2",
                params![number, revision.revision_uuid],
            )?;
        }
        Ok(())
    }

    pub fn commit_merged_sync_revision(
        &mut self,
        id: i64,
        parents: [String; 2],
        snapshot: &DraftInput,
        markdown: &str,
        notes: &str,
    ) -> Result<()> {
        let number = self.latest_manuscript_number(id)?.map_or(1, |n| n + 1);
        let revision = SyncRevision::new(
            parents.to_vec(),
            snapshot.clone(),
            markdown.to_string(),
            notes.to_string(),
            "合并版本".to_string(),
            "离线导入后合并".to_string(),
            Some(number),
        )?;
        let json = serde_json::to_string(snapshot)?;
        let title = crate::export::extract_title(markdown, &snapshot.title_hint);
        self.conn.execute(
            "INSERT INTO manuscript_versions
             (manuscript_id, version_number, name, comment, snapshot_json,
              content_markdown, notes, title, doc_number, doc_date, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                number,
                revision.name,
                revision.comment,
                json,
                markdown,
                notes,
                title,
                snapshot.profile.document_number,
                snapshot.date,
                revision.created_at
            ],
        )?;
        insert_revision(&self.conn, id, &revision)?;
        self.set_sync_head(id, &revision.revision_uuid)?;
        Ok(())
    }

    pub fn begin_sync_import(&self) -> Result<()> {
        self.conn
            .execute_batch("SAVEPOINT manuscript_sync_import")?;
        Ok(())
    }

    pub fn commit_sync_import(&self) -> Result<()> {
        self.conn.execute_batch("RELEASE manuscript_sync_import")?;
        Ok(())
    }

    pub fn rollback_sync_import(&self) -> Result<()> {
        self.conn
            .execute_batch("ROLLBACK TO manuscript_sync_import; RELEASE manuscript_sync_import")?;
        Ok(())
    }

    pub fn pending_sync_heads(&self, id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT revision_uuid FROM pending_sync_heads WHERE manuscript_id=?1 ORDER BY added_at",
        )?;
        Ok(stmt
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn pending_sync_status(&self, id: i64, head: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT imported_status FROM pending_sync_heads
             WHERE manuscript_id=?1 AND revision_uuid=?2",
                params![id, head],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn remove_pending_sync_head(&self, id: i64, head: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM pending_sync_heads WHERE manuscript_id=?1 AND revision_uuid=?2",
            params![id, head],
        )?;
        Ok(())
    }

    pub fn save_pending_sync_head(&self, id: i64, head: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO pending_sync_heads
             (manuscript_id, revision_uuid, imported_status, added_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, head, status, Local::now().to_rfc3339()],
        )?;
        Ok(())
    }
}
