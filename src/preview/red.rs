//! 红头呈批件：打印分页器、红头版式布局与覆盖绘制。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::red`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::theme;
use crate::export::{LocatedBlock, MarkdownBlock};
use crate::export;
use crate::models::{DraftInput};
use crate::units::{UnitDisplay};
use std::ops::{Range};
use std::sync::{Arc};
use eframe::egui::{Align, Color32, Stroke};
use eframe::egui;
use eframe::egui::text::{LayoutJob};
use crate::preview::{Metrics, BODY_PT, PAREN_PT, LINE_PT, INDENT_CHARS, MM, HEADER_PT, CLOSING_GAP_LINES, OFFICIAL_RED, HOVER_TINT, heading_family, text_format, job, single_line, layout, indent, line_block, is_renderable_paragraph, clickable, sheet, header_unit, document_number, signature_date, content_block};

/// 普通文种正文区渲染参数。红头呈批件走下方独立的打印分页模型。
pub(crate) struct BodyRun {
pub(crate)     compact: bool,
pub(crate)     compact_level: u8,
pub(crate)     numbered: bool,
}

/// 红头呈批件打印预览中的一段可见文字。正文段落可以被切成多个 fragment：
/// 首页片段按 100mm 排，续页片段重新按 156mm 排，而不是沿用首页的换行结果。
pub(crate) struct RedPrintFragment {
    pub(crate) range: Option<Range<usize>>,
    pub(crate) galley: Arc<egui::Galley>,
    x: f32,
    y: f32,
    pub(crate) width: f32,
    visible_height: f32,
    align: Align,
}

#[derive(Default)]
pub(crate) struct RedPrintPage {
    pub(crate) fragments: Vec<RedPrintFragment>,
}

#[derive(Clone, Copy)]
pub(crate) enum RedTextStyle {
    Body,
    Heading(u8),
    List,
}

pub(crate) struct RedPrintLayout {
    pub(crate) pages: Vec<RedPrintPage>,
    page_index: usize,
    cursor_y: f32,
    first_body_bottom: f32,
}

impl RedPrintLayout {

    pub(crate) fn new(first_cursor_y: f32, first_body_bottom: f32) -> Self {
        Self {
            pages: vec![RedPrintPage::default()],
            page_index: 0,
            cursor_y: first_cursor_y,
            first_body_bottom,
        }
    }

    pub(crate) fn body_left(&self, metrics: &Metrics) -> f32 {
        metrics.mm(28.0)
    }

    pub(crate) fn body_width(&self, metrics: &Metrics) -> f32 {
        metrics.mm(if self.page_index == 0 { 100.0 } else { 156.0 })
    }

    pub(crate) fn body_bottom(&self, metrics: &Metrics) -> f32 {
        if self.page_index == 0 {
            self.first_body_bottom
        } else {
            metrics.mm(37.0 + 225.0)
        }
    }

    pub(crate) fn next_page(&mut self, metrics: &Metrics) {
        self.pages.push(RedPrintPage::default());
        self.page_index += 1;
        self.cursor_y = metrics.mm(37.0);
    }

    pub(crate) fn push(&mut self, fragment: RedPrintFragment) {
        self.pages[self.page_index].fragments.push(fragment);
    }
}

/// 返回当前页能容纳的最大落款间距：优先 3 行，临界时缩为 2 行或 1 行。
/// 连 1 行都放不下时返回 `None`，由调用方另起“此页无正文”页。
pub(crate) fn fitting_closing_gap_lines(
    cursor_y: f32,
    signature_height: f32,
    page_bottom: f32,
    line_height: f32,
) -> Option<usize> {
    (1..=CLOSING_GAP_LINES)
        .rev()
        .find(|lines| cursor_y + *lines as f32 * line_height + signature_height <= page_bottom)
}

/// 开启专门的落款页，并保持“此页无正文”与落款之间空 3 行。
pub(crate) fn red_start_no_body_closing_page(
    ui: &egui::Ui,
    metrics: &Metrics,
    state: &mut RedPrintLayout,
    left: f32,
) {
    state.next_page(metrics);
    state.cursor_y += metrics.pt(58.0);
    let notice = red_fixed_fragment(
        ui,
        metrics,
        &format!("{}（此页无正文）", indent(INDENT_CHARS)),
        theme::FONT_FANGSONG,
        BODY_PT,
        metrics.mm(156.0),
        Align::LEFT,
        None,
        left,
        state.cursor_y,
    );
    state.cursor_y += notice.visible_height;
    state.push(notice);
    state.cursor_y += metrics.line * CLOSING_GAP_LINES as f32;
}

pub(crate) fn red_inline_job(
    metrics: &Metrics,
    width: f32,
    segments: &[export::InlineSegment],
    style: RedTextStyle,
    first_line_indent: bool,
) -> LayoutJob {
    let mut job = job(width);
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    if first_line_indent {
        job.append(
            &indent(INDENT_CHARS),
            0.0,
            text_format(normal.clone(), metrics.line),
        );
    }
    for segment in segments {
        let font = match style {
            RedTextStyle::Heading(level) => metrics.font(heading_family(level), BODY_PT),
            RedTextStyle::List => normal.clone(),
            RedTextStyle::Body if segment.parenthesized => {
                metrics.font(theme::FONT_KAITI, PAREN_PT)
            }
            RedTextStyle::Body if segment.bold => metrics.font(theme::FONT_HEITI, BODY_PT),
            RedTextStyle::Body => normal.clone(),
        };
        job.append(&segment.text, 0.0, text_format(font, metrics.line));
    }
    job
}

pub(crate) fn red_drop_chars(segments: &mut Vec<export::InlineSegment>, mut count: usize) {
    while count > 0 && !segments.is_empty() {
        let chars = segments[0].text.chars().count();
        if count >= chars {
            count -= chars;
            segments.remove(0);
        } else {
            segments[0].text = segments[0].text.chars().skip(count).collect();
            count = 0;
        }
    }
}

/// 把一个逻辑段落连续排入页面。首页放得下多少行就放多少行；剩余文本进入
/// 下一页后重新生成 galley，因此第二页第一行立即采用标准 156mm 版心。
pub(crate) fn red_place_flow_text(
    ui: &egui::Ui,
    metrics: &Metrics,
    layout_state: &mut RedPrintLayout,
    range: Range<usize>,
    mut segments: Vec<export::InlineSegment>,
    style: RedTextStyle,
    first_line_indent: bool,
) {
    let mut first_fragment = true;
    while !segments.is_empty() {
        let width = layout_state.body_width(metrics);
        let available = layout_state.body_bottom(metrics) - layout_state.cursor_y;
        if available < metrics.line * 0.75 {
            layout_state.next_page(metrics);
            continue;
        }
        let indent_this_fragment = first_fragment && first_line_indent;
        let galley = layout(
            ui,
            red_inline_job(metrics, width, &segments, style, indent_this_fragment),
        );
        let fitting = galley
            .rows
            .iter()
            .take_while(|row| row.rect().bottom() <= available + 0.5)
            .count();
        if fitting == 0 {
            layout_state.next_page(metrics);
            continue;
        }
        let visible_height = galley.rows[fitting - 1].rect().bottom();
        let mut consumed = galley.rows[..fitting]
            .iter()
            .map(|row| row.glyphs.len())
            .sum::<usize>();
        if indent_this_fragment {
            consumed = consumed.saturating_sub(INDENT_CHARS as usize);
        }
        let all_fit = fitting == galley.rows.len();
        layout_state.push(RedPrintFragment {
            range: Some(range.clone()),
            galley,
            x: layout_state.body_left(metrics),
            y: layout_state.cursor_y,
            width,
            visible_height,
            align: Align::LEFT,
        });
        layout_state.cursor_y += visible_height;
        if all_fit {
            break;
        }
        if consumed == 0 {
            layout_state.next_page(metrics);
            continue;
        }
        red_drop_chars(&mut segments, consumed);
        first_fragment = false;
        layout_state.next_page(metrics);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn red_fixed_fragment(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    width: f32,
    align: Align,
    range: Option<Range<usize>>,
    x: f32,
    y: f32,
) -> RedPrintFragment {
    let mut job = job(width);
    job.halign = align;
    job.append(
        text,
        0.0,
        text_format(metrics.font(family, size), metrics.line),
    );
    let galley = layout(ui, job);
    RedPrintFragment {
        range,
        visible_height: galley.size().y,
        galley,
        x,
        y,
        width,
        align,
    }
}

pub(crate) fn red_responsible_rows(input: &DraftInput, display: &UnitDisplay) -> Vec<[String; 3]> {
    let entries = crate::models::joint_responsible_entries(&input.profile);
    if entries.is_empty() {
        return vec![[String::new(), String::new(), String::new()]];
    }
    entries
        .iter()
        .map(|entry| {
            [
                display.abbr(&entry.unit),
                entry.name.clone(),
                entry.phone.clone(),
            ]
        })
        .collect()
}

pub(crate) fn red_build_print_layout(
    ui: &egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
    body: &[&LocatedBlock],
    title: &(String, Range<usize>),
    attachment_names: &[String],
) -> (RedPrintLayout, Vec<[String; 3]>) {
    let rows = red_responsible_rows(input, display);
    // TeX 承办区 = 0.4mm 红线 + 1mm 间距 + 每条固定 28pt 基线。
    let record_height = metrics.mm(1.4) + metrics.line * rows.len().max(1) as f32;
    let first_body_bottom = metrics.mm(37.0 + 225.0) - record_height - metrics.mm(2.0);
    let left = metrics.mm(28.0);
    let narrow = metrics.mm(100.0);
    let mut state = RedPrintLayout::new(metrics.mm(37.0 + 55.0), first_body_bottom);

    if !title.0.is_empty() {
        let fragment = red_fixed_fragment(
            ui,
            metrics,
            &title.0,
            theme::FONT_BIAOSONG,
            18.0,
            narrow,
            Align::Center,
            Some(title.1.clone()),
            left + narrow / 2.0,
            state.cursor_y,
        );
        state.cursor_y += fragment.visible_height;
        state.push(fragment);
    }
    state.cursor_y += metrics.line;
    let leaders = display.reporting_leaders(&input.profile.reporting_leaders);
    if !leaders.trim().is_empty() {
        let fragment = red_fixed_fragment(
            ui,
            metrics,
            &format!("{}：", leaders.trim().trim_end_matches('：')),
            theme::FONT_KAITI,
            BODY_PT,
            narrow,
            Align::LEFT,
            None,
            left,
            state.cursor_y,
        );
        state.cursor_y += fragment.visible_height;
        state.push(fragment);
    }

    let mut counters = [0usize; 4];
    for located in body {
        match &located.block {
            MarkdownBlock::Title(_) | MarkdownBlock::Marker(_) | MarkdownBlock::Html(_) => {}
            MarkdownBlock::Heading(level, text) => {
                let Some(text) = export::official_heading_text(*level, text, &mut counters) else {
                    continue;
                };
                red_place_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    vec![export::InlineSegment {
                        text: format!("{}{}", indent(INDENT_CHARS), export::plain_text(&text)),
                        bold: false,
                        parenthesized: false,
                    }],
                    RedTextStyle::Heading(*level),
                    false,
                );
            }
            MarkdownBlock::Paragraph(text) if is_renderable_paragraph(text) => {
                red_place_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    export::inline_segments(text),
                    RedTextStyle::Body,
                    true,
                );
            }
            MarkdownBlock::ListItem(text) => {
                red_place_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    vec![export::InlineSegment {
                        text: format!("　•{}", export::plain_text(text)),
                        bold: false,
                        parenthesized: false,
                    }],
                    RedTextStyle::List,
                    false,
                );
            }
            MarkdownBlock::Table { rows, .. } => {
                // 表格和图片与 TeX 一致，不进入首页批示窄栏。
                if state.page_index == 0 {
                    state.next_page(metrics);
                }
                let text = rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| export::plain_text(cell))
                            .collect::<Vec<_>>()
                            .join("　│　")
                    })
                    .collect::<Vec<_>>()
                    .join("；");
                red_place_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    vec![export::InlineSegment {
                        text,
                        bold: false,
                        parenthesized: false,
                    }],
                    RedTextStyle::Body,
                    false,
                );
            }
            MarkdownBlock::Image { alt, .. } => {
                if state.page_index == 0 {
                    state.next_page(metrics);
                }
                red_place_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    vec![export::InlineSegment {
                        text: format!("〔图片：{}〕", export::plain_text(alt)),
                        bold: false,
                        parenthesized: false,
                    }],
                    RedTextStyle::Body,
                    false,
                );
            }
            MarkdownBlock::Paragraph(_) => {}
        }
    }

    if !attachment_names.is_empty() {
        if state.page_index == 0 {
            state.next_page(metrics);
        }
        state.cursor_y += metrics.line * 2.0;
        for (index, name) in attachment_names.iter().enumerate() {
            let label = if attachment_names.len() == 1 {
                format!("附件：{name}")
            } else if index == 0 {
                format!("附件{}：{name}", index + 1)
            } else {
                format!("　　{}：{name}", index + 1)
            };
            red_place_flow_text(
                ui,
                metrics,
                &mut state,
                0..0,
                vec![export::InlineSegment {
                    text: label,
                    bold: false,
                    parenthesized: false,
                }],
                RedTextStyle::Body,
                false,
            );
        }
    }

    let signature_units = display
        .white_paper_signature_units(input)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect::<Vec<_>>();
    let signature_height = metrics.line * (signature_units.len().max(1) * 2 + 1) as f32;

    // 呈批件落款不得在首页。正文已经续页时优先空 3 行；若仅因这 3 行放不下，
    // 依次缩到 2 行、1 行，避免无谓制造“此页无正文”页。
    let body_ended_on_first = state.page_index == 0;
    if body_ended_on_first {
        red_start_no_body_closing_page(ui, metrics, &mut state, left);
    } else if let Some(gap_lines) = fitting_closing_gap_lines(
        state.cursor_y,
        signature_height,
        metrics.mm(37.0 + 225.0),
        metrics.line,
    ) {
        state.cursor_y += metrics.line * gap_lines as f32;
    } else {
        red_start_no_body_closing_page(ui, metrics, &mut state, left);
    }
    let signature_right = left + metrics.mm(156.0 - crate::export::SIGNATURE_ROOM_MM);
    for (index, unit) in signature_units.iter().enumerate() {
        if index > 0 {
            state.cursor_y += metrics.line;
        }
        let fragment = red_fixed_fragment(
            ui,
            metrics,
            unit,
            theme::FONT_FANGSONG,
            BODY_PT,
            metrics.mm(116.0),
            Align::Max,
            None,
            signature_right,
            state.cursor_y,
        );
        state.cursor_y += fragment.visible_height;
        state.push(fragment);
    }
    state.cursor_y += metrics.line;
    let date = red_fixed_fragment(
        ui,
        metrics,
        &signature_date(input),
        theme::FONT_FANGSONG,
        BODY_PT,
        metrics.mm(116.0),
        Align::Center,
        None,
        left + metrics.mm(116.0) / 2.0,
        state.cursor_y,
    );
    state.push(date);

    (state, rows)
}

pub(crate) fn red_overlay_text(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    color: Color32,
) -> Arc<egui::Galley> {
    let mut format = text_format(metrics.font(family, size), metrics.line);
    format.color = color;
    layout(ui, single_line(text, format))
}

pub(crate) fn paint_red_approval_overlay(
    ui: &egui::Ui,
    metrics: &Metrics,
    page: egui::Rect,
    input: &DraftInput,
    display: &UnitDisplay,
    rows: &[[String; 3]],
) {
    let painter = ui.painter();
    let at = |x: f32, y: f32| page.min + egui::vec2(metrics.mm(x), metrics.mm(y));
    let text_left = 28.0;
    let text_top = 37.0;
    let text_bottom = text_top + 225.0;
    let record_height = 1.4 + rows.len().max(1) as f32 * (LINE_PT / MM);
    let record_top = text_bottom - record_height;

    if !input.profile.security_level.trim().is_empty() {
        let security = format!(
            "{}★{}",
            input.profile.security_level.trim(),
            input.profile.security_period.trim()
        );
        let galley = red_overlay_text(
            ui,
            metrics,
            &security,
            theme::FONT_HEITI,
            BODY_PT,
            Color32::BLACK,
        );
        painter.galley(at(text_left, text_top + 10.0), galley, Color32::BLACK);
    }

    let unit = header_unit(input, display);
    if !unit.trim().is_empty() {
        let count = unit.chars().count() as f32;
        let mut format = text_format(
            metrics.font(theme::FONT_BIAOSONG, HEADER_PT),
            metrics.pt(HEADER_PT * 1.2),
        );
        format.color = OFFICIAL_RED;
        let natural = layout(ui, single_line(&unit, format.clone())).size().x;
        if count > 1.0 && natural < metrics.mm(156.0) {
            format.extra_letter_spacing =
                ((metrics.mm(156.0) - natural) / (count - 1.0)).min(metrics.pt(HEADER_PT));
        }
        let galley = layout(ui, single_line(&unit, format));
        let center = at(text_left + 78.0, text_top + 30.0);
        painter.galley(
            center - egui::vec2(galley.size().x / 2.0, 0.0),
            galley,
            OFFICIAL_RED,
        );
    }

    let number = document_number(input);
    let number_galley = red_overlay_text(
        ui,
        metrics,
        &number,
        theme::FONT_FANGSONG,
        BODY_PT,
        Color32::BLACK,
    );
    let number_x =
        at(text_left + 78.0, text_top + 43.0) - egui::vec2(number_galley.size().x / 2.0, 0.0);
    painter.galley(number_x, number_galley, Color32::BLACK);

    let red = Stroke::new(metrics.mm(0.4).max(1.0), OFFICIAL_RED);
    painter.line_segment(
        [
            at(text_left, text_top + 48.0),
            at(text_left + 156.0, text_top + 48.0),
        ],
        red,
    );
    painter.line_segment(
        [
            at(text_left + 100.0, text_top + 48.0),
            at(text_left + 100.0, record_top),
        ],
        red,
    );
    let instruction = red_overlay_text(
        ui,
        metrics,
        "批　示",
        theme::FONT_FANGSONG,
        BODY_PT,
        OFFICIAL_RED,
    );
    let instruction_center = at(text_left + 128.0, text_top + 61.0);
    painter.galley(
        instruction_center - egui::vec2(instruction.size().x / 2.0, 0.0),
        instruction,
        OFFICIAL_RED,
    );

    painter.line_segment(
        [at(text_left, record_top), at(text_left + 156.0, record_top)],
        red,
    );
    let columns = [62.86_f32, 39.51, 53.62];
    let labels = ["承办单位：", "联系人：", "电话："];
    for (row_index, row) in rows.iter().enumerate() {
        let y = record_top + 1.4 + row_index as f32 * (LINE_PT / MM);
        let mut x = text_left;
        for column in 0..3 {
            let label = red_overlay_text(
                ui,
                metrics,
                labels[column],
                theme::FONT_FANGSONG,
                BODY_PT,
                OFFICIAL_RED,
            );
            let label_width = label.size().x;
            painter.galley(at(x, y), label, OFFICIAL_RED);
            let value = red_overlay_text(
                ui,
                metrics,
                &row[column],
                theme::FONT_FANGSONG,
                BODY_PT,
                Color32::BLACK,
            );
            let value_pos = if column == 2 {
                at(x + columns[column], y) - egui::vec2(value.size().x, 0.0)
            } else {
                at(x, y) + egui::vec2(label_width, 0.0)
            };
            painter.galley(value_pos, value, Color32::BLACK);
            x += columns[column];
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_red_print_pages(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    layout_state: &RedPrintLayout,
    input: &DraftInput,
    display: &UnitDisplay,
    rows: &[[String; 3]],
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    for (page_index, page_layout) in layout_state.pages.iter().enumerate() {
        if page_index > 0 {
            ui.add_space(14.0);
        }
        ui.horizontal(|ui| {
            let side = ((metrics.viewport - metrics.page) / 2.0).max(0.0);
            ui.add_space(side);
            let (page, _) = ui.allocate_exact_size(
                egui::vec2(metrics.page, metrics.page_height),
                egui::Sense::hover(),
            );
            ui.painter().rect(
                page,
                egui::CornerRadius::same(3),
                Color32::WHITE,
                Stroke::new(1.0, theme::border()),
                egui::StrokeKind::Inside,
            );
            if page_index == 0 {
                paint_red_approval_overlay(ui, metrics, page, input, display, rows);
            } else {
                let page_number = red_overlay_text(
                    ui,
                    metrics,
                    &format!("— {} —", page_index + 1),
                    theme::FONT_FANGSONG,
                    14.0,
                    Color32::BLACK,
                );
                let center = page.min + egui::vec2(metrics.page / 2.0, metrics.mm(282.0));
                ui.painter().galley(
                    center - egui::vec2(page_number.size().x / 2.0, 0.0),
                    page_number,
                    Color32::BLACK,
                );
            }

            for (fragment_index, fragment) in page_layout.fragments.iter().enumerate() {
                let rect_left = match fragment.align {
                    Align::Center => fragment.x - fragment.width / 2.0,
                    Align::Max => fragment.x - fragment.width,
                    Align::Min => fragment.x,
                };
                let top_left = page.min + egui::vec2(rect_left, fragment.y);
                let rect = egui::Rect::from_min_size(
                    top_left,
                    egui::vec2(fragment.width, fragment.visible_height),
                );
                if let Some(range) = &fragment.range
                    && !range.is_empty()
                {
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("red-print-fragment", page_index, fragment_index)),
                        egui::Sense::click(),
                    );
                    if response.clicked() {
                        *clicked = Some(range.clone());
                    }
                    let anchored = anchor.is_some_and(|anchor| {
                        !anchor.is_empty() && anchor.start < range.end && range.start < anchor.end
                    });
                    if anchored && *scroll_to_anchor {
                        response.scroll_to_me(Some(egui::Align::Center));
                        *scroll_to_anchor = false;
                    }
                    if anchored || response.hovered() {
                        ui.painter().rect_filled(
                            rect.expand2(egui::vec2(3.0, 1.0)),
                            3.0,
                            if anchored {
                                theme::accent_soft()
                            } else {
                                HOVER_TINT
                            },
                        );
                    }
                }
                let anchor_pos = match fragment.align {
                    Align::Center | Align::Max | Align::Min => {
                        page.min + egui::vec2(fragment.x, fragment.y)
                    }
                };
                ui.painter().with_clip_rect(rect).galley(
                    anchor_pos,
                    fragment.galley.clone(),
                    Color32::BLACK,
                );
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn red_approval_print_preview(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
    body: &[&LocatedBlock],
    attachments: &[Vec<&LocatedBlock>],
    title: &(String, Range<usize>),
    attachment_names: &[String],
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let (layout_state, rows) =
        red_build_print_layout(ui, metrics, input, display, body, title, attachment_names);
    paint_red_print_pages(
        ui,
        metrics,
        &layout_state,
        input,
        display,
        &rows,
        anchor,
        scroll_to_anchor,
        clicked,
    );

    // 附件仍沿用现有的逐份分页渲染；正文和附件概要已经由上面的打印分页器处理。
    for (index, attachment) in attachments.iter().enumerate() {
        ui.add_space(14.0);
        sheet(ui, metrics, |ui| {
            let mut counters = [0usize; 4];
            line_block(
                ui,
                metrics,
                if attachments.len() == 1 {
                    "附件".to_string()
                } else {
                    format!("附件{}", index + 1)
                }
                .as_str(),
                theme::FONT_HEITI,
                BODY_PT,
                Align::LEFT,
            );
            for located in attachment {
                let range = located.range.clone();
                clickable(ui, &range, anchor, scroll_to_anchor, clicked, |ui| {
                    content_block(ui, metrics, &located.block, &mut counters, true)
                });
            }
        });
    }
}
