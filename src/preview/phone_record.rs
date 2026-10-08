//! 电话记录单纸面预览：左栏来电与建议，右栏首长批示留白。
use super::render::PreviewOutput;
use super::*;
use crate::{export, models::DraftInput};

fn field(ui: &mut egui::Ui, m: &Metrics, rect: egui::Rect, text: &str, size: f32, title: bool) {
    // 格内文字只绘制，不推进父光标；位置与导出的水平 / 垂直对齐一致。
    let inner = rect.shrink(m.mm(2.0));
    let galley = ui.fonts_mut(|fonts| {
        fonts.layout(
            text.to_owned(),
            m.font(
                if title {
                    theme::FONT_BIAOSONG
                } else {
                    theme::FONT_FANGSONG
                },
                size,
            ),
            theme::paper::ink(),
            inner.width(),
        )
    });
    let x = if title {
        inner.center().x - galley.size().x / 2.0
    } else {
        inner.left()
    };
    ui.painter().galley(
        egui::pos2(x, inner.center().y - galley.size().y / 2.0),
        galley,
        theme::paper::ink(),
    );
}

/// 使用 A4 的物理高度和公文下边距，不能按正文已用高度再次扩展纸张。
fn record_sheet(ui: &mut egui::Ui, m: &Metrics, contents: impl FnOnce(&mut egui::Ui)) {
    m.next_page();
    ui.horizontal(|ui| {
        ui.add_space(((m.viewport - m.page) / 2.0).max(0.0));
        let (_, paper) = ui.allocate_space(egui::vec2(m.page, m.page_height));
        let background = ui.painter().add(egui::Shape::Noop);
        let origin = paper.min + egui::vec2(m.margin_left, m.margin_top);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    origin,
                    egui::vec2(m.content, m.page_height - m.margin_top - m.mm(35.0)),
                ))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.spacing_mut().item_spacing = egui::Vec2::ZERO;
        child.style_mut().visuals.override_text_color = Some(theme::paper::ink());
        child.style_mut().interaction.selectable_labels = false;
        contents(&mut child);
        let mut paper = paper;
        paper.max.y = paper.bottom().max(child.min_rect().bottom() + m.mm(35.0));
        ui.painter().set(
            background,
            egui::epaint::RectShape::filled(paper, 3.0, theme::paper::bg()),
        );
        ui.painter().rect_stroke(
            paper,
            3.0,
            egui::Stroke::new(1.0, theme::paper::border()),
            egui::StrokeKind::Inside,
        );
        ui.advance_cursor_after_rect(paper);
    });
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
    let mut m = Metrics::new(scale.viewport.unwrap_or(visible.width()), scale.zoom)
        .with_bold_style(ui.ctx());
    // 通用公文预览的抬头留白为 52pt；记录单采用导出模板的 37mm 上边距。
    m.margin_top = m.mm(37.0);
    m.margin_left = m.mm(28.0);
    m.content = m.mm(156.0);
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
    record_sheet(ui, &m, |ui| {
        let page_top = ui.cursor().min.y - m.margin_top;
        let r = &input.phone_record;
        for (title, gap) in [(&r.institution[..], 3.0), ("电话记录单", 5.0)] {
            let (_, rect) = ui.allocate_space(egui::vec2(m.content, m.mm(16.0 * 25.4 / 72.0)));
            let galley = ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap(
                    title.to_owned(),
                    m.font(theme::FONT_BIAOSONG, 22.0),
                    theme::paper::red(),
                )
            });
            ui.painter().galley(
                egui::pos2(
                    rect.center().x - galley.size().x / 2.0,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                theme::paper::red(),
            );
            ui.add_space(m.mm(gap));
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
        let top = egui::pos2(header.left() + m.mm(2.0), header.bottom());
        let body_height = (page_top + m.mm(254.0) - top.y).max(m.mm(150.0));
        let mut body_m = Metrics::new(m.viewport, Some(m.scale))
            .with_bold_style(ui.ctx())
            .with_line_numbers(line_numbers);
        body_m.content = xs[4] - xs[0] - m.mm(4.0);
        body_m.body_pt = 16.0;
        body_m.line = m.pt(28.98 * 72.0 / 72.27);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    top,
                    egui::vec2(xs[4] - xs[0], body_height),
                ))
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                // 最小高度必须在排字前设置，egui 会从当前光标扩展高度。
                // 放在排字后会把整段高度再追加一次，导致纸张接近两页长。
                ui.set_min_height(body_height);
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
                let suggestion = format!("建议：{}", r.suggestion);
                let suggestion_height = ui
                    .fonts_mut(|fonts| {
                        fonts.layout(
                            suggestion.clone(),
                            body_m.body_font(),
                            theme::paper::ink(),
                            body_m.content,
                        )
                    })
                    .rows
                    .len() as f32
                    * body_m.line;
                ui.add_space(
                    (body_height - m.mm(2.0) - suggestion_height - (ui.cursor().min.y - top.y))
                        .max(m.mm(4.0)),
                );
                body_block(ui, &body_m, &suggestion, false);
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
