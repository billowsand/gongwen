//! 会议纪要的预览部件：版头（份号、密级、纪要标志、编号与红线）、名单与版记。
//!
//! 版式与 PDF（`assets/typst/gongwen.typ` 的 `minutes-*`）、Word（`export/docx/minutes.rs`）
//! 对应，规格见 `docs/meeting-minutes-design.md` 第二节。预览不分页，版记紧跟名单。

use crate::export::minutes::{RosterLine, minutes_heading, minutes_number_text, roster};
use crate::models::{DraftInput, split_units};
use crate::preview::header::security_line;
use crate::preview::{
    BODY_PT, HEADER_RULE_MM, Metrics, RECORD_GAP_MM, RECORD_PT, layout, line_galley, marks, place,
    single_line, text_format,
};
use crate::theme;
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use eframe::egui;
use eframe::egui::Align;

/// 纪要标志：小标宋 26 磅。
const HEADING_PT: f32 = 26.0;
/// 编号行字底到红线：与导出一致，红线在编号基线下约 4mm。
const NUMBER_RULE_GAP_MM: f32 = 1.5;
const ROSTER_COLUMNS: usize = 6;

/// 版头：份号、密级，空一行，纪要标志紧接编号与红线，空一行。
pub(crate) fn header(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
    elements: &ElementMarks,
) {
    let heiti = metrics.font(theme::FONT_HEITI, BODY_PT);
    let serial = line_galley(ui, metrics, "01", heiti, metrics.content, Align::LEFT);
    place(ui, metrics, metrics.line, |painter, rect| {
        painter.galley(rect.left_top(), serial.clone(), theme::paper::ink());
    });
    security_line(ui, metrics, input, elements.security());
    ui.add_space(metrics.line);

    // 纪要标志居中；排不下就缩字号（egui 只能整体缩，导出是横向压扁）。
    let heading = minutes_heading(input, display);
    let mut format = text_format(
        metrics.font(theme::FONT_BIAOSONG, HEADING_PT),
        metrics.pt(HEADING_PT) * 1.2,
    );
    format.color = theme::paper::red();
    let natural = layout(ui, single_line(&heading, format.clone())).size().x;
    if natural > metrics.content {
        format.font_id = metrics.font(theme::FONT_BIAOSONG, HEADING_PT * metrics.content / natural);
    }
    let galley = layout(ui, single_line(&heading, format));
    place(ui, metrics, galley.size().y, |painter, rect| {
        let left = rect.left() + (metrics.content - galley.size().x) / 2.0;
        painter.galley(
            egui::pos2(left, rect.top()),
            galley.clone(),
            theme::paper::red(),
        );
    });
    let number_font = text_format(metrics.font(theme::FONT_FANGSONG, BODY_PT), metrics.line);
    let [_, year_mark, serial_mark] = elements.number();
    let number = if year_mark.changed() || serial_mark.changed() {
        marks::marked_line_galley(
            ui,
            metrics,
            &elements.number_marked_line(""),
            metrics.content,
            Align::LEFT,
            number_font,
        )
    } else {
        let mut job = crate::preview::job(metrics.content);
        job.append(&minutes_number_text(input), 0.0, number_font);
        layout(ui, job)
    };
    let gap = metrics.mm(NUMBER_RULE_GAP_MM);
    let thickness = metrics.mm(HEADER_RULE_MM).max(1.0);
    place(
        ui,
        metrics,
        number.size().y + gap + thickness,
        |painter, rect| {
            let pos = egui::pos2(
                rect.left() + (metrics.content - number.size().x) / 2.0,
                rect.top(),
            );
            painter.galley(pos, number.clone(), theme::paper::ink());
            marks::paint_galley_marks(painter, metrics, pos, &number);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), rect.top() + number.size().y + gap),
                    egui::vec2(metrics.content, thickness),
                ),
                0.0,
                theme::paper::red(),
            );
        },
    );
    ui.add_space(metrics.line);
}

/// 名单：正文后空一行，左空二字，「出席：」黑体；网格每行 6 格居中，连排时回行
/// 与冒号后首字对齐。
pub(crate) fn roster_block(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput) {
    let lines = roster(input);
    if lines.is_empty() {
        return;
    }
    ui.add_space(metrics.line);
    for line in &lines {
        roster_line(ui, metrics, line);
    }
}

fn roster_line(ui: &mut egui::Ui, metrics: &Metrics, line: &RosterLine) {
    let em = metrics.pt(BODY_PT);
    let label_left = 2.0 * em;
    let names_left = 5.0 * em;
    let label = line_galley(
        ui,
        metrics,
        &format!("{}：", line.label),
        metrics.font(theme::FONT_HEITI, BODY_PT),
        // 换行宽度留一个字余量：三个字面（如「出席：」）恰好 3em 时，像素取整
        // 会让它在某些平台上差一点点折成两行。
        4.0 * em,
        Align::LEFT,
    );
    let fangsong = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    if !line.grid {
        let names = line_galley(
            ui,
            metrics,
            &line.names.join("、"),
            fangsong,
            metrics.content - names_left,
            Align::LEFT,
        );
        place(ui, metrics, names.size().y, |painter, rect| {
            painter.galley(
                egui::pos2(rect.left() + label_left, rect.top()),
                label.clone(),
                theme::paper::ink(),
            );
            painter.galley(
                egui::pos2(rect.left() + names_left, rect.top()),
                names.clone(),
                theme::paper::ink(),
            );
        });
        return;
    }
    let cell = (metrics.content - names_left) / ROSTER_COLUMNS as f32;
    for (row, chunk) in line.names.chunks(ROSTER_COLUMNS).enumerate() {
        let names: Vec<_> = chunk
            .iter()
            .map(|name| {
                line_galley(
                    ui,
                    metrics,
                    &padded_name(name),
                    fangsong.clone(),
                    cell,
                    Align::LEFT,
                )
            })
            .collect();
        place(ui, metrics, metrics.line, |painter, rect| {
            if row == 0 {
                painter.galley(
                    egui::pos2(rect.left() + label_left, rect.top()),
                    label.clone(),
                    theme::paper::ink(),
                );
            }
            for (column, galley) in names.iter().enumerate() {
                let x = rect.left()
                    + names_left
                    + cell * column as f32
                    + (cell - galley.size().x) / 2.0;
                painter.galley(
                    egui::pos2(x, rect.top()),
                    galley.clone(),
                    theme::paper::ink(),
                );
            }
        });
    }
}

/// 两字名中间空一字凑成三字宽，与导出一致。
fn padded_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() == 2 {
        format!("{}\u{3000}{}", chars[0], chars[1])
    } else {
        name.to_string()
    }
}

/// 版记：顶粗线、分送、抄送（末行右端印数）、细线、印发机关与印发日期、底粗线。
pub(crate) fn record_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
    elements: &ElementMarks,
) {
    ui.add_space(metrics.mm(RECORD_GAP_MM));
    let font = metrics.font(theme::FONT_FANGSONG, RECORD_PT);
    let em = metrics.pt(RECORD_PT);
    let hang = 3.0 * em;
    let value_width = metrics.content - hang;

    // 分送、抄送两行（可缺），每行一个标签与一段可折行的值。
    let mut upper: Vec<(String, std::sync::Arc<egui::Galley>)> = Vec::new();
    let distribute = split_units(&input.minutes.distribute);
    if !distribute.is_empty() {
        let value = line_galley(
            ui,
            metrics,
            &distribute.join("、"),
            font.clone(),
            value_width,
            Align::LEFT,
        );
        upper.push(("分送：".into(), value));
    }
    let copies = crate::export::element_display::copies_to_display(input, display);
    if !copies.trim().is_empty() || elements.copies_to().changed() {
        let text = if elements.copies_to().changed() {
            elements.copies_to().marked()
        } else {
            copies
        };
        let value = marks::marked_line_galley(
            ui,
            metrics,
            &text,
            value_width,
            Align::LEFT,
            text_format(font.clone(), metrics.line),
        );
        upper.push(("抄送：".into(), value));
    }
    let note = line_galley(
        ui,
        metrics,
        &format!("（共印{}份）", crate::export::automatic_print_copies(input)),
        font.clone(),
        metrics.content,
        Align::Max,
    );
    let mut date = crate::export::element_display::date_display_line(input);
    date.push_str("印发");
    let unit = line_galley(
        ui,
        metrics,
        &display.full_name(input.minutes_printing_unit()),
        font.clone(),
        metrics.content,
        Align::LEFT,
    );
    let date = line_galley(
        ui,
        metrics,
        &date,
        font.clone(),
        metrics.content,
        Align::Max,
    );
    let labels: Vec<_> = upper
        .iter()
        .map(|(label, _)| line_galley(ui, metrics, label, font.clone(), hang + em, Align::LEFT))
        .collect();

    let thick = metrics.mm(0.6).max(1.0);
    let thin = metrics.mm(0.3).max(1.0);
    let upper_height: f32 = upper
        .iter()
        .map(|(_, value)| value.size().y.max(metrics.line))
        .sum();
    let total =
        thick + upper_height + if upper.is_empty() { 0.0 } else { thin } + metrics.line + thick;
    place(ui, metrics, total, |painter, rect| {
        let rule = |y: f32, width: f32| {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), y),
                    egui::vec2(metrics.content, width),
                ),
                0.0,
                theme::paper::ink(),
            );
        };
        let ink = theme::paper::ink();
        let mut y = rect.top();
        rule(y, thick);
        y += thick;
        for (index, (label, (_, value))) in labels.iter().zip(&upper).enumerate() {
            painter.galley(egui::pos2(rect.left(), y), label.clone(), ink);
            let pos = egui::pos2(rect.left() + hang, y);
            painter.galley(pos, value.clone(), ink);
            marks::paint_galley_marks(painter, metrics, pos, value);
            let height = value.size().y.max(metrics.line);
            if index + 1 == upper.len() {
                // 印数挂在最后一行右端。
                let last_row = value.rows.last().map_or(0.0, |row| row.pos.y);
                painter.galley(egui::pos2(rect.right(), y + last_row), note.clone(), ink);
            }
            y += height;
        }
        if !upper.is_empty() {
            rule(y, thin);
            y += thin;
        }
        painter.galley(egui::pos2(rect.left() + em, y), unit.clone(), ink);
        painter.galley(egui::pos2(rect.right() - em, y), date.clone(), ink);
        y += metrics.line;
        rule(y, thick);
    });
}
