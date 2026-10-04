//! AI 侧栏会话的存储（`docs/ai-agent-workbench.md` 16.15 B）。
//!
//! 每篇稿件可以有多个会话，每个会话存若干轮；一轮整个存成一段 JSON（卡片上看得到的东西、
//! 挂起的流程、待确认的提案），格式由 `ai_panel::session` 定，这里只管存取。会话是本机工作
//! 现场：不进版本快照、不进离线同步 ZIP，稿件删除时随外键级联清除。
use super::ManuscriptStore;
use anyhow::Result;
use rusqlite::{Connection, params};

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS ai_sessions (
    id             TEXT    PRIMARY KEY,
    manuscript_id  INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
    title          TEXT    NOT NULL DEFAULT '',
    -- 压缩出的会话摘要，与它覆盖到的最后一轮
    summary        TEXT    NOT NULL DEFAULT '',
    compacted_upto INTEGER NOT NULL DEFAULT 0,
    -- 这篇稿件当前打开的会话（每篇至多一个为 1）
    is_current     INTEGER NOT NULL DEFAULT 0,
    -- 上次关掉时侧栏开着没有（只看当前会话这一行）
    panel_open     INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT    NOT NULL,
    updated_at     TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ai_sessions_manuscript ON ai_sessions(manuscript_id);
CREATE TABLE IF NOT EXISTS ai_session_turns (
    session_id TEXT    NOT NULL REFERENCES ai_sessions(id) ON DELETE CASCADE,
    turn_id    INTEGER NOT NULL,
    data       TEXT    NOT NULL,
    updated_at TEXT    NOT NULL,
    PRIMARY KEY (session_id, turn_id)
);
"#;

/// 一个会话的抬头（不含各轮）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiSessionRecord {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub compacted_upto: i64,
    pub is_current: bool,
    pub panel_open: bool,
    /// RFC3339。
    pub created_at: String,
    pub updated_at: String,
    /// 有几轮（列表里显示）。
    pub turns: usize,
}

/// 幂等建表，与候选区一样不单开 `user_version` 档位。
pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)
}

impl ManuscriptStore {
    /// 一篇稿件的全部会话，最近更新的在前。
    pub fn list_ai_sessions(&self, manuscript_id: i64) -> Result<Vec<AiSessionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.title, s.summary, s.compacted_upto, s.is_current, s.panel_open,
                    s.created_at, s.updated_at,
                    (SELECT COUNT(*) FROM ai_session_turns t WHERE t.session_id = s.id)
             FROM ai_sessions s WHERE s.manuscript_id = ?1
             ORDER BY s.updated_at DESC, s.rowid DESC",
        )?;
        let rows = stmt.query_map([manuscript_id], |row| {
            Ok(AiSessionRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                summary: row.get(2)?,
                compacted_upto: row.get(3)?,
                is_current: row.get::<_, i64>(4)? != 0,
                panel_open: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                turns: row.get::<_, i64>(8)? as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 一个会话的各轮 `(轮次 id, JSON)`，按轮次先后。
    pub fn load_ai_session_turns(&self, session_id: &str) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT turn_id, data FROM ai_session_turns WHERE session_id = ?1 ORDER BY turn_id",
        )?;
        let rows = stmt.query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 写会话抬头与改过的轮次，删掉不在 `keep` 里的旧轮次（裁剪、清空记录）。`is_current` 为真时
    /// 同一篇稿件的其他会话都改成非当前。
    pub fn save_ai_session(
        &mut self,
        manuscript_id: i64,
        session: &AiSessionRecord,
        turns: &[(i64, String)],
        keep: &[i64],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        if session.is_current {
            tx.execute(
                "UPDATE ai_sessions SET is_current = 0 WHERE manuscript_id = ?1 AND id <> ?2",
                params![manuscript_id, session.id],
            )?;
        }
        tx.execute(
            "INSERT INTO ai_sessions
             (id, manuscript_id, title, summary, compacted_upto, is_current, panel_open,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title, summary = excluded.summary,
               compacted_upto = excluded.compacted_upto, is_current = excluded.is_current,
               panel_open = excluded.panel_open, updated_at = excluded.updated_at",
            params![
                session.id,
                manuscript_id,
                session.title,
                session.summary,
                session.compacted_upto,
                session.is_current as i64,
                session.panel_open as i64,
                session.created_at,
                session.updated_at,
            ],
        )?;
        {
            let mut upsert = tx.prepare(
                "INSERT INTO ai_session_turns (session_id, turn_id, data, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(session_id, turn_id) DO UPDATE SET
                   data = excluded.data, updated_at = excluded.updated_at",
            )?;
            for (turn_id, data) in turns {
                upsert.execute(params![session.id, turn_id, data, session.updated_at])?;
            }
            let existing: Vec<i64> = {
                let mut stmt =
                    tx.prepare("SELECT turn_id FROM ai_session_turns WHERE session_id = ?1")?;
                stmt.query_map([&session.id], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for turn_id in existing.iter().filter(|id| !keep.contains(id)) {
                tx.execute(
                    "DELETE FROM ai_session_turns WHERE session_id = ?1 AND turn_id = ?2",
                    params![session.id, turn_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 只改会话标题（历史列表里改名）。
    pub fn rename_ai_session(&mut self, session_id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE ai_sessions SET title = ?2 WHERE id = ?1",
            params![session_id, title],
        )?;
        Ok(())
    }

    /// 删一个会话，连同它的各轮。
    pub fn delete_ai_session(&mut self, session_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM ai_sessions WHERE id = ?1", [session_id])?;
        Ok(())
    }

    /// 每篇稿件只留最近 `keep` 个会话（当前会话总是留着），返回删了几个。
    pub fn prune_ai_sessions(&mut self, manuscript_id: i64, keep: usize) -> Result<usize> {
        let sessions = self.list_ai_sessions(manuscript_id)?;
        let mut kept = 0usize;
        let mut removed = 0usize;
        for session in sessions {
            if session.is_current || kept < keep.saturating_sub(1) {
                if !session.is_current {
                    kept += 1;
                }
                continue;
            }
            self.delete_ai_session(&session.id)?;
            removed += 1;
        }
        Ok(removed)
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
                    content_markdown: "正文。\n".into(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        (store, id)
    }

    fn session(id: &str, updated: &str, current: bool) -> AiSessionRecord {
        AiSessionRecord {
            id: id.into(),
            title: format!("会话{id}"),
            is_current: current,
            created_at: updated.into(),
            updated_at: updated.into(),
            ..Default::default()
        }
    }

    #[test]
    fn sessions_and_turns_round_trip() {
        let (mut store, ms) = store_with_manuscript();
        let a = session("a", "2026-10-04T10:00:00+08:00", true);
        store
            .save_ai_session(
                ms,
                &a,
                &[(1, "{\"x\":1}".into()), (2, "{\"x\":2}".into())],
                &[1, 2],
            )
            .unwrap();
        assert_eq!(
            store.load_ai_session_turns("a").unwrap(),
            vec![(1, "{\"x\":1}".into()), (2, "{\"x\":2}".into())]
        );
        // 只改第 2 轮、裁掉第 1 轮。
        store
            .save_ai_session(ms, &a, &[(2, "{\"x\":22}".into())], &[2])
            .unwrap();
        assert_eq!(
            store.load_ai_session_turns("a").unwrap(),
            vec![(2, "{\"x\":22}".into())]
        );
        // 新会话设为当前，旧的自动改成非当前。
        let b = session("b", "2026-10-04T11:00:00+08:00", true);
        store.save_ai_session(ms, &b, &[], &[]).unwrap();
        let list = store.list_ai_sessions(ms).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "b");
        assert!(list[0].is_current);
        assert!(!list[1].is_current);
        assert_eq!(list[1].turns, 1);
        store.rename_ai_session("a", "改过的名字").unwrap();
        assert_eq!(store.list_ai_sessions(ms).unwrap()[1].title, "改过的名字");
        store.delete_ai_session("a").unwrap();
        assert!(store.load_ai_session_turns("a").unwrap().is_empty());
        assert_eq!(store.list_ai_sessions(ms).unwrap().len(), 1);
    }

    #[test]
    fn old_sessions_are_pruned_but_the_current_one_stays() {
        let (mut store, ms) = store_with_manuscript();
        for i in 0..5 {
            let s = session(
                &i.to_string(),
                &format!("2026-10-04T1{i}:00:00+08:00"),
                i == 0,
            );
            store.save_ai_session(ms, &s, &[], &[]).unwrap();
        }
        assert_eq!(store.prune_ai_sessions(ms, 3).unwrap(), 2);
        let ids: Vec<String> = store
            .list_ai_sessions(ms)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["4", "3", "0"], "最近两个 + 当前那个");
    }

    #[test]
    fn deleting_the_manuscript_removes_its_sessions() {
        let (mut store, ms) = store_with_manuscript();
        store
            .save_ai_session(
                ms,
                &session("a", "2026-10-04T10:00:00+08:00", true),
                &[(1, "{}".into())],
                &[1],
            )
            .unwrap();
        store
            .conn
            .execute("DELETE FROM manuscripts WHERE id = ?1", [ms])
            .unwrap();
        assert!(store.list_ai_sessions(ms).unwrap().is_empty());
        assert!(store.load_ai_session_turns("a").unwrap().is_empty());
    }
}
