//! 公文登记簿：外单位来文、纸质件等不在本机稿件库里的文件，登记一次、各篇共用。
//!
//! 正文里的引用就是普通文字 `《名称》（文号）`（见 `document_reference`），登记簿只是
//! 比对来源：引用面板按文号与名称把正文里的引用和登记条目对上。更正登记时保留更正前的
//! 写法，引用了旧写法的文稿据此提示「来源已变」，由用户确认才改正文。
//! 登记簿暂不进离线同步 ZIP；对方机器上没有该条目时，引用只是显示为未登记。
use super::ManuscriptStore;
use anyhow::{Result, ensure};
use chrono::Local;
use rusqlite::{Connection, OptionalExtension, params};

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS document_registry (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT    NOT NULL,
    -- 规范后的完整发文字号；无文号时为空串
    number      TEXT    NOT NULL DEFAULT '',
    -- 现行有效 / 已废止 / 被替代；替代关系只存文字，不强制外键
    status          TEXT    NOT NULL DEFAULT 'active',
    replaced_by     TEXT    NOT NULL DEFAULT '',
    effective_date  TEXT    NOT NULL DEFAULT '',
    abolished_date  TEXT    NOT NULL DEFAULT '',
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);
-- 更正前的写法，供识别引用了旧写法的文稿
CREATE TABLE IF NOT EXISTS document_registry_aliases (
    registry_id INTEGER NOT NULL REFERENCES document_registry(id) ON DELETE CASCADE,
    title       TEXT    NOT NULL,
    number      TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (registry_id, title, number)
);
"#;

/// 一条登记。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegisteredDocument {
    pub id: i64,
    pub title: String,
    /// 规范后的发文字号；无文号为空。
    pub number: String,
    /// 更正前的写法（名称、文号）。
    pub aliases: Vec<(String, String)>,
    /// `active` / `abolished` / `replaced`。
    pub status: String,
    /// 被替代时填新文件（名称/文号），只作提示文字。
    pub replaced_by: String,
    pub effective_date: String,
    pub abolished_date: String,
}

impl RegisteredDocument {
    pub fn effectiveness_notice(&self) -> Option<String> {
        match self.status.as_str() {
            "abolished" => Some("该文件已废止".to_string()),
            "replaced" if self.replaced_by.trim().is_empty() => Some("该文件已被替代".to_string()),
            "replaced" => Some(format!("该文件已被“{}”替代", self.replaced_by.trim())),
            _ => None,
        }
    }
}

/// 幂等建表，与候选区一样不单开 `user_version` 档位。
pub(crate) fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL)?;
    ensure_columns(conn)
}

/// 旧库补列；新库在 DDL 里已有，这里是空操作。
fn ensure_columns(conn: &Connection) -> rusqlite::Result<()> {
    for (name, ddl) in [
        (
            "status",
            "ALTER TABLE document_registry ADD COLUMN status TEXT NOT NULL DEFAULT 'active'",
        ),
        (
            "replaced_by",
            "ALTER TABLE document_registry ADD COLUMN replaced_by TEXT NOT NULL DEFAULT ''",
        ),
        (
            "effective_date",
            "ALTER TABLE document_registry ADD COLUMN effective_date TEXT NOT NULL DEFAULT ''",
        ),
        (
            "abolished_date",
            "ALTER TABLE document_registry ADD COLUMN abolished_date TEXT NOT NULL DEFAULT ''",
        ),
    ] {
        if !column_exists(conn, "document_registry", name)? {
            conn.execute_batch(ddl)?;
        }
    }
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

impl ManuscriptStore {
    /// 全部登记，最近登记或更正的在前。
    pub fn list_registry(&self) -> Result<Vec<RegisteredDocument>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, number, status, replaced_by, effective_date, abolished_date
             FROM document_registry ORDER BY updated_at DESC, id DESC",
        )?;
        let mut documents = stmt
            .query_map([], |row| {
                Ok(RegisteredDocument {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    number: row.get(2)?,
                    aliases: Vec::new(),
                    status: row.get(3)?,
                    replaced_by: row.get(4)?,
                    effective_date: row.get(5)?,
                    abolished_date: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = self
            .conn
            .prepare("SELECT registry_id, title, number FROM document_registry_aliases")?;
        let aliases = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, title, number) in aliases {
            if let Some(document) = documents.iter_mut().find(|document| document.id == id) {
                document.aliases.push((title, number));
            }
        }
        Ok(documents)
    }

    pub fn get_registered(&self, id: i64) -> Result<Option<RegisteredDocument>> {
        Ok(self
            .list_registry()?
            .into_iter()
            .find(|document| document.id == id))
    }

    /// 登记一份文件。名称与文号都相同的条目已存在时直接复用，避免同一份文件登记多遍。
    pub fn register_document(
        &mut self,
        title: &str,
        number: &str,
        no_number: bool,
    ) -> Result<RegisteredDocument> {
        let (title, number) = crate::document_reference::validate(title, number, no_number)?;
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM document_registry WHERE title=?1 AND number=?2",
                params![title, number],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            // 登记行可能刚被删掉：取不到就按新登记往下走。
            if let Some(record) = self.get_registered(id)? {
                return Ok(record);
            }
        }
        let now = Local::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO document_registry (title, number, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)",
            params![title, number, now],
        )?;
        Ok(RegisteredDocument {
            id: self.conn.last_insert_rowid(),
            title,
            number,
            aliases: Vec::new(),
            status: "active".into(),
            replaced_by: String::new(),
            effective_date: String::new(),
            abolished_date: String::new(),
        })
    }

    /// 更正一条登记，原写法记为别名。已引用旧写法的文稿会在「本篇引用」里看到来源已变。
    pub fn update_registered(
        &mut self,
        id: i64,
        title: &str,
        number: &str,
        no_number: bool,
    ) -> Result<()> {
        let (title, number) = crate::document_reference::validate(title, number, no_number)?;
        let old = self.get_registered(id)?;
        let old = old.ok_or_else(|| anyhow::anyhow!("登记已不存在"))?;
        if old.title == title && old.number == number {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE document_registry SET title=?2, number=?3, updated_at=?4 WHERE id=?1",
            params![id, title, number, Local::now().to_rfc3339()],
        )?;
        ensure!(changed == 1, "登记已不存在");
        tx.execute(
            "INSERT OR IGNORE INTO document_registry_aliases (registry_id, title, number)
             VALUES (?1, ?2, ?3)",
            params![id, old.title, old.number],
        )?;
        // 改回某个旧写法时，它就不再是旧写法。
        tx.execute(
            "DELETE FROM document_registry_aliases WHERE registry_id=?1 AND title=?2 AND number=?3",
            params![id, title, number],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 只改制度状态与替代关系，不动名称/文号。
    pub fn update_registry_meta(
        &mut self,
        id: i64,
        status: &str,
        replaced_by: &str,
        effective_date: &str,
        abolished_date: &str,
    ) -> Result<()> {
        ensure!(
            matches!(status, "active" | "abolished" | "replaced"),
            "未知的制度状态"
        );
        let changed = self.conn.execute(
            "UPDATE document_registry
             SET status=?2, replaced_by=?3, effective_date=?4, abolished_date=?5, updated_at=?6
             WHERE id=?1",
            params![
                id,
                status,
                replaced_by.trim(),
                effective_date.trim(),
                abolished_date.trim(),
                Local::now().to_rfc3339(),
            ],
        )?;
        ensure!(changed == 1, "登记已不存在");
        Ok(())
    }

    /// 删除登记。正文里的引用文字不受影响，只是不再能核对来源。
    pub fn delete_registered(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM document_registry WHERE id=?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn registry_status_and_replacement_round_trip() {
        let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
        let document = store
            .register_document("关于做好防火工作的通知", "某应急〔2026〕8号", false)
            .unwrap();
        assert_eq!(document.status, "active");
        assert_eq!(document.effectiveness_notice(), None);

        store
            .update_registry_meta(
                document.id,
                "replaced",
                "关于做好消防工作的通知（某应急〔2026〕12号）",
                "2026-01-01",
                "2026-06-30",
            )
            .unwrap();
        let updated = store.get_registered(document.id).unwrap().unwrap();
        assert_eq!(updated.status, "replaced");
        assert_eq!(updated.effective_date, "2026-01-01");
        assert_eq!(updated.abolished_date, "2026-06-30");
        assert_eq!(
            updated.effectiveness_notice().as_deref(),
            Some("该文件已被“关于做好消防工作的通知（某应急〔2026〕12号）”替代")
        );
        assert!(
            store
                .update_registry_meta(document.id, "nope", "", "", "")
                .is_err()
        );
    }

    #[test]
    fn registry_dedups_keeps_aliases_and_deletes() {
        let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
        let first = store
            .register_document("《关于做好防火工作的通知》", "某应急[2026]8号", false)
            .unwrap();
        assert_eq!(first.title, "关于做好防火工作的通知");
        assert_eq!(first.number, "某应急〔2026〕8号");
        let again = store
            .register_document("关于做好防火工作的通知", "某应急〔2026〕8号", false)
            .unwrap();
        assert_eq!(again, first);
        let no_number = store
            .register_document("关于做好防火工作的通知", "随便", true)
            .unwrap();
        assert_ne!(no_number.id, first.id);
        assert!(no_number.number.is_empty());
        assert!(store.register_document("某函", "第十二号", false).is_err());
        assert_eq!(store.list_registry().unwrap().len(), 2);

        store
            .update_registered(
                first.id,
                "关于做好秋季防火工作的通知",
                "某应急2026 9",
                false,
            )
            .unwrap();
        let found = store.get_registered(first.id).unwrap().unwrap();
        assert_eq!(found.title, "关于做好秋季防火工作的通知");
        assert_eq!(found.number, "某应急〔2026〕9号");
        assert_eq!(
            found.aliases,
            [(
                "关于做好防火工作的通知".to_owned(),
                "某应急〔2026〕8号".to_owned()
            )]
        );
        // 改回旧写法，别名随之消失、换成刚才那一版。
        store
            .update_registered(
                first.id,
                "关于做好防火工作的通知",
                "某应急〔2026〕8号",
                false,
            )
            .unwrap();
        let found = store.get_registered(first.id).unwrap().unwrap();
        assert_eq!(found.aliases.len(), 1);
        assert_eq!(found.aliases[0].1, "某应急〔2026〕9号");
        assert!(
            store
                .update_registered(first.id, "", "某应急〔2026〕9号", false)
                .is_err()
        );

        store.delete_registered(first.id).unwrap();
        assert!(store.get_registered(first.id).unwrap().is_none());
        assert!(store.get_registered(no_number.id).unwrap().is_some());
    }
}
