//! 个人层修改记录：每次改 `tables.json` 之前把旧文件存一份，保留最近若干次，
//! 可以逐条回到某次修改之前的状态。基础表另有 `base.previous.txt`，不在这里。
use super::{data, table::Personal};
use serde::{Deserialize, Serialize};
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
    let mut records: Vec<Record> = std::fs::read(index_path(dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    records.reverse();
    records
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
    data::save_bytes(
        &history.join(format!("{id}.json")),
        &std::fs::read(&current)?,
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
    data::save_json(&index_path(dir), &records)
}

/// 读出某条记录保存的个人层。
pub(super) fn load(dir: &Path, id: &str) -> anyhow::Result<Personal> {
    anyhow::ensure!(
        id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "修改记录编号无效"
    );
    let bytes = std::fs::read(dir.join("history").join(format!("{id}.json")))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            data::save_json(&dir.path().join("tables.json"), &personal).unwrap();
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
