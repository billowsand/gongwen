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
//! - **解压上限**：条目总数、单条尺寸、整包解压总量都有硬上限（见
//!   [`MAX_BACKUP_ENTRIES`] 等常量）。`inspect_backup` 校验与 `restore_backup`
//!   解压两条路径都会卡；超限的包在动当前数据之前就被拒，或解压中途出错后
//!   走既有回滚把留底目录改回来。
//! - **隔离暂存与原子性边界**：先把备份完整解压到配置目录同级的随机暂存目录，
//!   校验数据库、槽位与锁封套后才把当前配置目录整体改名为同级留底目录并换入。
//!   换入失败会尝试把留底目录改回来；校验失败根本不会碰当前数据。留底目录本身
//!   就是用户的最后一道恢复手段。

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};

/// 崩溃日志目录，只在当前机器上有意义。
const LOG_DIR: &str = "logs";
/// 诱饵槽位的崩溃日志目录（`crash_log::log_dir` 按槽位映射成 `alt-logs`），
/// 与真实槽一样只在本机有意义，不进备份。
const DECOY_LOG_DIR: &str = "alt-logs";
/// 上次崩溃的提示标记，恢复后不该再提示一次。
const UNSEEN_CRASH_MARKER: &str = ".unseen-crash";
/// 损坏配置留底文件的标记（`config.json.损坏-时间戳`）。
const CORRUPT_MARK: &str = ".损坏-";
/// 恢复前数据留底目录的前缀；放在配置目录的同级。
const RESTORE_BACKUP_PREFIX: &str = "GongwenAssistant-恢复前备份-";
/// 恢复暂存目录的前缀；与配置目录同级，保证最后换入是同一文件系统内的改名。
const RESTORE_STAGING_PREFIX: &str = ".gongwen-restore-";
/// 导出时使用的临时文件前缀，卡在导出目录里也不会被下次备份收进去。
const TEMP_PREFIX: &str = ".gongwen-backup-";

/// 解压上限：条目总数。
///
/// 备份的用途是把配置目录原样带走，正常体量是几十 MB 到一两 GB。下面三项按
/// 「够用且挡得住 ZIP 炸弹」取保守值，任何一项超限都拒绝整个恢复，绝不只跳
/// 过超限条目后继续——被构造过的包没有「差不多能用」的说法。
const MAX_BACKUP_ENTRIES: usize = 50_000;
/// 解压上限：单个条目（一个库文件、一张插图都远小于它）。
const MAX_BACKUP_ENTRY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 解压上限：整包所有条目解压后的总字节。
///
/// 本来想取「8 GiB 与可用磁盘空间较小者」，但不引入新依赖就只能做平台相关的
/// 可用空间查询（各平台接口不同、也不一定可靠）。固定 8 GiB 已经足够保守：
/// 真把磁盘写满时解压自己会报写入失败，恢复照样走回滚。
const MAX_BACKUP_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

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
    /// 是否含旧版明文数据库或明文配置；用于恢复确认中的明确提示。
    pub(crate) has_plaintext_data: bool,
    /// 按顶层条目归并的概要，`config.json` 与 `manuscripts.db` 排在最前。
    pub(crate) groups: Vec<BackupGroup>,
}

impl BackupInfo {
    /// 界面上逐行显示的条目概要。
    pub(crate) fn summary_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .groups
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
            .collect();
        if self.has_plaintext_data {
            lines.insert(
                0,
                "提示：备份含明文数据库或配置（旧明文备份未加密）；恢复后请按需要重新启用应用锁。"
                    .into(),
            );
        }
        lines
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
    let config_dir = crate::storage::config_dir()?;
    export_from(&config_dir, target_zip)
}

/// 主库与 WAL 必须由 SQLite 合并成一致快照，不能分别复制正在变化的文件。
/// 只读打开源库，任何快照错误都中止导出，不能退回原始文件复制。
/// 源库带数据密钥开（加密库不载 key 读不了）；目标库同样带密钥创建——
/// 备份包里装的是密文（等于自带加密的备份），无句柄（未启用应用锁）时
/// 两端都退化为明文，与磁盘形态一致。
fn snapshot_database(source: &Path, destination: &Path) -> Result<()> {
    // 只读权限打开源库即可满足「快照绝不写源库」；但 ATTACH 新库要可写
    // （只读连接会把附库也按只读开，建新快照文件直接打不开）。
    let connection =
        rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .context("无法打开稿件库以生成备份快照")?;
    crate::storage::apply_db_key(&connection)?;
    // 备份快照与磁盘同形态：句柄已激活（库是密文）则快照也带密钥导出，
    // 无句柄或本会话还没迁移时两端都明文。SQLCipher 不支持在线备份 API，
    // 迁移与快照共用 vault::export_database（sqlcipher_export）。
    let dest_key = crate::vault::key_handle()
        .filter(|handle| handle.db_active())
        .map(|handle| *handle.dek());
    crate::vault::export_database(&connection, destination, dest_key.as_ref())
        .context("无法生成稿件库的一致备份快照")?;
    Ok(())
}

/// 非当前槽没有对应 DEK，不能打开密文库做 `sqlcipher_export`。只有主库不存在
/// 未合并的 WAL / 热回滚日志，并且整段复制前后内容完全一致，才允许原样收进备份。
/// 两次完整读取同时充当轻量代次确认：复制期间有写入或 sidecar 状态变化就失败，
/// 绝不把「排除 WAL 后的主库」冒充完整快照。
fn copy_quiescent_database(source: &Path, destination: &Path) -> Result<()> {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "非当前槽位数据库".into());
    let ensure_clean = || -> Result<()> {
        for (suffix, kind) in [("-wal", "WAL"), ("-journal", "回滚日志")] {
            let sidecar = sibling_with_suffix(source, suffix);
            if sidecar.is_file() && fs::metadata(&sidecar)?.len() != 0 {
                bail!(
                    "非当前槽位数据库 {name} 仍有未检查点的{kind}，不能生成完整备份。请先切换到对应槽位并正常退出应用，让数据库完成检查点后再备份"
                );
            }
        }
        Ok(())
    };
    ensure_clean()?;
    let bytes = fs::read(source)
        .with_context(|| format!("无法读取非当前槽位数据库：{}", source.display()))?;
    ensure_clean()?;
    let verification = fs::read(source)
        .with_context(|| format!("无法复核非当前槽位数据库：{}", source.display()))?;
    ensure_clean()?;
    if bytes != verification {
        bail!(
            "非当前槽位数据库 {name} 在备份期间发生变化，已中止。请先切换到对应槽位并正常退出应用后再备份"
        );
    }
    fs::write(destination, bytes)
        .with_context(|| format!("无法写入数据库备份副本：{}", destination.display()))?;
    Ok(())
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

    let snapshot_dir = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempdir_in(parent)
        .context("无法创建数据库备份临时目录")?;
    // 只对**当前槽位**的库做带密钥的一致快照，因为进程里只有当前槽的 DEK。
    // 非当前槽不能用当前密钥试开；无 WAL / 热日志且复制前后完全相同时才保留
    // 原始密文副本，否则明确失败，绝不静默丢掉另一槽已提交但还在 WAL 里的数据。
    let current_db = crate::storage::manuscript_db_path().ok();
    let has_key_handle = crate::vault::key_handle().is_some();
    let mut snapshot_names = std::collections::HashSet::new();
    for name in ["manuscripts.db", "alt-manuscripts.db"] {
        let source = config_dir.join(name);
        if !source.is_file() {
            continue;
        }
        let is_current = current_db.as_deref() == Some(source.as_path());
        let snapshot = snapshot_dir.path().join(name);
        if !has_key_handle || is_current {
            snapshot_database(&source, &snapshot)?;
        } else {
            copy_quiescent_database(&source, &snapshot)?;
        }
        snapshot_names.insert(name.to_string());
    }
    let files = collect_files(config_dir)?;
    // 先写同目录临时文件，写完再原子替换目标：导出中途失败不会留下半个 ZIP。
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("无法在 {} 创建临时文件", parent.display()))?;
    {
        let mut zip = zip::ZipWriter::new(temp.as_file_mut());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for rel in &files {
            let name = rel.to_string_lossy().into_owned();
            let source = if snapshot_names.contains(&name) {
                snapshot_dir.path().join(rel)
            } else {
                config_dir.join(rel)
            };
            let name = zip_entry_name(rel);
            let bytes = match fs::read(&source) {
                Ok(bytes) => bytes,
                // 普通文件在遍历与读取之间被删掉时跳过；数据库快照必须存在，
                // 缺失或其他读取错误都应中止导出。
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && rel != Path::new("manuscripts.db")
                        && rel != Path::new("alt-manuscripts.db") =>
                {
                    continue;
                }
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

/// 一边遍历条目一边记 ZIP 炸弹的账。`inspect_backup` 与 `extract_all` 两条
/// 路径共用同一套口径：前者在动当前数据之前拒绝，后者防「校验后文件被换掉」
/// 与伪造头部的尺寸。
#[derive(Default)]
struct ZipBudget {
    entries: usize,
    total_bytes: u64,
}

impl ZipBudget {
    /// 记一个条目；`size` 是 ZIP 里声明的解压后尺寸。
    ///
    /// 声明的尺寸不可信（可以伪造），因此解压时还会按实际读出的字节再卡一次
    /// 单条上限（见 [`copy_capped`]）。
    fn account(&mut self, name: &str, size: u64) -> Result<()> {
        self.entries += 1;
        if self.entries > MAX_BACKUP_ENTRIES {
            bail!(
                "备份条目太多（超过上限 {MAX_BACKUP_ENTRIES} 个），可能是构造出来的压缩包，已拒绝恢复"
            );
        }
        if size > MAX_BACKUP_ENTRY_BYTES {
            bail!(
                "备份条目 {name} 解压后声明为 {}，超过单条 {} 上限，已拒绝恢复",
                format_bytes(size),
                format_bytes(MAX_BACKUP_ENTRY_BYTES)
            );
        }
        self.total_bytes = self.total_bytes.saturating_add(size);
        if self.total_bytes > MAX_BACKUP_TOTAL_BYTES {
            bail!(
                "备份解压后总量超过 {} 上限，可能是构造出来的压缩包，已拒绝恢复",
                format_bytes(MAX_BACKUP_TOTAL_BYTES)
            );
        }
        Ok(())
    }
}

/// 校验 ZIP 结构并给出条目概要。至少含 `config.json` 或 `manuscripts.db` 之一
/// 才算公文助手备份；条目名不安全、条目数或解压尺寸超过上限时直接拒绝。
pub(crate) fn inspect_backup(zip_path: &Path) -> Result<BackupInfo> {
    let mut archive = open_archive(zip_path)?;
    // 先按条目数一眼挡掉：几万个条目的炸弹包不必逐个解析。
    if archive.len() > MAX_BACKUP_ENTRIES {
        bail!(
            "备份条目太多（{} 个，超过上限 {MAX_BACKUP_ENTRIES} 个），可能是构造出来的压缩包，已拒绝恢复",
            archive.len()
        );
    }
    let mut info = BackupInfo::default();
    let mut groups: BTreeMap<String, BackupGroup> = BTreeMap::new();
    let mut budget = ZipBudget::default();
    let mut seen = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .with_context(|| format!("无法读取备份条目 #{index}"))?;
        let rel = safe_relative_path(entry.name())?;
        if !seen.insert(rel.clone()) {
            bail!("备份里包含重复条目，已拒绝：{}", entry.name());
        }
        budget.account(entry.name(), entry.size())?;
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
        if matches!(
            rel.to_str(),
            Some("manuscripts.db" | "alt-manuscripts.db" | "config.json" | "alt-config.json")
        ) {
            let mut head = Vec::new();
            // 概要里的明文提示是尽力探测；尺寸伪造或损坏条目仍由上面的预算与
            // 恢复时的实际解压校验拒绝，不能在这里提前变成另一种错误。
            if entry.take(16).read_to_end(&mut head).is_ok() {
                let plaintext_db = rel
                    .to_str()
                    .is_some_and(|name| name.ends_with("manuscripts.db"))
                    && head == b"SQLite format 3\0";
                let plaintext_config = rel
                    .to_str()
                    .is_some_and(|name| name.ends_with("config.json"))
                    && head
                        .iter()
                        .copied()
                        .find(|byte| !byte.is_ascii_whitespace())
                        == Some(b'{');
                info.has_plaintext_data |= plaintext_db || plaintext_config;
            }
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
/// 不可信 ZIP 始终先解到配置目录同级的隔离暂存目录；结构、大小、数据库与锁封套
/// 全部通过后，才把现有配置目录改名为 `GongwenAssistant-恢复前备份-<时间戳>` 并换入。
/// 换入失败会改回原目录，校验失败则只清理暂存目录，现有数据完全不动。
pub(crate) fn restore_backup(zip_path: &Path, config_dir: &Path) -> Result<RestoreReport> {
    inspect_backup(zip_path)?;
    let mut archive = open_archive(zip_path)?;
    let parent = config_dir.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("无法创建恢复暂存目录的父目录：{}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(RESTORE_STAGING_PREFIX)
        .tempdir_in(parent)
        .context("无法创建隔离恢复暂存目录")?;
    #[cfg(unix)]
    restrict_permissions(staging.path());

    let restored_files = extract_all(&mut archive, staging.path()).context("备份解压校验失败")?;
    drop(archive);
    validate_staged_backup(staging.path()).context("备份内容校验失败")?;
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
    // 旧目录已成功留底，接下来由显式改名负责暂存内容；TempDir 不再自动删除。
    let staged = staging.keep();

    if let Err(error) = fs::rename(&staged, config_dir) {
        let rollback = match &backup_dir {
            Some(backup_dir) => fs::rename(backup_dir, config_dir).with_context(|| {
                format!(
                    "无法把留底目录 {} 改回 {}",
                    backup_dir.display(),
                    config_dir.display()
                )
            }),
            None => Ok(()),
        };
        let error = anyhow::Error::new(error).context(format!(
            "无法把已校验的暂存目录 {} 换入 {}",
            staged.display(),
            config_dir.display()
        ));
        return match rollback {
            Ok(()) => {
                if backup_dir.is_none() {
                    let _ = fs::remove_dir_all(&staged);
                }
                Err(error.context("恢复失败，已还原恢复前的数据"))
            }
            Err(rollback_error) => Err(error.context(format!(
                "恢复前数据仍留在 {}，已校验的新数据仍留在 {}，请手工改回；{rollback_error:#}",
                backup_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                staged.display()
            ))),
        };
    }

    Ok(RestoreReport {
        backup_dir,
        restored_files,
    })
}

/// 恢复时只解析锁封套的公开磁盘结构，不借用当前机器的 DEK 试开备份。
#[derive(serde::Deserialize)]
struct BackupVault {
    v: u32,
    slots: [BackupVaultSlot; 2],
}

#[derive(serde::Deserialize)]
struct BackupVaultSlot {
    salt: String,
    seal: String,
    #[serde(default)]
    wrapped: String,
}

impl BackupVaultSlot {
    fn is_empty(&self) -> bool {
        self.salt.is_empty() && self.seal.is_empty() && self.wrapped.is_empty()
    }

    fn has_unwrap_key(&self) -> bool {
        !self.wrapped.is_empty()
    }

    fn validate(&self, slot: usize, version: u32) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let valid_hex = |value: &str, bytes: usize| {
            value.len() == bytes * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        };
        if !valid_hex(&self.salt, 16) || !valid_hex(&self.seal, 49) {
            bail!("锁文件槽位 {slot} 的盐或验证器结构损坏");
        }
        // v1 是应用锁开发期的明文时代格式，没有 DEK 包裹；v2 必须完整。
        if (version == 2 && !valid_hex(&self.wrapped, 60))
            || (!self.wrapped.is_empty() && !valid_hex(&self.wrapped, 60))
        {
            bail!("锁文件槽位 {slot} 的数据密钥包裹结构损坏");
        }
        Ok(())
    }
}

fn load_backup_vault(root: &Path) -> Result<Option<BackupVault>> {
    let path = root.join("vault.dat");
    if !path.is_file() {
        return Ok(None);
    }
    if fs::metadata(&path)?.len() > 64 * 1024 {
        bail!("锁文件 vault.dat 体积异常，已拒绝恢复");
    }
    let raw = fs::read(&path).context("无法读取备份里的锁文件 vault.dat")?;
    let vault: BackupVault = serde_json::from_slice(&raw).context("锁文件 vault.dat 内容损坏")?;
    if vault.v != 1 && vault.v != 2 {
        bail!("锁文件 vault.dat 的版本不识别");
    }
    for (index, slot) in vault.slots.iter().enumerate() {
        slot.validate(index, vault.v)?;
    }
    Ok(Some(vault))
}

fn require_slot_key(vault: Option<&BackupVault>, slot: usize, name: &str) -> Result<()> {
    let Some(slot_data) = vault.map(|vault| &vault.slots[slot]) else {
        bail!("{name} 是加密数据，但备份缺少 vault.dat，无法携带对应槽位的数据密钥");
    };
    if slot_data.is_empty() || !slot_data.has_unwrap_key() {
        bail!("{name} 是加密数据，但锁文件槽位 {slot} 没有可用的数据密钥包裹");
    }
    Ok(())
}

/// 明文库可以在隔离目录里真正打开并做完整性检查；旧包的 WAL 也在这里由
/// SQLite 自己恢复、合并。连接关闭后仍留有非空日志则拒绝换入。
fn validate_plain_database(path: &Path, name: &str) -> Result<()> {
    let connection = rusqlite::Connection::open(path)
        .with_context(|| format!("无法打开备份里的明文数据库 {name}"))?;
    connection
        .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
        .with_context(|| format!("备份数据库 {name} 不可读"))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .with_context(|| format!("无法检查备份数据库 {name} 的完整性"))?;
    if integrity != "ok" {
        bail!("备份数据库 {name} 完整性检查未通过：{integrity}");
    }
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .with_context(|| format!("无法合并备份数据库 {name} 的 WAL"))?;
    drop(connection);
    for suffix in ["-wal", "-journal"] {
        let sidecar = sibling_with_suffix(path, suffix);
        if sidecar.is_file() {
            if fs::metadata(&sidecar)?.len() != 0 {
                bail!("备份数据库 {name} 校验后仍有未合并日志，已拒绝恢复");
            }
            fs::remove_file(&sidecar)?;
        }
    }
    let shm = sibling_with_suffix(path, "-shm");
    if shm.is_file() {
        fs::remove_file(&shm)?;
    }
    Ok(())
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn validate_staged_backup(root: &Path) -> Result<()> {
    let vault = load_backup_vault(root)?;
    for (name, slot) in [("manuscripts.db", 0), ("alt-manuscripts.db", 1)] {
        let path = root.join(name);
        if !path.is_file() {
            continue;
        }
        let mut file = fs::File::open(&path)?;
        let mut head = [0u8; 16];
        let read = file.read(&mut head)?;
        let plaintext = read == 0 || (read == head.len() && head == *b"SQLite format 3\0");
        if plaintext {
            validate_plain_database(&path, name)?;
            continue;
        }
        require_slot_key(vault.as_ref(), slot, name)?;
        // 没有备份密码就不能解释密文 WAL；直接换入会把未经证明的一致集当真。
        for suffix in ["-wal", "-journal"] {
            let sidecar = sibling_with_suffix(&path, suffix);
            if sidecar.is_file() && fs::metadata(&sidecar)?.len() != 0 {
                bail!(
                    "加密数据库 {name} 带有无法在暂存阶段校验的日志；请使用由应用正常导出的完整备份"
                );
            }
        }
    }

    for slot in 0..2 {
        let prefix = if slot == 0 { "" } else { "alt-" };
        for base in crate::storage::SEALED_FILE_NAMES {
            let name = format!("{prefix}{base}");
            let path = root.join(&name);
            if !path.is_file() {
                continue;
            }
            let raw = fs::read(&path)?;
            if !crate::vault::is_sealed_file(&raw) && base == "config.json" {
                let value: serde_json::Value = serde_json::from_slice(&raw)
                    .with_context(|| format!("明文配置 {name} 不是有效 JSON"))?;
                if !value.is_object() {
                    bail!("明文配置 {name} 结构无效");
                }
            }
        }
    }

    // 动态资源也要核对密钥封套：Pi、个人词表、接口与技能都不在固定文件名单里。
    // 没有备份密码时不能认证内容，但至少不能换入一份永远解不开的孤立密文。
    for relative in collect_files(root)? {
        let path = root.join(&relative);
        let mut file = fs::File::open(&path)?;
        let mut head = [0u8; 4];
        let read = file.read(&mut head)?;
        if read == head.len() && crate::vault::is_sealed_file(&head) {
            if fs::metadata(&path)?.len() < crate::vault::SEALED_FILE_OVERHEAD as u64 {
                bail!("密封文件 {} 长度不足，认证数据不完整", relative.display());
            }
            let slot = usize::from(
                relative
                    .components()
                    .next()
                    .is_some_and(|part| part.as_os_str().to_string_lossy().starts_with("alt-")),
            );
            require_slot_key(vault.as_ref(), slot, &relative.to_string_lossy())?;
        }
    }
    Ok(())
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
    // 主库（含诱饵库）已经独立生成一致快照，原库的日志与共享内存不能混进快照。
    if matches!(
        rel.to_str(),
        Some(
            "manuscripts.db-wal"
                | "manuscripts.db-shm"
                | "manuscripts.db-journal"
                | "alt-manuscripts.db-wal"
                | "alt-manuscripts.db-shm"
                | "alt-manuscripts.db-journal"
        )
    ) {
        return true;
    }
    if rel.components().next().is_some_and(|first| {
        let name = first.as_os_str().to_string_lossy();
        // 真实槽与诱饵槽的崩溃日志目录都排除（见 `crash_log::log_dir`）。
        name.eq_ignore_ascii_case(LOG_DIR) || name.eq_ignore_ascii_case(DECOY_LOG_DIR)
    }) {
        return true;
    }
    let Some(name) = rel.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == UNSEEN_CRASH_MARKER
        // 停用加密的待办标记：带着它恢复，下次解锁会自动把数据解成明文并删锁。
        || name == "vault.pending-disable"
        || name.ends_with("-vault.pending-disable")
        || name.contains(CORRUPT_MARK)
        // config.json 的副本（身份迁移前备份）里可能是明文配置与 API 密钥；
        // 恢复旧备份时不该把它带回来绕过现有加密。
        || name.ends_with("身份迁移前备份")
        || name.starts_with(TEMP_PREFIX)
        || name.ends_with(".tmp")
        // 明文⇄密文迁移若被中断可能留下覆写过的旧库残骸，绝不进备份。
        || name.contains(".迁移-")
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
    let mut budget = ZipBudget::default();
    let mut extracted_bytes: u64 = 0;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("无法读取备份条目 #{index}"))?;
        let name = entry.name().to_string();
        let rel = safe_relative_path(&name)?;
        // 与 inspect 同一套上限：校验与解压是两次独立打开文件，不能假设它们
        // 看到的是同一个包；伪造头部的小尺寸也在这里被真正解压时再挡一次。
        budget.account(&name, entry.size())?;
        let dest = target.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&dest)
                .with_context(|| format!("无法创建目录：{}", dest.display()))?;
            #[cfg(unix)]
            restrict_permissions(&dest);
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目录：{}", parent.display()))?;
            // 恢复出来的资源目录（images 等）与配置目录本身都只给当前用户。
            #[cfg(unix)]
            restrict_permissions(parent);
        }
        let mut file = fs::File::create(&dest)
            .with_context(|| format!("无法写入恢复文件：{}", dest.display()))?;
        let written = copy_capped(&mut entry, &mut file, &dest)?;
        extracted_bytes = extracted_bytes.saturating_add(written);
        if extracted_bytes > MAX_BACKUP_TOTAL_BYTES {
            bail!(
                "备份实际解压后总量超过 {} 上限，可能是构造出来的压缩包，已拒绝恢复",
                format_bytes(MAX_BACKUP_TOTAL_BYTES)
            );
        }
        let _ = file.sync_all();
        #[cfg(unix)]
        restrict_permissions(&dest);
        files += 1;
    }
    Ok(files)
}

/// 解压单个条目，边写边卡单条上限。
///
/// ZIP 头里声明的解压尺寸可以伪造（声明很小、deflate 流实际解出很大），所以
/// 不看声明值，按实际写出的字节数判断：最多读 `上限 + 1` 字节，超过就报错。
fn copy_capped(
    entry: &mut zip::read::ZipFile<'_, fs::File>,
    file: &mut fs::File,
    dest: &Path,
) -> Result<u64> {
    let mut limited = entry.take(MAX_BACKUP_ENTRY_BYTES.saturating_add(1));
    let written = std::io::copy(&mut limited, file)
        .with_context(|| format!("写入恢复文件失败：{}", dest.display()))?;
    if written > MAX_BACKUP_ENTRY_BYTES {
        bail!(
            "备份条目实际解压后超过单条 {} 上限，可能是构造出来的压缩包，已拒绝恢复",
            format_bytes(MAX_BACKUP_ENTRY_BYTES)
        );
    }
    Ok(written)
}

/// 用户资源目录：B 阶段起 `images/`、`mermaid-cache/` 里的图片与 Mermaid 图会落成
/// AEAD 密文，恢复后仍是密文，同样只该给当前用户读写。真实槽与诱饵槽各一套名字
/// （`crash_log` 那类槽位映射的目录一律带 `alt-` 前缀）。
const PRIVATE_RESOURCE_DIRS: [&str; 12] = [
    "images",
    "alt-images",
    "mermaid-cache",
    "alt-mermaid-cache",
    "pi",
    "alt-pi",
    "ime",
    "alt-ime",
    "skills",
    "alt-skills",
    "apis",
    "alt-apis",
];

/// 配置与密钥文件、以及用户资源目录里的文件恢复后仍只给当前用户读写。ZIP 里的
/// 权限位不跨平台可信，这里按文件名 / 所在目录重新收紧，和 [`crate::storage::save`]
/// 的口径一致。目录一律 0700。
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    // 恢复包里可能同时有真实槽与诱饵槽的文件；两份都要按同一份敏感名单收紧。
    let plain_name = name.strip_prefix("alt-").unwrap_or(name);
    // 资源目录按「路径里任一段是它」判断：文件本身叫什么都算（`images/图.png`）。
    let in_resource_dir = path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| PRIVATE_RESOURCE_DIRS.contains(&part))
    });
    if crate::storage::SEALED_FILE_NAMES.contains(&plain_name)
        || name.ends_with("身份迁移前备份")
        || in_resource_dir
    {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    if path.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
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
        let connection = rusqlite::Connection::open(root.join("manuscripts.db")).unwrap();
        connection
            .execute_batch("CREATE TABLE sample(value); INSERT INTO sample VALUES ('稿件');")
            .unwrap();
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
        assert_eq!(info.file_count, 7, "只应打包 7 个有效文件");
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
        assert_eq!(report.restored_files, 7);
        assert_eq!(read(&target, "config.json"), br#"{"output_dir":"C:/out"}"#);
        assert!(!target.join("manuscripts.db-wal").exists());
        let restored = rusqlite::Connection::open(target.join("manuscripts.db")).unwrap();
        let value: String = restored
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "稿件");
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
    fn backup_keeps_committed_wal_data_with_a_pinned_reader_and_pending_writer() {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("manuscripts.db");
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.busy_timeout(std::time::Duration::ZERO).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; CREATE TABLE sample(value); \
                 PRAGMA wal_checkpoint(TRUNCATE);",
            )
            .unwrap();
        let reader = rusqlite::Connection::open(&path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM sample;")
            .unwrap();
        writer
            .execute("INSERT INTO sample VALUES ('已提交')", [])
            .unwrap();
        let busy: i64 = writer
            .query_row("PRAGMA wal_checkpoint(FULL)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 1, "读事务应阻止旧方案的 FULL 检查点完成");
        writer
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO sample VALUES ('未提交');")
            .unwrap();

        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("backup.zip");
        export_from(source.path(), &zip).unwrap();
        let mut archive = open_archive(&zip).unwrap();
        assert!(archive.by_name("manuscripts.db-wal").is_err());
        assert!(archive.by_name("manuscripts.db-shm").is_err());
        assert!(archive.by_name("manuscripts.db-journal").is_err());
        drop(archive);
        writer.execute_batch("ROLLBACK;").unwrap();
        reader.execute_batch("COMMIT;").unwrap();
        writer
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();

        let target = out.path().join("restored");
        restore_backup(&zip, &target).unwrap();
        let restored = rusqlite::Connection::open(target.join("manuscripts.db")).unwrap();
        let values: Vec<String> = restored
            .prepare("SELECT value FROM sample")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(values, ["已提交"]);
        let integrity: String = restored
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    struct TestVaultState;

    impl Drop for TestVaultState {
        fn drop(&mut self) {
            crate::storage::set_test_config_dir(None);
            crate::vault::clear_key_handle();
        }
    }

    fn use_plain_current_slot_with_lock_handle(config_dir: &Path) -> TestVaultState {
        crate::storage::set_test_config_dir(Some(config_dir.to_path_buf()));
        // 只让备份进入「有第二槽、非当前槽不能用当前 DEK 试开」的分支；
        // 测试库仍是明文，避免依赖真实配置目录或真实密码。
        crate::vault::set_key_handle([41; 32], false);
        TestVaultState
    }

    #[test]
    fn non_current_slot_with_pending_wal_aborts_instead_of_copying_only_main_db() {
        let source = tempfile::tempdir().unwrap();
        fake_config_dir(source.path());
        let _state = use_plain_current_slot_with_lock_handle(source.path());
        let alt = source.path().join("alt-manuscripts.db");
        let writer = rusqlite::Connection::open(&alt).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; CREATE TABLE sample(value); \
                 PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO sample VALUES ('另一槽已提交');",
            )
            .unwrap();
        assert!(sibling_with_suffix(&alt, "-wal").metadata().unwrap().len() > 0);

        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("backup.zip");
        fs::write(&zip, b"previous backup").unwrap();
        let error = export_from(source.path(), &zip).unwrap_err().to_string();
        assert!(
            error.contains("WAL") && error.contains("切换到对应槽位"),
            "{error}"
        );
        assert_eq!(fs::read(&zip).unwrap(), b"previous backup");

        // 正常关闭对应槽后 WAL 已合并；此时同一目录可以完整备份并恢复另一槽。
        drop(writer);
        export_from(source.path(), &zip).unwrap();
        let target = out.path().join("restored");
        restore_backup(&zip, &target).unwrap();
        let restored = rusqlite::Connection::open(target.join("alt-manuscripts.db")).unwrap();
        let value: String = restored
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "另一槽已提交");
    }

    #[test]
    fn current_slot_keeps_keyed_snapshot_and_clean_other_slot_restores() {
        let source = tempfile::tempdir().unwrap();
        fake_config_dir(source.path());
        crate::storage::set_test_config_dir(Some(source.path().to_path_buf()));
        let _state = TestVaultState;
        let alt = rusqlite::Connection::open(source.path().join("alt-manuscripts.db")).unwrap();
        alt.execute_batch("CREATE TABLE sample(value); INSERT INTO sample VALUES ('另一槽');")
            .unwrap();
        drop(alt);

        let (_vault, unlocked) =
            crate::vault::Vault::create("主密码aaaa", Some("第二密码bbbb"), false).unwrap();
        let unlocked = unlocked.unwrap();
        crate::vault::set_key_handle(*unlocked.dek, false);
        crate::vault::ensure_slot_encrypted().unwrap();
        crate::vault::activate_db_encryption();

        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("encrypted.zip");
        export_from(source.path(), &zip).unwrap();
        let target = out.path().join("restored");
        restore_backup(&zip, &target).unwrap();

        let restored = rusqlite::Connection::open(target.join("manuscripts.db")).unwrap();
        crate::storage::apply_db_key(&restored).unwrap();
        let value: String = restored
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "稿件");
        let alt = rusqlite::Connection::open(target.join("alt-manuscripts.db")).unwrap();
        let value: String = alt
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "另一槽");
    }

    #[test]
    fn staged_dynamic_sealed_resource_requires_its_slot_key() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("orphan-pi.zip");
        {
            let file = fs::File::create(&zip).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let mut sealed = b"GWA2".to_vec();
            sealed.resize(crate::vault::SEALED_FILE_OVERHEAD, 0);
            for (name, bytes) in [
                ("config.json", b"{}".to_vec()),
                ("pi/workspaces/doc-1/run-1/run.json", sealed),
            ] {
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(&bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        let target = dir.path().join("config");
        let error = format!("{:#}", restore_backup(&zip, &target).unwrap_err());
        assert!(error.contains("缺少 vault.dat"), "{error}");
        assert!(!target.exists(), "校验失败不能换入任何数据");
    }

    #[test]
    fn staged_database_validation_failure_preserves_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("broken.zip");
        {
            let file = fs::File::create(&zip).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            for (name, bytes) in [
                ("config.json", b"{}".as_slice()),
                ("manuscripts.db", b"not a sqlite database".as_slice()),
            ] {
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config");
        write(&target, "config.json", br#"{"kept":true}"#);
        write(&target, "keep.txt", b"keep");

        let error = restore_backup(&zip, &target).unwrap_err().to_string();
        assert!(error.contains("备份内容校验失败"), "{error}");
        assert_eq!(read(&target, "keep.txt"), b"keep");
        assert_eq!(read(&target, "config.json"), br#"{"kept":true}"#);
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            1,
            "失败不能留下留底或隔离暂存目录"
        );
    }

    #[test]
    fn a_broken_database_aborts_export_without_replacing_the_previous_backup() {
        let source = tempfile::tempdir().unwrap();
        write(source.path(), "manuscripts.db", b"broken database");
        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("backup.zip");
        fs::write(&zip, b"previous backup").unwrap();
        assert!(export_from(source.path(), &zip).is_err());
        assert_eq!(fs::read(&zip).unwrap(), b"previous backup");
        assert_eq!(read(source.path(), "manuscripts.db"), b"broken database");
        assert_eq!(fs::read_dir(out.path()).unwrap().count(), 1);
    }

    #[test]
    fn restores_legacy_backups_that_carry_a_database_and_its_wal() {
        let source = tempfile::tempdir().unwrap();
        let database = source.path().join("manuscripts.db");
        let writer = rusqlite::Connection::open(&database).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL; CREATE TABLE sample(value); \
                 PRAGMA wal_checkpoint(TRUNCATE); INSERT INTO sample VALUES ('旧包稿件');",
            )
            .unwrap();
        let out = tempfile::tempdir().unwrap();
        let zip = out.path().join("legacy.zip");
        let mut archive = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
        for name in ["manuscripts.db", "manuscripts.db-wal"] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(&read(source.path(), name)).unwrap();
        }
        archive.finish().unwrap();
        let info = inspect_backup(&zip).unwrap();
        assert!(info.has_plaintext_data);
        assert!(
            info.summary_lines()
                .iter()
                .any(|line| line.contains("旧明文备份"))
        );
        let target = out.path().join("restored");
        restore_backup(&zip, &target).unwrap();
        let restored = rusqlite::Connection::open(target.join("manuscripts.db")).unwrap();
        let value: String = restored
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "旧包稿件");
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
    /// 停用标记与配置副本绝不能进备份：带回去恢复会让下次解锁自动解密并删锁。
    #[test]
    fn backup_excludes_disable_marker_and_config_copies() {
        assert!(is_excluded(Path::new("vault.pending-disable")));
        assert!(is_excluded(Path::new("alt-vault.pending-disable")));
        assert!(is_excluded(Path::new("config.json.身份迁移前备份")));
        assert!(is_excluded(Path::new("alt-config.json.身份迁移前备份")));
        assert!(!is_excluded(Path::new("config.json")));
        assert!(!is_excluded(Path::new("manuscripts.db")));
    }

    /// 两个槽位的崩溃日志目录都不进备份（诱饵槽是 `alt-logs/`）。
    #[test]
    fn both_slots_crash_log_dirs_are_excluded() {
        assert!(is_excluded(Path::new("logs/crash-20260101-000000.log")));
        assert!(is_excluded(Path::new("alt-logs/crash-20260101-000000.log")));
        assert!(!is_excluded(Path::new("images/a.png")));
    }

    /// 手工拼一个 ZIP：条目用 stored，头部声明的**解压尺寸**由调用方填，可以
    /// 随意写大（真实数据只有几个字节），用来模拟「声明尺寸像炸弹」的备份包。
    fn write_zip_with_declared_sizes(path: &Path, entries: &[(&str, &[u8], u32)]) {
        fn u16(bytes: &mut Vec<u8>, value: u16) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fn u32(bytes: &mut Vec<u8>, value: u32) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        let mut body = Vec::new();
        let mut central = Vec::new();
        for (name, data, declared) in entries {
            let name = name.as_bytes();
            let offset = body.len() as u32;
            // 本地文件头。
            u32(&mut body, 0x0403_4b50);
            u16(&mut body, 20); // 解压所需版本
            u16(&mut body, 0); // 标志
            u16(&mut body, 0); // 压缩方法：stored
            u16(&mut body, 0); // 时间
            u16(&mut body, 0); // 日期
            u32(&mut body, 0); // CRC（不读数据就不校验）
            u32(&mut body, data.len() as u32);
            u32(&mut body, *declared);
            u16(&mut body, name.len() as u16);
            u16(&mut body, 0); // 扩展字段
            body.extend_from_slice(name);
            body.extend_from_slice(data);

            // 中央目录记录。
            u32(&mut central, 0x0201_4b50);
            u16(&mut central, 20); // 制作版本
            u16(&mut central, 20); // 解压所需版本
            u16(&mut central, 0); // 标志
            u16(&mut central, 0); // 压缩方法
            u16(&mut central, 0); // 时间
            u16(&mut central, 0); // 日期
            u32(&mut central, 0); // CRC
            u32(&mut central, data.len() as u32);
            u32(&mut central, *declared);
            u16(&mut central, name.len() as u16);
            u16(&mut central, 0); // 扩展字段
            u16(&mut central, 0); // 注释
            u16(&mut central, 0); // 起始磁盘
            u16(&mut central, 0); // 内部属性
            u32(&mut central, 0); // 外部属性
            u32(&mut central, offset);
            central.extend_from_slice(name);
        }
        let mut zip = body;
        let cd_offset = zip.len() as u32;
        zip.extend_from_slice(&central);
        u32(&mut zip, 0x0605_4b50); // 中央目录结束记录
        u16(&mut zip, 0); // 磁盘号
        u16(&mut zip, 0); // 中央目录所在磁盘
        u16(&mut zip, entries.len() as u16);
        u16(&mut zip, entries.len() as u16);
        u32(&mut zip, central.len() as u32);
        u32(&mut zip, cd_offset);
        u16(&mut zip, 0); // 注释长度
        fs::write(path, zip).unwrap();
    }

    /// 声明尺寸超单条上限 / 整包总量上限时，校验与恢复都拒绝，且不动当前数据。
    #[test]
    fn declared_bomb_sizes_are_rejected_before_touching_current_data() {
        let dir = tempfile::tempdir().unwrap();

        // 单条 3 GiB，超过 2 GiB 的单条上限。
        let single = dir.path().join("single.zip");
        write_zip_with_declared_sizes(&single, &[("config.json", b"{}", 3 * 1024 * 1024 * 1024)]);
        let error = inspect_backup(&single).unwrap_err().to_string();
        assert!(
            error.contains("单条") && error.contains("上限"),
            "应说清是单条上限：{error}"
        );

        // 每条 2 GiB 不超单条上限，但 5 条合计 10 GiB，超过 8 GiB 的整包上限。
        let total = dir.path().join("total.zip");
        write_zip_with_declared_sizes(
            &total,
            &[
                ("config.json", b"{}", 2 * 1024 * 1024 * 1024),
                ("manuscripts.db", b"", 2 * 1024 * 1024 * 1024),
                ("images/a.png", b"", 2 * 1024 * 1024 * 1024),
                ("images/b.png", b"", 2 * 1024 * 1024 * 1024),
                ("images/c.png", b"", 2 * 1024 * 1024 * 1024),
            ],
        );
        let error = inspect_backup(&total).unwrap_err().to_string();
        assert!(
            error.contains("总量") && error.contains("上限"),
            "应说清是整包总量上限：{error}"
        );

        // 超限包绝不能改动当前配置目录，也不留「恢复前备份」目录。
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config");
        write(&target, "keep.txt", b"keep");
        for zip_path in [&single, &total] {
            assert!(restore_backup(zip_path, &target).is_err());
            assert_eq!(read(&target, "keep.txt"), b"keep", "超限包不能改动当前数据");
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }

    /// 条目数上限：计数与报错文案由 [`ZipBudget`] 负责（`inspect_backup` 先用
    /// `archive.len()` 一眼挡掉，解压侧再用同一套账兜底）。
    #[test]
    fn too_many_entries_are_rejected() {
        let mut budget = ZipBudget::default();
        for _ in 0..MAX_BACKUP_ENTRIES {
            budget.account("条目", 0).unwrap();
        }
        let error = budget.account("条目", 0).unwrap_err().to_string();
        assert!(
            error.contains("条目太多") && error.contains("上限"),
            "{error}"
        );
    }

    /// 用户资源目录（images / mermaid-cache，含诱饵槽的 alt- 前缀）恢复后
    /// 文件 0600、目录 0700：B 阶段起它们也是密文资源。
    #[cfg(unix)]
    #[test]
    fn restored_resource_files_and_dirs_are_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("resources.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            for (name, bytes) in [
                ("config.json", b"{}".as_slice()),
                ("images/图.png", b"png".as_slice()),
                ("alt-images/图.png", b"png".as_slice()),
                ("mermaid-cache/图.svg", b"svg".as_slice()),
                ("alt-mermaid-cache/图.svg", b"svg".as_slice()),
                ("pi/workspaces/doc/run/run.json", b"{}".as_slice()),
                ("alt-ime/tables.json", b"{}".as_slice()),
            ] {
                writer
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }

        let target = dir.path().join("config");
        restore_backup(&zip_path, &target).unwrap();
        let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for rel in [
            "images/图.png",
            "alt-images/图.png",
            "mermaid-cache/图.svg",
            "alt-mermaid-cache/图.svg",
            "pi/workspaces/doc/run/run.json",
            "alt-ime/tables.json",
        ] {
            assert_eq!(mode(target.join(rel)), 0o600, "{rel} 应只有本人可读写");
        }
        for rel in [
            "images",
            "alt-images",
            "mermaid-cache",
            "alt-mermaid-cache",
            "pi/workspaces/doc/run",
            "alt-ime",
        ] {
            assert_eq!(mode(target.join(rel)), 0o700, "{rel}/ 应只有本人可读写");
        }

        // 直接调用权限函数也覆盖真实槽与诱饵槽两套名字。
        let loose = target.join("images").join("再存一张.png");
        fs::write(&loose, b"png").unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o644)).unwrap();
        restrict_permissions(&loose);
        assert_eq!(mode(loose), 0o600);
    }
}
