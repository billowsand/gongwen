//! 私有虚拟文件树：名称与内容一起密封到中性文件，真实目录仅作路径边界。
use anyhow::{Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
pub(crate) type Tree = BTreeMap<String, String>;
pub(crate) const MAX_TREE_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 15) as usize] as char);
    }
    text
}

pub(crate) fn write(path: &Path, tree: &Tree) -> Result<()> {
    let bytes = serde_json::to_vec(tree)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_TREE_BYTES,
        "加密文件树超过大小上限"
    );
    crate::vault::write_path(path, "pi", &bytes)
}

fn roots(slot: crate::storage::DataSlot) -> Result<Vec<(PathBuf, PathBuf)>> {
    let base = crate::storage::slot_subdir_for(slot, "pi")?.join("workspaces");
    let mut pending = vec![base];
    let mut found = Vec::new();
    let mut count = 0;
    while let Some(dir) = pending.pop() {
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            count += 1;
            anyhow::ensure!(count < 200_000, "Pi 文件树过大");
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            if name == "work" {
                found.push((path, dir.join("work-files.json")));
            } else if name == "pi-skills" && dir.file_name().is_some_and(|n| n == "skills") {
                let parent = dir.parent().context("技能目录缺少父目录")?;
                found.push((path, parent.join("skill-files.json")));
            } else {
                pending.push(path);
            }
        }
    }
    Ok(found)
}

pub(crate) fn pending(slot: crate::storage::DataSlot) -> bool {
    roots(slot).map_or(true, |roots| {
        roots.into_iter().any(|(root, _)| {
            std::fs::read_dir(root).map_or(true, |mut entries| entries.next().is_some())
        })
    })
}

pub(crate) fn migrate() -> Result<()> {
    for (root, bundle) in roots(crate::storage::current_slot())? {
        let mut tree: Tree = if bundle.exists() {
            serde_json::from_slice(&crate::vault::read_path(&bundle, "pi", MAX_TREE_BYTES)?)?
        } else {
            BTreeMap::new()
        };
        let mut pending = vec![root.clone()];
        let mut files = Vec::new();
        let mut total = 0;
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                anyhow::ensure!(
                    !entry.file_type()?.is_symlink(),
                    "旧 Pi 工作区含链接，请人工清理：{}",
                    path.display()
                );
                if entry.file_type()?.is_dir() {
                    pending.push(path);
                    continue;
                }
                let bytes = crate::vault::read_path(&path, "pi", 128 * 1024 * 1024)?;
                total += bytes.len() as u64;
                anyhow::ensure!(
                    total * 2 <= MAX_TREE_BYTES,
                    "旧 Pi 工作区过大，不能安全迁移"
                );
                let relative = path
                    .strip_prefix(&root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                let encoded = encode(&bytes);
                if let Some(previous) = tree.get(&relative) {
                    anyhow::ensure!(previous == &encoded, "旧文件与加密文件树冲突，已保留两份");
                } else {
                    tree.insert(relative, encoded);
                }
                files.push(path);
            }
        }
        if files.is_empty() {
            continue;
        }
        write(&bundle, &tree)?;
        for file in files {
            crate::vault::erase_file_checked(&file)?;
        }
        std::fs::remove_dir_all(&root)?;
        std::fs::create_dir_all(&root)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoy_name_migration_is_detected_before_switching_the_active_slot() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_owned()));
        let work = dir
            .path()
            .join("alt-pi/workspaces/doc-1/run-1/workspace/work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("旧业务标题.md"), b"GWA2 encrypted fixture").unwrap();
        assert!(pending(crate::storage::DataSlot::Decoy));
        assert!(!pending(crate::storage::DataSlot::Real));
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn legacy_names_and_content_move_into_the_same_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_owned()));
        crate::vault::set_key_handle([87; 32], true);
        let root = dir.path().join("pi/workspaces/doc-1/run-1/workspace/work");
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("关于某单位人事调整.md");
        crate::vault::write_path(&old, "pi", "保密工作稿".as_bytes()).unwrap();
        assert!(pending(crate::storage::DataSlot::Real));
        migrate().unwrap();
        assert!(!pending(crate::storage::DataSlot::Real));
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        let container = root.parent().unwrap().join("work-files.json");
        let raw = std::fs::read(&container).unwrap();
        for marker in ["关于某单位", "保密工作稿"] {
            assert!(!raw.windows(marker.len()).any(|v| v == marker.as_bytes()));
        }
        let tree: Tree = serde_json::from_slice(
            &crate::vault::read_path(&container, "pi", MAX_TREE_BYTES).unwrap(),
        )
        .unwrap();
        assert_eq!(
            tree["关于某单位人事调整.md"],
            encode("保密工作稿".as_bytes())
        );
        migrate().unwrap();
        crate::vault::clear_key_handle();
        crate::storage::set_test_config_dir(None);
    }
}
