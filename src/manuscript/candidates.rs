//! 起草页候选区的存储：写稿时从正文暂时移走、以后可能还要用的文字。
//!
//! 候选区按稿件各存一份，只随稿件保存：不进版本快照、不进离线同步 ZIP、不参与
//! 三方合并，稿件删除时随外键级联清除。条目不多，界面每改一次就把这篇的候选
//! 整组重写一遍，省去逐行对账。
use super::ManuscriptStore;
use anyhow::Result;
use rusqlite::{Connection, params};

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS draft_candidates (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    manuscript_id INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    -- 界面里的排列顺序，0 在最上面
    ord           INTEGER NOT NULL,
    text          TEXT    NOT NULL,
    -- 移入时所在的一级小节：标题文字（不含编号）用于分组，带编号的字面用于显示；
    -- 第一个标题之前的内容两者都为空
    section       TEXT    NOT NULL DEFAULT '',
    section_label TEXT    NOT NULL DEFAULT '',
    -- 更深一级的最近标题字面，只作条目说明
    subsection    TEXT    NOT NULL DEFAULT '',
    created_at    TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_draft_candidates_manuscript ON draft_candidates(manuscript_id);
"#;

/// 一条候选文字。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CandidateRecord {
    pub text: String,
    pub section: String,
    pub section_label: String,
    pub subsection: String,
    /// RFC3339。
    pub created_at: String,
}

/// 幂等建表，与知识库、词表一样不单开 `user_version` 档位。
pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)
}

impl ManuscriptStore {
    /// 读出一篇稿件的候选，按界面顺序。
    pub fn load_candidates(&self, manuscript_id: i64) -> Result<Vec<CandidateRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT text, section, section_label, subsection, created_at
             FROM draft_candidates WHERE manuscript_id=?1 ORDER BY ord, id",
        )?;
        let rows = stmt.query_map([manuscript_id], |row| {
            Ok(CandidateRecord {
                text: row.get(0)?,
                section: row.get(1)?,
                section_label: row.get(2)?,
                subsection: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 用给定内容整组替换一篇稿件的候选。
    pub fn save_candidates(
        &mut self,
        manuscript_id: i64,
        candidates: &[CandidateRecord],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM draft_candidates WHERE manuscript_id=?1",
            [manuscript_id],
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO draft_candidates
                 (manuscript_id, ord, text, section, section_label, subsection, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (ord, candidate) in candidates.iter().enumerate() {
                insert.execute(params![
                    manuscript_id,
                    ord as i64,
                    candidate.text,
                    candidate.section,
                    candidate.section_label,
                    candidate.subsection,
                    candidate.created_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::NewManuscript;
    use crate::models::ManuscriptStatus;
    use std::path::Path;

    fn store_with_manuscript() -> (ManuscriptStore, i64) {
        let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
        let id = store
            .create(
                &NewManuscript {
                    content_markdown: "## 主要做法\n\n正文。\n".into(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        (store, id)
    }

    fn record(text: &str) -> CandidateRecord {
        CandidateRecord {
            text: text.into(),
            section: "主要做法".into(),
            section_label: "一、主要做法".into(),
            subsection: String::new(),
            created_at: "2026-09-29T10:00:00+08:00".into(),
        }
    }

    #[test]
    fn candidates_round_trip_in_order_and_replace_as_a_whole() {
        let (mut store, id) = store_with_manuscript();
        assert!(store.load_candidates(id).unwrap().is_empty());
        let first = vec![record("甲"), record("乙"), record("丙")];
        store.save_candidates(id, &first).unwrap();
        assert_eq!(store.load_candidates(id).unwrap(), first);

        let second = vec![record("丙"), record("甲")];
        store.save_candidates(id, &second).unwrap();
        assert_eq!(store.load_candidates(id).unwrap(), second);

        store.save_candidates(id, &[]).unwrap();
        assert!(store.load_candidates(id).unwrap().is_empty());
    }

    #[test]
    fn candidates_are_kept_per_manuscript_and_removed_with_it() {
        let (mut store, a) = store_with_manuscript();
        let b = store
            .create(
                &NewManuscript {
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        store.save_candidates(a, &[record("甲")]).unwrap();
        store.save_candidates(b, &[record("乙")]).unwrap();
        assert_eq!(store.load_candidates(a).unwrap(), vec![record("甲")]);

        store.delete(a).unwrap();
        assert!(store.load_candidates(a).unwrap().is_empty());
        assert_eq!(store.load_candidates(b).unwrap(), vec![record("乙")]);
    }

    #[test]
    fn schema_is_idempotent() {
        let (store, _) = store_with_manuscript();
        ensure_schema(&store.conn).unwrap();
        ensure_schema(&store.conn).unwrap();
    }
}
