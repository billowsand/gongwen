//! 个人层修改记录：每次改 `tables.json` 之前把旧文件存一份，保留最近若干次，
//! 可以逐条回到某次修改之前的状态。基础表另有 `base.previous.txt`，不在这里。
use super::{data, table::Personal};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// 最多保留几次。
pub(super) const KEEP: usize = 20;

/// 一次修改：回到这里就是回到这次修改之前。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    pub id: String,
    pub time: String,
    pub label: String,
}

fn index_path(dir: &Path) -> std::path::PathBuf {
    dir.join("history").join("index.json")
}

/// 最近的修改记录，新的在前。读不到就当没有。
pub(super) fn list(dir: &Path) -> Vec<Record> {
    let mut records: Vec<Record> = read_settled(&index_path(dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    records.reverse();
    records
}

/// 读一个小文件，遇到「拒绝访问」重试几次。
///
/// Windows 上杀毒软件与索引服务会在文件刚落盘时短暂占住它（移动磁盘、沙箱里更明显），
/// 这时 `fs::read` 报 `os error 5`。修改记录是刚写完就要读回的，一次共享冲突就把
/// 快照读失败、甚至把索引读成空表，只能是偶发故障，所以重试几百毫秒。
fn read_settled(path: &Path) -> anyhow::Result<Vec<u8>> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..6u32 {
        match data::load_internal_bytes(path) {
            Ok(bytes) => return Ok(bytes),
            Err(error)
                if error
                    .root_cause()
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(
                    u64::from(attempt) * 20 + 10,
                ));
                last = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("循环至少跑过一次"))
}

/// 把当前 `tables.json` 存为一条记录。没有当前文件时什么也不做。
pub(super) fn record(dir: &Path, label: &str) -> anyhow::Result<()> {
    let current = dir.join("tables.json");
    if !current.is_file() {
        return Ok(());
    }
    let history = dir.join("history");
    std::fs::create_dir_all(&history)?;
    let now = chrono::Local::now();
    let id = format!(
        "{}-{}",
        now.format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    );
    data::save_internal_bytes(
        &history.join(format!("{id}.json")),
        &data::load_internal_bytes(&current)?,
    )?;
    let mut records = list(dir);
    records.reverse();
    records.push(Record {
        id,
        time: now.format("%m-%d %H:%M:%S").to_string(),
        label: label.into(),
    });
    let excess = records.len().saturating_sub(KEEP);
    for old in records.drain(..excess) {
        let _ = std::fs::remove_file(history.join(format!("{}.json", old.id)));
    }
    data::save_internal_json(&index_path(dir), &records)
}

/// 读出某条记录保存的个人层。
pub(super) fn load(dir: &Path, id: &str) -> anyhow::Result<Personal> {
    anyhow::ensure!(
        id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "修改记录编号无效"
    );
    let bytes = read_settled(&dir.join("history").join(format!("{id}.json")))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_history_stores_plaintext_once_and_can_be_restored() {
        let root = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(root.path().to_path_buf()));
        crate::vault::set_key_handle([42; 32], true);
        let dir = data::directory().unwrap();
        let first = Personal {
            entries: vec![super::super::table::Entry {
                code: "abcd".into(),
                text: "第一版".into(),
            }],
            ..Default::default()
        };
        data::save_internal_json(&dir.join("tables.json"), &first).unwrap();
        record(&dir, "加密前快照").unwrap();
        let second = Personal {
            entries: vec![super::super::table::Entry {
                code: "abcd".into(),
                text: "第二版".into(),
            }],
            ..Default::default()
        };
        data::save_internal_json(&dir.join("tables.json"), &second).unwrap();
        record(&dir, "再次修改").unwrap();
        let records = list(&dir);
        let historical = load(&dir, &records[1].id).unwrap();
        assert_eq!(historical.entries[0].text, "第一版");
        let snapshot = dir.join("history").join(format!("{}.json", records[1].id));
        assert!(std::fs::read(snapshot).unwrap().starts_with(b"GWA2"));
        crate::vault::clear_key_handle();
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn keeps_latest_records_and_loads_them_back() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), "无文件").unwrap();
        assert!(list(dir.path()).is_empty());
        for round in 0..KEEP + 3 {
            let personal = Personal {
                entries: vec![super::super::table::Entry {
                    code: "abcd".into(),
                    text: format!("第{round}次"),
                }],
                ..Default::default()
            };
            data::save_internal_json(&dir.path().join("tables.json"), &personal).unwrap();
            record(dir.path(), &format!("修改{round}")).unwrap();
        }
        let records = list(dir.path());
        assert_eq!(records.len(), KEEP);
        assert_eq!(records[0].label, format!("修改{}", KEEP + 2));
        let newest = load(dir.path(), &records[0].id).unwrap();
        assert_eq!(newest.entries[0].text, format!("第{}次", KEEP + 2));
        let files = std::fs::read_dir(dir.path().join("history"))
            .unwrap()
            .count();
        assert_eq!(files, KEEP + 1);
        assert!(load(dir.path(), "../tables").is_err());
    }
}
