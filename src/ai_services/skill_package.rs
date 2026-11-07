//! 技能包的通用安全读写原语。
//!
//! 旧流程技能包（`agent::skill_files`）与 Pi 标准技能包（`pi_agent::pi_skills`）都用
//! 这一份实现：相对路径统一校验、ZIP 同时卡声明大小与实际解压大小、目录读写拒绝
//! 符号链接与 Windows 目录联接。两种格式只在**入口文件与校验规则**上不同，包本身
//! 都是「相对路径 → 字节」的映射，不在这里解释技能内容。
//!
//! 调用方必须自己决定包内哪些文件是入口、入口怎么解析；这里只保证不越界、不超量、
//! 不静默覆盖。

use anyhow::Result;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 一套包体积、文件数、目录深度与 ZIP 条目上限。
///
/// 旧流程技能与 Pi 技能的 host 文件读取能力不同，不能共用同一组数字；但安全判据
/// （声明大小 + 实际大小、路径、链接）完全一致。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) max_file: u64,
    pub(crate) max_total: usize,
    pub(crate) max_files: usize,
    pub(crate) max_archive_total: u64,
    pub(crate) max_archive_entries: usize,
    pub(crate) max_depth: usize,
}

impl Limits {
    /// 旧流程技能包：保持 4B 之前的行为与报错措辞。
    pub(crate) const LEGACY: Self = Self {
        max_file: 16 * 1024 * 1024,
        max_total: 64 * 1024 * 1024,
        max_files: 512,
        max_archive_total: 128 * 1024 * 1024,
        max_archive_entries: 4096,
        max_depth: 64,
    };

    /// Pi 标准技能包：单文件不超过 host 的文件读取上限（8 MiB）。
    pub(crate) const PI: Self = Self {
        max_file: 8 * 1024 * 1024,
        max_total: 32 * 1024 * 1024,
        max_files: 256,
        max_archive_total: 64 * 1024 * 1024,
        max_archive_entries: 2048,
        max_depth: 8,
    };
}

/// 旧流程技能包沿用的常量（旧调用方仍引用）。
#[cfg(test)]
pub(crate) const MAX_FILE: u64 = Limits::LEGACY.max_file;
#[cfg(test)]
pub(crate) const MAX_PACKAGE: usize = Limits::LEGACY.max_total;
#[cfg(test)]
pub(crate) const MAX_FILES: usize = Limits::LEGACY.max_files;

/// 同时适用于 ZIP、Windows 与 Unix，拒绝磁盘前缀、父目录和特殊文件名。
pub(crate) fn relative_path(name: &str) -> Result<String> {
    relative_path_with(name, Limits::LEGACY)
}

/// 按指定上限校验相对路径（含目录深度）。
pub(crate) fn relative_path_with(name: &str, limits: Limits) -> Result<String> {
    let name = name.replace('\\', "/");
    anyhow::ensure!(!name.is_empty(), "技能文件路径不能为空");
    let parts = name.split('/').collect::<Vec<_>>();
    anyhow::ensure!(
        parts.len() <= limits.max_depth,
        "技能文件目录超过 {} 层：{name}",
        limits.max_depth
    );
    anyhow::ensure!(
        parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
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

/// 拒绝符号链接与 Windows 目录联接；不存在的路径视为安全。
pub(crate) fn no_link(path: &Path) -> Result<()> {
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

/// 解析包内相对路径到 `root` 下，逐段检查链接与文件/目录冲突。
pub(crate) fn destination_with(root: &Path, name: &str, limits: Limits) -> Result<PathBuf> {
    let name = relative_path_with(name, limits)?;
    no_link(root)?;
    let mut path = root.to_path_buf();
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

/// 删除 `base` 下的一个目录；解析后的目标必须仍位于 `base` 内。
#[cfg(test)]
pub(crate) fn remove_dir_within(base: &Path, root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    no_link(base)?;
    no_link(root)?;
    let base = base.canonicalize()?;
    let target = root.canonicalize()?;
    anyhow::ensure!(target.starts_with(&base) && target != base, "技能目录越界");
    std::fs::remove_dir_all(target)?;
    Ok(())
}

/// 一个技能包的全部文件（相对路径 → 字节）。
#[derive(Clone, Default, Debug)]
pub(crate) struct Package {
    pub(crate) files: BTreeMap<String, Vec<u8>>,
}

impl Package {
    #[cfg(test)]
    pub(crate) fn insert(&mut self, name: String, bytes: Vec<u8>) -> Result<()> {
        self.insert_with(name, bytes, Limits::LEGACY)
    }

    pub(crate) fn insert_with(
        &mut self,
        name: String,
        bytes: Vec<u8>,
        limits: Limits,
    ) -> Result<()> {
        let name = relative_path_with(&name, limits)?;
        anyhow::ensure!(
            bytes.len() as u64 <= limits.max_file,
            "文件超过 {} MB：{name}",
            limits.max_file / 1024 / 1024
        );
        anyhow::ensure!(
            self.files.len() < limits.max_files,
            "技能包文件数量超过 {}",
            limits.max_files
        );
        anyhow::ensure!(
            self.files.values().map(Vec::len).sum::<usize>() + bytes.len() <= limits.max_total,
            "技能包超过 {} MB",
            limits.max_total / 1024 / 1024
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

    #[cfg(test)]
    pub(crate) fn read_dir(root: &Path) -> Result<Self> {
        Self::read_dir_with(root, Limits::LEGACY)
    }

    pub(crate) fn read_dir_with(root: &Path, limits: Limits) -> Result<Self> {
        Self::read_dir_with_reader(root, limits, |path| {
            Ok(crate::file_limits::read_bytes(
                path,
                limits.max_file,
                "技能文件",
            )?)
        })
    }

    /// 同一份目录校验，内容由调用方读取；配置目录内的用户技能据此透明解密，
    /// 外部导入目录仍走普通文件读取。
    pub(crate) fn read_dir_with_reader(
        root: &Path,
        limits: Limits,
        read: impl Fn(&Path) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        fn walk(
            root: &Path,
            dir: &Path,
            depth: usize,
            package: &mut Package,
            limits: Limits,
            read: &impl Fn(&Path) -> Result<Vec<u8>>,
        ) -> Result<()> {
            anyhow::ensure!(
                depth <= limits.max_depth,
                "技能文件目录超过 {} 层：{}",
                limits.max_depth,
                dir.display()
            );
            no_link(dir)?;
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                no_link(&path)?;
                if path.is_dir() {
                    walk(root, &path, depth + 1, package, limits, read)?;
                } else if path.is_file() {
                    anyhow::ensure!(
                        path.metadata()?.len()
                            <= limits.max_file + crate::vault::SEALED_FILE_OVERHEAD as u64,
                        "文件超过 {} MB：{}",
                        limits.max_file / 1024 / 1024,
                        path.display()
                    );
                    let name = path
                        .strip_prefix(root)?
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("文件名不是 UTF-8"))?
                        .replace('\\', "/");
                    let bytes = read(&path)?;
                    package.insert_with(name, bytes, limits)?;
                }
            }
            Ok(())
        }
        let mut package = Self::default();
        walk(root, root, 0, &mut package, limits, &read)?;
        Ok(package)
    }

    #[cfg(test)]
    pub(crate) fn read_zip(path: &Path) -> Result<Self> {
        Self::read_zip_with(path, Limits::LEGACY)
    }

    pub(crate) fn read_zip_with(path: &Path, limits: Limits) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path)?)?;
        anyhow::ensure!(
            archive.len() <= limits.max_archive_entries,
            "技能包里的条目超过 {} 个",
            limits.max_archive_entries
        );
        let mut total: u64 = 0;
        let mut files = Self::default();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            anyhow::ensure!(!entry.is_symlink(), "技能包含符号链接");
            if entry.is_dir() {
                continue;
            }
            let name = relative_path_with(entry.name(), limits)?;
            anyhow::ensure!(
                entry.size() <= limits.max_file,
                "文件超过 {} MB：{name}",
                limits.max_file / 1024 / 1024
            );
            // 按声明大小累加，提前拒绝明显过大的包；实际字节由 `insert_with` 再卡。
            total = total.saturating_add(entry.size());
            anyhow::ensure!(
                total <= limits.max_archive_total,
                "技能包解压后超过 {} MB",
                limits.max_archive_total / 1024 / 1024
            );
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(limits.max_file.saturating_add(1))
                .read_to_end(&mut bytes)?;
            files.insert_with(name, bytes, limits)?;
        }
        Ok(files)
    }

    /// 写成 ZIP；`prefix` 是包内统一前缀（通常是技能名），空串表示直接放根下。
    pub(crate) fn write_zip(&self, target: &Path, prefix: &str) -> Result<()> {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(target)?);
        for (name, bytes) in &self.files {
            let full = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let full = relative_path(&full)?;
            zip.start_file(full, zip::write::SimpleFileOptions::default())?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
        Ok(())
    }

    /// 写到目录下；目标目录可以不存在，但每个已存在的前缀都必须是目录且不是链接。
    pub(crate) fn write_dir(&self, root: &Path) -> Result<()> {
        for (name, bytes) in &self.files {
            let path = destination_with(root, name, Limits::LEGACY)?;
            std::fs::create_dir_all(path.parent().expect("技能目录"))?;
            std::fs::write(path, bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, text: &str) -> Package {
        let mut package = Package::default();
        package
            .insert(name.into(), text.as_bytes().to_vec())
            .unwrap();
        package
    }

    #[test]
    fn paths_cannot_escape_or_name_windows_devices() {
        for path in [
            "../escape.txt",
            "a/../../escape.txt",
            "/etc/passwd",
            "C:/escape.txt",
            "a/CON.txt",
            "a/b.",
            "a/ b ",
            "a//b",
        ] {
            assert!(relative_path(path).is_err(), "{path}");
        }
        assert_eq!(
            relative_path("references/依据.md").unwrap(),
            "references/依据.md"
        );
        assert!(
            relative_path_with(
                "a/b/c",
                Limits {
                    max_depth: 2,
                    ..Limits::PI
                }
            )
            .is_err()
        );
    }

    #[test]
    fn package_rejects_duplicate_and_file_directory_conflicts() {
        let mut files = package("a.txt", "1");
        assert!(files.insert("A.TXT".into(), b"2".to_vec()).is_err());
        assert!(files.insert("a.txt/b".into(), b"2".to_vec()).is_err());
        let mut files = package("a/b", "1");
        assert!(files.insert("a".into(), b"2".to_vec()).is_err());
    }

    #[test]
    fn zip_round_trip_keeps_nested_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = Package::default();
        files
            .insert_with(
                "references/引用.md".into(),
                "# 引用".as_bytes().to_vec(),
                Limits::PI,
            )
            .unwrap();
        files
            .insert_with("assets/a.bin".into(), vec![0, 255, 1], Limits::PI)
            .unwrap();
        let zip = dir.path().join("a.zip");
        files.write_zip(&zip, "mine").unwrap();
        let read = Package::read_zip_with(&zip, Limits::PI).unwrap();
        let expected = files
            .files
            .into_iter()
            .map(|(name, bytes)| (format!("mine/{name}"), bytes))
            .collect();
        assert_eq!(read.files, expected);
    }

    #[test]
    fn oversized_declared_zip_is_refused_before_reading() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("big.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file("SKILL.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();
        let limits = Limits {
            max_archive_total: 0,
            ..Limits::PI
        };
        assert!(Package::read_zip_with(&zip_path, limits).is_err());
    }
}
