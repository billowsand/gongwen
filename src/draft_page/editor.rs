//! 编辑器绘制：行号、混合装饰、Markdown 编辑器、版式渲染与审校提示。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::editor`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::visible_rows;
use crate::draft_page::{
    DRAG_STEP_MAX, DraftPage, OFFICIAL_BODY_SIZE, OFFICIAL_EDITOR_CONTENT_WIDTH,
    OFFICIAL_PAGE_HEIGHT, OFFICIAL_PAGE_MARGIN_LEFT, OFFICIAL_PAGE_MARGIN_TOP, OFFICIAL_PAGE_WIDTH,
    PreviewMode, continue_ordered_list, editor_cursor, editor_selection, is_table_separator_line,
    is_table_source_line, jump_to_source, markdown_heading_level, markdown_matches_mode,
    select_source_range, table_column_count,
};
use crate::export;
use crate::highlight::ordered_list_lines;
use crate::preview;
use crate::theme;
use crate::units::UnitDisplay;
use eframe::egui;
use std::ops::Range;

/// 在公文预览里点中的那一块：记下它在 Markdown 中的字节范围，以及点击当时
/// 这段范围里的原文。
pub(crate) struct PreviewAnchor {
    pub(crate) range: Range<usize>,
    pub(crate) text: String,
}

impl PreviewAnchor {
    /// 正文改过之后，旧的字节范围可能越界、落在字符中间，或者已经指向别的内容。
    /// 只有范围内的原文仍与点击时一致，才认为锚点还指着同一段；`str::get`
    /// 顺带挡掉越界和非字符边界，避免把半个汉字切开。
    pub(crate) fn range_in(&self, text: &str) -> Option<Range<usize>> {
        (text.get(self.range.clone()) == Some(self.text.as_str())).then(|| self.range.clone())
    }
}

/// 审校区编辑框的固定 id：预览点击回跳时要按它取回光标状态。
pub(crate) fn editor_id() -> egui::Id {
    egui::Id::new("gw-markdown-editor")
}

pub(crate) fn source_line_at_char(text: &str, char_index: usize) -> usize {
    text.chars()
        .take(char_index.min(text.chars().count()))
        .filter(|ch| *ch == '\n')
        .count()
}

pub(crate) fn active_source_line(ctx: &egui::Context, text: &str) -> usize {
    egui::TextEdit::load_state(ctx, editor_id())
        .and_then(|state| state.cursor.char_range())
        .map_or(0, |range| source_line_at_char(text, range.primary.index.0))
}

#[derive(Clone, Copy)]
pub(crate) struct EditorLineVisual {
    top: f32,
    bottom: f32,
    baseline: f32,
}

pub(crate) fn editor_line_visuals(
    output: &egui::text_edit::TextEditOutput,
) -> Vec<EditorLineVisual> {
    let mut lines = Vec::new();
    let mut top = None;
    let mut baseline = None;
    let mut bottom = output.galley_pos.y;
    for placed in &output.galley.rows {
        let row_top = output.galley_pos.y + placed.pos.y;
        let row_bottom = row_top + placed.size.y;
        top.get_or_insert(row_top);
        if baseline.is_none() {
            baseline = placed
                .glyphs
                .iter()
                .find(|glyph| glyph.font_height > OFFICIAL_BODY_SIZE * 0.5)
                .or_else(|| placed.glyphs.first())
                .map(|glyph| row_top + glyph.pos.y);
        }
        bottom = row_bottom;
        if placed.ends_with_newline {
            let top = top.take().unwrap_or(row_top);
            lines.push(EditorLineVisual {
                top,
                bottom,
                baseline: baseline.take().unwrap_or((top + bottom) * 0.5),
            });
        }
    }
    if let Some(top) = top {
        lines.push(EditorLineVisual {
            top,
            bottom,
            baseline: baseline.unwrap_or((top + bottom) * 0.5),
        });
    }
    lines
}

/// 按 galley 的实际行高绘制源码行号。一个 Markdown 段落自动换行时，
/// 只在第一个视觉行旁显示编号，不把软换行误当成新的源码行。
pub(crate) fn paint_editor_line_numbers(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput) {
    let x = output.galley_pos.x - 10.0;
    let painter = ui.painter();
    let font = egui::FontId::new(theme::font_sizes::SMALL, egui::FontFamily::Proportional);
    for (index, line) in editor_line_visuals(output).into_iter().enumerate() {
        painter.text(
            egui::pos2(x, (line.top + line.bottom) * 0.5),
            egui::Align2::RIGHT_CENTER,
            (index + 1).to_string(),
            font.clone(),
            theme::text_muted(),
        );
    }
}

/// 实时排版中不写入 Markdown 的视觉层：公文自动编号和表格框线。
pub(crate) fn paint_hybrid_decorations(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    active_line: usize,
) {
    let visuals = editor_line_visuals(output);
    let source_lines = text.split('\n').collect::<Vec<_>>();
    let ordered_lines = ordered_list_lines(text);
    let painter = ui.painter();
    let mut counters = export::HeadingCounters::default();
    let attachment_count = export::parse_markdown(text)
        .iter()
        .filter(|block| {
            matches!(
                block,
                export::MarkdownBlock::Marker(export::MarkdownSection::Attachment)
            )
        })
        .count();
    let mut attachment_index = 0usize;

    for (index, line) in source_lines.iter().enumerate() {
        // 区段标记与每个附件标题处重置计数器；正文和附件使用同一标题层级。
        let prefix = counters.next(line);
        if export::parse_section_marker(line) == Some(export::MarkdownSection::Attachment) {
            attachment_index += 1;
            let next_title = source_lines[index + 1..]
                .iter()
                .map(|line| line.trim())
                .find(|line| !line.is_empty());
            let is_legacy = next_title
                .and_then(|line| line.strip_prefix("# "))
                .is_some_and(|title| export::legacy_attachment_label(title).is_some());
            if index != active_line
                && !is_legacy
                && let Some(visual) = visuals.get(index)
            {
                let label = if attachment_count == 1 {
                    "附件".to_string()
                } else {
                    format!("附件{attachment_index}")
                };
                let font = egui::FontId::new(
                    OFFICIAL_BODY_SIZE,
                    theme::official_family(theme::FONT_HEITI),
                );
                painter.text(
                    egui::pos2(output.galley_pos.x, (visual.top + visual.bottom) * 0.5),
                    egui::Align2::LEFT_CENTER,
                    label,
                    font,
                    egui::Color32::BLACK,
                );
            }
            continue;
        }
        if index != active_line
            && let (Some(info), Some(visual)) = (ordered_lines[index], visuals.get(index))
        {
            let label = if info.inline {
                export::circled_number(info.number)
            } else {
                format!("{}.", info.number)
            };
            let font = egui::FontId::new(
                OFFICIAL_BODY_SIZE,
                theme::official_family(theme::FONT_FANGSONG),
            );
            let label_galley = painter.layout_no_wrap(label, font, egui::Color32::BLACK);
            let label_baseline = label_galley
                .rows
                .first()
                .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
                .unwrap_or(label_galley.size().y);
            painter.galley(
                egui::pos2(
                    output.galley_pos.x
                        + if info.inline {
                            0.0
                        } else {
                            OFFICIAL_BODY_SIZE * 2.0
                        },
                    visual.baseline - label_baseline,
                ),
                label_galley,
                egui::Color32::BLACK,
            );
            continue;
        }
        let Some(level) = markdown_heading_level(line) else {
            continue;
        };
        if index == active_line {
            continue;
        }
        let (Some(prefix), Some(visual)) = (prefix, visuals.get(index)) else {
            continue;
        };
        let family = match level {
            2 => theme::FONT_HEITI,
            3 => theme::FONT_KAITI,
            _ => theme::FONT_FANGSONG,
        };
        let font = egui::FontId::new(OFFICIAL_BODY_SIZE, theme::official_family(family));
        let prefix_galley = painter.layout_no_wrap(prefix, font, egui::Color32::BLACK);
        let prefix_baseline = prefix_galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
            .unwrap_or(prefix_galley.size().y);
        painter.galley(
            egui::pos2(
                output.galley_pos.x + OFFICIAL_BODY_SIZE * 2.0,
                visual.baseline - prefix_baseline,
            ),
            prefix_galley,
            egui::Color32::BLACK,
        );
    }

    let stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
    let mut index = 0usize;
    while index < source_lines.len() {
        if !is_table_source_line(source_lines[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < source_lines.len() && is_table_source_line(source_lines[index]) {
            index += 1;
        }
        let end = index;
        let columns = source_lines[start..end]
            .iter()
            .map(|line| table_column_count(line))
            .max()
            .unwrap_or(1);
        for row in (start..end).filter(|row| !is_table_separator_line(source_lines[*row])) {
            let Some(visual) = visuals.get(row) else {
                continue;
            };
            let rect = egui::Rect::from_min_max(
                egui::pos2(output.galley_pos.x, visual.top),
                egui::pos2(
                    output.galley_pos.x + OFFICIAL_EDITOR_CONTENT_WIDTH,
                    visual.bottom,
                ),
            );
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
            for column in 1..columns {
                let x = rect.left() + rect.width() * column as f32 / columns as f32;
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    stroke,
                );
            }
        }
    }
}

impl DraftPage<'_> {
    pub(crate) fn open_result_drawer(&mut self) {
        self.doc.result_drawer_open = true;
    }

    pub(crate) fn preview_ui(&mut self, ui: &mut egui::Ui) {
        if self.doc.markdown_find.open {
            egui::Panel::top("preview_find")
                .frame(theme::panel(theme::surface(), 12))
                .show(ui, |ui| self.markdown_find_ui(ui));
        }
        egui::CentralPanel::default()
            .frame(theme::panel(theme::canvas(), 10))
            .show(ui, |ui| match self.doc.preview_mode {
                PreviewMode::Source => self.markdown_editor(ui),
                PreviewMode::Hybrid => self.markdown_hybrid_editor(ui),
                PreviewMode::Rendered => self.markdown_render(ui),
                PreviewMode::VersionDiff => self.version_diff_mode_ui(ui),
                PreviewMode::Split => {
                    egui::Panel::left("preview_split")
                        .default_size(420.0)
                        .size_range(280.0..=900.0)
                        .frame(egui::Frame::new().inner_margin(egui::Margin {
                            right: 8,
                            ..egui::Margin::ZERO
                        }))
                        .show(ui, |ui| self.markdown_editor(ui));
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| self.markdown_render(ui));
                }
            });
    }

    /// 返回是否点了关闭按钮——关闭请求由 `create_ui` 在面板动画之外落地，
    /// 闭包内直接改 `self.doc.result_drawer_open` 会被局部副本写回覆盖。
    pub(crate) fn result_drawer_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close_requested = false;
        // 标题与关闭按钮独占一行——右侧抽屉只有 300 点上下，挤不下一整排。
        ui.horizontal(|ui| {
            ui.strong(if self.doc.warnings.is_empty() {
                "审校提示".to_string()
            } else {
                format!("审校提示 {}", self.doc.warnings.len())
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, theme::Icon::X, "关闭审校提示（Esc）").clicked() {
                    close_requested = true;
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("warning_result_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                // 导出失败的原因原先挂在"导出文件"页上，那一页去掉后改挂这里，
                // 否则只剩状态栏一闪而过的一行，看不到全文。
                if let Some(error) = &self.doc.export_error {
                    egui::Frame::new()
                        .fill(theme::danger_soft())
                        .stroke(egui::Stroke::new(1.0, theme::danger().gamma_multiply(0.35)))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                egui::RichText::new("导出失败")
                                    .color(theme::danger())
                                    .strong(),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(error).color(theme::text_soft()),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    ui.add_space(8.0);
                }
                self.warnings_ui(ui);
            });
        close_requested
    }

    /// 预览缩放：默认按面板宽度自适应，也可以手动锁定倍率。
    pub(crate) fn zoom_controls(&mut self, ui: &mut egui::Ui) {
        // 自适应时以当前实际倍率为起点加减，避免首次放大反而变小。
        let current = self.doc.preview_zoom.unwrap_or(self.doc.preview_fit_scale);
        if theme::icon_button(ui, theme::Icon::ZoomOut, "缩小").clicked() {
            self.doc.preview_zoom = Some((current - 0.1).max(0.4));
        }
        ui.label(
            egui::RichText::new(format!("{:.0}%", current * 100.0)).color(theme::text_muted()),
        );
        if theme::icon_button(ui, theme::Icon::ZoomIn, "放大").clicked() {
            self.doc.preview_zoom = Some((current + 0.1).min(2.0));
        }
        if theme::icon_button_enabled(
            ui,
            self.doc.preview_zoom.is_some(),
            theme::Icon::FitWidth,
            "适应宽度",
        )
        .on_hover_text("回到按窗格宽度自动缩放")
        .clicked()
        {
            self.doc.preview_zoom = None;
        }
    }

    /// Markdown 源码编辑框，带语法高亮。
    pub(crate) fn markdown_editor(&mut self, ui: &mut egui::Ui) {
        self.markdown_editor_impl(ui, false);
    }

    /// 实时公文排版编辑器：Markdown 始终是唯一数据源，只改变屏幕上的布局。
    pub(crate) fn markdown_hybrid_editor(&mut self, ui: &mut egui::Ui) {
        self.markdown_editor_impl(ui, true);
    }

    pub(crate) fn markdown_editor_impl(&mut self, ui: &mut egui::Ui, hybrid: bool) {
        // 行数必须在进入 ScrollArea 之前算：滚动方向上的 available_height
        // 是无穷大，拿进去算会得到 usize::MAX 行，整个界面将无法布局。
        let rows = visible_rows(ui);
        let editable = !self.doc.read_only();
        // TextEdit 本身只会插入普通换行；在焦点确实位于有序列表、且没有选区时，
        // 抢在它之前消费 Enter，完成 Markdown 编辑器惯用的续号/空项退出行为。
        let ordered_enter = if editable
            && ui.ctx().memory(|memory| memory.has_focus(editor_id()))
            && editor_selection(ui.ctx(), &self.doc.generated_markdown)
                .is_some_and(|range| range.is_empty())
        {
            editor_cursor(ui.ctx(), &self.doc.generated_markdown)
                .and_then(|cursor| continue_ordered_list(&self.doc.generated_markdown, cursor))
        } else {
            None
        };
        if let Some((updated, cursor)) = ordered_enter
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            self.doc.generated_markdown = updated;
            self.doc.pending_source_jump = Some(cursor);
        }
        // 拆开借用：编辑框要可变借文本，布局器要可变借高亮缓存。
        let jump = self.doc.pending_source_jump.take();
        let selection = self.doc.pending_source_selection.take();
        let anchor = self
            .doc
            .preview_anchor
            .as_ref()
            .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown));
        let search_matches = if self.doc.markdown_find.open {
            markdown_matches_mode(
                &self.doc.generated_markdown,
                &self.doc.markdown_find.query,
                self.doc.markdown_find.case_sensitive,
                self.doc.markdown_find.regex,
            )
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        let active_line =
            if hybrid && editable && ui.ctx().memory(|memory| memory.has_focus(editor_id())) {
                active_source_line(ui.ctx(), &self.doc.generated_markdown)
            } else {
                usize::MAX
            };
        let show_line_numbers = self.config.show_editor_line_numbers;
        let text = &mut self.doc.generated_markdown;
        let highlighter = &mut self.doc.highlighter;
        let mut editor_lost_focus = false;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
            if hybrid {
                highlighter.layout_hybrid(
                    ui,
                    buffer.as_str(),
                    wrap_width.min(OFFICIAL_EDITOR_CONTENT_WIDTH),
                    active_line,
                    anchor.as_ref(),
                    &search_matches,
                )
            } else {
                highlighter.layout(
                    ui,
                    buffer.as_str(),
                    wrap_width,
                    anchor.as_ref(),
                    &search_matches,
                )
            }
        };
        if hybrid {
            let viewport_width = ui.available_width();
            egui::ScrollArea::both()
                .id_salt("hybrid_editor_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let side_space = ((viewport_width - OFFICIAL_PAGE_WIDTH) * 0.5).max(18.0);
                    ui.horizontal_top(|ui| {
                        ui.add_space(side_space);
                        egui::Frame::new()
                            .fill(egui::Color32::WHITE)
                            .stroke(egui::Stroke::new(1.0, theme::border_strong()))
                            .shadow(egui::epaint::Shadow {
                                offset: [0, 3],
                                blur: 14,
                                spread: 0,
                                color: egui::Color32::from_black_alpha(42),
                            })
                            .inner_margin(egui::Margin::ZERO)
                            .show(ui, |ui| {
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.set_min_size(egui::vec2(
                                        OFFICIAL_PAGE_WIDTH,
                                        OFFICIAL_PAGE_HEIGHT,
                                    ));
                                    ui.set_max_width(OFFICIAL_PAGE_WIDTH);
                                    ui.add_space(OFFICIAL_PAGE_MARGIN_TOP);
                                    ui.horizontal_top(|ui| {
                                        let gutter = if show_line_numbers { 38.0 } else { 0.0 };
                                        ui.add_space((OFFICIAL_PAGE_MARGIN_LEFT - gutter).max(0.0));
                                        if show_line_numbers {
                                            ui.add_space(gutter);
                                        }
                                        let output = egui::TextEdit::multiline(text)
                                        .id(editor_id())
                                        .interactive(editable)
                                        .frame(egui::Frame::NONE)
                                        .margin(egui::Margin::ZERO)
                                        .code_editor()
                                        .layouter(&mut layouter)
                                        .desired_width(OFFICIAL_EDITOR_CONTENT_WIDTH)
                                        .desired_rows(rows)
                                        .hint_text(
                                            "生成结果将在这里显示，也可以直接粘贴已有稿件再导出……",
                                        )
                                        .show(ui);
                                        editor_lost_focus |= output.response.lost_focus();
                                        if show_line_numbers {
                                            paint_editor_line_numbers(ui, &output);
                                        }
                                        paint_hybrid_decorations(ui, &output, text, active_line);
                                        if let Some(range) = selection {
                                            select_source_range(ui, &output, text, range);
                                        } else if let Some(offset) = jump {
                                            jump_to_source(ui, &output, text, offset);
                                        }
                                        if output.cursor_range.is_some_and(|range| {
                                            source_line_at_char(text, range.primary.index.0)
                                                != active_line
                                        }) {
                                            ui.ctx().request_repaint();
                                        }
                                    });
                                });
                            });
                    });
                });
        } else {
            theme::card().show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("preview_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let mut show_editor = |ui: &mut egui::Ui| {
                            egui::TextEdit::multiline(text)
                                .id(editor_id())
                                .interactive(editable)
                                .frame(egui::Frame::NONE)
                                .code_editor()
                                .layouter(&mut layouter)
                                .desired_width(f32::INFINITY)
                                .desired_rows(rows)
                                .hint_text("生成结果将在这里显示，也可以直接粘贴已有稿件再导出……")
                                .show(ui)
                        };
                        let output = if show_line_numbers {
                            ui.horizontal_top(|ui| {
                                ui.add_space(38.0);
                                show_editor(ui)
                            })
                            .inner
                        } else {
                            show_editor(ui)
                        };
                        editor_lost_focus |= output.response.lost_focus();
                        if show_line_numbers {
                            paint_editor_line_numbers(ui, &output);
                        }
                        if let Some(range) = selection {
                            select_source_range(ui, &output, text, range);
                        } else if let Some(offset) = jump {
                            jump_to_source(ui, &output, text, offset);
                        }
                    });
            });
        }
        if editable && editor_lost_focus {
            let normalized = export::normalize_ordered_list_punctuation(text);
            if normalized != *text {
                *text = normalized;
            }
        }
    }

    /// 公文版式预览。正文为空时也照排——红头、密级、文号、主送、落款这些
    /// 行文要素来自表单，填完就能先看版式。
    pub(crate) fn markdown_render(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::both()
            .id_salt("render_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let display = UnitDisplay::new(&self.config.vocabulary);
                let anchor = self
                    .doc
                    .preview_anchor
                    .as_ref()
                    .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown));

                // —— 宽度还在变的帧里不重排版面 ——
                // 自适应缩放是窗格宽度的连续函数，宽度每帧变一点，字号就每帧
                // 全新：galley 缓存全部落空，上千个汉字要按新字号重新栅格化进
                // 字体图集；图集一装满，epaint 会把整套字体连同缓存推倒重建、
                // 重传整张纹理（fonts.rs 的 fill_ratio > 0.8 分支）。这正是拖
                // 动分隔条时那一下下的顿挫——它是尖峰，不是普遍变慢，所以把字
                // 号量化成档位只能让它变稀，消不掉。
                //
                // 改成：宽度还在变的这些帧，版面沿用上一次落定的缩放（字号恒
                // 定、缓存全命中、图集一动不动），视觉上的缩放交给一次层变换连
                // 续完成——变换系数是浮点，要多连续有多连续，一格都不跳。宽度
                // 一停下来就按精确缩放重排一次，文字随即恢复锐利。
                let visible = ui
                    .clip_rect()
                    .intersect(ui.ctx().input(|input| input.content_rect()));
                let target = preview::fit_scale(visible.width(), self.doc.preview_zoom);
                // 只有"拖动幅度"的宽度变化才值得冻结版面：分隔条一帧走几个像素，
                // 中间那些帧连起来才是一个连续动作。首帧（滚动区还没量准，可视宽
                // 度是无穷大）、切换显示方式、窗口最大化这类一步到位的跳变没有
                // 连续过程可言，直接按精确倍率重排，省得白白糊一帧。
                let step = (visible.width() - self.doc.preview_last_width).abs();
                let settled =
                    !(0.5..=DRAG_STEP_MAX).contains(&step) || self.doc.preview_layout_scale <= 0.0;
                self.doc.preview_last_width = visible.width();
                if settled {
                    self.doc.preview_layout_scale = target;
                }
                let layout_scale = self.doc.preview_layout_scale;
                let ratio = target / layout_scale;
                // 以可视区顶边中点为支点：纸张本来就横向居中于 viewport，绕这个
                // 点缩放后依旧严丝合缝地居中，顶部那一行也钉在原处不漂。
                let transform = (!settled && (ratio - 1.0).abs() > 1e-4).then(|| {
                    let pivot = egui::pos2(visible.center().x, visible.top()).to_vec2();
                    egui::emath::TSTransform::from_translation(pivot)
                        * egui::emath::TSTransform::from_scaling(ratio)
                        * egui::emath::TSTransform::from_translation(-pivot)
                });

                // 变换会把裁剪矩形一并缩放，先按逆变换预补偿，变换之后正好落回
                // 真正的可视区，内容不会被切掉或漏出。
                let clip = ui.clip_rect();
                if let Some(transform) = transform {
                    ui.set_clip_rect(transform.inverse().mul_rect(clip));
                }
                // 只圈住预览自己发出的这段图形。滚动条是 ScrollArea 在这段范围
                // 之外画的，因此不会跟着一起缩放。
                let layer = ui.layer_id();
                let first = ui.painter().add(egui::Shape::Noop);

                let output = preview::official_preview(
                    ui,
                    &self.doc.draft,
                    &display,
                    &self.doc.generated_markdown,
                    preview::PreviewScale {
                        zoom: Some(layout_scale),
                        // 裁剪矩形被预补偿过，量出来会偏窄，这里给真实窗格宽度。
                        viewport: Some(visible.width()),
                    },
                    anchor.as_ref(),
                    self.doc.pending_render_jump,
                );

                if let Some(transform) = transform {
                    let last = ui.painter().add(egui::Shape::Noop);
                    ui.ctx().graphics_mut(|graphics| {
                        graphics
                            .entry(layer)
                            .transform_range(first, last, transform);
                    });
                    ui.set_clip_rect(clip);
                    // 拖动一停，还需要再来一帧才能发现"宽度没变"并按精确缩放重排。
                    ui.ctx().request_repaint();
                }

                self.doc.pending_render_jump = false;
                // 加减档以"眼睛看到的倍率"为起点，而不是本帧用来排版的那个。
                self.doc.preview_fit_scale = target;
                // 点中版式上的某一块：源码里同步高亮，并把光标带过去。
                if let Some(range) = output.clicked {
                    self.doc.pending_source_selection = None;
                    self.doc.pending_source_jump = Some(range.start);
                    self.doc.preview_anchor =
                        self.doc
                            .generated_markdown
                            .get(range.clone())
                            .map(|text| PreviewAnchor {
                                range,
                                text: text.to_owned(),
                            });
                    ui.ctx().request_repaint();
                }
                ui.add_space(12.0);
            });
    }

    pub(crate) fn warnings_ui(&mut self, ui: &mut egui::Ui) {
        if self.doc.warnings.is_empty() {
            ui.horizontal(|ui| {
                theme::dot(ui, theme::success());
                ui.add_space(2.0);
                ui.label(egui::RichText::new("暂无审校提示").color(theme::text_muted()));
            });
            return;
        }
        // 点中的定位目标要等渲染完再处理：循环里借着 &self.doc，跳转要 &mut self。
        let mut jump = None;
        egui::Frame::new()
            .fill(theme::warn_soft())
            .stroke(egui::Stroke::new(1.0, theme::warn().gamma_multiply(0.35)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // 条数由抽屉标题写，这里不再重复一遍。
                for warning in &self.doc.warnings {
                    // 提示往往很长，必须显式换行，否则会把底部面板顶宽。
                    let text = egui::RichText::new(format!("· {}", warning.message));
                    let Some(span) = warning.span.clone() else {
                        ui.add(
                            egui::Label::new(text.color(theme::text_soft()))
                                .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        continue;
                    };
                    // 能定位到正文的提示（孤行等）做成可点的：点一下切回 Markdown
                    // 视图并选中那一段，省得用户自己按行号数过去。
                    // sense 显式写出来：egui 默认给 Label 加的是"可选中文本"那套
                    // 感知，关掉 selectable_labels 就没了，不能指望它。
                    let response = ui
                        .add(
                            egui::Label::new(text.color(theme::accent()))
                                .wrap_mode(egui::TextWrapMode::Wrap)
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("点击定位到正文中的这一段");
                    if response.clicked() {
                        jump = Some(span);
                    }
                }
            });
        if let Some(span) = jump {
            self.jump_to_source(span);
        }
    }
}
