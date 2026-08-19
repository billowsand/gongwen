//! Linux 桌面会话探测。
//!
//! Omarchy 运行在 Hyprland/Wayland 上。平铺窗口不需要 Windows 风格的最小化、
//! 最大化按钮和八块自绘缩放命中区；保留关闭按钮与标题栏拖动即可。这里只按
//! compositor 会话能力调整窗口外壳，不把业务逻辑绑定到某个发行版名称。

#[cfg(target_os = "linux")]
pub fn hyprland_session() -> bool {
    use std::sync::OnceLock;

    static HYPRLAND: OnceLock<bool> = OnceLock::new();
    *HYPRLAND.get_or_init(|| {
        detects_hyprland(
            std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").as_deref(),
            std::env::var_os("XDG_CURRENT_DESKTOP").as_deref(),
            std::env::var_os("XDG_SESSION_DESKTOP").as_deref(),
        )
    })
}

#[cfg(not(target_os = "linux"))]
pub fn hyprland_session() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn detects_hyprland(
    signature: Option<&std::ffi::OsStr>,
    current_desktop: Option<&std::ffi::OsStr>,
    session_desktop: Option<&std::ffi::OsStr>,
) -> bool {
    signature.is_some_and(|value| !value.is_empty())
        || [current_desktop, session_desktop]
            .into_iter()
            .flatten()
            .any(|value| {
                value
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("hyprland")
            })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn detects_signature_or_desktop_name() {
        assert!(detects_hyprland(Some(OsStr::new("instance")), None, None));
        assert!(detects_hyprland(None, Some(OsStr::new("Hyprland")), None));
        assert!(detects_hyprland(
            None,
            Some(OsStr::new("something:Hyprland")),
            None
        ));
    }

    #[test]
    fn does_not_treat_an_ordinary_wayland_desktop_as_hyprland() {
        assert!(!detects_hyprland(
            None,
            Some(OsStr::new("GNOME")),
            Some(OsStr::new("gnome-wayland")),
        ));
    }
}
