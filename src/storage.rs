use crate::models::AppConfig;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

const ZIP_PASSWORD_FILE: &str = ".zip-password";
const MAX_REMEMBERED_PASSWORD_BYTES: u64 = 1024;

/// 应用锁启用后必须整份 AEAD 密封的文件。**这是唯一一份名单**：解锁迁移
/// （`vault::ensure_slot_encrypted` / `slot_needs_encryption` / `decrypt_slot_files`）、
/// 擦除（[`wipe_slot_files`]）与读写侧（`ai_services::persistence`）都从这里派生。
/// 名单漏一项的后果是「启用加密后该文件永久明文」——二期曾漏掉 `api-envs.json`
/// 与 `api-tests.json`，`styles.json` 里还有样稿原文片段，所以统一放这里。
pub(crate) const SEALED_FILE_NAMES: [&str; 9] = [
    "config.json",
    // 旧版（v0.8 之前）数据接口配置曾写在根目录 `apis.json`；新版只在打开
    // 「数据接口」页时才把它迁成 `apis/services.json`。它含内网地址、请求头与
    // 参数说明，必须和应用锁一样受保护，否则升级后可能永久留在磁盘上明文。
    "apis.json",
    ZIP_PASSWORD_FILE,
    ".export-password",
    "api-secrets.json",
    "api-envs.json",
    "api-tests.json",
    "styles.json",
    "metrics.json",
];

/// GUI Mermaid 缓存目录名（相对配置目录）。目录内的图与警告 sidecar 都是用户正文
/// 派生物，按槽位隔离并随资源一起加密。
pub(crate) const MERMAID_CACHE_DIR: &str = "mermaid-cache";

/// [`SEALED_FILE_NAMES`] 里的 JSON 子集：`ai_services::persistence` 的读写名单。
pub(crate) const SEALED_JSON_NAMES: [&str; 6] = [
    "apis.json",
    "api-secrets.json",
    "api-envs.json",
    "api-tests.json",
    "styles.json",
    "metrics.json",
];

/// 数据槽位：真实系统与诱饵系统（第二密码进入的空白系统）各有一套独立文件。
/// 解锁时由闸门设置；默认值是真实槽位，测试与未启用应用锁的环境不受影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSlot {
    /// 真实系统：沿用配置目录原有文件名（config.json、manuscripts.db…）。
    Real,
    /// 诱饵系统：同一目录下一套 `alt-` 前缀的中性文件名，磁盘上看不出语义。
    Decoy,
}

/// 当前进程打开的是哪个槽位。后台线程也会读路径（后台任务、同步导入），
/// 用原子量而不是线程局部；解锁与锁定时切换。
static CURRENT_SLOT: AtomicU8 = AtomicU8::new(0);

pub fn current_slot() -> DataSlot {
    match CURRENT_SLOT.load(Ordering::Relaxed) {
        1 => DataSlot::Decoy,
        _ => DataSlot::Real,
    }
}

/// 解锁成功时由闸门调用；锁定时回到真实槽位（中性状态）。
pub fn set_current_slot(slot: DataSlot) {
    CURRENT_SLOT.store(
        match slot {
            DataSlot::Real => 0,
            DataSlot::Decoy => 1,
        },
        Ordering::Relaxed,
    );
}

/// 诱饵槽位文件名前缀。中性、不带语义；第二期 SQLCipher 落地后文件内容本身
/// 已是密文，前缀只是第一道皮。注意：文件名映射是唯一入口，诱饵系统里任何
/// 写入都不能落进真实文件——新增敏感文件时一律走 [`slot_file`] / [`slot_subdir`]。
pub(crate) const DECOY_PREFIX: &str = "alt-";

/// 按当前槽位映射配置目录里的文件名。真实槽位用原名；诱饵槽位一律加前缀，
/// 包括未来新增的未知文件名（规则统一，不逐个点名）。
/// 按指定槽位映射配置目录里的文件名。`slot_file` 用当前槽位，本函数
/// 让「擦除另一个槽位」这类跨槽位操作不依赖进程状态。
pub fn slot_file_for(slot: DataSlot, name: &str) -> Result<PathBuf> {
    let name = match slot {
        DataSlot::Real => name.to_string(),
        DataSlot::Decoy => format!("{DECOY_PREFIX}{name}"),
    };
    Ok(config_dir()?.join(name))
}

pub fn slot_file(name: &str) -> Result<PathBuf> {
    slot_file_for(current_slot(), name)
}

/// 把一个配置目录条目名映射到指定槽位的「相对名」：真实槽只认不带 `alt-`
/// 前缀的文件，诱饵槽只认带前缀的。跨槽的文件绝不能被另一套 DEK 加密或擦除
/// （否则诱饵迁移会用诱饵密钥把真实槽的 config 副本封掉）。
pub(crate) fn slot_relative_name(name: &str, slot: DataSlot) -> Option<&str> {
    match slot {
        DataSlot::Real => (!name.starts_with(DECOY_PREFIX)).then_some(name),
        DataSlot::Decoy => name.strip_prefix(DECOY_PREFIX),
    }
}

/// 按当前槽位映射配置目录里的子目录（pi、apis 这类整体敏感的目录）。
pub fn slot_subdir(name: &str) -> Result<PathBuf> {
    slot_file(name)
}

/// 按指定槽位映射子目录，供不依赖进程状态的调用方（测试、擦除）使用。
pub fn slot_subdir_for(slot: DataSlot, name: &str) -> Result<PathBuf> {
    slot_file_for(slot, name)
}

/// 擦掉一个槽位的全部数据文件（库、配置、密钥文件、运行目录）。
/// 用于「清除第二密码」「启用时重置诱饵」：旧 DEK 已丢弃的文件本来就
/// 不可读，物理删除不留残留。目录整棵删掉；删不掉的单个文件尽力而为。
pub(crate) fn wipe_slot_files(slot: DataSlot) {
    // 单文件和库的 WAL/SHM 一起删；`manuscripts.db` 另有 `-wal/-shm` 后缀。
    const FILES: [&str; 10] = [
        "manuscripts.db",
        "config.json",
        "apis.json",
        ".zip-password",
        ".export-password",
        "api-secrets.json",
        "api-envs.json",
        "api-tests.json",
        "styles.json",
        "metrics.json",
    ];
    // 目录列表必须与 vault 的迁移/停用解密/锁文件丢失探测保持一致；`images` 在
    // B 阶段前仍是共享目录（`alt-images` 现在才成为诱饵槽的图片目录）。
    // **`logs` 不能漏**：诱饵槽的崩溃日志同样由旧 DEK 加密，漏删会在停用应用锁
    // 后留下「没有密钥的密文」，重启即被判成锁文件丢失。
    const DIRS: [&str; 7] = [
        "pi",
        "apis",
        "images",
        "mermaid-cache",
        "skills",
        "ime",
        "logs",
    ];
    if let Ok(dir) = config_dir() {
        for name in FILES {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(dir.join(format!(
                    "{}{suffix}",
                    match slot {
                        DataSlot::Real => name.to_string(),
                        DataSlot::Decoy => format!("{DECOY_PREFIX}{name}"),
                    }
                )));
            }
        }
        // 「身份迁移前备份」「损坏配置留底」是 config.json 的副本，同样属于槽位数据，
        // 不能让它们在清除诱饵时活下来（副本里可能是明文或旧密钥能解的配置）。
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if slot_relative_name(&name, slot).is_some_and(|name| {
                    name.starts_with("config.json.")
                        // 数据库迁移中断留下的旧库/临时库（`manuscripts.db.迁移-*`）：
                        // 旧 DEK 的密文残留同样会让停用残留检查失败。
                        || name.contains(".迁移-")
                }) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        for name in DIRS {
            let path = match slot {
                DataSlot::Real => dir.join(name),
                DataSlot::Decoy => dir.join(format!("{DECOY_PREFIX}{name}")),
            };
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }
}

/// 加密模式下暂停配置目录写入的开关：锁定流程先置 true，再丢弃工作区，
/// 这样被丢弃工作区里仍在收尾的线程即使晚到，也不会以「无密钥」状态把
/// 明文写回配置目录（文档要求先停写、再释放句柄）。迁移/停用任务在
/// `begin_work` 里重新放行——那时槽位与密钥句柄都已明确。
///
/// 生产构建里它是**进程级**的：要挡住的正是被丢弃工作区里那些跑在别的线程上、
/// 收尾晚到的写操作，必须让所有线程都看见。
///
/// 测试构建里改成**线程局部**：测试并行跑，一个测试走锁定流程（置 true）
/// 会让同时在跑的别的测试存盘失败——与 [`TEST_CONFIG_DIR`] 同样的坑，
/// 按同样的办法处理（见那里注释）。生产语义不受影响。
#[cfg(not(test))]
static WRITES_BLOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static WRITES_BLOCKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn set_writes_blocked(blocked: bool) {
    #[cfg(not(test))]
    WRITES_BLOCKED.store(blocked, std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    WRITES_BLOCKED.with(|flag| flag.set(blocked));
}

/// 只给「槽位私有且依赖密钥」的写路径用；不拦用户明确选择导出到输出目录的文件。
pub(crate) fn writes_blocked() -> bool {
    #[cfg(not(test))]
    {
        WRITES_BLOCKED.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(test)]
    WRITES_BLOCKED.with(|flag| flag.get())
}

// 测试覆盖的用户目录：只让稿件包同步测试把图片写进临时目录，不影响真实配置目录。
// 按线程覆盖：测试并行跑，全局覆盖会让同时在跑的别的测试（如稿件删除时清理孤儿
// 图片）读到另一个目录，时灵时不灵。
#[cfg(test)]
thread_local! {
    static TEST_CONFIG_DIR: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[cfg(test)]
pub fn set_test_config_dir(dir: Option<std::path::PathBuf>) {
    TEST_CONFIG_DIR.with(|cell| *cell.borrow_mut() = dir);
}

/// 当前测试线程是否显式指定了配置目录。
#[cfg(test)]
pub(crate) fn has_test_config_dir() -> bool {
    TEST_CONFIG_DIR.with(|cell| cell.borrow().is_some())
}

/// 测试用：把父线程解析出的配置目录**带到另一个线程**。
///
/// [`set_test_config_dir`] 写的是线程局部量，后台线程（如加密迁移的搬运线程）
/// 看不到，会退回**真实**配置目录——测试里那就等于去动用户的真实数据。
/// 搬运线程启动时调一次本函数（传入父线程拿到的目录），把目录带过去。
#[cfg(test)]
pub fn set_test_config_dir_in_current_thread(dir: std::path::PathBuf) {
    TEST_CONFIG_DIR.with(|cell| *cell.borrow_mut() = Some(dir));
}

/// 进程级配置目录覆盖：只给 ignored 真机评测用。Pi 工具在 process 派生的
/// 线程上执行，线程局部覆盖到不了那些线程；评测期间必须让工具线程也解析到
/// 临时目录，否则会去开真实库（加密库按明文读就是「file is not a database」）。
/// 全局覆盖会影响同进程其他测试，用完必须清回 `None`（用守卫，panic 也要清）。
#[cfg(test)]
static TEST_CONFIG_DIR_GLOBAL: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

#[cfg(test)]
pub fn set_test_config_dir_global(dir: Option<PathBuf>) {
    *TEST_CONFIG_DIR_GLOBAL
        .write()
        .unwrap_or_else(|error| error.into_inner()) = dir;
}

/// 全局覆盖的守卫：离开作用域（含 panic 展开）自动清回。
#[cfg(test)]
pub struct TestConfigDirGlobalGuard;

#[cfg(test)]
impl Drop for TestConfigDirGlobalGuard {
    fn drop(&mut self) {
        set_test_config_dir_global(None);
    }
}

#[cfg(test)]
pub fn push_test_config_dir_global(dir: PathBuf) -> TestConfigDirGlobalGuard {
    set_test_config_dir_global(Some(dir));
    TestConfigDirGlobalGuard
}

pub fn config_dir() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = TEST_CONFIG_DIR.with(|cell| cell.borrow().clone()) {
        return Ok(dir);
    }
    #[cfg(test)]
    if let Some(dir) = TEST_CONFIG_DIR_GLOBAL
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
    {
        return Ok(dir);
    }
    let dirs = ProjectDirs::from("cn", "LocalTools", "GongwenAssistant")
        .context("无法确定用户配置目录")?;
    Ok(dirs.config_dir().to_path_buf())
}

/// 本用户的缓存目录（预览之类可以随时重建的文件）。与系统临时目录不同，
/// 它只有当前用户能写，不会被同机其他用户抢先占用。
pub fn cache_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("cn", "LocalTools", "GongwenAssistant")
        .context("无法确定用户缓存目录")?;
    Ok(dirs.cache_dir().to_path_buf())
}

pub fn config_path() -> Result<PathBuf> {
    slot_file("config.json")
}

/// 启动最早处只为取窗口图标主题时使用：只读解析，不迁移、不挪走损坏文件。
///
/// 真正的配置读取与损坏兜底留给 `GongwenApp::new`——如果这里也调 [`load`]，
/// 损坏的 `config.json` 会在界面建起来之前就被改名，后面再也收不到「配置已损坏」
/// 的提示。
pub fn peek_theme() -> crate::models::ThemeName {
    let Ok(path) = config_path() else {
        return AppConfig::default().theme;
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return AppConfig::default().theme;
    };
    serde_json::from_str::<AppConfig>(&raw)
        .map(|config| config.theme)
        .unwrap_or_default()
}

/// 稿件库 SQLite 文件，与 config.json 同目录，仅保存在本机。
/// 槽位感知：真实系统用 manuscripts.db，诱饵系统用 alt-manuscripts.db。
pub fn manuscript_db_path() -> Result<PathBuf> {
    slot_file("manuscripts.db")
}

/// 读取用户明确选择“记住”的 ZIP 密码。密码单独存放，避免进入 config.json、
/// 配置版本历史或稿件导出包。Unix 下写入时把权限限制为仅当前用户可读写。
pub fn load_remembered_zip_password() -> Result<Option<String>> {
    let path = slot_file(ZIP_PASSWORD_FILE)?;
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::metadata(&path)?;
    if metadata.len() > MAX_REMEMBERED_PASSWORD_BYTES + 64 {
        anyhow::bail!("记住的 ZIP 密码文件异常，已拒绝读取");
    }
    let raw =
        fs::read(&path).with_context(|| format!("读取记住的 ZIP 密码失败：{}", path.display()))?;
    // 普通模式不再读取或保留旧版明文密码。
    if crate::vault::key_handle().is_none() && !crate::vault::is_sealed_file(&raw) {
        crate::vault::erase_file_checked(&path)?;
        return Ok(None);
    }
    let bytes = if crate::vault::is_sealed_file(&raw) {
        let handle = crate::vault::key_handle().context("ZIP 密码已加密，尚未解锁")?;
        crate::vault::open_path_bytes(&handle, &path, "config", &raw)?
    } else if let Some(handle) = crate::vault::key_handle() {
        // 启用加密前留下的明文密码：读到就顺手封成密文，不能让它长期躺在盘上。
        // 落盘失败不影响本次读取（下次启动再迁）。
        if let Ok(sealed) = crate::vault::seal_path_bytes(&handle, &path, "config", &raw) {
            let _ = atomic_write(&path, |file| {
                use std::io::Write;
                file.write_all(&sealed)?;
                Ok(())
            });
        }
        raw
    } else {
        raw
    };
    let password = String::from_utf8(bytes).context("记住的 ZIP 密码不是有效文本")?;
    Ok((!password.is_empty()).then_some(password))
}

pub fn save_remembered_zip_password(password: Option<&str>) -> Result<()> {
    let path = slot_file(ZIP_PASSWORD_FILE)?;
    if let Some(password) = password {
        anyhow::ensure!(
            crate::vault::key_handle().is_some(),
            "请先启用应用锁，才能加密记住导出密码"
        );
        if password.len() as u64 > MAX_REMEMBERED_PASSWORD_BYTES {
            anyhow::bail!("ZIP 密码过长，无法记住");
        }
        crate::vault::write_path(&path, "config", password.as_bytes())?;
    } else if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// Windows 上杀软 / 索引器会短时占住目标文件，`persist` 报 sharing violation（32）
/// 或 access denied（5）。这类错误等一会儿通常就过去，其它错误立即返回。
#[cfg(windows)]
fn is_transient_windows_lock(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5) | Some(32))
}

/// 把临时文件原子替换到目标路径。
///
/// Windows 上最多重试 6 次，退避 20/40/80/160/320 ms；每次失败要把 `PersistError`
/// 里的临时文件取回来再 `persist`，否则下一次没有可替换的暂存文件。非 Windows 仍
/// 只替换一次（POSIX rename 不会被占用挡住）。
#[cfg(windows)]
fn persist_with_retry(
    mut temp: tempfile::NamedTempFile,
    path: &std::path::Path,
) -> std::io::Result<()> {
    const BACKOFF_MS: [u64; 5] = [20, 40, 80, 160, 320];
    let mut attempt = 0;
    loop {
        match temp.persist(path) {
            Ok(_) => return Ok(()),
            Err(error) => {
                if !is_transient_windows_lock(&error.error) || attempt >= BACKOFF_MS.len() {
                    return Err(error.error);
                }
                temp = error.file;
                std::thread::sleep(std::time::Duration::from_millis(BACKOFF_MS[attempt]));
                attempt += 1;
            }
        }
    }
}

/// 临时文件刷盘。Windows 上同步刷盘也可能碰到同样的短暂占用，重试一次即可
/// （主路径的保护在 `persist`）。
#[cfg(windows)]
fn sync_temp_file(file: &fs::File) -> std::io::Result<()> {
    match file.sync_all() {
        Ok(()) => Ok(()),
        Err(error) if is_transient_windows_lock(&error) => {
            std::thread::sleep(std::time::Duration::from_millis(20));
            file.sync_all()
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(windows))]
fn sync_temp_file(file: &fs::File) -> std::io::Result<()> {
    file.sync_all()
}

/// 同目录暂存并刷盘后原子替换；失败时不删除原文件，暂存文件由 RAII 清理。
/// 配置里也可能包含服务密钥，Unix 下和密码一样限制为仅当前用户可读写。
///
/// 密钥文件（`api-secrets.json` 等）也走这条路径：写一半的密钥文件会让
/// 「读得到、但内容不完整」，比写失败更难查。
pub(crate) fn atomic_write(
    path: &std::path::Path,
    write: impl FnOnce(&mut fs::File) -> Result<()>,
) -> Result<()> {
    // 锁定已经开始：拒绝一切配置目录写入。被丢弃工作区里晚到的线程不能以
    // 「无密钥」状态把明文写回加密目录（见 set_writes_blocked 的注释）。
    if writes_blocked() {
        anyhow::bail!("正在锁定或切换数据槽位，已拒绝写入 {}", path.display());
    }
    let parent = path.parent().context("保存路径缺少父目录")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    if is_private_dir(parent) {
        harden_dir(parent);
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    write(temp.as_file_mut())?;
    sync_temp_file(temp.as_file())?;
    #[cfg(windows)]
    persist_with_retry(temp, path)?;
    #[cfg(not(windows))]
    temp.persist(path).map_err(|error| error.error)?;
    // Unix 上目录项也需刷盘。此处失败时新文件已替换，向调用方如实报告错误。
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Unix 下把目录权限收紧到仅本人可读写执行（0700）。配置目录里放着密文、
/// 明文图片、词表与导出中间文件，同机其他用户不该有列目录的权限。
/// 失败只忽略：某些文件系统（FAT / 网络盘）不支持权限位，不能因此启动不了。
#[cfg(unix)]
pub(crate) fn harden_dir(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub(crate) fn harden_dir(_path: &std::path::Path) {}

/// 目标目录是否属于应用自己的私密目录（配置目录 / 缓存目录之内）。
/// 导出到用户选择的输出目录时不能顺手改人家的目录权限。
#[cfg(unix)]
fn is_private_dir(dir: &std::path::Path) -> bool {
    [config_dir(), cache_dir()]
        .into_iter()
        .flatten()
        .any(|root| dir == root || dir.starts_with(&root))
}

/// 启动时把配置目录与已知子目录的权限收紧一次（幂等、尽力而为）。
pub(crate) fn harden_permissions() {
    let Ok(dir) = config_dir() else {
        return;
    };
    harden_dir(&dir);
    // 槽位目录两套都要收紧：诱饵槽的 `alt-*` 目录同样含用户内容。
    for name in [
        "apis",
        "images",
        "pi",
        "mermaid-cache",
        "skills",
        "ime",
        "logs",
    ] {
        for slot in [DataSlot::Real, DataSlot::Decoy] {
            if let Ok(path) = slot_subdir_for(slot, name)
                && path.is_dir()
            {
                harden_dir(&path);
            }
        }
    }
    // 帮助目录不区分槽位，只在真实配置目录下。
    let help = dir.join("help");
    if help.is_dir() {
        harden_dir(&help);
    }
    if let Ok(cache) = cache_dir()
        && cache.is_dir()
    {
        harden_dir(&cache);
    }
}

/// 读取配置。第二个返回值是本次被挪走的损坏配置路径：非空表示 `config.json`
/// 解析失败，调用方已经拿到一份默认配置，应把这件事告诉用户。
///
/// 启用应用锁后 config.json 是 AEAD 密文（`vault::is_sealed_file`）：
/// - 有句柄：解密，失败按「密钥不匹配 / 文件损坏」返回 Err，**绝不改名、
///   绝不回落到默认配置写回**（否则等于把解密失败当成损坏，再以明文覆盖）；
/// - 没有句柄：返回 Err，同样不动文件。闸门已经拦住「锁设置丢失却直接进工作区」，
///   这一层是防御纵深：任何降级路径都不能静默重置配置。
///
/// 只有**明文** JSON 解析失败时才把原文件原地改名为
/// `config.json.损坏-YYYYMMDD-HHMMSS`，保留证据，也避免下次保存时用默认值
/// 静默覆盖用户数据。改名失败同样回落到默认配置，只是没有可报告的留底路径。
/// 读取文件本身失败（权限、路径是目录等）仍返回 Err。
pub fn load() -> Result<(AppConfig, Option<PathBuf>)> {
    let path = config_path()?;
    let mut config = if !path.exists() {
        AppConfig::default()
    } else {
        let raw = fs::read(&path).with_context(|| format!("读取配置失败：{}", path.display()))?;
        let sealed_input = crate::vault::is_sealed_file(&raw);
        let bytes = if sealed_input {
            let handle = crate::vault::key_handle().context("配置已加密，尚未解锁")?;
            match crate::vault::open_path_bytes(&handle, &path, "config", &raw) {
                Ok(bytes) => bytes,
                Err(error) => {
                    // 密文解不开（另一代 DEK / 另一槽 / 损坏）：绝不能先回落默认配置
                    // 再让后续保存把唯一密文覆盖掉。把原密文原样留底（路径变了 AAD
                    // 就不再成立，但字节仍可用于人工恢复到原文件名），再返回错误。
                    let backup = unreadable_config_path(&path);
                    let kept = atomic_write(&backup, |file| {
                        use std::io::Write as _;
                        file.write_all(&raw)?;
                        Ok(())
                    })
                    .is_ok();
                    if kept {
                        let _ = fs::remove_file(&path);
                    }
                    anyhow::bail!(
                        "配置解密失败（密钥不匹配或文件损坏）：{error:#}{}",
                        if kept {
                            format!("；原密文已留底到 {}", backup.display())
                        } else {
                            "；原密文保留在原处，本次不会覆盖".into()
                        }
                    );
                }
            }
        } else {
            raw
        };
        match serde_json::from_slice::<AppConfig>(&bytes).context("配置内容无法解析") {
            Ok(config) => config,
            Err(error) => {
                let backup = corrupted_config_path(&path);
                let moved = if sealed_input {
                    // AAD 绑定逻辑路径，不能直接把密文改名；先用已验证的明文
                    // 在留底路径重新密封，再删除原文件，保证留底仍可解密。
                    crate::vault::write_path(&backup, "config", &bytes)
                        .with_context(|| format!("损坏配置留底失败：{error:#}"))?;
                    fs::remove_file(&path).is_ok()
                } else {
                    fs::rename(&path, &backup).is_ok()
                };
                return Ok((AppConfig::default(), moved.then_some(backup)));
            }
        }
    };
    // 旧配置里的内联地址 / 密钥归并成提供商清单；迁移结果立即写回，
    // 写盘失败也不影响本次运行（下次启动再迁一次）。
    if config.migrate_providers() && path.exists() {
        let _ = save(&config);
    }
    // 智能写作（Pi）的思考深度单独迁移：与提供商迁移互不相干，迁移后立即写回，
    // 之后改文字起草的思考深度不会影响 Pi。
    if config.migrate_pi_options() && path.exists() {
        let _ = save(&config);
    }
    if crate::units::identity::assign(&mut config.vocabulary) && path.exists() {
        let backup = path.with_extension("json.身份迁移前备份");
        if !backup.exists() {
            let current = fs::read(&path)?;
            let backup_bytes = if crate::vault::is_sealed_file(&current) {
                let handle = crate::vault::key_handle().context("配置已加密，尚未解锁")?;
                crate::vault::open_path_bytes(&handle, &path, "config", &current)?
            } else {
                current
            };
            // 副本按自己的逻辑路径重新密封，不能复制绑定原路径的 GWA2 密文。
            crate::vault::write_path(&backup, "config", &backup_bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
            }
        }
        // 旧词库可能已有同级重名或悬空关系，保留供界面核对，不能因此启动成空配置。
        write_config(&path, &config).context("保存身份迁移失败")?;
    }
    Ok((config, None))
}

/// 损坏配置的留底路径：与原文件同目录，文件名带秒级时间戳；同一秒里再次损坏
/// 就追加序号，绝不覆盖上一份证据。
fn corrupted_config_path(path: &std::path::Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.json".into());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut candidate = path.with_file_name(format!("{name}.损坏-{stamp}"));
    let mut index = 2;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{name}.损坏-{stamp}-{index}"));
        index += 1;
    }
    candidate
}

/// 解密失败的配置留底路径：与损坏留底分开命名，避免被「明文损坏」的处理逻辑
/// 当成 JSON 再解析一遍。
fn unreadable_config_path(path: &std::path::Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.json".into());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut candidate = path.with_file_name(format!("{name}.无法解密-{stamp}"));
    let mut index = 2;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{name}.无法解密-{stamp}-{index}"));
        index += 1;
    }
    candidate
}

/// 把配置写成目标形态：有句柄（应用锁启用中）写 AEAD 密文，没有写明文。
/// 已启用时绝不能把明文写回磁盘——那等于把刚锁好的门又打开。
fn write_config(path: &std::path::Path, config: &AppConfig) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(config).context("配置序列化失败")?;
    crate::vault::write_path(path, "config", &bytes)
}

pub fn save(config: &AppConfig) -> Result<()> {
    crate::units::identity::validate(&config.vocabulary).map_err(anyhow::Error::msg)?;
    let path = config_path()?;
    write_config(&path, config).context("保存配置失败，上一次有效配置会在替换前保留")
}

/// 清掉崩溃时留下的原子写临时文件（`tempfile` 默认名 `.tmpXXXXXX`）。
///
/// 为什么必须清：明文模式下保存 `config.json` / 密钥文件时进程被强杀，会留下
/// 一份**明文**临时副本；之后启用应用锁的迁移扫描只认已知文件名，不会碰它，
/// 于是永久明文，并让「全量备份内部只许密文」的检查永久失败。
///
/// 只删配置目录顶层、名字以 `.tmp` 开头、且超过一小时没有修改的普通文件：
/// 一小时足够避开任何在途写入，也不会碰用户自己的数据。
pub(crate) fn remove_stale_write_temp_files(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let stale_before = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(std::time::UNIX_EPOCH);
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(".tmp") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let old = meta
            .modified()
            .map(|modified| modified < stale_before)
            .unwrap_or(false);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 给 SQLite 连接应用当前槽位的数据密钥（SQLCipher PRAGMA key）。
///
/// 调用点：每个 `Connection::open` 之后**立即**调用，必须在任何读写之前。
/// - 无句柄（未启用应用锁、测试）：什么也不做，库保持明文形态；
/// - 有句柄但 `db_active = false`（设置页刚启用、迁移安排在下次解锁）：
///   本会话继续按明文开库，与磁盘形态一致；
/// - 有句柄且 `db_active = true`（解锁迁移完成后）：带 key 开库。
///
/// 必须用 `key = 'x'..'` 原始密钥写法：本构建的 `PRAGMA hexkey` 会把解码后的
/// 字节当口令再走 PBKDF2，与 `ATTACH ... KEY "x'..'"`（迁移导出用）派生出
/// 不同的密钥；x'..' 两种入口才是同一把原始密钥（见 src/vault.rs::apply_key_with）。
pub(crate) fn apply_db_key(conn: &rusqlite::Connection) -> Result<()> {
    let Some(handle) = crate::vault::key_handle() else {
        return Ok(());
    };
    if !handle.db_active() {
        return Ok(());
    }
    conn.pragma_update(None, "key", format!("x'{}'", handle.dek_hex().as_str()))
        .context("数据库密钥应用失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        JointIssuanceMode, LetterVersion, SecurityLevel, TemplateKind, VocabularyCategory,
    };

    #[test]
    fn ordinary_workspace_never_remembers_a_password_and_removes_the_legacy_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        set_test_config_dir(Some(dir.path().to_owned()));
        crate::vault::clear_key_handle();
        assert!(save_remembered_zip_password(Some("ShouldNeverReachDisk!80")).is_err());
        let password_file = slot_file(ZIP_PASSWORD_FILE).unwrap();
        assert!(!password_file.exists());
        fs::write(&password_file, b"old plaintext password").unwrap();
        assert!(load_remembered_zip_password().unwrap().is_none());
        assert!(!password_file.exists());
        set_test_config_dir(None);
    }

    #[test]
    fn failed_write_preserves_previous_file_and_cleans_up_temporary_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("配置.json");
        fs::write(&path, b"previous config").unwrap();
        let result = atomic_write(&path, |file| {
            file.write_all(b"partial replacement")?;
            anyhow::bail!("模拟写入中途失败")
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous config");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn atomic_save_replaces_existing_config_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("配置.json");
        fs::write(&path, b"old config").unwrap();
        let config = AppConfig {
            output_dir: "中文路径/导出".into(),
            ..AppConfig::default()
        };
        atomic_write(&path, |file| {
            serde_json::to_writer_pretty(file, &config)?;
            Ok(())
        })
        .unwrap();
        let loaded: AppConfig = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(loaded.output_dir, config.output_dir);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn failed_persist_does_not_delete_destination_or_leak_temporary_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("被占用的目标");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("旧数据"), b"keep me").unwrap();
        assert!(
            atomic_write(&path, |file| {
                file.write_all(b"new config")?;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(fs::read(path.join("旧数据")).unwrap(), b"keep me");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Windows 上杀软 / 索引器会短时占住目标文件：`persist` 应重试，占用一解除就
    /// 写入成功。用 `share_mode(0)` 独占打开模拟这种占用，100 ms 后释放。
    #[cfg(windows)]
    #[test]
    fn a_transient_windows_file_lock_is_retried() {
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("被占用.txt");
        fs::write(&path, b"old").unwrap();
        let lock = fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(lock);
        });
        atomic_write(&path, |file| {
            file.write_all(b"new")?;
            Ok(())
        })
        .expect("短暂占用解除后应写入成功");
        release.join().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "暂存文件不能留下"
        );
    }

    #[test]
    fn example_config_matches_the_current_schema() {
        let raw = include_str!("../config.example.json");
        let config: AppConfig = serde_json::from_str(raw).expect("示例配置应能反序列化");
        assert_eq!(config.security_rules.confidential_max_years, 20);
        assert!(config.export.docx);
        assert_eq!(
            config.profile(TemplateKind::OfficialLetter).letter_version,
            LetterVersion::Formal
        );
        assert_eq!(
            config
                .profile(TemplateKind::OfficialLetter)
                .joint_issuance_mode,
            JointIssuanceMode::Single
        );
        assert!(
            config
                .profiles
                .iter()
                .any(|profile| profile.kind == TemplateKind::PhoneNotice)
        );
        assert!(
            config
                .profiles
                .iter()
                .any(|profile| profile.kind == TemplateKind::RedHeadApproval)
        );
        let contact = config
            .vocabulary
            .iter()
            .find(|entry| entry.category == VocabularyCategory::Person)
            .expect("示例配置应包含人员词条");
        assert_eq!(contact.phone, "000-12345678");
        // 机关代字是单位自身的属性，不再单列一类词条。
        assert!(
            config
                .vocabulary
                .iter()
                .any(|entry| entry.department_code == "某函")
        );
        // 人员挂在末端单位下，层级编码由上级编码加两位构成。
        assert_eq!(contact.unit, "0001");
        let branch = config
            .vocabulary
            .iter()
            .find(|entry| entry.canonical == "综合处")
            .expect("示例配置应包含下级单位");
        assert_eq!(branch.parent, "00");
        assert_eq!(branch.code, "0001");
        // 知识库 RAG 配置节
        assert_eq!(config.rag.recall_top_k, 20);
        assert_eq!(config.rag.rerank.path, "rerank");
        assert_eq!(config.rag.embedding.batch_size, 32);
        assert!(!config.rag.enabled);
        assert_eq!(config.rag.min_score_ratio, 0.5);
        // 本地模型服务（LM Studio / Ollama）不提供 rerank 端点，示例配置默认走对话大模型重排。
        assert_eq!(config.rag.rerank.mode, crate::models::RerankMode::Llm);
        // 输入法：示例配置给的是「启用全拼 + 全角标点 + 一页五个」。
        assert!(config.ime.enabled);
        assert!(!config.ime.auto_commit);
        assert!(config.ime.fifth_commit);
        assert!(config.ime.full_width_punctuation);
        assert_eq!(config.ime.page_size, 5);
        assert_eq!(config.ime.page_keys, "[]");
    }

    /// 老配置里没有 `rag.rerank.mode` 字段，必须默认成「专用端点」，
    /// 保持既有行为不变——不能因为加了新枚举就把用户已配好的重排静默改掉。
    #[test]
    fn rerank_mode_defaults_to_api_for_existing_configs() {
        let legacy = r#"{"rag":{"rerank":{"model":"bge-reranker","path":"rerank"}}}"#;
        let config: AppConfig = serde_json::from_str(legacy).expect("老配置应能反序列化");
        assert_eq!(config.rag.rerank.mode, crate::models::RerankMode::Api);
        assert_eq!(config.rag.rerank.model, "bge-reranker");
        assert_eq!(
            config.rag.effective_rerank_mode(),
            crate::models::RerankMode::Api
        );
        // 选了专用端点却没填模型名时降级为不重排，而不是拿空模型名去请求。
        let mut empty = config.rag.clone();
        empty.rerank.model = String::new();
        assert_eq!(
            empty.effective_rerank_mode(),
            crate::models::RerankMode::None
        );
    }

    /// 校对词表是 v0.4 才有的字段，此前的配置里没有它，载入时必须回落空覆盖层
    /// ——而不是让整份配置反序列化失败，把用户的词库和模板一起丢掉。
    #[test]
    fn configs_written_before_the_proofread_lexicon_still_load() {
        let legacy = r#"{"output_dir":"C:/out"}"#;
        let config: AppConfig = serde_json::from_str(legacy).expect("老配置应能反序列化");
        assert!(config.proofread.overrides.is_empty());
        assert!(config.proofread.custom.is_empty());
    }

    /// 词表改动必须能存下来：停用一条内置、自建一条，存盘再读回来要一模一样。
    #[test]
    fn proofread_changes_survive_a_save_and_load_round_trip() {
        use crate::models::ProofreadRule;

        let mut config = AppConfig::default();
        config.proofread.override_mut("TYP-001").enabled = Some(false);
        config.proofread.custom.push(ProofreadRule {
            id: "USR-001".into(),
            wrong: "我办".into(),
            suggestion: "本办".into(),
            level: "疑似".into(),
            condition: "总是".into(),
            group: "自定义".into(),
            note: String::new(),
            enabled: true,
        });

        let raw = serde_json::to_string(&config).expect("序列化");
        let restored: AppConfig = serde_json::from_str(&raw).expect("反序列化");
        assert_eq!(restored.proofread, config.proofread);

        // 合并后确实生效：被停用的那条不再命中，自建的那条会命中。
        let lexicon = crate::proofread::Lexicon::resolved(&restored.proofread);
        let notes = lexicon.check("按上级布署办理，材料报我办。");
        assert!(notes.iter().all(|note| note.entry_id != "TYP-001"));
        assert!(notes.iter().any(|note| note.entry_id == "USR-001"));
    }

    /// 手写或精简过的配置里可以只给出必填字段，其余一律取默认值。
    #[test]
    fn sparse_config_still_loads_with_defaults() {
        let sparse = r#"{
            "output_dir": "C:/out",
            "vocabulary": [
                {"category":"Unit","canonical":"某某省教育厅"},
                {"category":"Unit","canonical":"教师工作处","parent":"某某省教育厅"},
                {"category":"Person","canonical":"张三处长","unit":"教师工作处"}
            ],
            "last_template": "MeetingAgenda"
        }"#;
        let config: AppConfig = serde_json::from_str(sparse).expect("精简配置应能反序列化");
        assert_eq!(config.output_dir, "C:/out");
        assert!(config.vocabulary[0].phone.is_empty());
        // 缺省的 id 与层级编码留待载入时统一整理。
        assert_eq!(config.vocabulary[0].id, 0);
        assert!(config.vocabulary[0].code.is_empty());
        assert_eq!(
            config
                .vocabulary
                .iter()
                .filter(|entry| entry.category == VocabularyCategory::Unit)
                .count(),
            2
        );
        assert!(config.export.markdown);
        assert!(config.show_editor_line_numbers);
        assert!(config.show_source_outline);
        assert!(config.show_source_minimap);
        assert!(config.show_preview_navigator);
        assert_eq!(config.security_rules.secret_max_years, 10);
        assert!(!config.auto_export);
        assert!(!config.profile(TemplateKind::OfficialLetter).duplex_printing);
        assert_eq!(
            config.profile(TemplateKind::OfficialLetter).letter_version,
            LetterVersion::Formal
        );
        assert_eq!(
            config
                .profile(TemplateKind::OfficialLetter)
                .joint_issuance_mode,
            JointIssuanceMode::Single
        );
        assert_eq!(
            SecurityLevel::from_marking(""),
            SecurityLevel::Unmarked,
            "旧配置里空密级应视为不标注"
        );
    }

    /// 损坏配置不能静默丢掉：返回默认配置，同时把原文件改名留底。
    #[test]
    fn corrupt_config_falls_back_to_defaults_and_keeps_the_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        super::set_test_config_dir(Some(dir.path().to_path_buf()));
        let path = dir.path().join("config.json");
        fs::write(&path, b"{ this is not json").unwrap();

        let (config, recovered) = load().expect("内容损坏不应返回 IO 错误");
        super::set_test_config_dir(None);

        assert_eq!(config.output_dir, AppConfig::default().output_dir);
        let recovered = recovered.expect("应报告留底的损坏文件");
        assert!(!path.exists(), "损坏的 config.json 应被挪走");
        assert!(
            recovered
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("config.json.损坏-"),
            "留底文件名应带「损坏」与时间戳：{}",
            recovered.display()
        );
        assert_eq!(
            fs::read(&recovered).unwrap(),
            b"{ this is not json",
            "留底文件应原样保留证据"
        );
    }

    /// 正常配置不带兜底标记，调用方不该弹恢复提示。
    #[test]
    fn valid_config_loads_without_recovery_marker() {
        let dir = tempfile::tempdir().unwrap();
        super::set_test_config_dir(Some(dir.path().to_path_buf()));
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            serde_json::to_vec(&AppConfig::default()).expect("序列化默认配置"),
        )
        .unwrap();

        let (config, recovered) = load().expect("正常配置应能读取");
        super::set_test_config_dir(None);

        assert_eq!(config.output_dir, AppConfig::default().output_dir);
        assert!(recovered.is_none());
        assert!(path.exists());
    }

    /// IO 错误不是「内容损坏」，必须维持 Err，不能拿默认值顶替。
    #[test]
    fn unreadable_config_still_returns_an_error() {
        let dir = tempfile::tempdir().unwrap();
        super::set_test_config_dir(Some(dir.path().to_path_buf()));
        // 把配置路径做成目录：exists() 为真，read_to_string 必然失败。
        fs::create_dir(dir.path().join("config.json")).unwrap();

        let result = load();
        super::set_test_config_dir(None);

        assert!(result.is_err(), "路径不可读时应如实报错");
    }

    /// 启动早期取主题只能只读：它若把损坏文件挪走，界面上就收不到恢复提示了。
    #[test]
    fn peek_theme_leaves_a_corrupt_config_in_place() {
        let dir = tempfile::tempdir().unwrap();
        super::set_test_config_dir(Some(dir.path().to_path_buf()));
        let path = dir.path().join("config.json");
        fs::write(&path, b"not json").unwrap();

        let theme = peek_theme();
        super::set_test_config_dir(None);

        assert_eq!(theme, AppConfig::default().theme);
        assert!(path.exists(), "peek_theme 不能改写或挪走配置文件");
    }
    /// 密封配置在「没有密钥句柄」时必须是 Err，不能当损坏文件改名，更不能
    /// 随后以明文默认配置覆盖（那是一次静默加密降级）。
    #[test]
    fn sealed_config_without_key_is_not_renamed_as_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        super::set_test_config_dir(Some(dir.path().to_path_buf()));
        crate::vault::clear_key_handle();
        let path = dir.path().join("config.json");
        let sealed = crate::vault::seal_bytes(&[7u8; 32], b"{\"output_dir\":\"x\"}").unwrap();
        fs::write(&path, &sealed).unwrap();

        let result = load();
        super::set_test_config_dir(None);

        assert!(result.is_err(), "密封配置在未解锁时必须报错");
        assert_eq!(fs::read(&path).unwrap(), sealed, "不得改名或覆盖密文");
        assert!(
            fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().contains("损坏")),
            "不能把解密失败当成配置文件损坏"
        );
        assert!(path.exists());
    }

    /// 配置密文解不开时：返回错误、把原始密文另存留底，绝不让后续保存
    /// 把唯一副本覆盖成默认配置。
    #[test]
    fn undecryptable_config_is_set_aside_before_default_save() {
        let dir = tempfile::tempdir().unwrap();
        super::set_test_config_dir(Some(dir.path().to_path_buf()));
        crate::vault::clear_key_handle();
        let path = dir.path().join("config.json");
        let sealed = crate::vault::seal_bytes(&[21u8; 32], b"{\"output_dir\":\"x\"}").unwrap();
        fs::write(&path, &sealed).unwrap();
        crate::vault::set_key_handle([22u8; 32], true);

        let result = load();
        crate::vault::clear_key_handle();
        super::set_test_config_dir(None);

        assert!(result.is_err(), "错误密钥解不开时应报错");
        assert!(!path.exists(), "原路径应让出来给新配置");
        let kept = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().contains("无法解密"))
            .expect("应留下无法解密的副本");
        assert_eq!(fs::read(kept).unwrap(), sealed, "留底必须是原始密文字节");
    }

    /// 无句柄时拒绝对已密封的配置写明文（降级防护）。
    #[test]
    fn plaintext_save_refuses_to_overwrite_a_sealed_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let sealed = crate::vault::seal_bytes(&[9u8; 32], b"{\"output_dir\":\"x\"}").unwrap();
        fs::write(&path, &sealed).unwrap();
        crate::vault::clear_key_handle();

        let result = write_config(&path, &AppConfig::default());
        assert!(result.is_err(), "无密钥时不得把密文配置改写成明文");
        assert_eq!(fs::read(&path).unwrap(), sealed);
    }

    /// ZIP 密码与配置走同一降级规则：密文目标在未解锁时绝不能被明文替换。
    #[test]
    fn plaintext_save_refuses_to_overwrite_a_sealed_zip_password() {
        let dir = tempfile::tempdir().unwrap();
        super::set_test_config_dir(Some(dir.path().to_path_buf()));
        let path = slot_file(ZIP_PASSWORD_FILE).unwrap();
        let sealed = crate::vault::seal_bytes(&[11u8; 32], b"old-secret").unwrap();
        fs::write(&path, &sealed).unwrap();
        crate::vault::clear_key_handle();

        let result = save_remembered_zip_password(Some("new-secret"));
        super::set_test_config_dir(None);

        assert!(result.is_err(), "无密钥时不得把 ZIP 密码改写成明文");
        assert_eq!(fs::read(&path).unwrap(), sealed);
    }

    /// 导出目录是用户选的，不属于应用的私密目录：写文件时不能顺手改它的权限。
    #[cfg(unix)]
    #[test]
    fn atomic_write_leaves_user_chosen_directory_permissions_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let target = dir.path().join("份号清单.txt");
        atomic_write(&target, |file| {
            use std::io::Write as _;
            file.write_all("清单".as_bytes())?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    /// 槽位前缀只能映射自己那一槽：真实文件不能被诱饵密钥处理，反之亦然。
    /// 崩溃残留的原子写临时文件（`.tmpXXXXXX`）可能是明文配置副本：清掉旧的，
    /// 但在途写入（新文件）与用户自己的 `.tmp` 后缀文件不能动。
    #[test]
    fn stale_atomic_write_temps_are_removed_but_fresh_files_survive() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(".tmpAbCdEf");
        let fresh = dir.path().join(".tmpFresh1");
        let user = dir.path().join("keep.tmp");
        fs::write(&stale, b"plaintext config").unwrap();
        fs::write(&fresh, b"in flight").unwrap();
        fs::write(&user, b"user file").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        remove_stale_write_temp_files(dir.path());
        assert!(!stale.exists(), "旧临时文件应清掉");
        assert!(fresh.exists(), "在途写入不能删");
        assert!(user.exists(), "只清 `.tmp` 前缀的原子写临时名");
    }

    /// 「清除第二密码」必须把诱饵槽的崩溃日志一起擦掉：日志用旧诱饵 DEK 密封，
    /// 锁文件一换就再也解不开；残留下来还会让停用应用锁后误判「锁文件丢失」。
    #[test]
    fn wiping_a_slot_also_removes_its_logs_and_legacy_api_file() {
        let dir = tempfile::tempdir().unwrap();
        set_test_config_dir(Some(dir.path().to_owned()));
        fs::create_dir_all(dir.path().join("alt-logs")).unwrap();
        fs::write(dir.path().join("alt-logs/crash-1.log"), b"sealed").unwrap();
        fs::write(dir.path().join("alt-apis.json"), b"legacy").unwrap();
        fs::write(dir.path().join("alt-config.json"), b"sealed").unwrap();
        wipe_slot_files(DataSlot::Decoy);
        assert!(!dir.path().join("alt-logs").exists());
        assert!(!dir.path().join("alt-apis.json").exists());
        assert!(!dir.path().join("alt-config.json").exists());
        // 真实槽的同名文件不受影响。
        fs::write(dir.path().join("apis.json"), b"real").unwrap();
        fs::write(dir.path().join("config.json"), b"real").unwrap();
        wipe_slot_files(DataSlot::Decoy);
        assert!(dir.path().join("apis.json").exists());
        assert!(dir.path().join("config.json").exists());
        set_test_config_dir(None);
    }

    #[test]
    fn slot_relative_name_never_mixes_slots() {
        assert_eq!(
            slot_relative_name("config.json", DataSlot::Real),
            Some("config.json")
        );
        assert_eq!(slot_relative_name("alt-config.json", DataSlot::Real), None);
        assert_eq!(
            slot_relative_name("alt-config.json", DataSlot::Decoy),
            Some("config.json")
        );
        assert_eq!(slot_relative_name("config.json", DataSlot::Decoy), None);
    }
}
