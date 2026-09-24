//! 起草页的「版本对照」模式：左代码 diff（统一视图，本期只读）+ 右花脸稿预览。
//!
//! 方案见 `docs/version-diff-redesign.md` 第三节。两层 diff 各算各的：
//! - 左栏是 Markdown 源码的代码 diff（`diff::manuscript_diff`），删除行红底在上、
//!   新增行绿底在下，未改动区折叠；
//! - 右栏是花脸稿预览：视觉 diff 引擎产出的带哨兵 Markdown 直接交给公文预览
//!   （`preview::official_preview`），与导出 PDF / Word 吃的是同一份数据，
//!   「预览 = 导出」由构造保证。
//!
//! 两栏只通过源码字节范围互相定位（`version_link::ChangeLinks`）：点击互跳、
//! 悬停高亮、统一的上一处 / 下一处（F7 / Shift+F7）。

use crate::app::{DocJob, WorkerResult, version_hover};
use crate::diff;
use crate::diff_view;
use crate::draft_page::DraftPage;
use crate::models::DraftInput;
use crate::preview;
use crate::redline::{self, RedlineFormats};
use crate::theme;
use crate::units::UnitDisplay;
use crate::version_link::ChangeLinks;
use eframe::egui;
use std::ops::Range;
use std::path::PathBuf;
use std::thread;

/// 右栏预览要画的花脸稿，连同联动表。与左栏的代码 diff 同一次重算，
/// 两边永远对得上。
pub(crate) struct RedlineView {
    pub(crate) doc: redline::RedlineDoc,
    pub(crate) links: ChangeLinks,
}

/// 一帧里两栏交互收集下来、等借用结束后再落地的动作。
#[derive(Default)]
struct Actions {
    pick_base: Option<i64>,
    revert: bool,
    export: Option<RedlineFormats>,
    print_preview: bool,
    edit_source: Option<Range<usize>>,
}

impl DraftPage<'_> {
    /// 起草页的"版本对照"模式。
    pub(crate) fn version_diff_mode_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.doc.manuscript_id else {
            self.diff_placeholder(
                ui,
                "这篇稿件还没有保存到稿件库。先点“保存到稿件库”，再提交一个版本，才有可对照的基准。",
            );
            return;
        };
        let versions = self.draft_version_rows();
        if versions.is_empty() {
            self.diff_placeholder(
                ui,
                "这篇稿件还没有提交过版本。点“提交版本”固化第一版之后，这里会逐字标出后续修改。",
            );
            return;
        }
        let latest = versions.last().map_or(1, |row| row.version_number);
        // 基准默认跟着最新版走：提交一版之后对照自动以新版为基准，不用手动换选。
        let base = self
            .doc
            .draft_diff
            .base
            .filter(|number| versions.iter().any(|row| row.version_number == *number))
            .unwrap_or(latest);
        self.sync_draft_diff(id, base);

        let busy = self.doc.busy;
        let mut actions = Actions::default();
        // 按字段拆开借用：代码 diff 结果只读，视图状态与联动状态可写，互不冲突。
        let super::DraftDiffState {
            cache,
            view: diff_state,
            redline: redline_view,
            preview_scroll,
            preview_target,
            preview_hover,
            ..
        } = &mut self.doc.draft_diff;
        let Some((_, report)) = cache.as_ref() else {
            return;
        };
        let total = report.body.changed_count;

        // —— 统一导航：F7 下一处、Shift+F7 上一处，两栏一起跳 ——
        let (next, previous) = ui.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, egui::Key::F7),
                input.consume_key(egui::Modifiers::SHIFT, egui::Key::F7),
            )
        });
        let mut step: Option<bool> = next.then_some(true).or(previous.then_some(false));

        // —— 工具栏 ——
        ui.horizontal_wrapped(|ui| {
            ui.label("基准");
            let label = |row: &crate::manuscript::VersionRow| {
                format!(
                    "v{} · {}{}",
                    row.version_number,
                    row.name,
                    if row.is_latest { " · 最新" } else { "" }
                )
            };
            let base_label = versions
                .iter()
                .find(|row| row.version_number == base)
                .map_or_else(|| format!("v{base}"), label);
            egui::ComboBox::from_id_salt("draft_diff_base")
                .selected_text(base_label)
                .width(200.0)
                .show_ui(ui, |ui| {
                    for row in versions.iter().rev() {
                        if ui
                            .selectable_label(row.version_number == base, label(row))
                            .on_hover_text(version_hover(row))
                            .clicked()
                        {
                            actions.pick_base = Some(row.version_number);
                        }
                    }
                })
                .response
                .on_hover_text("换成更早的版本，可以看到从那一版至今累计改了什么");
            ui.label("→");
            ui.strong("当前未提交");
            ui.separator();
            theme::chip(
                ui,
                &format!("共 {} 处变更", report.total()),
                theme::accent(),
                theme::accent_soft(),
            );
            if total > 0 {
                ui.label(
                    egui::RichText::new(format!("第 {} / {total} 处", diff_state.focus() + 1))
                        .color(theme::text_muted()),
                );
                if theme::icon_button(ui, theme::Icon::ArrowUp, "上一处（Shift+F7）").clicked()
                {
                    step = Some(false);
                }
                if theme::icon_button(ui, theme::Icon::ArrowDown, "下一处（F7）").clicked() {
                    step = Some(true);
                }
            }
            ui.checkbox(&mut diff_state.only_changes, "折叠未改动")
                .on_hover_text("关掉后左栏未改动的段落也全量显示；右栏预览始终是全文");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::RotateCcw,
                        "回退到基准版",
                    ))
                    .on_hover_text("用基准版内容覆盖当前稿件内容（需二次确认）")
                    .clicked()
                {
                    actions.revert = true;
                }
                let has_marks = redline_view
                    .as_ref()
                    .is_some_and(|view| !view.doc.is_empty());
                ui.add_enabled_ui(has_marks && !busy, |ui| {
                    ui.menu_button("导出花脸稿", |ui| {
                        for (icon, label, formats) in [
                            (theme::Icon::FileTypePdf, "导出 PDF", (true, false)),
                            (theme::Icon::FileTypeDoc, "导出 Word", (false, true)),
                            (theme::Icon::Package, "两者都导", (true, true)),
                        ] {
                            if ui.add(theme::menu_item(icon, label)).clicked() {
                                actions.export = Some(RedlineFormats {
                                    pdf: formats.0,
                                    docx: formats.1,
                                });
                                ui.close();
                            }
                        }
                    });
                    if ui
                        .add(theme::icon_text_button(theme::Icon::Print, "打印预览"))
                        .on_hover_text(
                            "后台真编译一遍 PDF，在内置查看器里看真实纸面；\
                             左右预览的换行与分页不保证与纸面逐行相同",
                        )
                        .clicked()
                    {
                        actions.print_preview = true;
                    }
                });
            });
        });
        ui.separator();
        if let Some(forward) = step {
            diff_state.step(forward, total);
            *preview_scroll = true;
            *preview_target = None;
        }

        // —— 左栏：要素变化 + 代码 diff ——
        let old_label = format!("v{base}");
        let mut left = diff_view::UnifiedOutput::default();
        let hover_from_preview = *preview_hover;
        egui::Panel::left("version_diff_code")
            .default_size(ui.available_width() * 0.45)
            .size_range(280.0..=1400.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                right: 8,
                ..egui::Margin::ZERO
            }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("version_diff_code_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if report.is_empty() {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| {
                                ui.weak(format!("当前内容与 {old_label} 一致，没有差异。"));
                            });
                            return;
                        }
                        if !report.fields.is_empty() {
                            egui::CollapsingHeader::new(format!(
                                "要素变化（{} 项）",
                                report.fields.len()
                            ))
                            .default_open(true)
                            .show(ui, |ui| {
                                diff_view::field_changes_table(
                                    ui,
                                    &report.fields,
                                    &old_label,
                                    "当前",
                                )
                            });
                            ui.add_space(4.0);
                        }
                        if let Some(notes) = &report.notes {
                            egui::CollapsingHeader::new("备注变化")
                                .default_open(false)
                                .show(ui, |ui| {
                                    diff_view::field_changes_table(
                                        ui,
                                        std::slice::from_ref(notes),
                                        &old_label,
                                        "当前",
                                    )
                                });
                            ui.add_space(4.0);
                        }
                        left = diff_view::unified_body_ui(
                            ui,
                            &report.body,
                            diff_state,
                            hover_from_preview,
                        );
                    });
            });

        // 左栏的点击先落地：右栏本帧就能按新的焦点画锚点。
        if let Some(change) = left.clicked_change {
            diff_state.set_focus(change, false);
            *preview_scroll = true;
            *preview_target = None;
        }
        if let Some(source) = &left.clicked_context
            && let Some(view) = redline_view.as_ref()
        {
            *preview_target = view.links.marked_for_new_source(source);
            *preview_scroll = preview_target.is_some();
        }
        actions.edit_source = left.edit_source.clone().or_else(|| {
            let view = redline_view.as_ref()?;
            view.links.new_source(left.edit_change?)
        });

        // —— 右栏：花脸稿预览 ——
        let Some(view) = redline_view.as_ref() else {
            return;
        };
        // 锚点优先级：左栏悬停 > 点过的未改动块 > 当前焦点。
        let anchor = left
            .hovered_change
            .and_then(|change| view.links.marked_range(change))
            .or_else(|| preview_target.clone())
            .or_else(|| {
                (total > 0)
                    .then(|| view.links.marked_range(diff_state.focus()))
                    .flatten()
            });
        let scroll = std::mem::take(preview_scroll) && left.hovered_change.is_none();
        let display = UnitDisplay::new(&self.config.vocabulary);
        let mut output = None;
        let mut hovered = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt("version_diff_preview_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        output = Some(preview::official_preview(
                            ui,
                            &self.doc.draft,
                            &display,
                            &view.doc.markdown,
                            preview::PreviewScale::zoom(None),
                            anchor.as_ref(),
                            scroll,
                            &self.config.numbering,
                            false,
                        ));
                        hovered = preview::hovered_source(ui.ctx());
                        ui.add_space(12.0);
                    });
            });

        // 右栏的悬停与点击：换算成代码变更，下一帧左栏描边 / 滚动。
        let hover_change = hovered
            .as_ref()
            .and_then(|range| view.links.change_at(range));
        if hover_change != *preview_hover {
            *preview_hover = hover_change;
            ui.ctx().request_repaint();
        }
        if let Some(clicked) = output.and_then(|output| output.clicked) {
            match view.links.change_at(&clicked) {
                Some(change) => {
                    diff_state.set_focus(change, true);
                    *preview_target = None;
                }
                // 点在没改动的块上：只在预览里标亮它，左栏不动。
                None => *preview_target = Some(clicked),
            }
            ui.ctx().request_repaint();
        }

        // —— 借用结束，落地动作 ——
        if let Some(number) = actions.pick_base {
            self.doc.draft_diff.base = Some(number);
            self.doc.draft_diff.view.reset();
            self.doc.draft_diff.preview_target = None;
        }
        if actions.revert {
            *self.revert_confirm = Some((id, base));
        }
        if let Some(formats) = actions.export {
            self.export_redline(formats, false);
        }
        if actions.print_preview {
            self.export_redline(
                RedlineFormats {
                    pdf: true,
                    docx: false,
                },
                true,
            );
        }
        if let Some(range) = actions.edit_source {
            self.jump_to_source(range);
        }
    }

    /// 导出花脸稿：基准版 → 当前修订，删掉的画红色删除线、新增的套蓝框。
    ///
    /// 与定稿导出一样放后台线程——PDF 要真编译一遍，同步做会把界面卡住几秒。
    /// `print_preview` 为 true 时只出 PDF、写进临时目录，编完在内置查看器里打开，
    /// 不在导出目录里留文件（方案需求结论第 20 条「打印预览」）。
    fn export_redline(&mut self, formats: RedlineFormats, print_preview: bool) {
        let Some(view) = &self.doc.draft_diff.redline else {
            return;
        };
        if view.doc.is_empty() {
            *self.status = "当前修订与基准版一致，没有可出的花脸稿。".into();
            return;
        }
        let doc = view.doc.clone();
        let (key, seq) = self.begin_job();
        *self.status = if print_preview {
            "正在编译花脸稿打印预览…".into()
        } else {
            "正在生成花脸稿…".into()
        };
        let input = self.doc.draft.clone();
        let output_dir = if print_preview {
            std::env::temp_dir().join("gongwen-redline-preview")
        } else {
            PathBuf::from(&self.config.output_dir)
        };
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let numbering = self.config.numbering;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let display = UnitDisplay::new(&vocabulary);
            // 与定稿导出同样先落实字体：TeX 里写死按哪个文件加载，等编译时才
            // 发现缺文件就来不及退回内置字体了。
            let (fonts, _warnings) = crate::system_fonts::resolve(&fonts);
            let result = redline::export_files(
                &output_dir,
                &input,
                &doc,
                formats,
                &display,
                &fonts,
                &numbering,
            )
            .map_err(|error| format!("{error:#}"));
            let job = if print_preview {
                DocJob::RedlinePrintPreview(result)
            } else {
                DocJob::RedlineExported(result)
            };
            let _ = tx.send(WorkerResult::Doc { key, seq, job });
        });
    }

    /// 版本对照模式下的空状态提示。
    pub(crate) fn diff_placeholder(&self, ui: &mut egui::Ui, message: &str) {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(message).color(theme::text_muted()))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
    }

    /// 重算起草页对照结果，输入没变就复用上一帧的。这个模式每帧都要渲染，
    /// 长稿边打字边全量 diff 会掉帧。代码 diff 与花脸稿同一次重算，
    /// 两栏的联动表因此永远对得上。
    pub(crate) fn sync_draft_diff(&mut self, id: i64, base: i64) {
        let notes = self
            .store
            .as_deref()
            .and_then(|store| store.notes_of(id).ok())
            .flatten()
            .unwrap_or_default();
        let draft_json = serde_json::to_string(&self.doc.draft).unwrap_or_default();
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::hash::DefaultHasher::new();
            id.hash(&mut hasher);
            base.hash(&mut hasher);
            self.doc.generated_markdown.hash(&mut hasher);
            notes.hash(&mut hasher);
            draft_json.hash(&mut hasher);
            hasher.finish()
        };
        if self
            .doc
            .draft_diff
            .cache
            .as_ref()
            .is_some_and(|(cached, _)| *cached == key)
        {
            return;
        }
        let old = self
            .store
            .as_deref_mut()
            .and_then(|store| store.get_manuscript_version(id, base).ok())
            .flatten()
            .map(diff::ContentSnapshot::from)
            .unwrap_or_else(|| {
                diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
            });
        let new = diff::ContentSnapshot::new(
            self.doc.draft.clone(),
            self.doc.generated_markdown.clone(),
            notes,
        );
        let report = diff::manuscript_diff(&old, &new);
        let doc = redline::build(&old.content_markdown, &new.content_markdown);
        let links = ChangeLinks::new(&report.body, &old.content_markdown, &doc.spans);
        self.doc.draft_diff.cache = Some((key, report));
        self.doc.draft_diff.redline = Some(RedlineView { doc, links });
        self.doc.draft_diff.preview_target = None;
        self.doc.draft_diff.preview_hover = None;
    }
}

#[cfg(test)]
mod tests {
    //! 版本对照页的端到端检查：真库（内存）、真 egui 上下文，画几帧看两栏
    //! 是否都画出来、联动是否落到同一处变更。

    use super::*;
    use crate::app::{VersionSwitchPrompt, WorkerResult};
    use crate::draft_page::{DraftAction, DraftSession, ExportLinks};
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    use crate::models::{AppConfig, ManuscriptStatus};
    use std::path::Path;
    use std::sync::mpsc::{Receiver, Sender};

    const OLD: &str = "# 关于报送材料的函\n\n## 工作目标\n\n请于八月十日前报送材料。\n\n各单位要高度重视。\n\n## 工作要求\n\n多余的一段话。\n\n本通知自印发之日起施行。";
    const NEW: &str = "# 关于报送材料的函\n\n## 工作目标\n\n请于八月十五日前报送材料。\n\n各单位要高度重视。\n\n## 工作要求\n\n本通知自印发之日起施行。\n\n## 保障措施\n\n新增的保障措施。";

    struct Harness {
        ctx: egui::Context,
        doc: DraftSession,
        config: AppConfig,
        store: ManuscriptStore,
        sender: Sender<WorkerResult>,
        status: String,
        version_switch: Option<VersionSwitchPrompt>,
        revert_confirm: Option<(i64, i64)>,
        actions: Vec<DraftAction>,
        export_links: ExportLinks,
        metrics: crate::metrics::Metrics,
        _keep: Receiver<WorkerResult>,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::configure_icons(&ctx);
            theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
            let config = AppConfig::default();
            let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
            let snapshot = DraftInput::default();
            let id = store
                .create(
                    &NewManuscript {
                        snapshot: snapshot.clone(),
                        content_markdown: OLD.into(),
                        notes: String::new(),
                        status: ManuscriptStatus::Draft,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            store
                .commit_manuscript_version(id, "送审稿", "", &snapshot, OLD, "")
                .unwrap();
            let mut doc = DraftSession::with_markdown(1, &config, NEW.into());
            doc.manuscript_id = Some(id);
            doc.draft = snapshot;
            let (sender, _keep) = std::sync::mpsc::channel();
            Self {
                ctx,
                doc,
                config,
                store,
                sender,
                status: String::new(),
                version_switch: None,
                revert_confirm: None,
                actions: Vec::new(),
                export_links: ExportLinks::default(),
                metrics: crate::metrics::Metrics::default(),
                _keep,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                events,
                ..Default::default()
            };
            self.ctx.clone().run_ui(raw, |ui| {
                let mut page = DraftPage {
                    doc: &mut self.doc,
                    config: &mut self.config,
                    store: Some(&mut self.store),
                    sender: &self.sender,
                    status: &mut self.status,
                    version_switch: &mut self.version_switch,
                    revert_confirm: &mut self.revert_confirm,
                    actions: &mut self.actions,
                    export_links: &mut self.export_links,
                    metrics: &mut self.metrics,
                };
                page.version_diff_mode_ui(ui);
            })
        }
    }

    fn texts(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => Some(shape.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn both_panes_render_and_share_one_change_list() {
        let mut harness = Harness::new();
        let output = harness.frame(Vec::new());
        let text = texts(&output);
        // 左栏：代码 diff 的删 / 增两行都在，折叠行也在。
        assert!(text.contains("请于八月十日前报送材料。"), "{text}");
        assert!(text.contains("请于八月十五日前报送材料。"), "{text}");
        assert!(text.contains("多余的一段话。"), "{text}");
        // 右栏：花脸稿预览画出了新增框，哨兵没漏到纸上。
        let blue = output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::epaint::Shape::LineSegment { stroke, .. }
                if stroke.color == preview::marks::ADD_COLOR)
        });
        assert!(blue, "预览应画出新增框");
        assert!(
            !text.chars().any(crate::export::is_redline_sentinel),
            "{text}"
        );
        let view = harness
            .doc
            .draft_diff
            .redline
            .as_ref()
            .expect("花脸稿已算好");
        let (_, report) = harness.doc.draft_diff.cache.as_ref().unwrap();
        // 每处代码变更都能在花脸稿里找到位置。
        for change in 0..report.body.changed_count {
            assert!(
                view.links.marked_range(change).is_some(),
                "第 {change} 处变更在预览里没有落点"
            );
        }
    }

    #[test]
    fn f7_steps_through_changes() {
        let mut harness = Harness::new();
        harness.frame(Vec::new());
        assert_eq!(harness.doc.draft_diff.view.focus(), 0);
        let key = |modifiers| egui::Event::Key {
            key: egui::Key::F7,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        harness.frame(vec![key(egui::Modifiers::NONE)]);
        assert_eq!(harness.doc.draft_diff.view.focus(), 1);
        harness.frame(vec![key(egui::Modifiers::SHIFT)]);
        harness.frame(vec![key(egui::Modifiers::SHIFT)]);
        let total = harness
            .doc
            .draft_diff
            .cache
            .as_ref()
            .unwrap()
            .1
            .body
            .changed_count;
        assert_eq!(harness.doc.draft_diff.view.focus(), total - 1, "首尾循环");
    }
}
