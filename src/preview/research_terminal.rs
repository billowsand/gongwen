//! 研究终端纸面装饰与封面；正文仍走研究报告的编号、公式和源码定位。

use super::{Metrics, layout::sheet, math_flow};
use crate::{export, models::DraftInput, theme};
use eframe::egui::{self, Color32, Stroke};

pub(super) fn accent() -> Color32 {
    if theme::paper::is_dark() {
        Color32::from_rgb(255, 189, 89)
    } else {
        Color32::from_rgb(181, 61, 48)
    }
}

pub(super) fn rule(ui: &mut egui::Ui, metrics: &Metrics) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(metrics.content, metrics.mm(2.0)),
        egui::Sense::hover(),
    );
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(metrics.pt(0.6), accent()),
    );
}

pub(super) fn paper(
    ui: &egui::Ui,
    metrics: &Metrics,
    rect: egui::Rect,
    index: egui::layers::ShapeIdx,
) {
    let grid = if theme::paper::is_dark() {
        Color32::from_rgb(28, 41, 49)
    } else {
        Color32::from_rgb(237, 240, 241)
    };
    let step = metrics.mm(5.0);
    let mut shapes = Vec::new();
    // 连续纸面只画视口附近的网格，长报告不会每帧生成数千条离屏线。
    let visible = rect.intersect(ui.clip_rect());
    for i in 0..=42 {
        let x = rect.left() + i as f32 * step;
        shapes.push(egui::Shape::line_segment(
            [
                egui::pos2(x, visible.top()),
                egui::pos2(x, visible.bottom()),
            ],
            Stroke::new(metrics.pt(0.2), grid),
        ));
    }
    let start = ((visible.top() - rect.top()) / step).max(0.0) as usize;
    let end = ((visible.bottom() - rect.top()) / step).max(0.0) as usize;
    for i in start..=end {
        let y = rect.top() + i as f32 * step;
        shapes.push(egui::Shape::line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(metrics.pt(0.2), grid),
        ));
    }
    shapes.push(egui::Shape::rect_stroke(
        rect.shrink(metrics.mm(15.0)),
        0.0,
        Stroke::new(metrics.pt(0.5), theme::paper::ink_faint()),
        egui::StrokeKind::Inside,
    ));
    ui.painter().set(index, egui::Shape::Vec(shapes));
}

pub(super) fn cover(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput, markdown: &str) {
    sheet(ui, metrics, |ui| {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                metrics.content,
                metrics.page_height - 2.0 * metrics.margin_top,
            ),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        let meta = &input.research;
        let text = |y: f32, value: &str, pt: f32, color: Color32| {
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = metrics.content;
            job.append(
                value,
                0.0,
                egui::TextFormat {
                    font_id: metrics.font(theme::FONT_HEITI, pt),
                    color,
                    line_height: Some(metrics.pt(pt + 10.0)),
                    ..Default::default()
                },
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let bottom = y + galley.size().y / metrics.mm(1.0);
            painter.galley(rect.min + egui::vec2(0.0, metrics.mm(y)), galley, color);
            bottom
        };
        let ink = theme::paper::ink();
        text(4.0, "RESEARCH / 研究终端", 10.0, theme::paper::ink_muted());
        text(22.0, &meta.file_type, 14.0, accent());
        text(40.0, &meta.ident, 12.0, theme::paper::ink_muted());
        let title = export::research::cover_title(input, markdown)
            .unwrap_or_else(|| "【待核实：文件名称】".into());
        let title = export::title::cover_title_lines(&title).join("\n");
        let title_bottom = if math_flow::has_math(&title) {
            let font = metrics.font(theme::FONT_HEITI, 28.0);
            let style = math_flow::FlowStyle::block(
                (font.clone(), font),
                28.0,
                metrics.pt(40.0),
                metrics.content,
            );
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = metrics.content;
            let mut bold_ranges = Vec::new();
            let slots = math_flow::append_with_math(
                ui,
                metrics,
                &mut job,
                &title,
                &style,
                &mut bold_ranges,
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let origin = rect.min + egui::vec2(0.0, metrics.mm(58.0));
            painter.galley(origin, galley.clone(), ink);
            math_flow::paint_slots(painter, metrics, origin, &galley, &slots);
            58.0 + galley.size().y / metrics.mm(1.0)
        } else {
            text(58.0, &title, 28.0, ink)
        };
        if !meta.original_title.is_empty() {
            text(title_bottom + 6.0, &meta.original_title, 12.0, ink);
        }
        let y = rect.top() + metrics.mm(118.0);
        painter.hline(rect.x_range(), y, Stroke::new(metrics.pt(1.0), accent()));
        text(
            128.0,
            "CONTENTS / 研究结构",
            10.0,
            theme::paper::ink_muted(),
        );
        let chapters = super::research::outline(markdown);
        for (i, entry) in chapters
            .iter()
            .filter(|entry| entry.level == 2)
            .take(5)
            .enumerate()
        {
            text(
                138.0 + i as f32 * 8.0,
                &format!("{}  {}", entry.number.as_deref().unwrap_or("—"), entry.text),
                12.0,
                ink,
            );
        }
        let family = mdx::cover::Family::of(&meta.file_type);
        let stage = if family.is_project() {
            format!(
                "立项论证 / 建设实施 / 技术实现 / 项目总结\n当前阶段：{}",
                meta.file_type
            )
        } else {
            meta.byline.clone()
        };
        text(192.0, &stage, 10.0, accent());
        text(214.0, &meta.institution, 16.0, ink);
        text(225.0, &mdx::cover::chinese_date(&meta.date), 12.0, ink);
        let security = mdx::cover::security_label(&meta.security);
        let security = if !security.is_empty() && !meta.security_years.is_empty() {
            format!("{security}★{}", meta.security_years)
        } else {
            security.into()
        };
        text(
            -10.0,
            &format!("{security}    {}    {}", meta.file_number, meta.version),
            9.0,
            theme::paper::ink_muted(),
        );
    });
}
