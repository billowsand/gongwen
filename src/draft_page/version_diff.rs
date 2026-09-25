//! 起草页的「版本对照」模式：可编辑稿件走可还原 diff；已发布 / 归档稿件委托给
//! `version_pair_view`，与稿件管理共用只读历史版本对照。
//!
//! 可编辑路径沿用两层 diff：左栏是 Markdown 源码代码 diff（`diff::manuscript_diff`）与
//! `diff_editor`，右栏把视觉 diff 引擎产出的带哨兵 Markdown 交给 `preview::official_preview`。
//! 花脸稿要跑 jieba，可编辑路径首帧同步计算，后续后台线程防抖；历史版本不可变，
//! 由共享组件按版本对同步计算并缓存。
//!
//! 两栏只通过源码字节范围互相定位（`version_link::ChangeLinks`）：点击互跳、
//! 悬停高亮、统一的上一处 / 下一处（F7 / Shift+F7）。

use super::diff_editor::{self, DiffEditorInput};
use super::diff_hunks;
use crate::app::{DocJob, WorkerResult, version_hover};
use crate::diff;
use crate::diff_view;
use crate::draft_page::DraftPage;
use crate::models::DraftInput;
use crate::preview;
use crate::redline::{self, RedlineFormats};
use crate::theme;
use crate::units::UnitDisplay;
use crate::version_link::{ChangeLinks, line_starts};
use eframe::egui;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// 花脸稿重算的防抖：停手这么久之后才发后台任务。
const REDLINE_DEBOUNCE: f64 = 0.15;

/// 右栏预览要画的花脸稿，连同联动表。
pub(crate) struct RedlineView {
    pub(crate) doc: redline::RedlineDoc,
    pub(crate) links: ChangeLinks,
    /// 这份花脸稿对应的正文哈希；与当前正文不符说明它已过期，后台正在重算。
    pub(crate) hash: u64,
}

/// 对照基准：换稿件 / 换基准版时从库里取一次，之后每帧不再读库。
pub(crate) struct Baseline {
    id: i64,
    number: i64,
    pub(crate) snapshot: diff::ContentSnapshot,
    /// 当前稿件的备注（备注不在起草页里改，取一次即可）。
    notes: String,
}

impl Baseline {
    /// `(稿件, 基准版)`：后台花脸稿据此判断是不是对着当前基准算的。
    fn key(&self) -> (i64, i64) {
        (self.id, self.number)
    }
}

/// 一帧里两栏交互收集下来、等借用结束后再落地的动作。
#[derive(Default)]
struct Actions {
    pick_base: Option<i64>,
    revert_all: bool,
    revert_hunk: Option<usize>,
    export: Option<RedlineFormats>,
    print_preview: bool,
}

/// 正文哈希：代码 diff 与花脸稿都按它判断是否过期。
fn text_hash(text: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// 新文本第 `line` 行（0 基）行首的字节位置。
fn line_offset(text: &str, line: usize) -> usize {
    line_starts(text).get(line).copied().unwrap_or(text.len())
}

impl super::DraftDiffState {
    /// 后台算好的花脸稿回来了：只收与当前稿件、基准版和正文都对得上的那一份，
    /// 过期的直接丢。任务在途时换了基准版而正文没动，正文哈希照样对得上，
    /// 不核对基准版就会把旧基准的花脸稿收下，而且之后哈希一致、再不重算。
    pub(crate) fn accept_redline(&mut self, base: (i64, i64), hash: u64, doc: redline::RedlineDoc) {
        if self.redline_in_flight == Some((base, hash)) {
            self.redline_in_flight = None;
        }
        let current = self.baseline.as_ref().map(Baseline::key);
        if current != Some(base) || hash != self.text_hash {
            return;
        }
        let (Some((_, report)), Some(baseline)) = (&self.cache, &self.baseline) else {
            return;
        };
        let links = ChangeLinks::new(
            &report.body,
            &baseline.snapshot.content_markdown,
            &doc.spans,
        );
        self.redline = Some(RedlineView { doc, links, hash });
    }

    /// 第 `change` 处逐行变更属于哪个可编辑变更块。
    fn unit_of_change(&self, change: usize) -> Option<usize> {
        self.hunks
            .iter()
            .position(|hunk| hunk.changes.contains(&change))
    }

    /// 可编辑变更块在花脸稿里的范围（右栏锚点）。
    fn marked_of_unit(&self, unit: usize) -> Option<Range<usize>> {
        let view = self.redline.as_ref()?;
        let hunk = self.hunks.get(unit)?;
        hunk.changes
            .clone()
            .filter_map(|change| view.links.marked_range(change))
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
    }
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
        if self.doc.read_only() {
            self.read_only_version_pair_mode_ui(ui, id, base, latest, &versions);
            return;
        }
        self.sync_draft_diff(ui.ctx(), id, base);

        let busy = self.doc.busy;
        let mut actions = Actions::default();
        let total = self.doc.draft_diff.hunks.len();

        // —— 统一导航：F7 下一处、Shift+F7 上一处，两栏一起跳 ——
        // 先认 Shift+F7：egui 的 `consume_key` 按「逻辑上匹配」比修饰键，不带 Shift
        // 的那一条也会吃掉 Shift+F7，先查它「上一处」就永远变成「下一处」。
        let (previous, next) = ui.input_mut(|input| {
            let previous = input.consume_key(egui::Modifiers::SHIFT, egui::Key::F7);
            (
                previous,
                !previous && input.consume_key(egui::Modifiers::NONE, egui::Key::F7),
            )
        });
        let mut step: Option<bool> = next.then_some(true).or(previous.then_some(false));

        // —— 工具栏 ——
        let state = &mut self.doc.draft_diff;
        let Some((_, report)) = state.cache.as_ref() else {
            return;
        };
        let report_total = report.total();
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
                &format!("共 {report_total} 处变更"),
                theme::accent(),
                theme::accent_soft(),
            );
            if total > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "第 {} / {total} 块",
                        state.view.focus().min(total - 1) + 1
                    ))
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::RotateCcw,
                        "回退到基准版",
                    ))
                    .on_hover_text("用基准版内容覆盖当前稿件内容（需二次确认）")
                    .clicked()
                {
                    actions.revert_all = true;
                }
                let has_marks = state
                    .redline
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
            state.view.step(forward, total);
            state.preview_scroll = true;
            state.preview_target = None;
            if let Some(hunk) = state.hunks.get(state.view.focus()) {
                state.editor_jump =
                    Some(line_offset(&self.doc.generated_markdown, hunk.first_line()));
            }
        }

        // —— 左栏：要素变化 + 代码 diff（可编辑 / 只读）——
        let old_label = format!("v{base}");
        let focus = (total > 0).then(|| state.view.focus().min(total - 1));
        let hover_from_preview = state.preview_hover;
        let jump = state.editor_jump.take();
        let mut left_hover: Option<usize> = None;
        let mut editor = diff_editor::DiffEditorOutput::default();
        let editor_font_size = self.config.editor_font_size.clamp(
            crate::models::EDITOR_FONT_SIZE_MIN,
            crate::models::EDITOR_FONT_SIZE_MAX,
        );
        let research = self.doc.draft.kind.is_research();
        let editor_fonts = self.config.editor_fonts;
        let text = &mut self.doc.generated_markdown;
        let highlighter = &mut self.doc.highlighter;
        let state = &mut self.doc.draft_diff;
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
                        let Some((_, report)) = state.cache.as_ref() else {
                            return;
                        };
                        if report.is_empty() {
                            ui.weak(format!("当前内容与 {old_label} 一致，没有差异。"));
                            ui.add_space(4.0);
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
                        editor = diff_editor::diff_editor(
                            ui,
                            text,
                            highlighter,
                            DiffEditorInput {
                                hunks: &state.hunks,
                                focus,
                                highlight: hover_from_preview,
                                editable: true,
                                font_size: editor_font_size,
                                fonts: &editor_fonts,
                                research,
                            },
                        );
                        // 光标跳转必须在滚动区里做：它要把目标行滚进这块滚动区。
                        if let (Some(offset), Some(output)) = (jump, &editor.output) {
                            crate::draft_page::jump_to_source(ui, output, text, offset);
                        }
                        left_hover = editor.hovered;
                    });
            });

        // 左栏的交互先落地：右栏本帧就能按新的焦点画锚点。
        if editor.changed {
            ui.ctx().request_repaint();
        }
        let cursor_moved =
            editor.cursor_line.is_some() && editor.cursor_line != state.editor_cursor_line;
        if editor.cursor_line.is_some() {
            state.editor_cursor_line = editor.cursor_line;
        }
        if cursor_moved
            && let Some(line) = editor.cursor_line
            && let Some(hunk) = state.hunks.iter().position(|hunk| hunk.touches_line(line))
            && focus != Some(hunk)
        {
            // 编辑器里光标所在的变更块就是当前焦点，右栏跟着滚过去。
            state.view.set_focus(hunk, false);
            state.preview_scroll = true;
            state.preview_target = None;
        }
        actions.revert_hunk = editor.revert;

        // —— 右栏：花脸稿预览 ——
        let focus = (total > 0).then(|| state.view.focus().min(total - 1));
        // 锚点优先级：左栏悬停 > 点过的未改动块 > 当前焦点。
        let anchor = left_hover
            .and_then(|unit| state.marked_of_unit(unit))
            .or_else(|| state.preview_target.clone())
            .or_else(|| focus.and_then(|unit| state.marked_of_unit(unit)));
        let scroll = std::mem::take(&mut state.preview_scroll) && left_hover.is_none();
        let Some(view) = state.redline.as_ref() else {
            return;
        };
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
                            &view.doc.elements,
                        ));
                        hovered = preview::hovered_source(ui.ctx());
                        ui.add_space(12.0);
                    });
            });

        // 右栏的悬停与点击：换算成导航单位，下一帧左栏描边 / 跳光标。
        let hover_unit = hovered
            .as_ref()
            .and_then(|range| view.links.change_at(range))
            .and_then(|change| state.unit_of_change(change));
        if hover_unit != state.preview_hover {
            state.preview_hover = hover_unit;
            ui.ctx().request_repaint();
        }
        if let Some(clicked) = output.and_then(|output| output.clicked) {
            let unit = view
                .links
                .change_at(&clicked)
                .and_then(|change| state.unit_of_change(change));
            match unit {
                Some(unit) => {
                    state.view.set_focus(unit, true);
                    state.preview_target = None;
                    if let Some(hunk) = state.hunks.get(unit) {
                        state.editor_jump =
                            Some(line_offset(&self.doc.generated_markdown, hunk.first_line()));
                    }
                }
                // 点在没改动的块上：只在预览里标亮它，左栏不动。
                None => state.preview_target = Some(clicked),
            }
            ui.ctx().request_repaint();
        }

        // —— 借用结束，落地动作 ——
        if let Some(number) = actions.pick_base {
            self.doc.draft_diff.base = Some(number);
            self.doc.draft_diff.view.reset();
            self.doc.draft_diff.preview_target = None;
        }
        if actions.revert_all {
            *self.revert_confirm = Some((id, base));
        }
        if let Some(index) = actions.revert_hunk {
            self.revert_hunk(ui.ctx(), index);
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
    }

    /// 已发布 / 归档稿件只读：用共享历史版本组件代替起草页专用渲染分支。
    fn read_only_version_pair_mode_ui(
        &mut self,
        ui: &mut egui::Ui,
        manuscript_id: i64,
        base: i64,
        latest: i64,
        versions: &[crate::manuscript::VersionRow],
    ) {
        let new_version = self
            .doc
            .loaded_version
            .as_ref()
            .filter(|loaded| loaded.manuscript_id == manuscript_id)
            .map_or(latest, |loaded| loaded.version_number);
        let key = crate::version_pair_view::VersionPairKey {
            manuscript_id,
            old_version_number: Some(base),
            new_version_number: new_version,
        };
        if !self.doc.draft_diff.version_pair.matches(key) {
            let old = self
                .store
                .as_deref_mut()
                .and_then(|store| store.get_manuscript_version(manuscript_id, base).ok())
                .flatten()
                .map(diff::ContentSnapshot::from)
                .unwrap_or_else(|| {
                    diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
                });
            let notes = self
                .store
                .as_deref()
                .and_then(|store| store.notes_of(manuscript_id).ok())
                .flatten()
                .unwrap_or_default();
            let new = diff::ContentSnapshot::new(
                self.doc.draft.clone(),
                self.doc.generated_markdown.clone(),
                notes,
            );
            let display = UnitDisplay::new(&self.config.vocabulary);
            self.doc
                .draft_diff
                .version_pair
                .set_pair(key, &old, &new, &display);
        }

        let old_label = format!("v{base}");
        let new_label = "当前";
        let state = &mut self.doc.draft_diff.version_pair;
        let busy = self.doc.busy;
        let has_marks = state.redline().is_some_and(|doc| !doc.is_empty());
        let mut picked_base = None;
        let mut export = None;
        let mut print_preview = false;

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
                            picked_base = Some(row.version_number);
                        }
                    }
                })
                .response
                .on_hover_text("选择累计对照的旧版");
            ui.label("→");
            ui.strong("当前未提交");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled(
                    false,
                    theme::icon_text_button(theme::Icon::RotateCcw, "回退到基准版"),
                );
                ui.add_enabled_ui(has_marks && !busy, |ui| {
                    ui.menu_button("导出花脸稿", |ui| {
                        for (icon, label, formats) in [
                            (theme::Icon::FileTypePdf, "导出 PDF", (true, false)),
                            (theme::Icon::FileTypeDoc, "导出 Word", (false, true)),
                            (theme::Icon::Package, "两者都导", (true, true)),
                        ] {
                            if ui.add(theme::menu_item(icon, label)).clicked() {
                                export = Some(RedlineFormats {
                                    pdf: formats.0,
                                    docx: formats.1,
                                });
                                ui.close();
                            }
                        }
                    });
                    if ui
                        .add(theme::icon_text_button(theme::Icon::Print, "打印预览"))
                        .clicked()
                    {
                        print_preview = true;
                    }
                });
            });
        });

        let display = UnitDisplay::new(&self.config.vocabulary);
        let jump = state.show(
            ui,
            egui::Id::new("version_diff_code"),
            &old_label,
            new_label,
            &display,
            &self.config.numbering,
        );
        if let Some(range) = jump {
            self.jump_to_source(range);
        }
        if let Some(number) = picked_base {
            self.doc.draft_diff.base = Some(number);
            ui.ctx().request_repaint();
        }
        if (export.is_some() || print_preview)
            && let Some(doc) = self.doc.draft_diff.version_pair.redline().cloned()
        {
            let formats = export.unwrap_or(RedlineFormats {
                pdf: true,
                docx: false,
            });
            self.export_redline_document(doc, self.doc.draft.clone(), formats, print_preview);
        }
    }
    /// 逐块还原：把第 `index` 个变更块换回基准版的内容。前后各记一个撤销点，
    /// Ctrl+Z 一次就回到还原前；光标落在被还原块的起点。
    fn revert_hunk(&mut self, ctx: &egui::Context, index: usize) {
        let state = &self.doc.draft_diff;
        let (Some(baseline), Some(hunk)) = (&state.baseline, state.hunks.get(index)) else {
            return;
        };
        let (reverted, cursor) = diff_hunks::revert(
            hunk,
            &baseline.snapshot.content_markdown,
            &self.doc.generated_markdown,
        );
        diff_editor::replace_with_undo(ctx, &mut self.doc.generated_markdown, reverted, cursor);
        self.doc.draft_diff.editor_jump = Some(cursor);
        self.doc.draft_diff.preview_scroll = true;
        *self.status = "已还原这一块（Ctrl+Z 可撤回）。".into();
        ctx.request_repaint();
    }

    /// 导出花脸稿：基准版 → 当前修订，删掉的画红色删除线、新增的套蓝框。
    ///
    /// 与定稿导出一样放后台线程——PDF 要真编译一遍，同步做会把界面卡住几秒。
    /// `print_preview` 为 true 时只出 PDF、写进临时目录，编完在内置查看器里打开，
    /// 不在导出目录里留文件（方案需求结论第 20 条「打印预览」）。
    fn export_redline(&mut self, formats: RedlineFormats, print_preview: bool) {
        let state = &self.doc.draft_diff;
        let Some(view) = &state.redline else {
            return;
        };
        // 右栏的花脸稿可能比正文晚一拍（后台防抖中）；导出必须用最新的正文。
        // 就地重算的一份同样带上新旧 DraftInput，要素标注跟着进导出。
        let doc = if view.hash == state.text_hash {
            view.doc.clone()
        } else {
            let fallback = DraftInput::default();
            let (old, old_input) = state.baseline.as_ref().map_or(("", &fallback), |baseline| {
                (
                    baseline.snapshot.content_markdown.as_str(),
                    &baseline.snapshot.snapshot,
                )
            });
            let display = UnitDisplay::new(&self.config.vocabulary);
            redline::build_with_inputs(
                old,
                &self.doc.generated_markdown,
                old_input,
                &self.doc.draft,
                &display,
            )
        };
        let input = self.doc.draft.clone();
        self.export_redline_document(doc, input, formats, print_preview);
    }

    fn export_redline_document(
        &mut self,
        doc: redline::RedlineDoc,
        input: DraftInput,
        formats: RedlineFormats,
        print_preview: bool,
    ) {
        if doc.is_empty() {
            *self.status = "当前修订与基准版一致，没有可出的花脸稿。".into();
            return;
        }
        let (key, seq) = self.begin_job();
        *self.status = if print_preview {
            "正在编译花脸稿打印预览…".into()
        } else {
            "正在生成花脸稿…".into()
        };
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

    /// 让对照结果跟上当前内容：
    /// - 基准快照与备注只在换稿件 / 换基准版时读一次库；
    /// - 代码 diff（便宜）在正文或要素变了的那一帧同步重算；
    /// - 花脸稿（要跑 jieba）第一次同步算，之后放后台线程、停手 150ms 再算，
    ///   结果带正文哈希回来，过期的丢掉（见 [`super::DraftDiffState::accept_redline`]）。
    pub(crate) fn sync_draft_diff(&mut self, ctx: &egui::Context, id: i64, base: i64) {
        let fresh_base = !self
            .doc
            .draft_diff
            .baseline
            .as_ref()
            .is_some_and(|baseline| baseline.id == id && baseline.number == base);
        if fresh_base {
            let snapshot = self
                .store
                .as_deref_mut()
                .and_then(|store| store.get_manuscript_version(id, base).ok())
                .flatten()
                .map(diff::ContentSnapshot::from)
                .unwrap_or_else(|| {
                    diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
                });
            let notes = self
                .store
                .as_deref()
                .and_then(|store| store.notes_of(id).ok())
                .flatten()
                .unwrap_or_default();
            let state = &mut self.doc.draft_diff;
            state.baseline = Some(Baseline {
                id,
                number: base,
                snapshot,
                notes,
            });
            state.cache = None;
            state.redline = None;
            state.redline_wait = None;
            state.redline_in_flight = None;
        }

        let hash = text_hash(&self.doc.generated_markdown);
        let state = &mut self.doc.draft_diff;
        let Some(baseline) = state.baseline.as_ref() else {
            return;
        };
        let draft_changed = state.last_draft.as_ref() != Some(&self.doc.draft);
        if state.cache.is_none() || state.text_hash != hash || draft_changed {
            let new = diff::ContentSnapshot::new(
                self.doc.draft.clone(),
                self.doc.generated_markdown.clone(),
                baseline.notes.clone(),
            );
            let report = diff::manuscript_diff(&baseline.snapshot, &new);
            state.hunks = diff_hunks::hunks(&report.body, &self.doc.generated_markdown);
            state.cache = Some((hash, report));
            state.text_hash = hash;
            state.last_draft = Some(self.doc.draft.clone());
        }

        let old = &baseline.snapshot.content_markdown;
        match &state.redline {
            None => {
                // 第一次进对照模式：同步算一次，右栏不空着等。要素标注按新旧
                // DraftInput 就地对照（旧快照在 Baseline.snapshot 里）。
                let display = UnitDisplay::new(&self.config.vocabulary);
                let doc = redline::build_with_inputs(
                    old,
                    &self.doc.generated_markdown,
                    &baseline.snapshot.snapshot,
                    &self.doc.draft,
                    &display,
                );
                let Some((_, report)) = &state.cache else {
                    return;
                };
                let links = ChangeLinks::new(&report.body, old, &doc.spans);
                state.redline = Some(RedlineView { doc, links, hash });
                state.preview_target = None;
                state.preview_hover = None;
            }
            Some(view) if view.hash == hash => state.redline_wait = None,
            Some(_) => {
                // 防抖：正文每变一次就重新计时，停手够久才发后台任务。
                let now = ctx.input(|input| input.time);
                let since = match state.redline_wait {
                    Some((waiting, since)) if waiting == hash => since,
                    _ => {
                        state.redline_wait = Some((hash, now));
                        now
                    }
                };
                let waited = now - since;
                if waited < REDLINE_DEBOUNCE {
                    ctx.request_repaint_after(Duration::from_secs_f64(REDLINE_DEBOUNCE - waited));
                } else if state.redline_in_flight != Some((baseline.key(), hash)) {
                    let base = baseline.key();
                    state.redline_in_flight = Some((base, hash));
                    let old = old.clone();
                    let new = self.doc.generated_markdown.clone();
                    // 要素标注要在后台线程里对照新旧 DraftInput：旧值取基准快照，
                    // 新值取当前表单；词库随任务带走，UnitDisplay 就地重建。
                    let old_input = baseline.snapshot.snapshot.clone();
                    let new_input = self.doc.draft.clone();
                    let vocabulary = self.config.vocabulary.clone();
                    let key = self.doc.key;
                    let tx = self.sender.clone();
                    thread::spawn(move || {
                        let display = UnitDisplay::new(&vocabulary);
                        let doc = redline::build_with_inputs(
                            &old, &new, &old_input, &new_input, &display,
                        );
                        let _ = tx.send(WorkerResult::Redline {
                            key,
                            base,
                            hash,
                            doc: Box::new(doc),
                        });
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 版本对照页的端到端检查：真库（内存）、真 egui 上下文，画几帧看两栏
    //! 是否都画出来、联动是否落到同一处变更、左栏能不能改字与还原。

    use super::*;
    use crate::app::{VersionSwitchPrompt, WorkerResult};
    use crate::draft_page::{DraftAction, DraftSession, ExportLinks, LoadedVersion};
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
        clock: f64,
        receiver: Receiver<WorkerResult>,
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
            let (sender, receiver) = std::sync::mpsc::channel();
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
                clock: 0.0,
                receiver,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.clock += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                time: Some(self.clock),
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

        fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> egui::FullOutput {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }])
        }

        fn click(&mut self, at: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(at)]);
            for pressed in [true, false] {
                self.frame(vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
        }

        /// 编辑框当前的光标（字符下标）。
        fn cursor(&self) -> Option<usize> {
            egui::TextEdit::load_state(&self.ctx, super::super::editor::editor_id())
                .and_then(|state| state.cursor.char_range())
                .map(|range| range.primary.index.0)
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

    /// 画出来的某段文字（第一份）的屏幕位置与 galley。
    fn find_text(
        output: &egui::FullOutput,
        needle: &str,
    ) -> Option<(egui::Pos2, std::sync::Arc<egui::Galley>)> {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) if shape.galley.text().contains(needle) => {
                    Some((shape.pos, shape.galley.clone()))
                }
                _ => None,
            })
    }

    #[test]
    fn published_and_archived_drafts_use_the_cached_readonly_pair_view() {
        for record_status in [ManuscriptStatus::Published, ManuscriptStatus::Archived] {
            let mut harness = Harness::new();
            let id = harness.doc.manuscript_id.expect("稿件库 id");
            let snapshot = harness.doc.draft.clone();
            harness
                .store
                .commit_manuscript_version(id, "最新稿", "", &snapshot, NEW, "")
                .unwrap();
            harness.doc.record_status = record_status;
            harness.doc.loaded_version = Some(LoadedVersion {
                manuscript_id: id,
                version_number: 2,
                name: "最新稿".into(),
            });
            harness.doc.draft_diff.base = Some(1);

            let output = harness.frame(Vec::new());
            let text = texts(&output);
            assert!(text.contains("请于八月十日前报送材料。"), "{text}");
            assert!(text.contains("请于八月十五日前报送材料。"), "{text}");
            assert!(
                harness.doc.draft_diff.version_pair.matches(
                    crate::version_pair_view::VersionPairKey {
                        manuscript_id: id,
                        old_version_number: Some(1),
                        new_version_number: 2,
                    }
                ),
                "{record_status:?} 稿件应走共享只读版本对组件"
            );
            assert!(
                harness.doc.draft_diff.cache.is_none(),
                "只读路径不再使用起草页 diff 缓存"
            );

            let before = harness.doc.generated_markdown.clone();
            harness.frame(vec![egui::Event::Text("不得编辑".into())]);
            assert_eq!(harness.doc.generated_markdown, before);
        }
    }
    #[test]
    fn both_panes_render_and_share_one_change_list() {
        let mut harness = Harness::new();
        let output = harness.frame(Vec::new());
        let text = texts(&output);
        // 左栏：编辑器里的新文本、空隙里的旧行都画出来了。
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
        let state = &harness.doc.draft_diff;
        let view = state.redline.as_ref().expect("花脸稿已算好");
        let (_, report) = state.cache.as_ref().unwrap();
        // 每处正文变更都能在花脸稿里找到位置，每个变更块也是。空行变更除外：
        // 视觉层不标空行，它们只在左栏的代码 diff 里出现。
        let changes = report.body.blocks.iter().filter_map(|block| match block {
            diff::DiffBlock::Changed(change) => Some(change),
            diff::DiffBlock::Unchanged(_) => None,
        });
        for (change, block) in changes.enumerate() {
            if block.role == diff::BlockRole::Blank {
                continue;
            }
            assert!(
                view.links.marked_range(change).is_some(),
                "第 {change} 处变更在预览里没有落点"
            );
        }
        for unit in 0..state.hunks.len() {
            assert!(state.marked_of_unit(unit).is_some(), "第 {unit} 块");
        }
    }

    /// 已删除的旧行画在编辑器的空隙里：旧行在两条新行之间，编辑器的文本里没有它。
    #[test]
    fn deleted_lines_sit_in_a_gap_between_editor_lines() {
        let mut harness = Harness::new();
        let output = harness.frame(Vec::new());
        let (editor_pos, editor) = find_text(&output, "本通知自印发之日起施行").expect("编辑器");
        let (old_pos, _) = find_text(&output, "多余的一段话。").expect("旧行");
        assert!(
            !editor.text().contains("多余的一段话"),
            "旧行不在可编辑文本里"
        );
        // 旧行夹在「## 工作要求」与「本通知…」之间。
        let row_of = |needle: &str| {
            let char_index = editor
                .text()
                .find(needle)
                .map(|byte| editor.text()[..byte].chars().count())?;
            Some(
                editor
                    .pos_from_cursor(egui::text::CCursor::new(char_index))
                    .translate(editor_pos.to_vec2()),
            )
        };
        let above = row_of("## 工作要求").unwrap();
        let below = row_of("本通知自印发之日起施行").unwrap();
        assert!(
            old_pos.y >= above.bottom() - 0.5 && old_pos.y < below.top(),
            "{above:?} {old_pos:?} {below:?}"
        );
    }

    #[test]
    fn f7_steps_through_hunks_and_moves_the_cursor() {
        let mut harness = Harness::new();
        harness.frame(Vec::new());
        assert_eq!(harness.doc.draft_diff.view.focus(), 0);
        let total = harness.doc.draft_diff.hunks.len();
        assert!(total >= 2, "{:#?}", harness.doc.draft_diff.hunks);
        harness.key(egui::Key::F7, egui::Modifiers::NONE);
        harness.frame(Vec::new());
        assert_eq!(harness.doc.draft_diff.view.focus(), 1);
        // 光标跟到了第二块的首行。
        let line = harness.doc.draft_diff.hunks[1].first_line();
        let cursor = harness.cursor().expect("跳转后有光标");
        let start = harness.doc.generated_markdown
            [..line_offset(&harness.doc.generated_markdown, line)]
            .chars()
            .count();
        assert_eq!(cursor, start);
        // Shift+F7 往回：1 → 0 → 首尾循环到最后一块。
        harness.key(egui::Key::F7, egui::Modifiers::SHIFT);
        harness.frame(Vec::new());
        assert_eq!(harness.doc.draft_diff.view.focus(), 0, "Shift+F7 是上一处");
        harness.key(egui::Key::F7, egui::Modifiers::SHIFT);
        harness.frame(Vec::new());
        assert_eq!(harness.doc.draft_diff.view.focus(), total - 1, "首尾循环");
    }

    /// 在编辑器里点空隙下面那一行：光标落在那一行，焦点跟到这个变更块。
    #[test]
    fn clicking_the_line_below_a_gap_puts_the_cursor_there() {
        let mut harness = Harness::new();
        let output = harness.frame(Vec::new());
        let (pos, editor) = find_text(&output, "本通知自印发之日起施行").unwrap();
        let byte = editor.text().find("本通知").unwrap();
        let index = editor.text()[..byte].chars().count();
        let rect = editor
            .pos_from_cursor(egui::text::CCursor::new(index + 2))
            .translate(pos.to_vec2());
        harness.click(rect.center());
        harness.frame(Vec::new());
        let cursor = harness.cursor().expect("点过后有光标");
        assert!(
            (index..index + 4).contains(&cursor),
            "光标 {cursor} 应在「本通知」一行（{index}）"
        );
        let hunk = harness
            .doc
            .draft_diff
            .hunks
            .iter()
            .position(|hunk| {
                hunk.deleted
                    .iter()
                    .any(|row| row.text() == "多余的一段话。")
            })
            .unwrap();
        assert_eq!(harness.doc.draft_diff.view.focus(), hunk, "焦点跟着光标走");
    }

    /// 逐块还原：文本换回基准版那一块，该块从 diff 里消失；Ctrl+Z 一次回到还原前。
    #[test]
    fn reverting_a_hunk_is_undoable() {
        let mut harness = Harness::new();
        harness.frame(Vec::new());
        let before = harness.doc.generated_markdown.clone();
        let hunks = harness.doc.draft_diff.hunks.len();
        let first = harness.doc.draft_diff.hunks[0].clone();
        {
            let ctx = harness.ctx.clone();
            let mut page = DraftPage {
                doc: &mut harness.doc,
                config: &mut harness.config,
                store: Some(&mut harness.store),
                sender: &harness.sender,
                status: &mut harness.status,
                version_switch: &mut harness.version_switch,
                revert_confirm: &mut harness.revert_confirm,
                actions: &mut harness.actions,
                export_links: &mut harness.export_links,
                metrics: &mut harness.metrics,
            };
            page.revert_hunk(&ctx, 0);
        }
        let (expected, _) = diff_hunks::revert(&first, OLD, &before);
        assert_eq!(harness.doc.generated_markdown, expected);
        assert!(
            harness
                .doc
                .generated_markdown
                .contains("请于八月十日前报送材料。")
        );
        harness.frame(Vec::new());
        harness.frame(Vec::new());
        assert_eq!(
            harness.doc.draft_diff.hunks.len(),
            hunks - 1,
            "还原的块消失了"
        );
        // 编辑框要有焦点才收快捷键。
        harness
            .ctx
            .memory_mut(|memory| memory.request_focus(super::super::editor::editor_id()));
        harness.frame(Vec::new());
        harness.key(egui::Key::Z, egui::Modifiers::COMMAND);
        harness.frame(Vec::new());
        assert_eq!(harness.doc.generated_markdown, before, "Ctrl+Z 撤回还原");
    }

    /// 新增的空行也标「（空行）」：从前只有删掉的空行（画在空隙里）有，新增的
    /// 空行只是一行绿底，看不出加了什么（第 ③ 期测试 F10）。
    #[test]
    fn an_added_blank_line_shows_a_placeholder() {
        let mut harness = Harness::new();
        let before = texts(&harness.frame(Vec::new()))
            .matches("（空行）")
            .count();
        harness.doc.generated_markdown = harness
            .doc
            .generated_markdown
            .replace("\n\n各单位要高度重视。", "\n\n\n各单位要高度重视。");
        harness.frame(Vec::new());
        let output = harness.frame(Vec::new());
        let text = texts(&output);
        assert_eq!(
            text.matches("（空行）").count(),
            before + 1,
            "新增的空行补一个「（空行）」：{text}"
        );
        // 它只是画上去的标签，不进正文。
        assert!(!harness.doc.generated_markdown.contains("（空行）"));
    }

    /// 改字之后花脸稿在后台防抖重算：停手之前不发任务；回来的结果若已过期就丢掉。
    #[test]
    fn the_redline_catches_up_in_the_background() {
        let mut harness = Harness::new();
        harness.frame(Vec::new());
        let first_hash = harness.doc.draft_diff.redline.as_ref().unwrap().hash;
        harness.doc.generated_markdown.push_str("\n\n再加一段。");
        harness.frame(Vec::new());
        // 代码 diff 立刻跟上，花脸稿还是旧的。
        let hash = harness.doc.draft_diff.text_hash;
        assert_ne!(hash, first_hash);
        assert_eq!(
            harness.doc.draft_diff.redline.as_ref().unwrap().hash,
            first_hash
        );
        assert!(
            harness.doc.draft_diff.redline_in_flight.is_none(),
            "防抖期内不发任务"
        );
        // 停手 150ms 以上：发后台任务。
        for _ in 0..5 {
            harness.frame(Vec::new());
        }
        let base = (harness.doc.manuscript_id.unwrap(), 1);
        assert_eq!(harness.doc.draft_diff.redline_in_flight, Some((base, hash)));
        let result = harness
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("后台花脸稿");
        let WorkerResult::Redline {
            base: done_base,
            hash: done,
            doc,
            ..
        } = result
        else {
            panic!("应是花脸稿结果");
        };
        assert_eq!(done_base, base);
        // 过期的结果（哈希对不上）丢掉。
        harness
            .doc
            .draft_diff
            .accept_redline(base, done ^ 1, (*doc).clone());
        assert_eq!(
            harness.doc.draft_diff.redline.as_ref().unwrap().hash,
            first_hash
        );
        harness.doc.draft_diff.accept_redline(base, done, *doc);
        let view = harness.doc.draft_diff.redline.as_ref().unwrap();
        assert_eq!(view.hash, hash);
        assert!(crate::export::strip_redline(&view.doc.markdown).contains("再加一段"));
    }

    /// 后台花脸稿在途时换了基准版、正文没动：晚到的旧基准结果正文哈希照样对得上，
    /// 必须按基准版丢掉，右栏保持对着新基准算的那一份。
    #[test]
    fn a_late_redline_for_the_previous_baseline_is_dropped() {
        let mut harness = Harness::new();
        let id = harness.doc.manuscript_id.unwrap();
        // 再提交一版：基准默认跟最新版走（v2），v1 是更早的一版。
        let v2 = "# 关于报送材料的函\n\n## 工作目标\n\n请于八月二十日前报送材料。";
        harness
            .store
            .commit_manuscript_version(id, "二稿", "", &DraftInput::default(), v2, "")
            .unwrap();
        harness.frame(Vec::new());
        harness.doc.generated_markdown.push_str("\n\n再加一段。");
        for _ in 0..6 {
            harness.frame(Vec::new());
        }
        let hash = harness.doc.draft_diff.text_hash;
        assert_eq!(
            harness.doc.draft_diff.redline_in_flight,
            Some(((id, 2), hash)),
            "对着 v2 发出后台任务"
        );
        let WorkerResult::Redline {
            base,
            hash: done,
            doc,
            ..
        } = harness
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("后台花脸稿")
        else {
            panic!("应是花脸稿结果");
        };
        assert_eq!((base, done), ((id, 2), hash));
        // 结果回来之前，用户把基准换成 v1（正文不动）。
        harness.doc.draft_diff.base = Some(1);
        harness.frame(Vec::new());
        let expected = redline::build(OLD, &harness.doc.generated_markdown).markdown;
        assert_eq!(
            harness
                .doc
                .draft_diff
                .redline
                .as_ref()
                .unwrap()
                .doc
                .markdown,
            expected,
            "换基准后右栏对着 v1 重算"
        );
        // 晚到的 v2 结果：正文哈希对得上，但基准不对，丢掉。
        harness.doc.draft_diff.accept_redline(base, done, *doc);
        assert_eq!(
            harness
                .doc
                .draft_diff
                .redline
                .as_ref()
                .unwrap()
                .doc
                .markdown,
            expected,
            "旧基准的结果不得覆盖右栏"
        );
    }

    /// 长稿打字的帧耗时探针（人工运行：`cargo test --release --bin gongwen-assistant
    /// long_draft_typing_probe -- --ignored --nocapture`）。2000 行、100 处改动，
    /// 逐字追加并逐帧计时；花脸稿在后台，这里量的是界面线程。
    #[test]
    #[ignore]
    fn long_draft_typing_probe() {
        let old = (1..=2000)
            .map(|index| {
                format!("第{index}段，各地各校要深刻认识本项工作的重要意义，确保各项部署落到实处。")
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let new = (1..=2000)
            .map(|index| {
                if index % 20 == 0 {
                    format!("第{index}段，各地各校要充分认识本项工作的重大意义，确保部署落到实处、见到实效。")
                } else {
                    format!("第{index}段，各地各校要深刻认识本项工作的重要意义，确保各项部署落到实处。")
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut harness = Harness::new();
        let id = harness
            .store
            .create(
                &NewManuscript {
                    snapshot: DraftInput::default(),
                    content_markdown: old.clone(),
                    notes: String::new(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        harness
            .store
            .commit_manuscript_version(id, "基准", "", &DraftInput::default(), &old, "")
            .unwrap();
        harness.doc.manuscript_id = Some(id);
        harness.doc.generated_markdown = new;
        let started = std::time::Instant::now();
        harness.frame(Vec::new());
        eprintln!("首帧（含同步花脸稿）：{:?}", started.elapsed());
        eprintln!("变更块：{}", harness.doc.draft_diff.hunks.len());
        harness.frame(Vec::new());
        let mut times = Vec::new();
        for index in 0..30 {
            harness
                .doc
                .generated_markdown
                .push(if index % 2 == 0 { '加' } else { '字' });
            let started = std::time::Instant::now();
            harness.frame(Vec::new());
            times.push(started.elapsed());
        }
        times.sort();
        eprintln!(
            "打字帧耗时：中位 {:?}，最慢 {:?}",
            times[times.len() / 2],
            times[times.len() - 1]
        );
        let mut idle = Vec::new();
        for _ in 0..10 {
            let started = std::time::Instant::now();
            harness.frame(Vec::new());
            idle.push(started.elapsed());
        }
        idle.sort();
        eprintln!("空闲帧耗时：中位 {:?}", idle[idle.len() / 2]);
    }
}
