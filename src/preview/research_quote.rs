//! 研究报告的引文与文框：Markdown 的 `>` 引用块。
//!
//! 版式照 `md2tex.cls` 的 `mdxquote` 与 `mdxboxtblr` 抄，与编译出的 PDF 对得上：
//!
//! - 引文：楷体、与正文同字号（14 磅、行距 24 磅），整段左右各缩进两字，首行再
//!   缩进两字，前后各空半行；出处行（`——《…》`）靠右、不缩进；
//! - 文框（专栏、案例……写什么印什么）：满版心宽的细框、浅灰底，标题行
//!   "专栏 2.1　标题"黑体小四居中，每种名称各编各的号，内文
//!   楷体小四（行距 20 磅），左右各留一字、首行缩进两字。
//!
//! 块内一行一段（与 mdx 一致），每一行各自可点、各自上行号。引文、文框（含
//! 文框标题）里的行内公式与 PDF 一样排成公式，含公式的行交给 `math_flow` 混排。

use super::layout::{
    TextRun, clickable_rows, indent, job, justified_rows, layout, line_block_runs_spaced,
    mark_gutter_rows, paint_justified_rows, place, push_galley_tints, row_spans, text_format,
};
use super::{Metrics, RESEARCH_BODY_PT, RESEARCH_CAPTION_PT, marks, math_flow};
use crate::export::{self, MarkdownBlock, QuoteLine, QuoteLineKind};
use crate::theme;
use eframe::egui;
use egui::Align;
use egui::text::LayoutJob;
use std::ops::Range;

/// 引文左右各缩进两字（`\leftmargin` / `\rightmargin` = 2em，正文 14 磅）。
const CITATION_INSET_PT: f32 = 2.0 * RESEARCH_BODY_PT;
/// 文框内文行距（`\fontsize{12bp}{20pt}`）。
const BOX_LINE_PT: f32 = 20.0;
/// 文框内文左右各留一字（`leftsep` / `rightsep` = 1em，小四 12 磅）。
const BOX_INSET_PT: f32 = RESEARCH_CAPTION_PT;
/// 文框标题行上下的空（`abovesep=6pt, belowsep=4pt`）与末行下方的空（`belowsep=8pt`）。
const BOX_TITLE_ABOVE_PT: f32 = 6.0;
const BOX_TITLE_BELOW_PT: f32 = 4.0;
const BOX_BOTTOM_PT: f32 = 8.0;
/// 文框底色：纸色里掺 7% 墨色，对应 TeX 的 `black!7`；深色纸面同样浅一档。
const BOX_FILL_INK: f32 = 0.07;
/// 文框边框粗细（`0.6pt`）。
const BOX_RULE_PT: f32 = 0.6;

/// 引文。
pub(super) fn citation(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    block: &MarkdownBlock,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let MarkdownBlock::Quote { lines, .. } = block else {
        return;
    };
    if lines.is_empty() {
        return;
    }
    let style = Style {
        family: theme::FONT_KAITI,
        size: RESEARCH_BODY_PT,
        line: metrics.line,
        inset: metrics.pt(CITATION_INSET_PT),
        right_inset: metrics.pt(CITATION_INSET_PT),
        math: true,
    };
    ui.add_space(metrics.line * 0.5);
    quote_lines(
        ui,
        metrics,
        lines,
        &style,
        anchor,
        scroll_to_anchor,
        clicked,
    );
    ui.add_space(metrics.line * 0.5);
}

/// 文框：先占一个空图形位，内容排完量出高度再回填底色与边框，让底色压在字下面。
#[allow(clippy::too_many_arguments)]
pub(super) fn boxed(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    heading: &str,
    block: &MarkdownBlock,
    source: &Range<usize>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let MarkdownBlock::Quote {
        boxed: Some(boxed),
        lines,
    } = block
    else {
        return;
    };
    ui.add_space(metrics.line * 0.5);
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let top = ui.cursor().top();
    let left = ui.cursor().left();

    ui.add_space(metrics.pt(BOX_TITLE_ABOVE_PT));
    // 标题行对应源码的首行：块首到第一条内容行之前。
    let title_source = source.start
        ..lines
            .first()
            .map_or(source.end, |line| line.source.start)
            .max(source.start);
    // `heading` 是"案例 2.1"，与标题之间空一字（TeX 的 `\quad`）。
    let heading = if boxed.title.is_empty() {
        heading.to_string()
    } else {
        format!("{heading}\u{3000}{}", boxed.title)
    };
    clickable_rows(
        ui,
        metrics,
        &title_source,
        anchor,
        scroll_to_anchor,
        clicked,
        |ui| {
            if math_flow::has_math(&heading) {
                let style = math_flow::FlowStyle::heading(
                    metrics,
                    theme::FONT_HEITI,
                    RESEARCH_CAPTION_PT,
                    metrics.pt(BOX_LINE_PT),
                    Align::Center,
                    "",
                );
                math_flow::flow(ui, metrics, &heading, &style);
                return;
            }
            line_block_runs_spaced(
                ui,
                metrics,
                &[TextRun {
                    text: &heading,
                    family: theme::FONT_HEITI,
                    size: RESEARCH_CAPTION_PT,
                }],
                Align::Center,
                metrics.pt(BOX_LINE_PT),
            );
        },
    );
    ui.add_space(metrics.pt(BOX_TITLE_BELOW_PT));
    let style = Style {
        family: theme::FONT_KAITI,
        size: RESEARCH_CAPTION_PT,
        line: metrics.pt(BOX_LINE_PT),
        inset: metrics.pt(BOX_INSET_PT),
        right_inset: metrics.pt(BOX_INSET_PT),
        math: true,
    };
    quote_lines(
        ui,
        metrics,
        lines,
        &style,
        anchor,
        scroll_to_anchor,
        clicked,
    );
    ui.add_space(metrics.pt(BOX_BOTTOM_PT));

    let rect = egui::Rect::from_min_max(
        egui::pos2(left, top),
        egui::pos2(left + metrics.content, ui.cursor().top()),
    );
    let paper = theme::paper::bg();
    let ink = theme::paper::ink();
    ui.painter().set(
        backdrop,
        egui::Shape::Vec(vec![
            egui::Shape::rect_filled(rect, 0.0, paper.lerp_to_gamma(ink, BOX_FILL_INK)),
            egui::Shape::rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(metrics.pt(BOX_RULE_PT).max(1.0), ink),
                egui::StrokeKind::Middle,
            ),
        ]),
    );
    ui.add_space(metrics.line * 0.5);
}

/// 块内文字的字面与缩进。
struct Style {
    family: &'static str,
    size: f32,
    line: f32,
    /// 相对版心左沿缩进多少。
    inset: f32,
    /// 右侧让出多少。
    right_inset: f32,
    /// 行内公式排成公式还是印源码。
    math: bool,
}

/// 块内各行：一行一段，首行缩进两字，列表项前补 ⑴ ⑵，出处行靠右。
fn quote_lines(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    lines: &[QuoteLine],
    style: &Style,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let width = (metrics.content - style.inset - style.right_inset).max(1.0);
    let mut list_no = 0usize;
    for line in lines {
        if style.math && math_flow::has_math(&line.text) {
            let flow = flow_style(metrics, line.kind, &mut list_no, style, width);
            clickable_rows(
                ui,
                metrics,
                &line.source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| math_flow::flow(ui, metrics, &line.text, &flow),
            );
            continue;
        }
        let mut job = job(width);
        let font = metrics.font(style.family, style.size);
        match line.kind {
            QuoteLineKind::Source => {
                list_no = 0;
                job.halign = Align::Max;
            }
            kind => {
                job.append(&indent(2.0), 0.0, text_format(font.clone(), style.line));
                if kind == QuoteLineKind::ListItem {
                    list_no += 1;
                    job.append(
                        &list_label(list_no),
                        0.0,
                        text_format(metrics.font(theme::FONT_SONGTI, style.size), style.line),
                    );
                } else {
                    list_no = 0;
                }
            }
        }
        append_text(&mut job, metrics, &line.text, style);
        clickable_rows(
            ui,
            metrics,
            &line.source,
            anchor,
            scroll_to_anchor,
            clicked,
            |ui| inset_paragraph(ui, metrics, job, style.inset),
        );
    }
}

/// 含行内公式的一行交给 `math_flow` 混排：首行前缀、靠右与字面同 `quote_lines`，
/// 加粗换黑体、不套括号楷体规则，与 [`append_text`] 一致。
fn flow_style(
    metrics: &Metrics,
    kind: QuoteLineKind,
    list_no: &mut usize,
    style: &Style,
    width: f32,
) -> math_flow::FlowStyle {
    let normal = metrics.font(style.family, style.size);
    let mut lead = Vec::new();
    let align_right = kind == QuoteLineKind::Source;
    if align_right {
        *list_no = 0;
    } else {
        lead.push((indent(2.0), normal.clone()));
        if kind == QuoteLineKind::ListItem {
            *list_no += 1;
            lead.push((
                list_label(*list_no),
                metrics.font(theme::FONT_SONGTI, style.size),
            ));
        } else {
            *list_no = 0;
        }
    }
    math_flow::FlowStyle {
        normal,
        bold: metrics.font(theme::FONT_BOLD, style.size),
        paren: None,
        math_pt: style.size,
        line: style.line,
        left: style.inset,
        width,
        lead,
        align: if align_right { Align::Max } else { Align::Min },
    }
}

/// 块内第 `n` 个列表项的前缀，与正文一级列表相同（⑴ …… ⒇，再往后 (21)）。
fn list_label(n: usize) -> String {
    const PAREN_CIRCLED: &[char] = &[
        '⑴', '⑵', '⑶', '⑷', '⑸', '⑹', '⑺', '⑻', '⑼', '⑽', '⑾', '⑿', '⒀', '⒁', '⒂', '⒃', '⒄', '⒅',
        '⒆', '⒇',
    ];
    PAREN_CIRCLED
        .get(n.wrapping_sub(1))
        .map(|ch| format!("{ch} "))
        .unwrap_or_else(|| format!("({n}) "))
}

/// 行内文字：加粗换黑体，其余一律用块的字面（不套公文的括号楷体规则），
/// 花脸稿的增删标记照正文的办法打。
fn append_text(job: &mut LayoutJob, metrics: &Metrics, text: &str, style: &Style) {
    let normal = metrics.font(style.family, style.size);
    let bold = metrics.font(theme::FONT_BOLD, style.size);
    let mut previous = export::RedlineKind::Same;
    for chunk in export::redline_chunks(text) {
        let mut gap = marks::chunk_gap(metrics, previous, chunk.kind);
        for segment in export::inline_segments(&chunk.text) {
            let font = if segment.bold {
                bold.clone()
            } else {
                normal.clone()
            };
            job.append(
                &segment.text,
                std::mem::take(&mut gap),
                marks::mark_format(text_format(font, style.line), chunk.kind),
            );
            previous = chunk.kind;
        }
    }
}

/// 一段相对版心左沿缩进 `left` 的文字：两端对齐（末行除外）；`halign` 是
/// `Max` 时整段靠右。底色、行号、花脸稿标注与正文同一口径。
fn inset_paragraph(ui: &mut egui::Ui, metrics: &Metrics, job: LayoutJob, left: f32) {
    let right_aligned = job.halign == Align::Max;
    let width = job.wrap.max_width;
    let base = layout(ui, job.clone());
    let height = base.size().y;
    let spans = row_spans(&base);
    let rows = (!right_aligned).then(|| justified_rows(ui, &job, &base));
    place(ui, metrics, height, |painter, rect| {
        match rows {
            Some(rows) => {
                let origin = egui::pos2(rect.left() + left, rect.top());
                push_galley_tints(metrics, &base, origin);
                paint_justified_rows(painter, metrics, origin, &job, &base, rows);
            }
            // halign 让每行相对 galley 原点靠右，原点放在可用宽度的右沿上。
            None => {
                let origin = egui::pos2(rect.left() + left + width, rect.top());
                push_galley_tints(metrics, &base, origin);
                marks::paint_galley_marks(painter, metrics, origin, &base);
                painter.galley(origin, base, theme::paper::ink());
            }
        }
        mark_gutter_rows(metrics, rect, &spans);
    });
}
