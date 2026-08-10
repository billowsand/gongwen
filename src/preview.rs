//! 公文版式预览：把审校区的 Markdown 连同表单锁定的行文要素，按导出后的样子画出来。
//!
//! 正文版式与 `export::docx` 一一对应——仿宋三号、固定行距 28 磅、首行缩进 2 字，
//! 一级标题黑体、二级标题楷体，文档标题小标宋二号，括号内容楷体四号，表格四号且
//! 列宽取自导出器。抬头、主送、落款、版记则对齐 `gonghan-gwa.cls`（即 LaTeX/PDF
//! 这条真正出文的链路）：红头小标宋 29 磅红色分散、红色反线 0.53mm、密级黑体三号
//! 顶格、主送/呈报领导楷体三号顶格、落款右侧 11cm 块、版记四号三线表。
//!
//! 界面里无法精确分页，因此只按“正文 / 每份附件”分张纸；版记在成文时贴版心底部，
//! 预览中紧跟落款之后。
//!
//! 来自 Markdown 的每一块都记着自己在源码中的字节范围（`export::LocatedBlock`），
//! 点一下就能把编辑器的光标带过去，改稿时不必在两栏之间人工找位置。

use crate::{
    export::{self, LocatedBlock, MarkdownBlock, MarkdownSection},
    images,
    models::{DraftInput, JointIssuanceMode, LetterVersion, StyleMode, TemplateKind, split_units},
    theme,
    units::UnitDisplay,
};
use eframe::egui::{
    self, Align, Color32, FontId, Stroke,
    text::{LayoutJob, TextFormat},
};
use std::{ops::Range, sync::Arc};

/// Word 的“磅”换算成 egui 的逻辑像素（96dpi）。
const PT: f32 = 96.0 / 72.0;

// 与 export::docx 的常量对应（那边是半磅，这里是磅）。
const BODY_PT: f32 = 16.0; // 三号
const TITLE_PT: f32 = 22.0; // 二号
const TABLE_PT: f32 = 14.0; // 四号
const PAREN_PT: f32 = 14.0; // 括号内容，楷体四号
const LINE_PT: f32 = 28.0; // 固定行距
const TABLE_LINE_PT: f32 = 21.0; // 表格内行距（420 缇）
const INDENT_CHARS: f32 = 2.0; // 首行缩进 2 字
const LIST_INDENT_PT: f32 = 21.0; // 列表项左缩进 420 缇

// 抬头与版记的参数取自 gonghan-gwa.cls。
/// 毫米换算成磅。
const MM: f32 = 72.0 / 25.4;
const HEADER_PT: f32 = 29.0; // 红头字号 \HeaderFontSize
const HEADER_RULE_MM: f32 = 0.53; // 红色反线粗细
const HEADER_RULE_GAP_MM: f32 = 4.0; // 红头与反线之间
const HEADER_MAX_GAP_EM: f32 = 1.0; // 红头最大字距 \HeaderMaxGap
const RECORD_PT: f32 = 14.0; // 版记四号
/// 联系电话列宽：标签 5em + 11 位半角数字 5.5em + 0.5em 余量。
const RECORD_PHONE_COLUMN_EM: f32 = 11.0;
const CLOSING_GAP_MM: f32 = 20.0; // 正文与落款之间 \ClosingGap
const RECORD_GAP_MM: f32 = 10.0; // 落款与版记之间 \SignatureRecordGap
const SIGNATURE_WIDTH_MM: f32 = 110.0; // 落款块宽 11cm
const WHITE_PAPER_ROOM_MM: f32 = 40.0; // 白头件落款右侧签字空间 \WhitePaperSignatureRoom
const JOINT_COLUMN_MM: f32 = 72.0; // 联合发文落款每列宽
const JOINT_ROW_GAP_MM: f32 = 45.0; // 联合发文落款行间公章空档
const JOINT_DATE_GAP_MM: f32 = 6.0; // 联合发文落款与成文日期之间
const WHITE_PAPER_BLANK_LINES: f32 = 10.0; // 白头件密级后空 10 行
/// 红头与红色反线的颜色：类里用的是 LaTeX 的纯红。
const OFFICIAL_RED: Color32 = Color32::from_rgb(0xFF, 0x00, 0x00);
/// 预览版留白占位：规格 §3.3 统一 1em 宽。
const PREVIEW_PLACEHOLDER: &str = "\u{2003}";
/// 鼠标悬停在可点击块上时的淡底。
const HOVER_TINT: Color32 = Color32::from_rgb(0xF7, 0xF2, 0xEC);

// A4 版心：页宽 210mm，左边距 1587 缇、右边距 1474 缇。
const PAGE_PT: f32 = 595.28;
const MARGIN_LEFT_PT: f32 = 79.35;
const MARGIN_RIGHT_PT: f32 = 73.70;
const MARGIN_TOP_PT: f32 = 52.0;

/// 缩放后的版式尺寸，单位都是 egui 逻辑像素。
struct Metrics {
    scale: f32,
    /// 窗格里真正看得见的宽度，用来把纸张居中。
    viewport: f32,
    page: f32,
    content: f32,
    margin_left: f32,
    margin_top: f32,
    line: f32,
}

impl Metrics {
    /// `zoom` 为 None 时按可用宽度自适应，否则按给定倍率。
    /// `available` 必须是调用方在进入滚动区之前量好的宽度：滚动方向上的
    /// `available_width()` 是无穷大，拿进去算会把整页放大到上限。
    fn new(available: f32, zoom: Option<f32>) -> Self {
        let natural = PAGE_PT * PT;
        let scale = match zoom {
            Some(zoom) => zoom,
            // 留出滚动条与外边距，再夹到一个仍然读得清的区间。
            None => ((available - 24.0) / natural).clamp(0.3, 1.6),
        };
        Self {
            scale,
            viewport: available,
            page: PAGE_PT * PT * scale,
            content: (PAGE_PT - MARGIN_LEFT_PT - MARGIN_RIGHT_PT) * PT * scale,
            margin_left: MARGIN_LEFT_PT * PT * scale,
            margin_top: MARGIN_TOP_PT * PT * scale,
            line: LINE_PT * PT * scale,
        }
    }

    fn pt(&self, size: f32) -> f32 {
        size * PT * self.scale
    }

    fn mm(&self, size: f32) -> f32 {
        self.pt(size * MM)
    }

    fn font(&self, family: &str, size: f32) -> FontId {
        FontId::new(self.pt(size), theme::official_family(family))
    }
}

/// 正文各级标题的字体：与 `export::docx::heading_paragraph` 保持一致。
fn heading_family(level: u8) -> &'static str {
    match level {
        2 => theme::FONT_HEITI,
        3 => theme::FONT_KAITI,
        _ => theme::FONT_FANGSONG,
    }
}

fn text_format(font: FontId, line: f32) -> TextFormat {
    TextFormat {
        font_id: font,
        color: Color32::BLACK,
        line_height: Some(line),
        ..Default::default()
    }
}

fn job(width: f32) -> LayoutJob {
    LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: width,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 单行不换行的排版任务，用于量红头这类需要先测宽度的文字。
fn single_line(text: &str, format: TextFormat) -> LayoutJob {
    let mut job = job(f32::INFINITY);
    job.append(text, 0.0, format);
    job
}

fn layout(ui: &egui::Ui, job: LayoutJob) -> Arc<egui::Galley> {
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

fn draw(ui: &mut egui::Ui, job: LayoutJob) {
    let galley = layout(ui, job);
    ui.add(egui::Label::new(galley));
}

/// 占一块高 `height` 的版心宽区域，把绘制交给回调。抬头、落款、版记这些需要
/// 自己算横向位置的块都走这里。
fn place(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    height: f32,
    paint: impl FnOnce(&egui::Painter, egui::Rect),
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
    paint(ui.painter(), rect);
}

/// 把若干行叠放在版心内的一个块里。`left` 是块左沿相对版心左沿的偏移，
/// `align` 同时决定每行的对齐方式和锚点位置。
fn stacked(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    galleys: &[Arc<egui::Galley>],
    left: f32,
    width: f32,
    align: Align,
) {
    let height = galleys.iter().map(|galley| galley.size().y).sum::<f32>();
    place(ui, metrics, height, |painter, rect| {
        let anchor = match align {
            Align::Center => rect.left() + left + width / 2.0,
            Align::Max => rect.left() + left + width,
            _ => rect.left() + left,
        };
        let mut y = rect.top();
        for galley in galleys {
            painter.galley(egui::pos2(anchor, y), galley.clone(), Color32::BLACK);
            y += galley.size().y;
        }
    });
}

/// 排一行文字并返回 galley：`width` 为换行宽度，`align` 为行内对齐。
fn line_galley(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    font: FontId,
    width: f32,
    align: Align,
) -> Arc<egui::Galley> {
    let mut job = job(width);
    job.halign = align;
    job.append(text, 0.0, text_format(font, metrics.line));
    layout(ui, job)
}

/// 首行缩进用两个全角空格实现：仿宋的全角空格正好一个字宽，
/// 两个即 2 字，与 Word 的 640 缇一致。
fn indent(count: f32) -> String {
    "\u{3000}".repeat(count as usize)
}

/// 一行文字的整段渲染（居中标题、缩进标题等都走这里）。
fn line_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    align: Align,
) {
    let mut job = job(metrics.content);
    job.halign = align;
    job.append(
        text,
        0.0,
        text_format(metrics.font(family, size), metrics.line),
    );
    if align == Align::Center {
        // halign 只让 galley 内部每行相对自身宽度居中，经 Label 摆到光标处后，
        // 单行标题会偏左、多行才看似居中。这里把 galley 直接画在版心正中，
        // 让每一行都相对版心宽居中，与 Word 的居中段落一致。
        let galley = layout(ui, job);
        let height = galley.size().y;
        place(ui, metrics, height, |painter, rect| {
            painter.galley(
                egui::pos2(rect.left() + metrics.content / 2.0, rect.top()),
                galley,
                Color32::BLACK,
            );
        });
    } else {
        draw(ui, job);
    }
}

/// 正文段落：仿宋三号、首行缩进 2 字；行内保留加粗与括号楷体。
///
/// 这里不开 `justify`：egui 在两端对齐时会把行首空白排除在对齐范围外，首行缩进
/// 的两个全角空格会被直接吃掉。中文正文各字等宽，行末本就基本对齐，取舍下
/// 保住缩进更要紧。
fn body_block(ui: &mut egui::Ui, metrics: &Metrics, text: &str, first_line_indent: bool) {
    let mut job = job(metrics.content);
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    if first_line_indent {
        job.append(
            &indent(INDENT_CHARS),
            0.0,
            text_format(normal.clone(), metrics.line),
        );
    }
    append_inline(&mut job, metrics, text, &normal);
    draw(ui, job);
}

/// 行内片段按导出规则上色：括号内容楷体四号，加粗用黑体近似（egui 不做假粗）。
fn append_inline(job: &mut LayoutJob, metrics: &Metrics, text: &str, normal: &FontId) {
    for segment in export::inline_segments(text) {
        let font = if segment.parenthesized {
            metrics.font(theme::FONT_KAITI, PAREN_PT)
        } else if segment.bold {
            metrics.font(theme::FONT_HEITI, BODY_PT)
        } else {
            normal.clone()
        };
        job.append(&segment.text, 0.0, text_format(font, metrics.line));
    }
}

/// 与导出一致：空段落和 HTML 包裹行不成段。
fn is_renderable_paragraph(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains("<div") && !text.contains("</div")
}

/// 表格：四号字、行距 21 磅，表头黑体居中，列宽直接取导出器算好的智能列宽，
/// 因此预览的列宽与导出的 Word 表格一致。
fn table_block(ui: &mut egui::Ui, metrics: &Metrics, rows: &[Vec<String>]) {
    let columns = export::table_columns(rows);
    if columns.is_empty() {
        return;
    }
    let widths = columns
        .iter()
        .map(|column| metrics.content * column.fraction)
        .collect::<Vec<_>>();

    // 单元格内边距不能超过列宽的一小部分，否则窄列会算出负的换行宽度。
    let line = metrics.pt(TABLE_LINE_PT);
    let stroke = Stroke::new(1.0_f32.max(metrics.scale), Color32::BLACK);
    for (index, row) in rows.iter().enumerate() {
        let header = index == 0;
        let font = metrics.font(
            if header {
                theme::FONT_HEITI
            } else {
                theme::FONT_FANGSONG
            },
            TABLE_PT,
        );
        let cells = widths
            .iter()
            .enumerate()
            .map(|(column, width)| {
                let padding = metrics.pt(3.0).min(width * 0.12);
                let text = export::plain_text(row.get(column).map_or("", String::as_str));
                let mut job = job((width - 2.0 * padding).max(1.0));
                // 表头一律居中；正文列按导出器判定的对齐方式。
                let centered = header || columns[column].centered;
                job.halign = if centered { Align::Center } else { Align::LEFT };
                job.append(&text, 0.0, text_format(font.clone(), line));
                let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
                (galley, padding, centered)
            })
            .collect::<Vec<_>>();
        let height = cells
            .iter()
            .map(|(galley, padding, _)| galley.size().y + 2.0 * padding)
            .fold(line, f32::max);

        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        let mut x = rect.left();
        for (column, (galley, padding, centered)) in cells.iter().enumerate() {
            if column > 0 {
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    stroke,
                );
            }
            // halign 决定每行相对锚点的位置：居中时锚点取单元格中线，靠左时取内边距。
            let anchor = if *centered {
                x + widths[column] / 2.0
            } else {
                x + padding
            };
            let top = rect.top() + (height - galley.size().y) / 2.0;
            painter.galley(egui::pos2(anchor, top), galley.clone(), Color32::BLACK);
            x += widths[column];
        }
    }
}

// ── 行文要素：抬头、主送、落款、版记 ────────────────────────────────────────
// 版式对齐 gonghan-gwa.cls：函稿/电话通知走 \DocumentHeader，白头件走
// \WhitePaperHeader，会议议程走 \MeetingAgendaHeader，普通公文走 \PlainDocumentHeader。

/// 联合发文模式 1：多家单位并列成文，落款与版记都改成多行。
fn is_joint_mode_one(input: &DraftInput) -> bool {
    input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1
}

/// 红头上印的发文机关：联合发文取主办单位，其余取发文单位，一律展开为全称。
fn header_unit(input: &DraftInput, display: &UnitDisplay) -> String {
    let external = input.uses_external_unit_names();
    if !is_joint_mode_one(input) {
        return display.full_name_for(&input.profile.issuing_unit, external);
    }
    let units = split_units(&input.profile.joint_issuing_units);
    let main = input.profile.main_issuing_unit.trim();
    let chosen = if units.iter().any(|unit| unit == main) {
        main.to_string()
    } else {
        units.first().cloned().unwrap_or_default()
    };
    display.full_name_for(&chosen, external)
}

/// 落款单位：留空时回落发文单位；电话通知用简称并逐字加空格，其余用全称。
fn signature_unit(input: &DraftInput, display: &UnitDisplay) -> String {
    let raw = if input.profile.signing_unit.trim().is_empty() {
        input.profile.issuing_unit.trim()
    } else {
        input.profile.signing_unit.trim()
    };
    match input.kind {
        TemplateKind::PhoneNotice => display.abbr_spaced(raw),
        TemplateKind::WhitePaper => display.full_name(raw),
        _ => display.full_name_for(raw, input.uses_external_unit_names()),
    }
}

/// 成文日期。预览版把“日”留成 1em 空位，与导出一致。
fn signature_date(input: &DraftInput) -> String {
    let preview = input.profile.letter_version == LetterVersion::Preview;
    match export::chinese_date_parts(&input.date) {
        Some((year, month, day)) => {
            let day = if preview { PREVIEW_PLACEHOLDER } else { day };
            format!("{year}年{month}月{day}日")
        }
        None => input.date.trim().to_string(),
    }
}

/// 代章标注：当前落款单位在标准词库中启用代章时返回“（代章）”。
fn signature_seal_mark(input: &DraftInput, display: &UnitDisplay) -> Option<&'static str> {
    if export::seals_on_behalf(input, display) {
        Some("（代章）")
    } else {
        None
    }
}

/// 密级行的文字：“密级★保密期限”，勾选指人专办时空一个全角空格再标注。
/// 未标注密级时返回 None，整行不排。仅用于测试断言：实际渲染在 `security_line`
/// 里按“黑体 + 数字等宽”分段画，不经过这里。
#[cfg(test)]
fn security_text(input: &DraftInput) -> Option<String> {
    let level = input.profile.security_level.trim();
    if level.is_empty() {
        return None;
    }
    let period = input.profile.security_period.trim();
    let mut text = format!("{level}★{period}");
    // 普通公文不带“指人专办”，与 export::latex::security_commands 一致。
    if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        text.push('\u{2003}');
        text.push_str("指人专办");
    }
    Some(text)
}

/// 密级行：黑体三号顶格。返回是否真的画了东西，供调用方决定要不要留后续空行。
/// 保密期限的数字随整行用黑体，不另设等宽西文字体。
fn security_line(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput) -> bool {
    let level = input.profile.security_level.trim();
    if level.is_empty() {
        return false;
    }
    let period = input.profile.security_period.trim();
    let special = if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        "\u{2003}指人专办"
    } else {
        ""
    };
    let mut job = job(metrics.content);
    job.halign = Align::LEFT;
    let heiti = metrics.font(theme::FONT_HEITI, BODY_PT);
    job.append(
        &format!("{level}★{period}"),
        0.0,
        text_format(heiti.clone(), metrics.line),
    );
    if !special.is_empty() {
        job.append(special, 0.0, text_format(heiti.clone(), metrics.line));
    }
    draw(ui, job);
    true
}

/// 发文字号：代字〔年〕序号 号。预览版把流水号留成 1em 空位，与导出一致。
fn document_number(input: &DraftInput) -> String {
    let year = input.document_year();
    let serial = if input.profile.letter_version == LetterVersion::Preview {
        PREVIEW_PLACEHOLDER
    } else {
        input.profile.document_number.trim()
    };
    format!(
        "{}〔{year}〕{serial} 号",
        input.profile.department_code.trim()
    )
}

/// 红头（发文机关标志）：小标宋 29 磅红色，排得下就只拉开字距（上限 1em）、
/// 字形不变，排不下才缩小字号；下面是与版心等宽的红色反线。
fn red_header(ui: &mut egui::Ui, metrics: &Metrics, unit: &str) {
    if unit.trim().is_empty() {
        return;
    }
    let count = unit.chars().count() as f32;
    let em = metrics.pt(HEADER_PT);
    let mut format = text_format(metrics.font(theme::FONT_BIAOSONG, HEADER_PT), em * 1.2);
    format.color = OFFICIAL_RED;

    let natural = layout(ui, single_line(unit, format.clone())).size().x;
    let mut block = natural;
    if natural < metrics.content && count > 1.0 {
        let gap = ((metrics.content - natural) / (count - 1.0)).min(em * HEADER_MAX_GAP_EM);
        format.extra_letter_spacing = gap;
        block = natural + gap * (count - 1.0);
    } else if natural > metrics.content {
        // 类里是横向压扁字形，egui 只能整体缩；两者都保证一行放得下。
        let shrunk = HEADER_PT * metrics.content / natural;
        format.font_id = metrics.font(theme::FONT_BIAOSONG, shrunk);
        block = metrics.content;
    }
    let galley = layout(ui, single_line(unit, format));
    place(ui, metrics, galley.size().y, |painter, rect| {
        let left = rect.left() + (metrics.content - block) / 2.0;
        painter.galley(egui::pos2(left, rect.top()), galley.clone(), OFFICIAL_RED);
    });

    let gap = metrics.mm(HEADER_RULE_GAP_MM);
    let thickness = metrics.mm(HEADER_RULE_MM).max(1.0);
    place(ui, metrics, gap + thickness, |painter, rect| {
        let top = rect.top() + gap;
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), top),
                egui::vec2(metrics.content, thickness),
            ),
            0.0,
            OFFICIAL_RED,
        );
    });
}

/// 份号与发文字号同一行：左端份号（黑体三号），右端“代字〔年〕序号 号”（仿宋三号）。
/// 份号类里固定为 01，导出器不覆盖，这里照排。
fn serial_and_number(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput) {
    let number = document_number(input);
    let left = line_galley(
        ui,
        metrics,
        "01",
        metrics.font(theme::FONT_HEITI, BODY_PT),
        metrics.content,
        Align::LEFT,
    );
    let right = line_galley(
        ui,
        metrics,
        &number,
        metrics.font(theme::FONT_FANGSONG, BODY_PT),
        metrics.content,
        Align::Max,
    );
    let height = left.size().y.max(right.size().y);
    place(ui, metrics, height, |painter, rect| {
        painter.galley(rect.left_top(), left.clone(), Color32::BLACK);
        painter.galley(
            egui::pos2(rect.right(), rect.top()),
            right.clone(),
            Color32::BLACK,
        );
    });
}

/// 抬头：按文种排出密级、红头、份号文号，并留出与标题之间的空行。
fn header_block(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput, display: &UnitDisplay) {
    match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => {
            red_header(ui, metrics, &header_unit(input, display));
            // 电话通知不编发文字号，只有函稿排份号那一行。
            if input.kind == TemplateKind::OfficialLetter {
                serial_and_number(ui, metrics, input);
            }
            security_line(ui, metrics, input);
            ui.add_space(metrics.line);
        }
        TemplateKind::WhitePaper => {
            security_line(ui, metrics, input);
            ui.add_space(metrics.line * WHITE_PAPER_BLANK_LINES);
        }
        TemplateKind::MeetingAgenda => {
            security_line(ui, metrics, input);
            ui.add_space(metrics.line);
        }
        TemplateKind::PlainDocument => {
            if security_line(ui, metrics, input) {
                ui.add_space(metrics.line);
            }
        }
    }
}

/// 主送机关（函稿、电话通知）或呈报领导（白头件）：楷体三号顶格，末尾加冒号。
fn addressee_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    let text = match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => display.join_hierarchical_for(
            &split_units(&input.profile.recipient),
            input.uses_external_unit_names(),
        ),
        TemplateKind::WhitePaper => display.reporting_leaders(&input.profile.reporting_leaders),
        TemplateKind::PlainDocument | TemplateKind::MeetingAgenda => String::new(),
    };
    let text = text.trim().trim_end_matches('：');
    if text.is_empty() {
        return;
    }
    line_block(
        ui,
        metrics,
        &format!("{text}："),
        theme::FONT_KAITI,
        BODY_PT,
        Align::LEFT,
    );
}

/// 落款：单位与成文日期。函稿/电话通知排在版心右侧 11cm 块内居中；白头件右侧
/// 留 4cm 签字空间、单位与日期之间空一行；联合发文模式 1 排成两列。
fn signature_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    if input.kind == TemplateKind::MeetingAgenda || input.kind == TemplateKind::PlainDocument {
        return;
    }
    ui.add_space(metrics.mm(CLOSING_GAP_MM));
    if crate::models::is_joint_signature(input) {
        joint_signature_block(ui, metrics, input, display);
        return;
    }
    let unit = if is_joint_mode_one(input) {
        // 联合发文模式 1 只剩 1 个发文单位：回落右侧落款，单位取该唯一发文单位。
        let raw = split_units(&input.profile.joint_issuing_units)
            .into_iter()
            .next()
            .unwrap_or_default();
        display.full_name_for(&raw, input.uses_external_unit_names())
    } else {
        signature_unit(input, display)
    };
    if unit.trim().is_empty() {
        return;
    }
    let date = signature_date(input);
    let font = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    let width = metrics.mm(SIGNATURE_WIDTH_MM).min(metrics.content);
    match input.kind {
        TemplateKind::WhitePaper => {
            // 靠右但留出签字空间，块内右对齐；单位与日期之间空一行。
            let room = metrics.mm(WHITE_PAPER_ROOM_MM);
            let left = (metrics.content - room - width).max(0.0);
            let galleys = [
                line_galley(ui, metrics, &unit, font.clone(), width, Align::Max),
                line_galley(ui, metrics, "", font.clone(), width, Align::Max),
                line_galley(ui, metrics, &date, font, width, Align::Max),
            ];
            stacked(ui, metrics, &galleys, left, width, Align::Max);
        }
        _ => {
            let left = metrics.content - width;
            // 代章直接跟在落款单位后面同一行，不另起一行。
            let unit_line = signature_seal_mark(input, display)
                .map(|mark| format!("{unit}{mark}"))
                .unwrap_or_else(|| unit.clone());
            let galleys = [
                line_galley(ui, metrics, &unit_line, font.clone(), width, Align::Center),
                line_galley(ui, metrics, &date, font, width, Align::Center),
            ];
            stacked(ui, metrics, &galleys, left, width, Align::Center);
        }
    }
}

/// 联合发文模式 1 的落款：每行两个单位、各占 72mm 居中；单位多于两个且为奇数时
/// 最后一个跨两列居中；行间留出公章空档；最后把成文日期（含代章）压在主发文单位
/// 所在列下方居中，主单位跨两列时整体居中。
fn joint_signature_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
) {
    let external = input.uses_external_unit_names();
    let mut units = split_units(&input.profile.joint_issuing_units)
        .iter()
        .map(|unit| display.full_name_for(unit, external))
        .collect::<Vec<_>>();
    if units.is_empty() {
        return;
    }
    // 代章直接跟在主发文单位后面同一行，不另起一行。
    if let Some(index) = export::joint_seal_index(input, display)
        && let Some(name) = units.get_mut(index)
    {
        name.push_str("（代章）");
    }
    let column = metrics.mm(JOINT_COLUMN_MM).min(metrics.content / 2.0);
    let table = column * 2.0;
    let left = (metrics.content - table) / 2.0;
    let font = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    let rows = units.len().div_ceil(2);
    let odd_last = units.len() > 2 && units.len() % 2 == 1;

    for (index, pair) in units.chunks(2).enumerate() {
        let last = index + 1 == rows;
        if odd_last && last {
            let galley = line_galley(ui, metrics, &pair[0], font.clone(), table, Align::Center);
            stacked(ui, metrics, &[galley], left, table, Align::Center);
        } else {
            let cells = pair
                .iter()
                .map(|unit| line_galley(ui, metrics, unit, font.clone(), column, Align::Center))
                .collect::<Vec<_>>();
            let height = cells
                .iter()
                .map(|galley| galley.size().y)
                .fold(0.0, f32::max);
            place(ui, metrics, height, |painter, rect| {
                for (column_index, galley) in cells.iter().enumerate() {
                    let center = rect.left() + left + column * (column_index as f32 + 0.5);
                    painter.galley(
                        egui::pos2(center, rect.top()),
                        galley.clone(),
                        Color32::BLACK,
                    );
                }
            });
        }
        // 公章需要空档：单位多于两个时，除最后一行外行间留 45mm。
        if units.len() > 2 && !last {
            ui.add_space(metrics.mm(JOINT_ROW_GAP_MM));
        }
    }
    ui.add_space(metrics.mm(JOINT_DATE_GAP_MM));
    // 日期压在主发文单位所在列下方，而不是整块居中；主单位跨列时整体居中。
    let (closing_left, closing_width) = match export::joint_main_column(input) {
        Some(col) => (left + column * col as f32, column),
        None => (left, table),
    };
    let date = line_galley(
        ui,
        metrics,
        &signature_date(input),
        font,
        closing_width,
        Align::Center,
    );
    stacked(
        ui,
        metrics,
        &[date],
        closing_left,
        closing_width,
        Align::Center,
    );
}

/// 版记（仅函稿）：四号三线表。第一行是抄送与共印份数，第二行起是承办单位、
/// 联系人、联系电话三列。成文时版记贴在版心底部，预览里紧跟落款。
fn footer_record(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput, display: &UnitDisplay) {
    if input.kind != TemplateKind::OfficialLetter {
        return;
    }
    ui.add_space(metrics.mm(RECORD_GAP_MM));
    let font = metrics.font(theme::FONT_FANGSONG, RECORD_PT);
    let line = metrics.pt(TABLE_LINE_PT);
    let joint = is_joint_mode_one(input);
    let external = input.uses_external_unit_names();

    // 共印份数 = 主送 + 抄送 + 承办单位数（类中的 autocalc）。
    let responsible_field = if joint {
        &input.profile.joint_responsible_units
    } else {
        &input.profile.responsible_unit
    };
    let copies = split_units(&input.profile.recipient).len()
        + split_units(&input.profile.copies_to).len()
        + split_units(responsible_field).len();
    let copies_to = display.join_hierarchical_for(&split_units(&input.profile.copies_to), external);
    let head = if copies_to.trim().is_empty() {
        String::new()
    } else {
        format!("抄送：{copies_to}")
    };

    // 承办单位/联系人/电话：联合发文有多行，其余只有一行。联合发文的承办单位
    // 与联系人成对录入（一一对应），旧稿件按索引回落配对。
    let rows: Vec<[String; 3]> = if joint {
        let entries = crate::models::joint_responsible_entries(&input.profile);
        let count = entries.len().max(1);
        (0..count)
            .map(|index| {
                let entry = entries.get(index);
                [
                    display.abbr(entry.map_or("", |value| value.unit.as_str())),
                    entry.map_or(String::new(), |value| value.name.clone()),
                    entry.map_or(String::new(), |value| value.phone.clone()),
                ]
            })
            .collect()
    } else {
        vec![[
            display.abbr(&input.profile.responsible_unit),
            input.profile.contact_person.trim().to_string(),
            input.profile.contact_phone.trim().to_string(),
        ]]
    };

    let phone_column = metrics.pt(RECORD_PT) * RECORD_PHONE_COLUMN_EM;
    let column = (metrics.content - phone_column) / 2.0;
    let columns = [column, column, phone_column];
    let thick = metrics.mm(0.6).max(1.0);
    let thin = metrics.mm(0.3).max(1.0);

    // 抄送行：三字符悬挂缩进，共印份数固定在行末。
    let head_galley = line_galley(
        ui,
        metrics,
        &head,
        font.clone(),
        metrics.content,
        Align::LEFT,
    );
    let copies_galley = line_galley(
        ui,
        metrics,
        &format!("（共印{copies}份）"),
        font.clone(),
        metrics.content,
        Align::Max,
    );
    let head_height = head_galley.size().y.max(line);

    // 三列：承办单位左、联系人中、联系电话右；第 2 行起用 5em/4em 占位与首行对齐。
    let cells: Vec<[Arc<egui::Galley>; 3]> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            // 第 2 行起用全角空格占位（一个全角空格正好 1em），让名称与首行标签后对齐。
            let pad = |ems: usize| {
                if index == 0 {
                    String::new()
                } else {
                    "\u{2003}".repeat(ems)
                }
            };
            let labels = ["承办单位：", "联系人：", "联系电话："];
            let aligns = [Align::LEFT, Align::Center, Align::Max];
            let pads = [5usize, 4, 0];
            std::array::from_fn(|index_column| {
                let label = if index == 0 { labels[index_column] } else { "" };
                let text = format!("{label}{}{}", pad(pads[index_column]), row[index_column]);
                line_galley(
                    ui,
                    metrics,
                    &text,
                    font.clone(),
                    columns[index_column],
                    aligns[index_column],
                )
            })
        })
        .collect();
    let row_heights = cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|galley| galley.size().y)
                .fold(line, f32::max)
        })
        .collect::<Vec<_>>();

    let total = thick + head_height + thin + row_heights.iter().sum::<f32>() + thick;
    place(ui, metrics, total, |painter, rect| {
        let rule = |y: f32, width: f32| {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), y),
                    egui::vec2(metrics.content, width),
                ),
                0.0,
                Color32::BLACK,
            );
        };
        let mut y = rect.top();
        rule(y, thick);
        y += thick;
        painter.galley(
            egui::pos2(rect.left(), y),
            head_galley.clone(),
            Color32::BLACK,
        );
        painter.galley(
            egui::pos2(rect.right(), y),
            copies_galley.clone(),
            Color32::BLACK,
        );
        y += head_height;
        rule(y, thin);
        y += thin;
        for (row, height) in cells.iter().zip(&row_heights) {
            for (index_column, galley) in row.iter().enumerate() {
                let x = match index_column {
                    0 => rect.left(),
                    1 => rect.left() + columns[0] + columns[1] / 2.0,
                    _ => rect.right(),
                };
                painter.galley(egui::pos2(x, y), galley.clone(), Color32::BLACK);
            }
            y += height;
        }
        rule(y, thick);
    });
}

/// 预览的一次绘制结果。
pub struct PreviewOutput {
    /// 本帧实际使用的缩放倍率，供“适应宽度”状态下的加减档以它为起点。
    pub scale: f32,
    /// 本帧被点击的正文块在 Markdown 源码中的字节范围。
    pub clicked: Option<Range<usize>>,
}

/// 画一个来自 Markdown 的块，并让它可以点：悬停时淡底提示，点击后把它在源码中的
/// 范围报给调用方；`anchor` 命中的块常亮，与编辑器里的高亮一一对应。
fn clickable(
    ui: &mut egui::Ui,
    range: &Range<usize>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    // 底色要压在文字下面：先占一个空图形位，量出块的范围后再回填。
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let inner = ui.scope(add_contents).response.rect;
    if !inner.is_positive() {
        return;
    }
    let rect = inner.expand2(egui::vec2(4.0, 1.0));
    let response = ui.interact(
        rect,
        egui::Id::new(("gw-preview-block", range.start, range.end)),
        egui::Sense::click(),
    );
    if response.clicked() {
        *clicked = Some(range.clone());
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // 查找命中通常只是块内的一小段文字；与块范围相交也要标亮，这样在“公文预览”
    // 模式中仍能看出当前命中位于哪一段。
    let anchored = anchor.is_some_and(|anchor| {
        !anchor.is_empty()
            && !range.is_empty()
            && anchor.start < range.end
            && range.start < anchor.end
    });
    if anchored && *scroll_to_anchor {
        response.scroll_to_me(Some(egui::Align::Center));
        *scroll_to_anchor = false;
    }
    let fill = if anchored {
        theme::accent_soft()
    } else if response.hovered() {
        HOVER_TINT
    } else {
        return;
    };
    ui.painter().set(
        backdrop,
        egui::epaint::RectShape::filled(rect, egui::CornerRadius::same(3), fill),
    );
}

/// 一张“纸”：白底、细边、内含公文版心。
fn sheet(ui: &mut egui::Ui, metrics: &Metrics, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        // 用量好的可见宽度而不是 available_width：滚动区里后者可能是无穷大。
        let side = ((metrics.viewport - metrics.page) / 2.0).max(0.0);
        ui.add_space(side);
        ui.vertical(|ui| {
            ui.set_max_width(metrics.page);
            egui::Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(3))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 2],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(18),
                })
                .show(ui, |ui| {
                    // 页边距用 add_space 铺出来：`Margin` 是 i8，放大后会溢出。
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    ui.set_width(metrics.page);
                    ui.add_space(metrics.margin_top);
                    ui.horizontal_top(|ui| {
                        ui.add_space(metrics.margin_left);
                        ui.vertical(|ui| {
                            ui.set_width(metrics.content);
                            ui.style_mut().visuals.override_text_color = Some(Color32::BLACK);
                            // 预览是拿来看版式和点回源码的，不做文字选择，
                            // 否则 Label 会把点击当成拖选吃掉。
                            ui.style_mut().interaction.selectable_labels = false;
                            add_contents(ui);
                        });
                    });
                    ui.add_space(metrics.margin_top);
                });
        });
    });
}

/// 把 Markdown 连同表单锁定的行文要素按公文版式画在 `ui` 里；调用方负责套滚动区。
/// 返回本次实际使用的缩放倍率，供“适应宽度”状态下的加减档以它为起点。
pub fn official_preview(
    ui: &mut egui::Ui,
    input: &DraftInput,
    display: &UnitDisplay,
    markdown: &str,
    zoom: Option<f32>,
    anchor: Option<&Range<usize>>,
    mut scroll_to_anchor: bool,
) -> PreviewOutput {
    // 自适应缩放要按“看得见的宽度”算：滚动方向上的 available_width 是无穷大，
    // 拿它算会把整页放大到上限。裁剪矩形就是滚动区的可视范围，再与窗口取交集兜底。
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|input| input.content_rect()));
    let metrics = Metrics::new(visible.width(), zoom);
    // 五个文种的正文都走 export::latex::official_letter_sections_to_tex，标题一律
    // 自动编号为 一、（一）1.（1）；紧缩风格跟随模板配置。
    let numbered = true;
    let compact = input.profile.style_mode == StyleMode::Compact;
    let located = export::parse_markdown_located(markdown);
    let blocks = located
        .iter()
        .map(|block| block.block.clone())
        .collect::<Vec<_>>();
    let names = export::attachment_names(&blocks);

    // 与导出一致地切分：正文一张纸，之后每份附件另起一张。
    let mut body: Vec<&LocatedBlock> = Vec::new();
    let mut attachments: Vec<Vec<&LocatedBlock>> = Vec::new();
    let mut in_attachment = false;
    let mut seen_title = false;
    for located in &located {
        match &located.block {
            MarkdownBlock::Title(_) if !seen_title => {
                seen_title = true;
                body.push(located);
            }
            MarkdownBlock::Title(_) => {
                in_attachment = true;
                attachments.push(vec![located]);
            }
            MarkdownBlock::Marker(section) => {
                in_attachment = matches!(section, MarkdownSection::Attachment);
            }
            _ if in_attachment => match attachments.last_mut() {
                Some(last) => last.push(located),
                None => attachments.push(vec![located]),
            },
            _ => body.push(located),
        }
    }

    // 紧缩风格合并的是正文区 # 号最多的那一级标题；附件区不参与。
    let compact_level = export::body_heading_max_level(&blocks);
    // 标题取正文首个 `# `，缺省回落表单里的标题提示，与导出器一致。
    let title = body
        .iter()
        .find_map(|located| match &located.block {
            MarkdownBlock::Title(text) => Some((export::plain_text(text), located.range.clone())),
            _ => None,
        })
        .unwrap_or_else(|| (export::plain_text(input.title_hint.trim()), 0..0));
    let mut clicked = None;
    // 版记排在全文最后：有附件时跟在最后一份附件后面，没有附件时跟在落款后面。
    let record_on_body = attachments.is_empty();

    let (title, title_range) = title;
    sheet(ui, &metrics, |ui| {
        header_block(ui, &metrics, input, display);
        if !title.is_empty() {
            clickable(
                ui,
                &title_range,
                anchor,
                &mut scroll_to_anchor,
                &mut clicked,
                |ui| {
                    line_block(
                        ui,
                        &metrics,
                        &title,
                        theme::FONT_BIAOSONG,
                        TITLE_PT,
                        Align::Center,
                    );
                },
            );
        }
        // 类里标题与主送（或正文）之间固定空一行。
        ui.add_space(metrics.line);
        addressee_block(ui, &metrics, input, display);

        let mut counters = [0usize; 4];
        let mut index = 0usize;
        while index < body.len() {
            let located = body[index];
            index += 1;
            match &located.block {
                // 文档标题已在上面按版式排过，正文区不再重复。
                MarkdownBlock::Title(_) => {}
                MarkdownBlock::Heading(level, heading)
                    if compact
                        && *level == compact_level
                        && matches!(body.get(index), Some(next)
                            if matches!(&next.block, MarkdownBlock::Paragraph(text)
                                if is_renderable_paragraph(text))) =>
                {
                    let Some(next) = body.get(index) else {
                        unreachable!("已在守卫里确认过后面还有块")
                    };
                    let MarkdownBlock::Paragraph(text) = &next.block else {
                        unreachable!("已在守卫里确认过是正文段落")
                    };
                    index += 1;
                    // 合并成一段的标题与正文，回跳时一并选中。
                    let range = located.range.start..next.range.end;
                    clickable(
                        ui,
                        &range,
                        anchor,
                        &mut scroll_to_anchor,
                        &mut clicked,
                        |ui| {
                            compact_block(
                                ui,
                                &metrics,
                                *level,
                                heading,
                                text,
                                &mut counters,
                                numbered,
                            );
                        },
                    );
                }
                _ => {
                    let range = located.range.clone();
                    clickable(
                        ui,
                        &range,
                        anchor,
                        &mut scroll_to_anchor,
                        &mut clicked,
                        |ui| {
                            content_block(ui, &metrics, &located.block, &mut counters, numbered);
                        },
                    );
                }
            }
        }
        // 正文之后的附件概要：空两行再逐条列出（与导出一致）。
        if !names.is_empty() {
            ui.add_space(metrics.line * 2.0);
            for (index, name) in names.iter().enumerate() {
                let label = if names.len() == 1 {
                    format!("附件：{name}")
                } else {
                    format!("附件{}：{name}", index + 1)
                };
                body_block(ui, &metrics, &label, true);
            }
        }
        signature_block(ui, &metrics, input, display);
        if record_on_body {
            footer_record(ui, &metrics, input, display);
        }
    });

    let last_attachment = attachments.len().saturating_sub(1);
    for (sheet_index, attachment) in attachments.into_iter().enumerate() {
        ui.add_space(14.0);
        sheet(ui, &metrics, |ui| {
            let mut counters = [0usize; 4];
            for located in attachment {
                let range = located.range.clone();
                clickable(
                    ui,
                    &range,
                    anchor,
                    &mut scroll_to_anchor,
                    &mut clicked,
                    |ui| {
                        match &located.block {
                            // 附件标签：黑体、左对齐、不缩进。
                            MarkdownBlock::Title(text) => line_block(
                                ui,
                                &metrics,
                                &export::plain_text(text),
                                theme::FONT_HEITI,
                                BODY_PT,
                                Align::LEFT,
                            ),
                            // 附件自身的标题：小标宋二号居中。
                            MarkdownBlock::Heading(2, text) => {
                                counters.fill(0);
                                line_block(
                                    ui,
                                    &metrics,
                                    &export::plain_text(text),
                                    theme::FONT_BIAOSONG,
                                    TITLE_PT,
                                    Align::Center,
                                );
                                ui.add_space(metrics.pt(18.0));
                            }
                            // 附件内部的层级整体上移一级。
                            MarkdownBlock::Heading(level, text) if *level > 2 => {
                                heading_block(
                                    ui,
                                    &metrics,
                                    level - 1,
                                    text,
                                    &mut counters,
                                    numbered,
                                );
                            }
                            block => content_block(ui, &metrics, block, &mut counters, numbered),
                        }
                    },
                );
            }
            if sheet_index == last_attachment {
                footer_record(ui, &metrics, input, display);
            }
        });
    }
    PreviewOutput {
        scale: metrics.scale,
        clicked,
    }
}

/// 紧缩风格的一段：标题（带编号与句号，用该级标题字体）后面直接接正文。
fn compact_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    level: u8,
    heading: &str,
    body: &str,
    counters: &mut [usize; 4],
    numbered: bool,
) {
    let heading = match numbered {
        true => match export::official_heading_text(level, heading, counters) {
            Some(text) => text,
            None => return,
        },
        false => heading.to_string(),
    };
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    let mut job = job(metrics.content);
    job.append(
        &indent(INDENT_CHARS),
        0.0,
        text_format(normal.clone(), metrics.line),
    );
    job.append(
        &format!("{}。", export::plain_text(&heading)),
        0.0,
        text_format(metrics.font(heading_family(level), BODY_PT), metrics.line),
    );
    append_inline(&mut job, metrics, body, &normal);
    draw(ui, job);
}

fn heading_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
    numbered: bool,
) {
    let text = if numbered {
        match export::official_heading_text(level, text, counters) {
            Some(text) => text,
            None => return,
        }
    } else {
        text.to_string()
    };
    let mut job = job(metrics.content);
    let font = metrics.font(heading_family(level), BODY_PT);
    job.append(
        &indent(INDENT_CHARS),
        0.0,
        text_format(font.clone(), metrics.line),
    );
    job.append(
        &export::plain_text(&text),
        0.0,
        text_format(font, metrics.line),
    );
    draw(ui, job);
}

fn content_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    block: &MarkdownBlock,
    counters: &mut [usize; 4],
    numbered: bool,
) {
    match block {
        MarkdownBlock::Heading(level, text) => {
            heading_block(ui, metrics, *level, text, counters, numbered);
        }
        MarkdownBlock::Paragraph(text) if is_renderable_paragraph(text) => {
            body_block(ui, metrics, text, true);
        }
        MarkdownBlock::ListItem(text) => {
            ui.horizontal(|ui| {
                ui.add_space(metrics.pt(LIST_INDENT_PT));
                ui.vertical(|ui| {
                    ui.set_width(metrics.content - metrics.pt(LIST_INDENT_PT));
                    let mut job = job(metrics.content - metrics.pt(LIST_INDENT_PT));
                    job.append(
                        text,
                        0.0,
                        text_format(metrics.font(theme::FONT_FANGSONG, BODY_PT), metrics.line),
                    );
                    draw(ui, job);
                });
            });
        }
        MarkdownBlock::Table(rows) => table_block(ui, metrics, rows),
        MarkdownBlock::Image { alt, src } => image_block(ui, metrics, alt, src),
        MarkdownBlock::Title(_) | MarkdownBlock::Marker(_) | MarkdownBlock::Html(_) => {}
        MarkdownBlock::Paragraph(_) => {}
    }
}

/// 图片块：位图按版心宽度等比渲染，加载失败显示占位卡片；PDF 显示占位卡片
/// （预览不渲染 PDF 内容，导出时由导出器嵌入）。外层已由 clickable 包装。
fn image_block(ui: &mut egui::Ui, metrics: &Metrics, alt: &str, src: &str) {
    let file_name = src.rsplit('/').next().unwrap_or(src).to_string();
    let path = match images::resolve(src) {
        Ok(path) => path,
        Err(error) => return image_placeholder(ui, metrics, alt, &file_name, &error.to_string()),
    };
    let is_pdf = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        return image_placeholder(
            ui,
            metrics,
            alt,
            &file_name,
            "PDF 附件：预览暂不渲染，导出时嵌入",
        );
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return image_placeholder(ui, metrics, alt, &file_name, &format!("无法读取：{error}"));
        }
    };
    // 每张图片一个固定 uri，egui 的 ImageCache 按 uri 缓存解码结果，避免每帧重读重解码。
    // src 形如 `images/xxx.png`，直接用其做 uri 后缀保证唯一。
    let uri = format!("bytes://{src}");
    // 用 fit_to_original_size 按纹理尺寸布局，再受 max_size 限制（宽=版心、高不设限）。
    // 不能用默认的 Fraction 适配：滚动区里 available_size.y 是无穷大，会把图片高度
    // 撑成无穷大，矩形超出可视区域而不绘制。
    ui.add(
        egui::Image::from_bytes(uri, bytes)
            .fit_to_original_size(1.0)
            .max_size(egui::vec2(metrics.content, f32::INFINITY)),
    );
}

/// 图片占位卡片：细边框 + 文件名与说明，宽度=版心。
fn image_placeholder(ui: &mut egui::Ui, metrics: &Metrics, alt: &str, file_name: &str, note: &str) {
    let height = (metrics.line * 2.0).max(metrics.pt(24.0));
    place(ui, metrics, height, |painter, rect| {
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(2),
            Stroke::new(1.0_f32.max(metrics.scale), Color32::from_gray(150)),
            egui::StrokeKind::Inside,
        );
        let font = metrics.font(theme::FONT_FANGSONG, BODY_PT);
        let caption = if alt.is_empty() {
            file_name.to_string()
        } else {
            format!("{file_name}（{alt}）")
        };
        let text = format!("【图片】{caption}\n{note}");
        let galley = painter.layout(
            text,
            font,
            Color32::from_gray(90),
            rect.width() - metrics.pt(8.0),
        );
        let y = rect.top() + ((rect.height() - galley.size().y) / 2.0).max(metrics.pt(4.0));
        painter.galley(
            egui::pos2(rect.left() + metrics.pt(4.0), y),
            galley,
            Color32::WHITE,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TemplateProfile, VocabularyEntry};

    /// 一份两级单位的词库：厅下面挂一个处，处有简称。
    fn vocabulary() -> Vec<VocabularyEntry> {
        vec![
            VocabularyEntry {
                code: "00".into(),
                canonical: "星海省教育厅".into(),
                abbr: "省教育厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "0001".into(),
                canonical: "教师工作处".into(),
                parent: "00".into(),
                abbr: "教师处".into(),
                ..Default::default()
            },
        ]
    }

    fn draft(kind: TemplateKind) -> DraftInput {
        DraftInput {
            kind,
            date: "2026年8月7日".into(),
            profile: TemplateProfile {
                kind,
                issuing_unit: "星海省教育厅".into(),
                department_code: "星教函".into(),
                document_number: "12".into(),
                security_level: "秘密".into(),
                security_period: "10年".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn security_line_carries_level_period_and_special_handling() {
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(security_text(&input).as_deref(), Some("秘密★10年"));

        input.profile.special_handling = true;
        assert_eq!(
            security_text(&input).as_deref(),
            Some("秘密★10年\u{2003}指人专办")
        );

        // 普通公文不标“指人专办”，与 export::latex::security_commands 一致。
        input.kind = TemplateKind::PlainDocument;
        assert_eq!(security_text(&input).as_deref(), Some("秘密★10年"));

        input.profile.security_level.clear();
        assert_eq!(security_text(&input), None);
    }

    #[test]
    fn document_number_blanks_the_serial_in_preview_version() {
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(document_number(&input), "星教函〔2026〕12 号");

        input.profile.letter_version = LetterVersion::Preview;
        assert_eq!(document_number(&input), "星教函〔2026〕\u{2003} 号");
    }

    #[test]
    fn signature_date_blanks_the_day_in_preview_version() {
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(signature_date(&input), "2026年8月7日");

        input.profile.letter_version = LetterVersion::Preview;
        assert_eq!(signature_date(&input), "2026年8月\u{2003}日");
    }

    #[test]
    fn signature_unit_follows_the_rule_of_each_template() {
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);

        // 公函落款用全称；落款单位留空时回落发文单位。
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(signature_unit(&input, &display), "星海省教育厅");
        input.profile.signing_unit = "教师工作处".into();
        assert_eq!(signature_unit(&input, &display), "星海省教育厅教师工作处");

        // 电话通知落款用简称，少于 5 字逐字加空格。
        let mut notice = draft(TemplateKind::PhoneNotice);
        notice.profile.signing_unit = "星海省教育厅".into();
        assert_eq!(signature_unit(&notice, &display), "省 教 育 厅");
    }

    #[test]
    fn signature_seal_mark_follows_the_daizhang_option() {
        let input = draft(TemplateKind::OfficialLetter);
        let mut vocabulary = vocabulary();
        assert_eq!(
            signature_seal_mark(&input, &UnitDisplay::new(&vocabulary)),
            None
        );
        vocabulary[0].seal_on_behalf = true;
        assert_eq!(
            signature_seal_mark(&input, &UnitDisplay::new(&vocabulary)),
            Some("（代章）")
        );
    }

    #[test]
    fn joint_issuance_puts_the_main_unit_on_the_red_header() {
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = draft(TemplateKind::OfficialLetter);
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "教师工作处，星海省教育厅".into();

        // 未指定主办单位时取第一个。
        assert_eq!(header_unit(&input, &display), "星海省教育厅教师工作处");

        input.profile.main_issuing_unit = "星海省教育厅".into();
        assert_eq!(header_unit(&input, &display), "星海省教育厅");

        // 单独发文时红头永远是发文单位，不看联合发文字段。
        input.profile.joint_issuance_mode = JointIssuanceMode::Single;
        assert_eq!(header_unit(&input, &display), "星海省教育厅");
    }

    /// 把一帧里画出的文字包络出来，供对齐断言用。
    fn text_bounds(output: &egui::FullOutput) -> egui::Rect {
        let mut bounds = egui::Rect::NOTHING;
        for clipped in &output.shapes {
            if let egui::epaint::Shape::Text(shape) = &clipped.shape {
                bounds = bounds.union(shape.visual_bounding_rect());
            }
        }
        bounds
    }

    #[test]
    fn centered_heading_is_centered_within_the_content_width() {
        // 单行与多行的小标宋标题都要以版心宽居中，单行不能贴左缘。
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        for title in ["重点工作通知", "关于做好二〇二六年重点工作有关事项的通知"]
        {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(available, 1200.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(raw, |ui| {
                line_block(
                    ui,
                    &metrics,
                    title,
                    theme::FONT_BIAOSONG,
                    TITLE_PT,
                    Align::Center,
                );
            });
            let bounds = text_bounds(&output);
            assert!(
                bounds.is_positive(),
                "标题“{title}”应有可见文字：{bounds:?}"
            );
            let center = bounds.min.x + bounds.width() / 2.0;
            let expected = metrics.content / 2.0;
            assert!(
                (center - expected).abs() <= 1.0,
                "标题“{title}”应相对版心居中：实际中心 {center:.1}，期望 {expected:.1}（{bounds:?}）"
            );
        }
    }

    #[test]
    fn official_font_families_do_not_contain_a_separate_latin_font() {
        // 英文、数字随对应的中文字体排：字体族里不再把 Times New Roman 放在最前。
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        // 字体集要跑过一帧才就绪。
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let definitions = ctx.fonts(|fonts| fonts.definitions().clone());
        for family in [
            theme::FONT_FANGSONG,
            theme::FONT_HEITI,
            theme::FONT_KAITI,
            theme::FONT_BIAOSONG,
        ] {
            let list = definitions
                .families
                .get(&egui::FontFamily::Name(family.into()))
                .expect("官方字体族应已注册");
            assert!(
                !list.iter().any(|key| key == "gw-latin"),
                "{family} 字体族不应含单独的西文字体 gw-latin：{list:?}"
            );
        }
    }

    #[test]
    fn image_block_falls_back_to_placeholder_without_panicking() {
        // 文件缺失、路径非法、PDF 三种情况都走占位卡片，且不 panic。
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        for (alt, src) in [
            ("缺失图", "images/20260809_120000_不存在的.png"),
            ("穿越", "../etc/passwd"),
            ("PDF 附件", "images/20260809_120000_扫描件.pdf"),
        ] {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(available, 1200.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(raw, |ui| {
                let mut counters = [0usize; 4];
                content_block(
                    ui,
                    &metrics,
                    &MarkdownBlock::Image {
                        alt: alt.to_string(),
                        src: src.to_string(),
                    },
                    &mut counters,
                    false,
                );
            });
        }
    }

    #[test]
    fn image_block_loads_texture_after_background_decode() {
        // content_block 的 Image 块：后台线程解码完成后，纹理应可加载（Ready）。
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_diag_{}.png", std::process::id());
        let img_path = dir.join(&name);
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            100,
            50,
            image::Rgba([255, 0, 0, 255]),
        ))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
        std::fs::write(&img_path, cursor.into_inner()).unwrap();
        let src = format!("images/{name}");
        let metrics = Metrics::new(1000.0, Some(1.0));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 1200.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw.clone(), |ui| {
            let mut counters = [0usize; 4];
            content_block(
                ui,
                &metrics,
                &MarkdownBlock::Image {
                    alt: "图".into(),
                    src: src.clone(),
                },
                &mut counters,
                false,
            );
        });
        // 后台线程解码需要时间与后续帧推进；等解码完成后重查。
        std::thread::sleep(std::time::Duration::from_millis(500));
        let uri = format!("bytes://{src}");
        let mut result = ctx.try_load_texture(
            &uri,
            egui::TextureOptions::default(),
            egui::load::SizeHint::default(),
        );
        if matches!(result, Ok(egui::load::TexturePoll::Pending { .. })) {
            let _ = ctx.run_ui(raw, |_| {});
            result = ctx.try_load_texture(
                &uri,
                egui::TextureOptions::default(),
                egui::load::SizeHint::default(),
            );
        }
        let _ = std::fs::remove_file(&img_path);
        match result {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                assert!(texture.size.x > 0.0 && texture.size.y > 0.0);
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => {
                panic!("图片纹理应在多帧后加载完成，但仍 Pending");
            }
            Err(error) => {
                panic!("图片纹理加载失败：{error:?}");
            }
        }
    }

    #[test]
    fn official_preview_loads_image_texture() {
        // 完整走 official_preview：构造含图片的公文，多帧渲染后纹理应可加载。
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_diag2_{}.png", std::process::id());
        let img_path = dir.join(&name);
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            200,
            100,
            image::Rgba([0, 0, 255, 255]),
        ))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
        std::fs::write(&img_path, cursor.into_inner()).unwrap();
        let src = format!("images/{name}");
        let markdown = format!("# 关于测试的函\n\n正文。\n\n![示意图]({src})\n\n特此函告。");
        let input = draft(TemplateKind::OfficialLetter);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 2000.0),
            )),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run_ui(raw.clone(), |ui| {
                let _ = official_preview(
                    ui,
                    &input,
                    &UnitDisplay::new(&[]),
                    &markdown,
                    None,
                    None,
                    false,
                );
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let uri = format!("bytes://{src}");
        let result = ctx.try_load_texture(
            &uri,
            egui::TextureOptions::default(),
            egui::load::SizeHint::default(),
        );
        let _ = std::fs::remove_file(&img_path);
        match result {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                assert!(texture.size.x > 0.0 && texture.size.y > 0.0);
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => {
                panic!("official_preview 中图片纹理应加载完成，但仍 Pending");
            }
            Err(error) => {
                panic!("official_preview 中图片纹理加载失败：{error:?}");
            }
        }
    }

    #[test]
    fn official_preview_draws_image_with_finite_size() {
        // 回归测试：含图片的公文在滚动区内渲染时，图片必须绘制为尺寸有限的
        // 带纹理矩形（旧实现默认 ImageFit::Fraction 在滚动区把高算成无穷大，
        // tessellate 后画面损坏，正是用户看不到图片的原因）。
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_diag3_{}.png", std::process::id());
        let img_path = dir.join(&name);
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            200,
            100,
            image::Rgba([0, 255, 0, 255]),
        ))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
        std::fs::write(&img_path, cursor.into_inner()).unwrap();
        let src = format!("images/{name}");
        let markdown = format!("# 关于测试的函\n\n正文。\n\n![示意图]({src})\n\n特此函告。");
        let input = draft(TemplateKind::OfficialLetter);
        let uri = format!("bytes://{src}");
        let mut states = Vec::new();
        for frame in 0..4 {
            let full = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 2000.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    // 与真实 markdown_render 一致：official_preview 在滚动区内渲染，
                    // 滚动方向的 available_size 是无穷大，正是旧实现的崩溃点。
                    egui::ScrollArea::both().show(ui, |ui| {
                        let _ = official_preview(
                            ui,
                            &input,
                            &UnitDisplay::new(&[]),
                            &markdown,
                            None,
                            None,
                            false,
                        );
                    });
                },
            );
            let state = ctx
                .try_load_texture(
                    &uri,
                    egui::TextureOptions::default(),
                    egui::load::SizeHint::default(),
                )
                .map(|poll| match poll {
                    egui::load::TexturePoll::Ready { .. } => "ready".to_string(),
                    egui::load::TexturePoll::Pending { .. } => "pending".to_string(),
                })
                .unwrap_or_else(|e| format!("err({e:?})"));
            let textured_rects = full
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Rect(rect)
                            if rect.fill_texture_id() != egui::TextureId::default()
                    )
                })
                .count();
            // 图片矩形必须尺寸有限（旧实现把高算成无穷大，tessellate 后画面损坏）。
            let finite_tex_rects = full
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Rect(rect)
                            if rect.fill_texture_id() != egui::TextureId::default()
                                && rect.rect.width().is_finite()
                                && rect.rect.height().is_finite()
                                && rect.rect.height() > 0.0
                                && rect.rect.height() < 10000.0
                    )
                })
                .count();
            states.push(format!(
                "frame{frame}:{state}:tex_rects={textured_rects}:finite_tex_rects={finite_tex_rects}"
            ));
            if frame == 0 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }
        let _ = std::fs::remove_file(&img_path);
        // 文件存在期间（循环内），图片应真正绘制：出现尺寸有限的带纹理矩形。
        // （旧实现把图片高度算成无穷大，tessellate 后画面损坏，正是用户看不到图的原因。）
        assert!(
            states
                .iter()
                .any(|s| s.contains(":finite_tex_rects=") && !s.ends_with("finite_tex_rects=0")),
            "图片应绘制尺寸有限的带纹理矩形：{states:?}"
        );
    }
}
