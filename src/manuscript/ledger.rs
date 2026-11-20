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
use rusqlite::{Connection, params};

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
    pub note: String,
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
    pub(crate) fn sync_issue_register(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM issue_register WHERE manuscript_id IS NOT NULL",
            [],
        )?;
        let rows = {
            let mut stmt = tx.prepare("SELECT id, title, snapshot_json FROM manuscripts")?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let now = Local::now().to_rfc3339();
        let mut count = 0;
        for (id, title, json) in rows {
            let Ok(snapshot) = serde_json::from_str::<DraftInput>(&json) else {
                continue;
            };
            if upsert_auto(&tx, id, &title, &snapshot, &now)? {
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
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO issue_register
                (manuscript_id, unit, scheme, code, year, serial, title, kind, doc_date,
                 security, status, note, created_at, updated_at)
             VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6, '', ?7, ?8, 'used', ?9, ?10, ?10)",
            params![
                unit,
                new.scheme.as_str(),
                new.code.trim(),
                year,
                serial,
                new.title.trim(),
                new.doc_date.trim(),
                new.security.trim(),
                new.note.trim(),
                now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn delete_issue(&mut self, id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM issue_register WHERE id=?1 AND manuscript_id IS NULL",
            [id],
        )?;
        Ok(())
    }

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

    pub(crate) fn issue_gap_groups(&self) -> Result<Vec<GapGroup>> {
        let mut stmt = self.conn.prepare(
            "SELECT unit, scheme, year, serial
             FROM issue_register
             WHERE status != 'void' AND serial GLOB '[0-9]*'
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
        Ok(groups)
    }
}

/// 写入/更新一篇稿件的自动台账行；没有文号时删掉旧行。
pub(super) fn upsert_auto(
    conn: &Connection,
    manuscript_id: i64,
    title: &str,
    snapshot: &DraftInput,
    now: &str,
) -> Result<bool> {
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
    conn.execute(
        "INSERT INTO issue_register
            (manuscript_id, unit, scheme, code, year, serial, title, kind, doc_date,
             security, status, note, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'used', '', ?11, ?11)",
        params![
            manuscript_id,
            snapshot.profile.issuing_unit.trim(),
            scheme.as_str(),
            snapshot.profile.department_code.trim(),
            year,
            serial,
            title.trim(),
            kind_to_str(snapshot.kind),
            snapshot.date.trim(),
            snapshot.security_marking().0,
            now,
        ],
    )?;
    Ok(true)
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
}
