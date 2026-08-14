/// AppKit 标题栏中需要由 egui 避让的原生控件尺寸。
#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeTitlebarMetrics {
    pub(crate) reserved_width: f32,
}

#[cfg(target_os = "macos")]
pub(crate) fn configure_native_titlebar(
    cc: &eframe::CreationContext<'_>,
) -> Option<NativeTitlebarMetrics> {
    // 即使极少数环境下拿不到 raw handle，也仍然是由 main.rs 创建的原生
    // AppKit 标题栏；只对避让宽度使用标准尺寸兜底，不能误画一套右侧按钮。
    Some(
        configure_native_titlebar_impl(cc).unwrap_or(NativeTitlebarMetrics {
            reserved_width: 88.0,
        }),
    )
}

#[cfg(target_os = "macos")]
fn configure_native_titlebar_impl(
    cc: &eframe::CreationContext<'_>,
) -> Option<NativeTitlebarMetrics> {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSView, NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask};
    use objc2_foundation::MainThreadMarker;
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

    MainThreadMarker::new()?;
    let window_handle = cc.window_handle().ok()?;
    let raw_handle = window_handle.as_raw();
    let RawWindowHandle::AppKit(handle) = raw_handle else {
        return None;
    };

    // SAFETY: raw-window-handle 保证该指针在窗口存活期内是有效的 NSView；
    // Retained 只在配置期间延长它的生命。
    let view = unsafe { Retained::<NSView>::retain(handle.ns_view.as_ptr().cast()) }?;
    let window = view.window()?;

    // 这些能力位必须属于窗口本身，而不只是创建按钮时使用的外观参数。
    // AppKit 的关闭、最小化、缩放和全屏都会检查相应能力位。
    let required_style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    window.setStyleMask(window.styleMask() | required_style);

    // 声明它是可进入独立 Space 的主窗口，并允许现代 macOS 在绿色按钮
    // 悬停菜单中提供左右平铺等窗口形态。
    let mut behavior = unsafe { window.collectionBehavior() };
    behavior.remove(
        NSWindowCollectionBehavior::FullScreenNone
            | NSWindowCollectionBehavior::FullScreenDisallowsTiling,
    );
    behavior.insert(
        NSWindowCollectionBehavior::FullScreenPrimary
            | NSWindowCollectionBehavior::FullScreenAllowsTiling,
    );
    unsafe { window.setCollectionBehavior(behavior) };

    // 使用 NSWindow 自己的按钮实例，保留系统目标/动作、Option 点击、
    // 全屏过渡动画、悬停菜单和辅助功能信息，不再手工绑定 performZoom:。
    for kind in [
        NSWindowButton::NSWindowCloseButton,
        NSWindowButton::NSWindowMiniaturizeButton,
        NSWindowButton::NSWindowZoomButton,
    ] {
        let button = window.standardWindowButton(kind)?;
        button.setHidden(false);
        button.setEnabled(true);
    }

    let chrome = eframe::WindowChromeMetrics::from_window_handle(&raw_handle)?;
    let zoom = cc.egui_ctx.zoom_factor().max(0.1);
    Some(NativeTitlebarMetrics {
        // 系统测出的按钮组宽度已包含左右边距，再加 10pt 隔开交互区。
        reserved_width: chrome.traffic_lights_size.x / zoom + 10.0,
    })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn configure_native_titlebar(
    _cc: &eframe::CreationContext<'_>,
) -> Option<NativeTitlebarMetrics> {
    None
}

/// 按 macOS“桌面与程序坞”中的标题栏双击偏好执行动作。
/// 未配置时采用系统常见默认值“缩放”。
#[cfg(target_os = "macos")]
pub(crate) fn perform_titlebar_double_click() {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSString, NSUserDefaults};

    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(main_thread);
    let Some(window) = app.keyWindow().or_else(|| unsafe { app.mainWindow() }) else {
        return;
    };
    let key = NSString::from_str("AppleActionOnDoubleClick");
    let action = unsafe { NSUserDefaults::standardUserDefaults().stringForKey(&key) }
        .map(|value| value.to_string())
        .unwrap_or_else(|| "Maximize".to_owned());

    // 不使用跨平台 Maximized：AppKit 的 zoom 会在用户尺寸和系统建议尺寸间切换，
    // miniaturize 则走 Dock 动画；None/Do Nothing 保持无动作。
    if action.eq_ignore_ascii_case("Minimize") {
        unsafe { window.performMiniaturize(None) };
    } else if !action.eq_ignore_ascii_case("None") && !action.eq_ignore_ascii_case("Do Nothing") {
        unsafe { window.performZoom(None) };
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn perform_titlebar_double_click() {}
