#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod diff;
mod diff_view;
mod doc_import;
mod draft_page;
mod export;
mod highlight;
mod images;
mod knowledge;
mod knowledge_ui;
mod last_char_orphan;
mod lmstudio;
mod manuscript;
mod manuscript_io;
mod models;
mod portable_runtime;
mod preview;
mod prompt;
mod qa;
mod rag;
mod rag_client;
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([980.0, 680.0])
            .with_title(version::APP_TITLE)
            // 自绘窗口：去掉系统标题栏，标题与最小化/最大化/关闭按钮由应用自己
            // 绘制（见 app.rs 的 window_titlebar），三平台外观一致。
            .with_decorations(false)
            .with_icon(app_icon),
        ..Default::default()
    };

    eframe::run_native(
        version::APP_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(GongwenApp::new(cc)))),
    )
}
