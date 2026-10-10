//! 管理页的真实 UI 样张：内存数据与三帧布局，不读取用户的词库或稿件。
use super::*;
use crate::models::{ManuscriptStatus, VocabularyCategory, VocabularyEntry};

fn fixture() -> GongwenApp {
    let config = AppConfig {
        theme: crate::models::ThemeName::Smartisan,
        ime: crate::models::ImeConfig {
            enabled: false,
            ..Default::default()
        },
        vocabulary_setup: VocabularySetupStatus::Completed,
        ..Default::default()
    };
    let kind = config.last_template;
    let ime = crate::ime::Ime::new(session::ime_settings_of(&config.ime));
    let docs = vec![DraftSession::blank(0, &config)];
    let (sender, receiver) = mpsc::channel();
    let manuscript_store =
        Some(manuscript::ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap());
    let manuscript_error = None;
    let knowledge_store =
        Some(knowledge::KnowledgeStore::open(std::path::Path::new(":memory:")).unwrap());
    let knowledge_error = None;
    let lexicon_store =
        Some(lexicon::LexiconStore::open(std::path::Path::new(":memory:")).unwrap());
    let lexicon_error = None;
    let mut app = GongwenApp {
        config,
        ime,
        metrics: crate::metrics::Metrics::default(),
        send_package: None,
        send_package_export: None,
        macos_titlebar_metrics: None,
        docs,
        pdfs: Vec::new(),
        active_doc: 0,
        next_doc_key: 1,
        next_pdf_key: 1,
        draft_actions: Vec::new(),
        export_links: ExportLinks::default(),
        provider_status: std::collections::HashMap::new(),
        provider_edit: std::collections::HashSet::new(),
        model_filter: BTreeMap::new(),
        model_picker_show_all: std::collections::HashSet::new(),
        vocabulary_import_conflicts: None,
        vocabulary_dirty: false,
        vocabulary_setup_name: String::new(),
        vocabulary_move: None,
        vocabulary_sort_undo: None,
        about_window_open: false,
        help: crate::help::HelpState::default(),
        quick_find: None,
        modal_guard: crate::modal::ModalGuard::default(),
        vocabulary_filter: String::new(),
        vocabulary_selected: None,
        vocabulary_collapsed: BTreeSet::new(),
        vocabulary_delete_confirm: None,
        vocabulary_clear_confirm: false,
        proofread_page: ProofreadPageState::default(),
        ai_prompt_editor: None,
        ai_prompt_selected: None,
        ai_prompt_delete_confirm: None,
        ai_contract_preview_kind: kind,
        manuscript_store,
        manuscript_error,
        manuscript_filter: ManuscriptFilter::default(),
        manuscript_applied: None,
        manuscript_dirty: true,
        manuscript_rows: Vec::new(),
        manuscript_selected: BTreeSet::new(),
        manuscript_count: [0; 4],
        manuscript_delete_confirm: None,
        manuscript_delete_refs: Vec::new(),
        manuscript_batch_delete: None,
        manuscript_pdf_export_busy: false,
        manuscript_pdf_export: None,
        manuscript_zip_password: None,
        remembered_zip_password: None,
        manuscript_archive_pending: None,
        manuscript_detail: None,
        manuscript_detail_delete_pdf: None,
        manuscript_import_preview: None,
        pending_merge: None,
        version_commit: None,
        version_diff: None,
        version_switch: None,
        switch_after_commit: None,
        revert_confirm: None,
        manuscript_versions: Vec::new(),
        manuscript_package_titles: Vec::new(),
        manuscript_referrers: Vec::new(),
        config_versions_open: false,
        config_apply_confirm: None,
        tabs: vec![TabRef::Doc(0)],
        active_tab: 0,
        close_confirm: None,
        exit_prompt: None,
        exit_confirmed: false,
        last_autosave: std::time::Instant::now(),
        // 上次闪退留下了日志就先告诉用户在哪，方便发给维护者排查。
        status: "样张数据 · 仅用于界面检查".into(),
        busy: false,
        sender,
        receiver,
        knowledge_store,
        knowledge_error,
        knowledge_docs: Vec::new(),
        knowledge_chunk_count: 0,
        knowledge_retrieval_elapsed: None,
        knowledge_filter_kind: None,
        knowledge_dirty: true,
        knowledge_delete_confirm: None,
        knowledge_list: Default::default(),
        knowledge_index_progress: None,
        knowledge_progress_verb: "正在建立索引",
        knowledge_index_result: None,
        knowledge_unindexed: 0,
        knowledge_test_query: String::new(),
        knowledge_test_results: Vec::new(),
        knowledge_mode: KnowledgeMode::default(),
        knowledge_qa_history: Vec::new(),
        knowledge_qa_pending: None,
        knowledge_search_warnings: Vec::new(),
        knowledge_indexed_manuscripts: std::collections::HashSet::new(),
        knowledge_embed_models: Vec::new(),
        knowledge_busy: false,
        knowledge_import: None,
        lexicon_store,
        lexicon_error,
        lexicon_stats: lexicon::LexiconStats::default(),
        lexicon_terms: Vec::new(),
        lexicon_filter: lexicon::TermFilter::default(),
        lexicon_dirty: true,
        lexicon_busy: false,
        lexicon_scan_options: lexicon::scan::ScanOptions::default(),
        lexicon_scan_progress: None,
        lexicon_scan_result: None,
        lexicon_export: lexicon::export::ExportOptions::default(),
        lexicon_preview: None,
        ime_lexicon_changed_at: None,
        lexicon_export_result: None,
        lexicon_new_term: String::new(),
        lexicon_clear_confirm: false,
        system_fonts: Vec::new(),
        system_fonts_busy: false,
        system_fonts_scanned: false,
        font_filter: BTreeMap::new(),
        last_content_tab: None,
        settings_section: SettingsSection::default(),
        ai_section: AiSection::default(),
        agent_settings: Default::default(),
        styles_page: Default::default(),
        rerank_verify_busy: false,
        rerank_verify_result: None,
        knowledge_preview: None,
    };
    app.manuscript_dirty = false;
    app.manuscript_applied = Some(app.manuscript_filter.clone());
    for (i, title) in [
        "年度工作总结",
        "研究报告全格式测试",
        "公文助手表格格式测试合集",
        "政策研究材料",
        "会议纪要",
        "附件边界测试",
        "数学公式排版测试",
    ]
    .iter()
    .enumerate()
    {
        app.manuscript_rows.push(manuscript::ManuscriptRow {
            id: i as i64 + 1,
            title: (*title).into(),
            kind: TemplateKind::ResearchReport,
            status: ManuscriptStatus::Draft,
            security_level: "内部".into(),
            doc_number: String::new(),
            doc_date: "2026-10-09".into(),
            updated_at: "2026-10-09 12:30:00".into(),
            archived_at: None,
        });
    }
    app.manuscript_count = [0, 7, 0, 0];
    app.knowledge_dirty = false;
    for (i, title) in [
        "地震",
        "国际传播研究",
        "战略叙事：传播权力与世界秩序",
        "政策研究资料",
        "语言与权力",
        "公共管理案例",
        "组织行为研究",
        "作者的话",
    ]
    .iter()
    .enumerate()
    {
        app.knowledge_docs.push(knowledge::KnowledgeDocRow {
            id: i as i64 + 1,
            source: "external".into(),
            source_manuscript_id: None,
            source_path: String::new(),
            kind: TemplateKind::ResearchReport,
            title: (*title).into(),
            chunk_count: 132 + i as i64 * 17,
            embed_model: "sample".into(),
            created_at: "2026-10-09".into(),
            updated_at: "2026-10-09".into(),
        });
    }
    app.knowledge_chunk_count = 1848;
    app.knowledge_list.selected.insert(2);
    for name in [
        "政策研究",
        "规范管理",
        "信息公开",
        "年度总结",
        "基层治理",
        "公共服务",
        "风险防控",
        "协同机制",
    ]
    .iter()
    {
        app.lexicon_store
            .as_mut()
            .unwrap()
            .add_manual(name)
            .unwrap();
    }
    app.refresh_lexicon();
    app.config.vocabulary.push(VocabularyEntry {
        id: 1,
        category: VocabularyCategory::Unit,
        code: "01".into(),
        canonical: "政策研究办公室".into(),
        duties: "政策研究、综合协调、材料报送".into(),
        ..Default::default()
    });
    for (i, name) in [
        "欧阳景澄",
        "陈景澄",
        "陈知远",
        "欧阳知远",
        "陈若衡",
        "欧阳若衡",
    ]
    .iter()
    .enumerate()
    {
        app.config.vocabulary.push(VocabularyEntry {
            id: i as u64 + 2,
            category: VocabularyCategory::Person,
            canonical: (*name).into(),
            unit: "01".into(),
            position: "政策顾问".into(),
            phone: format!("62000{}", i + 1),
            profile: "负责政策研究与资料整理，参与综合材料起草和专题分析。".into(),
            ..Default::default()
        });
    }
    units::normalize(&mut app.config.vocabulary);
    app.vocabulary_selected = Some(2);
    app
}

#[test]
#[ignore = "出五个管理页样张，手动跑"]
fn smartisan_management_samples() {
    let ctx = egui::Context::default();
    theme::set_current(crate::models::ThemeName::Smartisan);
    theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    theme::configure_icons(&ctx);
    theme::configure_style(&ctx);
    ctx.set_pixels_per_point(1.0);
    let mut app = fixture();
    let mut canvas = crate::ui_snapshot::Canvas::default();
    for (index, (page, stem)) in [
        (NavPage::Manuscript, "manuscript"),
        (NavPage::Vocabulary, "vocabulary"),
        (NavPage::Proofread, "proofread"),
        (NavPage::Lexicon, "lexicon"),
        (NavPage::Knowledge, "knowledge"),
        (NavPage::Vocabulary, "vocabulary-narrow"),
        (NavPage::Manuscript, "manuscript-narrow"),
    ]
    .into_iter()
    .enumerate()
    {
        let size = egui::vec2(
            if stem.ends_with("narrow") {
                760.0
            } else {
                1280.0
            },
            820.0,
        );
        app.tabs = vec![TabRef::Page(page), TabRef::Doc(0)];
        app.active_tab = 0;
        for frame in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(index as f64 * 4.0 + frame as f64),
                    ..Default::default()
                },
                |ui| {
                    egui::Panel::top("sample_title")
                        .show_separator_line(false)
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| app.window_titlebar(ui));
                    egui::Panel::top("sample_tabs")
                        .show_separator_line(false)
                        .frame(theme::panel(theme::surface(), 12))
                        .show(ui, |ui| app.top_bar(ui));
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            theme::smartisan::management_page(ui, |ui| match page {
                                NavPage::Manuscript => app.manuscript_ui(ui),
                                NavPage::Vocabulary => app.vocabulary_ui(ui),
                                NavPage::Proofread => app.proofread_ui(ui),
                                NavPage::Lexicon => crate::lexicon_ui::lexicon_ui(&mut app, ui),
                                NavPage::Knowledge => {
                                    crate::knowledge_ui::knowledge_ui(&mut app, ui)
                                }
                                _ => unreachable!(),
                            });
                        });
                },
            );
            if frame < 2 {
                canvas.absorb(&output.textures_delta);
            } else {
                canvas.render(
                    &ctx,
                    output,
                    size,
                    theme::canvas(),
                    std::path::Path::new(&format!("tmp/smartisan-{stem}.png")),
                );
            }
        }
    }
    theme::set_current(crate::models::ThemeName::default());
}

/// 使用正式标题栏和标签事件入口出图，检查文稿工具按钮与页面标签之间的层次。
#[test]
#[ignore = "出文稿顶部样张，手动跑"]
fn smartisan_document_chrome_samples() {
    let ctx = egui::Context::default();
    theme::set_current(crate::models::ThemeName::Smartisan);
    theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    theme::configure_icons(&ctx);
    theme::configure_style(&ctx);
    let mut app = fixture();
    app.tabs = vec![
        TabRef::Page(NavPage::Manuscript),
        TabRef::Doc(0),
        TabRef::Page(NavPage::Vocabulary),
    ];
    app.active_tab = 1;
    app.tabs.push(TabRef::Page(NavPage::Knowledge));
    app.tabs.push(TabRef::Page(NavPage::Proofread));
    let size = egui::vec2(1280.0, 130.0);
    let mut canvas = crate::ui_snapshot::Canvas::default();
    for scale in [1.0, 1.25, 1.5, 2.0] {
        ctx.set_pixels_per_point(scale);
        for frame in 0..3 {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    egui::Panel::top("chrome_title")
                        .show_separator_line(false)
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| app.window_titlebar(ui));
                    egui::Panel::top("chrome_tabs")
                        .show_separator_line(false)
                        .frame(theme::panel(theme::surface(), 12))
                        .show(ui, |ui| app.top_bar(ui));
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE.fill(theme::surface()))
                        .show(ui, |_| {});
                },
            );
            if frame < 2 {
                canvas.absorb(&output.textures_delta);
            } else {
                canvas.render(
                    &ctx,
                    output,
                    size,
                    theme::canvas(),
                    std::path::Path::new(&format!(
                        "tmp/smartisan-chrome-{}.png",
                        (scale * 100.0) as u32
                    )),
                );
            }
        }
    }
    theme::set_current(crate::models::ThemeName::default());
}
