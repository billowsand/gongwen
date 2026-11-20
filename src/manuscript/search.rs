//! 快捷查找只取列表元数据，避免为了找标题载入每篇稿件的完整要素快照。

use super::{ManuscriptStore, str_to_kind, str_to_status};
use crate::models::{ManuscriptStatus, TemplateKind};
use crate::platform::doc::{DocNumber, NumberingScheme};
use crate::platform::search::{SearchHit, Snippet, fts_query, index_text, query_tokens};
use anyhow::Result;
use rusqlite::{Connection, params};

const DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS manuscript_fts USING fts5(tokens, tokenize='unicode61');
"#;

/// 幂等建表；与稿件库其他候选区一样不单开 `user_version` 档位。
pub(super) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)
}

/// 写入一篇稿件的正文索引。标题与正文一起进 tokens，标题命中也能被搜到。
pub(super) fn index_row(conn: &Connection, id: i64, title: &str, content: &str) -> Result<()> {
    let tokens = index_text(title, content);
    conn.execute("DELETE FROM manuscript_fts WHERE rowid=?1", [id])?;
    conn.execute(
        "INSERT INTO manuscript_fts(rowid, tokens) VALUES (?1, ?2)",
        params![id, tokens],
    )?;
    Ok(())
}

pub(super) fn remove_row(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM manuscript_fts WHERE rowid=?1", [id])?;
    Ok(())
}

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
        let Some(scheme) = NumberingScheme::for_kind(self.kind) else {
            return "未填文号".into();
        };
        let serial = self.doc_number.trim();
        if serial.is_empty() {
            return "未填文号".into();
        }
        let explicit = self.document_year.trim();
        let fallback = self.doc_date.trim().chars().take(4).collect::<String>();
        let year = if !explicit.is_empty() {
            explicit.to_string()
        } else if fallback.len() == 4 && fallback.chars().all(|ch| ch.is_ascii_digit()) {
            fallback
        } else {
            String::new()
        };
        let code = (!self.department_code.trim().is_empty())
            .then(|| self.department_code.trim().to_string());
        let label = DocNumber {
            scheme,
            code,
            year,
            serial: serial.to_string(),
        }
        .display();
        if label.is_empty() {
            "未填文号".into()
        } else {
            label
        }
    }
}

impl ManuscriptStore {
    /// 正文索引已覆盖的篇数与稿件总数。应用启动时据此决定要不要后台补建。
    pub fn search_index_counts(&self) -> Result<(i64, i64)> {
        let indexed: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM manuscript_fts", [], |row| row.get(0))?;
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM manuscripts", [], |row| row.get(0))?;
        Ok((indexed, total))
    }

    /// 全量重建正文索引，返回重建的篇数。旧库升级、索引损坏时用。
    pub fn rebuild_search_index(&mut self) -> Result<usize> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM manuscript_fts", [])?;
        let rows = {
            let mut stmt = tx.prepare("SELECT id, title, content_markdown FROM manuscripts")?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, title, content) in &rows {
            index_row(&tx, *id, title, content)?;
        }
        tx.commit()?;
        Ok(rows.len())
    }

    /// 正文检索：返回稿件、片段与命中位置。`limit` 最大 100。
    pub fn search_content(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let query = query.trim();
        if query.chars().count() > 256 {
            anyhow::bail!("检索词过长，最多 256 字");
        }
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = limit.clamp(1, 100) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.title, m.kind, m.doc_date, m.content_markdown,
                    bm25(manuscript_fts) AS rank
             FROM manuscript_fts
             JOIN manuscripts m ON m.id = manuscript_fts.rowid
             WHERE manuscript_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![fts, limit], |row| {
                let kind =
                    str_to_kind(&row.get::<_, String>(2)?).unwrap_or(TemplateKind::OfficialLetter);
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    kind,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, f64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let tokens = query_tokens(query);
        Ok(rows
            .into_iter()
            .map(|(id, title, kind, doc_date, content, rank)| SearchHit {
                manuscript_id: id,
                title: title.clone(),
                kind,
                doc_date,
                score: -(rank as f32),
                snippets: snippets(&title, &content, &tokens),
            })
            .collect())
    }

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

fn snippets(title: &str, content: &str, tokens: &[String]) -> Vec<Snippet> {
    if tokens.is_empty() {
        return Vec::new();
    }
    let haystack = content.to_lowercase();
    let found = tokens
        .iter()
        .map(|token| token.to_lowercase())
        .find(|token| !token.is_empty() && haystack.contains(token));
    let Some(needle) = found else {
        // 标题命中、正文没有：片段退回标题，至少让人看见为什么命中。
        let title_haystack = title.to_lowercase();
        if tokens
            .iter()
            .any(|token| title_haystack.contains(&token.to_lowercase()))
        {
            return vec![Snippet {
                text: title.to_string(),
                span: 0..title.len(),
            }];
        }
        return Vec::new();
    };
    let Some(byte) = haystack.find(&needle) else {
        return Vec::new();
    };
    let start = floor_char_boundary(content, byte.saturating_sub(36));
    let end = ceil_char_boundary(content, (byte + needle.len() + 60).min(content.len()));
    let mut text = content[start..end]
        .replace(['\n', '\r'], " ")
        .trim()
        .to_string();
    if start > 0 {
        text.insert(0, '…');
    }
    if end < content.len() {
        text.push('…');
    }
    vec![Snippet {
        text,
        span: start..end,
    }]
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(value: &str, mut index: usize) -> usize {
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::NewManuscript;
    use crate::models::TemplateProfile;

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

    fn content_manuscript(content: &str) -> NewManuscript {
        NewManuscript {
            snapshot: crate::models::DraftInput::default(),
            content_markdown: content.into(),
            status: ManuscriptStatus::Draft,
            ..Default::default()
        }
    }

    #[test]
    fn content_search_finds_body_text_and_reports_a_snippet() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let body_id = store
            .create(
                &content_manuscript("# 通知\n\n请各单位做好防汛值班安排。"),
                None,
            )
            .unwrap();
        store
            .create(&content_manuscript("# 另一篇\n\n与检索无关的内容。"), None)
            .unwrap();

        let hits = store.search_content("防汛值班", 10).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].manuscript_id, body_id);
        assert!(hits[0].snippets[0].text.contains("防汛"));
        assert!(
            store
                .search_content("完全不存在的词", 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn content_search_follows_updates_and_deletes() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let id = store
            .create(&content_manuscript("# 通知\n\n旧内容。"), None)
            .unwrap();
        assert_eq!(store.search_content("旧内容", 10).unwrap().len(), 1);

        store
            .update(
                id,
                &crate::manuscript::ManuscriptUpdate {
                    snapshot: crate::models::DraftInput::default(),
                    content_markdown: "# 通知\n\n新内容。".into(),
                    notes: String::new(),
                },
            )
            .unwrap();
        assert!(store.search_content("旧内容", 10).unwrap().is_empty());
        assert_eq!(store.search_content("新内容", 10).unwrap().len(), 1);

        store.delete(id).unwrap();
        assert!(store.search_content("新内容", 10).unwrap().is_empty());
    }

    #[test]
    fn rebuild_search_index_fills_missing_rows() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        store
            .create(&content_manuscript("# 甲\n\n需要补建索引。"), None)
            .unwrap();
        store
            .create(&content_manuscript("# 乙\n\n也需要补建。"), None)
            .unwrap();
        store
            .conn
            .execute("DELETE FROM manuscript_fts", [])
            .unwrap();
        assert_eq!(store.search_index_counts().unwrap(), (0, 2));
        assert_eq!(store.rebuild_search_index().unwrap(), 2);
        assert_eq!(store.search_index_counts().unwrap(), (2, 2));
        assert_eq!(store.search_content("补建", 10).unwrap().len(), 2);
    }

    #[test]
    fn query_syntax_is_treated_as_literal_text() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        store
            .create(&content_manuscript("# 通知\n\n普通的正文。"), None)
            .unwrap();
        assert!(store.search_content("\" OR *", 10).unwrap().is_empty());
    }

    #[test]
    fn minutes_number_label_is_not_treated_as_missing() {
        let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
        let new = NewManuscript {
            snapshot: crate::models::DraftInput {
                kind: TemplateKind::MeetingMinutes,
                date: "2026-09-30".into(),
                profile: TemplateProfile {
                    document_year: "2026".into(),
                    document_number: "3".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            content_markdown: "# 会议纪要\n\n正文。".into(),
            status: ManuscriptStatus::Draft,
            ..Default::default()
        };
        store.create(&new, None).unwrap();
        assert_eq!(
            store.search_rows().unwrap()[0].number_label(),
            "〔2026〕3号"
        );
    }
}
