//! 版本对照左栏的可编辑统一 diff（第 ③ 期）：编辑器就是 diff 本身。
//!
//! - 正文照常在 `TextEdit` 里编辑，排版交给源码模式同一套高亮（`MarkdownHighlighter`）；
//! - 已删除 / 被改写的旧行画在行间空隙里（`diff_gaps`：布局器把之后的行往下挪），
//!   红底、只读、不可选中，行号槽写旧版行号；
//! - 新增 / 改写后的新行铺绿底，真正改掉的字再加深一档；
//! - 每个变更块（hunk）悬停或聚焦时右上角出「还原」按钮。
//!
//! 数据（变更块、还原）在 `diff_hunks`；本文件只管画和收集交互，不改状态。

use super::diff_gaps::{Gap, gap_spans, line_first_rows, open_gaps};
use super::diff_hunks::{DeletedRow, Hunk};
use super::editor::editor_id;
use crate::diff::{InlineSpan, SpanKind};
use crate::highlight::{self, MarkdownHighlighter};
use crate::models::EditorFontScheme;
use crate::theme;
use eframe::egui;
use eframe::egui::epaint::text::Galley;
use eframe::egui::text::LayoutJob;
use std::ops::Range;
use std::sync::Arc;

/// 行号槽宽：行号 + 增删色条。
const GUTTER: f32 = 48.0;
/// 色条离正文左沿的距离与宽度。
const BAR_GAP: f32 = 8.0;
const BAR_WIDTH: f32 = 3.0;

/// 一次绘制的输入。
pub(crate) struct DiffEditorInput<'a> {
    pub(crate) hunks: &'a [Hunk],
    /// 当前焦点变更块（描边、常驻还原按钮）。
    pub(crate) focus: Option<usize>,
    /// 右栏预览悬停对应的变更块（淡色描边）。
    pub(crate) highlight: Option<usize>,
    pub(crate) editable: bool,
    pub(crate) font_size: f32,
    pub(crate) fonts: &'a EditorFontScheme,
    pub(crate) research: bool,
}

/// 一次绘制里收集到的交互。
#[derive(Default)]
pub(crate) struct DiffEditorOutput {
    /// 这一帧用户改了字。
    pub(crate) changed: bool,
    /// 编辑框有焦点时光标所在的源码行（0 基）。
    pub(crate) cursor_line: Option<usize>,
    /// 鼠标悬停的变更块。
    pub(crate) hovered: Option<usize>,
    /// 点了「还原」的变更块。
    pub(crate) revert: Option<usize>,
    /// 编辑框本体的输出，调用方拿它做光标跳转（`find::jump_to_source`）。
    pub(crate) output: Option<egui::text_edit::TextEditOutput>,
}

/// 画可编辑的统一 diff。调用方负责套滚动区。
pub(crate) fn diff_editor(
    ui: &mut egui::Ui,
    text: &mut String,
    highlighter: &mut MarkdownHighlighter,
    input: DiffEditorInput<'_>,
) -> DiffEditorOutput {
    let DiffEditorInput {
        hunks,
        focus,
        highlight: preview_highlight,
        editable,
        font_size,
        fonts,
        research,
    } = input;
    let width = ui.available_width().max(GUTTER + 160.0);
    let text_width = width - GUTTER;
    // 绿底要压在字下面：先占位，排完版量出行矩形再回填（egui 常用的占位手法）。
    let backdrop = ui.painter().add(egui::Shape::Noop);

    // 有旧行要放的变更块；布局器按它们开空隙，画的时候用同一份，保证本帧一致。
    let gapped: Vec<usize> = hunks
        .iter()
        .enumerate()
        .filter(|(_, hunk)| !hunk.deleted.is_empty())
        .map(|(index, _)| index)
        .collect();
    let mut deleted_galleys: Vec<Vec<Arc<Galley>>> = Vec::new();
    let mut gaps: Vec<Gap> = Vec::new();
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap: f32| {
        let base = highlighter.layout(
            ui,
            buffer.as_str(),
            wrap,
            font_size,
            None,
            &[],
            fonts,
            research,
        );
        deleted_galleys = gapped
            .iter()
            .map(|&index| {
                hunks[index]
                    .deleted
                    .iter()
                    .map(|row| deleted_galley(ui, row, wrap, font_size, fonts, research))
                    .collect()
            })
            .collect();
        gaps = gapped
            .iter()
            .zip(&deleted_galleys)
            .map(|(&index, galleys)| Gap {
                line: hunks[index].gap_line,
                height: galleys.iter().map(|galley| galley.size().y).sum(),
            })
            .collect();
        Arc::new(open_gaps(&base, &gaps))
    };

    let output = ui
        .horizontal_top(|ui| {
            ui.add_space(GUTTER);
            egui::TextEdit::multiline(text)
                .id(editor_id())
                .interactive(editable)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .code_editor()
                .layouter(&mut layouter)
                .desired_width(text_width)
                .desired_rows(4)
                .show(ui)
        })
        .inner;

    let galley = output.galley.clone();
    let origin = output.galley_pos;
    let left = origin.x - GUTTER;
    let right = origin.x + text_width;
    let firsts = line_first_rows(&galley);
    let line_span = |line: usize| -> Option<(f32, f32)> {
        let first = *firsts.get(line)?;
        let end = firsts.get(line + 1).copied().unwrap_or(galley.rows.len());
        let rows = galley.rows.get(first..end)?;
        Some((rows.first()?.min_y(), rows.last()?.max_y()))
    };
    let spans = gap_spans(&galley, &gaps);
    let line_chars = line_char_starts(text);

    // —— 字下面：新增行绿底、改掉的字加深 ——
    let mut under = Vec::new();
    for hunk in hunks {
        for row in &hunk.added {
            let Some((top, bottom)) = line_span(row.line) else {
                continue;
            };
            under.push(egui::Shape::rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(left, origin.y + top),
                    egui::pos2(right, origin.y + bottom),
                ),
                0.0,
                theme::success_soft(),
            ));
            let Some(&line_start) = line_chars.get(row.line) else {
                continue;
            };
            for range in span_char_ranges(&row.spans, SpanKind::Added) {
                let range = line_start + range.start..line_start + range.end;
                for rect in char_rects(&galley, range) {
                    under.push(egui::Shape::rect_filled(
                        rect.translate(origin.to_vec2()),
                        2.0,
                        theme::success().gamma_multiply(0.28),
                    ));
                }
            }
        }
    }
    ui.painter().set(backdrop, egui::Shape::Vec(under));

    // —— 空隙里：已删除的旧行 ——
    // 克隆一份画笔：后面还要往 `ui` 上放还原按钮。
    let painter = ui.painter().clone();
    let number_font = egui::TextStyle::Monospace.resolve(ui.style());
    for ((&index, galleys), &(top, bottom)) in gapped.iter().zip(&deleted_galleys).zip(&spans) {
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(left, origin.y + top),
                egui::pos2(right, origin.y + bottom),
            ),
            0.0,
            theme::danger_soft(),
        );
        let mut y = origin.y + top;
        for (row, galley) in hunks[index].deleted.iter().zip(galleys) {
            painter.text(
                egui::pos2(origin.x - BAR_GAP - 6.0, y + 1.0),
                egui::Align2::RIGHT_TOP,
                row.old_line.to_string(),
                number_font.clone(),
                theme::text_muted(),
            );
            painter.galley(egui::pos2(origin.x, y), galley.clone(), theme::text());
            y += galley.size().y;
        }
        paint_bar(&painter, origin.x, top, bottom, origin.y, theme::danger());
    }

    // —— 行号槽：新版行号 + 新增色条 ——
    for (line, &first) in firsts.iter().enumerate() {
        let Some(row) = galley.rows.get(first) else {
            continue;
        };
        painter.text(
            egui::pos2(origin.x - BAR_GAP - 6.0, origin.y + row.min_y() + 1.0),
            egui::Align2::RIGHT_TOP,
            (line + 1).to_string(),
            number_font.clone(),
            theme::text_muted(),
        );
    }
    let source_lines: Vec<&str> = text.split('\n').collect();
    for hunk in hunks {
        for row in &hunk.added {
            if let Some((top, bottom)) = line_span(row.line) {
                paint_bar(&painter, origin.x, top, bottom, origin.y, theme::success());
            }
            // 新增的是空行：绿底空行看不出加了什么，与删除的空行一样补一个淡色
            // 「（空行）」。它不是文本，只画在行尾，不占光标位置；一打字就消失。
            let blank = source_lines
                .get(row.line)
                .is_some_and(|line| crate::diff::is_blank_line(line));
            if blank && let Some(first) = firsts.get(row.line) {
                let placed = &galley.rows[*first];
                let mut job = egui::text::LayoutJob::default();
                crate::diff_view::blank_placeholder(
                    &mut job,
                    egui::FontId::proportional(font_size),
                );
                let label = painter.layout_job(job);
                painter.galley(
                    egui::pos2(origin.x + placed.rect().right(), origin.y + placed.min_y()),
                    label,
                    theme::text_muted(),
                );
            }
        }
    }

    // —— 变更块：悬停 / 焦点描边与「还原」按钮 ——
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let mut result = DiffEditorOutput {
        changed: output.response.changed(),
        cursor_line: output
            .response
            .has_focus()
            .then(|| {
                output
                    .cursor_range
                    .map(|range| super::editor::source_line_at_char(text, range.primary.index.0))
            })
            .flatten(),
        ..Default::default()
    };
    for (index, hunk) in hunks.iter().enumerate() {
        let gap = gapped
            .iter()
            .position(|&gapped| gapped == index)
            .and_then(|slot| spans.get(slot).copied());
        let added = hunk
            .added
            .iter()
            .filter_map(|row| line_span(row.line))
            .fold(None, |acc: Option<(f32, f32)>, (top, bottom)| {
                Some(acc.map_or((top, bottom), |(a, b)| (a.min(top), b.max(bottom))))
            });
        let Some((top, bottom)) = [gap, added]
            .into_iter()
            .flatten()
            .reduce(|(a, b), (top, bottom)| (a.min(top), b.max(bottom)))
        else {
            continue;
        };
        let rect = egui::Rect::from_min_max(
            egui::pos2(left, origin.y + top),
            egui::pos2(right, origin.y + bottom),
        );
        let hovered = pointer.is_some_and(|pointer| rect.contains(pointer));
        if hovered {
            result.hovered = Some(index);
        }
        let focused = focus == Some(index);
        let outline = if focused {
            Some(egui::Stroke::new(1.5, theme::accent()))
        } else if hovered || preview_highlight == Some(index) {
            Some(egui::Stroke::new(1.0, theme::accent().gamma_multiply(0.5)))
        } else {
            None
        };
        if let Some(stroke) = outline {
            painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
        }
        if editable && (hovered || focused) {
            let button = egui::Rect::from_min_size(
                egui::pos2(right - 24.0, rect.top() + 1.0),
                egui::vec2(22.0, 20.0),
            );
            if ui
                .put(
                    button,
                    egui::Button::image(theme::Icon::RotateCcw.image())
                        .frame(false)
                        .image_tint_follows_text_color(true),
                )
                .on_hover_text("还原这一块：换回基准版的内容（Ctrl+Z 可撤回）")
                .clicked()
            {
                result.revert = Some(index);
            }
        }
    }
    result.output = Some(output);
    result
}

/// 行号槽里的一条增删色条。
fn paint_bar(
    painter: &egui::Painter,
    text_left: f32,
    top: f32,
    bottom: f32,
    origin_y: f32,
    color: egui::Color32,
) {
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(text_left - BAR_GAP, origin_y + top),
            egui::pos2(text_left - BAR_GAP + BAR_WIDTH, origin_y + bottom),
        ),
        0.0,
        color,
    );
}

/// 一条已删除旧行的排版：与编辑器同一套高亮与字号，真正删掉的字加深底色。
fn deleted_galley(
    ui: &egui::Ui,
    row: &DeletedRow,
    wrap: f32,
    font_size: f32,
    fonts: &EditorFontScheme,
    research: bool,
) -> Arc<Galley> {
    let text = row.text();
    let mut job = highlight::highlight(&text, wrap, font_size, None, &[], fonts, research);
    let ranges = span_byte_ranges(&row.spans, SpanKind::Removed);
    apply_background(&mut job, &ranges, theme::danger().gamma_multiply(0.28));
    // 删掉的是空行：红底空行看不出删了什么，补一个淡色「（空行）」。
    crate::diff_view::blank_placeholder(&mut job, egui::FontId::proportional(font_size));
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

/// 字级片段里某一类在整行里的字节范围。
fn span_byte_ranges(spans: &[InlineSpan], kind: SpanKind) -> Vec<Range<usize>> {
    let mut offset = 0usize;
    let mut out = Vec::new();
    for span in spans {
        let end = offset + span.text.len();
        if span.kind == kind {
            out.push(offset..end);
        }
        offset = end;
    }
    out
}

/// 同上，按字符计（egui 光标按字符）。
fn span_char_ranges(spans: &[InlineSpan], kind: SpanKind) -> Vec<Range<usize>> {
    let mut offset = 0usize;
    let mut out = Vec::new();
    for span in spans {
        let end = offset + span.text.chars().count();
        if span.kind == kind {
            out.push(offset..end);
        }
        offset = end;
    }
    out
}

/// 给排版任务里落在 `ranges`（字节）内的文字铺底色：按范围边界切开 section。
fn apply_background(job: &mut LayoutJob, ranges: &[Range<usize>], color: egui::Color32) {
    if ranges.is_empty() {
        return;
    }
    let mut cuts: Vec<usize> = ranges
        .iter()
        .flat_map(|range| [range.start, range.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut sections = Vec::with_capacity(job.sections.len() + cuts.len());
    for section in std::mem::take(&mut job.sections) {
        let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
        let mut piece_start = start;
        let inner = cuts
            .iter()
            .copied()
            .filter(|cut| *cut > start && *cut < end);
        for piece_end in inner.chain(std::iter::once(end)) {
            let mut piece = section.clone();
            piece.byte_range = egui::text::ByteIndex(piece_start)..egui::text::ByteIndex(piece_end);
            if piece_start != start {
                piece.leading_space = 0.0;
            }
            if ranges
                .iter()
                .any(|range| range.start <= piece_start && piece_start < range.end)
            {
                piece.format.background = color;
            }
            sections.push(piece);
            piece_start = piece_end;
        }
    }
    job.sections = sections;
}

/// 每条源码行首字符的字符下标。
fn line_char_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, ch) in text.chars().enumerate() {
        if ch == '\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// 一段字符范围在 galley 里逐行的矩形（galley 坐标）。
fn char_rects(galley: &Galley, range: Range<usize>) -> Vec<egui::Rect> {
    let mut rects = Vec::new();
    let mut row_start = 0usize;
    for row in &galley.rows {
        let count = row.glyphs.len();
        let start = range.start.max(row_start);
        let end = range.end.min(row_start + count);
        if start < end {
            let first = &row.glyphs[start - row_start];
            let last = &row.glyphs[end - row_start - 1];
            rects.push(egui::Rect::from_min_max(
                egui::pos2(row.pos.x + first.pos.x, row.min_y()),
                egui::pos2(row.pos.x + last.max_x(), row.max_y()),
            ));
        }
        row_start += count + usize::from(row.ends_with_newline);
    }
    rects
}

/// 用基准版的一块替换正文，并在前后各记一个撤销点：Ctrl+Z 一次就回到还原前
/// （egui 的撤销器只在状态稳定一秒后才自动存点，外部改字不插点就会和之前的
/// 输入并成一步）。光标落在 `cursor_byte`。
pub(crate) fn replace_with_undo(
    ctx: &egui::Context,
    text: &mut String,
    replacement: String,
    cursor_byte: usize,
) {
    use egui::text::{CCursor, CCursorRange};
    let mut state = egui::TextEdit::load_state(ctx, editor_id()).unwrap_or_default();
    let before = state
        .cursor
        .char_range()
        .unwrap_or(CCursorRange::one(CCursor::new(0)));
    let mut undoer = state.undoer();
    undoer.add_undo(&(before, text.clone()));
    *text = replacement;
    let mut cursor_byte = cursor_byte.min(text.len());
    while !text.is_char_boundary(cursor_byte) {
        cursor_byte -= 1;
    }
    let cursor = CCursorRange::one(CCursor::new(text[..cursor_byte].chars().count()));
    undoer.add_undo(&(cursor, text.clone()));
    state.set_undoer(undoer);
    state.cursor.set_char_range(Some(cursor));
    state.store(ctx, editor_id());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backgrounds_split_sections_at_range_edges() {
        let mut job = LayoutJob::default();
        job.append("甲乙丙丁", 0.0, egui::TextFormat::default());
        let red = egui::Color32::RED;
        // 「乙丙」：字节 3..9。
        let middle = 3..9;
        apply_background(&mut job, std::slice::from_ref(&middle), red);
        let pieces: Vec<(String, bool)> = job
            .sections
            .iter()
            .map(|section| {
                (
                    job.text[section.byte_range.start.0..section.byte_range.end.0].to_string(),
                    section.format.background == red,
                )
            })
            .collect();
        assert_eq!(
            pieces,
            [
                ("甲".to_string(), false),
                ("乙丙".to_string(), true),
                ("丁".to_string(), false)
            ]
        );
    }

    #[test]
    fn span_ranges_count_bytes_and_chars_separately() {
        let spans = [
            InlineSpan {
                kind: SpanKind::Same,
                text: "请于八月".into(),
            },
            InlineSpan {
                kind: SpanKind::Added,
                text: "十五".into(),
            },
        ];
        let (bytes, chars) = (12..18, 4..6);
        assert_eq!(span_byte_ranges(&spans, SpanKind::Added), [bytes]);
        assert_eq!(span_char_ranges(&spans, SpanKind::Added), [chars]);
    }
}
