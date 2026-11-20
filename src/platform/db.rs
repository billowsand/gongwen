//! 一个开库口、一张迁移表。
//!
//! 所有打开 `manuscripts.db` 的路径都从这里走：目录、SQLCipher 密钥、WAL、
//! `busy_timeout`、外键只有一份。后台线程各开各的连接（`rusqlite::Connection`
//! 不能跨线程共享），但入口相同。
//!
//! 现有 `ManuscriptStore::migrate` 的旧迁移先不动；新表从 F4/F5 起登记到
//! `Migration` 列表，版本只增不减。

use crate::storage;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs;
use std::path::Path;
use std::time::Duration;

pub(crate) struct Db {
    conn: Connection,
}

impl Db {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).context("创建数据库目录失败")?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("无法打开数据库 {}", path.display()))?;
        storage::apply_db_key(&conn)?;
        conn.busy_timeout(Duration::from_millis(2000))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        Ok(Self { conn })
    }

    pub(crate) fn into_connection(self) -> Connection {
        self.conn
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn transaction<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.transaction()?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

/// 一条迁移：版本号、名字、执行函数。执行函数必须幂等或只在 `version` 未到
/// 时跑一次；失败整条回滚，`user_version` 不前进。
pub(crate) struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub apply: fn(&Connection) -> Result<()>,
}

/// 按顺序执行未应用的迁移。已到达的版本跳过，重复调用是空操作。
pub(crate) fn apply_migrations(conn: &Connection, migrations: &[Migration]) -> Result<()> {
    let mut current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for migration in migrations {
        if migration.version <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        (migration.apply)(&tx)?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
        current = migration.version;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_table(conn: &Connection) -> Result<()> {
        conn.execute_batch("CREATE TABLE t (x INTEGER)")?;
        Ok(())
    }

    fn add_column(conn: &Connection) -> Result<()> {
        conn.execute_batch("ALTER TABLE t ADD COLUMN y TEXT")?;
        Ok(())
    }

    #[test]
    fn migrations_apply_once_and_record_the_version() {
        let conn = Connection::open_in_memory().unwrap();
        let migrations = [
            Migration {
                version: 1,
                name: "建表",
                apply: create_table,
            },
            Migration {
                version: 2,
                name: "补列",
                apply: add_column,
            },
        ];
        apply_migrations(&conn, &migrations).unwrap();
        apply_migrations(&conn, &migrations).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
        let columns: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('t')", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(columns, 2);
    }
}
