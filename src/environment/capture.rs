//! Windows 窗口捕获排除。只验证本窗口的防护状态，不声称检测截屏动作。

#[cfg(windows)]
use std::ptr;
#[cfg(windows)]
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows_sys::Win32::Foundation::HWND;

#[cfg(windows)]
const CHECK_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct CaptureGuard {
    #[cfg(windows)]
    hwnd: HWND,
    #[cfg(windows)]
    requested: bool,
    #[cfg(windows)]
    healthy: bool,
    #[cfg(windows)]
    last_check: Option<Instant>,
}

impl CaptureGuard {
    #[cfg(test)]
    pub(crate) fn disabled_for_test() -> Self {
        Self {
            #[cfg(windows)]
            hwnd: ptr::null_mut(),
            #[cfg(windows)]
            requested: false,
            #[cfg(windows)]
            healthy: true,
            #[cfg(windows)]
            last_check: None,
        }
    }

    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
            let hwnd = cc
                .window_handle()
                .ok()
                .and_then(|handle| match handle.as_raw() {
                    RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
                    _ => None,
                })
                .unwrap_or_else(ptr::null_mut);
            return Self {
                hwnd,
                requested: false,
                healthy: true,
                last_check: None,
            };
        }
        #[cfg(not(windows))]
        {
            let _ = cc;
            Self {}
        }
    }

    pub(crate) fn update(&mut self, requested: bool) -> bool {
        #[cfg(windows)]
        {
            let now = Instant::now();
            if self.requested == requested
                && self
                    .last_check
                    .is_some_and(|last| now.saturating_duration_since(last) < CHECK_INTERVAL)
            {
                return self.healthy;
            }
            self.healthy = if self.hwnd.is_null() {
                !requested
            } else if !requested {
                set_affinity(self.hwnd, false)
            } else if self.requested {
                // 持续复核只读当前状态：保护被移除时不能先偷偷补设再报健康，
                // 否则界面承诺的「被移除时锁定」永远触发不了。
                affinity_is_protected(self.hwnd)
            } else {
                // 首次启用或从关闭切回启用时，才允许主动设置。
                set_affinity(self.hwnd, true) && affinity_is_protected(self.hwnd)
            };
            self.requested = requested;
            self.last_check = Some(now);
            return self.healthy;
        }
        #[cfg(not(windows))]
        {
            // 配置包跨平台移动时，Windows 专用开关不能让其他平台误判为不安全。
            let _ = requested;
            true
        }
    }
}

#[cfg(windows)]
fn set_affinity(hwnd: HWND, protected: bool) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };
    let affinity = if protected {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: hwnd 来自本进程 CreationContext 的顶层 Win32 窗口；窗口存活期覆盖 AppShell。
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) != 0 }
}

#[cfg(windows)]
fn affinity_is_protected(hwnd: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR,
    };
    let mut affinity = 0;
    // SAFETY: hwnd 有效且 affinity 指向本函数栈上的可写 u32。
    (unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) != 0 })
        && (affinity == WDA_EXCLUDEFROMCAPTURE || affinity == WDA_MONITOR)
}
