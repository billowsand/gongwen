//! X11 错误钩子。
//!
//! winit 收到任何 X 错误都会 panic（`flush_requests().expect("Failed to call XMapRaised")`）。
//! 可 X11 的错误是异步送达的：一条早先的 GLX 错误可能直到窗口第一次显示、
//! 冲刷请求时才被取出来，于是报成「显示窗口失败」，程序当场崩掉，清理阶段
//! 还会因为互斥量被毒化再崩一次，最后变成核心转储。
//!
//! 这里把扩展类错误（GLX 等，错误码 ≥ 128）接住、记进日志，不再往上抛；
//! 核心错误（BadWindow、BadMatch 之类，错误码 < 128）仍旧交回 winit，
//! 保持原来的行为。

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

/// 扩展错误从 128 号开始（X11 的 FirstExtensionError）。
const FIRST_EXTENSION_ERROR: u32 = 128;
/// 最多记多少条扩展错误，避免刷屏。
const MAX_LOGGED: u32 = 50;

/// 已经记了多少条扩展错误。
static LOGGED: AtomicU32 = AtomicU32::new(0);

/// GLX 的错误名（按 GLX 协议里的编号，编号 = 错误码 − 扩展错误码起点）。
const GLX_ERROR_NAMES: [&str; 14] = [
    "GLXBadContext",
    "GLXBadContextState",
    "GLXBadDrawable",
    "GLXBadPixmap",
    "GLXBadContextTag",
    "GLXBadCurrentWindow",
    "GLXBadRenderRequest",
    "GLXBadLargeRequest",
    "GLXUnsupportedPrivateRequest",
    "GLXBadFBConfig",
    "GLXBadPbuffer",
    "GLXBadCurrentDrawable",
    "GLXBadWindow",
    "GLXBadProfileARB",
];

/// 装上钩子。Linux 的 X11 后端才有这回事，其它平台空转。
pub fn install() {
    #[cfg(target_os = "linux")]
    {
        winit::platform::x11::register_xlib_error_hook(Box::new(on_x_error));
    }
}

/// winit 的 X11 错误回调。
///
/// 返回 `true` 表示这条错误已经处理，winit 不会再把它当成致命错误。
fn on_x_error(_display: *mut c_void, event: *mut c_void) -> bool {
    // SAFETY: winit 传进来的是 Xlib 的错误事件，Xlib 保证回调期间它有效；
    // 这里只读前几个字段，读完就返回。
    let Some(event) = (unsafe { event.cast::<x11_dl::xlib::XErrorEvent>().as_ref() }) else {
        return false;
    };
    let code = event.error_code as u32;
    if code < FIRST_EXTENSION_ERROR {
        // 核心错误：交给 winit 原样处理。
        return false;
    }
    let name = glx_error_name(code);
    let logged = LOGGED.fetch_add(1, Ordering::Relaxed);
    if logged < MAX_LOGGED {
        super::log_line(&format!(
            "X 扩展错误：{name}（错误码 {code}，请求 {}{}）",
            event.request_code,
            if event.minor_code > 0 {
                format!("，子请求 {}", event.minor_code)
            } else {
                String::new()
            }
        ));
    }
    true
}

/// GLX 错误的名字；不是 GLX 的错误就只报错误码。
fn glx_error_name(code: u32) -> String {
    let base = super::glx_error_base();
    if base > 0 && code >= base {
        let index = (code - base) as usize;
        if let Some(name) = GLX_ERROR_NAMES.get(index) {
            return (*name).to_owned();
        }
    }
    format!("扩展错误 {code}")
}
