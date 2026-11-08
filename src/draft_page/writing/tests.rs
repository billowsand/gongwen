//! 真实 egui 六阶段页面与窄窗回归；不启动模型，不读用户配置库。

use super::*;
use crate::agent::writing::{ChunkProgress, ChunkStatus, MaterialCard, Section};
use crate::draft_page::{DraftSession, ExportLinks};

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
            status: crate::agent::writing::SectionStatus::Done,
            chunk_bytes: 3200,
            ..Default::default()
        });
    }
    let hash = crate::agent::writing::chunk_hash(&source, 0, source.text.len());
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
    draw_with(stage, width, sample, |_| {});
}

fn draw_with(
    stage: Stage,
    width: f32,
    sample: Option<&std::path::Path>,
    prepare: impl FnOnce(&mut DraftSession),
) {
    let ctx = egui::Context::default();
    crate::theme::configure_icons(&ctx);
    crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    crate::theme::configure_style(&ctx);
    let mut config = AppConfig::default();
    let mut doc = DraftSession::with_markdown(7, &config, "# 原有正式正文\n\n由用户维护。".into());
    doc.writing.task = fixture();
    prepare(&mut doc);
    doc.draft.kind = TemplateKind::PlainDocument;
    doc.writing.task.stale_reason = None;
    doc.writing.task.stage = stage;
    doc.writing.view_stage = stage;
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
                    page.writing_workspace(ui);
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
    for width in [1280.0, 680.0] {
        draw_with(
            Stage::Extract,
            width,
            Some(std::path::Path::new(&format!(
                "tmp/staged-writing/failed-blocks-{width}.png"
            ))),
            |doc| {
                let task = &mut doc.writing.task;
                let query = task.topic_query();
                if let Some(chunk) = task
                    .chunks
                    .iter_mut()
                    .find(|chunk| chunk.section_id.is_none() && chunk.query == query)
                {
                    chunk.status = ChunkStatus::Failed;
                    chunk.error =
                        Some("模型输出的原句未能在原文块中逐字匹配，该块未标记成功".into());
                }
            },
        );
        draw_with(
            Stage::Review,
            width,
            Some(std::path::Path::new(&format!(
                "tmp/staged-writing/review-compare-{width}.png"
            ))),
            |doc| {
                let task = &mut doc.writing.task;
                task.sections[0].review_suggestion = Some(
                    "各有关单位要统筹安排检查事项，推动已有数据共享利用，减少不必要的重复填报。"
                        .into(),
                );
                task.sections[1].advisory_suggestion =
                    Some("建议压缩为：规范报表填报，推动数据共享。".into());
                task.final_markdown = task
                    .sections
                    .iter()
                    .map(|section| format!("## {}\n\n{}", section.title, section.markdown))
                    .collect::<Vec<_>>()
                    .join("\n\n");
            },
        );
    }
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
    doc.writing.view_stage = stage;
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
    let mut harness = crate::test_support::harness_ui_state(
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
fn history_materials_cannot_be_removed_from_the_read_only_view() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Prepare);
    let before = serde_json::to_value(&h.state().doc.writing.task).unwrap();
    h.run();
    assert!(h.query_by_label_contains("移除").is_none());
    assert_eq!(
        serde_json::to_value(&h.state().doc.writing.task).unwrap(),
        before
    );
}

#[test]
fn history_does_not_apply_pending_topic_edits() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Prepare);
    let topic = h.state().doc.writing.task.topic.clone();
    let chunks = h.state().doc.writing.task.chunks.len();
    h.state_mut().doc.writing.topic_draft = Some("不应应用的新主题".into());
    h.run();
    assert!(h.query_by_label("应用新主题").is_none());
    assert_eq!(h.state().doc.writing.task.topic, topic);
    assert_eq!(h.state().doc.writing.task.chunks.len(), chunks);
}

#[test]
fn history_does_not_offer_repolishing_an_old_manual_final() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Review);
    h.state_mut().doc.writing.task.final_manual = true;
    h.run();
    assert!(h.query_by_label("重新统稿").is_none());
    assert!(h.state().doc.writing.task.final_manual);
    assert!(!h.state().doc.writing.running);
}

#[test]
fn history_does_not_reclassify_a_failed_block_as_completed() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Extract);
    h.state_mut().doc.writing.task.chunks[0].status = ChunkStatus::Failed;
    h.run();
    assert!(h.query_by_label("人工确认无相关内容").is_none());
    assert_eq!(
        h.state().doc.writing.task.chunks[0].status,
        ChunkStatus::Failed
    );
    assert!(!h.state().doc.writing.running);
}

#[test]
fn history_does_not_apply_old_advisory_suggestions() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Review);
    h.state_mut().doc.writing.task.sections[0].advisory_suggestion = Some("仅历史建议".into());
    let original = h.state().doc.writing.task.sections[0].markdown.clone();
    h.run();
    assert!(h.query_by_label("采纳到本节").is_none());
    assert_eq!(h.state().doc.writing.task.sections[0].markdown, original);
}

#[test]
fn changing_current_document_kind_does_not_rewrite_the_historical_task() {
    let mut h = harness(Stage::Prepare);
    let original = h.state().doc.writing.task.kind;
    h.state_mut().doc.draft.kind = TemplateKind::OfficialLetter;
    h.run();
    assert_eq!(h.state().doc.writing.task.kind, original);
    assert!(!h.state().doc.writing.running);
}

#[test]
fn coverage_is_cached_until_the_task_changes() {
    let mut harness = harness(Stage::Extract);
    let first = harness.state_mut().doc.writing.coverage();
    let computations = harness.state().doc.writing.coverage_computations;
    let again = harness.state_mut().doc.writing.coverage();
    assert_eq!(first, again);
    assert_eq!(
        harness.state().doc.writing.coverage_computations,
        computations,
        "同一修订号直接复用缓存"
    );

    harness
        .state_mut()
        .doc
        .writing
        .task
        .add_source(SourceSnapshot::new("补充", SourceRole::Content, "补充材料。").unwrap());
    harness.state_mut().doc.writing.changed();
    let after = harness.state_mut().doc.writing.coverage();
    assert_eq!(
        harness.state().doc.writing.coverage_computations,
        computations + 1,
        "任务变化后缓存失效并重算"
    );
    assert_eq!(after.total, first.total + 1);
}

#[test]
fn progress_updates_follow_the_task_unless_the_user_switched_the_stage() {
    let mut harness = harness(Stage::Write);
    let (sender, receiver) = mpsc::channel();
    harness.state_mut().doc.writing.receiver = Some(receiver);
    // 用户点去别的阶段查看，任务自己前进时不把人拉回去。
    harness.state_mut().doc.writing.view_stage = Stage::Collect;
    harness.state_mut().doc.writing.view_manual = true;
    let mut task = harness.state().doc.writing.task.clone();
    task.stage = Stage::Review;
    sender.send(Update::Progress(Box::new(task))).unwrap();
    harness.state_mut().doc.writing.sync(None, false);
    harness.run();
    assert_eq!(
        harness.state().doc.writing.view_stage,
        Stage::Collect,
        "运行中看别的阶段不被拉回"
    );

    // 用户没手动切过（或切回任务当前阶段）时跟随任务阶段。
    harness.state_mut().doc.writing.view_stage = Stage::Review;
    harness.state_mut().doc.writing.view_manual = false;
    let mut task = harness.state().doc.writing.task.clone();
    task.stage = Stage::Write;
    sender.send(Update::Progress(Box::new(task))).unwrap();
    harness.state_mut().doc.writing.sync(None, false);
    harness.run();
    assert_eq!(
        harness.state().doc.writing.view_stage,
        Stage::Write,
        "没有手动切换时跟随任务阶段"
    );
}

#[test]
fn history_does_not_change_the_verification_of_saved_cards() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Write);
    let before = serde_json::to_value(&h.state().doc.writing.task.topic_cards).unwrap();
    h.run();
    assert!(h.query_by_label("存在冲突").is_none());
    assert_eq!(
        serde_json::to_value(&h.state().doc.writing.task.topic_cards).unwrap(),
        before
    );
}

#[test]
#[ignore = "测量 1 MB 材料、8 节时覆盖率缓存前后的单帧开销"]
fn coverage_benchmark_one_megabyte_and_eight_sections() {
    use std::hint::black_box;
    use std::time::Instant;
    let mut task = WritingTask::new(TemplateKind::PlainDocument);
    task.topic = "基层减负".into();
    task.add_source(
        SourceSnapshot::new(
            "大材料",
            SourceRole::Content,
            "材料段落内容。".repeat(50_000),
        )
        .unwrap(),
    );
    task.sections = (0..8)
        .map(|index| Section {
            id: format!("s{index}"),
            title: format!("第{}节", index + 1),
            points: "要点".into(),
            target_chars: 300,
            chunk_bytes: 3200,
            ..Default::default()
        })
        .collect();
    let rounds = 20;
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(task.coverage());
    }
    let uncached = start.elapsed() / rounds;

    let mut panel = WritingPanel::new(TemplateKind::PlainDocument);
    panel.task = task;
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(panel.coverage());
    }
    let cached = start.elapsed() / rounds;
    println!("1 MB 材料 / 8 节：每次覆盖率 未缓存 {uncached:?} → 缓存命中 {cached:?}");
}

#[test]
fn history_is_read_only_at_every_old_stage() {
    use egui_kittest::kittest::Queryable;
    for stage in Stage::ALL {
        let mut h = harness(stage);
        h.run();
        assert!(h.query_by_label("旧分阶段写作历史").is_some());
        assert!(h.query_by_label("复制为新的 Pi 任务").is_some());
        assert!(h.query_by_label("接着跑").is_none());
        assert!(h.state().actions.is_empty());
    }
}

#[test]
fn history_original_materials_remain_visible_and_exportable() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Prepare);
    let source = h.state().doc.writing.task.sources[0].text.clone();
    h.run();
    h.get_by_label("材料 1 · 基层报表梳理清单").click();
    h.run();
    assert!(h.query_by_label_contains(&source).is_some());
    h.get_by_label("导出 Markdown…").click();
    h.run();
    assert!(h.state().actions.iter().any(|action| matches!(action, crate::app::DraftAction::ExportWritingArtifact {text,..} if *text == source)));
}

#[test]
fn history_does_not_reorder_the_old_outline() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Outline);
    let before = serde_json::to_value(&h.state().doc.writing.task.sections).unwrap();
    h.run();
    assert!(h.query_by_label("↓ 下移").is_none());
    assert_eq!(
        serde_json::to_value(&h.state().doc.writing.task.sections).unwrap(),
        before
    );
}

#[test]
fn history_cannot_start_an_old_rewrite_even_with_a_saved_request() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Write);
    h.state_mut()
        .doc
        .writing
        .rewrite_drafts
        .insert("first".into(), "重写要求".into());
    h.run();
    assert!(h.query_by_label("按要求重写本节").is_none());
    assert!(h.state().actions.is_empty());
    assert!(!h.state().doc.writing.running);
}

#[test]
fn history_final_can_be_exported_without_applying_it_to_the_document() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Review);
    h.state_mut().doc.writing.task.final_markdown = "旧统稿全文。".into();
    let current = h.state().doc.generated_markdown.clone();
    h.run();
    h.get_by_label("统稿成果").click();
    h.run();
    h.get_by_label("导出 Markdown…").click();
    h.run();
    assert!(h.query_by_label("采纳建议").is_none());
    assert!(h.state().actions.iter().any(|action| matches!(action, crate::app::DraftAction::ExportWritingArtifact {text,..} if text == "旧统稿全文。")));
    assert_eq!(h.state().doc.generated_markdown, current);
}

#[test]
fn history_keeps_the_original_fact_excerpts_available() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(Stage::Extract);
    h.run();
    assert!(h.query_by_label("提炼素材").is_some());
    let before = serde_json::to_value(&h.state().doc.writing.task.topic_cards).unwrap();
    h.get_by_label("提炼素材").click();
    h.run();
    assert!(h.query_by_label("全部摘记").is_some());
    assert_eq!(
        serde_json::to_value(&h.state().doc.writing.task.topic_cards).unwrap(),
        before
    );
}

#[test]
fn a_new_task_does_not_offer_an_empty_legacy_history_entry() {
    let mut panel = WritingPanel::new(TemplateKind::PlainDocument);
    assert!(!panel.has_history());
    panel.task = fixture();
    assert!(panel.has_history());
}

#[test]
#[ignore = "生成便签主题六阶段与窄窗样张供目视检查"]
fn smartisan_staged_writing_ui_samples() {
    crate::theme::set_current(crate::models::ThemeName::Smartisan);
    std::fs::create_dir_all("tmp/staged-writing").unwrap();
    for width in [1280.0, 680.0] {
        for stage in Stage::ALL {
            draw(
                stage,
                width,
                Some(std::path::Path::new(&format!(
                    "tmp/staged-writing/smartisan-{}-{width}.png",
                    stage.index() + 1
                ))),
            );
        }
    }
    crate::theme::set_current(crate::models::ThemeName::default());
}
