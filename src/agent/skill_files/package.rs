//! 技能包的完整目录读写；所有相对路径在访问磁盘之前统一校验。

use super::{check_id, check_text, skills_dir, source_text, user_file};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_PACKAGE: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 512;

include!(concat!(env!("OUT_DIR"), "/builtin_skill_files.rs"));

#[derive(Clone, Default)]
pub(crate) struct Package {
    pub(crate) files: BTreeMap<String, Vec<u8>>,
}

/// 同时适用于 ZIP、Windows 与 Unix，拒绝磁盘前缀、父目录和特殊文件名。
pub(crate) fn relative_path(name: &str) -> anyhow::Result<String> {
    let name = name.replace('\\', "/");
    anyhow::ensure!(
        !name.is_empty()
            && name.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && !part.ends_with(['.', ' '])
                    && !part
                        .chars()
                        .any(|c| c.is_control() || ":*?\"<>|".contains(c))
                    && !matches!(
                        part.split('.')
                            .next()
                            .unwrap_or_default()
                            .to_uppercase()
                            .as_str(),
                        "CON"
                            | "PRN"
                            | "AUX"
                            | "NUL"
                            | "COM1"
                            | "COM2"
                            | "COM3"
                            | "COM4"
                            | "COM5"
                            | "COM6"
                            | "COM7"
                            | "COM8"
                            | "COM9"
                            | "LPT1"
                            | "LPT2"
                            | "LPT3"
                            | "LPT4"
                            | "LPT5"
                            | "LPT6"
                            | "LPT7"
                            | "LPT8"
                            | "LPT9"
                    )
            }),
        "无效的技能文件路径：{name}"
    );
    Ok(name)
}

fn no_link(path: &Path) -> anyhow::Result<()> {
    if path.exists() || path.symlink_metadata().is_ok() {
        let metadata = path.symlink_metadata()?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "不支持符号链接：{}",
            path.display()
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            anyhow::ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "不支持目录联接：{}",
                path.display()
            );
        }
    }
    Ok(())
}

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
    let name = relative_path(name)?;
    let mut path = folder(id)?;
    let parts = name.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        no_link(&path)?;
        if path.exists() {
            anyhow::ensure!(
                if index + 1 == parts.len() {
                    path.is_file()
                } else {
                    path.is_dir()
                },
                "文件与目录冲突：{}",
                path.display()
            );
        }
    }
    Ok(path)
}

impl Package {
    fn insert(&mut self, name: String, bytes: Vec<u8>) -> anyhow::Result<()> {
        let name = relative_path(&name)?;
        anyhow::ensure!(bytes.len() as u64 <= MAX_FILE, "文件超过 16 MB：{name}");
        anyhow::ensure!(self.files.len() < MAX_FILES, "技能包文件数量超过 512");
        anyhow::ensure!(
            self.files.values().map(Vec::len).sum::<usize>() + bytes.len() <= MAX_PACKAGE,
            "技能包超过 64 MB"
        );
        anyhow::ensure!(
            !self.files.keys().any(|key| key.eq_ignore_ascii_case(&name)),
            "技能包有重复路径：{name}"
        );
        let lower = name.to_lowercase();
        anyhow::ensure!(
            !self.files.keys().any(|key| {
                let key = key.to_lowercase();
                lower.starts_with(&format!("{key}/")) || key.starts_with(&format!("{lower}/"))
            }),
            "技能包的文件与目录冲突：{name}"
        );
        self.files.insert(name, bytes);
        Ok(())
    }

    fn read_dir(root: &Path) -> anyhow::Result<Self> {
        fn walk(root: &Path, dir: &Path, package: &mut Package) -> anyhow::Result<()> {
            no_link(dir)?;
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                no_link(&path)?;
                if path.is_dir() {
                    walk(root, &path, package)?;
                } else if path.is_file() {
                    anyhow::ensure!(
                        path.metadata()?.len() <= MAX_FILE,
                        "文件超过 16 MB：{}",
                        path.display()
                    );
                    let name = path
                        .strip_prefix(root)?
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("文件名不是 UTF-8"))?
                        .replace('\\', "/");
                    package.insert(name, std::fs::read(&path)?)?;
                }
            }
            Ok(())
        }
        let mut package = Self::default();
        walk(root, root, &mut package)?;
        Ok(package)
    }

    fn install(&self, id: &str) -> anyhow::Result<Vec<String>> {
        check_id(id)?;
        let entry = self
            .files
            .get("SKILL.md")
            .ok_or_else(|| anyhow::anyhow!("缺少 SKILL.md"))?;
        let problems = check_text(id, std::str::from_utf8(entry)?).map_err(anyhow::Error::msg)?;
        // 全包先检查路径与目标，解析失败时不写入任何辅助文件。
        let destinations = self
            .files
            .keys()
            .map(|name| destination(id, name))
            .collect::<anyhow::Result<Vec<_>>>()?;
        for ((_, bytes), path) in self.files.iter().zip(destinations) {
            std::fs::create_dir_all(path.parent().expect("技能目录"))?;
            std::fs::write(path, bytes)?;
        }
        Ok(problems)
    }
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

pub(crate) fn copy_for_editing(id: &str) -> anyhow::Result<()> {
    load(id)?.install(id)?;
    Ok(())
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
    std::fs::create_dir_all(path.parent().expect("技能目录"))?;
    std::fs::write(path, text)?;
    Ok(problems)
}

pub(crate) fn export(id: &str, target: &Path) -> anyhow::Result<PathBuf> {
    let package = load(id)?;
    if target
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("skill") || e.eq_ignore_ascii_case("zip"))
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(target)?);
        for (name, bytes) in package.files {
            zip.start_file(
                format!("{id}/{name}"),
                zip::write::SimpleFileOptions::default(),
            )?;
            zip.write_all(&bytes)?;
        }
        zip.finish()?;
        Ok(target.to_path_buf())
    } else {
        let root = target.join(id);
        no_link(target)?;
        no_link(&root)?;
        for (name, bytes) in package.files {
            let mut path = root.clone();
            for part in name.split('/') {
                path.push(part);
                no_link(&path)?;
            }
            std::fs::create_dir_all(path.parent().expect("技能目录"))?;
            std::fs::write(path, bytes)?;
        }
        Ok(root.join("SKILL.md"))
    }
}

pub(crate) fn import(path: &Path) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let mut packages = BTreeMap::new();
    if path.is_file() {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
        let mut files = Package::default();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            anyhow::ensure!(!entry.is_symlink(), "技能包含符号链接");
            if entry.is_dir() {
                continue;
            }
            let name = relative_path(entry.name())?;
            anyhow::ensure!(entry.size() <= MAX_FILE, "文件超过 16 MB：{name}");
            let mut bytes = Vec::new();
            entry.by_ref().take(MAX_FILE + 1).read_to_end(&mut bytes)?;
            files.insert(name, bytes)?;
        }
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
        match package.install(&id) {
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
        let base = skills_dir()?.canonicalize()?;
        anyhow::ensure!(root.canonicalize()?.starts_with(base), "技能目录越界");
        std::fs::remove_dir_all(root)?;
    }
    Ok(())
}
