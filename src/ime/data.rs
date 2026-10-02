//! 基础词表与个人词表的本机存储。首次载入兼容旧音形表路径。
use anyhow::Context as _;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub(super) fn directory() -> anyhow::Result<PathBuf> {
    let dir = crate::storage::config_dir()?.join("ime");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 同目录临时文件替换，避免失败时把已有词表截断。
pub(super) fn save_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().context("词表路径没有父目录")?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write as _;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
pub(super) fn save_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    save_bytes(path, &serde_json::to_vec_pretty(value)?)
}

pub(super) fn load_base() -> anyhow::Result<Vec<super::table::Entry>> {
    let dir = directory()?;
    let target = dir.join("base.txt");
    let mut paths = vec![target.clone()];
    if let Some(root) = std::env::var_os("GONGWEN_RUNTIME_DIR") {
        paths.push(PathBuf::from(root).join("ime/base.txt"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        paths.push(parent.join("runtime/ime/base.txt"));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fuma/quan.txt"));
    paths.push(dir.join("yinxing/flypy-yinxing.txt"));
    for path in paths {
        if path.is_file() {
            let parsed = super::table::parse(&crate::text_file::read_to_string(&path)?, false);
            anyhow::ensure!(
                !parsed.entries.is_empty(),
                "基础词表没有有效记录：{}",
                path.display()
            );
            if path != target {
                save_base(&parsed.entries)?;
            }
            return Ok(parsed.entries);
        }
    }
    anyhow::bail!("请导入基础词表")
}
pub(super) fn save_base(entries: &[super::table::Entry]) -> anyhow::Result<()> {
    let mut text = String::from("\u{feff}# 词表输入法基础表\n");
    let mut positions = std::collections::BTreeMap::<&str, usize>::new();
    for entry in entries {
        let position = positions.entry(&entry.code).or_default();
        *position += 1;
        text.push_str(&format!("{},{}={}\n", entry.code, position, entry.text));
    }
    save_bytes(&directory()?.join("base.txt"), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_replace_and_failed_save_preserve_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("词表.txt");
        save_bytes(&path, "旧表".as_bytes()).unwrap();
        save_bytes(&path, "新表".as_bytes()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "新表");
        let occupied = dir.path().join("目录");
        std::fs::create_dir(&occupied).unwrap();
        assert!(save_bytes(&occupied, b"failed").is_err());
        assert!(occupied.is_dir());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "新表");
    }
}
