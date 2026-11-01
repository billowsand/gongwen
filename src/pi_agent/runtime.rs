//! 运行时定位：随包 Node + host，或开发时显式指定的绝对路径。
//!
//! 不在 PATH 里找 `node` / `pi`，不运行 npm，也不读用户的 Pi 配置。缺少运行时
//! 时返回可操作的中文提示，由界面直接显示。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 随包 / 开发运行时。`host` 指向 `main.mjs`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiRuntime {
    pub(crate) node: PathBuf,
    pub(crate) host: PathBuf,
}

impl PiRuntime {
    /// 生产优先 exe 邻接 `runtime/pi`；开发用环境变量显式指定。
    pub(crate) fn locate() -> Result<Self, String> {
        let mut tried = Vec::new();
        let mut problem: Option<String> = None;
        if let Some(dir) = env_absolute("GONGWEN_PI_RUNTIME_DIR")? {
            let runtime = Self::from_dir(&dir, env_absolute("GONGWEN_PI_NODE")?)?;
            runtime.verify()?;
            return Ok(runtime);
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            let dir = parent.join("runtime").join("pi");
            tried.push(dir.clone());
            if dir.is_dir() {
                match Self::from_dir(&dir, None).and_then(|runtime| {
                    runtime.verify()?;
                    Ok(runtime)
                }) {
                    Ok(runtime) => return Ok(runtime),
                    Err(error) => problem = Some(error),
                }
            }
        }
        let hint = if cfg!(windows) {
            "开发时先运行 scripts\\pi-dev.ps1 设置 GONGWEN_PI_RUNTIME_DIR / GONGWEN_PI_NODE；\
             安装版应随包携带 runtime/pi（Node 与 host）。"
        } else {
            "开发时先运行 scripts/pi-dev.ps1 设置 GONGWEN_PI_RUNTIME_DIR / GONGWEN_PI_NODE；\
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
        Ok(Self { node, host })
    }

    fn verify(&self) -> Result<(), String> {
        if !self.node.is_file() {
            return Err(format!("Node 不存在：{}", self.node.display()));
        }
        if !self.host.is_file() {
            return Err(format!(
                "Pi host 不存在：{}。随包运行时应有 runtime/pi/host/main.mjs。",
                self.host.display()
            ));
        }
        let version = node_version(&self.node)?;
        if version < (22, 19, 0) {
            return Err(format!(
                "Pi host 需要 Node >= 22.19.0，当前是 {}.{}.{}（{}）。",
                version.0,
                version.1,
                version.2,
                self.node.display()
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
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
