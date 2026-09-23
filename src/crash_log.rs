//! 崩溃日志。
//!
//! 从桌面图标、开始菜单双击打开时没有终端，程序一旦 panic 或窗口建不起来，
//! 用户看到的只是「闪退」，原因全丢了。这里在进程最早处装一个 panic hook，
//! 把错误、调用栈和系统环境写进用户配置目录的 `logs/`，下次启动在状态栏提示
//! 日志位置，用户把文件发过来就能定位。
//!
//! 只接得住 Rust 层的 panic 与 `eframe::run_native` 返回的错误；显卡驱动里的
//! 段错误这类信号级崩溃不经过这里。

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// 日志目录里最多保留这么多份崩溃日志，更旧的在写新日志时删掉。
const MAX_REPORTS: usize = 20;
/// 写过崩溃日志后留下的标记，内容是最新那份日志的路径；下次启动读到就提示并删掉。
const UNSEEN_MARKER: &str = ".unseen-crash";
const REPORT_PREFIX: &str = "crash-";

/// 同一次进程里只写第一份：panic 之后连锁出的错误多半只是余波。
static WRITTEN: AtomicBool = AtomicBool::new(false);

/// 崩溃日志所在目录：`config_dir()/logs/`。
pub fn log_dir() -> Option<PathBuf> {
    crate::storage::config_dir()
        .ok()
        .map(|dir| dir.join("logs"))
}

/// 装 panic hook。必须在 `main` 最开头调用，赶在读配置、装字体之前。
///
/// 原来的 hook 照常执行，终端里启动时 stderr 的输出不变。
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("<unnamed>");
        let message = panic_message(info.payload());
        let location = info
            .location()
            .map(|location| location.to_string())
            .unwrap_or_else(|| "未知位置".into());
        let backtrace = std::backtrace::Backtrace::force_capture();
        let detail =
            format!("线程：{thread}\n位置：{location}\n信息：{message}\n\n调用栈：\n{backtrace}");
        if let Some(path) = write_report("程序异常（panic）", &detail) {
            eprintln!("崩溃日志已写入：{}", path.display());
        }
        previous(info);
    }));
}

/// 记录 `eframe::run_native` 返回的启动错误（窗口、图形上下文建不起来等）。
pub fn record_startup_error(error: &dyn std::fmt::Display) {
    let detail = format!("信息：{error}");
    if let Some(path) = write_report("启动失败（无法创建窗口或图形上下文）", &detail)
    {
        eprintln!("崩溃日志已写入：{}", path.display());
    }
}

/// 上次运行留下、还没提示过的崩溃日志。读到就把标记删掉，只提示一次。
pub fn take_unseen_report() -> Option<PathBuf> {
    take_unseen_in(&log_dir()?)
}

fn take_unseen_in(dir: &Path) -> Option<PathBuf> {
    let marker = dir.join(UNSEEN_MARKER);
    let path = fs::read_to_string(&marker).ok()?;
    let _ = fs::remove_file(&marker);
    let path = PathBuf::from(path.trim());
    path.is_file().then_some(path)
}

fn write_report(kind: &str, detail: &str) -> Option<PathBuf> {
    if WRITTEN.swap(true, Ordering::SeqCst) {
        return None;
    }
    let dir = log_dir()?;
    write_report_in(&dir, kind, detail, chrono::Local::now())
}

fn write_report_in(
    dir: &Path,
    kind: &str,
    detail: &str,
    now: chrono::DateTime<chrono::Local>,
) -> Option<PathBuf> {
    fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!(
        "{REPORT_PREFIX}{}.log",
        now.format("%Y%m%d-%H%M%S-%3f")
    ));
    let mut file = fs::File::create(&path).ok()?;
    file.write_all(render_report(kind, detail, &now).as_bytes())
        .ok()?;
    let _ = fs::write(dir.join(UNSEEN_MARKER), path.display().to_string());
    prune(dir);
    Some(path)
}

fn render_report(kind: &str, detail: &str, now: &chrono::DateTime<chrono::Local>) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "公文助手崩溃日志");
    let _ = writeln!(text, "类型：{kind}");
    let _ = writeln!(text, "时间：{}", now.format("%Y-%m-%d %H:%M:%S %:z"));
    let _ = writeln!(text, "版本：{}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        text,
        "平台：{} / {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if let Some(name) = os_pretty_name() {
        let _ = writeln!(text, "系统：{name}");
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = writeln!(text, "程序：{}", exe.display());
    }
    // 图形会话相关的环境变量：闪退多半与 X11/Wayland、驱动选择有关。
    for key in [
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "LANG",
        "LIBGL_ALWAYS_SOFTWARE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            let _ = writeln!(text, "{key}={}", value.to_string_lossy());
        }
    }
    let _ = writeln!(text, "\n{detail}");
    text
}

/// Linux 发行版名称（`/etc/os-release` 的 PRETTY_NAME），其他平台返回 `None`。
fn os_pretty_name() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let text = fs::read_to_string("/etc/os-release").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim_matches('"').to_owned())
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "（非文本 panic 信息）".into()
    }
}

/// 只留最新的 `MAX_REPORTS` 份。文件名带时间戳，按名字排序即按时间排序。
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut reports: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(REPORT_PREFIX) && name.ends_with(".log"))
        })
        .collect();
    if reports.len() <= MAX_REPORTS {
        return;
    }
    reports.sort();
    for old in &reports[..reports.len() - MAX_REPORTS] {
        let _ = fs::remove_file(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(second: u32) -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 9, 23, 10, 0, second)
            .unwrap()
    }

    #[test]
    fn report_has_environment_and_detail_and_is_announced_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_report_in(dir.path(), "测试", "信息：boom", at(1)).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("类型：测试"));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("信息：boom"));

        assert_eq!(take_unseen_in(dir.path()), Some(path));
        assert_eq!(take_unseen_in(dir.path()), None, "只提示一次");
    }

    #[test]
    fn keeps_only_the_newest_reports() {
        let dir = tempfile::tempdir().unwrap();
        for second in 0..(MAX_REPORTS as u32 + 5) {
            write_report_in(dir.path(), "测试", "", at(second)).unwrap();
        }
        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.starts_with(REPORT_PREFIX))
            .collect();
        names.sort();
        assert_eq!(names.len(), MAX_REPORTS);
        assert!(
            names[0].contains("100005"),
            "最旧的 5 份应被删掉：{names:?}"
        );
    }
}
