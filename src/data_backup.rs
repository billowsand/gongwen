//! 本地数据的一键全量备份与恢复。
//!
//! 配置目录里放着用户的全部本机数据：`config.json`、稿件库 / 知识库 / 公文词表
//! 共用的 `manuscripts.db`、`images/`、`apis/`、`ime/`、`skills/` 等。手工复制
//! 目录容易漏文件，也会把崩溃日志、损坏配置的留底这类瞬态文件一起带走；这里
//! 提供一个范围明确、可校验、可回滚的 ZIP 备份。
//!
//! 取舍：
//! - **排除瞬态与诊断文件**：`logs/`、`.unseen-crash`、`*.损坏-*`。它们只对
//!   本机排查有用，恢复后反而可能让程序误报「上次异常退出」。
//! - **文件名净化**：恢复时任何含 `..`、绝对路径、盘符或控制字符的条目都会
//!   被拒绝，整个恢复在动当前目录之前就中止，防 ZipSlip。
//! - **原子性边界**：先把当前配置目录整体改名为同级留底目录，再解压新数据；
//!   解压中途失败会删掉半成品并尝试把留底目录改回来。改名失败则直接中止，
//!   当前数据保持原样。留底目录本身就是用户的最后一道恢复手段。

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 崩溃日志目录，只在当前机器上有意义。
const LOG_DIR: &str = "logs";
/// 上次崩溃的提示标记，恢复后不该再提示一次。
const UNSEEN_CRASH_MARKER: &str = ".unseen-crash";
/// 损坏配置留底文件的标记（`config.json.损坏-时间戳`）。
const CORRUPT_MARK: &str = ".损坏-";
/// 恢复前数据留底目录的前缀；放在配置目录的同级。
const RESTORE_BACKUP_PREFIX: &str = "GongwenAssistant-恢复前备份-";
/// 导出时使用的临时文件前缀，卡在导出目录里也不会被下次备份收进去。
const TEMP_PREFIX: &str = ".gongwen-backup-";

/// 备份里的一类顶层内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupGroup {
    /// 顶层文件名或目录名。
    pub(crate) name: String,
    /// 是否是目录（目录在界面上带 `/`）。
    pub(crate) directory: bool,
    /// 这一类里的文件数。
    pub(crate) files: usize,
    /// 这一类解压后的字节数。
    pub(crate) bytes: u64,
}

/// [`inspect_backup`] 的结果：足够判断「是不是公文助手备份」并给用户一个概要。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BackupInfo {
    /// ZIP 里的条目总数（含目录条目）。
    pub(crate) entry_count: usize,
    /// 实际文件条目数。
    pub(crate) file_count: usize,
    /// 所有文件解压后的总字节数。
    pub(crate) total_bytes: u64,
    /// 是否含配置。
    pub(crate) has_config: bool,
    /// 是否含稿件库 / 知识库 / 公文词表共用的数据库。
    pub(crate) has_manuscripts: bool,
    /// 按顶层条目归并的概要，`config.json` 与 `manuscripts.db` 排在最前。
    pub(crate) groups: Vec<BackupGroup>,
}

impl BackupInfo {
    /// 界面上逐行显示的条目概要。
    pub(crate) fn summary_lines(&self) -> Vec<String> {
        self.groups
            .iter()
            .map(|group| {
                if group.directory {
                    format!(
                        "{}/：{} 个文件，{}",
                        group.name,
                        group.files,
                        format_bytes(group.bytes)
                    )
                } else {
                    format!("{}：{}", group.name, format_bytes(group.bytes))
                }
            })
            .collect()
    }
}

/// 恢复完成后的报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreReport {
    /// 恢复前数据留底目录；`None` 表示恢复前本来就没有配置目录。
    pub(crate) backup_dir: Option<PathBuf>,
    /// 从 ZIP 解出的文件数。
    pub(crate) restored_files: usize,
}

/// 把配置目录整目录打包到 `target_zip`。
pub(crate) fn export_backup(target_zip: &Path) -> Result<()> {
    checkpoint_database();
    let config_dir = crate::storage::config_dir()?;
    export_from(&config_dir, target_zip)
}

/// 导出前把 WAL 合并回主库。SQLite 的自动检查点可能正好发生在复制主库的过程中，
/// 先做一次 FULL 检查点能显著降低「主库拷到一半、页又被写回」的概率；之后新提交
/// 只会追加到 WAL，而 WAL 在主库之后打包，合起来仍能恢复到最新一致状态。
/// 检查点失败（其他连接持锁等）不阻断导出。
fn checkpoint_database() {
    let Ok(path) = crate::storage::manuscript_db_path() else {
        return;
    };
    if !path.is_file() {
        return;
    }
    let Ok(connection) = rusqlite::Connection::open(&path) else {
        return;
    };
    let _ = connection.execute_batch("PRAGMA wal_checkpoint(FULL);");
}

/// [`export_backup`] 的可测试入口：显式指定要打包的目录。
pub(crate) fn export_from(config_dir: &Path, target_zip: &Path) -> Result<()> {
    if !config_dir.is_dir() {
        bail!("配置目录不存在：{}", config_dir.display());
    }
    if target_zip.starts_with(config_dir) {
        bail!("备份文件不能放在配置目录内，请选其他位置");
    }
    let parent = target_zip
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("无法创建备份目录：{}", parent.display()))?;

    let files = collect_files(config_dir)?;
    // 先写同目录临时文件，写完再原子替换目标：导出中途失败不会留下半个 ZIP。
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("无法在 {} 创建临时文件", parent.display()))?;
    {
        let mut zip = zip::ZipWriter::new(temp.as_file_mut());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for rel in &files {
            let source = config_dir.join(rel);
            let name = zip_entry_name(rel);
            let bytes = match fs::read(&source) {
                Ok(bytes) => bytes,
                // 遍历与读取之间文件被删掉（如 SQLite 的临时日志）属于正常竞争，
                // 跳过即可；其余读取错误如实上报。
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("无法读取 {}", source.display()));
                }
            };
            zip.start_file(&name, options)
                .with_context(|| format!("无法写入备份条目：{name}"))?;
            zip.write_all(&bytes)
                .with_context(|| format!("无法写入备份条目：{name}"))?;
        }
        zip.finish().context("无法写完备份文件")?;
    }
    temp.as_file().sync_all()?;
    temp.persist(target_zip)
        .map_err(|error| error.error)
        .with_context(|| format!("无法保存备份文件：{}", target_zip.display()))?;
    Ok(())
}

/// 校验 ZIP 结构并给出条目概要。至少含 `config.json` 或 `manuscripts.db` 之一
/// 才算公文助手备份；条目名不安全时直接拒绝。
pub(crate) fn inspect_backup(zip_path: &Path) -> Result<BackupInfo> {
    let mut archive = open_archive(zip_path)?;
    let mut info = BackupInfo::default();
    let mut groups: BTreeMap<String, BackupGroup> = BTreeMap::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .with_context(|| format!("无法读取备份条目 #{index}"))?;
        let rel = safe_relative_path(entry.name())?;
        info.entry_count += 1;
        if entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            bail!("备份里包含符号链接，已拒绝：{}", entry.name());
        }
        info.file_count += 1;
        let size = entry.size();
        info.total_bytes = info.total_bytes.saturating_add(size);
        if rel == Path::new("config.json") {
            info.has_config = true;
        }
        if rel == Path::new("manuscripts.db") {
            info.has_manuscripts = true;
        }
        let mut parts = rel.components();
        let Some(first) = parts.next() else {
            continue;
        };
        let name = first.as_os_str().to_string_lossy().into_owned();
        let directory = parts.next().is_some();
        let group = groups.entry(name.clone()).or_insert_with(|| BackupGroup {
            name,
            directory,
            files: 0,
            bytes: 0,
        });
        group.files += 1;
        group.bytes = group.bytes.saturating_add(size);
    }
    if !info.has_config && !info.has_manuscripts {
        bail!("这不是公文助手的全量备份：既没有 config.json，也没有 manuscripts.db");
    }
    info.groups = groups.into_values().collect();
    // 两个关键文件排在最前，其余按名称排，界面上先看到「有没有真东西」。
    info.groups.sort_by_key(|group| {
        let rank = match group.name.as_str() {
            "config.json" => 0,
            "manuscripts.db" => 1,
            _ => 2,
        };
        (rank, group.name.clone())
    });
    Ok(info)
}

/// 把备份恢复到 `config_dir`。
///
/// 恢复前把现有配置目录整体改名为同级 `GongwenAssistant-恢复前备份-<时间戳>`；
/// 改名失败就中止，不动任何现有数据。解压失败会删掉半成品并尝试改回留底目录。
pub(crate) fn restore_backup(zip_path: &Path, config_dir: &Path) -> Result<RestoreReport> {
    // 结构与条目名先整体过一遍：确认安全后再碰当前数据。
    inspect_backup(zip_path)?;
    let mut archive = open_archive(zip_path)?;

    let existed = config_dir.exists();
    let backup_dir = if existed {
        let backup_dir = next_restore_backup_path(config_dir);
        fs::rename(config_dir, &backup_dir).with_context(|| {
            format!(
                "无法把当前配置目录留底到 {}，恢复已中止",
                backup_dir.display()
            )
        })?;
        Some(backup_dir)
    } else {
        None
    };

    let extracted = extract_all(&mut archive, config_dir);
    match extracted {
        Ok(restored_files) => Ok(RestoreReport {
            backup_dir,
            restored_files,
        }),
        Err(error) => {
            // 半成品不能留着冒充「已恢复」；留底目录在就改回去。
            let _ = fs::remove_dir_all(config_dir);
            let rollback = match &backup_dir {
                Some(backup_dir) => fs::rename(backup_dir, config_dir).with_context(|| {
                    format!(
                        "恢复失败，且无法把留底目录 {} 改回 {}",
                        backup_dir.display(),
                        config_dir.display()
                    )
                }),
                None => Ok(()),
            };
            let error = error.context("恢复备份失败");
            match rollback {
                Ok(()) => Err(error.context("已还原恢复前的数据")),
                Err(rollback_error) => Err(error.context(format!(
                    "恢复前数据仍留在 {}，请手工改回；{rollback_error:#}",
                    backup_dir
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default()
                ))),
            }
        }
    }
}

/// 界面上预告「当前数据会先备份到哪」用的路径（实际执行时若同一秒已存在，
/// 会自动换一个序号）。
pub(crate) fn restore_backup_location(config_dir: &Path) -> PathBuf {
    let parent = config_dir.parent().unwrap_or_else(|| Path::new("."));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    parent.join(format!("{RESTORE_BACKUP_PREFIX}{stamp}"))
}

fn next_restore_backup_path(config_dir: &Path) -> PathBuf {
    let mut candidate = restore_backup_location(config_dir);
    if !candidate.exists() {
        return candidate;
    }
    let base = candidate
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut index = 2;
    loop {
        candidate = config_dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{base}-{index}"));
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}

fn open_archive(zip_path: &Path) -> Result<zip::ZipArchive<fs::File>> {
    let file = fs::File::open(zip_path)
        .with_context(|| format!("无法打开备份文件：{}", zip_path.display()))?;
    zip::ZipArchive::new(file).context("文件不是有效的 ZIP 备份")
}

/// 递归收集要打包的文件（相对配置目录）。不跟随符号链接；瞬态文件在这里被排除。
fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).with_context(|| format!("无法读取目录：{}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            if is_excluded(rel) {
                continue;
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                files.push(rel.to_path_buf());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// 瞬态 / 诊断文件不进入备份。
fn is_excluded(rel: &Path) -> bool {
    if rel.components().next().is_some_and(|first| {
        first
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(LOG_DIR)
    }) {
        return true;
    }
    let Some(name) = rel.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == UNSEEN_CRASH_MARKER
        || name.contains(CORRUPT_MARK)
        || name.starts_with(TEMP_PREFIX)
        || name.ends_with(".tmp")
}

/// ZIP 条目名用 `/` 分隔，与平台无关。
fn zip_entry_name(rel: &Path) -> String {
    rel.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// 净化并校验 ZIP 条目名。任何可能跑到目标目录之外的写法都拒绝整个备份，
/// 而不是改名后继续——被篡改的备份没有「差不多能用」的说法。
fn safe_relative_path(name: &str) -> Result<PathBuf> {
    if name.is_empty() {
        bail!("备份里存在空条目名");
    }
    if name.chars().any(char::is_control) {
        bail!("备份条目名含控制字符，已拒绝：{name:?}");
    }
    if name.starts_with('/') || name.starts_with('\\') {
        bail!("备份条目名是绝对路径，已拒绝：{name}");
    }
    let mut path = PathBuf::new();
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            bail!("备份条目名含路径穿越（..），已拒绝：{name}");
        }
        // 盘符（`C:`）与 NTFS 备用数据流（`file:stream`）都不是正常文件名。
        if part.contains(':') {
            bail!("备份条目名含非法字符，已拒绝：{name}");
        }
        path.push(part);
    }
    if path.as_os_str().is_empty() {
        bail!("备份条目名无效：{name}");
    }
    Ok(path)
}

fn extract_all(archive: &mut zip::ZipArchive<fs::File>, target: &Path) -> Result<usize> {
    let mut files = 0;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("无法读取备份条目 #{index}"))?;
        let rel = safe_relative_path(entry.name())?;
        let dest = target.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&dest)
                .with_context(|| format!("无法创建目录：{}", dest.display()))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目录：{}", parent.display()))?;
        }
        let mut file = fs::File::create(&dest)
            .with_context(|| format!("无法写入恢复文件：{}", dest.display()))?;
        std::io::copy(&mut entry, &mut file)
            .with_context(|| format!("写入恢复文件失败：{}", dest.display()))?;
        let _ = file.sync_all();
        #[cfg(unix)]
        restrict_permissions(&dest);
        files += 1;
    }
    Ok(files)
}

/// 配置与密钥文件恢复后仍只给当前用户读写。ZIP 里的权限位不跨平台可信，
/// 这里按文件名重新收紧，和 [`crate::storage::save`] 的口径一致。
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if matches!(name, "config.json" | ".zip-password" | "api-secrets.json") {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

/// 人类可读的字节数，备份完成提示与条目概要共用。
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn read(root: &Path, rel: &str) -> Vec<u8> {
        fs::read(root.join(rel)).unwrap()
    }

    /// 造一份带瞬态文件的假配置目录。
    fn fake_config_dir(root: &Path) {
        write(root, "config.json", br#"{"output_dir":"C:/out"}"#);
        write(root, "manuscripts.db", b"sqlite-db");
        write(root, "manuscripts.db-wal", b"wal");
        write(root, "images/a.png", b"png-a");
        write(root, "apis/svc.openapi.json", b"{}");
        write(root, "ime/base.txt", b"aaaa");
        write(root, "skills/mine/SKILL.md", b"# skill");
        write(root, ".zip-password", b"secret");
        // 以下都不该进备份。
        write(root, "logs/crash-20260101-000000.log", b"panic");
        write(root, ".unseen-crash", b"logs/crash.log");
        write(root, "config.json.损坏-20260101-000000", b"broken");
    }

    #[test]
    fn export_excludes_diagnostics_and_restore_replaces_the_config_dir() {
        let source = tempfile::tempdir().unwrap();
        fake_config_dir(source.path());

        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("backup.zip");
        export_from(source.path(), &zip).unwrap();

        let info = inspect_backup(&zip).unwrap();
        assert!(info.has_config && info.has_manuscripts);
        assert_eq!(info.file_count, 8, "只应打包 8 个有效文件");
        let names: Vec<&str> = info.groups.iter().map(|g| g.name.as_str()).collect();
        assert!(!names.contains(&"logs"), "崩溃日志不该进备份");
        assert!(!names.contains(&".unseen-crash"));
        assert_eq!(names[0], "config.json");
        assert_eq!(names[1], "manuscripts.db");

        // 目标目录模拟当前配置目录：恢复后旧文件应整目录换成备份内容。
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config");
        write(&target, "keep.txt", b"keep");
        write(&target, "config.json", b"old");

        let report = restore_backup(&zip, &target).unwrap();
        assert_eq!(report.restored_files, 8);
        assert_eq!(read(&target, "config.json"), br#"{"output_dir":"C:/out"}"#);
        assert_eq!(read(&target, "manuscripts.db"), b"sqlite-db");
        assert_eq!(read(&target, "manuscripts.db-wal"), b"wal");
        assert_eq!(read(&target, "images/a.png"), b"png-a");
        assert_eq!(read(&target, "apis/svc.openapi.json"), b"{}");
        assert_eq!(read(&target, "ime/base.txt"), b"aaaa");
        assert_eq!(read(&target, "skills/mine/SKILL.md"), b"# skill");
        assert_eq!(read(&target, ".zip-password"), b"secret");
        assert!(!target.join("keep.txt").exists(), "恢复应替换整个目录");
        assert!(!target.join("logs").exists());
        assert!(!target.join(".unseen-crash").exists());

        // 旧数据留底在同级目录，原名内容还在。
        let backup = report.backup_dir.expect("应留下恢复前备份");
        assert_eq!(read(&backup, "keep.txt"), b"keep");
        assert_eq!(read(&backup, "config.json"), b"old");
        assert_eq!(backup.parent(), Some(root.path()));
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(RESTORE_BACKUP_PREFIX)
        );
    }

    #[test]
    fn zip_without_config_or_manuscripts_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("other.zip");
        {
            let file = fs::File::create(&zip).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"not a gongwen backup").unwrap();
            writer.finish().unwrap();
        }
        let error = inspect_backup(&zip).unwrap_err().to_string();
        assert!(error.contains("config.json"), "{error}");

        // 恢复也不能动当前目录。
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config");
        write(&target, "keep.txt", b"keep");
        assert!(restore_backup(&zip, &target).is_err());
        assert_eq!(read(&target, "keep.txt"), b"keep");
    }

    #[test]
    fn path_traversal_entries_are_rejected_before_touching_current_data() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("evil.zip");
        {
            let file = fs::File::create(&zip).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            for (name, bytes) in [
                ("config.json", b"{}".as_slice()),
                ("images/../evil.png", b"evil".as_slice()),
            ] {
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }

        let error = inspect_backup(&zip).unwrap_err().to_string();
        assert!(error.contains("路径穿越"), "{error}");

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config");
        write(&target, "keep.txt", b"keep");
        assert!(restore_backup(&zip, &target).is_err());
        assert_eq!(
            read(&target, "keep.txt"),
            b"keep",
            "校验失败时当前数据必须原样保留"
        );
        // 也不该留下恢复前的留底目录。
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn export_refuses_to_write_inside_the_config_dir() {
        let source = tempfile::tempdir().unwrap();
        fake_config_dir(source.path());
        let error = export_from(source.path(), &source.path().join("out.zip"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("不能放在配置目录内"), "{error}");
        assert!(!source.path().join("out.zip").exists());
    }
}
