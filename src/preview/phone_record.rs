//! 电话记录单纸面预览：左栏来电与建议，右栏首长批示留白。
use super::render::PreviewOutput;
use super::*;
use crate::{export, models::DraftInput};

fn field(ui: &mut egui::Ui, m: &Metrics, rect: egui::Rect, text: &str, size: f32, title: bool) {
    // 表格格内绘制不能推进父级光标，否则正文会回退到元数据行。
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(m.mm(2.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.add(
        egui::Label::new(egui::RichText::new(text).font(m.font(
            if title {
                theme::FONT_BIAOSONG
            } else {
                theme::FONT_FANGSONG
            },
            size,
        )))
        .wrap(),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show(
    ui: &mut egui::Ui,
    input: &DraftInput,
    markdown: &str,
    scale: PreviewScale,
    anchor: Option<&Range<usize>>,
    mut scroll: bool,
    numbering: &crate::models::NumberingConfig,
    line_numbers: bool,
) -> PreviewOutput {
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|i| i.content_rect()));
    let m = Metrics::new(scale.viewport.unwrap_or(visible.width()), scale.zoom)
        .with_bold_style(ui.ctx());
    let mut clicked = None;
    let mut counters = [0; 4];
    let blocks = export::parse_markdown_located_with_numbering(markdown, numbering);
    let attachment = blocks
        .iter()
        .position(|b| {
            matches!(
                b.block,
                export::MarkdownBlock::Marker(export::MarkdownSection::Attachment)
            )
        })
        .unwrap_or(blocks.len());
    sheet(ui, &m, |ui| {
        let page_top = ui.cursor().min.y - m.margin_top;
        let r = &input.phone_record;
        for title in [&r.institution, "电话记录单"] {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(title)
                        .font(m.font(theme::FONT_BIAOSONG, 22.0))
                        .color(theme::paper::red()),
                );
            });
            ui.add_space(m.mm(3.0));
        }
        let (_, header) = ui.allocate_space(egui::vec2(m.content, m.mm(33.0)));
        let xs =
            [0.0, 25.0, 66.5, 87.5, 116.0, 160.0].map(|x| header.left() + m.mm(x * 156.0 / 160.0));
        let ys = [0.0, 12.0, 22.5, 33.0].map(|y| header.top() + m.mm(y));
        let rect = |col: usize, row: usize, cols: usize, rows: usize| {
            egui::Rect::from_min_max(
                egui::pos2(xs[col], ys[row]),
                egui::pos2(xs[col + cols], ys[row + rows]),
            )
        };
        let security = export::element_display::security_display(input).unwrap_or_default();
        for (c, y, text, size) in [
            (0, 0, "单位", 16.0),
            (1, 0, r.caller_unit.as_str(), 12.0),
            (2, 0, "电话", 16.0),
            (3, 0, r.caller_phone.as_str(), 12.0),
            (0, 1, "谈话人", 16.0),
            (1, 1, r.caller_person.as_str(), 12.0),
            (2, 1, "密级", 16.0),
            (3, 1, security.as_str(), 12.0),
        ] {
            field(ui, &m, rect(c, y, 1, 1), text, size, false);
        }
        field(ui, &m, rect(0, 2, 1, 1), "时间", 16.0, false);
        field(ui, &m, rect(1, 2, 3, 1), &r.call_time, 12.0, false);
        field(ui, &m, rect(4, 0, 1, 2), "首长批示", 22.0, true);
        let stroke = egui::Stroke::new(0.5, theme::paper::ink());
        for (y, end) in [
            (ys[0], xs[5]),
            (ys[1], xs[4]),
            (ys[2], xs[5]),
            (ys[3], xs[4]),
        ] {
            ui.painter()
                .line_segment([egui::pos2(xs[0], y), egui::pos2(end, y)], stroke);
        }
        for (x, end) in [
            (xs[1], ys[3]),
            (xs[2], ys[2]),
            (xs[3], ys[2]),
            (xs[4], ys[3]),
        ] {
            ui.painter()
                .line_segment([egui::pos2(x, ys[0]), egui::pos2(x, end)], stroke);
        }
        let top = egui::pos2(header.left(), header.bottom());
        let body_height = (page_top + m.mm(254.0) - top.y).max(m.mm(150.0));
        let mut body_m = Metrics::new(m.viewport, Some(m.scale))
            .with_bold_style(ui.ctx())
            .with_line_numbers(line_numbers);
        body_m.content = xs[4] - xs[0] - m.mm(4.0);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    top,
                    egui::vec2(xs[4] - xs[0], body_height),
                ))
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_width(body_m.content);
                ui.add_space(m.mm(2.0));
                for located in &blocks[..attachment] {
                    if matches!(
                        located.block,
                        export::MarkdownBlock::Title(_) | export::MarkdownBlock::Marker(_)
                    ) {
                        continue;
                    }
                    clickable_content_block(
                        ui,
                        &body_m,
                        located,
                        markdown,
                        &mut counters,
                        true,
                        numbering,
                        anchor,
                        &mut scroll,
                        &mut clicked,
                    );
                }
                ui.add_space(
                    (body_height - m.mm(18.0) - (ui.cursor().min.y - top.y)).max(m.mm(4.0)),
                );
                body_block(ui, &body_m, &format!("建议：{}", r.suggestion), false);
                ui.set_min_height(body_height);
            },
        );
        let bottom = ui.cursor().min.y;
        ui.painter().line_segment(
            [egui::pos2(xs[4], ys[3]), egui::pos2(xs[4], bottom)],
            stroke,
        );
        ui.painter().line_segment(
            [egui::pos2(xs[0], bottom), egui::pos2(xs[5], bottom)],
            stroke,
        );
        ui.add_space(m.mm(2.0));
        line_block(
            ui,
            &m,
            &format!(
                "承办单位：{}　联系人：{}　电话：{}",
                input.profile.responsible_unit,
                input.profile.contact_person,
                input.profile.contact_phone
            ),
            theme::FONT_FANGSONG,
            14.0,
            egui::Align::Min,
        );
        for located in &blocks[attachment..] {
            if matches!(located.block, export::MarkdownBlock::Marker(_)) {
                continue;
            }
            clickable_content_block(
                ui,
                &m,
                located,
                markdown,
                &mut counters,
                true,
                numbering,
                anchor,
                &mut scroll,
                &mut clicked,
            );
        }
        clicked = clicked.take().or(gutter::paint(ui, &body_m, anchor));
    });
    PreviewOutput {
        scale: m.scale,
        clicked,
    }
}
