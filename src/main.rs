#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod diff;
mod diff_view;
mod doc_import;
mod draft_page;
mod export;
mod highlight;
mod images;
mod ime;
mod knowledge;
mod knowledge_ui;
mod last_char_orphan;
mod linux_desktop;
mod lmstudio;
mod macos_window;
mod manuscript;
mod manuscript_io;
mod models;
mod orphan_probe;
mod pdf_viewer;
mod portable_runtime;
mod preview;
mod print_pdf;
mod prompt;
mod proofread;
mod proofread_rules;
mod proofread_xlsx;
mod qa;
mod rag;
mod rag_client;
mod redline;
mod storage;
mod system_fonts;
mod texcompile;
mod theme;
mod units;
mod validator;
mod version;
mod vocabulary_xlsx;

use app::GongwenApp;

fn main() -> eframe::Result {
    let app_icon = theme::app_icon(storage::load().unwrap_or_default().theme);
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 820.0])
        // 760 宽能在常见 1920 屏幕的二等分平铺与高 DPI 逻辑尺寸下正常容纳；
        // 功能区本身支持横向滚动，左侧表单最窄 332，中央区仍保留约 400。
        .with_min_inner_size([760.0, 600.0])
        .with_title(version::APP_TITLE)
        .with_app_id(version::APP_ID)
        .with_icon(app_icon);

    // macOS 保留真正的 AppKit 标题栏和标准红黄绿按钮，仅把标题栏设为透明，
    // 让 egui 顶栏内容延伸到其下方；Windows/Linux 继续使用无边框自绘方案。
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_decorations(true)
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false)
        .with_titlebar_buttons_shown(true);
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_decorations(false);

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        version::APP_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(GongwenApp::new(cc)))),
    )
}
