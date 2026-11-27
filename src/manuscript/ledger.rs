//! 发文字号台账：公函、红头呈批件、会议纪要三个池。
//!
//! 稿件库里的稿件自动维护一行（`manuscript_id` 非空）；纸质件、旧年文件可以
//! 只登记不挂稿件（`manuscript_id` 为空）。池键是“单位 + 方案 + 年度”，
//! 代字只影响显示；重号与断号按池计算，不把同一单位的纪要拆成两个池。

use super::{ManuscriptStore, kind_to_str};
use crate::models::DraftInput;
use crate::platform::doc::{DocNumber, NumberingScheme};
use anyhow::{Result, ensure};
use chrono::Local;
use rusqlite::{Connection, OptionalExtension, params};

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS issue_register (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    manuscript_id INTEGER UNIQUE REFERENCES manuscripts(id) ON DELETE SET NULL,
    unit          TEXT NOT NULL DEFAULT '',
    scheme        TEXT NOT NULL,
    code          TEXT NOT NULL DEFAULT '',
    year          TEXT NOT NULL DEFAULT '',
    serial        TEXT NOT NULL DEFAULT '',
    title         TEXT NOT NULL DEFAULT '',
    kind          TEXT NOT NULL DEFAULT '',
    doc_date      TEXT NOT NULL DEFAULT '',
    security      TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'used',
    note          TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_issue_register_pool
    ON issue_register(unit, scheme, year, serial);
CREATE INDEX IF NOT EXISTS idx_issue_register_manuscript
    ON issue_register(manuscript_id);
"#;

pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueEntry {
    pub id: i64,
    pub manuscript_id: Option<i64>,
    pub unit: String,
    pub scheme: String,
    pub code: String,
    pub year: String,
    pub serial: String,
    pub title: String,
    pub kind: String,
    pub doc_date: String,
    pub security: String,
    pub status: String,
    pub note: String,
}

impl IssueEntry {
    pub(crate) fn number_label(&self) -> String {
        let Some(scheme) = NumberingScheme::from_str(&self.scheme) else {
            return String::new();
        };
        DocNumber {
            scheme,
            code: (!self.code.trim().is_empty()).then(|| self.code.trim().to_string()),
            year: self.year.trim().to_string(),
            serial: self.serial.trim().to_string(),
        }
        .display()
    }

    pub(crate) fn status_label(&self) -> &'static str {
        match self.status.as_str() {
            "reserved" => "占号",
            "void" => "废号",
            _ => "已用",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewIssue {
    pub unit: String,
    pub scheme: NumberingScheme,
    pub code: String,
    pub year: String,
    pub serial: String,
    pub title: String,
    pub doc_date: String,
    pub security: String,
    /// `used` / `reserved` / `void`。手工补录默认为已用，补登废号时显式选择。
    pub status: String,
    pub note: String,
}

impl Default for NewIssue {
    fn default() -> Self {
        Self {
            unit: String::new(),
            scheme: NumberingScheme::Letter,
            code: String::new(),
            year: String::new(),
            serial: String::new(),
            title: String::new(),
            doc_date: String::new(),
            security: "内部".into(),
            status: "used".into(),
            note: String::new(),
        }
    }
}

/// 一篇稿件在台账里归到哪个单位的池。联合发文模式 1 的 `issuing_unit` 是空的，
/// 主发文单位才是红头与代字的来源，台账也跟着它走。
pub(crate) fn effective_issue_unit(draft: &DraftInput) -> &str {
    if draft.kind == crate::models::TemplateKind::OfficialLetter
        && draft.profile.joint_issuance_mode == crate::models::JointIssuanceMode::Mode1
    {
        let main = draft.profile.main_issuing_unit.trim();
        if !main.is_empty() {
            return main;
        }
    }
    draft.profile.issuing_unit.trim()
}

/// 稿件状态到台账状态的默认映射：草稿里预填的号是“占号”，发布/归档才算“已用”。
pub(crate) fn status_for_manuscript(status: crate::models::ManuscriptStatus) -> &'static str {
    match status {
        crate::models::ManuscriptStatus::Published | crate::models::ManuscriptStatus::Archived => {
            "used"
        }
        _ => "reserved",
    }
}

pub(crate) fn valid_status(status: &str) -> bool {
    matches!(status, "used" | "reserved" | "void")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IssueFilter {
    pub year: String,
    pub scheme: Option<String>,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DuplicateGroup {
    pub unit: String,
    pub scheme: String,
    pub year: String,
    pub serial: String,
    pub count: usize,
    pub titles: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GapGroup {
    pub unit: String,
    pub scheme: String,
    pub year: String,
    pub missing: Vec<u32>,
}

impl ManuscriptStore {
    pub(crate) fn list_issue_entries(&self, filter: &IssueFilter) -> Result<Vec<IssueEntry>> {
        let mut sql = String::from(
            "SELECT id, manuscript_id, unit, scheme, code, year, serial, title, kind,
                    doc_date, security, status, note
             FROM issue_register WHERE 1=1",
        );
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if !filter.year.trim().is_empty() {
            sql.push_str(" AND year=?");
            values.push(Box::new(filter.year.trim().to_string()));
        }
        if let Some(scheme) = filter.scheme.as_deref() {
            sql.push_str(" AND scheme=?");
            values.push(Box::new(scheme.to_string()));
        }
        if !filter.unit.trim().is_empty() {
            sql.push_str(" AND unit=?");
            values.push(Box::new(filter.unit.trim().to_string()));
        }
        sql.push_str(" ORDER BY year DESC, unit, scheme, CAST(serial AS INTEGER), serial, id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(values.iter().map(|v| v.as_ref())),
                |row| {
                    Ok(IssueEntry {
                        id: row.get(0)?,
                        manuscript_id: row.get(1)?,
                        unit: row.get(2)?,
                        scheme: row.get(3)?,
                        code: row.get(4)?,
                        year: row.get(5)?,
                        serial: row.get(6)?,
                        title: row.get(7)?,
                        kind: row.get(8)?,
                        doc_date: row.get(9)?,
                        security: row.get(10)?,
                        status: row.get(11)?,
                        note: row.get(12)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// 从稿件库全量重建自动行，手工补录的行不动。返回同步的稿件数。
    ///
    /// 重建前先记下每篇稿件原有的登记状态：用户标过的「废号」「占号」不能因为
    /// 点一次同步就被改回「已用」。
    pub(crate) fn sync_issue_register(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut preserved = std::collections::HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT manuscript_id, status, note FROM issue_register
                 WHERE manuscript_id IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, status, note) = row?;
                preserved.insert(id, (status, note));
            }
        }
        tx.execute(
            "DELETE FROM issue_register WHERE manuscript_id IS NOT NULL",
            [],
        )?;
        let rows = {
            let mut stmt =
                tx.prepare("SELECT id, title, snapshot_json, status FROM manuscripts")?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let now = Local::now().to_rfc3339();
        let mut count = 0;
        for (id, title, json, status) in rows {
            let Ok(snapshot) = serde_json::from_str::<DraftInput>(&json) else {
                continue;
            };
            let manuscript_status =
                super::str_to_status(&status).unwrap_or(crate::models::ManuscriptStatus::Draft);
            if upsert_auto(
                &tx,
                id,
                &title,
                &snapshot,
                manuscript_status,
                preserved
                    .get(&id)
                    .map(|(status, note)| (status.as_str(), note.as_str())),
                &now,
            )? {
                count += 1;
            }
        }
        tx.commit()?;
        Ok(count)
    }

    /// 手工补录。单位和年度、序号必填；代字可空（纪要）。
    pub(crate) fn add_manual_issue(&mut self, new: &NewIssue) -> Result<i64> {
        let unit = new.unit.trim();
        let year = new.year.trim();
        let serial = new.serial.trim();
        ensure!(!unit.is_empty(), "请填写登记单位");
        ensure!(!year.is_empty(), "请填写年度");
        ensure!(!serial.is_empty(), "请填写序号");
        ensure!(valid_status(&new.status), "未知的登记状态");
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO issue_register
                (manuscript_id, unit, scheme, code, year, serial, title, kind, doc_date,
                 security, status, note, created_at, updated_at)
             VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, '', ?7, ?8, ?9, ?10, ?11, ?11)",
            params![
                unit,
                new.scheme.as_str(),
                new.code.trim(),
                year,
                serial,
                new.title.trim(),
                new.doc_date.trim(),
                new.security.trim(),
                new.status,
                new.note.trim(),
                now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 编辑一条手工补录；稿件自动行只允许改状态与备注，正文内容回稿件里改。
    pub(crate) fn update_manual_issue(&mut self, id: i64, new: &NewIssue) -> Result<()> {
        let unit = new.unit.trim();
        let year = new.year.trim();
        let serial = new.serial.trim();
        ensure!(!unit.is_empty(), "请填写登记单位");
        ensure!(!year.is_empty(), "请填写年度");
        ensure!(!serial.is_empty(), "请填写序号");
        ensure!(valid_status(&new.status), "未知的登记状态");
        let changed = self.conn.execute(
            "UPDATE issue_register
             SET unit=?2, scheme=?3, code=?4, year=?5, serial=?6, title=?7, doc_date=?8,
                 security=?9, status=?10, note=?11, updated_at=?12
             WHERE id=?1 AND manuscript_id IS NULL",
            params![
                id,
                unit,
                new.scheme.as_str(),
                new.code.trim(),
                year,
                serial,
                new.title.trim(),
                new.doc_date.trim(),
                new.security.trim(),
                new.status,
                new.note.trim(),
                Local::now().to_rfc3339(),
            ],
        )?;
        ensure!(changed == 1, "只有手工补录的条目可以编辑");
        Ok(())
    }

    /// 改一条台账的状态（已用 / 占号 / 废号）。稿件自动行也能改，改完不会被
    /// 下次保存稿件冲掉，见 `upsert_auto` 的状态保留规则。
    pub(crate) fn set_issue_status(&mut self, id: i64, status: &str) -> Result<()> {
        ensure!(valid_status(status), "未知的登记状态");
        let changed = self.conn.execute(
            "UPDATE issue_register SET status=?2, updated_at=?3 WHERE id=?1",
            params![id, status, Local::now().to_rfc3339()],
        )?;
        ensure!(changed == 1, "台账条目已不存在");
        Ok(())
    }

    /// 改台账备注。备注不参与重号/断号计算，纯登记说明。
    pub(crate) fn set_issue_note(&mut self, id: i64, note: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE issue_register SET note=?2, updated_at=?3 WHERE id=?1",
            params![id, note.trim(), Local::now().to_rfc3339()],
        )?;
        ensure!(changed == 1, "台账条目已不存在");
        Ok(())
    }

    /// 某个池（年度 + 方案 + 一组单位写法）里的全部条目，供起草页取号卡使用。
    ///
    /// 单位在自动行里存的是 `@unit:uuid`，手工行可能是名称或编码，所以调用方把
    /// 同一单位的几种写法都传进来，这里统一按 `IN` 查。
    pub(crate) fn issue_pool_entries(
        &self,
        unit_keys: &[String],
        scheme: NumberingScheme,
        year: &str,
    ) -> Result<Vec<IssueEntry>> {
        if unit_keys.is_empty() {
            return Ok(Vec::new());
        }
        let year = year.trim();
        if year.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", unit_keys.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, manuscript_id, unit, scheme, code, year, serial, title, kind,
                    doc_date, security, status, note
             FROM issue_register
             WHERE year=?1 AND scheme=?2 AND unit IN ({placeholders})
             ORDER BY CAST(serial AS INTEGER), serial, id"
        );
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(year.to_string()),
            Box::new(scheme.as_str().to_string()),
        ];
        for key in unit_keys {
            values.push(Box::new(key.clone()));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(values.iter().map(|v| v.as_ref())),
                |row| {
                    Ok(IssueEntry {
                        id: row.get(0)?,
                        manuscript_id: row.get(1)?,
                        unit: row.get(2)?,
                        scheme: row.get(3)?,
                        code: row.get(4)?,
                        year: row.get(5)?,
                        serial: row.get(6)?,
                        title: row.get(7)?,
                        kind: row.get(8)?,
                        doc_date: row.get(9)?,
                        security: row.get(10)?,
                        status: row.get(11)?,
                        note: row.get(12)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub(crate) fn delete_issue(&mut self, id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM issue_register WHERE id=?1 AND manuscript_id IS NULL",
            [id],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn duplicate_issue_groups(&self) -> Result<Vec<DuplicateGroup>> {
        let mut stmt = self.conn.prepare(
            "SELECT unit, scheme, year, serial, COUNT(*) AS n, GROUP_CONCAT(title, ' | ')
             FROM issue_register
             WHERE status != 'void'
             GROUP BY unit, scheme, year, serial
             HAVING n > 1
             ORDER BY year DESC, unit, scheme, CAST(serial AS INTEGER)",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(DuplicateGroup {
                    unit: row.get(0)?,
                    scheme: row.get(1)?,
                    year: row.get(2)?,
                    serial: row.get(3)?,
                    count: row.get::<_, i64>(4)? as usize,
                    titles: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    #[cfg(test)]
    pub(crate) fn issue_gap_groups(&self) -> Result<Vec<GapGroup>> {
        let mut stmt = self.conn.prepare(
            "SELECT unit, scheme, year, serial
             FROM issue_register
             WHERE serial GLOB '[0-9]*'
             ORDER BY unit, scheme, year, CAST(serial AS INTEGER)",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut groups: Vec<GapGroup> = Vec::new();
        for (unit, scheme, year, serial) in rows {
            let Ok(value) = serial.parse::<u32>() else {
                continue;
            };
            if let Some(group) = groups.last_mut()
                && group.unit == unit
                && group.scheme == scheme
                && group.year == year
            {
                group.missing.push(value);
            } else {
                groups.push(GapGroup {
                    unit,
                    scheme,
                    year,
                    missing: vec![value],
                });
            }
        }
        // 每组已按序号排序；把“已有”换成“缺号”。上限 100，避免空号列表撑爆界面。
        for group in &mut groups {
            let mut present = std::mem::take(&mut group.missing);
            present.sort_unstable();
            present.dedup();
            let mut missing = Vec::new();
            let mut cursor = 1u32;
            for value in present {
                while cursor < value && missing.len() < 100 {
                    missing.push(cursor);
                    cursor += 1;
                }
                cursor = cursor.max(value + 1);
            }
            group.missing = missing;
        }
        // 没有缺号的池不出现在“断号”清单里：界面上的组数就是真正有缺号的池数。
        groups.retain(|group| !group.missing.is_empty());
        Ok(groups)
    }
}

/// 写入/更新一篇稿件的自动台账行；没有文号时删掉旧行。
pub(super) fn upsert_auto(
    conn: &Connection,
    manuscript_id: i64,
    title: &str,
    snapshot: &DraftInput,
    manuscript_status: crate::models::ManuscriptStatus,
    // 同步重建时从删除前的行里带过来的 `(状态, 备注)`；`None` 表示从库里现读。
    inherited: Option<(&str, &str)>,
    now: &str,
) -> Result<bool> {
    let existing = match inherited {
        Some((status, note)) => Some((status.to_string(), note.to_string())),
        None => conn
            .query_row(
                "SELECT status, note FROM issue_register WHERE manuscript_id=?1",
                [manuscript_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?,
    };
    conn.execute(
        "DELETE FROM issue_register WHERE manuscript_id=?1",
        [manuscript_id],
    )?;
    let Some(scheme) = NumberingScheme::for_kind(snapshot.kind) else {
        return Ok(false);
    };
    let year = snapshot.document_year();
    let serial = snapshot.profile.document_number.trim().to_string();
    if year.is_empty() || serial.is_empty() {
        return Ok(false);
    }
    // 用户标过的废号永远保留；发布/归档一律算已用；其余沿用原状态（例如草稿里
    // 手工标成已用），没有旧行才按稿件状态派生。
    let status = match existing.as_ref().map(|(status, _)| status.as_str()) {
        Some("void") => "void",
        _ if matches!(
            manuscript_status,
            crate::models::ManuscriptStatus::Published | crate::models::ManuscriptStatus::Archived
        ) =>
        {
            "used"
        }
        Some(status) => status,
        None => status_for_manuscript(manuscript_status),
    };
    let note = existing
        .as_ref()
        .map(|(_, note)| note.clone())
        .unwrap_or_default();
    conn.execute(
        "INSERT INTO issue_register
            (manuscript_id, unit, scheme, code, year, serial, title, kind, doc_date,
             security, status, note, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![
            manuscript_id,
            effective_issue_unit(snapshot),
            scheme.as_str(),
            snapshot.profile.department_code.trim(),
            year,
            serial,
            title.trim(),
            kind_to_str(snapshot.kind),
            snapshot.date.trim(),
            snapshot.security_marking().0,
            status,
            note,
            now,
        ],
    )?;
    Ok(true)
}

/// 稿件发布/归档时把台账行推到“已用”，但用户标过的“废号”不动。
pub(super) fn mark_used(conn: &Connection, manuscript_id: i64, now: &str) -> Result<()> {
    conn.execute(
        "UPDATE issue_register SET status='used', updated_at=?2
         WHERE manuscript_id=?1 AND status!='void'",
        params![manuscript_id, now],
    )?;
    Ok(())
}

pub(super) fn remove_auto(conn: &Connection, manuscript_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM issue_register WHERE manuscript_id=?1",
        [manuscript_id],
    )?;
    Ok(())
}

/// 当前稿件的台账行（测试/界面同步用）。
#[cfg(test)]
pub(crate) fn issue_for_manuscript(store: &ManuscriptStore, id: i64) -> Result<Option<IssueEntry>> {
    Ok(store
        .list_issue_entries(&IssueFilter::default())?
        .into_iter()
        .find(|entry| entry.manuscript_id == Some(id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::{ManuscriptUpdate, NewManuscript};
    use crate::models::{DraftInput, ManuscriptStatus, TemplateKind, TemplateProfile};

    fn numbered(kind: TemplateKind, code: &str, year: &str, serial: &str) -> NewManuscript {
        NewManuscript {
            snapshot: DraftInput {
                kind,
                date: "2026-09-30".into(),
                profile: TemplateProfile {
                    department_code: code.into(),
                    document_year: year.into(),
                    document_number: serial.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            content_markdown: "# 台账测试\n\n正文。".into(),
            status: ManuscriptStatus::Draft,
            ..Default::default()
        }
    }

    #[test]
    fn create_update_and_delete_follow_the_manuscript() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let id = store
            .create(
                &numbered(TemplateKind::OfficialLetter, "某函", "2026", "12"),
                None,
            )
            .unwrap();
        let entry = issue_for_manuscript(&store, id).unwrap().unwrap();
        assert_eq!(entry.number_label(), "某函〔2026〕12号");
        assert_eq!(entry.scheme, "letter");

        store
            .update(
                id,
                &ManuscriptUpdate {
                    snapshot: numbered(TemplateKind::OfficialLetter, "某函", "2026", "13").snapshot,
                    content_markdown: "# 台账测试\n\n改后正文。".into(),
                    notes: String::new(),
                },
            )
            .unwrap();
        assert_eq!(
            issue_for_manuscript(&store, id)
                .unwrap()
                .unwrap()
                .number_label(),
            "某函〔2026〕13号"
        );

        store.delete(id).unwrap();
        assert!(issue_for_manuscript(&store, id).unwrap().is_none());
    }

    #[test]
    fn minutes_with_and_without_code_share_one_pool() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let with_code = numbered(TemplateKind::MeetingMinutes, "某纪", "2026", "3");
        let mut without = numbered(TemplateKind::MeetingMinutes, "", "2026", "4");
        without.snapshot.profile.issuing_unit = "甲单位".into();
        let mut with = with_code;
        with.snapshot.profile.issuing_unit = "甲单位".into();
        store.create(&with, None).unwrap();
        store.create(&without, None).unwrap();
        let entries = store.list_issue_entries(&IssueFilter::default()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .any(|entry| entry.number_label() == "某纪〔2026〕3号")
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.number_label() == "〔2026〕4号")
        );
    }

    #[test]
    fn manual_entries_do_not_need_a_manuscript_and_can_be_deleted() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let id = store
            .add_manual_issue(&NewIssue {
                unit: "甲单位".into(),
                scheme: NumberingScheme::Letter,
                code: "某函".into(),
                year: "2025".into(),
                serial: "7".into(),
                title: "纸质旧文".into(),
                doc_date: "2025-03-01".into(),
                security: "内部".into(),
                status: "used".into(),
                note: "档案室补录".into(),
            })
            .unwrap();
        let entries = store.list_issue_entries(&IssueFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].manuscript_id.is_none());
        store.delete_issue(id).unwrap();
        assert!(
            store
                .list_issue_entries(&IssueFilter::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn duplicate_and_gap_checks_ignore_void_rows() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        for serial in ["1", "1", "3"] {
            store
                .create(
                    &numbered(TemplateKind::OfficialLetter, "某函", "2026", serial),
                    None,
                )
                .unwrap();
        }
        let duplicates = store.duplicate_issue_groups().unwrap();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].serial, "1");
        let gaps = store.issue_gap_groups().unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].missing, [2]);
    }

    #[test]
    fn draft_numbers_are_reserved_until_published() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let id = store
            .create(
                &numbered(TemplateKind::OfficialLetter, "某函", "2026", "12"),
                None,
            )
            .unwrap();
        let entry = issue_for_manuscript(&store, id).unwrap().unwrap();
        assert_eq!(entry.status, "reserved");
        store.set_status(id, ManuscriptStatus::Published).unwrap();
        let entry = issue_for_manuscript(&store, id).unwrap().unwrap();
        assert_eq!(entry.status, "used");
    }

    #[test]
    fn sync_preserves_status_and_note_on_auto_rows() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let id = store
            .create(
                &numbered(TemplateKind::OfficialLetter, "某函", "2026", "1"),
                None,
            )
            .unwrap();
        let entry = issue_for_manuscript(&store, id).unwrap().unwrap();
        store.set_issue_status(entry.id, "void").unwrap();
        store.set_issue_note(entry.id, "手工作废").unwrap();
        store.sync_issue_register().unwrap();
        let entry = issue_for_manuscript(&store, id).unwrap().unwrap();
        assert_eq!(entry.status, "void");
        assert_eq!(entry.note, "手工作废");
    }

    #[test]
    fn manual_entries_can_be_edited_and_marked_void() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let id = store
            .add_manual_issue(&NewIssue {
                unit: "甲单位".into(),
                year: "2026".into(),
                serial: "1".into(),
                title: "旧标题".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .update_manual_issue(
                id,
                &NewIssue {
                    unit: "甲单位".into(),
                    year: "2026".into(),
                    serial: "1".into(),
                    title: "新标题".into(),
                    status: "void".into(),
                    note: "作废".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let entry = store
            .list_issue_entries(&IssueFilter::default())
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == id)
            .unwrap();
        assert_eq!(entry.title, "新标题");
        assert_eq!(entry.status_label(), "废号");
        assert!(store.issue_gap_groups().unwrap().is_empty());
    }

    #[test]
    fn pool_query_accepts_ref_and_name_forms_of_the_same_unit() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let mut ref_row = numbered(TemplateKind::OfficialLetter, "某函", "2026", "1");
        ref_row.snapshot.profile.issuing_unit = "@unit:abc".into();
        store.create(&ref_row, None).unwrap();
        store
            .add_manual_issue(&NewIssue {
                unit: "甲单位".into(),
                year: "2026".into(),
                serial: "2".into(),
                ..Default::default()
            })
            .unwrap();
        let pool = store
            .issue_pool_entries(
                &["@unit:abc".to_string(), "甲单位".to_string()],
                NumberingScheme::Letter,
                "2026",
            )
            .unwrap();
        assert_eq!(pool.len(), 2);
    }
}
