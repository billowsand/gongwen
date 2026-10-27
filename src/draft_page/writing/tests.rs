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
fn marking_a_card_conflict_updates_the_verification_counts() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Write);
    harness.run();
    assert!(
        harness
            .query_by_label("已核对 0 · 存在冲突 0 · 依据不足 0 · 待核对 1")
            .is_some(),
        "初始统计按当前节素材计算"
    );
    harness.get_by_label("存在冲突").click();
    harness.run();
    assert!(
        harness
            .query_by_label("已核对 0 · 存在冲突 1 · 依据不足 0 · 待核对 0")
            .is_some()
    );
    harness.get_by_label("存在冲突").click();
    harness.run();
    assert!(
        harness
            .query_by_label("已核对 0 · 存在冲突 0 · 依据不足 0 · 待核对 1")
            .is_some(),
        "再点一次撤回为待核对"
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
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(task.section_coverage("s0"));
    }
    let uncached_section = start.elapsed() / rounds;

    let mut panel = WritingPanel::new(TemplateKind::PlainDocument);
    panel.task = task;
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(panel.coverage());
    }
    let cached = start.elapsed() / rounds;
    let start = Instant::now();
    for _ in 0..rounds {
        black_box(panel.section_coverage("s0"));
    }
    let cached_section = start.elapsed() / rounds;
    println!(
        "1 MB 材料 / 8 节：每次覆盖率 未缓存 {uncached:?} → 缓存命中 {cached:?}；单节 未缓存 {uncached_section:?} → 缓存命中 {cached_section:?}"
    );
}

#[test]
fn completed_stages_show_a_check_and_incomplete_stages_warn() {
    use egui_kittest::kittest::Queryable;
    let mut complete = harness(Stage::Extract);
    complete.run();
    assert!(
        complete.query_by_label_contains("2 提炼素材 √").is_some(),
        "完成的阶段带勾"
    );
    let mut pending = harness(Stage::Outline);
    pending.state_mut().doc.writing.task.outline_confirmed = false;
    pending.run();
    assert!(
        pending.query_by_label_contains("3 确认提纲 ⚠").is_some(),
        "待人工确认的阶段带提示"
    );
}

#[test]
fn the_material_list_shows_parse_status_and_extraction_progress() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Prepare);
    harness.run();
    assert!(
        harness
            .query_by_label_contains("已解析 · 提炼 1/1 块")
            .is_some(),
        "每份材料显示解析状态与提炼进度"
    );
}

#[test]
fn the_outline_can_move_sections_down_and_flags_bad_length_split() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Outline);
    harness.run();
    let first = harness.state().doc.writing.task.sections[0].title.clone();
    let second = harness.state().doc.writing.task.sections[1].title.clone();
    harness.get_all_by_label("↓ 下移").next().unwrap().click();
    harness.run();
    let sections = &harness.state().doc.writing.task.sections;
    assert_eq!(sections[0].title, second);
    assert_eq!(sections[1].title, first);
    assert!(
        harness.query_by_label_contains("偏差超过 15%").is_some(),
        "各节字数合计与目标篇幅相差过大时提示"
    );
}

#[test]
fn section_tabs_show_state_marks_and_rewrite_needs_a_request() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Write);
    harness.run();
    assert!(
        harness
            .query_by_label_contains("√ 一、精简规范检查考核")
            .is_some(),
        "完成章节页签带勾"
    );
    assert!(
        harness.query_by_label_contains("当前 ").is_some(),
        "正文下方显示当前字数与目标字数"
    );
    // 修改要求为空时重写按钮禁用：点击不产生任何动作。
    harness.get_by_label("按要求重写本节").click();
    harness.run();
    assert!(harness.state().actions.is_empty());
    // 给本节单独写要求后可以重写；没有稿件身份时先把动作转成保存。
    harness
        .state_mut()
        .doc
        .writing
        .rewrite_drafts
        .insert("first".into(), "压缩到两句话".into());
    harness.run();
    harness.get_by_label("按要求重写本节").click();
    harness.run();
    assert!(
        harness
            .state()
            .actions
            .iter()
            .any(|action| matches!(action, crate::app::DraftAction::SaveToLibrary))
    );
}

#[test]
fn review_uses_per_section_comparison_and_can_keep_the_draft() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Review);
    harness.state_mut().doc.writing.task.sections[0].review_suggestion =
        Some("统稿建议正文。".into());
    harness.run();
    assert!(
        harness.query_by_label("原稿").is_some(),
        "默认视图是逐节原稿 / 统稿建议对照"
    );
    assert!(harness.query_by_label("全文用：统稿建议").is_some());
    harness.get_all_by_label("保留原稿").next().unwrap().click();
    harness.run();
    let task = &harness.state().doc.writing.task;
    assert!(
        task.sections[0].review_suggestion.is_some(),
        "保留原稿不丢建议，可以改回"
    );
    assert!(!task.final_markdown.contains("统稿建议正文。"));
    assert!(harness.query_by_label("全文用：原稿").is_some());

    harness.get_all_by_label("采纳建议").next().unwrap().click();
    harness.run();
    assert!(
        harness
            .state()
            .doc
            .writing
            .task
            .final_markdown
            .contains("统稿建议正文。")
    );

    // 统稿人工改过：逐节选择不动它，提示并给出重拼入口（人工稿先进统稿历史）。
    harness.state_mut().doc.writing.task.final_markdown = "人工统稿。".into();
    harness.state_mut().doc.writing.task.final_manual = true;
    harness.run();
    harness.get_by_label("按逐节选择重新拼全文").click();
    harness.run();
    let task = &harness.state().doc.writing.task;
    assert!(!task.final_manual);
    assert!(task.final_markdown.contains("统稿建议正文。"));
    assert_eq!(
        task.final_history.last().map(String::as_str),
        Some("人工统稿。")
    );
}

#[test]
fn the_evidence_column_shows_stage_specific_status() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Outline);
    harness.run();
    assert!(
        harness.query_by_label_contains("各节备料状态").is_some(),
        "提纲页右栏显示每节备料状态"
    );
    assert!(
        harness
            .query_all_by_label_contains("素材 1 条 · 已备料")
            .count()
            >= 1
    );
}

#[test]
fn the_footer_shows_call_estimate_and_long_task_progress() {
    use egui_kittest::kittest::Queryable;
    let mut harness = harness(Stage::Prepare);
    harness.run();
    assert!(
        harness
            .query_by_label_contains("开始提炼前：预计约")
            .is_some(),
        "开始提炼前显示预计模型调用次数"
    );
    harness.state_mut().doc.writing.running = true;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    harness.state_mut().doc.writing.task.started_at_unix = Some(now - 65);
    harness.run_steps(2);
    assert!(
        harness.query_by_label_contains("总进度").is_some(),
        "长任务显示总进度条"
    );
    assert!(harness.query_by_label_contains("已用 1:").is_some());
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
