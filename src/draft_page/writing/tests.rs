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
