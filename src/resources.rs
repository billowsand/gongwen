//! 资源对象库：图片、PDF 插图与 Mermaid 图表缓存以 BLOB 存在稿件主库里。
//!
//! 为什么放库里而不是一图一文件：文件名的原名、格式与导入时间会留在目录列表里，
//! 即使内容套了 AEAD 也等于把「这是什么图」写在磁盘上。放进 SQLCipher 主库后，
//! 文件名、格式、内容、时间全部在密文里；写入与正文/版本同一个事务，崩溃恢复
//! 也只剩一套状态。PDF 扫描件附件早就是按这个办法存的（`pdf_attachments`），
//! 这里把图片与图表缓存并到同一条路上。
//!
//! 表结构（幂等建表，不占 `user_version` 档位，避免与知识库/词表/任务助手
//! 各自那套迁移互相干扰）：
//!
//! - `resource_objects`：按明文 SHA-256 去重的内容对象（同一张图在多篇稿件、
//!   多个版本里只存一份）；
//! - `resource_aliases`：正文里的逻辑引用 → 对象。新导入写随机 ID
//!   （`images/<uuid>`）；旧数据的 `images/<原文件名>` 也作为别名迁移进来。
//!   别名另存原始文件名，导出 DOCX / 稿件包时给人类看。
//!
//! 读取顺序由 [`crate::images`] 决定：先查别名，没有别名再回退到磁盘文件
//! （帮助截图、未迁移的旧数据、外部基准目录）。写入只在应用初始化过资源库
//! （[`init`]）时进库；测试夹具或库打不开时由调用方回退到旧的文件写法。

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
#[cfg(test)]
use std::cell::RefCell;
use std::path::Path;
#[cfg(not(test))]
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::Mutex;
use std::time::Duration;

/// 建表语句。`IF NOT EXISTS` 幂等，每次打开稿件库都跑一遍。
pub(crate) const RESOURCE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS resource_objects (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    sha256        TEXT    NOT NULL UNIQUE,
    media_type    TEXT    NOT NULL,
    original_name TEXT    NOT NULL,
    plain_size    INTEGER NOT NULL,
    created_at    TEXT    NOT NULL,
    bytes         BLOB    NOT NULL
);
CREATE TABLE IF NOT EXISTS resource_aliases (
    logical_path  TEXT    PRIMARY KEY,
    object_id     INTEGER NOT NULL REFERENCES resource_objects(id) ON DELETE CASCADE,
    original_name TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_resource_aliases_object
    ON resource_aliases(object_id);
"#;

/// 可识别的资源格式。返回值同时给出 MIME 与导出/稿件包使用的扩展名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Media {
    pub(crate) media_type: &'static str,
    pub(crate) ext: &'static str,
}

/// 按文件头判断资源格式（不信任扩展名，新引用本来也没有扩展名）。
pub(crate) fn sniff(bytes: &[u8]) -> Option<Media> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const BMP: Media = Media {
        media_type: "image/bmp",
        ext: "bmp",
    };
    if bytes.starts_with(PNG) {
        return Some(Media {
            media_type: "image/png",
            ext: "png",
        });
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some(Media {
            media_type: "image/jpeg",
            ext: "jpg",
        });
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(Media {
            media_type: "image/gif",
            ext: "gif",
        });
    }
    if bytes.starts_with(b"BM") {
        return Some(BMP);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(Media {
            media_type: "image/webp",
            ext: "webp",
        });
    }
    if bytes.starts_with(b"%PDF-") {
        return Some(Media {
            media_type: "application/pdf",
            ext: "pdf",
        });
    }
    None
}

/// 逻辑引用对应的对象元数据（不含内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceMeta {
    pub(crate) media_type: String,
    pub(crate) original_name: String,
    pub(crate) sha256: String,
    pub(crate) plain_size: u64,
}

/// 建表（幂等）。稿件库每次打开、资源库连接初始化时都调一次。
pub(crate) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(RESOURCE_DDL)
        .context("创建资源对象表失败")?;
    Ok(())
}

/// 把一份内容与一条逻辑引用写进资源库（调用方负责事务）。
///
/// - 按明文 SHA-256 去重：同一份内容只留一个 `resource_objects` 行；
/// - `media_override` 给 Mermaid 警告这类文本内容用，图片/PDF 填 `None` 走魔数；
/// - 同一个逻辑路径重复写会改指向新对象（迁移与导入的冲突检查在调用方）。
pub(crate) fn put_with(
    conn: &Connection,
    logical: &str,
    original_name: &str,
    media_override: Option<&str>,
    bytes: &[u8],
) -> Result<()> {
    anyhow::ensure!(!logical.is_empty(), "资源逻辑路径为空");
    let media_type = match media_override {
        Some(media) => media.to_string(),
        None => sniff(bytes)
            .with_context(|| format!("无法识别的资源格式：{logical}"))?
            .media_type
            .to_string(),
    };
    let sha256 = crate::manuscript::sync::bytes_hash(bytes);
    let created_at = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO resource_objects
             (sha256, media_type, original_name, plain_size, created_at, bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            sha256,
            media_type,
            original_name,
            bytes.len() as i64,
            created_at,
            bytes
        ],
    )
    .with_context(|| format!("写入资源对象失败：{logical}"))?;
    let object_id: i64 = conn
        .query_row(
            "SELECT id FROM resource_objects WHERE sha256=?1",
            [&sha256],
            |row| row.get(0),
        )
        .with_context(|| format!("读取资源对象失败：{logical}"))?;
    conn.execute(
        "INSERT INTO resource_aliases (logical_path, object_id, original_name)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(logical_path) DO UPDATE SET
             object_id=excluded.object_id, original_name=excluded.original_name",
        params![logical, object_id, original_name],
    )
    .with_context(|| format!("写入资源引用失败：{logical}"))?;
    Ok(())
}

/// 按逻辑引用读内容；没有这条引用返回 `Ok(None)`。
pub(crate) fn get_with(conn: &Connection, logical: &str) -> Result<Option<Vec<u8>>> {
    conn.query_row(
        "SELECT o.bytes FROM resource_aliases a
           JOIN resource_objects o ON o.id = a.object_id
          WHERE a.logical_path=?1",
        [logical],
        |row| row.get(0),
    )
    .optional()
    .with_context(|| format!("读取资源失败：{logical}"))
}

/// 按逻辑引用读元数据；没有这条引用返回 `Ok(None)`。
pub(crate) fn meta_with(conn: &Connection, logical: &str) -> Result<Option<ResourceMeta>> {
    conn.query_row(
        "SELECT o.media_type, a.original_name, o.sha256, o.plain_size
           FROM resource_aliases a
           JOIN resource_objects o ON o.id = a.object_id
          WHERE a.logical_path=?1",
        [logical],
        |row| {
            Ok(ResourceMeta {
                media_type: row.get(0)?,
                original_name: row.get(1)?,
                sha256: row.get(2)?,
                plain_size: row.get::<_, i64>(3)?.max(0) as u64,
            })
        },
    )
    .optional()
    .with_context(|| format!("读取资源元数据失败：{logical}"))
}

/// 这条逻辑引用是否已指向一个对象。
pub(crate) fn has_with(conn: &Connection, logical: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM resource_aliases WHERE logical_path=?1",
            [logical],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// 删除一条逻辑引用；对象本身留给 [`gc_objects_with`] 回收。
pub(crate) fn remove_alias_with(conn: &Connection, logical: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM resource_aliases WHERE logical_path=?1",
        [logical],
    )?;
    Ok(())
}

/// 回收没有任何逻辑引用指向的对象。返回删除的对象数。
pub(crate) fn gc_objects_with(conn: &Connection) -> Result<usize> {
    let removed = conn.execute(
        "DELETE FROM resource_objects
          WHERE id NOT IN (SELECT object_id FROM resource_aliases)",
        [],
    )?;
    Ok(removed)
}

/// 迁移/清理用：列出全部逻辑引用。只给后台维护逻辑用，不参与正常读取。
#[cfg(test)]
pub(crate) fn all_aliases_with(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT logical_path FROM resource_aliases ORDER BY logical_path")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 当前进程的资源库连接。`config_dir` / `slot` 用来拒绝测试或切槽后的误用：
/// 与当前配置目录或槽位不符时一律当作「资源库不可用」，让读取回退到文件路径。
struct Global {
    conn: Connection,
    #[cfg(not(test))]
    config_dir: PathBuf,
    #[cfg(not(test))]
    slot: crate::storage::DataSlot,
}

/// 生产构建是进程级单例：界面线程与后台线程共用同一个 SQLCipher 连接。
#[cfg(not(test))]
static GLOBAL_MUTEX: Mutex<Option<Global>> = Mutex::new(None);

// 测试构建改成线程局部：测试并行跑时各用各的临时库，互不覆盖。
// 生产语义（进程级、锁保护）不变。
#[cfg(test)]
thread_local! {
    static GLOBAL_TEST: RefCell<Option<Global>> = const { RefCell::new(None) };
}

#[cfg(not(test))]
fn with_global<R>(f: impl FnOnce(&mut Option<Global>) -> R) -> R {
    let mut guard = GLOBAL_MUTEX
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    f(&mut guard)
}

#[cfg(test)]
fn with_global<R>(f: impl FnOnce(&mut Option<Global>) -> R) -> R {
    GLOBAL_TEST.with(|cell| f(&mut cell.borrow_mut()))
}

/// 按当前槽位打开资源库。解锁进入工作区时调用；失败时图片/图表仍按文件读写。
pub(crate) fn init() -> Result<()> {
    let path = crate::storage::manuscript_db_path()?;
    init_at(&path)
}

/// [`init`] 的显式路径入口，供测试与显式数据库路径复用。
pub(crate) fn init_at(path: &Path) -> Result<()> {
    let conn =
        Connection::open(path).with_context(|| format!("无法打开资源库 {}", path.display()))?;
    crate::storage::apply_db_key(&conn)?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .context("设置资源库参数失败")?;
    ensure_schema(&conn)?;
    with_global(|slot| {
        #[cfg(test)]
        {
            let _ = slot;
            *slot = Some(Global { conn });
        }
        #[cfg(not(test))]
        {
            *slot = Some(Global {
                conn,
                config_dir: crate::storage::config_dir()?,
                slot: crate::storage::current_slot(),
            });
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

/// 锁定、切槽或重建数据库之前关掉连接；在打开的写事务之前调用一次。
pub(crate) fn close() {
    with_global(|slot| *slot = None);
}

/// 资源库是否已打开，且（生产构建）仍对应当前配置目录与槽位。
pub(crate) fn available() -> bool {
    #[cfg(test)]
    {
        with_global(|slot| slot.is_some())
    }
    #[cfg(not(test))]
    {
        let Ok(config_dir) = crate::storage::config_dir() else {
            return false;
        };
        let slot_now = crate::storage::current_slot();
        with_global(|slot| {
            slot.as_ref()
                .is_some_and(|global| global.config_dir == config_dir && global.slot == slot_now)
        })
    }
}

fn with_current<T>(f: impl FnOnce(&Connection) -> Result<T>) -> Result<Option<T>> {
    #[cfg(test)]
    {
        // 测试构建：线程局部的库就是当前库，不再比对目录/槽位。
        with_global(|slot| {
            let Some(global) = slot.as_ref() else {
                return Ok(None);
            };
            f(&global.conn).map(Some)
        })
    }
    #[cfg(not(test))]
    {
        let Ok(config_dir) = crate::storage::config_dir() else {
            return Ok(None);
        };
        let slot_now = crate::storage::current_slot();
        with_global(|slot| {
            let Some(global) = slot.as_ref() else {
                return Ok(None);
            };
            if global.config_dir != config_dir || global.slot != slot_now {
                return Ok(None);
            }
            f(&global.conn).map(Some)
        })
    }
}

/// 全局读：资源库不可用或没有这条引用时都返回 `Ok(None)`，交给文件回退。
pub(crate) fn get(logical: &str) -> Result<Option<Vec<u8>>> {
    let Some(found) = with_current(|conn| get_with(conn, logical))? else {
        return Ok(None);
    };
    Ok(found)
}

/// 全局元数据查询，语义同 [`get`]。
pub(crate) fn meta(logical: &str) -> Result<Option<ResourceMeta>> {
    let Some(found) = with_current(|conn| meta_with(conn, logical))? else {
        return Ok(None);
    };
    Ok(found)
}

/// 全局存在性查询。
pub(crate) fn has(logical: &str) -> Result<bool> {
    Ok(with_current(|conn| has_with(conn, logical))?.unwrap_or(false))
}

/// 全局写：对象 + 别名在一个事务里。资源库不可用时返回 `Ok(false)`，
/// 由调用方回退到旧的文件写法（测试夹具、库打不开的降级路径）。
pub(crate) fn put(
    logical: &str,
    original_name: &str,
    media_override: Option<&str>,
    bytes: &[u8],
) -> Result<bool> {
    #[cfg(not(test))]
    let (expected_dir, expected_slot) = {
        let Ok(config_dir) = crate::storage::config_dir() else {
            return Ok(false);
        };
        (config_dir, crate::storage::current_slot())
    };
    with_global(|slot| {
        let Some(global) = slot.as_ref() else {
            return Ok(false);
        };
        #[cfg(not(test))]
        if global.config_dir != expected_dir || global.slot != expected_slot {
            return Ok(false);
        }
        global
            .conn
            .execute_batch("BEGIN IMMEDIATE")
            .context("开始资源事务失败")?;
        let result = put_with(&global.conn, logical, original_name, media_override, bytes);
        match result {
            Ok(()) => {
                global.conn.execute_batch("COMMIT")?;
                Ok(true)
            }
            Err(error) => {
                let _ = global.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    })
}

/// 把 WAL 里的已提交内容并回主库文件。
///
/// 迁移旧图片时先入库、确认落盘、再删原文件：WAL 模式下仅提交还不够，
/// 断电可能丢掉尚未 checkpoint 的帧，那时原文件已经被删就是唯一副本丢失。
pub(crate) fn checkpoint() -> Result<()> {
    with_current(|conn| {
        conn.execute_batch("PRAGMA wal_checkpoint(FULL)")
            .context("资源库 checkpoint 失败")?;
        Ok(())
    })?;
    Ok(())
}

/// 把某一前缀下的逻辑引用裁到最近 `keep` 条（按写入顺序），并回收无引用对象。
///
/// 只给可重建缓存用（Mermaid 图表）：缓存进资源库后不再有「一图一文件」的自然
/// 上限，长期使用会让主库无限增长；超过上限时按新近程度保留，旧缓存下次用到时
/// 重新渲染。返回删除的引用数。
pub(crate) fn prune_aliases(prefix: &str, keep: usize) -> Result<usize> {
    Ok(with_current(|conn| prune_aliases_with(conn, prefix, keep))?.unwrap_or(0))
}

/// [`prune_aliases`] 的连接版；稿件库自己的连接也能调。
pub(crate) fn prune_aliases_with(conn: &Connection, prefix: &str, keep: usize) -> Result<usize> {
    if prefix.is_empty() {
        return Ok(0);
    }
    let pattern = format!("{prefix}%");
    let removed = conn
        .execute(
            "DELETE FROM resource_aliases WHERE logical_path IN (
                 SELECT logical_path FROM resource_aliases
                  WHERE logical_path LIKE ?1
                  ORDER BY rowid DESC
                  LIMIT -1 OFFSET ?2)",
            params![pattern, keep as i64],
        )
        .context("裁剪缓存引用失败")?;
    if removed > 0 {
        gc_objects_with(conn)?;
    }
    Ok(removed)
}

/// 全局删除一条引用（孤儿资源清理用）。
pub(crate) fn remove_alias(logical: &str) -> Result<()> {
    with_current(|conn| remove_alias_with(conn, logical))?;
    Ok(())
}

/// 全局回收无引用对象。
pub(crate) fn gc_objects() -> Result<usize> {
    Ok(with_current(gc_objects_with)?.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn objects_are_deduped_and_aliases_resolve_to_bytes() {
        let conn = conn();
        let png = b"\x89PNG\r\n\x1a\nrest";
        put_with(&conn, "images/a", "扫描件.png", None, png).unwrap();
        put_with(&conn, "images/b", "另一份.png", None, png).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM resource_objects", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "同一份内容只存一个对象"
        );
        assert_eq!(
            get_with(&conn, "images/a").unwrap().as_deref(),
            Some(&png[..])
        );
        assert_eq!(
            get_with(&conn, "images/b").unwrap().as_deref(),
            Some(&png[..])
        );
        let meta = meta_with(&conn, "images/a").unwrap().unwrap();
        assert_eq!(meta.media_type, "image/png");
        assert_eq!(meta.original_name, "扫描件.png");
        assert_eq!(meta.plain_size, png.len() as u64);
        assert!(get_with(&conn, "images/missing").unwrap().is_none());
        assert!(!has_with(&conn, "images/missing").unwrap());
    }

    #[test]
    fn gc_drops_unreferenced_objects_only() {
        let conn = conn();
        put_with(&conn, "images/a", "a.png", None, b"\x89PNG\r\n\x1a\na").unwrap();
        put_with(&conn, "images/b", "b.png", None, b"\x89PNG\r\n\x1a\nb").unwrap();
        remove_alias_with(&conn, "images/a").unwrap();
        assert_eq!(gc_objects_with(&conn).unwrap(), 1);
        assert_eq!(
            get_with(&conn, "images/b").unwrap().as_deref(),
            Some(b"\x89PNG\r\n\x1a\nb".as_slice())
        );
        assert!(get_with(&conn, "images/a").unwrap().is_none());
    }

    #[test]
    fn cache_prefixes_are_pruned_to_the_newest_entries() {
        let conn = conn();
        for index in 0u8..6 {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            bytes.push(index);
            let logical = format!("mermaid-cache/{index}");
            put_with(&conn, &logical, "图表", Some("image/png"), &bytes).unwrap();
        }
        put_with(
            &conn,
            "images/keep",
            "a.png",
            None,
            b"\x89PNG\r\n\x1a\nkeep",
        )
        .unwrap();
        assert_eq!(prune_aliases_with(&conn, "mermaid-cache/", 2).unwrap(), 4);
        let left = all_aliases_with(&conn).unwrap();
        assert_eq!(
            left,
            vec![
                "images/keep".to_string(),
                "mermaid-cache/4".to_string(),
                "mermaid-cache/5".to_string()
            ]
        );
        // 被裁掉的缓存对象没有引用，一并回收；非缓存对象不受影响。
        assert_eq!(
            get_with(&conn, "images/keep").unwrap().as_deref(),
            Some(b"\x89PNG\r\n\x1a\nkeep".as_slice())
        );
        assert!(get_with(&conn, "mermaid-cache/0").unwrap().is_none());
    }

    #[test]
    fn text_override_is_stored_without_magic_bytes() {
        let conn = conn();
        put_with(
            &conn,
            "mermaid-cache/x.warning",
            "图表警告",
            Some("text/plain"),
            "图表过大".as_bytes(),
        )
        .unwrap();
        assert_eq!(
            meta_with(&conn, "mermaid-cache/x.warning")
                .unwrap()
                .unwrap()
                .media_type,
            "text/plain"
        );
    }

    #[test]
    fn unknown_binary_without_override_is_rejected() {
        let conn = conn();
        assert!(
            put_with(&conn, "images/x", "x", None, b"not an image").is_err(),
            "认不出格式的内容不能进资源库"
        );
        assert!(get_with(&conn, "images/x").unwrap().is_none());
    }
}
