//! 编辑器绘制：行号、混合装饰、Markdown 编辑器、版式渲染与审校提示。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::editor`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::visible_rows;
use crate::draft_page::candidates;
use crate::draft_page::caret::show_with_glyph_caret;
use crate::draft_page::markdown::byte_at_char;
use crate::draft_page::{
    DraftPage, OFFICIAL_BODY_SIZE, OFFICIAL_EDITOR_CONTENT_WIDTH, OFFICIAL_PAGE_HEIGHT,
    OFFICIAL_PAGE_MARGIN_LEFT, OFFICIAL_PAGE_MARGIN_TOP, OFFICIAL_PAGE_WIDTH, PreviewMode,
    PreviewScroll, continue_ordered_list, editor_cursor, editor_selection, is_table_separator_line,
    is_table_source_line, jump_to_source, markdown_heading_level, markdown_matches_mode,
    select_source_range, table_column_count,
};
use crate::export;
use crate::highlight::ordered_list_lines;
use crate::models::{EDITOR_FONT_SIZE_MAX, EDITOR_FONT_SIZE_MIN, NumberingConfig};
use crate::preview;
use crate::storage;
use crate::theme;
use crate::units::UnitDisplay;
use eframe::egui;
use std::cell::RefCell;
use std::ops::Range;
use std::sync::Arc;

/// 当前帧 TextEdit 的实际选区；拖拽期间不能只读上一帧存下的光标状态。
fn output_selection(text: &str, output: &egui::text_edit::TextEditOutput) -> Option<Range<usize>> {
    let range = output.cursor_range?;
    let primary = byte_at_char(text, range.primary.index.0);
    let secondary = byte_at_char(text, range.secondary.index.0);
    (primary != secondary).then_some(primary.min(secondary)..primary.max(secondary))
}

/// TextEdit 目前只支持拖动选区端点，按在已有选区里仍会重新选字。
/// 这里保存按下时的选区，在松开前恢复它，并在落点确定后一次性移动正文。
pub(crate) struct TextDrag {
    source: Range<usize>,
    original: String,
    press_cursor: egui::text::CCursor,
    press_pos: egui::Pos2,
    moved: bool,
}

fn restore_drag_selection(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    range: egui::text::CCursorRange,
) {
    let mut state = output.state.clone();
    state.cursor.set_char_range(Some(range));
    state.store(ui.ctx(), editor_id());
}

/// 从屏幕坐标换算成 Markdown 字符位置，软换行和实时排版复用 TextEdit 的 galley。
fn drag_cursor_at(
    output: &egui::text_edit::TextEditOutput,
    pos: egui::Pos2,
) -> egui::text::CCursor {
    output.galley.cursor_from_pos(pos - output.galley_pos)
}

struct DropLocation {
    byte: usize,
    caret: egui::Rect,
}

/// 落点只认编辑框的可见区域，避免鼠标移到侧栏或滚动区外时误插入。
fn drag_drop_location(
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    visible: egui::Rect,
    pos: egui::Pos2,
) -> Option<DropLocation> {
    if !visible.contains(pos) {
        return None;
    }
    let cursor = drag_cursor_at(output, pos);
    Some(DropLocation {
        byte: byte_at_char(text, cursor.index.0),
        caret: output
            .galley
            .pos_from_cursor(cursor)
            .translate(output.galley_pos.to_vec2()),
    })
}

/// 浮卡只显示一个短摘录；换行用可见符号表示，不让长段正文盖住落点。
fn drag_excerpt(selected: &str) -> (String, String) {
    let mut excerpt = String::new();
    let mut shown = 0usize;
    for ch in selected.chars() {
        if ch == '\r' {
            continue;
        }
        if shown == 18 {
            excerpt.push('…');
            break;
        }
        excerpt.push(if ch == '\n' { '↵' } else { ch });
        shown += 1;
    }
    let lines = selected.split('\n').count();
    let chars = selected.chars().filter(|ch| !ch.is_whitespace()).count();
    let detail = if lines > 1 {
        format!("{lines} 行 · {chars} 字")
    } else {
        format!("{chars} 字")
    };
    (excerpt, detail)
}

fn drag_card_rect(pointer: egui::Pos2, size: egui::Vec2, screen: egui::Rect) -> egui::Rect {
    let mut left = pointer.x + 16.0;
    if left + size.x > screen.right() - 8.0 {
        left = pointer.x - size.x - 16.0;
    }
    let mut top = pointer.y + 18.0;
    if top + size.y > screen.bottom() - 8.0 {
        top = pointer.y - size.y - 14.0;
    }
    let left = left.clamp(
        screen.left() + 8.0,
        (screen.right() - size.x - 8.0).max(screen.left() + 8.0),
    );
    let top = top.clamp(
        screen.top() + 8.0,
        (screen.bottom() - size.y - 8.0).max(screen.top() + 8.0),
    );
    egui::Rect::from_min_size(egui::pos2(left, top), size)
}

/// 浮卡不参与命中测试；鼠标点仍直接交给编辑器的 galley 决定落点。
fn paint_drag_card(
    ctx: &egui::Context,
    pointer: egui::Pos2,
    selected: &str,
    status: &str,
    valid: bool,
) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("gw-markdown-drag-card"),
    ));
    let (excerpt, detail) = drag_excerpt(selected);
    let first = painter.layout_no_wrap(excerpt, egui::FontId::proportional(13.0), theme::text());
    let second = painter.layout_no_wrap(
        format!("{detail} · {status}"),
        egui::FontId::proportional(11.0),
        theme::text_muted(),
    );
    let size = egui::vec2(
        first.size().x.max(second.size().x) + 24.0,
        first.size().y + second.size().y + 20.0,
    );
    let rect = drag_card_rect(pointer, size, ctx.content_rect());
    painter.rect_filled(
        rect.translate(egui::vec2(0.0, 3.0)),
        7.0,
        egui::Color32::from_black_alpha(32),
    );
    painter.rect_filled(rect, 7.0, theme::surface());
    painter.rect_stroke(
        rect,
        7.0,
        egui::Stroke::new(
            1.0,
            if valid {
                theme::accent()
            } else {
                theme::border_strong()
            },
        ),
        egui::StrokeKind::Inside,
    );
    let second_y = 10.0 + first.size().y;
    painter.galley(rect.min + egui::vec2(12.0, 8.0), first, theme::text());
    painter.galley(
        rect.min + egui::vec2(12.0, second_y),
        second,
        theme::text_muted(),
    );
}

fn scroll_drag_edge(ui: &egui::Ui, visible: egui::Rect, pos: egui::Pos2) {
    if pos.x < visible.left()
        || pos.x > visible.right()
        || pos.y < visible.top() - 20.0
        || pos.y > visible.bottom() + 20.0
    {
        return;
    }
    let band = 28.0;
    let near_top = ((visible.top() + band - pos.y) / band).clamp(0.0, 1.0);
    let near_bottom = ((pos.y - visible.bottom() + band) / band).clamp(0.0, 1.0);
    let delta = (near_top - near_bottom) * 14.0;
    if delta.abs() > 0.5 {
        ui.scroll_with_delta(egui::vec2(0.0, delta));
    }
}

fn paint_drop_marker(ui: &egui::Ui, visible: egui::Rect, caret: egui::Rect) {
    let painter = ui.painter_at(visible);
    let x = caret.left();
    let top = caret.top();
    painter.line_segment(
        [egui::pos2(x, top + 4.0), egui::pos2(x, caret.bottom())],
        egui::Stroke::new(2.0, theme::accent()),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - 4.0, top),
            egui::pos2(x + 4.0, top),
            egui::pos2(x, top + 5.0),
        ],
        theme::accent(),
        egui::Stroke::NONE,
    ));
}

/// 返回移动后的全文与新选区。落在原选区内（含两端）不改正文。
fn moved_text(text: &str, source: Range<usize>, target: usize) -> Option<(String, Range<usize>)> {
    if source.is_empty()
        || source.end > text.len()
        || target > text.len()
        || !text.is_char_boundary(source.start)
        || !text.is_char_boundary(source.end)
        || !text.is_char_boundary(target)
        || (source.start..=source.end).contains(&target)
    {
        return None;
    }
    let selected = &text[source.clone()];
    let insert_at = if target > source.end {
        target - selected.len()
    } else {
        target
    };
    let mut updated = text.to_owned();
    updated.replace_range(source, "");
    updated.insert_str(insert_at, selected);
    Some((updated, insert_at..insert_at + selected.len()))
}

/// 每帧在 TextEdit 绘制后处理按下、拖动、松开；返回需要提交的单次编辑。
fn handle_text_drag(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    drag: &mut Option<TextDrag>,
    before: Option<egui::text::CCursorRange>,
    editable: bool,
    paint: Option<(&Arc<egui::Galley>, egui::Color32)>,
) -> Option<(String, Range<usize>)> {
    if !editable {
        *drag = None;
        return None;
    }
    let (pressed, down, released, pos, shift) = ui.input(|input| {
        (
            input.pointer.button_pressed(egui::PointerButton::Primary),
            input.pointer.button_down(egui::PointerButton::Primary),
            input.pointer.button_released(egui::PointerButton::Primary),
            input.pointer.latest_pos(),
            input.modifiers.shift,
        )
    });
    let visible = output.response.rect.intersect(ui.clip_rect());
    if pressed
        && !shift
        && let (Some(pos), Some(range)) = (pos, before.filter(|range| !range.is_empty()))
        && visible.contains(pos)
    {
        let cursor = drag_cursor_at(output, pos);
        let [start, end] = range.sorted_cursors();
        if (start.index.0..end.index.0).contains(&cursor.index.0) {
            *drag = Some(TextDrag {
                source: byte_at_char(text, start.index.0)..byte_at_char(text, end.index.0),
                original: text.to_owned(),
                press_cursor: cursor,
                press_pos: pos,
                moved: false,
            });
        }
    }
    if drag.is_none()
        && let (Some(pos), Some(range)) = (pos, before.filter(|range| !range.is_empty()))
        && visible.contains(pos)
    {
        let index = drag_cursor_at(output, pos).index.0;
        let [start, end] = range.sorted_cursors();
        if (start.index.0..end.index.0).contains(&index) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }
    }
    let active = drag.as_mut()?;
    if active.original != text {
        *drag = None;
        return None;
    }
    let start = egui::text::CCursor::new(text[..active.source.start].chars().count());
    let end = egui::text::CCursor::new(text[..active.source.end].chars().count());
    let selection = egui::text::CCursorRange::two(start, end);
    if down {
        if pos.is_some_and(|pos| pos.distance(active.press_pos) > 4.0) {
            active.moved = true;
        }
        restore_drag_selection(ui, output, selection);
        // TextEdit 本帧已画了它自己的拖选范围。用布局器留的干净 galley
        // 覆画原选区，避免源文字在拖动中消失或出现另一块错误选区。
        if let Some((clean_galley, background)) = paint {
            let painter = ui.painter_at(output.text_clip_rect);
            painter.rect_filled(output.text_clip_rect, 0.0, background);
            let mut galley = Arc::clone(clean_galley);
            egui::text_selection::visuals::paint_text_selection(
                &mut galley,
                ui.visuals(),
                &selection,
                None,
            );
            painter.galley(
                output.galley_pos - egui::vec2(galley.rect.left(), 0.0),
                galley,
                theme::text(),
            );
        }
        if active.moved
            && let Some(pos) = pos
        {
            scroll_drag_edge(ui, visible, pos);
            let location = drag_drop_location(output, text, visible, pos);
            let valid = location.as_ref().is_some_and(|location| {
                !(active.source.start..=active.source.end).contains(&location.byte)
            });
            if valid && let Some(location) = location {
                paint_drop_marker(ui, visible, location.caret);
            }
            let status = if valid {
                "移到此处"
            } else if visible.contains(pos) {
                "原位置"
            } else {
                "移出编辑区"
            };
            paint_drag_card(
                ui.ctx(),
                pos,
                &active.original[active.source.clone()],
                status,
                valid,
            );
            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
        }
        ui.ctx().request_repaint();
        return None;
    }
    let active = drag.take().unwrap();
    if !released {
        return None;
    }
    if !active.moved {
        restore_drag_selection(
            ui,
            output,
            egui::text::CCursorRange::one(active.press_cursor),
        );
        return None;
    }
    restore_drag_selection(ui, output, selection);
    let location = drag_drop_location(output, text, visible, pos?)?;
    moved_text(text, active.source, location.byte)
}

#[cfg(test)]
mod text_drag_tests {
    use super::*;

    #[test]
    fn moving_chinese_selection_adjusts_the_destination() {
        let text = "甲乙丙丁戊";
        assert_eq!(
            moved_text(text, 3..9, text.len()),
            Some(("甲丁戊乙丙".to_owned(), 9..15))
        );
        assert_eq!(
            moved_text(text, 6..12, 0),
            Some(("丙丁甲乙戊".to_owned(), 0..6))
        );
        assert_eq!(moved_text(text, 3..9, 6), None);
        assert_eq!(moved_text(text, 3..9, 3), None);
        assert_eq!(moved_text(text, 3..9, 9), None);
        assert_eq!(moved_text(text, 1..9, 0), None);
    }

    #[test]
    fn drag_card_stays_on_screen_and_summarizes_long_text() {
        let (excerpt, detail) = drag_excerpt("第一行\n第二行很长很长很长很长很长很长很长");
        assert!(excerpt.contains('↵'));
        assert!(excerpt.ends_with('…'));
        assert!(detail.starts_with("2 行"));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let card = drag_card_rect(egui::pos2(390.0, 290.0), egui::vec2(180.0, 44.0), screen);
        assert!(screen.contains_rect(card));
        assert!(card.right() < 390.0);
        assert!(card.bottom() < 290.0);
    }

    #[test]
    fn pointer_drag_moves_existing_selection() {
        let ctx = egui::Context::default();
        let mut text = "甲乙丙丁".to_owned();
        let mut drag = None;
        let mut clock = 0.0;
        {
            let mut frame = |events: Vec<egui::Event>| {
                clock += 0.05;
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 300.0),
                    )),
                    time: Some(clock),
                    events,
                    ..Default::default()
                };
                let mut positions = [egui::Pos2::ZERO; 3];
                let output = ctx.clone().run_ui(raw, |ui| {
                    let before = candidates::selection_before_show(ui.ctx(), editor_id());
                    let output = egui::TextEdit::multiline(&mut text)
                        .id(editor_id())
                        .desired_width(400.0)
                        .show(ui);
                    for (slot, index) in [(0, 1), (1, 2), (2, 4)] {
                        positions[slot] = output
                            .galley
                            .pos_from_cursor(egui::text::CCursor::new(index))
                            .min
                            + output.galley_pos.to_vec2()
                            + egui::vec2(2.0, 5.0);
                    }
                    if let Some((updated, range)) =
                        handle_text_drag(ui, &output, &text, &mut drag, before, true, None)
                    {
                        crate::draft_page::diff_editor::replace_with_undo(
                            ui.ctx(),
                            &mut text,
                            updated,
                            range.end,
                        );
                    }
                });
                (positions, output, text.clone())
            };
            let [start, middle, end] = frame(Vec::new()).0;
            let mut state = egui::TextEdit::load_state(&ctx, editor_id()).unwrap();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(1),
                    egui::text::CCursor::new(3),
                )));
            state.store(&ctx, editor_id());
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
            let (_, inside, _) = frame(vec![egui::Event::PointerMoved(middle)]);
            assert!(inside.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("原位置"))
            }));
            assert_eq!(frame(vec![button(middle, false)]).2, "甲乙丙丁");
            let outside = egui::pos2(500.0, 250.0);
            frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
            let (_, outside_frame, _) = frame(vec![egui::Event::PointerMoved(outside)]);
            assert!(outside_frame.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("移出编辑区"))
            }));
            assert_eq!(frame(vec![button(outside, false)]).2, "甲乙丙丁");
            frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
            let (_, dragging, _) = frame(vec![egui::Event::PointerMoved(end)]);
            assert!(dragging.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("移到此处"))
            }));
            frame(vec![button(end, false)]);
        }
        assert_eq!(text, "甲丁乙丙");
    }
}

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

/// 编辑光标所在 Markdown 源码行的字节范围，不包含行尾换行符。
pub(crate) fn source_line_range_at_char(text: &str, char_index: usize) -> Range<usize> {
    let offset = text
        .char_indices()
        .nth(char_index)
        .map_or(text.len(), |(offset, _)| offset);
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index);
    let current = start..end;
    if text
        .get(current.clone())
        .is_some_and(|line| !line.trim().is_empty())
    {
        return current;
    }
    let lines = export::source_lines(text);
    lines
        .iter()
        .find(|(line_start, line)| *line_start > start && !line.trim().is_empty())
        .or_else(|| {
            lines
                .iter()
                .rev()
                .find(|(line_start, line)| *line_start < start && !line.trim().is_empty())
        })
        .map_or(current, |(line_start, line)| {
            *line_start..*line_start + line.len()
        })
}

pub(crate) fn active_source_line(ctx: &egui::Context, text: &str) -> usize {
    egui::TextEdit::load_state(ctx, editor_id())
        .and_then(|state| state.cursor.char_range())
        .map_or(0, |range| source_line_at_char(text, range.primary.index.0))
}

#[derive(Clone, Copy)]
pub(crate) struct EditorLineVisual {
    pub(crate) top: f32,
    pub(crate) bottom: f32,
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
/// 行号字号跟随编辑器正文字号（略小两档），避免调大正文后行号显得突兀；
/// `family` 由调用方按模式给定——源码模式用编辑器字体族，行号和正文的数字
/// 才是同一副字面，实时排版模式的行号是纸面外的界面元素，仍用界面字体。
pub(crate) fn paint_editor_line_numbers(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    font_size: f32,
    family: egui::FontFamily,
) {
    let x = output.galley_pos.x - 10.0;
    let painter = ui.painter();
    let font = egui::FontId::new(font_size, family);
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
    numbering: &NumberingConfig,
) {
    let visuals = editor_line_visuals(output);
    let source_lines = text.split('\n').collect::<Vec<_>>();
    let ordered_lines = ordered_list_lines(text);
    let painter = ui.painter();
    let mut counters = export::HeadingCounters::with_numbering(*numbering);
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
                    theme::paper::ink(),
                );
            }
            continue;
        }
        if index != active_line
            && let (Some(info), Some(visual)) = (ordered_lines[index], visuals.get(index))
        {
            let label = if info.inline {
                export::render_list_number(numbering.list1, info.number)
            } else {
                export::render_list_number(numbering.list2, info.number)
            };
            let font = egui::FontId::new(
                OFFICIAL_BODY_SIZE,
                theme::official_family(theme::FONT_FANGSONG),
            );
            let label_galley = painter.layout_no_wrap(label, font, theme::paper::ink());
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
                theme::paper::ink(),
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
        let prefix_galley = painter.layout_no_wrap(prefix, font, theme::paper::ink());
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
            theme::paper::ink(),
        );
    }

    let stroke = egui::Stroke::new(1.0, theme::paper::ink());
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
            .frame(egui::Frame::new().fill(theme::canvas()))
            .show(ui, |ui| {
                // 候选区挂在编辑区底部，对照模式下横跨源码与版式两栏。
                if self.candidates_available() {
                    self.candidate_panel_ui(ui);
                }
                self.preview_body_ui(ui);
            });
    }

    fn preview_body_ui(&mut self, ui: &mut egui::Ui) {
        match self.doc.preview_mode {
            PreviewMode::Source => self.source_editor_ui(ui),
            PreviewMode::Hybrid => self.markdown_hybrid_editor(ui),
            PreviewMode::Rendered => {
                let region = ui.max_rect();
                self.markdown_render(ui);
                // 必须排在预览之后：标题的屏幕位置是回查预览本帧注册的
                // widget 得来的，先画就只能拿到上一帧的版面。
                self.navigator_overlay(ui, region);
            }
            PreviewMode::VersionDiff => self.version_diff_mode_ui(ui),
            PreviewMode::Split => {
                egui::Panel::left("preview_split")
                    .default_size(420.0)
                    .size_range(280.0..=900.0)
                    .frame(theme::pane())
                    .show(ui, |ui| self.markdown_editor(ui));
                egui::CentralPanel::default()
                    .frame(theme::pane())
                    .show(ui, |ui| {
                        let region = ui.max_rect();
                        self.markdown_render(ui);
                        self.navigator_overlay(ui, region);
                    });
            }
        }
    }

    /// 返回是否点了关闭按钮——关闭请求由 `create_ui` 在面板动画之外落地，
    /// 闭包内直接改 `self.doc.result_drawer_open` 会被局部副本写回覆盖。
    pub(crate) fn result_drawer_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close_requested = false;
        let mut copy_all = false;
        // 标题与关闭按钮独占一行——右侧抽屉只有 300 点上下，挤不下一整排。
        ui.horizontal(|ui| {
            // 抽屉里现在有两类东西：能一键改的修订建议，和只能提请注意的要素
            // 提示。标题按两者之和数，否则用户看见徽章写 8 条、点进来只数出 3 条。
            let total = self.doc.warnings.len() + self.doc.revisions.pending_count();
            ui.strong(if total == 0 {
                "审校提示".to_string()
            } else {
                format!("审校提示 {total}")
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, theme::Icon::X, "关闭审校提示（Esc）").clicked() {
                    close_requested = true;
                }
                // 提示常要贴给别人或贴进聊天里问，逐条右键复制之外再给一个整份复制。
                let copyable = self.doc.export_error.is_some() || !self.doc.warnings.is_empty();
                if copyable
                    && theme::icon_button(ui, theme::Icon::Copy, "复制全部审校提示").clicked()
                {
                    copy_all = true;
                }
            });
        });
        if copy_all {
            ui.ctx().copy_text(self.warnings_text());
            *self.status = "审校提示已复制到剪贴板。".into();
        }
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
                            let response = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(error).color(theme::text_soft()),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                            copy_menu(&response, error, self.status);
                        });
                    ui.add_space(8.0);
                }
                // 修订建议排在最前面：它是这个抽屉里唯一「点一下就能改好」的
                // 部分，压在一串只能看的提示下面等于没有。
                let has_revisions = self.doc.revisions.pending_count() > 0
                    || self.doc.revisions.applied_count() > 0;
                self.revisions_ui(ui);
                // 两段都在时才分栏标注，否则空抽屉里会出现「要素与版式提示 0」
                // 紧跟着「暂无审校提示」这种自相矛盾的两行。
                if has_revisions {
                    if self.doc.warnings.is_empty() {
                        return;
                    }
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(4.0);
                    ui.strong(format!("要素与版式提示 {}", self.doc.warnings.len()));
                    ui.add_space(4.0);
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
        let source_mode = !hybrid && self.doc.preview_mode == PreviewMode::Source;
        let source_scroll_request = source_mode
            .then(|| self.doc.source_minimap.requested_offset.take())
            .flatten();
        let mut source_content = crate::draft_page::source_nav::SourceMiniContent::default();
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
        let reveal = std::mem::take(&mut self.doc.pending_source_reveal);
        let programmatic_source_move = jump.is_some() || selection.is_some();
        let selected_before = if self.doc.preview_mode == PreviewMode::Split {
            editor_selection(ui.ctx(), &self.doc.generated_markdown)
                .filter(|range| !range.is_empty())
                .or_else(|| selection.clone().filter(|range| !range.is_empty()))
        } else {
            None
        };
        let anchor = selected_before
            .is_none()
            .then(|| {
                self.doc
                    .preview_anchor
                    .as_ref()
                    .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown))
            })
            .flatten();
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
        let editor_font_size = self
            .config
            .editor_font_size
            .clamp(EDITOR_FONT_SIZE_MIN, EDITOR_FONT_SIZE_MAX);
        let line_number_size = (editor_font_size - 2.0).max(9.0);
        let text = &mut self.doc.generated_markdown;
        let highlighter = &mut self.doc.highlighter;
        let numbering = self.config.numbering;
        let editor_fonts = self.config.editor_fonts;
        // 研究报告的 mdx 扩展标记（{#id}、{@id}、[@key]、[^id]:(…)）只在
        // 源码高亮里上色；在闭包外先算成 bool，避免闭包再去借 self.doc。
        let research = self.doc.draft.kind.is_research();
        let mut editor_lost_focus = false;
        let mut cursor_follow = None;
        let mut selected_after = None;
        let mut drag_move = None;
        let selection_before = candidates::selection_before_show(ui.ctx(), editor_id());
        let mut menu_action = None;
        let clean_galley = RefCell::new(None);
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
            let galley = if hybrid {
                highlighter.layout_hybrid(
                    ui,
                    buffer.as_str(),
                    wrap_width.min(OFFICIAL_EDITOR_CONTENT_WIDTH),
                    active_line,
                    anchor.as_ref(),
                    &search_matches,
                    &numbering,
                )
            } else {
                highlighter.layout(
                    ui,
                    buffer.as_str(),
                    wrap_width,
                    editor_font_size,
                    anchor.as_ref(),
                    &search_matches,
                    &editor_fonts,
                    research,
                )
            };
            *clean_galley.borrow_mut() = Some(Arc::clone(&galley));
            galley
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
                            .fill(theme::paper::bg())
                            .stroke(egui::Stroke::new(1.0, theme::border_strong()))
                            // 编辑区的纸比预览页更贴近眼睛，投影一直比预览重一档：
                            // 明色纸下 18+24 与改成跟随纸面之前的 42 完全一致。
                            .shadow(theme::float_shadow(
                                theme::paper::shadow_alpha().saturating_add(24),
                            ))
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
                                        let output = show_with_glyph_caret(ui, editable, |ui| {
                                            egui::TextEdit::multiline(text)
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
                                            .show(ui)
                                        });
                                        editor_lost_focus |= output.response.lost_focus();
                                        menu_action = candidates::editor_context_menu(
                                            ui,
                                            &output,
                                            selection_before,
                                            editable,
                                        );
                                        drag_move = handle_text_drag(
                                            ui,
                                            &output,
                                            text,
                                            &mut self.doc.text_drag,
                                            selection_before,
                                            editable,
                                            clean_galley
                                                .borrow()
                                                .as_ref()
                                                .map(|galley| (galley, theme::paper::bg())),
                                        );
                                        if show_line_numbers {
                                            paint_editor_line_numbers(
                                                ui,
                                                &output,
                                                line_number_size,
                                                egui::FontFamily::Proportional,
                                            );
                                        }
                                        paint_hybrid_decorations(
                                            ui,
                                            &output,
                                            text,
                                            active_line,
                                            &numbering,
                                        );
                                        if let Some(range) = selection {
                                            select_source_range(ui, &output, text, range);
                                        } else if let Some(offset) = jump {
                                            jump_to_source(ui, &output, text, offset);
                                        }
                                        if reveal {
                                            // 换模式重排后光标可能滚出了视野：拉回
                                            // 来并恢复焦点，让用户接着刚才的位置改。
                                            // 本帧另有跳转/选区请求时以它们为准，
                                            // 它们自己会滚动。
                                            ui.ctx()
                                                .memory_mut(|memory| memory.request_focus(editor_id()));
                                            if !programmatic_source_move
                                                && let Some(cursor) =
                                                    output.cursor_range.map(|range| range.primary)
                                            {
                                                let rect = output
                                                    .galley
                                                    .pos_from_cursor(cursor)
                                                    .translate(output.galley_pos.to_vec2());
                                                ui.scroll_to_rect(rect, None);
                                            }
                                        }
                                        if output.cursor_range.is_some_and(|range| {
                                            source_line_at_char(text, range.primary.index.0)
                                                != active_line
                                        }) {
                                            ui.ctx().request_repaint();
                                        }
                                        if !programmatic_source_move
                                            && self.doc.preview_mode == PreviewMode::Split
                                            && output.response.has_focus()
                                        {
                                            cursor_follow = output.cursor_range.map(|range| {
                                                source_line_range_at_char(
                                                    text,
                                                    range.primary.index.0,
                                                )
                                            });
                                        }
                                    });
                                });
                            });
                    });
                });
        } else {
            let source_scroll = theme::card()
                .show(ui, |ui| {
                    let mut scroll = egui::ScrollArea::vertical()
                        .id_salt("preview_scroll")
                        .auto_shrink([false; 2]);
                    if source_mode && self.doc.source_minimap.visible {
                        scroll = scroll.scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                        );
                    }
                    if let Some(offset) = source_scroll_request {
                        scroll = scroll.vertical_scroll_offset(offset);
                    }
                    let scrolled = scroll.show(ui, |ui| {
                        let mut show_editor = |ui: &mut egui::Ui| {
                            ui.scope(|ui| {
                                if self.doc.preview_mode == PreviewMode::Split {
                                    ui.visuals_mut().selection.bg_fill = theme::md::selection_bg();
                                    ui.visuals_mut().selection.stroke.color = theme::text();
                                }
                                show_with_glyph_caret(ui, editable, |ui| {
                                    egui::TextEdit::multiline(text)
                                        .id(editor_id())
                                        .interactive(editable)
                                        .frame(egui::Frame::NONE)
                                        .code_editor()
                                        .layouter(&mut layouter)
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(rows)
                                        .hint_text(
                                            "生成结果将在这里显示，也可以直接粘贴已有稿件再导出……",
                                        )
                                        .show(ui)
                                })
                            })
                            .inner
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
                        menu_action = candidates::editor_context_menu(
                            ui,
                            &output,
                            selection_before,
                            editable,
                        );
                        drag_move = handle_text_drag(
                            ui,
                            &output,
                            text,
                            &mut self.doc.text_drag,
                            selection_before,
                            editable,
                            clean_galley
                                .borrow()
                                .as_ref()
                                .map(|galley| (galley, theme::surface())),
                        );
                        // Ctrl+滚轮调整源码字号：按住 Ctrl（mac 为 Cmd）时 egui 把滚动量
                        // 报成 zoom_delta，滚动区不会同时滚动，两者天然不冲突。
                        let zoom_delta = ui.ctx().input(|input| input.zoom_delta());
                        if zoom_delta != 1.0 && output.response.hovered() {
                            let size = ((editor_font_size * zoom_delta * 2.0).round() / 2.0)
                                .clamp(EDITOR_FONT_SIZE_MIN, EDITOR_FONT_SIZE_MAX);
                            if size != self.config.editor_font_size {
                                self.config.editor_font_size = size;
                                let _ = storage::save(self.config);
                            }
                        }
                        if show_line_numbers {
                            paint_editor_line_numbers(
                                ui,
                                &output,
                                line_number_size,
                                egui::FontFamily::Name(theme::EDITOR_FONT_FAMILY.into()),
                            );
                        }
                        if source_mode {
                            source_content = crate::draft_page::source_nav::capture_source_rows(
                                text,
                                &output,
                                &self.doc.source_outline,
                                clean_galley.borrow().clone(),
                            );
                        }
                        if let Some(range) = selection {
                            select_source_range(ui, &output, text, range);
                        } else if let Some(offset) = jump {
                            jump_to_source(ui, &output, text, offset);
                        }
                        if reveal {
                            // 换模式重排后光标可能滚出了视野：拉回来并恢复焦点，
                            // 让用户接着刚才的位置改。本帧另有跳转/选区请求时以
                            // 它们为准，它们自己会滚动。
                            ui.ctx()
                                .memory_mut(|memory| memory.request_focus(editor_id()));
                            if !programmatic_source_move
                                && let Some(cursor) = output.cursor_range.map(|range| range.primary)
                            {
                                let rect = output
                                    .galley
                                    .pos_from_cursor(cursor)
                                    .translate(output.galley_pos.to_vec2());
                                ui.scroll_to_rect(rect, None);
                            }
                        }
                        if !programmatic_source_move
                            && self.doc.preview_mode == PreviewMode::Split
                            && output.response.has_focus()
                        {
                            selected_after = output_selection(text, &output);
                            cursor_follow = selected_after
                                .is_none()
                                .then(|| {
                                    output.cursor_range.map(|range| {
                                        source_line_range_at_char(text, range.primary.index.0)
                                    })
                                })
                                .flatten();
                        } else if self.doc.preview_mode == PreviewMode::Split {
                            selected_after = output_selection(text, &output).or_else(|| {
                                editor_selection(ui.ctx(), text).filter(|range| !range.is_empty())
                            });
                        }
                    });
                    (
                        scrolled.state.offset.y,
                        scrolled.content_size.y,
                        scrolled.inner_rect.top(),
                        scrolled.inner_rect.height(),
                    )
                })
                .inner;
            if source_mode {
                let first_layout =
                    self.doc.source_minimap.rows.is_empty() && !source_content.rows.is_empty();
                self.doc.source_minimap.update(
                    source_content,
                    source_scroll.0,
                    source_scroll.1,
                    source_scroll.2,
                    source_scroll.3,
                );
                if first_layout {
                    ui.ctx().request_repaint();
                }
            }
        }
        if let Some((updated, range)) = drag_move {
            crate::draft_page::diff_editor::replace_with_undo(ui.ctx(), text, updated, range.end);
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), editor_id()) {
                let start = egui::text::CCursor::new(text[..range.start].chars().count());
                let end = egui::text::CCursor::new(text[..range.end].chars().count());
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(start, end)));
                state.store(ui.ctx(), editor_id());
            }
            ui.ctx().request_repaint();
        }
        if editable && editor_lost_focus {
            let normalized = export::normalize_ordered_list_punctuation(text);
            if normalized != *text {
                *text = normalized;
            }
        }
        if self.doc.preview_mode == PreviewMode::Split {
            if self.doc.preview_selection != selected_after {
                self.doc.preview_selection = selected_after;
                ui.ctx().request_repaint();
            }
            if self.doc.preview_selection.is_some() {
                self.doc.pending_render_jump = false;
            }
        }
        if let Some(range) = cursor_follow {
            let line_changed = self.doc.preview_cursor_line != Some(range.start);
            self.doc.preview_cursor_line = Some(range.start);
            if let Some(text) = self.doc.generated_markdown.get(range.clone()) {
                self.doc.preview_anchor = Some(PreviewAnchor {
                    range,
                    text: text.to_owned(),
                });
                if line_changed {
                    self.doc.pending_render_jump = true;
                    ui.ctx().request_repaint();
                }
            }
        }
        // 单栏模式（Markdown / 实时排版）没有光标跟随，同步高亮不会自己更新：
        // 光标离开它所在的段时把它撤掉，否则从对照模式带过来的旧高亮会一直
        // 留在版面上，选在别处也不消失。
        if self.doc.preview_mode != PreviewMode::Split && !programmatic_source_move {
            let focused = ui.ctx().memory(|memory| memory.has_focus(editor_id()));
            let source = &self.doc.generated_markdown;
            let cursor_line = focused
                .then(|| editor_cursor(ui.ctx(), source))
                .flatten()
                .map(|byte| source[..byte].rfind('\n').map_or(0, |index| index + 1));
            if let (Some(anchor_line), Some(cursor_line)) =
                (self.doc.preview_cursor_line, cursor_line)
                && cursor_line != anchor_line
            {
                self.doc.preview_anchor = None;
                self.doc.preview_cursor_line = None;
                ui.ctx().request_repaint();
            }
        }
        if let Some(action) = menu_action {
            self.run_editor_menu_action(ui.ctx(), action);
        }
    }

    /// 切换审校显示方式。离开对照模式时清掉光标跟随高亮——它锚在旧段落上，
    /// 单栏模式不会随光标更新，留着就是一块甩不掉的底色；切到带源码编辑框的
    /// 模式时请求下一帧把光标滚回视野并恢复焦点，否则换栏宽重排后刚才改到
    /// 哪儿就看不到了。查找条开着时不抢输入焦点。
    pub(crate) fn switch_preview_mode(&mut self, mode: PreviewMode) {
        if self.doc.preview_mode == PreviewMode::Split && mode != PreviewMode::Split {
            self.doc.preview_anchor = None;
            self.doc.preview_selection = None;
            self.doc.preview_cursor_line = None;
        }
        self.doc.preview_mode = mode;
        if matches!(
            mode,
            PreviewMode::Source | PreviewMode::Hybrid | PreviewMode::Split
        ) && !self.doc.markdown_find.open
        {
            self.doc.pending_source_reveal = true;
        }
    }

    /// 公文版式预览。正文为空时也照排——红头、密级、文号、主送、落款这些
    /// 行文要素来自表单，填完就能先看版式。
    pub(crate) fn markdown_render(&mut self, ui: &mut egui::Ui) {
        let selection = (self.doc.preview_mode == PreviewMode::Split)
            .then(|| self.doc.preview_selection.clone())
            .flatten()
            .filter(|range| {
                !range.is_empty() && self.doc.generated_markdown.get(range.clone()).is_some()
            });
        preview::set_text_selection(ui.ctx(), &self.doc.generated_markdown, selection.clone());
        let scrolled = egui::ScrollArea::both()
            .id_salt("render_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let display = UnitDisplay::new(&self.config.vocabulary);
                let anchor = selection.or_else(|| {
                    self.doc
                        .preview_anchor
                        .as_ref()
                        .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown))
                });

                // 宽度还在变的帧里不重排版面，缩放交给层变换（见 `preview::freeze`）。
                let frozen = preview::show_frozen(
                    ui,
                    &mut self.doc.preview_freeze,
                    self.doc.preview_zoom,
                    |ui, scale| {
                        preview::official_preview(
                            ui,
                            &self.doc.draft,
                            &display,
                            &self.doc.generated_markdown,
                            scale,
                            anchor.as_ref(),
                            self.doc.pending_render_jump,
                            &self.config.numbering,
                            self.config.show_preview_line_numbers,
                            &crate::visual_diff::ElementMarks::default(),
                        )
                    },
                );
                let output = frozen.inner;
                let target = frozen.target;

                self.doc.pending_render_jump = false;
                // 加减档以"眼睛看到的倍率"为起点，而不是本帧用来排版的那个。
                self.doc.preview_fit_scale = target;
                // 点中版式上的某一块：源码里同步高亮，并把光标带过去。
                if let Some(range) = output.clicked {
                    let line_start = range.start;
                    self.doc.preview_selection = None;
                    self.doc.pending_source_selection = None;
                    self.doc.pending_source_jump = Some(line_start);
                    self.doc.preview_anchor =
                        self.doc
                            .generated_markdown
                            .get(range.clone())
                            .map(|text| PreviewAnchor {
                                range,
                                text: text.to_owned(),
                            });
                    self.doc.preview_cursor_line = Some(line_start);
                    ui.ctx().request_repaint();
                }
                ui.add_space(12.0);
            });
        preview::set_text_selection(ui.ctx(), &self.doc.generated_markdown, None);
        // 右缘导航刻度要把标题的版面位置换算成刻度条上的位置，量度只有滚动区知道。
        self.doc.preview_scroll = PreviewScroll {
            offset_y: scrolled.state.offset.y,
            content_height: scrolled.content_size.y,
            viewport_top: scrolled.inner_rect.top(),
            viewport_height: scrolled.inner_rect.height(),
        };
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
                        let response = ui.add(
                            egui::Label::new(text.color(theme::text_soft()))
                                .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        copy_menu(&response, &warning.message, self.status);
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
                        .on_hover_text("点击定位到正文中的这一段，右键可复制");
                    if response.clicked() {
                        jump = Some(span);
                    }
                    copy_menu(&response, &warning.message, self.status);
                }
            });
        if let Some(span) = jump {
            self.jump_to_source(span);
        }
    }

    /// 抽屉里全部提示的纯文本：导出失败原因在前，要素与版式提示逐条一行。
    fn warnings_text(&self) -> String {
        let mut lines = Vec::new();
        if let Some(error) = &self.doc.export_error {
            lines.push(format!("导出失败：{error}"));
        }
        lines.extend(
            self.doc
                .warnings
                .iter()
                .map(|warning| format!("· {}", warning.message)),
        );
        lines.join("\n")
    }
}

/// 给一条提示挂右键菜单「复制」。可点的提示左键已经用来定位正文，复制只能放右键；
/// 普通提示虽能拖选，但抽屉窄、长提示折好几行，拖选很难一次选全。
fn copy_menu(response: &egui::Response, text: &str, status: &mut String) {
    response.context_menu(|ui| {
        if ui.button("复制这条提示").clicked() {
            ui.ctx().copy_text(text.to_owned());
            *status = "已复制到剪贴板。".into();
            ui.close();
        }
    });
}

#[cfg(test)]
mod source_cursor_tests {
    use super::*;

    #[test]
    fn cursor_range_tracks_the_exact_markdown_source_line() {
        let text = "第一行\n第二行\n第三行";
        assert_eq!(source_line_range_at_char(text, 0), 0.."第一行".len());
        let second_char = "第一行\n第".chars().count();
        let second_start = "第一行\n".len();
        assert_eq!(
            source_line_range_at_char(text, second_char),
            second_start..second_start + "第二行".len()
        );
        let blank = "第一行\n\n## 下一节";
        let blank_char = "第一行\n".chars().count();
        let heading_start = "第一行\n\n".len();
        assert_eq!(
            source_line_range_at_char(blank, blank_char),
            heading_start..blank.len()
        );
    }
}
