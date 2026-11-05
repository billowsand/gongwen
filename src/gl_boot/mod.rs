//! 启动前的图形环境兜底。
//!
//! 麒麟 V10 这类机器上，OpenGL 上下文可能建不起来：X 服务器回一条
//! 「GLXBadContextTag」错误，而 winit 对任何 X 错误都直接 panic，于是程序在
//! 第一次显示窗口时崩掉，清理阶段还会二次 panic，变成核心转储，还来不及
//! 留下有用的日志。
//!
//! 这里做三件事（Linux 专有）：
//!
//! 1. 建窗口之前先在子进程里探测一次图形能力（能不能拿到 OpenGL 上下文、
//!    版本多少、用的哪个驱动）。探测确认建不出上下文时，改用软件渲染
//!    （Mesa 的 llvmpipe）再探一次；其余情况不动环境变量，正常机器上
//!    行为与从前完全一样。
//! 2. 装一个 X11 错误钩子，把迟到的 GLX 错误接住记进日志，不再让程序崩；
//!    核心错误（BadWindow 之类）仍旧交回 winit，保持原样。
//! 3. 探测全在子进程里做：探测用的是裸 FFI，真出了段错误也只是子进程退出。
//!
//! 探测的结论、GLX 错误和降级过程都写进配置目录的 `logs/gl-boot.log`。

mod hook;
mod probe;

use std::sync::atomic::{AtomicU32, Ordering};

/// 置上这个环境变量时，本进程只做图形探测、打印一行结果就退出。
pub(super) const PROBE_ENV: &str = "GONGWEN_GL_PROBE";
/// 置上这个环境变量时跳过探测（探测误判时的后门）。
pub(super) const SKIP_ENV: &str = "GONGWEN_SKIP_GL_PROBE";

/// GLX 扩展的第一个错误码（探测子进程报上来）；0 表示还不知道。
static GLX_ERROR_BASE: AtomicU32 = AtomicU32::new(0);

/// GLX 扩展的第一个错误码；0 表示还不知道。
pub(super) fn glx_error_base() -> u32 {
    GLX_ERROR_BASE.load(Ordering::Relaxed)
}

/// 探测结论。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// 拿得到图形上下文，版本够用。
    Ok,
    /// 拿得到上下文，但版本偏旧（界面可能不正常）。
    Weak,
    /// 建不出图形上下文。
    Broken,
    /// 没法探测（没有 X、没有 GLX、库加载不了等）。
    Unknown,
}

/// 一次探测的结果。
#[derive(Clone, Debug)]
pub struct Report {
    pub verdict: Verdict,
    /// GLX 扩展的第一个错误码（-1 = 未知）。扩展错误码从这个号开始排，
    /// 拿它就能把 X 错误翻译成「GLXBadContextTag」这样的名字。
    pub glx_error_base: i32,
    /// OpenGL 版本串。
    pub version: String,
    /// 渲染器名（llvmpipe、Mali 之类）。
    pub renderer: String,
    /// 中文说明。
    pub detail: String,
}

impl Report {
    fn new(verdict: Verdict, detail: impl Into<String>) -> Self {
        Self {
            verdict,
            glx_error_base: -1,
            version: String::new(),
            renderer: String::new(),
            detail: detail.into(),
        }
    }

    /// 补上 GLX 的错误码起点。
    fn with_glx(mut self, error_base: i32) -> Self {
        self.glx_error_base = error_base;
        self
    }

    /// 一行中文摘要，写日志用。
    pub fn summary(&self) -> String {
        let mut text = String::new();
        if !self.version.is_empty() {
            text.push_str(&self.version);
        }
        if !self.renderer.is_empty() {
            if !text.is_empty() {
                text.push('（');
                text.push_str(&self.renderer);
                text.push('）');
            } else {
                text.push_str(&self.renderer);
            }
        }
        if text.is_empty() {
            text.push_str(&self.detail);
        }
        text
    }
}

/// 把结果编码成一行：用 Tab 分隔，值里的换行与 Tab 换成空格。
fn encode(report: &Report) -> String {
    let clean = |value: &str| value.replace(['\t', '\n', '\r'], " ");
    format!(
        "{}\t{}\t{}\t{}\t{}",
        match report.verdict {
            Verdict::Ok => "ok",
            Verdict::Weak => "weak",
            Verdict::Broken => "broken",
            Verdict::Unknown => "unknown",
        },
        report.glx_error_base,
        clean(&report.version),
        clean(&report.renderer),
        clean(&report.detail),
    )
}

/// 解析子进程打印的那一行。
fn decode(text: &str) -> Option<Report> {
    let line = text.lines().next()?.trim();
    let mut fields = line.split('\t');
    let verdict = match fields.next()? {
        "ok" => Verdict::Ok,
        "weak" => Verdict::Weak,
        "broken" => Verdict::Broken,
        _ => Verdict::Unknown,
    };
    let error_base = fields
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(-1);
    Some(Report {
        verdict,
        glx_error_base: error_base,
        version: fields.next().unwrap_or_default().to_owned(),
        renderer: fields.next().unwrap_or_default().to_owned(),
        detail: fields.next().unwrap_or_default().to_owned(),
    })
}

/// 版本够不够：桌面 OpenGL 要 3.3，OpenGL ES 2.0 就够。
fn verdict_of(version: &str) -> Verdict {
    let Some((major, minor)) = parse_version(version) else {
        return Verdict::Weak;
    };
    if version.contains("OpenGL ES") {
        return if major >= 2 {
            Verdict::Ok
        } else {
            Verdict::Weak
        };
    }
    if major > 3 || (major == 3 && minor >= 3) {
        Verdict::Ok
    } else {
        Verdict::Weak
    }
}

/// 从 "4.6 (Core Profile) Mesa 23.0.0" / "OpenGL ES 3.2 Mesa" 里取版本号。
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let head = match text.strip_prefix("OpenGL ES ") {
        Some(rest) => rest.split_whitespace().next()?,
        None => text.split_whitespace().next()?,
    };
    let mut parts = head.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    Some((major, minor))
}

/// 探测子进程的入口：环境变量置上时本进程只做探测，打印一行结果就退出。
///
/// 返回 `Some(退出码)` 表示「我是探测子进程」，父进程该等着拿这个退出码。
pub fn probe_child_entry() -> Option<i32> {
    probe::child_entry()
}

/// 建窗口之前调用：探测图形环境、必要时降级到软件渲染，并装上 X11 错误钩子。
pub fn prepare() {
    // 钩子先装：后面不管走哪条路，迟到的 X 错误都不会再把程序打崩。
    hook::install();

    if std::env::var_os(SKIP_ENV).is_some() {
        log_line("按环境变量跳过图形环境探测");
        return;
    }
    // Wayland 会话走 EGL / OpenGL ES，不经过 GLX，探测结论对不上，不动它。
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return;
    }
    if std::env::var_os("DISPLAY").is_none() {
        return;
    }

    let report = probe::run_child();
    remember_glx_error_base(&report);
    match report.verdict {
        Verdict::Ok => {
            log_line(&format!("图形环境可用：{}", report.summary()));
        }
        Verdict::Unknown => {
            log_line(&format!("没能探测图形环境：{}", report.detail));
        }
        Verdict::Weak => {
            log_line(&format!(
                "图形环境偏弱（{}），先用着看，真起不来再试软件渲染",
                report.summary()
            ));
        }
        Verdict::Broken => {
            log_line(&format!(
                "图形环境不可用（{}），改用软件渲染再试一次",
                report.detail
            ));
            enable_software_rendering();
            let retry = probe::run_child();
            remember_glx_error_base(&retry);
            match retry.verdict {
                Verdict::Ok => log_line(&format!(
                    "已切换到软件渲染，图形环境可用：{}",
                    retry.summary()
                )),
                Verdict::Weak => log_line(&format!(
                    "软件渲染下仍然只有较弱 OpenGL（{}），界面可能不正常",
                    retry.summary()
                )),
                Verdict::Broken => log_line(&format!(
                    "软件渲染下仍然建不出 OpenGL 上下文（{}），窗口可能起不来",
                    retry.detail
                )),
                Verdict::Unknown => log_line("软件渲染下没能再探测一次"),
            }
        }
    }
}

/// 记下 GLX 的错误码起点，好把 X 错误翻译成人能看懂的名字。
fn remember_glx_error_base(report: &Report) {
    GLX_ERROR_BASE.store(report.glx_error_base.max(0) as u32, Ordering::Relaxed);
}

/// 改用软件渲染。这两个变量只对 Mesa 有效，别的 GL 实现会忽略。
fn enable_software_rendering() {
    // SAFETY: 进程刚启动，还没开别的线程，此刻改环境变量不会与其他线程打架。
    unsafe {
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        std::env::set_var("GALLIUM_DRIVER", "llvmpipe");
    }
}

/// 启动失败时给用户的说法（写进日志，也打印到终端）。
pub fn startup_failure_hint() -> String {
    "这台机器的 OpenGL / GLX 不可用，窗口和图形上下文建不起来。\
     可以先在终端里用软件渲染启动：LIBGL_ALWAYS_SOFTWARE=1 公文助手；\
     若仍不行，请确认系统装了 Mesa 驱动（libgl1-mesa-dri），\
     或用 GONGWEN_SKIP_GL_PROBE=1 跳过探测再试。"
        .to_owned()
}

/// 把一行诊断写进配置目录的 `logs/gl-boot.log`。
pub fn log_line(line: &str) {
    use std::io::Write as _;
    let Ok(dir) = crate::storage::config_dir() else {
        return;
    };
    let dir = dir.join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("gl-boot.log"))
    else {
        return;
    };
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let _ = writeln!(file, "[{stamp}] {line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_parsed_from_gl_version_strings() {
        assert_eq!(
            parse_version("4.6 (Core Profile) Mesa 23.0.0"),
            Some((4, 6))
        );
        assert_eq!(parse_version("OpenGL ES 3.2 Mesa 23.0.0"), Some((3, 2)));
        assert_eq!(parse_version("2.1 Mesa"), Some((2, 1)));
        assert_eq!(parse_version("garbage"), None);
    }

    #[test]
    fn verdict_needs_gl_3_3_or_es_2() {
        assert_eq!(verdict_of("4.6 (Core Profile) Mesa"), Verdict::Ok);
        assert_eq!(verdict_of("3.3 (Core Profile) Mesa"), Verdict::Ok);
        assert_eq!(verdict_of("3.1 Mesa"), Verdict::Weak);
        assert_eq!(verdict_of("OpenGL ES 3.2 Mesa"), Verdict::Ok);
        assert_eq!(verdict_of("OpenGL ES 2.0 Mesa"), Verdict::Ok);
    }

    #[test]
    fn report_survives_a_round_trip_through_the_child_line() {
        let report = Report {
            verdict: Verdict::Weak,
            glx_error_base: 158,
            version: "3.1 Mesa".to_owned(),
            renderer: "llvmpipe".to_owned(),
            detail: "只建出弱上下文".to_owned(),
        };
        let decoded = decode(&encode(&report)).unwrap();
        assert_eq!(decoded.verdict, Verdict::Weak);
        assert_eq!(decoded.glx_error_base, 158);
        assert_eq!(decoded.version, "3.1 Mesa");
        assert_eq!(decoded.renderer, "llvmpipe");
        assert_eq!(decoded.detail, "只建出弱上下文");
    }
}
