//! 查找浮层的绘制：主题色、清晰的两行条目、细描边和轻量投影。

use super::Target;
use super::index::Entry;
use crate::theme;
use eframe::egui::{self, Color32, FontId, Rect, Stroke, TextFormat};

/// 次要信息仍需清楚可读，尤其避免深色主题的低对比提示色用于整行文号。
pub(super) fn secondary_text() -> Color32 {
    theme::text_soft().lerp_to_gamma(theme::text_muted(), 0.5)
}

pub(super) fn search_icon(ui: &egui::Ui, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.8, color);
    ui.painter()
        .circle_stroke(center - egui::vec2(2.0, 2.0), 7.0, stroke);
    ui.painter().line_segment(
        [center + egui::vec2(3.0, 3.0), center + egui::vec2(9.0, 9.0)],
        stroke,
    );
}

pub(super) fn keycap(ui: &egui::Ui, rect: Rect, text: &str, hovered: bool) {
    ui.painter().rect(
        rect,
        5,
        if hovered {
            theme::surface_hover()
        } else {
            theme::surface()
        },
        Stroke::new(1.0, theme::border()),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.0),
        secondary_text(),
    );
}

/// 常见中文界面字体不一定有回车符号，使用描线保证每个平台都能显示。
fn enter_icon(ui: &egui::Ui, center: egui::Pos2, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    let points = [
        center + egui::vec2(6.0, -5.0),
        center + egui::vec2(6.0, 2.0),
        center + egui::vec2(-6.0, 2.0),
    ];
    ui.painter().line(points.to_vec(), stroke);
    ui.painter().line(
        vec![
            center + egui::vec2(-2.0, -2.0),
            center + egui::vec2(-6.0, 2.0),
            center + egui::vec2(-2.0, 6.0),
        ],
        stroke,
    );
}

fn document_icon(ui: &egui::Ui, rect: Rect, color: Color32) {
    let point = |x, y| rect.min + egui::vec2(x, y);
    let stroke = Stroke::new(1.6, color);
    ui.painter().line(
        vec![
            point(14.0, 2.0),
            point(4.0, 2.0),
            point(4.0, 22.0),
            point(20.0, 22.0),
            point(20.0, 8.0),
            point(14.0, 2.0),
            point(14.0, 8.0),
            point(20.0, 8.0),
        ],
        stroke,
    );
    for y in [12.0, 16.0] {
        ui.painter()
            .line_segment([point(8.0, y), point(16.0, y)], stroke);
    }
}

pub(super) fn divider(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, theme::border()),
    );
}

pub(super) fn result_row(
    ui: &mut egui::Ui,
    entry: &Entry,
    query: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 64.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            format!("{}\n{}", entry.title, entry.subtitle),
        )
    });
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            7,
            if selected {
                theme::accent_soft()
            } else {
                theme::surface_hover()
            },
        );
    }
    if selected {
        let stripe = Rect::from_min_size(
            rect.left_top() + egui::vec2(0.0, 10.0),
            egui::vec2(3.0, 44.0),
        );
        ui.painter().rect_filled(stripe, 2, theme::accent());
    }
    let icon_rect = Rect::from_center_size(
        egui::pos2(rect.left() + 28.0, rect.center().y),
        egui::vec2(24.0, 24.0),
    );
    let icon_color = if selected {
        theme::text()
    } else {
        theme::text_soft()
    };
    if matches!(entry.target, Target::Document(_) | Target::Manuscript(_)) {
        document_icon(ui, icon_rect, icon_color);
    } else {
        entry
            .icon
            .image_sized(24.0)
            .tint(icon_color)
            .paint_at(ui, icon_rect);
    }
    let arrow_width = if selected { 24.0 } else { 0.0 };
    let mut right = rect.right() - 12.0 - arrow_width;
    if let Some(badge) = entry.badge.filter(|_| rect.width() >= 300.0) {
        let galley = ui.painter().layout_no_wrap(
            badge.into(),
            FontId::proportional(11.0),
            if selected {
                theme::accent()
            } else {
                secondary_text()
            },
        );
        let badge_rect = Rect::from_center_size(
            egui::pos2(right - (galley.size().x + 16.0) / 2.0, rect.center().y),
            galley.size() + egui::vec2(16.0, 10.0),
        );
        ui.painter().rect_filled(
            badge_rect,
            12,
            if selected {
                theme::surface()
            } else {
                theme::surface_sunk()
            },
        );
        ui.painter().galley(
            badge_rect.center() - galley.size() / 2.0,
            galley,
            secondary_text(),
        );
        right = badge_rect.left() - 10.0;
    }
    if selected {
        enter_icon(
            ui,
            egui::pos2(rect.right() - 20.0, rect.center().y),
            theme::accent(),
        );
    }
    let left = rect.left() + 54.0;
    let width = (right - left).max(12.0);
    let job = title_job(&entry.title, query, width);
    let title = ui.painter().layout_job(job);
    let mut subtitle = egui::text::LayoutJob::simple(
        entry.subtitle.clone(),
        FontId::proportional(12.0),
        secondary_text(),
        width,
    );
    subtitle.wrap.max_rows = 1;
    subtitle.wrap.break_anywhere = true;
    let subtitle = ui.painter().layout_job(subtitle);
    ui.painter()
        .galley(egui::pos2(left, rect.top() + 11.0), title, theme::text());
    ui.painter().galley(
        egui::pos2(left, rect.top() + 36.0),
        subtitle,
        secondary_text(),
    );
    response
        .on_hover_text(format!("{}\n{}", entry.title, entry.subtitle))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 按字符而非字节标记原文命中，中文、组合字符和表情不会形成非法切片。
fn title_job(title: &str, query: &str, width: f32) -> egui::text::LayoutJob {
    let chars: Vec<char> = title.chars().collect();
    let mut marked = vec![false; chars.len()];
    for term in query.split_whitespace() {
        let term: Vec<char> = term.chars().collect();
        if term.len() > chars.len() {
            continue;
        }
        for (start, window) in chars.windows(term.len()).enumerate() {
            if window
                .iter()
                .zip(&term)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
            {
                marked[start..start + term.len()].fill(true);
            }
        }
    }
    let mut job = egui::text::LayoutJob::default();
    for (ch, marked) in chars.into_iter().zip(marked) {
        job.append(
            &ch.to_string(),
            0.0,
            TextFormat {
                font_id: FontId::proportional(16.0),
                color: if marked {
                    theme::accent()
                } else {
                    theme::text()
                },
                ..Default::default()
            },
        );
    }
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job
}

pub(super) fn footer(ui: &mut egui::Ui, count: usize) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::hover());
    if count > 0 {
        ui.painter().text(
            rect.left_center() + egui::vec2(18.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "↑↓ 选择",
            FontId::proportional(11.0),
            secondary_text(),
        );
        enter_icon(
            ui,
            rect.left_center() + egui::vec2(96.0, 0.0),
            secondary_text(),
        );
        ui.painter().text(
            rect.left_center() + egui::vec2(111.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "打开",
            FontId::proportional(11.0),
            secondary_text(),
        );
    }
    let count = if count == super::ui::MAX_RESULTS {
        "前 50 项 · 继续输入缩小范围".into()
    } else {
        format!("{count} 项结果")
    };
    if rect.width() >= 360.0 {
        ui.painter().text(
            rect.right_center() - egui::vec2(18.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            count,
            FontId::proportional(11.0),
            secondary_text(),
        );
    }
}
