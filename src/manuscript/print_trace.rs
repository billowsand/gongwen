//! 印制追溯登记：与稿件一起保存在现有数据库，沿用 SQLCipher 与槽位隔离。
use super::ManuscriptStore;
use anyhow::Result;
use rusqlite::{Connection, params};

pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS print_trace_exports (
            code TEXT PRIMARY KEY,
            unit TEXT NOT NULL,
            created_at TEXT NOT NULL,
            title TEXT NOT NULL,
            source_hash TEXT NOT NULL,
            pdf_hash TEXT NOT NULL
        );",
    )
}

#[derive(Debug)]
pub(crate) struct TraceRecord {
    pub code: String,
    pub unit: String,
    pub created_at: String,
    pub title: String,
    pub source_hash: String,
    pub pdf_hash: String,
}

impl ManuscriptStore {
    pub(crate) fn register_print_trace(&self, record: &TraceRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO print_trace_exports VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.code,
                record.unit,
                record.created_at,
                record.title,
                record.source_hash,
                record.pdf_hash
            ],
        )?;
        Ok(())
    }

    /// 查询只按编号；空查询列出最近 20 次，避免在设置页载入整个历史。
    pub(crate) fn find_print_traces(&self, query: &str) -> Result<Vec<TraceRecord>> {
        let code: String = query.chars().filter(|c| c.is_ascii_digit()).collect();
        if !query.trim().is_empty() && code.len() != 16 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT code, unit, created_at, title, source_hash, pdf_hash
             FROM print_trace_exports WHERE (?1 = '' OR code = ?1)
             ORDER BY created_at DESC, rowid DESC LIMIT 20",
        )?;
        let records = stmt
            .query_map([code], |row| {
                Ok(TraceRecord {
                    code: row.get(0)?,
                    unit: row.get(1)?,
                    created_at: row.get(2)?,
                    title: row.get(3)?,
                    source_hash: row.get(4)?,
                    pdf_hash: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_trace_records_survive_reopening_and_reject_duplicate_codes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.db");
        let record = TraceRecord {
            code: "1234567890123456".into(),
            unit: "示例办公室".into(),
            created_at: "2026-11-06T10:00:00+08:00".into(),
            title: "样张".into(),
            source_hash: "source".into(),
            pdf_hash: "pdf".into(),
        };
        let store = ManuscriptStore::open(&path).unwrap();
        store.register_print_trace(&record).unwrap();
        assert!(store.register_print_trace(&record).is_err());
        drop(store);
        let store = ManuscriptStore::open(&path).unwrap();
        let found = store.find_print_traces("1234-5678-9012-3456").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].unit, record.unit);
        assert!(
            store
                .find_print_traces("0000000000000000")
                .unwrap()
                .is_empty()
        );
    }
}
