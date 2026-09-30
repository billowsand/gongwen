//! 快捷查找只取列表元数据，避免为了找标题载入每篇稿件的完整要素快照。

use super::{ManuscriptStore, str_to_kind, str_to_status};
use crate::models::{ManuscriptStatus, TemplateKind};
use anyhow::Result;

pub(crate) struct SearchRow {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) kind: TemplateKind,
    pub(crate) status: ManuscriptStatus,
    pub(crate) doc_number: String,
    pub(crate) doc_date: String,
    department_code: String,
    document_year: String,
}

impl SearchRow {
    pub(crate) fn number_label(&self) -> String {
        if !self.kind.has_document_number() || self.doc_number.trim().is_empty() {
            return "未填文号".into();
        }
        let explicit = self.document_year.trim();
        let fallback = self.doc_date.trim().chars().take(4).collect::<String>();
        let year = if !explicit.is_empty() {
            explicit
        } else if fallback.len() == 4 && fallback.chars().all(|ch| ch.is_ascii_digit()) {
            &fallback
        } else {
            ""
        };
        if year.is_empty() {
            format!(
                "{}{}号",
                self.department_code.trim(),
                self.doc_number.trim()
            )
        } else {
            format!(
                "{}〔{}〕{}号",
                self.department_code.trim(),
                year,
                self.doc_number.trim()
            )
        }
    }
}

impl ManuscriptStore {
    pub(crate) fn search_rows(&self) -> Result<Vec<SearchRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, kind, status, doc_number, doc_date,
             CAST(COALESCE(json_extract(CASE WHEN json_valid(snapshot_json) THEN snapshot_json ELSE '{}' END, '$.profile.department_code'), '') AS TEXT),
             CAST(COALESCE(json_extract(CASE WHEN json_valid(snapshot_json) THEN snapshot_json ELSE '{}' END, '$.profile.document_year'), '') AS TEXT)
             FROM manuscripts ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SearchRow {
                id: row.get(0)?,
                title: row.get(1)?,
                kind: str_to_kind(&row.get::<_, String>(2)?)
                    .unwrap_or(TemplateKind::OfficialLetter),
                status: str_to_status(&row.get::<_, String>(3)?).unwrap_or(ManuscriptStatus::Draft),
                doc_number: row.get(4)?,
                doc_date: row.get(5)?,
                department_code: row.get(6)?,
                document_year: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::NewManuscript;

    #[test]
    fn search_rows_preserve_identity_and_explicit_document_year() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let mut new = NewManuscript::default();
        new.snapshot.kind = TemplateKind::OfficialLetter;
        new.snapshot.date = "2026-09-30".into();
        new.snapshot.profile.department_code = "综办".into();
        new.snapshot.profile.document_number = "12".into();
        new.snapshot.profile.document_year = "2025".into();
        new.content_markdown = "# 防汛通知\n\n正文。".into();
        new.status = ManuscriptStatus::Draft;
        let id = store.create(&new, None).unwrap();
        let rows = store.search_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].title, "防汛通知");
        assert_eq!(rows[0].number_label(), "综办〔2025〕12号");
        store.set_status(id, ManuscriptStatus::Archived).unwrap();
        assert_eq!(
            store.search_rows().unwrap()[0].status,
            ManuscriptStatus::Archived
        );
    }
}
