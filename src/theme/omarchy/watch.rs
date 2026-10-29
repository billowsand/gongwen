//! 后台轮询 Omarchy 当前主题，换了就重读，界面下一帧跟上。
//!
//! 每秒只 `stat` 几个文件，比对「指纹」：主题目录的真实路径（`current/theme` 是被整个
//! 替换的符号链接）、`colors.toml` 的修改时间与长度、`theme.name` 的修改时间。指纹变了
//! 才读文件、解析、换算，成功后递增 [`super::generation`] 并请界面重画；主线程按版本号
//! 重套样式。不在主线程做，不碰 Wayland 合成器的存活探测。
//!
//! Omarchy 切主题是先换目录、再渲染、再逐个通知应用，中途 `colors.toml` 可能短暂缺失
//! 或只写了一半。读失败时缓存里的上一份配色不动，也不记下这次指纹，下一秒再试。
//!
//! 不用 inotify：目录本身是被替换的符号链接，inotify 要跟着重挂，轮询更简单可靠。

use super::detect;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// 轮询间隔。Omarchy 切主题时终端、Waybar 也要一两秒才换完，一秒跟上不显得慢。
const INTERVAL: Duration = Duration::from_secs(1);

static STARTED: AtomicBool = AtomicBool::new(false);

/// 一个主题目录在某一刻的样子；任何一项变了都当作换了主题。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    dir: PathBuf,
    colors: (Option<SystemTime>, u64),
    name: Option<SystemTime>,
}

/// 起后台轮询线程；重复调用只起一条。启动时没读到过 Omarchy 配色就不起，
/// 不在没有 Omarchy 的机器上空转。
pub(crate) fn start(ctx: &egui::Context) {
    if !super::available() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("omarchy-theme-watch".into())
        .spawn(move || {
            // 启动时 `refresh` 刚读过一次，以那时的样子为起点，不重复换算。
            let mut last = detect::theme_dir().and_then(|dir| fingerprint(&dir));
            loop {
                std::thread::sleep(INTERVAL);
                if let Some(next) = poll(last.as_ref()) {
                    last = Some(next);
                    ctx.request_repaint();
                }
            }
        });
    if spawned.is_err() {
        STARTED.store(false, Ordering::SeqCst);
    }
}

/// 轮询一次：指纹没变或读失败返回 `None`；换了主题且重读成功返回新指纹。
fn poll(last: Option<&Fingerprint>) -> Option<Fingerprint> {
    let dir = detect::theme_dir()?;
    let next = fingerprint(&dir)?;
    if last == Some(&next) {
        return None;
    }
    super::refresh().ok()?;
    Some(next)
}

fn fingerprint(dir: &Path) -> Option<Fingerprint> {
    let colors = std::fs::metadata(dir.join(detect::COLORS_FILE)).ok()?;
    let name = dir
        .parent()
        .and_then(|parent| std::fs::metadata(parent.join("theme.name")).ok())
        .and_then(|meta| meta.modified().ok());
    Some(Fingerprint {
        dir: std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()),
        colors: (colors.modified().ok(), colors.len()),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_when_colors_or_name_change() {
        let root = tempfile::tempdir().expect("临时目录");
        let theme = root.path().join("theme");
        std::fs::create_dir(&theme).expect("建目录");
        assert_eq!(fingerprint(&theme), None, "没有 colors.toml 不算主题目录");

        let colors = theme.join(detect::COLORS_FILE);
        std::fs::write(&colors, "background = \"#000000\"\n").expect("写入");
        let first = fingerprint(&theme).expect("有指纹");
        assert_eq!(fingerprint(&theme).as_ref(), Some(&first), "没变就相同");

        std::fs::write(
            &colors,
            "background = \"#000000\"\nforeground = \"#ffffff\"\n",
        )
        .expect("写入");
        let second = fingerprint(&theme).expect("有指纹");
        assert_ne!(second, first, "长度变了指纹就变");

        std::fs::write(root.path().join("theme.name"), "tokyo-night\n").expect("写入");
        assert_ne!(
            fingerprint(&theme).expect("有指纹"),
            second,
            "主题名出现也算变"
        );
    }
}
