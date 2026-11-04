//! 运行时定位：随包 Node + host，或开发时显式指定的绝对路径。
//!
//! 不在 PATH 里找 `node` / `pi`，不运行 npm，也不读用户的 Pi 配置。缺少运行时
//! 时返回可操作的中文提示，由界面直接显示。

use sha2::Digest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

/// 随包固定的 Node 版本；升级时同时改 packaging/pi-runtime.json 与 manifest。
pub(crate) const EXPECTED_NODE_VERSION: &str = "22.22.3";
const EXPECTED_NODE_VERSION_PARTS: (u32, u32, u32) = (22, 22, 3);
/// manifest 的读取上限；当前约 2.6 MB（1.3 万个文件）。
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

/// Node 与 manifest 等已验证文件的（字节数, mtime）指纹。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeFingerprint(Vec<(u64, u64)>);
/// 已通过校验的运行时目录与指纹。
type VerifiedCache = Mutex<HashMap<PathBuf, RuntimeFingerprint>>;

/// 本进程里已经完整校验通过的运行时目录。
///
/// 键是 `canonicalize` 之后的目录（测试与开发用的临时目录不会互相命中），值是
/// Node 与 manifest 的（字节数, mtime）指纹。完整校验要读约 2.6 MB manifest、哈希
/// 约 83 MB Node 再起 `node --version` 子进程，实测 300 ms 以上；同一进程里每次开跑
/// 都重做没有意义。任一文件大小或 mtime 变化就重新完整校验。
static VERIFIED: OnceLock<VerifiedCache> = OnceLock::new();

/// 随包运行时 manifest 里启动时要用的部分。
#[derive(Debug, serde::Deserialize)]
struct PiManifest {
    schema: u32,
    source: String,
    platform: String,
    node: ManifestNode,
    host: ManifestHost,
    files: Vec<ManifestFile>,
}

#[derive(Debug, serde::Deserialize)]
struct ManifestNode {
    version: String,
}

#[derive(Debug, serde::Deserialize)]
struct ManifestHost {
    version: String,
    protocol: u32,
    #[serde(default, rename = "lockfileSha256")]
    lockfile_sha256: String,
}

#[derive(Debug, serde::Deserialize)]
struct ManifestFile {
    path: String,
    sha256: String,
}

impl PiManifest {
    fn file_hash(&self, relative: &str) -> Option<String> {
        self.files
            .iter()
            .find(|file| file.path == relative)
            .map(|file| file.sha256.to_ascii_lowercase())
    }
}

fn read_manifest(dir: &Path) -> Result<PiManifest, String> {
    let path = dir.join("manifest.json");
    let meta = std::fs::metadata(&path).map_err(|error| {
        format!(
            "随包 Pi 运行时缺 manifest（{}）：{error}。请重装或重新组装运行时。",
            path.display()
        )
    })?;
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err("随包 Pi 运行时 manifest 超过读取上限，已拒绝。".into());
    }
    let bytes =
        std::fs::read(&path).map_err(|error| format!("读不到随包 Pi 运行时 manifest：{error}"))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("随包 Pi 运行时 manifest 读不出来：{error}"))
}

/// 当前系统对应的随包运行时平台标记。
///
/// 用运行期系统 / 架构判定，不用 `cfg!` 把非 Windows 一律当 linux：macOS 的包与
/// linux 的包平台标记不同，混用会把给 linux 组装的运行时误当成可用。
fn platform_for(os: &str, arch: &str) -> Result<&'static str, String> {
    match (os, arch) {
        ("windows", "x86_64") => Ok("win-x64"),
        // Windows ARM64 尚未组装随包运行时；其它 windows 架构同样没有。
        ("windows", _) => Err("Windows ARM64 版 Pi 运行时暂未提供，请安装对应平台的包。".into()),
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        _ => Err(format!(
            "当前平台（{os}/{arch}）没有随包 Pi 运行时，请安装对应平台的包。"
        )),
    }
}

/// 随包 Node 的相对路径：Windows 是 `node.exe`，其余平台是 `bin/node`。
fn node_relative() -> &'static str {
    if std::env::consts::OS == "windows" {
        "node.exe"
    } else {
        "bin/node"
    }
}

/// 文件指纹：（字节数, mtime 纳秒）。读不到就返回 None，调用方据此放弃缓存。
fn file_fingerprint(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let nanos = modified.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((metadata.len(), nanos.min(u64::MAX as u128) as u64))
}

/// 一次完整校验实际读过的文件指纹。node 与 manifest 之外，还要覆盖那几个按哈希
/// 抽查的入口文件：只比 node/manifest 会漏掉“同进程里依赖被换掉”的情况。
fn runtime_fingerprint(dir: &Path, node: &Path) -> Option<RuntimeFingerprint> {
    let paths = [
        node.to_path_buf(),
        dir.join("manifest.json"),
        dir.join("host/main.mjs"),
        dir.join("host/package.json"),
        dir.join("host/node_modules/@earendil-works/pi-durable/package.json"),
        dir.join("host/node_modules/@earendil-works/pi-ai/package.json"),
        dir.join("host/node_modules/@earendil-works/chord/package.json"),
    ];
    let mut parts = Vec::with_capacity(paths.len());
    for path in &paths {
        parts.push(file_fingerprint(path)?);
    }
    Some(RuntimeFingerprint(parts))
}

fn verified_cache() -> &'static VerifiedCache {
    VERIFIED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_verified(dir: &Path, fingerprint: &RuntimeFingerprint) -> bool {
    verified_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(dir)
        == Some(fingerprint)
}

fn remember_verified(dir: &Path, fingerprint: RuntimeFingerprint) {
    verified_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(dir.to_path_buf(), fingerprint);
}

/// 随包 / 开发运行时。`host` 指向 `main.mjs`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiRuntime {
    pub(crate) node: PathBuf,
    pub(crate) host: PathBuf,
    /// 来自 exe 邻接 `runtime/pi` 并经过 manifest 校验；开发环境变量模式为 false。
    pub(crate) bundled: bool,
}

impl PiRuntime {
    /// 生产优先 exe 邻接 `runtime/pi`；开发用环境变量显式指定。
    pub(crate) fn locate() -> Result<Self, String> {
        let mut tried = Vec::new();
        let mut problem: Option<String> = None;
        if let Some(dir) = env_absolute("GONGWEN_PI_RUNTIME_DIR")? {
            let runtime = Self::from_dir(&dir, env_absolute("GONGWEN_PI_NODE")?)?;
            runtime.verify_dev()?;
            return Ok(runtime);
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            let dir = parent.join("runtime").join("pi");
            tried.push(dir.clone());
            if dir.is_dir() {
                match Self::from_dir(&dir, None).and_then(|mut runtime| {
                    runtime.bundled = true;
                    runtime.verify_bundled(&dir)?;
                    Ok(runtime)
                }) {
                    Ok(runtime) => return Ok(runtime),
                    Err(error) => problem = Some(error),
                }
            }
        }
        let hint = if std::env::consts::OS == "windows" {
            "开发时先运行 scripts\\pi-dev.ps1 设置 GONGWEN_PI_RUNTIME_DIR / GONGWEN_PI_NODE；\
             安装版应随包携带 runtime/pi（Node 与 host）。"
        } else {
            "开发时先运行 scripts/pi-dev.sh 设置 GONGWEN_PI_RUNTIME_DIR / GONGWEN_PI_NODE；\
             安装版应随包携带 runtime/pi（Node 与 host）。"
        };
        Err(format!(
            "Pi 智能写作运行时未安装：{}。{}{hint} 不会自动下载依赖或调用 PATH 里的 pi。",
            problem
                .map(|problem| format!("{problem} "))
                .unwrap_or_default(),
            if tried.is_empty() {
                "未找到 exe 邻接的 runtime/pi。".to_string()
            } else {
                format!(
                    "查找过：{}。",
                    tried
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("、")
                )
            }
        ))
    }

    fn from_dir(dir: &Path, explicit_node: Option<PathBuf>) -> Result<Self, String> {
        let host = dir.join("host").join("main.mjs");
        let node = explicit_node
            .or_else(|| {
                [
                    dir.join("node.exe"),
                    dir.join("node"),
                    dir.join("bin").join("node"),
                ]
                .into_iter()
                .find(|path| path.is_file())
            })
            .ok_or_else(|| {
                format!(
                    "运行时目录里没有 Node：{}。请设置 GONGWEN_PI_NODE 指向 node 可执行文件。",
                    dir.display()
                )
            })?;
        Ok(Self {
            node,
            host,
            bundled: false,
        })
    }

    /// 开发模式：只查文件与 Node 最低版本，不冒充生产运行时。
    fn verify_dev(&self) -> Result<(), String> {
        self.verify_files()?;
        let version = node_version(&self.node)?;
        if version < (22, 19, 0) {
            return Err(format!(
                "Pi host 需要 Node >= 22.19.0，当前是 {}.{}.{}（开发运行时：{}）。",
                version.0,
                version.1,
                version.2,
                self.node.display()
            ));
        }
        Ok(())
    }

    /// 随包模式：manifest 必须存在且平台 / Node / host / 协议 / 关键文件哈希都对。
    ///
    /// 同一进程内对同一目录只做一次完整校验：文件指纹没变就直接返回。开发模式的
    /// [`Self::verify_dev`] 不受影响，仍然每次都查。
    fn verify_bundled(&self, dir: &Path) -> Result<(), String> {
        self.verify_files()?;
        let expected_platform = platform_for(std::env::consts::OS, std::env::consts::ARCH)?;
        let node_relative = node_relative();
        let cache_key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let fingerprint = runtime_fingerprint(dir, &self.node);
        if let Some(fingerprint) = &fingerprint
            && cached_verified(&cache_key, fingerprint)
        {
            return Ok(());
        }
        let manifest = read_manifest(dir)?;
        if manifest.schema != 1 {
            return Err(format!(
                "随包 Pi 运行时 manifest 版本是 {}，当前程序只认 1。请重装。",
                manifest.schema
            ));
        }
        if manifest.source != "npm-ci" {
            return Err("随包 Pi 运行时不是固定生产依赖构建，请重新组装。".into());
        }
        if manifest.platform != expected_platform {
            return Err(format!(
                "随包 Pi 运行时是给 {} 的，当前系统是 {expected_platform}。请安装对应平台的包。",
                manifest.platform
            ));
        }
        if manifest.node.version != EXPECTED_NODE_VERSION {
            return Err(format!(
                "随包 Pi 运行时的 Node 是 {}，应用需要 {EXPECTED_NODE_VERSION}。请重装。",
                manifest.node.version
            ));
        }
        if manifest.host.version != super::protocol::HOST_VERSION
            || manifest.host.protocol != super::protocol::PROTOCOL_VERSION
        {
            return Err(format!(
                "随包 Pi host 版本不匹配（host {} / 协议 {}，应用需要 {} / {}）。请重装。",
                manifest.host.version,
                manifest.host.protocol,
                super::protocol::HOST_VERSION,
                super::protocol::PROTOCOL_VERSION
            ));
        }
        let lock_hash =
            super::delivery::sha256_hex(include_str!("../../pi-host/package-lock.json"));
        if manifest.host.lockfile_sha256 != lock_hash {
            return Err("随包 Pi 依赖锁文件与应用不一致，请重新组装或重装。".into());
        }
        for relative in [
            node_relative,
            "host/main.mjs",
            "host/package.json",
            "host/node_modules/@earendil-works/pi-durable/package.json",
            "host/node_modules/@earendil-works/pi-ai/package.json",
            "host/node_modules/@earendil-works/chord/package.json",
        ] {
            let Some(expected) = manifest.file_hash(relative) else {
                return Err(format!(
                    "随包 Pi 运行时 manifest 没有列出 {relative}。请重装或重新组装运行时。"
                ));
            };
            let path = dir.join(relative);
            let mut file = std::fs::File::open(&path).map_err(|error| {
                format!(
                    "随包 Pi 运行时缺文件（{}）：{error}。请重装。",
                    path.display()
                )
            })?;
            let mut hasher = sha2::Sha256::new();
            let max = if relative == node_relative {
                128 * 1024 * 1024
            } else {
                8 * 1024 * 1024
            };
            if file.metadata().map_err(|error| error.to_string())?.len() > max {
                return Err(format!("随包运行时文件超过上限：{relative}"));
            }
            let mut reader = std::io::Read::take(&mut file, max + 1);
            let mut buffer = [0u8; 32 * 1024];
            let mut total = 0;
            loop {
                let count = std::io::Read::read(&mut reader, &mut buffer)
                    .map_err(|error| format!("校验运行时文件失败：{error}"))?;
                if count == 0 {
                    break;
                }
                total += count as u64;
                if total > max {
                    return Err(format!("随包运行时文件超过上限：{relative}"));
                }
                hasher.update(&buffer[..count]);
            }
            let digest = hasher.finalize();
            let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            if actual != expected {
                return Err(format!(
                    "随包 Pi 运行时文件校验失败（{relative}）。请重装。"
                ));
            }
        }
        let version = node_version(&self.node)?;
        if version != EXPECTED_NODE_VERSION_PARTS {
            return Err(format!(
                "随包 Node 版本不符：{}.{}.{}",
                version.0, version.1, version.2
            ));
        }
        // 只有完整走完所有校验才记缓存；失败路径不缓存，下次仍如实报错。
        if let Some(fingerprint) = fingerprint {
            remember_verified(&cache_key, fingerprint);
        }
        Ok(())
    }

    fn verify_files(&self) -> Result<(), String> {
        if !self.node.is_file() {
            return Err(format!("Node 不存在：{}", self.node.display()));
        }
        if !self.host.is_file() {
            return Err(format!(
                "Pi host 不存在：{}。随包运行时应有 runtime/pi/host/main.mjs。",
                self.host.display()
            ));
        }
        Ok(())
    }
}

fn env_absolute(name: &str) -> Result<Option<PathBuf>, String> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{name} 必须是绝对路径：{}", path.display()));
    }
    Ok(Some(path))
}

fn node_version(node: &Path) -> Result<(u32, u32, u32), String> {
    let output = hidden_command(node)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("运行 Node 版本检查失败（{}）：{error}", node.display()))?;
    if !output.status.success() {
        return Err(format!("Node 版本检查返回失败：{}", node.display()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts.next().and_then(|part| part.parse().ok());
    let minor = parts.next().and_then(|part| part.parse().ok());
    let patch = parts
        .next()
        .and_then(|part| part.split('-').next())
        .and_then(|part| part.parse().ok());
    match (major, minor, patch) {
        (Some(major), Some(minor), Some(patch)) => Ok((major, minor, patch)),
        _ => Err(format!("认不出 Node 版本号：{}", text.trim())),
    }
}

/// Windows 上隐藏控制台窗口；其余平台原样启动。参数用结构化数组，从不拼 shell。
pub(crate) fn hidden_command(program: &Path) -> Command {
    let command = Command::new(program);
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = command;
        command.creation_flags(CREATE_NO_WINDOW);
        command
    };
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "需要先组装真实随包运行时"]
    fn shipped_pi_runtime_manifest() {
        let dir = std::env::var_os("GONGWEN_PI_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/pi"));
        let runtime = PiRuntime::from_dir(&dir, None).unwrap();
        runtime.verify_bundled(&dir).unwrap();
    }

    #[test]
    fn platform_marks_come_from_the_running_system_not_a_cfg_guess() {
        assert_eq!(platform_for("windows", "x86_64"), Ok("win-x64"));
        assert_eq!(platform_for("linux", "x86_64"), Ok("linux-x64"));
        assert_eq!(platform_for("linux", "aarch64"), Ok("linux-arm64"));
        assert_eq!(platform_for("macos", "aarch64"), Ok("darwin-arm64"));
        assert_eq!(platform_for("macos", "x86_64"), Ok("darwin-x64"));
        // macOS 不能被当成 linux：两个平台的 manifest 平台标记不同。
        assert_ne!(
            platform_for("macos", "aarch64"),
            platform_for("linux", "aarch64")
        );
        let arm = platform_for("windows", "aarch64").unwrap_err();
        assert!(arm.contains("ARM64"), "{arm}");
        assert!(arm.contains("对应平台的包"), "{arm}");
        let unknown = platform_for("freebsd", "x86_64").unwrap_err();
        assert!(unknown.contains("freebsd/x86_64"), "{unknown}");
        assert!(unknown.contains("对应平台的包"), "{unknown}");
    }

    #[test]
    fn a_corrupt_bundled_node_is_rejected_before_execution() {
        // 当前平台没有随包运行时（如未知架构）时这条用例不适用。
        let Ok(platform) = platform_for(std::env::consts::OS, std::env::consts::ARCH) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("host")).unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        let node_name = node_relative();
        let node = dir.path().join(node_name);
        let host = dir.path().join("host/main.mjs");
        std::fs::write(&node, b"not an executable").unwrap();
        std::fs::write(&host, b"// test").unwrap();
        let manifest = serde_json::json!({
            "schema":1,"source":"npm-ci",
            "platform":platform,
            "node":{"version":EXPECTED_NODE_VERSION},
            "host":{"version":super::super::protocol::HOST_VERSION,"protocol":super::super::protocol::PROTOCOL_VERSION,
                "lockfileSha256":super::super::delivery::sha256_hex(include_str!("../../pi-host/package-lock.json"))},
            "files":[{"path":node_name,"sha256":"0".repeat(64)}]
        });
        std::fs::write(dir.path().join("manifest.json"), manifest.to_string()).unwrap();
        let runtime = PiRuntime {
            node,
            host,
            bundled: true,
        };
        assert!(
            runtime
                .verify_bundled(dir.path())
                .unwrap_err()
                .contains("文件校验失败")
        );
    }

    #[test]
    fn a_bad_bundled_manifest_is_a_readable_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.json"), b"{ not json").unwrap();
        let error = read_manifest(dir.path()).unwrap_err();
        assert!(error.contains("读不出来"), "{error}");
        std::fs::write(
            dir.path().join("manifest.json"),
            vec![b'x'; (MAX_MANIFEST_BYTES + 1) as usize],
        )
        .unwrap();
        let error = read_manifest(dir.path()).unwrap_err();
        assert!(error.contains("上限"), "{error}");
        std::fs::remove_file(dir.path().join("manifest.json")).unwrap();
        let error = read_manifest(dir.path()).unwrap_err();
        assert!(error.contains("manifest"), "{error}");
    }

    /// 版本一致性的唯一来源是 `packaging/pi-runtime.json`：改 Node / host / 协议时
    /// 必须同步 Rust 常量，这里把“漏改一处”变成编译期之后立刻失败的测试。
    #[test]
    fn packaging_pi_runtime_json_matches_rust_constants() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../packaging/pi-runtime.json"))
                .expect("packaging/pi-runtime.json 必须是合法 JSON");
        assert_eq!(config["node"], EXPECTED_NODE_VERSION);
        assert_eq!(
            config["host"]["version"],
            super::super::protocol::HOST_VERSION
        );
        assert_eq!(
            config["host"]["protocol"],
            super::super::protocol::PROTOCOL_VERSION
        );
        for platform in [
            "win-x64",
            "linux-arm64",
            "linux-x64",
            "darwin-arm64",
            "darwin-x64",
        ] {
            let entry = &config["platforms"][platform];
            assert!(
                entry["file"].as_str().is_some_and(|file| !file.is_empty()),
                "{platform} 缺少 Node 归档文件名"
            );
            assert!(
                entry["sha256"]
                    .as_str()
                    .is_some_and(|hash| hash.len() == 64),
                "{platform} 缺少 64 位 SHA-256"
            );
            let expected_binary = if platform == "win-x64" {
                "node.exe"
            } else {
                "bin/node"
            };
            assert_eq!(entry["binary"], expected_binary);
        }
    }
}
