//! 旧流程技能包的完整目录读写；所有相对路径在访问磁盘之前统一校验。
//!
//! 包的通用安全读写（路径、链接、体积、ZIP 条目）在
//! [`crate::ai_services::skill_package`]；这里只保留旧格式自己的入口校验、安装位置
//! 与导入分组规则。4C 起 Pi 标准技能复用同一套原语，但不复用旧流程语义。
//! 旧流程管理页已无入口，这些接口保留到 4F 退役。

use super::{check_id, check_text, skills_dir, source_text, user_file};
use crate::ai_services::skill_package::{
    self, Limits, MAX_FILE, MAX_FILES, MAX_PACKAGE, Package, destination_with, no_link,
    relative_path,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/builtin_skill_files.rs"));

pub(crate) fn folder(id: &str) -> anyhow::Result<PathBuf> {
    check_id(id)?;
    relative_path(id)?;
    let base = skills_dir()?;
    no_link(&base)?;
    let root = base.join(id);
    no_link(&root)?;
    Ok(root)
}

pub(super) fn destination(id: &str, name: &str) -> anyhow::Result<PathBuf> {
    destination_with(&folder(id)?, name, Limits::LEGACY)
}

fn install(package: &Package, id: &str) -> anyhow::Result<Vec<String>> {
    check_id(id)?;
    let entry = package
        .files
        .get("SKILL.md")
        .ok_or_else(|| anyhow::anyhow!("缺少 SKILL.md"))?;
    let problems = check_text(id, std::str::from_utf8(entry)?).map_err(anyhow::Error::msg)?;
    // 全包先检查路径与目标，解析失败时不写入任何辅助文件。
    let destinations = package
        .files
        .keys()
        .map(|name| destination(id, name))
        .collect::<anyhow::Result<Vec<_>>>()?;
    for ((_, bytes), path) in package.files.iter().zip(destinations) {
        std::fs::create_dir_all(path.parent().expect("技能目录"))?;
        std::fs::write(path, bytes)?;
    }
    Ok(problems)
}

pub(crate) fn load(id: &str) -> anyhow::Result<Package> {
    let root = folder(id)?;
    if user_file(id)?.is_file() {
        Package::read_dir(&root)
    } else {
        let mut package = Package::default();
        for (_, name, bytes) in BUILTIN_FILES
            .iter()
            .filter(|(skill_id, _, _)| *skill_id == id)
        {
            package.insert((*name).to_string(), bytes.to_vec())?;
        }
        if !package.files.contains_key("SKILL.md") {
            package.insert("SKILL.md".into(), source_text(id)?.into_bytes())?;
        }
        Ok(package)
    }
}

pub(crate) fn save_file(id: &str, name: &str, text: &str) -> anyhow::Result<Vec<String>> {
    let path = destination(id, name)?;
    anyhow::ensure!(user_file(id)?.is_file(), "请先复制为我的技能");
    anyhow::ensure!(text.len() as u64 <= MAX_FILE, "文件超过 16 MB");
    let files = load(id)?;
    let normalized = relative_path(name)?;
    anyhow::ensure!(
        !files
            .files
            .keys()
            .any(|existing| existing != &normalized && existing.eq_ignore_ascii_case(&normalized)),
        "已存在同名文件，请保持路径大小写一致：{normalized}"
    );
    anyhow::ensure!(
        files.files.contains_key(&normalized) || files.files.len() < MAX_FILES,
        "技能包文件数量超过 512"
    );
    let previous = files.files.get(&normalized).map_or(0, Vec::len);
    anyhow::ensure!(
        files.files.values().map(Vec::len).sum::<usize>() - previous + text.len() <= MAX_PACKAGE,
        "技能包超过 64 MB"
    );
    let problems = if normalized == "SKILL.md" {
        check_text(id, text).map_err(anyhow::Error::msg)?
    } else {
        Vec::new()
    };
    crate::storage::atomic_write(&path, |file| {
        use std::io::Write;
        file.write_all(text.as_bytes())?;
        Ok(())
    })?;
    Ok(problems)
}

pub(crate) fn export(id: &str, target: &Path) -> anyhow::Result<PathBuf> {
    let package = load(id)?;
    if target
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("skill") || e.eq_ignore_ascii_case("zip"))
    {
        package.write_zip(target, id)?;
        Ok(target.to_path_buf())
    } else {
        let root = target.join(id);
        no_link(target)?;
        no_link(&root)?;
        package.write_dir(&root)?;
        Ok(root.join("SKILL.md"))
    }
}

pub(crate) fn import(path: &Path) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let mut packages = BTreeMap::new();
    if path.is_file() {
        let files = Package::read_zip(path)?;
        // 支持外层封装目录和一个 ZIP 中多个技能，辅助文件按入口所在目录归属。
        if files.files.contains_key("SKILL.md") {
            let id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            packages.insert(id, files.clone());
        }
        let roots = files
            .files
            .keys()
            .filter_map(|name| name.strip_suffix("/SKILL.md").map(str::to_owned))
            .collect::<Vec<_>>();
        for root in roots {
            let id = root.rsplit('/').next().unwrap_or_default().to_string();
            let prefix = format!("{root}/");
            let package = Package {
                files: files
                    .files
                    .iter()
                    .filter_map(|(name, bytes)| {
                        name.strip_prefix(&prefix)
                            .map(|name| (name.to_string(), bytes.clone()))
                    })
                    .collect(),
            };
            anyhow::ensure!(
                packages.insert(id.clone(), package).is_none(),
                "重复的技能 id：{id}"
            );
        }
    } else if path.join("SKILL.md").is_file() {
        let id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        packages.insert(id, Package::read_dir(path)?);
    } else {
        for entry in std::fs::read_dir(path)? {
            let dir = entry?.path();
            if dir.join("SKILL.md").is_file() {
                let id = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                packages.insert(id, Package::read_dir(&dir)?);
            }
        }
    }
    anyhow::ensure!(
        !packages.is_empty(),
        "{} 里没有找到 SKILL.md",
        path.display()
    );
    let mut ids = Vec::new();
    let mut notes = Vec::new();
    for (id, package) in packages {
        match install(&package, &id) {
            Ok(problems) => {
                if !problems.is_empty() {
                    notes.push(format!("「{id}」有问题：{}", problems.join("；")));
                }
                ids.push(id);
            }
            Err(error) => notes.push(format!("「{id}」导入失败，跳过：{error:#}")),
        }
    }
    Ok((ids, notes))
}

pub(crate) fn remove(id: &str) -> anyhow::Result<()> {
    let root = folder(id)?;
    if root.exists() {
        // folder 验证 id 与目录联接；解析后的目标必须仍位于配置的技能目录内。
        skill_package::remove_dir_within(&skills_dir()?, &root)?;
    }
    Ok(())
}
