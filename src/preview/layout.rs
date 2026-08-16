//! 排版基础：文本布局、段落/表格块、占位与版心度量辅助。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::layout`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::table::ColumnAlignment;
use crate::preview::{BODY_PT, INDENT_CHARS, Metrics, PAREN_PT, TABLE_LINE_PT, TABLE_PT};
use crate::theme;
use eframe::egui;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Align, Color32, FontId, Stroke};
use std::ops::Range;
use std::sync::Arc;

/// 正文各级标题的字体：与 `export::docx::heading_paragraph` 保持一致。
pub(crate) fn heading_family(level: u8) -> &'static str {
    match level {
        2 => theme::FONT_HEITI,
        3 => theme::FONT_KAITI,
        _ => theme::FONT_FANGSONG,
    }
}

pub(crate) fn text_format(font: FontId, line: f32) -> TextFormat {
    TextFormat {
        font_id: font,
        color: theme::paper::ink(),
        line_height: Some(line),
        ..Default::default()
    }
}

pub(crate) fn job(width: f32) -> LayoutJob {
    LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: width,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 单行不换行的排版任务，用于量红头这类需要先测宽度的文字。
pub(crate) fn single_line(text: &str, format: TextFormat) -> LayoutJob {
    let mut job = job(f32::INFINITY);
    job.append(text, 0.0, format);
    job
}

pub(crate) fn layout(ui: &egui::Ui, job: LayoutJob) -> Arc<egui::Galley> {
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

pub(crate) fn draw(ui: &mut egui::Ui, job: LayoutJob) {
    let galley = layout(ui, job);
    ui.add(egui::Label::new(galley));
}

/// 占一块高 `height` 的版心宽区域，把绘制交给回调。抬头、落款、版记这些需要
/// 自己算横向位置的块都走这里。
pub(crate) fn place(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    height: f32,
    paint: impl FnOnce(&egui::Painter, egui::Rect),
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
    paint(ui.painter(), rect);
}

/// 把若干行叠放在版心内的一个块里。`left` 是块左沿相对版心左沿的偏移，
/// `align` 同时决定每行的对齐方式和锚点位置。
pub(crate) fn stacked(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    galleys: &[Arc<egui::Galley>],
    left: f32,
    width: f32,
    align: Align,
) {
    let height = galleys.iter().map(|galley| galley.size().y).sum::<f32>();
    place(ui, metrics, height, |painter, rect| {
        let anchor = match align {
            Align::Center => rect.left() + left + width / 2.0,
            Align::Max => rect.left() + left + width,
            _ => rect.left() + left,
        };
        let mut y = rect.top();
        for galley in galleys {
            // 右对齐按字形实际右缘（mesh_bounds.max.x）摆位，精确落在锚点上；
            // 用 size() 会算入行前 leading，产生 1~4px 错位。空行等无字形时兜底。
            let pos = match align {
                Align::Max => {
                    let right = if galley.mesh_bounds.max.x.is_finite() {
                        galley.mesh_bounds.max.x
                    } else {
                        galley.size().x
                    };
                    egui::pos2(anchor - right, y)
                }
                _ => egui::pos2(anchor, y),
            };
            painter.galley(pos, galley.clone(), theme::paper::ink());
            y += galley.size().y;
        }
    });
}

/// 排一行文字并返回 galley：`width` 为换行宽度，`align` 为行内对齐。
pub(crate) fn line_galley(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    font: FontId,
    width: f32,
    align: Align,
) -> Arc<egui::Galley> {
    let mut job = job(width);
    job.halign = align;
    job.append(text, 0.0, text_format(font, metrics.line));
    layout(ui, job)
}

/// 首行缩进用两个全角空格实现：仿宋的全角空格正好一个字宽，
/// 两个即 2 字，与 Word 的 640 缇一致。
pub(crate) fn indent(count: f32) -> String {
    "\u{3000}".repeat(count as usize)
}

/// 一行文字的整段渲染（居中标题、缩进标题等都走这里）。
pub(crate) fn line_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    align: Align,
) {
    let mut job = job(metrics.content);
    job.halign = align;
    job.append(
        text,
        0.0,
        text_format(metrics.font(family, size), metrics.line),
    );
    if align == Align::Center {
        // halign 只让 galley 内部每行相对自身宽度居中，经 Label 摆到光标处后，
        // 单行标题会偏左、多行才看似居中。这里把 galley 直接画在版心正中，
        // 让每一行都相对版心宽居中，与 Word 的居中段落一致。
        let galley = layout(ui, job);
        let height = galley.size().y;
        place(ui, metrics, height, |painter, rect| {
            painter.galley(
                egui::pos2(rect.left() + metrics.content / 2.0, rect.top()),
                galley,
                theme::paper::ink(),
            );
        });
    } else {
        draw(ui, job);
    }
}

/// 正文段落：仿宋三号、首行缩进 2 字；行内保留加粗与括号楷体。
///
/// 这里不开 `justify`：egui 在两端对齐时会把行首空白排除在对齐范围外，首行缩进
/// 的两个全角空格会被直接吃掉。中文正文各字等宽，行末本就基本对齐，取舍下
/// 保住缩进更要紧。
pub(crate) fn body_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    first_line_indent: bool,
) {
    let mut job = job(metrics.content);
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    if first_line_indent {
        job.append(
            &indent(INDENT_CHARS),
            0.0,
            text_format(normal.clone(), metrics.line),
        );
    }
    append_inline(&mut job, metrics, text, &normal);
    draw(ui, job);
}

/// 行内片段按导出规则上色：括号内容楷体四号，加粗用黑体近似（egui 不做假粗）。
pub(crate) fn append_inline(job: &mut LayoutJob, metrics: &Metrics, text: &str, normal: &FontId) {
    for segment in export::inline_segments(text) {
        let font = if segment.parenthesized {
            metrics.font(theme::FONT_KAITI, PAREN_PT)
        } else if segment.bold {
            metrics.font(theme::FONT_HEITI, BODY_PT)
        } else {
            normal.clone()
        };
        job.append(&segment.text, 0.0, text_format(font, metrics.line));
    }
}

/// 与导出一致：空段落和 HTML 包裹行不成段。
pub(crate) fn is_renderable_paragraph(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains("<div") && !text.contains("</div")
}

/// 表格：四号字、行距 21 磅，表头黑体居中，列宽直接取导出器算好的智能列宽，
/// 因此预览的列宽与导出的 Word 表格一致。
pub(crate) fn table_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    rows: &[Vec<String>],
    aligns: &[export::ColumnAlign],
) {
    let columns = export::table_columns(rows, aligns);
    if columns.is_empty() {
        return;
    }
    let widths = columns
        .iter()
        .map(|column| metrics.content * column.fraction)
        .collect::<Vec<_>>();

    // 单元格内边距不能超过列宽的一小部分，否则窄列会算出负的换行宽度。
    let line = metrics.pt(TABLE_LINE_PT);
    let stroke = Stroke::new(1.0_f32.max(metrics.scale), theme::paper::ink());
    for (index, row) in rows.iter().enumerate() {
        let header = index == 0;
        let font = metrics.font(
            if header {
                theme::FONT_HEITI
            } else {
                theme::FONT_FANGSONG
            },
            TABLE_PT,
        );
        let cells = widths
            .iter()
            .enumerate()
            .map(|(column, width)| {
                let padding = metrics.pt(3.0).min(width * 0.12);
                let text = export::plain_text(row.get(column).map_or("", String::as_str));
                let mut job = job((width - 2.0 * padding).max(1.0));
                // 表头一律居中；正文列按导出器判定的对齐方式。
                let align = if header {
                    ColumnAlignment::Center
                } else {
                    columns[column].alignment
                };
                job.halign = match align {
                    ColumnAlignment::Center => Align::Center,
                    ColumnAlignment::Right => Align::RIGHT,
                    ColumnAlignment::Left => Align::LEFT,
                };
                job.append(&text, 0.0, text_format(font.clone(), line));
                let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
                (galley, padding, align)
            })
            .collect::<Vec<_>>();
        let height = cells
            .iter()
            .map(|(galley, padding, _)| galley.size().y + 2.0 * padding)
            .fold(line, f32::max);

        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        let mut x = rect.left();
        for (column, (galley, padding, align)) in cells.iter().enumerate() {
            if column > 0 {
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    stroke,
                );
            }
            // halign 决定每行相对锚点的位置：居中时锚点取单元格中线，靠左时取内边距。
            let anchor = match align {
                ColumnAlignment::Center => x + widths[column] / 2.0,
                ColumnAlignment::Right => x + widths[column] - padding,
                ColumnAlignment::Left => x + padding,
            };
            let top = rect.top() + (height - galley.size().y) / 2.0;
            painter.galley(egui::pos2(anchor, top), galley.clone(), theme::paper::ink());
            x += widths[column];
        }
    }
}

/// 画一个来自 Markdown 的块，并让它可以点：悬停时淡底提示，点击后把它在源码中的
/// 范围报给调用方；`anchor` 命中的块常亮，与编辑器里的高亮一一对应。
pub(crate) fn clickable(
    ui: &mut egui::Ui,
    range: &Range<usize>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    // 底色要压在文字下面：先占一个空图形位，量出块的范围后再回填。
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let inner = ui.scope(add_contents).response.rect;
    if !inner.is_positive() {
        return;
    }
    let rect = inner.expand2(egui::vec2(4.0, 1.0));
    let response = ui.interact(
        rect,
        egui::Id::new(("gw-preview-block", range.start, range.end)),
        egui::Sense::click(),
    );
    if response.clicked() {
        *clicked = Some(range.clone());
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // 查找命中通常只是块内的一小段文字；与块范围相交也要标亮，这样在“公文预览”
    // 模式中仍能看出当前命中位于哪一段。
    let anchored = anchor.is_some_and(|anchor| {
        !anchor.is_empty()
            && !range.is_empty()
            && anchor.start < range.end
            && range.start < anchor.end
    });
    if anchored && *scroll_to_anchor {
        response.scroll_to_me(Some(egui::Align::Center));
        *scroll_to_anchor = false;
    }
    let fill = if anchored {
        theme::accent_soft()
    } else if response.hovered() {
        theme::paper::hover_tint()
    } else {
        return;
    };
    ui.painter().set(
        backdrop,
        egui::epaint::RectShape::filled(rect, egui::CornerRadius::same(3), fill),
    );
}

/// 一张“纸”：白底、细边、内含公文版心。
pub(crate) fn sheet(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        // 用量好的可见宽度而不是 available_width：滚动区里后者可能是无穷大。
        let side = ((metrics.viewport - metrics.page) / 2.0).max(0.0);
        ui.add_space(side);
        ui.vertical(|ui| {
            ui.set_max_width(metrics.page);
            egui::Frame::new()
                .fill(theme::paper::bg())
                .stroke(Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(3))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 2],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(theme::paper::shadow_alpha()),
                })
                .show(ui, |ui| {
                    // 页边距用 add_space 铺出来：`Margin` 是 i8，放大后会溢出。
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    ui.set_width(metrics.page);
                    ui.add_space(metrics.margin_top);
                    ui.horizontal_top(|ui| {
                        ui.add_space(metrics.margin_left);
                        ui.vertical(|ui| {
                            ui.set_width(metrics.content);
                            ui.set_min_height((metrics.page - metrics.margin_top * 2.0).max(0.0));
                            ui.style_mut().visuals.override_text_color = Some(theme::paper::ink());
                            // 预览是拿来看版式和点回源码的，不做文字选择，
                            // 否则 Label 会把点击当成拖选吃掉。
                            ui.style_mut().interaction.selectable_labels = false;
                            add_contents(ui);
                        });
                    });
                    ui.add_space(metrics.margin_top);
                });
        });
    });
}
