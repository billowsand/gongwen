use super::*;
use crate::app::{DraftAction, management_samples};
use crate::models::{RibbonTab, TemplateKind};
use egui_kittest::kittest::Queryable;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const MARKDOWN: &str = "# 排版预览\n\n未保存内容\n\n\
    ```mermaid\nflowchart LR\nA[受理] --> B[办理]\n```\n";

fn app_with_draft() -> GongwenApp {
    let mut app = management_samples::fixture();
    app.config.auto_save = false;
    app.config.ribbon_tab = RibbonTab::Output;
    app.docs[0].draft.kind = TemplateKind::PlainDocument;
    app.docs[0].generated_markdown = MARKDOWN.into();
    app
}

fn wait_for_layout(app: &mut GongwenApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.draft_pdf_is_compiling() && Instant::now() < deadline {
        app.poll_worker(ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.draft_pdf_is_compiling(), "后台排版未完成");
}

fn directory_entries(dir: &Path) -> Vec<std::ffi::OsString> {
    let mut entries: Vec<_> = dir
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    entries.sort();
    entries
}

#[test]
fn draft_pdf_previews_unsaved_content_without_export_password_or_disk_output() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    storage::set_test_config_dir(Some(dir.path().to_owned()));
    let mut app = app_with_draft();
    app.config.output_dir = dir.path().join("output").display().to_string();
    app.config.export.print_trace.enabled = true;
    app.vault_locked = true;
    crate::vault::set_key_handle([17; 32], true);
    assert!(crate::export::protection::Protection::current().is_err());
    let before = directory_entries(dir.path());

    let ctx = egui::Context::default();
    app.draft_actions.push(DraftAction::PreviewPdf(0));
    app.apply_draft_actions(&ctx);
    assert_eq!(app.tabs[app.active_tab], TabRef::DraftPdf(0));
    wait_for_layout(&mut app, &ctx);
    let preview = &app.draft_pdfs.tabs[&0];
    assert!(preview.error.is_none(), "{:?}", preview.error);
    assert!(
        !preview
            .warnings
            .iter()
            .any(|warning| warning.contains("插图文件不存在"))
    );
    let pdf = preview.pdf.as_ref().unwrap();
    let first_key = pdf.key;
    let bytes = pdf.memory_bytes().unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    let hits = search::matching_pages(bytes, "未保存内容", &AtomicBool::new(false)).unwrap();
    assert_eq!(hits, vec![0]);

    app.activate_doc(0);
    app.start_draft_pdf(0, &ctx, false);
    assert!(!app.draft_pdf_is_compiling(), "未修改时应复用标签");
    assert_eq!(app.draft_pdfs.tabs[&0].pdf.as_ref().unwrap().key, first_key);
    app.activate_doc(0);
    app.docs[0].generated_markdown = "# 排版预览\n\n第二轮正文".into();
    assert!(
        !app.draft_pdfs.tabs[&0]
            .displayed
            .as_ref()
            .unwrap()
            .matches(&app.docs[0], &app.config)
    );
    app.start_draft_pdf(0, &ctx, true);
    wait_for_layout(&mut app, &ctx);
    assert_eq!(app.tabs.len(), 2);
    let preview = &app.draft_pdfs.tabs[&0];
    assert!(preview.error.is_none(), "{:?}", preview.error);
    let pdf = preview.pdf.as_ref().unwrap();
    assert_ne!(pdf.key, first_key);
    let bytes = pdf.memory_bytes().unwrap();
    assert_eq!(
        search::matching_pages(bytes, "第二轮正文", &AtomicBool::new(false)).unwrap(),
        vec![0]
    );
    assert!(app.docs[0].output_files.is_empty());
    assert!(app.pending_output.is_none());
    assert_eq!(directory_entries(dir.path()), before);
    assert!(!dir.path().join("output").exists());
    assert!(!dir.path().join("mermaid-cache").exists());
    crate::vault::clear_key_handle();
    storage::set_test_config_dir(None);
}

#[test]
fn draft_pdf_ignores_completion_after_close_or_lock() {
    let ctx = egui::Context::default();
    let mut app = app_with_draft();
    let dir = tempfile::tempdir().unwrap();
    storage::set_test_config_dir(Some(dir.path().to_owned()));
    app.start_draft_pdf(0, &ctx, false);
    assert!(app.draft_pdf_is_compiling());
    app.close_tab(app.active_tab);
    assert_eq!(app.tabs[app.active_tab], TabRef::Doc(0));
    assert!(app.draft_pdfs.tabs.is_empty());
    assert!(app.draft_pdf_is_compiling(), "锁定前仍须等待排版线程退出");
    wait_for_layout(&mut app, &ctx);
    assert!(app.draft_pdfs.tabs.is_empty());

    app.vault_locked = true;
    app.start_draft_pdf(0, &ctx, false);
    app.request_lock();
    assert_eq!(app.tabs[app.active_tab], TabRef::Doc(0));
    assert!(app.draft_pdfs.tabs.is_empty());
    assert!(app.lock_requested);
    assert!(app.draft_pdf_is_compiling());
    wait_for_layout(&mut app, &ctx);
    assert!(app.draft_pdfs.tabs.is_empty());
    assert!(!app.tabs.contains(&TabRef::DraftPdf(0)));
    storage::set_test_config_dir(None);
}

#[test]
fn draft_pdf_typesets_research_reports_in_memory() {
    let Some(fonts) = crate::portable_runtime::find_font_dir() else {
        return;
    };
    if crate::portable_runtime::validate_research_fonts(&fonts).is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_draft();
    app.docs[0].draft.kind = TemplateKind::ResearchReport;
    app.docs[0].generated_markdown =
        "# 研究报告预览\n\n## 计算说明\n\n研究报告正文 $x^2$。\n".into();
    let snapshot = PdfInput::capture(&app.docs[0], &app.config);
    let pdf = snapshot.compile(dir.path()).unwrap();
    assert!(pdf.pages > 0);
    assert!(pdf.pdf.starts_with(b"%PDF-"));
    assert!(dir.path().read_dir().unwrap().next().is_none());
}

#[test]
fn draft_pdf_ribbon_click_opens_preview_instead_of_export() {
    let mut app = app_with_draft();
    let dir = tempfile::tempdir().unwrap();
    storage::set_test_config_dir(Some(dir.path().to_owned()));
    app.config.output_dir = dir.path().join("output").display().to_string();
    let mut harness = crate::test_support::harness_ui_state(
        |ui, app| {
            let ctx = ui.ctx().clone();
            app.poll_worker(&ctx);
            match app.tabs[app.active_tab] {
                TabRef::Doc(_) => {
                    ui.horizontal(|ui| app.draft_page().ribbon_output(ui));
                    app.apply_draft_actions(&ctx);
                }
                TabRef::DraftPdf(key) => app.draft_pdf_ui(key, ui),
                _ => {}
            }
        },
        app,
    );
    harness.set_size(egui::vec2(760.0, 600.0));
    harness.run();
    harness.get_by_label("PDF 预览").click();
    harness.step();
    harness.step();
    assert!(harness.query_by_label("返回稿件").is_some());
    assert!(harness.query_by_label("刷新预览").is_some());
    assert!(harness.query_by_label("打印").is_none());
    let app = harness.state_mut();
    assert!(app.pending_output.is_none());
    wait_for_layout(app, &egui::Context::default());
    if crate::portable_runtime::find_font_dir().is_some() {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let pdf = harness.state_mut().draft_pdfs.tabs[&0]
                .pdf
                .as_ref()
                .unwrap();
            if pdf.page_texture(0).is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "预览的首页未交给查看器");
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(harness.query_by_label("打印").is_none());
        assert!(harness.query_by_label("另存并系统打开").is_none());
    }
    harness.get_by_label("返回稿件").click();
    harness.step();
    harness.step();
    harness.get_by_label("PDF 导出与文件操作").click();
    harness.step();
    harness.step();
    assert!(harness.query_by_label("导出 PDF 并打开").is_some());
    storage::set_test_config_dir(None);
}
