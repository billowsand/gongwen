//! 用独立临时库走六个工作区的真实界面路径。可选输出样张供目视检查。

use super::*;
use crate::app::{NavPage, ui_fixture};
use crate::knowledge::{KnowledgeImportItem, KnowledgeSource, KnowledgeStore};
use crate::pi_agent::PiActivity;
use crate::pi_agent::PiEvent;
use crate::pi_agent::wiki_jobs::{FrameProposal, PageProposal, PageProposalSet};
use crate::wiki::{FrameCategory, FrameTheme, PageInput, WikiStore};

const PAGE: &str = "重点隐患闭环整改";

#[test]
fn wiki_workspaces_render() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("wiki.db");
    let mut sources = KnowledgeStore::open(&path).unwrap();
    let source = sources
        .import_document(&KnowledgeImportItem {
            source: KnowledgeSource::Markdown,
            source_manuscript_id: None,
            source_path: "检查计划.md".into(),
            kind: crate::models::TemplateKind::default(),
            title: "2026 年度安全生产检查计划".into(),
            content_markdown: "# 检查计划\n\n对重点隐患建立整改台账，明确责任分工和复查程序。"
                .into(),
        })
        .unwrap();
    let wiki = WikiStore::open(&path).unwrap();
    let themes = vec![FrameTheme {
        name: "安全生产".into(),
        categories: vec![FrameCategory {
            name: "监督检查".into(),
            doc_ids: vec![source],
        }],
    }];
    wiki.apply_frame(&themes).unwrap();
    let node = wiki.list_tree().unwrap()[0].categories[0].id;
    let refs = vec![(source, "支持整改安排和复查程序。".to_owned())];
    let body = format!(
        "## 整改流程\n\n详见 [[监督检查导览]]。落实整改要求〔E{source}〕。\n\n## 复查销号\n\n复查合格后登记销号。\n\n> 对重点隐患建立整改台账。\n> ——检查计划\n\n| 环节 | 依据 |\n| --- | :---: |\n| [[监督检查导览|检查安排]] | 〔E{source}〕 |"
    );
    let input = PageInput {
        title: PAGE.into(),
        node_id: node,
        kind: "normal",
        body: body.clone(),
        source_run_id: "manual".into(),
        review_edited: false,
        doc_refs: refs.clone(),
        base_version: None,
    };
    let page_id = wiki.create_page(&input).unwrap();
    wiki.create_page(&PageInput {
        title: "监督检查导览".into(),
        kind: "guide",
        body: format!("检查要求见 [[{PAGE}]]。"),
        ..input.clone()
    })
    .unwrap();
    wiki.update_page(
        &page_id,
        &PageInput {
            body: body.replace("落实整改要求", "逐项登记整改要求"),
            ..input
        },
    )
    .unwrap();

    let mut app = ui_fixture();
    app.wiki_store = Some(wiki);
    app.knowledge_store = Some(sources);
    app.knowledge_local_backfill_started = true;
    app.refresh_knowledge();
    open_wiki_page(&mut app, &page_id);

    let ctx = egui::Context::default();
    theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    theme::configure_icons(&ctx);
    theme::configure_style(&ctx);
    let size = egui::vec2(1360.0, 850.0);
    let mut canvas = crate::ui_snapshot::Canvas::default();
    let output_dir = std::env::var_os("GONGWEN_WIKI_SAMPLES").map(std::path::PathBuf::from);
    if let Some(dir) = &output_dir {
        std::fs::create_dir_all(dir).unwrap();
    }
    for (index, (name, label)) in [
        ("01-browse", PAGE),
        ("02-framework", "归纳依据"),
        ("03-sources", "AI 工作状态"),
        ("04-inbox", "打开审核台"),
        ("05-review", "采纳并入库"),
        ("06-history", "版本详情"),
        ("07-split", "双栏对照"),
    ]
    .into_iter()
    .enumerate()
    {
        match index {
            1 => {
                app.wiki.open_frame_proposal();
                app.wiki.frame_proposal = Some(FrameProposal {
                    themes: themes.clone(),
                });
                app.wiki.frame_node = Some((0, Some(0)));
                let run = WikiRun::new(WikiRequest::Frame, "安全生产".into());
                run.sink()(PiEvent::Progress("正在归纳主题与分类".into()));
                app.wiki.frame_run = Some(run);
                app.wiki.show_job();
            }
            2 => {
                app.wiki
                    .frame_run
                    .as_mut()
                    .unwrap()
                    .finish(Outcome::Delivered);
                let run = WikiRun::new(
                    WikiRequest::Write {
                        node,
                        docs: vec![source],
                    },
                    "安全生产 / 监督检查 · 已选 1 篇".into(),
                );
                run.sink()(PiEvent::Activity(PiActivity {
                    id: "read".into(),
                    action: "读取原文".into(),
                    outcome: Some("已读取 1 篇原文".into()),
                }));
                run.sink()(PiEvent::Progress("撰写条目".into()));
                app.wiki.page_run = Some(run);
            }
            3 => {
                app.wiki
                    .page_run
                    .as_mut()
                    .unwrap()
                    .finish(Outcome::Delivered);
                app.wiki.review = Some(ReviewState::new(PageProposalSet {
                    run_id: "sample".into(),
                    category: "监督检查".into(),
                    node_id: node,
                    pages: vec![PageProposal {
                        uuid: Some(page_id.clone()),
                        base_version: Some(2),
                        title: PAGE.into(),
                        kind: "normal".into(),
                        body: body.replace("登记销号", "登记并复核销号"),
                        change_note: "补充复查销号程序。".into(),
                        doc_refs: refs.clone(),
                    }],
                }));
                app.wiki.open_inbox();
            }
            4 => {
                app.wiki.route = Route::Review;
                app.wiki.job_tab = false;
                workspaces::read_evidence(&mut app, source);
            }
            5 => app.wiki.route = Route::History,
            6 => {
                open_wiki_page(&mut app, &page_id);
                app.wiki.reader_mode = ReaderMode::Split;
            }
            _ => {}
        }
        app.open_page(if index == 2 {
            NavPage::Knowledge
        } else {
            NavPage::Wiki
        });
        for frame in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        if index == 2 {
                            crate::knowledge_ui::knowledge_ui(&mut app, ui);
                        } else {
                            wiki_ui(&mut app, ui);
                        }
                    });
                },
            );
            if frame == 2 {
                assert!(
                    output
                        .shapes
                        .iter()
                        .any(|shape| contains_text(&shape.shape, label)),
                    "{name} 缺少界面文本 {label}"
                );
                if matches!(index, 0 | 4 | 6) {
                    let preview = output
                        .shapes
                        .iter()
                        .find(|shape| {
                            contains_text(&shape.shape, "整改流程")
                                && !contains_text(&shape.shape, "## 整改流程")
                        })
                        .expect("正文应在当前视口内排版");
                    assert!(
                        preview.clip_rect.max.x <= size.x - 145.0,
                        "{name} 正文越过右栏"
                    );
                }
                if index == 0 {
                    for text in ["对重点隐患建立整改台账。", "检查安排"] {
                        assert!(
                            output
                                .shapes
                                .iter()
                                .any(|shape| contains_text(&shape.shape, text))
                        );
                    }
                }
                if index == 6 {
                    assert!(
                        output
                            .shapes
                            .iter()
                            .any(|shape| contains_text(&shape.shape, "## 整改流程"))
                    );
                }
                if let Some(dir) = &output_dir {
                    canvas.absorb(&mut output.textures_delta);
                    output.textures_delta.clear();
                    canvas.render(
                        &ctx,
                        output,
                        size,
                        theme::canvas(),
                        &dir.join(format!("{name}.png")),
                    );
                    continue;
                }
            } else if output_dir.is_some() {
                canvas.absorb(&mut output.textures_delta);
            }
            output.textures_delta.clear();
        }
    }
}

fn contains_text(shape: &egui::Shape, needle: &str) -> bool {
    match shape {
        egui::Shape::Text(text) => text.galley.text().contains(needle),
        egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, needle)),
        _ => false,
    }
}

#[test]
fn wiki_inline_hit_uses_the_rendered_text_range() {
    let ctx = egui::Context::default();
    theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    let body = "先查检查计划。复查时另查[[检查计划]]。";
    let links = HashMap::from([(
        "检查计划".to_owned(),
        LinkTarget::Page {
            uuid: "plan".into(),
            title: "检查计划".into(),
        },
    )]);
    let mut clicked = None;
    let mut draw = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 620.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    clicked = show_page_body(ui, body, &links, &[]);
                });
            },
        );
        output.textures_delta.clear();
        output
    };
    draw(Vec::new());
    let first = draw(Vec::new());
    let pos = first
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().contains("复查时另查") => {
                let content = text.galley.text();
                let start = content[..content.rfind("检查计划").unwrap()]
                    .chars()
                    .count();
                let mut offset = 0;
                for row in &text.galley.rows {
                    if let Some(glyph) = start.checked_sub(offset).and_then(|i| row.glyphs.get(i)) {
                        return Some(
                            text.pos + row.pos.to_vec2() + glyph.logical_rect().center().to_vec2(),
                        );
                    }
                    offset += row.glyphs.len() + usize::from(row.ends_with_newline);
                }
                None
            }
            _ => None,
        })
        .expect("应找到正文中第二个检查计划");
    draw(vec![egui::Event::PointerMoved(pos)]);
    for pressed in [true, false] {
        draw(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    assert_eq!(clicked, Some(BodyTarget::Page("plan".into())));
}

#[test]
fn wiki_long_body_scrolls_to_the_last_heading() {
    let ctx = egui::Context::default();
    theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
    let body = format!(
        "{}\n\n## 文末核验",
        "### 执行要求\n\n请查阅本节工作安排。\n\n".repeat(42)
    );
    let point = egui::pos2(250.0, 150.0);
    let draw = |frame, events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(480.0, 320.0),
                )),
                time: Some(f64::from(frame) / 60.0),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    body::scroll_pane(ui, "long_page", |ui| {
                        show_page_body(ui, &body, &HashMap::new(), &[]);
                    });
                });
            },
        );
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "文末核验"))
    };
    assert!(!draw(0, vec![egui::Event::PointerMoved(point)]));
    let mut reached_end = false;
    for frame in 1..=40 {
        reached_end |= draw(
            frame,
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -260.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
                source: egui::MouseWheelSource::Wheel,
            }],
        );
    }
    assert!(reached_end, "长页面滚到底应能看到最后一节");
}
