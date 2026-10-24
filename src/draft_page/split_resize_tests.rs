//! 起草页分栏与版面缩放的回归测试（拖拽、缩略图、纸面行号、冻结缩放）。
//!
//! 由 `src/draft_page.rs` 拆分而来，作为 `draft_page` 的子模块编译（仅测试构建）。

use super::*;
use std::sync::mpsc::Receiver;

/// 拖动的起始窗格宽度，预热也停在这个宽度上。
const SETTLE_WIDTH: f32 = 700.0;

/// 本帧是否整份重传了字体图集（含初建、长满重建与扩容）。
///
/// 只认 epaint 的字体纹理——`TextureId::Managed(0)`，见 `TextureId::default()`。
/// 图片、图标等别的纹理整张上传是后台解码完成的时刻决定的，与拖动无关，
/// 不能混进来。
fn font_atlas_reuploaded(out: &egui::FullOutput) -> bool {
    out.textures_delta.set.iter().any(|(id, deltas)| {
        *id == egui::TextureId::default() && deltas.iter().any(|delta| delta.pos.is_none())
    })
}

/// 把 `markdown_render` 放进一个可控的 egui 上下文里跑若干帧，
/// 返回逐帧耗时、字体图集整体重建次数，以及每帧实际排版所用的缩放。
struct Harness {
    ctx: egui::Context,
    doc: DraftSession,
    config: AppConfig,
    sender: Sender<WorkerResult>,
    status: String,
    version_switch: Option<VersionSwitchPrompt>,
    revert_confirm: Option<(i64, i64)>,
    actions: Vec<DraftAction>,
    export_links: ExportLinks,
    metrics: crate::metrics::Metrics,
    /// 已经画过多少帧。egui 的指针历史不容许时间倒流，所以帧时间必须跨
    /// `preview_frames` 的多次调用单调递增，不能每次都从 0 重新计。
    clock: usize,
    _keep: Receiver<WorkerResult>,
}

/// 使用真实起草页检查直角功能区、表单与正文窗格，样张只写入 tmp。
#[test]
#[ignore = "出直角界面样张，手动跑"]
fn mdex_rectangular_workspace_samples() {
    mdex_workspace_samples(PreviewMode::Source, "square");
}

/// 使用同一渲染入口检查对照区的单分隔线、浮动滚动条与纸张投影。
#[test]
#[ignore = "出对照界面样张，手动跑"]
fn mdex_comparison_workspace_samples() {
    mdex_workspace_samples(PreviewMode::Split, "comparison");
}

fn mdex_workspace_samples(mode: PreviewMode, stem: &str) {
    workspace_theme_samples(
        mode,
        &format!("mdex-{stem}"),
        &[
            (crate::models::ThemeName::MdexDark, "dark"),
            (crate::models::ThemeName::Mdex, "light"),
        ],
    );
}

/// 从真实编辑器出图，检查稿纸横线与中文换行、左右纸面的材质层次。
#[test]
#[ignore = "出锤子稿纸真实界面样张，手动跑"]
fn smartisan_workspace_samples() {
    for (mode, stem) in [
        (PreviewMode::Split, "smartisan-comparison"),
        (PreviewMode::Source, "smartisan-source"),
        (PreviewMode::Source, "smartisan-source-outline"),
        (PreviewMode::Source, "smartisan-source-empty"),
        (PreviewMode::Source, "smartisan-source-scrolled"),
    ] {
        workspace_theme_samples(
            mode,
            stem,
            &[(crate::models::ThemeName::Smartisan, "light")],
        );
    }
}

/// 从真实预览页出图，检查便签纸面的双线页框、四角收口与投影。
#[test]
#[ignore = "出锤子便签纸面样张，手动跑"]
fn smartisan_preview_sample() {
    workspace_theme_samples(
        PreviewMode::Rendered,
        "smartisan-preview",
        &[(crate::models::ThemeName::Smartisan, "light")],
    );
}

/// 全主题使用同一套目录布局，检查深浅配色与无标题时的窄栏。
#[test]
#[ignore = "出全主题目录样张，手动跑"]
fn source_outline_theme_samples() {
    for (index, name) in crate::models::ThemeName::ALL.into_iter().enumerate() {
        for state in ["outline", "empty"] {
            workspace_theme_samples(
                PreviewMode::Source,
                &format!("source-themes-{state}-{index}"),
                &[(name, "sample")],
            );
        }
    }
}

fn workspace_theme_samples(
    mode: PreviewMode,
    stem: &str,
    names: &[(crate::models::ThemeName, &str)],
) {
    let size = egui::vec2(1280.0, 820.0);
    for &(name, suffix) in names {
        let path = format!("tmp/{stem}-{suffix}.png");
        let mut harness = Harness::new();
        harness.ctx.set_pixels_per_point(1.0);
        harness.doc.generated_markdown =
            "# 关于加强教育教学工作的函\n\n请各地结合实际，认真组织落实。".into();
        harness.doc.preview_mode = mode;
        harness.doc.form_collapsed = mode == PreviewMode::Split;
        harness.config.ribbon_tab = crate::models::RibbonTab::Home;
        harness.config.ribbon_collapsed = mode == PreviewMode::Split;
        harness.config.show_source_outline = false;
        harness.config.show_source_minimap = false;
        harness.config.show_editor_line_numbers = false;
        if name == crate::models::ThemeName::Smartisan {
            harness.doc.generated_markdown = "# 关于开展年度工作总结的通知\n\n各部门：\n\n为全面总结年度工作，梳理重点任务落实情况，现将有关事项通知如下。\n\n## 一、总体要求\n\n请各部门认真总结工作成效，客观分析存在的问题，提出下一年度工作思路。\n\n## 二、报送安排\n\n请于规定时间前将总结材料报送办公室。\n\n**联系人：张某，电话：12345678。**".into();
            harness.doc.draft.kind = crate::models::TemplateKind::PlainDocument;
            harness.doc.form_collapsed = false;
            harness.config.ribbon_collapsed = false;
            harness.config.show_editor_line_numbers = true;
            harness.config.editor_font_size = 16.0;
            harness.config.ribbon_tab = crate::models::RibbonTab::Output;
            if mode == PreviewMode::Rendered {
                harness.doc.form_collapsed = true;
                harness.config.ribbon_collapsed = true;
                harness.doc.preview_zoom = Some(0.65);
            }
            if stem.contains("outline") || stem.contains("empty") || stem.contains("scrolled") {
                harness.config.show_source_outline = true;
                harness.config.show_source_minimap = true;
                harness.doc.form_collapsed = true;
                harness.doc.generated_markdown = "# 研究报告\n\n## 前言\n\n### 测试约定\n\n本文用于检查目录、稿纸和缩略图在同一本稿本中的层次。\n\n## 研究设计\n\n### 样本与指标\n\n请结合实际，认真组织落实各项研究任务。\n\n## 表格组合\n\n## 图表与公式\n\n## 参考文献\n".into();
                if stem.contains("empty") {
                    harness.doc.generated_markdown.clear();
                }
                if stem.contains("scrolled") {
                    harness.doc.generated_markdown.push_str(
                        &"\n正文材料用于检验长稿滚动、行号和缩略图的映射。\n".repeat(100),
                    );
                    harness.doc.source_minimap.requested_offset = Some(600.0);
                }
            }
        }
        if stem.starts_with("source-themes-") {
            harness.config.show_source_outline = true;
            harness.config.show_source_minimap = false;
            harness.doc.form_collapsed = true;
            harness.doc.generated_markdown = if stem.contains("empty") {
                String::new()
            } else {
                "# 年度工作总结\n\n## 总体要求\n\n请结合实际，认真组织落实各项重点任务。\n\n### 重点任务\n\n## 工作安排\n\n### 时间与责任\n\n## 报送要求\n".into()
            };
        }
        theme::set_current(name);
        theme::set_current_paper(crate::models::PaperMode::Follow);
        theme::configure_style(&harness.ctx);
        let mut canvas = crate::ui_snapshot::Canvas::default();
        for frame in 0..3 {
            let mut output = harness.ctx.clone().run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ui| {
                    if name == crate::models::ThemeName::Smartisan {
                        smartisan_sample_header(ui);
                    }
                    let mut page = DraftPage {
                        doc: &mut harness.doc,
                        config: &mut harness.config,
                        store: None,
                        sender: &harness.sender,
                        status: &mut harness.status,
                        version_switch: &mut harness.version_switch,
                        revert_confirm: &mut harness.revert_confirm,
                        actions: &mut harness.actions,
                        export_links: &mut harness.export_links,
                        metrics: &mut harness.metrics,
                    };
                    page.create_ui(ui);
                },
            );
            if frame < 2 {
                canvas.absorb(&mut output.textures_delta);
                continue;
            }
            canvas.render(
                &harness.ctx,
                output,
                size,
                theme::canvas(),
                std::path::Path::new(&path),
            );
        }
    }
    theme::set_current(crate::models::ThemeName::default());
    theme::set_current_paper(crate::models::PaperMode::default());
}

/// 顶栏样张复用正式的底座与标签绘制器，下方功能区仍由真实起草页渲染。
fn smartisan_sample_header(ui: &mut egui::Ui) {
    egui::Panel::top("sample_titlebar")
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::hover());
            theme::smartisan::chrome(ui.painter(), rect);
            ui.painter().text(
                rect.left_center() + egui::vec2(14.0, 0.0),
                egui::Align2::LEFT_CENTER,
                "公文助手",
                egui::FontId::proportional(14.0),
                theme::smartisan::chrome_ink(),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "年度工作总结通知",
                egui::FontId::proportional(14.0),
                theme::smartisan::chrome_ink(),
            );
        });
    egui::Panel::top("sample_tabs")
        .show_separator_line(false)
        .frame(theme::panel(theme::surface(), 12))
        .show(ui, |ui| {
            let background = ui.painter().add(egui::Shape::Noop);
            ui.horizontal(|ui| {
                let _ = ui.add(theme::icon_text_button(theme::Icon::Menu, "菜单"));
                ui.add_space(10.0);
                for (title, width, selected) in [
                    ("稿件管理", 126.0, false),
                    ("年度工作总结通知", 218.0, true),
                ] {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(width, TOOLBAR_CONTROL_HEIGHT),
                        egui::Sense::click(),
                    );
                    theme::smartisan::document_tab(ui.painter(), rect, selected, false, false);
                    ui.painter().text(
                        rect.left_center() + egui::vec2(12.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        title,
                        egui::FontId::proportional(14.0),
                        theme::text(),
                    );
                    ui.painter().text(
                        rect.right_center() - egui::vec2(12.0, 0.0),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        egui::FontId::proportional(14.0),
                        theme::text_muted(),
                    );
                }
                let _ = theme::icon_button(ui, theme::Icon::Plus, "新建");
            });
            let rect = egui::Rect::from_min_max(
                egui::pos2(ui.max_rect().left() - 12.0, ui.min_rect().top() - 6.0),
                egui::pos2(ui.max_rect().right() + 12.0, ui.min_rect().bottom() + 6.0),
            );
            theme::smartisan::tab_background(ui.painter(), background, rect);
        });
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        ctx.set_pixels_per_point(2.0);
        let config = AppConfig::default();
        let mut markdown = String::from("# 关于加强全省中小学教育教学质量管理工作的函\n\n");
        for i in 1..=40 {
            markdown.push_str(&format!("## 第{i}节 关于进一步做好相关工作的意见\n\n"));
            for _ in 0..4 {
                markdown.push_str("各地各校要深刻认识本项工作的重要意义，紧紧围绕立德树人根本任务，统筹推进课程建设、师资培养与教学评价改革，确保各项部署落到实处、见到实效。\n\n");
            }
        }
        let doc = DraftSession::with_markdown(1, &config, markdown);
        let (sender, _keep) = std::sync::mpsc::channel();
        Self {
            ctx,
            doc,
            config,
            sender,
            status: String::new(),
            version_switch: None,
            revert_confirm: None,
            actions: Vec::new(),
            export_links: ExportLinks::default(),
            metrics: crate::metrics::Metrics::default(),
            clock: 0,
            _keep,
        }
    }

    /// 画一帧并取回纸张（最大的那块白色矩形）在屏幕上的位置与大小。
    /// 这是"眼睛看到的版面"，用来验证层变换画出的画面与重排出的一致。
    fn paper(&mut self, width: f32) -> egui::Rect {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 900.0),
            )),
            ..Default::default()
        };
        let out = crate::test_support::run_ui(&self.ctx, raw, |ui| {
            let mut page = DraftPage {
                doc: &mut self.doc,
                config: &mut self.config,
                store: None,
                sender: &self.sender,
                status: &mut self.status,
                version_switch: &mut self.version_switch,
                revert_confirm: &mut self.revert_confirm,
                actions: &mut self.actions,
                export_links: &mut self.export_links,
                metrics: &mut self.metrics,
            };
            page.markdown_render(ui);
        });
        // 形状会嵌套（Frame 的底 + 阴影 + 内容都装在 Shape::Vec 里），要递归摊平。
        fn collect(shape: &egui::epaint::Shape, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::epaint::Shape::Rect(rect)
                    if rect.fill == theme::paper::bg() && rect.rect.is_finite() =>
                {
                    out.push(rect.rect);
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut rects = Vec::new();
        for clipped in &out.shapes {
            collect(&clipped.shape, &mut rects);
        }
        rects
            .into_iter()
            .max_by(|a, b| {
                (a.width() * a.height())
                    .partial_cmp(&(b.width() * b.height()))
                    .unwrap()
            })
            .expect("本帧应当画出纸张")
    }

    /// 用给定的原始输入画一帧审校提示面板。
    fn warnings_frame(&mut self, raw: egui::RawInput) {
        let _ = crate::test_support::run_ui(&self.ctx, raw, |ui| {
            let mut page = DraftPage {
                doc: &mut self.doc,
                config: &mut self.config,
                store: None,
                sender: &self.sender,
                status: &mut self.status,
                version_switch: &mut self.version_switch,
                revert_confirm: &mut self.revert_confirm,
                actions: &mut self.actions,
                export_links: &mut self.export_links,
                metrics: &mut self.metrics,
            };
            page.warnings_ui(ui);
        });
    }

    /// 在审校面板的 `at` 处点一下（按下与抬起分两帧，egui 才认这是一次点击）。
    fn click_warning_at(&mut self, at: egui::Pos2) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        for pressed in [true, false] {
            self.warnings_frame(egui::RawInput {
                screen_rect: Some(screen),
                events: vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            });
        }
    }
}

/// 清空审校稿不能只删正文：所有依赖旧正文的提示、定位、查找、导出结果和
/// 版本来源标记必须一起复位，否则界面会继续展示已经失效的信息。
#[test]
fn clearing_review_output_resets_dependent_ui_state() {
    let mut harness = Harness::new();
    harness.doc.output_files.push(PathBuf::from("old.pdf"));
    harness.doc.export_error = Some("旧错误".into());
    harness.doc.loaded_version = Some(LoadedVersion {
        manuscript_id: 7,
        version_number: 3,
        name: "旧版本".into(),
    });
    harness.doc.draft_diff.base = Some(2);
    harness.doc.result_drawer_open = true;
    harness.doc.preview_anchor = Some(PreviewAnchor {
        range: 0..1,
        text: "#".into(),
    });
    harness.doc.pending_source_jump = Some(8);
    harness.doc.pending_source_selection = Some(1..4);
    harness.doc.pending_render_jump = true;
    harness.doc.markdown_find.open = true;

    {
        let mut page = DraftPage {
            doc: &mut harness.doc,
            config: &mut harness.config,
            store: None,
            sender: &harness.sender,
            status: &mut harness.status,
            version_switch: &mut harness.version_switch,
            revert_confirm: &mut harness.revert_confirm,
            actions: &mut harness.actions,
            export_links: &mut harness.export_links,
            metrics: &mut harness.metrics,
        };
        page.clear_review_output();
    }

    assert!(harness.doc.generated_markdown.is_empty());
    assert!(harness.doc.output_files.is_empty());
    assert!(harness.doc.export_error.is_none());
    assert!(harness.doc.loaded_version.is_none());
    assert!(harness.doc.draft_diff.base.is_none());
    assert!(!harness.doc.result_drawer_open);
    assert!(harness.doc.preview_anchor.is_none());
    assert!(harness.doc.pending_source_jump.is_none());
    assert!(harness.doc.pending_source_selection.is_none());
    assert!(!harness.doc.pending_render_jump);
    assert!(!harness.doc.markdown_find.open);
}

/// 词表命中要一路走到「可采纳的修订建议」，而不是停在一条只能看的提示上。
///
/// 这条锁的是阶段 0 的接线本身：`replacement` 原先在 `ProofNote → ReviewNote`
/// 那一步被丢掉，词表里「必错可一键替换」的能力界面上根本取不到。断了任何
/// 一节——校验没投进总线、`after` 没带过来、闸门误拦——这里都会红。
#[test]
fn a_lexicon_hit_becomes_an_acceptable_revision() {
    let mut harness = Harness::new();
    harness.doc.generated_markdown = "# 标题\n\n按上级布署办理。\n".into();

    let mut page = DraftPage {
        doc: &mut harness.doc,
        config: &mut harness.config,
        store: None,
        sender: &harness.sender,
        status: &mut harness.status,
        version_switch: &mut harness.version_switch,
        revert_confirm: &mut harness.revert_confirm,
        actions: &mut harness.actions,
        export_links: &mut harness.export_links,
        metrics: &mut harness.metrics,
    };
    page.revalidate();

    let hit = page
        .doc
        .revisions
        .items()
        .iter()
        .find(|item| item.before() == "布署")
        .expect("「布署」应当产生一条修订建议");
    assert_eq!(
        hit.after.as_deref(),
        Some("部署"),
        "必错条目应当带上替换写法"
    );
    assert!(hit.is_actionable(), "刚校验出来的必错建议应当可以一键采纳");
    let id = hit.id;

    page.accept_revision(id);
    assert_eq!(page.doc.generated_markdown, "# 标题\n\n按上级部署办理。\n");
    assert_eq!(page.doc.revisions.applied_count(), 1);

    page.undo_revision();
    assert_eq!(
        page.doc.generated_markdown, "# 标题\n\n按上级布署办理。\n",
        "撤销应当把原文一字不差地还回来"
    );
    assert_eq!(page.doc.revisions.applied_count(), 0);
}

/// 孤行提示要能点：点中之后切回 Markdown 视图，并选中出问题的那一段。
///
/// 逐点扫过面板而不是写死坐标——egui 改了边距也不会让这条测试失灵，而提示
/// 一旦不可点，扫遍整块面板也不会触发，测试照样红。
#[test]
fn clicking_a_located_warning_selects_that_paragraph() {
    let mut harness = Harness::new();
    let filler: String = "工作要点部署要求经研究决定开展检查评估现将有关事项通知如下请各单位遵照执行并及时反馈情况我们将根据反馈意见完善制度"
        .chars()
        .take(54)
        .collect();
    harness.doc.generated_markdown = format!("# 标题\n\n短段落。\n\n{filler}。\n");
    harness.doc.preview_mode = PreviewMode::Rendered;

    {
        let mut page = DraftPage {
            doc: &mut harness.doc,
            config: &mut harness.config,
            store: None,
            sender: &harness.sender,
            status: &mut harness.status,
            version_switch: &mut harness.version_switch,
            revert_confirm: &mut harness.revert_confirm,
            actions: &mut harness.actions,
            export_links: &mut harness.export_links,
            metrics: &mut harness.metrics,
        };
        page.revalidate();
    }
    let expected = harness
        .doc
        .warnings
        .iter()
        .find_map(|note| note.span.clone())
        .expect("孤行提示应带定位范围");
    assert_eq!(
        harness.doc.generated_markdown[expected.clone()],
        format!("{filler}。")
    );

    // 先画一帧完成布局，之后的点击才有可命中的控件。
    harness.warnings_frame(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 600.0),
        )),
        ..Default::default()
    });
    let mut hit = None;
    'scan: for y in 0..40 {
        for x in 0..30 {
            harness.click_warning_at(egui::pos2(4.0 + x as f32 * 30.0, 4.0 + y as f32 * 8.0));
            if harness.doc.preview_mode == PreviewMode::Source {
                hit = Some((x, y));
                break 'scan;
            }
        }
    }
    assert!(
        hit.is_some(),
        "整块审校面板都点不动，孤行提示没有做成可点的"
    );
    assert_eq!(
        harness.doc.pending_source_selection,
        Some(expected),
        "点击后应选中提示指向的那一段"
    );
}

/// 从对照模式切回 Markdown：光标跟随高亮要清掉，且要登记一次「把光标滚回
/// 视野」的请求——否则旧高亮会一直留在版面上，重排后用户找不到刚才的位置。
#[test]
fn leaving_split_mode_clears_cursor_follow_highlight() {
    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Split;
    let start = harness.doc.generated_markdown.find('加').unwrap();
    let range = start..start + "加强".len();
    let text = harness.doc.generated_markdown[range.clone()].to_owned();
    harness.doc.preview_anchor = Some(PreviewAnchor { range, text });
    harness.doc.preview_selection = Some(start..start + "加强".len());
    harness.doc.preview_cursor_line = Some(start);

    {
        let mut page = DraftPage {
            doc: &mut harness.doc,
            config: &mut harness.config,
            store: None,
            sender: &harness.sender,
            status: &mut harness.status,
            version_switch: &mut harness.version_switch,
            revert_confirm: &mut harness.revert_confirm,
            actions: &mut harness.actions,
            export_links: &mut harness.export_links,
            metrics: &mut harness.metrics,
        };
        page.switch_preview_mode(PreviewMode::Source);
    }

    assert_eq!(harness.doc.preview_mode, PreviewMode::Source);
    assert!(
        harness.doc.preview_anchor.is_none(),
        "对照模式的高亮不该带到单栏模式"
    );
    assert!(harness.doc.preview_selection.is_none());
    assert!(harness.doc.preview_cursor_line.is_none());
    assert!(
        harness.doc.pending_source_reveal,
        "切回带编辑框的模式后应把光标滚回视野"
    );
}

/// 切到不带编辑框的模式不需要拉回光标；查找条开着时不抢输入焦点。
#[test]
fn switching_mode_respects_rendered_view_and_find_bar() {
    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Split;
    harness.doc.markdown_find.open = true;
    let (reveal_with_find, reveal_rendered, reveal_source) = {
        let mut page = DraftPage {
            doc: &mut harness.doc,
            config: &mut harness.config,
            store: None,
            sender: &harness.sender,
            status: &mut harness.status,
            version_switch: &mut harness.version_switch,
            revert_confirm: &mut harness.revert_confirm,
            actions: &mut harness.actions,
            export_links: &mut harness.export_links,
            metrics: &mut harness.metrics,
        };
        // 查找条开着：切回 Markdown 也不抢焦点。
        page.switch_preview_mode(PreviewMode::Source);
        let reveal_with_find = page.doc.pending_source_reveal;
        // 没有编辑框的模式：无需拉回光标。
        page.switch_preview_mode(PreviewMode::Split);
        page.doc.markdown_find.open = false;
        page.switch_preview_mode(PreviewMode::Rendered);
        let reveal_rendered = page.doc.pending_source_reveal;
        // 编辑框与查找条都就绪：正常登记拉回请求。
        page.switch_preview_mode(PreviewMode::Source);
        let reveal_source = page.doc.pending_source_reveal;
        (reveal_with_find, reveal_rendered, reveal_source)
    };
    assert!(!reveal_with_find, "查找条开着时不应请求聚焦编辑框");
    assert!(!reveal_rendered);
    assert!(reveal_source);
}

impl Harness {
    /// 画一帧，窗格宽度为 `width`。返回 (耗时 ms, 是否整体重建了字体图集)。
    fn frame(&mut self, width: f32) -> (f32, bool) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 900.0),
            )),
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let mut out = self.ctx.clone().run_ui(raw, |ui| {
            // 字体图集增量要给 `font_atlas_reuploaded` 看，不能提前清掉。
            let mut page = DraftPage {
                doc: &mut self.doc,
                config: &mut self.config,
                store: None,
                sender: &self.sender,
                status: &mut self.status,
                version_switch: &mut self.version_switch,
                revert_confirm: &mut self.revert_confirm,
                actions: &mut self.actions,
                export_links: &mut self.export_links,
                metrics: &mut self.metrics,
            };
            page.markdown_render(ui);
        });
        let elapsed = started.elapsed().as_secs_f32() * 1000.0;
        let reuploaded = font_atlas_reuploaded(&out);
        out.textures_delta.clear();
        (elapsed, reuploaded)
    }

    /// 在同一宽度上反复出帧，直到字体图集连续若干帧不再整份重传。
    ///
    /// 预热要“安静下来”而不是数固定帧数：不同平台装进图集的字体不同，
    /// 图集尺寸与装填节奏也不同。装填超过八成时 epaint 会在下一帧开头
    /// 整份重建，重建那一帧同样是一次整张上传——若正好落在测量区间里，
    /// 就会被算成“拖动导致重建”。等图集稳定后再开始计数，测的才是拖动
    /// 本身的影响。
    fn settle_font_atlas(&mut self) {
        const QUIET_FRAMES: usize = 4;
        const FRAME_LIMIT: usize = 200;
        let mut quiet = 0;
        for _ in 0..FRAME_LIMIT {
            let (_, reuploaded) = self.frame(SETTLE_WIDTH);
            quiet = if reuploaded { 0 } else { quiet + 1 };
            if quiet >= QUIET_FRAMES {
                return;
            }
        }
        panic!("字体图集在 {FRAME_LIMIT} 帧内没有稳定，无法测量拖动期间的图集行为");
    }

    /// 跑若干帧完整的审校区（含右缘导航），鼠标一直停在 `pointer`。
    /// 展开是带动画的，一帧看不出结果，所以要把时间往前推够。
    fn preview_frames(&mut self, pointer: egui::Pos2, frames: usize) -> egui::FullOutput {
        let mut output = None;
        for _ in 0..frames {
            self.clock += 1;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 900.0),
                )),
                time: Some(self.clock as f64 / 60.0),
                predicted_dt: 1.0 / 60.0,
                events: vec![egui::Event::PointerMoved(pointer)],
                ..Default::default()
            };
            let frame = crate::test_support::run_ui(&self.ctx, raw, |ui| {
                let mut page = DraftPage {
                    doc: &mut self.doc,
                    config: &mut self.config,
                    store: None,
                    sender: &self.sender,
                    status: &mut self.status,
                    version_switch: &mut self.version_switch,
                    revert_confirm: &mut self.revert_confirm,
                    actions: &mut self.actions,
                    export_links: &mut self.export_links,
                    metrics: &mut self.metrics,
                };
                page.preview_ui(ui);
            });
            output = Some(frame);
        }
        output.expect("至少画一帧")
    }

    /// 右缘刻度带这一帧占的位置。导航关掉时它根本不登记，返回 None。
    fn navigator_rail(&self) -> Option<egui::Rect> {
        self.ctx
            .read_response(egui::Id::new("gw_nav_rail_strip"))
            .map(|response| response.rect)
    }

    /// 在 `at` 处按下再抬起。egui 要跨帧才认这是一次点击。
    fn preview_click(&mut self, at: egui::Pos2) {
        for pressed in [true, false] {
            self.clock += 1;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 900.0),
                )),
                time: Some(self.clock as f64 / 60.0),
                predicted_dt: 1.0 / 60.0,
                events: vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            let _ = crate::test_support::run_ui(&self.ctx, raw, |ui| {
                let mut page = DraftPage {
                    doc: &mut self.doc,
                    config: &mut self.config,
                    store: None,
                    sender: &self.sender,
                    status: &mut self.status,
                    version_switch: &mut self.version_switch,
                    revert_confirm: &mut self.revert_confirm,
                    actions: &mut self.actions,
                    export_links: &mut self.export_links,
                    metrics: &mut self.metrics,
                };
                page.preview_ui(ui);
            });
        }
    }
}

fn percentile(values: &mut [f32], p: f32) -> f32 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[((values.len() as f32 - 1.0) * p) as usize]
}

/// 源码缩略图必须覆盖整篇，而不是只画当前屏幕；点底部应把长稿滚到后段。
#[test]
fn source_minimap_click_reaches_later_sections() {
    let mut harness = Harness::new();
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);
    assert_eq!(harness.doc.source_outline.len(), 41);
    assert!(harness.doc.source_minimap.rows.len() > 100);
    assert!(harness.doc.source_minimap.content_height > harness.doc.source_minimap.viewport_height);
    let rect = harness
        .ctx
        .read_response(egui::Id::new("gw_source_minimap"))
        .expect("源码模式应显示可交互的缩略图")
        .rect;
    let before = harness.doc.source_minimap.offset;
    harness.preview_click(egui::pos2(rect.center().x, rect.bottom() - 10.0));
    assert!(
        harness.doc.source_minimap.offset > before + 100.0,
        "点缩略图底部后应跳到后段，实际偏移 {}",
        harness.doc.source_minimap.offset
    );
}

#[test]
fn source_minimap_wheel_scrolls_its_own_long_canvas() {
    let mut harness = Harness::new();
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);
    let rect = harness
        .ctx
        .read_response(egui::Id::new("gw_source_minimap"))
        .expect("源码模式应显示缩略图")
        .rect;
    assert!(
        harness.doc.source_minimap.content_height * 0.16 > rect.height(),
        "测试稿应让缩略图长于右侧栏"
    );
    let editor_before = harness.doc.source_minimap.offset;
    harness.clock += 1;
    let at = rect.center();
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 900.0),
        )),
        time: Some(harness.clock as f64 / 60.0),
        events: vec![
            egui::Event::PointerMoved(at),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -160.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        ..Default::default()
    };
    let _ = crate::test_support::run_ui(&harness.ctx, raw, |ui| {
        let mut page = DraftPage {
            doc: &mut harness.doc,
            config: &mut harness.config,
            store: None,
            sender: &harness.sender,
            status: &mut harness.status,
            version_switch: &mut harness.version_switch,
            revert_confirm: &mut harness.revert_confirm,
            actions: &mut harness.actions,
            export_links: &mut harness.export_links,
            metrics: &mut harness.metrics,
        };
        page.preview_ui(ui);
    });
    assert!(harness.doc.source_minimap.mini_scroll > 0.0);
    assert_eq!(harness.doc.source_minimap.offset, editor_before);
    harness.preview_click(at);
    assert!(harness.doc.source_minimap.offset > editor_before);
}

#[test]
fn source_minimap_edge_drag_advances_smoothly_across_frames() {
    fn frame(harness: &mut Harness, events: Vec<egui::Event>) {
        harness.clock += 1;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            time: Some(harness.clock as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let _ = crate::test_support::run_ui(&harness.ctx, raw, |ui| {
            let mut page = DraftPage {
                doc: &mut harness.doc,
                config: &mut harness.config,
                store: None,
                sender: &harness.sender,
                status: &mut harness.status,
                version_switch: &mut harness.version_switch,
                revert_confirm: &mut harness.revert_confirm,
                actions: &mut harness.actions,
                export_links: &mut harness.export_links,
                metrics: &mut harness.metrics,
            };
            page.preview_ui(ui);
        });
    }

    let mut harness = Harness::new();
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);
    let rect = harness
        .ctx
        .read_response(egui::Id::new("gw_source_minimap"))
        .expect("源码模式应显示缩略图")
        .rect;
    // 可见框现在随正文视口等比缩放；从中心拖到下沿外 10 点，验证小幅越界
    // 的速度，而不是依赖旧版固定 96 点高的可见框。
    let half_box = harness.doc.source_minimap.viewport_height * 0.16 * 0.5;
    let start = egui::pos2(rect.center().x, rect.top() + half_box);
    let edge = egui::pos2(start.x, rect.bottom() - half_box + 10.0);
    frame(
        &mut harness,
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(&mut harness, vec![egui::Event::PointerMoved(edge)]);
    let first = harness.doc.source_minimap.mini_scroll;
    frame(&mut harness, vec![egui::Event::PointerMoved(edge)]);
    let second = harness.doc.source_minimap.mini_scroll;
    assert!(first > 0.0 && second > first, "触边后应继续缓慢滚动");
    assert!(second - first < 2.0, "按住边缘一帧不应突然跳过多行");
    frame(&mut harness, vec![egui::Event::PointerMoved(edge)]);
    let third = harness.doc.source_minimap.mini_scroll;
    assert!(third > second && third - second < 2.0);
    frame(
        &mut harness,
        vec![egui::Event::PointerButton {
            pos: edge,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let released = harness.doc.source_minimap.mini_scroll;
    frame(&mut harness, vec![egui::Event::PointerMoved(edge)]);
    assert!(
        (harness.doc.source_minimap.mini_scroll - released).abs() < 2.0,
        "松开后正文回传不应把缩略图重新居中"
    );
}

/// 指着刻度点一下，就该跳到那一节——这是整个导航唯一的核心动作。
/// 顺带锁住刻度带确实压在版面之上拿得到指针：它要是被下面的正文块抢了点击，
/// 表现就是点了刻度却选中了一段正文。
#[test]
fn clicking_a_tick_jumps_to_that_heading() {
    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    assert!(
        harness.config.show_preview_navigator,
        "导航默认应当是开着的"
    );
    // 先跑几帧把版面和刻度带的位置量准。
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);
    let rail = harness.navigator_rail().expect("导航开着时应当有刻度带");

    harness.doc.pending_source_jump = None;
    harness.preview_click(rail.center());

    let jumped = harness
        .doc
        .pending_source_jump
        .expect("点刻度应当发出跳转请求");
    let starts = navigator::collect_entries(
        &harness.doc.generated_markdown,
        &harness.config.numbering,
        harness.doc.draft.kind,
    )
    .into_iter()
    .map(|entry| entry.line.start)
    .collect::<Vec<_>>();
    assert!(
        starts.contains(&jumped),
        "跳转目标 {jumped} 应当正好落在某个标题行的行首"
    );
}

/// 靠近右缘时，整篇大纲要一次铺开；而且每一条的高度只跟层级走——指针沿
/// 刻度带从上划到下，同一条标题不许变高变矮。曾经按指针远近逐档放大的
/// 那版就是在这里露馅的：一列字忽大忽小，看着晃眼。
#[test]
fn approaching_the_right_edge_lays_out_a_steady_outline() {
    fn bands(harness: &Harness, count: usize) -> Vec<(usize, egui::Rect)> {
        (0..count)
            .filter_map(|index| {
                harness
                    .ctx
                    .read_response(egui::Id::new(("gw_nav_label", index)))
                    .map(|hit| (index, hit.rect))
            })
            .collect()
    }

    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);
    let rail = harness.navigator_rail().expect("导航开着时应当有刻度带");
    let entries = navigator::collect_entries(
        &harness.doc.generated_markdown,
        &harness.config.numbering,
        harness.doc.draft.kind,
    );

    // 指针停在刻度带偏上处，跑够帧数让淡入与滚动都走完。
    let x = rail.center().x;
    harness.preview_frames(egui::pos2(x, rail.top() + rail.height() * 0.15), 30);
    let high = bands(&harness, entries.len());
    harness.preview_frames(egui::pos2(x, rail.top() + rail.height() * 0.85), 30);
    let low = bands(&harness, entries.len());

    assert!(
        high.len() >= 10,
        "应当一次铺开整篇大纲，而不是焦点附近那几条，实测 {} 条",
        high.len()
    );
    assert!(high.len() <= entries.len());

    // 两次都在板上、且都不在板的头尾那一条（头尾的横带要铺到板沿，本来就
    // 比中间高）的标题，高度必须分毫不差。
    let interior = |rows: &[(usize, egui::Rect)]| {
        rows.iter()
            .skip(1)
            .take(rows.len().saturating_sub(2))
            .copied()
            .collect::<Vec<_>>()
    };
    let (high_inner, low_inner) = (interior(&high), interior(&low));
    let mut compared = 0;
    for (index, rect) in &high_inner {
        let Some((_, other)) = low_inner.iter().find(|(at, _)| at == index) else {
            continue;
        };
        assert!(
            (rect.height() - other.height()).abs() < 0.01,
            "第 {index} 条随指针变了高度：{} 对 {}",
            rect.height(),
            other.height()
        );
        compared += 1;
    }
    assert!(compared > 0, "两次没有共同的标题可比，这条测试就没验到东西");

    for (_, rect) in &high {
        // 可点的是整条横带，但到刻度带左缘为止：刻度那一条仍旧归刻度，
        // 它答的是"文中什么位置"，与大纲的第几行不是一回事。
        assert!(
            rect.right() <= rail.left() + 1.0,
            "标题带不该盖住刻度带，实测右边界 {} 对刻度带左边界 {}",
            rect.right(),
            rail.left()
        );
        assert!(
            rect.left() < rail.left(),
            "标题带应当铺到刻度带左侧的纸面上，实测左边界 {} 对刻度带左边界 {}",
            rect.left(),
            rail.left()
        );
        // 板不超出刻度带那一段的上下（两端各留一档板的内边距）。
        assert!(
            rect.top() > rail.top() - 12.0 && rect.bottom() < rail.bottom() + 12.0,
            "标题带越出了刻度带那一段：{rect:?} 对 {rail:?}"
        );
    }
    // 横带首尾相接铺满整块板：板是不透明的，板上不许有哪一处点下去什么都不发生。
    for pair in high.windows(2) {
        assert!(
            (pair[0].1.bottom() - pair[1].1.top()).abs() < 0.01,
            "横带之间漏了缝：{:?} 与 {:?}",
            pair[0].1,
            pair[1].1
        );
    }
}

/// 「视图 → 导航」关掉之后，右缘就该彻底安静：刻度带一层都不建，
/// 在同一位置点下去也不许有任何跳转。关不掉的开关等于没有这个开关。
#[test]
fn turning_the_navigator_off_keeps_the_right_edge_quiet() {
    // 先在开着的状态下量出刻度带的位置，再关掉，点同一个地方。
    let mut located = Harness::new();
    located.doc.preview_mode = PreviewMode::Rendered;
    located.preview_frames(egui::pos2(10.0, 10.0), 3);
    let rail = located.navigator_rail().expect("导航开着时应当有刻度带");

    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    harness.config.show_preview_navigator = false;
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);
    assert!(
        harness.navigator_rail().is_none(),
        "导航关掉后不该再登记刻度带"
    );

    harness.doc.pending_source_jump = None;
    harness.preview_click(rail.center());
    assert!(
        harness.doc.pending_source_jump.is_none(),
        "导航关掉后，点在原本的刻度带位置不该跳转"
    );
}

/// 导航刻度靠回查预览注册的 widget id 拿标题的版面位置：id 在
/// `preview::layout` 那边构造，在 `navigator` 这边重建。两边一旦对不上，
/// 导航不会报错，只会**静默**退化成按源码字节估位——刻度看着还在，位置却是错的。
/// 这条测试把两边钉在一起，同时确认滚动区外的标题也照样注册了矩形
/// （否则长稿里大半标题量不到，整篇都会掉进估位分支）。
#[test]
fn every_heading_is_locatable_through_the_preview_widget_ids() {
    let mut harness = Harness::new();
    // egui 要一两帧才量准滚动区。
    harness.frame(900.0);
    harness.frame(900.0);
    let entries = navigator::collect_entries(
        &harness.doc.generated_markdown,
        &harness.config.numbering,
        harness.doc.draft.kind,
    );
    let headings = entries
        .iter()
        .filter(|entry| entry.level >= 2)
        .collect::<Vec<_>>();
    assert!(headings.len() >= 20, "样稿应当有足够多的标题用来判断");

    let tops = headings
        .iter()
        .map(|entry| {
            navigator::heading_top(&harness.ctx, &entry.line)
                .unwrap_or_else(|| panic!("标题「{}」没能按 id 查回矩形", entry.text))
        })
        .collect::<Vec<_>>();
    // 位置还要顺着正文往下走：第 n 节必须排在第 n+1 节上面，
    // 否则刻度的疏密就不是各节的真实长短。
    assert!(
        tops.windows(2).all(|pair| pair[0] < pair[1]),
        "标题的版面位置应当自上而下单调递增，实测 {tops:?}"
    );
}

impl Harness {
    /// 本帧画在页边的行号：号码连同它在屏幕上的位置，自上而下。
    /// 滚动区外的行不画也不登记，所以这里拿到的就是"看得见的那些号"。
    fn paper_line_numbers(&self) -> Vec<(usize, egui::Rect)> {
        (1..=400usize)
            .filter_map(|number| {
                self.ctx
                    .read_response(egui::Id::new(("gw-preview-line-number", number)))
                    .map(|response| (number, response.rect))
            })
            .collect()
    }

    /// 文档标题那一块在版面上的位置。版心左沿与标题所在的行高都取自它。
    fn title_block(&self) -> egui::Rect {
        let title = self
            .doc
            .generated_markdown
            .lines()
            .next()
            .expect("样稿第一行是文档标题");
        self.ctx
            .read_response(egui::Id::new(("gw-preview-block", 0usize, title.len())))
            .expect("文档标题应当登记成可点的块")
            .rect
    }
}

/// 纸面行号是投屏对稿时唯一的坐标系，它必须满足三件事：号码自上而下逐行递增、
/// 整列右对齐成一条干净的竖线、全部落在版心之外的页边留白里。三件事错一件，
/// 页边那一列就从"刻度"变成"压在正文旁边的一串数字"。
#[test]
fn paper_line_numbers_stand_in_the_margin_as_one_column() {
    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    harness.config.show_preview_line_numbers = true;
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);

    let numbers = harness.paper_line_numbers();
    assert!(
        numbers.len() >= 10,
        "一屏纸面至少该标出十来个行号，实测 {}",
        numbers.len()
    );
    assert!(
        numbers.windows(2).all(|pair| pair[0].0 + 1 == pair[1].0),
        "号码必须一号不落地连着数：{:?}",
        numbers
            .iter()
            .map(|(number, _)| *number)
            .collect::<Vec<_>>()
    );
    assert!(
        numbers
            .windows(2)
            .all(|pair| pair[0].1.top() < pair[1].1.top()),
        "号码必须自上而下排，不能出现下一号反而更靠上"
    );
    let rights = numbers
        .iter()
        .map(|(_, rect)| rect.right())
        .collect::<Vec<_>>();
    let spread = rights.iter().copied().fold(f32::MIN, f32::max)
        - rights.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        spread < 1.0,
        "整列号码要右对齐成一条竖线，右缘却相差 {spread}"
    );

    let body_left = harness.title_block().left();
    for (number, rect) in &numbers {
        assert!(
            rect.right() < body_left,
            "第 {number} 号压进了版心：号码右缘 {} ≥ 版心左沿 {body_left}",
            rect.right()
        );
    }
}

/// 号码只给来自 Markdown、改得动的行。红头、密级、文号排在文档标题上面，
/// 它们出自表单，报一个号过去也改不动——所以 1 号必须正落在文档标题那一行。
#[test]
fn the_first_number_lands_on_the_document_title() {
    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    harness.config.show_preview_line_numbers = true;
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);

    let (number, rect) = harness
        .paper_line_numbers()
        .into_iter()
        .next()
        .expect("开着行号就该有号");
    assert_eq!(number, 1, "页边第一个号应当是 1");
    let title = harness.title_block();
    assert!(
        rect.center().y > title.top() && rect.center().y < title.bottom(),
        "1 号应当与文档标题齐平：号在 {:?}，标题在 {title:?}",
        rect.center()
    );
}

/// 点一下号码，光标就该落到那一行的源码上——看稿的人报号、敲字的人点号，
/// 中间不必再数。号码要是点不动，它就只是一列装饰。
#[test]
fn clicking_a_paper_line_number_takes_the_cursor_to_that_line() {
    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    harness.config.show_preview_line_numbers = true;
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);

    let (_, rect) = harness
        .paper_line_numbers()
        .into_iter()
        .find(|(number, _)| *number == 4)
        .expect("一屏之内应当画得出第 4 号");
    harness.doc.pending_source_jump = None;
    harness.preview_click(rect.center());

    let jumped = harness
        .doc
        .pending_source_jump
        .expect("点行号应当把光标带到那一行");
    let starts = markdown::line_ranges(&harness.doc.generated_markdown)
        .into_iter()
        .map(|range| range.start)
        .collect::<Vec<_>>();
    assert!(
        starts.contains(&jumped),
        "跳转目标 {jumped} 应当正好落在某一行源码的行首"
    );
}

/// 页边的号必须与那一行的正文**踩同一条基线**。公文行距 28 磅比字高出一截，
/// 字靠在行的上半段，号码若按行框居中就整体偏下，看着像挂在两行中间。
#[test]
fn every_number_sits_on_the_baseline_of_its_line() {
    /// 摊平图形，取出每一行文字的 (基线, 左缘, 是不是纯数字)。
    fn collect(shape: &egui::epaint::Shape, out: &mut Vec<(f32, f32, bool)>) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                for row in text.galley.rows.iter() {
                    let Some(glyph) = row.glyphs.first() else {
                        continue;
                    };
                    out.push((
                        text.pos.y + row.pos.y + glyph.pos.y,
                        text.pos.x + row.pos.x + glyph.pos.x,
                        row.glyphs.iter().all(|glyph| glyph.chr.is_ascii_digit()),
                    ));
                }
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    harness.config.show_preview_line_numbers = true;
    let output = harness.preview_frames(egui::pos2(-50.0, -50.0), 3);
    let body_left = harness.title_block().left();

    let mut lines = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut lines);
    }
    let (numbers, body): (Vec<_>, Vec<_>) = lines
        .iter()
        .partition(|(_, left, digits)| *digits && *left < body_left);
    assert!(numbers.len() >= 10, "一屏纸面至少该标出十来个行号");
    for (baseline, _, _) in &numbers {
        assert!(
            body.iter()
                .any(|(other, _, _)| (other - baseline).abs() < 0.51),
            "基线 {baseline} 上的行号没有与任何一行正文对齐"
        );
    }
}

/// 空稿时正文原点那句提示：每个主题、源码与对照两种模式都要画出来，而且要画成
/// 正文的一行——同一处原点、同一副字面、同一个字号、同一条基线。
///
/// 从前走的是 `TextEdit::hint_text`：那行字只认界面默认字体与默认行高、贴着行框
/// 顶端，调字号、换字面、换主题它都不动，而且只有锤子稿纸那一套才有。现在提示语
/// 自己按正文的排版排一遍、落在正文原点，所以换哪套主题都该是同一副样子。
#[test]
fn every_theme_draws_the_empty_hint_on_the_body_baseline() {
    use super::editor::EMPTY_HINT;

    /// 摊平图形，收下编辑器字体的每段文字：正文、字面、首字基线与左缘（屏幕坐标）。
    fn collect(
        shape: &egui::epaint::Shape,
        editor_family: &egui::FontFamily,
        out: &mut Vec<(String, egui::FontId, f32, f32)>,
    ) {
        match shape {
            egui::epaint::Shape::Text(text) => {
                let Some(row) = text.galley.rows.first() else {
                    return;
                };
                let Some(glyph) = row.glyphs.first() else {
                    return;
                };
                let Some(section) = text.galley.job.sections.first() else {
                    return;
                };
                let font = section.format.font_id.clone();
                if &font.family != editor_family {
                    return;
                }
                out.push((
                    text.galley.job.text.clone(),
                    font,
                    text.pos.y + row.pos.y + glyph.pos.y,
                    text.pos.x + row.pos.x + glyph.pos.x,
                ));
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, editor_family, out);
                }
            }
            _ => {}
        }
    }

    let mut harness = Harness::new();
    harness.ctx.set_pixels_per_point(1.0);
    harness.config.show_source_outline = false;
    harness.doc.form_collapsed = true;
    // 字号与正文之外的字面都换成非默认值：提示语若退回界面默认排版，这里就量得出来。
    harness.config.editor_font_size = 22.0;
    harness.config.editor_fonts.body = crate::models::EditorFontFace::Biaosong;
    let off_screen = egui::pos2(-1000.0, -1000.0);
    let mut problems = Vec::new();
    for name in crate::models::ThemeName::ALL {
        theme::set_current(name);
        theme::configure_style(&harness.ctx);
        let editor_family = theme::editor_face_family(
            harness
                .config
                .editor_fonts
                .face(crate::models::EditorFontSlot::Body),
        );
        let font_size = harness.config.editor_font_size;
        for mode in [PreviewMode::Source, PreviewMode::Split] {
            harness.doc.preview_mode = mode;

            // 空稿：只该有那句提示，量它的字面、基线、左缘。
            harness.doc.generated_markdown.clear();
            let empty_frame = harness.preview_frames(off_screen, 3);
            let mut painted = Vec::new();
            for clipped in &empty_frame.shapes {
                collect(&clipped.shape, &editor_family, &mut painted);
            }
            let hint = painted
                .iter()
                .find(|(text, ..)| text == EMPTY_HINT)
                .cloned();

            // 换成一行业已写下的正文，量正文那一行的字面、基线、左缘来对比。
            harness.doc.generated_markdown = "正文第一行".into();
            let body_frame = harness.preview_frames(off_screen, 3);
            let mut painted = Vec::new();
            for clipped in &body_frame.shapes {
                collect(&clipped.shape, &editor_family, &mut painted);
            }
            let body = painted
                .iter()
                .find(|(text, ..)| text == "正文第一行")
                .cloned();

            match (hint, body) {
                (Some(hint), Some(body)) => {
                    let place = format!("{name:?} 的 {mode:?} 模式");
                    if hint.1.family != body.1.family || hint.1.size != body.1.size {
                        problems.push(format!(
                            "{place}：提示语用的是 {:?} / {}，正文用的是 {:?} / {}",
                            hint.1.family, hint.1.size, body.1.family, body.1.size
                        ));
                    }
                    if (hint.1.size - font_size).abs() > 0.01 {
                        problems.push(format!(
                            "{place}：提示语字号 {} 没有跟上设置的编辑字号 {font_size}",
                            hint.1.size
                        ));
                    }
                    if (hint.2 - body.2).abs() > 0.01 {
                        problems.push(format!(
                            "{place}：提示语基线 {} 与正文基线 {} 不在同一条线上",
                            hint.2, body.2
                        ));
                    }
                    if (hint.3 - body.3).abs() > 0.01 {
                        problems.push(format!(
                            "{place}：提示语左缘 {} 与正文左缘 {} 没有对齐",
                            hint.3, body.3
                        ));
                    }
                }
                (None, _) => {
                    problems.push(format!("{name:?} 的 {mode:?} 模式：空稿没有画出提示语"))
                }
                (_, None) => problems.push(format!("{name:?} 的 {mode:?} 模式：没找到正文那一行")),
            }
        }
    }
    theme::set_current(crate::models::ThemeName::default());
    theme::set_current_paper(crate::models::PaperMode::default());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// 敲字的人光标停在哪一行，页边那个号就该亮起来——这是投屏对稿真正省下的
/// 那一轮口舌：不必再念「我现在在第几行」，屏幕自己说了。亮的还必须正好是
/// 那一行的号：错一行，两边说的就不是同一处。
#[test]
fn the_number_of_the_line_under_the_cursor_lights_up() {
    /// 摊平嵌套的图形，取出主色实心牌子与正文那一行的主色底衬。
    fn collect(
        shape: &egui::epaint::Shape,
        chips: &mut Vec<egui::Rect>,
        bands: &mut Vec<egui::Rect>,
    ) {
        match shape {
            egui::epaint::Shape::Rect(rect) if rect.fill == theme::accent() => {
                chips.push(rect.rect);
            }
            egui::epaint::Shape::Rect(rect) if rect.fill == theme::accent_soft() => {
                bands.push(rect.rect);
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, chips, bands);
                }
            }
            _ => {}
        }
    }

    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    harness.config.show_preview_line_numbers = true;
    // 把光标放在正文第一节的标题行上，并让指针远离纸面，免得混进悬停高亮。
    let line = markdown::line_ranges(&harness.doc.generated_markdown)
        .into_iter()
        .find(|range| harness.doc.generated_markdown[range.clone()].starts_with("## "))
        .expect("样稿里应当有二级标题");
    harness.doc.preview_anchor = Some(PreviewAnchor {
        text: harness.doc.generated_markdown[line.clone()].to_string(),
        range: line,
    });
    let output = harness.preview_frames(egui::pos2(-50.0, -50.0), 3);

    let (mut chips, mut bands) = (Vec::new(), Vec::new());
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut chips, &mut bands);
    }
    // 右缘导航也用主色，按版心左沿把页边那一侧筛出来。
    let body_left = harness.title_block().left();
    chips.retain(|chip| chip.right() < body_left);
    assert_eq!(
        chips.len(),
        1,
        "光标停在一行上，页边只该亮这一行的牌子，实测亮了 {} 枚",
        chips.len()
    );
    assert!(!bands.is_empty(), "光标所在的那一行正文应当有底衬");
    let chip = chips[0].center();
    assert!(
        bands
            .iter()
            .any(|band| chip.y > band.top() && chip.y < band.bottom()),
        "亮起来的牌子应当与光标所在那一行齐平：牌子在 {chip:?}，底衬在 {bands:?}"
    );
}

/// 关掉之后页边必须彻底干净：一个号都不登记，点在原来有号的位置也不许跳转。
/// 这是"默认关、要时开"的前提——关不掉的开关等于没有这个开关。
#[test]
fn turning_paper_line_numbers_off_leaves_the_margin_empty() {
    let mut located = Harness::new();
    located.doc.preview_mode = PreviewMode::Rendered;
    located.config.show_preview_line_numbers = true;
    located.preview_frames(egui::pos2(10.0, 10.0), 3);
    let (_, rect) = located
        .paper_line_numbers()
        .into_iter()
        .find(|(number, _)| *number == 4)
        .expect("开着时应当画得出第 4 号");

    let mut harness = Harness::new();
    harness.doc.preview_mode = PreviewMode::Rendered;
    assert!(
        !harness.config.show_preview_line_numbers,
        "纸面行号默认应当是关着的"
    );
    harness.preview_frames(egui::pos2(10.0, 10.0), 3);
    assert!(
        harness.paper_line_numbers().is_empty(),
        "关掉后页边不该再登记任何号码"
    );

    harness.doc.pending_source_jump = None;
    harness.preview_click(rect.center());
    assert!(
        harness.doc.pending_source_jump.is_none(),
        "关掉后，点在原本有号的位置不该跳转"
    );
}

/// 拖动分隔条的过程中，版面缩放必须冻结在上一次落定的值上——
/// 这是"字号恒定、缓存全命中"的前提，也是不再顿挫的根本原因。
#[test]
fn layout_scale_is_frozen_while_width_changes() {
    let mut harness = Harness::new();
    // egui 要一两帧才量准滚动区，先热一下再取基准。
    harness.frame(900.0);
    harness.frame(900.0);
    let settled = harness.doc.preview_freeze.layout_scale;
    assert!(settled > 0.0, "首帧应当直接落定");

    // 连续改变宽度：排版缩放不许动。
    for i in 1..=20 {
        harness.frame(900.0 + i as f32 * 3.7);
        assert_eq!(
            harness.doc.preview_freeze.layout_scale, settled,
            "第 {i} 帧宽度仍在变，版面缩放不应重排"
        );
    }
    // 但"眼睛看到的倍率"必须一路跟着宽度连续走，否则就不是丝滑而是冻住。
    assert!(
        harness.doc.preview_fit_scale > settled,
        "显示倍率应随宽度增长，实际 {} vs {settled}",
        harness.doc.preview_fit_scale
    );
}

/// 宽度一停下来，下一帧就按精确倍率重排，文字恢复锐利。
#[test]
fn layout_settles_on_the_frame_after_motion_stops() {
    let mut harness = Harness::new();
    harness.frame(900.0);
    harness.frame(900.0);
    for i in 1..=10 {
        harness.frame(900.0 + i as f32 * 3.7);
    }
    let frozen = harness.doc.preview_freeze.layout_scale;
    let shown = harness.doc.preview_fit_scale;
    assert_ne!(frozen, shown, "拖动中两者本就该不同");

    // 松手：宽度不再变化。
    harness.frame(900.0 + 10.0 * 3.7);
    assert_eq!(
        harness.doc.preview_freeze.layout_scale, shown,
        "落定帧必须按精确倍率重排"
    );
}

/// 显示倍率与宽度之间保持严格连续——没有任何档位/台阶。
#[test]
fn displayed_scale_is_continuous_in_width() {
    let mut harness = Harness::new();
    harness.frame(900.0);
    harness.frame(900.0);
    let mut previous = None;
    // 亚像素步进：每一步都必须带来一个不同的、单调增长的显示倍率。
    for step in 0..40 {
        let width = 900.0 + step as f32 * 0.25;
        harness.frame(width);
        let shown = harness.doc.preview_fit_scale;
        if let Some(previous) = previous {
            assert!(
                shown > previous,
                "宽度 {width} 处显示倍率没有随宽度增长：{previous} → {shown}（出现了台阶）"
            );
        }
        previous = Some(shown);
    }
}

/// 最要紧的一条：拖动中间那些帧是用层变换画出来的，画面必须与"老老实实
/// 重排一遍"得到的画面重合——纸张一样大、一样居中。否则就是拿糊掉的画面
/// 换性能，不是丝滑。
#[test]
fn transformed_frame_matches_a_real_relayout() {
    let width = 1000.0;

    // 甲：正常落定（无变换）时纸张的位置与大小。
    let mut settled = Harness::new();
    settled.frame(width);
    settled.frame(width);
    let expected = settled.paper(width);

    // 乙：从别处拖过来、本帧仍在动，因而走层变换路径。
    let mut dragging = Harness::new();
    dragging.frame(width - 40.0);
    dragging.frame(width - 40.0);
    let frozen = dragging.doc.preview_freeze.layout_scale;
    let actual = dragging.paper(width);
    assert_ne!(
        dragging.doc.preview_freeze.layout_scale, 0.0,
        "应当仍冻结在旧倍率上"
    );
    assert_eq!(
        dragging.doc.preview_freeze.layout_scale, frozen,
        "这一帧宽度在变，不应重排"
    );

    // 半个像素以内即认为重合。
    assert!(
        (actual.width() - expected.width()).abs() < 0.5,
        "纸张宽度对不上：变换后 {:.2}，重排应为 {:.2}",
        actual.width(),
        expected.width()
    );
    // 高度不可能逐像素相等：换行是离散的，每行的 ascent/descent 又各自取整，
    // 所以 k·H(s) 与 H(k·s) 必然差一点点。要紧的是这点误差摊到"一屏"之内
    // 有多大——变换以可视区顶边为支点，误差随离支点的距离线性累积。
    let relative = (actual.height() - expected.height()).abs() / expected.height();
    assert!(
        relative < 0.005,
        "纸张高度相对误差 {:.3}% 偏大：变换后 {:.2}，重排应为 {:.2}",
        relative * 100.0,
        actual.height(),
        expected.height()
    );
    let drift_in_one_screen = relative * 900.0;
    assert!(
        drift_in_one_screen < 3.0,
        "一屏之内会漂 {drift_in_one_screen:.2} 像素，松手时看得出跳动"
    );
    assert!(
        (actual.center().x - expected.center().x).abs() < 0.5,
        "纸张没有居中：变换后中心 {:.2}，重排应为 {:.2}",
        actual.center().x,
        expected.center().x
    );
    assert!(
        (actual.top() - expected.top()).abs() < 0.5,
        "纸张顶边漂了：变换后 {:.2}，重排应为 {:.2}",
        actual.top(),
        expected.top()
    );
}

/// 跳变（切换显示方式、窗口最大化）不该走变换路径糊一帧，直接重排。
#[test]
fn large_jumps_relayout_immediately() {
    let mut harness = Harness::new();
    harness.frame(900.0);
    harness.frame(900.0);
    harness.frame(1400.0);
    assert_eq!(
        harness.doc.preview_freeze.layout_scale, harness.doc.preview_fit_scale,
        "一步到位的跳变应当立即按精确倍率重排"
    );
}

/// 拖动整段过程中：字体图集一次都不许重建，版面一次都不许重排。
///
/// 这两条是确定性的，也正是省下时间的因果本身——不拿墙钟时间做断言：
/// 机器负载一高就会误报，而它证明不了任何这两条之外的东西。真实耗时打出来
/// 供人看（本机实测：改动前 均值 3.56 / 最差 5.96 ms、重建 55 次；
/// 改动后 均值 0.44 / 最差 0.81 ms、重建 0 次）。
#[test]
fn dragging_neither_rebuilds_the_atlas_nor_relayouts() {
    let mut harness = Harness::new();
    // 先把图集喂饱并等它稳定：测量区间里不能再出现“前一帧刚好越过八成”
    // 这种与拖动无关的重建。
    harness.settle_font_atlas();
    let frozen = harness.doc.preview_freeze.layout_scale;

    let mut times = Vec::new();
    let mut rebuilds = 0;
    let mut relayouts = 0;
    for i in 0..120 {
        let (elapsed, rebuilt) = harness.frame(SETTLE_WIDTH + i as f32 * 3.7);
        times.push(elapsed);
        rebuilds += u32::from(rebuilt);
        if harness.doc.preview_freeze.layout_scale != frozen {
            relayouts += 1;
        }
    }
    let mean = times.iter().sum::<f32>() / times.len() as f32;
    let p50 = percentile(&mut times, 0.5);
    let p95 = percentile(&mut times, 0.95);
    let max = percentile(&mut times, 1.0);
    let (atlas, fill) = harness
        .ctx
        .fonts(|fonts| (fonts.font_image_size(), fonts.font_atlas_fill_ratio()));
    println!(
        "拖动 120 帧：均值 {mean:.2} / 中位 {p50:.2} / p95 {p95:.2} / 最差 {max:.2} ms，\
         图集重建 {rebuilds} 次，重排 {relayouts} 次，图集 {atlas:?} 填充 {:.0}%",
        fill * 100.0
    );

    assert_eq!(rebuilds, 0, "拖动期间字号恒定，字体图集不应重建");
    assert_eq!(relayouts, 0, "拖动期间版面缩放应始终冻结，一次都不该重排");
}
