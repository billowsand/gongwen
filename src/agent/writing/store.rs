//! 稿件关联的分阶段写作任务存储。

use super::{MAX_SOURCE_BYTES, MAX_TOTAL_SOURCE_BYTES, WritingTask};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

const MAX_TASK_JSON_BYTES: usize = 96 * 1024 * 1024;

pub(crate) fn load(manuscript_id: i64) -> Result<Option<WritingTask>> {
    load_at(&crate::storage::manuscript_db_path()?, manuscript_id)
}

pub(crate) fn save(manuscript_id: i64, task: &WritingTask) -> Result<()> {
    save_at(&crate::storage::manuscript_db_path()?, manuscript_id, task)
}

pub(crate) fn load_at(path: &Path, manuscript_id: i64) -> Result<Option<WritingTask>> {
    let mut database = connect(path)?;
    let conn = database.transaction()?;
    let size: Option<i64> = conn.query_row(
        "SELECT length(CAST(task_json AS BLOB)) FROM staged_writing_tasks WHERE manuscript_id=?1",
        [manuscript_id], |row| row.get(0),
    ).optional()?;
    if let Some(size) = size {
        anyhow::ensure!(
            size >= 0 && size as u64 <= MAX_TASK_JSON_BYTES as u64,
            "写作任务数据超过安全上限"
        );
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT task_json FROM staged_writing_tasks WHERE manuscript_id=?1",
            [manuscript_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        anyhow::ensure!(raw.len() <= MAX_TASK_JSON_BYTES, "写作任务数据超过安全上限");
        let mut task: WritingTask =
            serde_json::from_str(&raw).context("写作任务数据损坏，已拒绝恢复")?;
        anyhow::ensure!(task.version == 1, "写作任务版本不兼容，已拒绝恢复");
        task.adopt_legacy_records();
        let mut total = 0usize;
        for source in &mut task.sources {
            if source.text.is_empty() {
                let size: i64 = conn.query_row(
                    "SELECT length(CAST(source_text AS BLOB)) FROM staged_writing_sources WHERE manuscript_id=?1 AND source_id=?2 AND source_hash=?3",
                    params![manuscript_id,source.id,source.hash], |row|row.get(0),
                ).context("原文快照缺失或版本不符，已拒绝恢复")?;
                anyhow::ensure!(size >= 0 && size as u64 <= MAX_SOURCE_BYTES as u64,"原文快照超过安全上限");
                total = total.saturating_add(size as usize);
                anyhow::ensure!(total <= MAX_TOTAL_SOURCE_BYTES,"原文快照总量超过安全上限");
                source.text = conn.query_row("SELECT source_text FROM staged_writing_sources WHERE manuscript_id=?1 AND source_id=?2",params![manuscript_id,source.id],|row|row.get(0))?;
            } else {
                total = total.saturating_add(source.text.len());
                anyhow::ensure!(source.text.len() <= MAX_SOURCE_BYTES && total <= MAX_TOTAL_SOURCE_BYTES,"原文快照超过安全上限");
            }
            anyhow::ensure!(super::digest(source.text.as_bytes()) == source.hash,"原文快照校验值不符，已拒绝恢复");
        }
        Ok(task)
    })
    .transpose()
}

pub(crate) fn save_at(path: &Path, manuscript_id: i64, task: &WritingTask) -> Result<()> {
    anyhow::ensure!(task.version == 1, "不能保存不兼容版本的写作任务");
    let mut metadata = task.clone();
    for source in &mut metadata.sources {
        source.text.clear();
    }
    let raw = serde_json::to_string(&metadata)?;
    anyhow::ensure!(
        raw.len() <= MAX_TASK_JSON_BYTES,
        "写作任务超过 96 MiB 存储上限；请减少材料后重试"
    );
    let mut conn = connect(path)?;
    let tx = conn.transaction()?;
    let mut total = 0usize;
    for source in &task.sources {
        total = total.saturating_add(source.text.len());
        anyhow::ensure!(
            source.text.len() <= MAX_SOURCE_BYTES && total <= MAX_TOTAL_SOURCE_BYTES,
            "原文材料超过保存上限"
        );
        let existing: Option<String> = tx.query_row("SELECT source_hash FROM staged_writing_sources WHERE manuscript_id=?1 AND source_id=?2",params![manuscript_id,source.id],|row|row.get(0)).optional()?;
        if let Some(hash) = existing {
            anyhow::ensure!(
                hash == source.hash,
                "原文快照发生变化，请移除后重新添加材料"
            );
        } else {
            tx.execute("INSERT INTO staged_writing_sources(manuscript_id,source_id,source_hash,source_text) VALUES(?1,?2,?3,?4)",params![manuscript_id,source.id,source.hash,source.text])?;
        }
    }
    tx.execute(
        "INSERT INTO staged_writing_tasks(manuscript_id,task_json,updated_at) VALUES(?1,?2,datetime('now')) ON CONFLICT(manuscript_id) DO UPDATE SET task_json=excluded.task_json,updated_at=excluded.updated_at",
        params![manuscript_id, raw],
    ).context("保存分阶段写作任务失败")?;
    let obsolete = {
        let mut query =
            tx.prepare("SELECT source_id FROM staged_writing_sources WHERE manuscript_id=?1")?;
        query
            .query_map([manuscript_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for source in obsolete {
        if !task.sources.iter().any(|item| item.id == source) {
            tx.execute(
                "DELETE FROM staged_writing_sources WHERE manuscript_id=?1 AND source_id=?2",
                params![manuscript_id, source],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn connect(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("创建稿件库目录失败")?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("打开稿件库失败：{}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS staged_writing_tasks (
          manuscript_id INTEGER PRIMARY KEY REFERENCES manuscripts(id) ON DELETE CASCADE,
          task_json TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS staged_writing_sources (
          manuscript_id INTEGER NOT NULL REFERENCES manuscripts(id) ON DELETE CASCADE,
          source_id TEXT NOT NULL,
          source_hash TEXT NOT NULL,
          source_text TEXT NOT NULL,
          PRIMARY KEY(manuscript_id,source_id)
        );",
    )?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::writing::{SourceRole, SourceSnapshot};

    #[test]
    fn round_trip_is_atomic_and_cascades_with_manuscript() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; CREATE TABLE manuscripts(id INTEGER PRIMARY KEY);
          INSERT INTO manuscripts(id) VALUES(8);",
        )
        .unwrap();
        drop(conn);
        let mut task = WritingTask::default();
        task.sources
            .push(SourceSnapshot::new("材料", SourceRole::Content, "原文末尾").unwrap());
        save_at(&db, 8, &task).unwrap();
        assert_eq!(load_at(&db, 8).unwrap(), Some(task));
        let conn = Connection::open(&db).unwrap();
        let raw: String = conn
            .query_row(
                "SELECT task_json FROM staged_writing_tasks WHERE manuscript_id=8",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!raw.contains("原文末尾"), "不可在每个检查点重复写整份原文");
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute("DELETE FROM manuscripts WHERE id=8", [])
            .unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM staged_writing_tasks", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn refuses_missing_manuscript_and_unknown_version() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; CREATE TABLE manuscripts(id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        drop(conn);
        assert!(save_at(&db, 99, &WritingTask::default()).is_err());
    }
}
