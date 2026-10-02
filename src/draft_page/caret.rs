//! 编辑框光标：按光标旁字形的字体框画，而不是 egui 默认的整行高。
//!
//! egui 的 `TextEdit` 把光标画成整行高、再上下各多出 1.5 px。编辑区给行设了固定
//! 行高（源码模式为字号的 1.35 倍），epaint 又把字形贴在行顶、多出来的行距全留
//! 在字下方——于是光标比字高出一截，还往下拖进行间空白，空行上尤其显眼。这里在
//! 编辑框绘制期间关掉 egui 自带的光标，事后按字形高度补画一条；闪烁节奏照搬 egui
//! （有改动或光标移动就重新从「亮」开始）。

use eframe::egui::{self, Galley, Rect, Stroke, text::CCursor, text_edit::TextEditOutput};

/// 比这还小的字形是源码里被压成近零宽的隐藏标记，不能拿来量光标高度。
const MIN_GLYPH_HEIGHT: f32 = 4.0;

/// 空行上找不到字形时，往上下各看几行借一个字高。
const NEIGHBOUR_ROWS: usize = 6;

/// 绘制编辑框：期间关掉 egui 自带的整行高光标，画完后按字形高度补画。
pub(crate) fn show_with_glyph_caret(
    ui: &mut egui::Ui,
    editable: bool,
    show: impl FnOnce(&mut egui::Ui) -> TextEditOutput,
) -> TextEditOutput {
    let stroke = ui.visuals().text_cursor.stroke;
    ui.visuals_mut().text_cursor.stroke = Stroke::NONE;
    let output = show(ui);
    ui.visuals_mut().text_cursor.stroke = stroke;
    if editable {
        paint(ui, &output, stroke);
    }
    output
}

/// 光标最近一次改动（文本变化或光标移动）的时刻，决定闪烁相位。
#[derive(Clone, Copy)]
struct Blink {
    primary: usize,
    secondary: usize,
    since: f64,
}

fn paint(ui: &egui::Ui, output: &TextEditOutput, stroke: Stroke) {
    // 与 egui 一致：窗口没有焦点时不画、也不为闪烁请求重绘。
    if !output.response.has_focus() || !ui.input(|input| input.focused) {
        return;
    }
    let Some(range) = output.cursor_range else {
        return;
    };
    let now = ui.input(|input| input.time);
    let key = output.response.id.with("glyph_caret_blink");
    let (primary, secondary) = (range.primary.index.0, range.secondary.index.0);
    let since = ui.ctx().data_mut(|data| {
        let blink = data.get_temp::<Blink>(key);
        let since = match blink {
            Some(blink)
                if !output.response.changed()
                    && blink.primary == primary
                    && blink.secondary == secondary =>
            {
                blink.since
            }
            _ => now,
        };
        data.insert_temp(
            key,
            Blink {
                primary,
                secondary,
                since,
            },
        );
        since
    });

    let cursor = &ui.visuals().text_cursor;
    if cursor.blink {
        let total = cursor.on_duration + cursor.off_duration;
        let phase = ((now - since) % total as f64) as f32;
        if phase >= cursor.on_duration {
            ui.ctx().request_repaint_after_secs(total - phase);
            return;
        }
        ui.ctx()
            .request_repaint_after_secs(cursor.on_duration - phase);
    }

    let offset = output.galley_pos.to_vec2() - egui::vec2(output.galley.rect.left(), 0.0);
    let rect = glyph_caret_rect(&output.galley, range.primary).translate(offset);
    ui.painter()
        .line_segment([rect.center_top(), rect.center_bottom()], stroke);
}

/// 光标在 galley 坐标系里的矩形（零宽）：竖向只覆盖光标旁字形的字体框。
fn glyph_caret_rect(galley: &Galley, cursor: CCursor) -> Rect {
    let line = galley.pos_from_cursor(cursor);
    let layout = galley.layout_from_cursor(cursor);
    let Some(row) = galley.rows.get(layout.row) else {
        return line;
    };
    // 字形框相对所在行顶的偏移与高度。
    let glyph_box = glyph_near(&row.row.glyphs, layout.column.0).or_else(|| {
        // 空行：借上下最近一行的字高，位置仍按行顶对齐——epaint 排字就是贴着行顶。
        (1..=NEIGHBOUR_ROWS).find_map(|distance| {
            [
                layout.row.checked_sub(distance),
                Some(layout.row + distance),
            ]
            .into_iter()
            .flatten()
            .filter_map(|index| galley.rows.get(index))
            .find_map(|row| glyph_near(&row.row.glyphs, 0))
        })
    });
    let Some((top, height)) = glyph_box else {
        return line;
    };
    let min_y = (line.min.y + top).clamp(line.min.y, line.max.y);
    let max_y = (min_y + height).min(line.max.y);
    Rect::from_min_max(egui::pos2(line.min.x, min_y), egui::pos2(line.max.x, max_y))
}

/// 在一行里找离第 `column` 个字符最近的可见字形，优先光标左边那个（正在打的字）。
/// 返回字形框顶相对行顶的偏移和字形框高度。
fn glyph_near(glyphs: &[egui::epaint::text::Glyph], column: usize) -> Option<(f32, f32)> {
    let visible = |index: usize| {
        glyphs
            .get(index)
            .filter(|glyph| glyph.font_height >= MIN_GLYPH_HEIGHT)
            .map(|glyph| (glyph.pos.y - glyph.font_ascent, glyph.font_height))
    };
    (0..glyphs.len()).find_map(|distance| {
        column
            .checked_sub(distance + 1)
            .and_then(visible)
            .or_else(|| visible(column + distance))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::text::{LayoutJob, TextFormat};

    const FONT: f32 = 20.0;
    const LINE: f32 = 40.0;

    fn galley(text: &str) -> std::sync::Arc<Galley> {
        let ctx = egui::Context::default();
        let mut galley = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut job = LayoutJob::default();
            job.append(
                text,
                0.0,
                TextFormat {
                    font_id: egui::FontId::proportional(FONT),
                    line_height: Some(LINE),
                    ..Default::default()
                },
            );
            galley = Some(ui.ctx().fonts_mut(|fonts| fonts.layout_job(job)));
        });
        galley.unwrap()
    }

    #[test]
    fn caret_is_shorter_than_a_tall_row() {
        let galley = galley("abc\ndef");
        let row = galley.pos_from_cursor(CCursor::new(2));
        let caret = glyph_caret_rect(&galley, CCursor::new(2));
        assert_eq!(caret.min.x, row.min.x);
        assert!(
            caret.height() < row.height() * 0.8,
            "光标 {caret:?} 应明显矮于行 {row:?}"
        );
        assert!(caret.min.y >= row.min.y && caret.max.y <= row.max.y);
    }

    #[test]
    fn empty_line_borrows_the_neighbour_glyph_height() {
        let galley = galley("abc\n\ndef");
        // 第 4 个字符（下标 4）是空行上的光标位置。
        let empty = glyph_caret_rect(&galley, CCursor::new(4));
        let filled = glyph_caret_rect(&galley, CCursor::new(1));
        let row = galley.pos_from_cursor(CCursor::new(4));
        assert_eq!(empty.height(), filled.height());
        assert_eq!(
            empty.min.y,
            row.min.y + (filled.min.y - galley.rows[0].pos.y)
        );
    }

    #[test]
    fn tiny_markers_are_skipped() {
        let tiny = TextFormat {
            font_id: egui::FontId::proportional(0.1),
            line_height: Some(LINE),
            ..Default::default()
        };
        let body = TextFormat {
            font_id: egui::FontId::proportional(FONT),
            line_height: Some(LINE),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut caret = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut job = LayoutJob::default();
            job.append("## ", 0.0, tiny.clone());
            job.append("标题", 0.0, body.clone());
            let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            // 光标紧贴隐藏的「## 」之后、正文之前。
            caret = Some(glyph_caret_rect(&galley, CCursor::new(3)));
        });
        assert!(caret.unwrap().height() >= MIN_GLYPH_HEIGHT);
    }
}
