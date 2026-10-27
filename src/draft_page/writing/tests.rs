//! 真实 egui 六阶段页面与窄窗回归；不启动模型，不读用户配置库。

use super::*;
use crate::agent::writing::{ChunkProgress, ChunkStatus, MaterialCard, Section};
use crate::draft_page::{DraftSession, ExportLinks};
use sha2::{Digest, Sha256};

fn fixture() -> WritingTask {
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "部署基层减负工作，规范检查考核与报表填报".into();
    task.style_md = "# 写作风格\n先写任务，再写措施；表述明确、克制，不夸大成效。".into();
    task.framework_md = "# 篇章框架\n简述目的，按工作任务分节，结尾明确落实要求。".into();
    let source = SourceSnapshot::new(
        "基层报表梳理清单",
        SourceRole::Content,
        "部分报表存在内容重复、口径相近但分别报送的情况。",
    )
    .unwrap();
    let card = MaterialCard {
        source_id: source.id.clone(),
        quote: source.text.clone(),
        point: "合并同类报表，减少重复填报".into(),
        start: 0,
        end: source.text.len(),
        ..Default::default()
    };
    task.add_source(source.clone());
    task.topic_cards = vec![card.clone()];
    for (id, title, points) in [
        (
            "first",
            "一、精简规范检查考核",
            "统筹同类事项，防止多头重复检查。",
        ),
        (
            "second",
            "二、规范基层报表填报",
            "梳理重复报表，推动数据共享。",
        ),
        ("third", "三、健全长效机制", "明确责任分工与跟踪纠偏机制。"),
    ] {
        let mut card = card.clone();
        card.section_id = Some(id.into());
        task.sections.push(Section {
            id: id.into(),
            title: title.into(),
            points: points.into(),
            target_chars: 500,
            markdown: "各有关单位要统筹安排，推动已有数据共享利用，减少不必要的重复填报。".into(),
            cards: vec![card],
            status: "已完成".into(),
            chunk_bytes: 3200,
            ..Default::default()
        });
    }
    let hash = Sha256::digest(format!("{}:0:{}", source.hash, source.text.len()).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    for section_id in [
        None,
        Some("first".into()),
        Some("second".into()),
        Some("third".into()),
    ] {
        task.chunks.push(ChunkProgress {
            source_id: source.id.clone(),
            chunk_hash: hash.clone(),
            role: SourceRole::Content,
            section_id,
            start: 0,
            end: source.text.len(),
            status: ChunkStatus::Extracted,
            error: None,
            output: String::new(),
            query: String::new(),
        });
    }
    task.outline_confirmed = true;
    task.final_markdown = task
        .sections
        .iter()
        .map(|section| format!("## {}\n\n{}", section.title, section.markdown))
        .collect::<Vec<_>>()
        .join("\n\n");
    task.status = "阶段成果已保存".into();
    task.adopt_legacy_records();
    task
}

fn draw(stage: Stage, width: f32, sample: Option<&std::path::Path>) {
    let ctx = egui::Context::default();
    crate::theme::configure_icons(&ctx);
    crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    crate::theme::configure_style(&ctx);
    let mut config = AppConfig::default();
    let mut doc = DraftSession::with_markdown(7, &config, "# 原有正式正文\n\n由用户维护。".into());
    doc.writing.task = fixture();
    doc.draft.kind = TemplateKind::PlainDocument;
    doc.writing.task.stale_reason = None;
    doc.writing.task.stage = stage;
    doc.writing.open = true;
    let original = doc.generated_markdown.clone();
    let (sender, _receiver) = mpsc::channel();
    let mut status = String::new();
    let mut version_switch = None;
    let mut revert_confirm = None;
    let mut metrics = crate::metrics::Metrics::default();
    let mut actions = Vec::new();
    let mut exports = ExportLinks::default();
    let size = egui::vec2(width, 880.0);
    let mut canvas = crate::ui_snapshot::Canvas::default();
    for index in 0..3 {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut page = DraftPage {
                        doc: &mut doc,
                        config: &mut config,
                        store: None,
                        sender: &sender,
                        status: &mut status,
                        version_switch: &mut version_switch,
                        revert_confirm: &mut revert_confirm,
                        metrics: &mut metrics,
                        actions: &mut actions,
                        export_links: &mut exports,
                    };
                    page.writing_header(ui);
                    page.writing_ui(ui);
                });
            },
        );
        if index == 2
            && let Some(path) = sample
        {
            canvas.render(&ctx, out, size, crate::theme::canvas(), path);
        } else {
            canvas.absorb(&mut out.textures_delta);
        }
    }
    assert_eq!(
        doc.generated_markdown, original,
        "查看写作各阶段不得改写正式正文"
    );
    assert!(!doc.writing.running, "单纯查看阶段不得启动模型");
}

#[test]
fn all_stages_draw_at_desktop_and_narrow_width_without_touching_the_document() {
    for width in [1280.0, 680.0] {
        for stage in Stage::ALL {
            draw(stage, width, None);
        }
    }
}

#[test]
#[ignore = "生成六阶段真实界面样张供目视检查"]
fn staged_writing_ui_samples() {
    std::fs::create_dir_all("tmp/staged-writing").unwrap();
    for stage in Stage::ALL {
        draw(
            stage,
            1280.0,
            Some(std::path::Path::new(&format!(
                "tmp/staged-writing/stage-{}.png",
                stage.index() + 1
            ))),
        );
    }
    draw(
        Stage::Outline,
        680.0,
        Some(std::path::Path::new("tmp/staged-writing/narrow.png")),
    );
}

/// 交互测试用的起草页外壳；`DraftPage` 借用的状态都放在这里。
struct Bench {
    doc: DraftSession,
    config: AppConfig,
    sender: mpsc::Sender<crate::app::WorkerResult>,
    status: String,
    version_switch: Option<crate::app::VersionSwitchPrompt>,
    revert_confirm: Option<(i64, i64)>,
    metrics: crate::metrics::Metrics,
    actions: Vec<crate::app::DraftAction>,
    exports: ExportLinks,
}

fn harness(stage: Stage) -> egui_kittest::Harness<'static, Bench> {
    let config = AppConfig::default();
    let mut doc = DraftSession::with_markdown(7, &config, "# 原有正式正文\n\n由用户维护。".into());
    doc.manuscript_id = None;
    doc.writing.task = fixture();
    doc.draft.kind = TemplateKind::PlainDocument;
    doc.writing.task.stage = stage;
    doc.writing.open = true;
    let (sender, _receiver) = mpsc::channel();
    let bench = Bench {
        doc,
        config,
        sender,
        status: String::new(),
        version_switch: None,
        revert_confirm: None,
        metrics: Default::default(),
        actions: Vec::new(),
        exports: ExportLinks::default(),
    };
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui, bench: &mut Bench| {
            crate::theme::configure_icons(ui.ctx());
            let mut page = DraftPage {
                doc: &mut bench.doc,
                config: &mut bench.config,
                store: None,
                sender: &bench.sender,
                status: &mut bench.status,
                version_switch: &mut bench.version_switch,
                revert_confirm: &mut bench.revert_confirm,
                metrics: &mut bench.metrics,
                actions: &mut bench.actions,
                export_links: &mut bench.exports,
            };
            page.writing_header(ui);
            page.writing_ui(ui);
        },
        bench,
    );
    harness.set_size(egui::vec2(1280.0, 880.0));
    harness.run();
    harness
}

#[test]
fn removing_a_processed_material_asks_first_and_cancel_keeps_it() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Prepare);
    harness.get_by_label("移除").click();
    harness.run();
    assert_eq!(harness.state().doc.writing.task.sources.len(), 1);
    harness.get_by_label("移除材料");
    harness.get_by_label("取消").click();
    harness.run();
    assert!(harness.state().doc.writing.confirm.is_none());
    assert_eq!(harness.state().doc.writing.task.sources.len(), 1);

    harness.get_by_label("移除").click();
    harness.run();
    harness.get_all_by_label("移除").last().unwrap().click();
    harness.run();
    let task = &harness.state().doc.writing.task;
    assert!(task.sources.is_empty());
    assert!(task.chunks.is_empty() && task.topic_cards.is_empty());
}

#[test]
fn a_topic_edit_waits_for_apply_and_keeps_finished_extraction() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Prepare);
    let original = harness.state().doc.writing.task.topic.clone();
    harness.state_mut().doc.writing.topic_draft = Some("另拟主题".into());
    harness.run();
    assert_eq!(harness.state().doc.writing.task.topic, original);
    harness.get_by_label("放弃修改").click();
    harness.run();
    assert_eq!(
        harness.state().doc.writing.topic_draft.as_deref(),
        Some(original.as_str()),
        "放弃后草稿回到原主题"
    );
    assert_eq!(harness.state().doc.writing.task.topic, original);
    assert!(harness.query_by_label("应用新主题").is_none());

    harness.state_mut().doc.writing.topic_draft = Some("另拟主题".into());
    harness.run();
    let chunks = harness.state().doc.writing.task.chunks.len();
    harness.get_by_label("应用新主题").click();
    harness.run();
    let task = &harness.state().doc.writing.task;
    assert_eq!(task.topic, "另拟主题");
    assert!(!task.outline_confirmed);
    assert_eq!(
        task.chunks.len(),
        chunks,
        "已完成的提炼保留，改回原主题可恢复"
    );
}

#[test]
fn repolishing_over_a_hand_edited_final_asks_first() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Review);
    harness.state_mut().doc.writing.task.final_manual = true;
    harness.run();
    harness.get_by_label("重新统稿").click();
    harness.run();
    assert!(matches!(
        harness.state().doc.writing.confirm,
        Some(Confirm::Repolish)
    ));
    assert!(!harness.state().doc.writing.running, "确认前不得启动模型");
    harness.get_by_label("取消").click();
    harness.run();
    assert!(harness.state().doc.writing.confirm.is_none());
    assert!(harness.state().doc.writing.task.final_manual);
}

#[test]
fn a_failed_block_can_be_confirmed_manually_as_no_relevant_content() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Extract);
    {
        let task = &mut harness.state_mut().doc.writing.task;
        let query = task.topic_query();
        let chunk = task
            .chunks
            .iter_mut()
            .find(|chunk| chunk.section_id.is_none() && chunk.query == query)
            .unwrap();
        chunk.status = ChunkStatus::Failed;
        chunk.error = Some("模拟失败：格式不合规".into());
    }
    harness.run();
    harness.get_by_label("人工确认无相关内容").click();
    harness.run();
    let task = &harness.state().doc.writing.task;
    assert_eq!(task.coverage().failed, 0);
    assert_eq!(task.coverage().skipped, 1);
    assert!(
        task.chunks
            .iter()
            .any(|chunk| chunk.status == ChunkStatus::SkippedByUser),
        "人工处置记为独立状态"
    );
}

#[test]
fn an_advisory_suggestion_can_be_adopted_into_a_manual_section() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Review);
    harness.state_mut().doc.writing.task.sections[0].advisory_suggestion =
        Some("建议修改后的正文。".into());
    harness.run();
    harness.get_by_label("采纳到本节").click();
    harness.run();
    let section = &harness.state().doc.writing.task.sections[0];
    assert_eq!(section.markdown, "建议修改后的正文。");
    assert!(section.manually_edited);
}

#[test]
fn changing_the_document_kind_updates_the_task_without_running() {
    let mut harness = harness(Stage::Prepare);
    harness.state_mut().doc.draft.kind = TemplateKind::OfficialLetter;
    harness.run();
    let writing = &harness.state().doc.writing;
    assert_eq!(writing.task.kind, TemplateKind::OfficialLetter);
    assert!(
        writing
            .task
            .stale_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("文种")),
        "切换文种后不点运行也能看到提示"
    );
    assert!(!writing.running, "切换文种不得启动模型");
}
