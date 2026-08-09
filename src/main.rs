#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod diff;
mod diff_view;
mod draft_page;
mod export;
mod highlight;
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
    let app_icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/app-icon/app-icon-256.png"))
            .expect("embedded application icon must be a valid PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([980.0, 680.0])
            .with_title(version::APP_TITLE)
            .with_icon(app_icon),
        ..Default::default()
    };

    eframe::run_native(
        version::APP_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(GongwenApp::new(cc)))),
    )
}
