use super::{
    ColumnAlign, MarkdownBlock, MarkdownSection, attachment_names, body_heading_max_level,
    chinese_date_parts, inline_segments, official_heading_text, parse_markdown, plain_text,
    table::{ColumnAlignment, to_docx_grid},
    title::{self, TitlePlan},
};
use crate::images;
use crate::models::{
    DraftInput, JointIssuanceMode, LetterVersion, StyleMode, TemplateKind, split_period_digits,
    split_units,
};
use crate::units::UnitDisplay;
use anyhow::{Context, Result};
use docx_rs::*;
use image::GenericImageView;
use regex::Regex;
use std::{fs::File, path::Path};

const BODY_SIZE: usize = 32; // 16 pt，OOXML 使用半磅
const TITLE_SIZE: usize = 44; // 22 pt，二号
const RED_APPROVAL_TITLE_SIZE: usize = 36; // 18 pt，小二号
pub(super) const TABLE_SIZE: usize = 28; // 14 pt，四号
const FOOTER_SIZE: usize = 28; // 14 pt，四号；版记字号独立固定，不随正文表格调整
const PAGE_NUMBER_SIZE: usize = 36; // 18 pt，四号
/// 正文中完整括号（全角/半角）及其中内容的字号：14 pt，四号，比正文小一号。
const PAREN_SIZE: usize = 28;
pub(super) const TABLE_CONTENT_WIDTH_TWIPS: usize = 8_844; // 156 mm 版心
/// 版记一个 em 的宽度（twips）：四号 14 pt，1 pt = 20 twips。
const FOOTER_EM_TWIPS: usize = FOOTER_SIZE * 10;
/// 联系电话列宽：5 个全角字标签 + 11 位半角数字（5.5em）+ 0.5em 余量。
const RECORD_PHONE_COLUMN_TWIPS: usize = 11 * FOOTER_EM_TWIPS;
/// 承办单位、联系人两列均分剩余宽度。
const RECORD_OTHER_COLUMN_TWIPS: usize =
    (TABLE_CONTENT_WIDTH_TWIPS - RECORD_PHONE_COLUMN_TWIPS) / 2;
const AGENDA_NUMBERING_ID: usize = 17;
const JOINT_SIGNATURE_SEAL_GAP_TWIPS: f32 = 2_551.0; // 45 mm 公章安全高度

/// 红头（发文机关标志）可用的版心宽度：A4 页宽减去左右页边距，约 15.6cm。
const HEADER_WIDTH_TWIPS: i32 = 11906 - 1587 - 1474;
/// 红头字号：二号（24 pt）。
const HEADER_SIZE: usize = 48;
/// 字数过多时允许缩到的最小字号：三号（16 pt），再小就不合公文规范。
const HEADER_MIN_SIZE: usize = 32;

/// 红头的排布方式。
#[derive(Debug, PartialEq, Eq)]
struct HeaderLayout {
    /// 字号，半磅为单位。
    size: usize,
    /// 左右缩进，把拉开后的整块摆到版心中间。
    side_indent: i32,
}

/// 发文机关标志分两种情形排布：
/// 1. 版心排得下——字号不变，用分散对齐把字距均匀撑开，字形不会变形；
///    字数很少时把字距限制在一个字宽以内，免得几个字散得满页都是。
/// 2. 版心排不下——缩小字号让它回到一行，而不是把字压扁。
fn issuing_unit_header(unit: &str) -> HeaderLayout {
    // 汉字是等宽的：一个字的宽度 = 字号（半磅）× 10 twip。
    let char_width = |size: usize| size as i32 * 10;
    let count = unit
        .chars()
        .filter(|value| !value.is_whitespace())
        .count()
        .max(1) as i32;
    let natural = count * char_width(HEADER_SIZE);

    if natural >= HEADER_WIDTH_TWIPS {
        let fitted = (HEADER_WIDTH_TWIPS / count / 10) as usize;
        return HeaderLayout {
            size: fitted.clamp(HEADER_MIN_SIZE, HEADER_SIZE),
            side_indent: 0,
        };
    }

    // 字距拉到一个字宽时的总宽度，作为散开的上限。
    let widest = natural + (count - 1) * char_width(HEADER_SIZE);
    let block = widest.min(HEADER_WIDTH_TWIPS);
    HeaderLayout {
        size: HEADER_SIZE,
        side_indent: (HEADER_WIDTH_TWIPS - block) / 2,
    }
}

fn chinese_fonts(name: &str) -> RunFonts {
    RunFonts::new()
        .ascii("Times New Roman")
        .hi_ansi("Times New Roman")
        .east_asia(name)
}

fn body_run(text: impl Into<String>) -> Run {
    Run::new()
        .add_text(text)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(BODY_SIZE)
}

/// 正文 run 序列：Markdown 加粗保留为同字体自动加粗；完整圆括号/方头括号及其中内容
/// 用楷体_GB2312 四号，其余用仿宋三号。
/// 标题（文档标题、各级标题、附件标签）不经由此处，不受此规则影响。
fn body_runs(text: &str) -> Vec<Run> {
    let segments = inline_segments(text);
    if segments.is_empty() {
        return vec![body_run("")];
    }
    segments
        .into_iter()
        .map(|segment| {
            let mut run = if segment.parenthesized {
                Run::new()
                    .add_text(segment.text)
                    .fonts(chinese_fonts("楷体_GB2312"))
                    .size(PAREN_SIZE)
            } else {
                body_run(segment.text)
            };
            if segment.bold {
                // 不切换为单独粗体字体，只设置加粗属性，由当前字体自动加粗。
                run = run.bold();
            }
            run
        })
        .collect()
}

fn heiti_run(text: impl Into<String>) -> Run {
    Run::new()
        .add_text(text)
        .fonts(chinese_fonts("黑体"))
        .size(BODY_SIZE)
        .bold()
}

/// 密级行 run 序列：数字年限的保密期限，前导数字用等宽西文字体（对应 LaTeX 的
/// `\ttfamily`），其余用行内基准字体；指人专办以黑体加粗追加在末尾。
fn security_runs(level: &str, period: &str, special: &str, base: &str, bold: bool) -> Vec<Run> {
    let base_run = |text: &str| {
        let mut run = Run::new()
            .add_text(text.to_string())
            .fonts(chinese_fonts(base))
            .size(BODY_SIZE);
        if bold {
            run = run.bold();
        }
        run
    };
    let (digits, rest) = split_period_digits(period);
    let mut runs = vec![base_run(&format!("{level}★"))];
    if !digits.is_empty() {
        let mut run = Run::new()
            .add_text(digits)
            .fonts(
                RunFonts::new()
                    .ascii("Courier New")
                    .hi_ansi("Courier New")
                    .east_asia(base),
            )
            .size(BODY_SIZE);
        if bold {
            run = run.bold();
        }
        runs.push(run);
    }
    if !rest.is_empty() {
        runs.push(base_run(rest));
    }
    if !special.is_empty() {
        runs.push(heiti_run(special));
    }
    runs
}

fn page_number_footer(alignment: AlignmentType) -> Footer {
    let page_number = Run::new()
        .add_text("— ")
        .add_field_char(FieldCharType::Begin, false)
        .add_instr_text(InstrText::PAGE(InstrPAGE::new()))
        .add_field_char(FieldCharType::Separate, false)
        .add_text("1")
        .add_field_char(FieldCharType::End, false)
        .add_text(" —")
        .fonts(chinese_fonts("SimSun"))
        .size(PAGE_NUMBER_SIZE);
    Footer::new().add_paragraph(
        Paragraph::new()
            .add_run(page_number)
            .align(alignment)
            .line_spacing(
                LineSpacing::new()
                    .line(360)
                    .line_rule(LineSpacingType::Exact),
            ),
    )
}

fn add_official_page_footers(doc: Docx, duplex: bool) -> Docx {
    let doc = if duplex {
        doc.footer(page_number_footer(AlignmentType::Right))
            .even_footer(page_number_footer(AlignmentType::Left))
    } else {
        doc.footer(page_number_footer(AlignmentType::Center))
    };
    // 与函件 LaTeX 类保持一致：首页不显示页码。
    doc.first_footer(Footer::new())
}

fn body_paragraph(text: &str) -> Paragraph {
    let mut paragraph = Paragraph::new()
        .align(AlignmentType::Both)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .widow_control(true);
    for run in body_runs(text) {
        paragraph = paragraph.add_run(run);
    }
    paragraph
}

fn label_paragraph(text: &str) -> Paragraph {
    let mut paragraph = Paragraph::new().line_spacing(
        LineSpacing::new()
            .line(560)
            .line_rule(LineSpacingType::Exact),
    );
    for run in body_runs(text) {
        paragraph = paragraph.add_run(run);
    }
    paragraph
}

/// 函稿/电话通知顶格的密级行：密级 + ★ + 保密期限。勾选“指人专办”时，
/// 在“密级★保密期限”后空一个全角空格，再以黑体标注“指人专办”四个字。
/// 数字年限的保密期限数字部分用等宽西文字体（`security_runs`）。
fn letter_security_paragraph(input: &DraftInput) -> Paragraph {
    let level = input.profile.security_level.trim();
    let period = input.profile.security_period.trim();
    let special = if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        "　指人专办"
    } else {
        ""
    };
    let mut paragraph = Paragraph::new().line_spacing(
        LineSpacing::new()
            .line(560)
            .line_rule(LineSpacingType::Exact),
    );
    for run in security_runs(level, period, special, "仿宋_GB2312", false) {
        paragraph = paragraph.add_run(run);
    }
    paragraph
}

fn heading_paragraph(level: u8, text: &str) -> Paragraph {
    let font = match level {
        2 => "黑体",
        3 => "楷体_GB2312",
        _ => "仿宋_GB2312",
    };
    let mut run = Run::new()
        .add_text(plain_text(text))
        .fonts(chinese_fonts(font))
        .size(BODY_SIZE);
    if matches!(level, 2 | 5) {
        run = run.bold();
    }
    Paragraph::new()
        .add_run(run)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

/// 紧缩风格（规格 §4.2）：正文区 # 号最多的那一级标题与紧随其后的正文合并为一行，
/// 标题部分用该级标题字体并带编号与句号，正文部分用仿宋正文字体。
fn compact_heading_paragraph(level: u8, title: &str, body: &str) -> Paragraph {
    // 与 `heading_paragraph` 的层级字体保持一致：2 级黑体、3 级楷体、其余仿宋（5 级加粗）。
    let font = match level {
        2 => "黑体",
        3 => "楷体_GB2312",
        _ => "仿宋_GB2312",
    };
    let mut run = Run::new()
        .add_text(plain_text(&format!("{title}。")))
        .fonts(chinese_fonts(font))
        .size(BODY_SIZE);
    if matches!(level, 2 | 5) {
        run = run.bold();
    }
    let mut paragraph = Paragraph::new()
        .add_run(run)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .widow_control(true);
    for body_run in body_runs(body) {
        paragraph = paragraph.add_run(body_run);
    }
    paragraph
}

fn attachment_label_paragraph(text: &str) -> Paragraph {
    let fonts = RunFonts::new()
        .ascii("SimHei")
        .hi_ansi("SimHei")
        .east_asia("黑体");
    Paragraph::new()
        .add_run(
            Run::new()
                .add_text(plain_text(text))
                .fonts(fonts)
                .size(BODY_SIZE)
                .bold(),
        )
        .align(AlignmentType::Left)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

/// 规格 §3.3：预览版所有占位区域统一 1em 宽，用一个全角空格表示。
const PREVIEW_PLACEHOLDER: &str = "\u{2003}";

fn official_document_number(input: &DraftInput) -> Option<String> {
    let code = input.profile.department_code.trim();
    let serial = input.profile.document_number.trim();
    if input.profile.letter_version == LetterVersion::Preview {
        let year = input.document_year();
        return Some(format!("{code}〔{year}〕{PREVIEW_PLACEHOLDER}号"));
    }
    if serial.is_empty() {
        return None;
    }
    if code.is_empty() {
        Some(serial.to_string())
    } else {
        let year = input.document_year();
        Some(format!("{code}〔{year}〕{serial}号"))
    }
}

fn official_signature_date(input: &DraftInput) -> String {
    if input.profile.letter_version == LetterVersion::Preview
        && let Some((year, month, _)) = chinese_date_parts(&input.date)
    {
        return format!("{year}年{month}月{PREVIEW_PLACEHOLDER}日");
    }
    input.date.trim().to_string()
}

/// 附件概要：正文结束后、落款之前，与正文之间空两行、首行缩进两个汉字，
/// 按顺序列出附件名称。单个附件写“附件：名称”；多个附件只有第一行写“附件N：名称”，
/// 其余行的“附件”二字用两个全角空格占位对齐、不再重复。
fn add_attachment_summary(mut doc: Docx, names: &[String]) -> Docx {
    // 与正文之间空两行。
    for _ in 0..2 {
        doc = doc.add_paragraph(
            Paragraph::new().line_spacing(
                LineSpacing::new()
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            ),
        );
    }
    for (index, name) in names.iter().enumerate() {
        // 多个附件只有第一行保留“附件”二字，其余行用两个全角空格占位，使序号列对齐。
        let label = if names.len() == 1 {
            format!("附件：{name}")
        } else if index == 0 {
            format!("附件{}：{name}", index + 1)
        } else {
            format!("　　{}：{name}", index + 1)
        };
        let mut paragraph = Paragraph::new()
            .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
            .line_spacing(
                LineSpacing::new()
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true);
        for run in body_runs(&label) {
            paragraph = paragraph.add_run(run);
        }
        doc = doc.add_paragraph(paragraph);
    }
    doc
}

fn is_joint_mode_one(input: &DraftInput) -> bool {
    input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1
}

fn main_issuing_unit(input: &DraftInput, display: &UnitDisplay) -> String {
    if !is_joint_mode_one(input) {
        return display.full_name_for(
            &input.profile.issuing_unit,
            input.uses_external_unit_names(),
        );
    }
    let selected = split_units(&input.profile.joint_issuing_units);
    let main = input.profile.main_issuing_unit.trim();
    let chosen = if selected.iter().any(|unit| unit == main) {
        main.to_string()
    } else {
        selected.first().cloned().unwrap_or_default()
    };
    display.full_name_for(&chosen, input.uses_external_unit_names())
}

/// 白头件与红头呈批件的落款：多单位自上而下分行、行间空一行（便于分别签字），
/// 整体右对齐；显示文本少于 5 字时按字间距分散对齐到 5 字宽。
///
/// `signing_room_twips` 是落款右侧留给签字的空间：白头件传 0（版式已在别处
/// 留位），红头呈批件传 4cm——单位名整体左移让出签字位，成文日期则落在
/// 「落款单位 + 签字空间」这一整段的正中。
fn add_white_paper_signature(
    doc: Docx,
    input: &DraftInput,
    display: &UnitDisplay,
    signing_room_twips: usize,
) -> Docx {
    let units = display
        .white_paper_signature_units(input)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect::<Vec<_>>();
    if units.is_empty() {
        return doc;
    }
    let mut doc = doc;
    for (index, unit) in units.iter().enumerate() {
        if index > 0 {
            doc = doc.add_paragraph(
                Paragraph::new().add_run(body_run("")).line_spacing(
                    LineSpacing::new()
                        .line(560)
                        .line_rule(LineSpacingType::Exact),
                ),
            );
        }
        let mut paragraph = Paragraph::new()
            .align(AlignmentType::Right)
            // 右缩进就是签字空间：单位名右对齐到“版心右缘减去签字位”。
            .indent(Some(0), None, Some(signing_room_twips as i32), None)
            .line_spacing(
                LineSpacing::new()
                    .before(if index == 0 { 360 } else { 0 })
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            );
        for run in spread_runs(unit) {
            paragraph = paragraph.add_run(run);
        }
        doc = doc.add_paragraph(paragraph);
    }
    // 最后一个单位与成文日期之间固定空一行（单位只有一个时同样空行），
    // 与 LaTeX 的 \vspace{\baselineskip} 和预览的空行保持一致。
    doc = doc.add_paragraph(
        Paragraph::new().add_run(body_run("")).line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        ),
    );
    let date = Paragraph::new()
        .add_run(body_run(official_signature_date(input)))
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        );
    let date = if signing_room_twips == 0 {
        date.align(AlignmentType::Right)
    } else {
        // 左缩进到最宽那行单位的左沿，再在剩下的“单位 + 签字空间”里居中，
        // 段落右缘就是版心右缘，居中位置正好是这一整段的中点。
        let unit_width = crate::export::red_signature_unit_width_twips(&units);
        let left = TABLE_CONTENT_WIDTH_TWIPS.saturating_sub(signing_room_twips + unit_width);
        date.align(AlignmentType::Center)
            .indent(Some(left as i32), None, Some(0), None)
    };
    doc.add_paragraph(date)
}

/// 落款单位 run 序列：少于 5 字时逐字设置字符间距（单位缇，1/20 磅）分散对齐到
/// 5 字宽——16pt 字号下 1em=320 缇，总宽恰好为 5 个字；否则整串一个 run。
fn spread_runs(text: &str) -> Vec<Run> {
    match crate::units::spread_gap(text) {
        Some(gap) => {
            let spacing = (gap * 320.0).round() as i32;
            let chars = text.chars().collect::<Vec<_>>();
            chars
                .iter()
                .enumerate()
                .map(|(index, ch)| {
                    let mut run = body_run(ch.to_string());
                    // 最后一个字后面没有字符，不再设间距，避免总宽超出 5 字。
                    if index + 1 < chars.len() {
                        run = run.character_spacing(spacing);
                    }
                    run
                })
                .collect()
        }
        None => vec![body_run(text)],
    }
}

fn add_joint_signature(doc: Docx, input: &DraftInput, display: &UnitDisplay) -> Docx {
    let units = split_units(&input.profile.joint_issuing_units);
    if units.is_empty() {
        return doc;
    }
    let chunks = units.chunks(2).collect::<Vec<_>>();
    let row_count = chunks.len();
    // 规格 §2.5：超过两个单位且为奇数时，最后一个单位跨两列居中。
    let odd_last = units.len() > 2 && units.len() % 2 == 1;
    // 代章直接跟在主发文单位后面，不另起一行。
    let main_index = crate::export::joint_seal_index(input, display);
    let mut rows: Vec<TableRow> = chunks
        .into_iter()
        .enumerate()
        .map(|(row_index, chunk)| {
            let base_index = row_index * 2;
            let mut row = if odd_last && row_index + 1 == row_count && chunk.len() == 1 {
                TableRow::new(vec![
                    TableCell::new()
                        .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
                        .grid_span(2)
                        .add_paragraph(joint_signature_cell_paragraph(
                            &joint_unit_name(input, display, &chunk[0], base_index, main_index),
                            row_index,
                        )),
                ])
                .cant_split()
            } else {
                let cells = (0..2)
                    .map(|column| {
                        let value = chunk.get(column).map_or("", String::as_str);
                        TableCell::new().width(4_422, WidthType::Dxa).add_paragraph(
                            joint_signature_cell_paragraph(
                                &joint_unit_name(
                                    input,
                                    display,
                                    value,
                                    base_index + column,
                                    main_index,
                                ),
                                row_index,
                            ),
                        )
                    })
                    .collect();
                TableRow::new(cells).cant_split()
            };
            if units.len() > 2 && row_index + 1 < row_count {
                row = row
                    .row_height(JOINT_SIGNATURE_SEAL_GAP_TWIPS)
                    .height_rule(HeightRule::AtLeast);
            }
            row
        })
        .collect();
    // 日期压在主发文单位所在列下方，而不是整块居中；主单位跨列时整行居中。
    rows.push(joint_closing_row(input));
    doc.add_table(
        Table::without_borders(rows)
            .set_grid(vec![4_422, 4_422])
            .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
            .align(TableAlignmentType::Center)
            .layout(TableLayoutType::Fixed),
    )
}

/// 联合发文落款单元格的单位名；代章直接跟在主发文单位后面（如“乙单位（代章）”）。
fn joint_unit_name(
    input: &DraftInput,
    display: &UnitDisplay,
    value: &str,
    unit_index: usize,
    main_index: Option<usize>,
) -> String {
    let mut name = display.full_name_for(value, input.uses_external_unit_names());
    if Some(unit_index) == main_index {
        name.push_str("（代章）");
    }
    name
}

/// 联合发文落款的收尾行：把成文日期放进主发文单位所在列的单元格，
/// 让日期压在主单位下方而不是整块居中；主单位跨两列时整行居中。
fn joint_closing_row(input: &DraftInput) -> TableRow {
    let date = official_signature_date(input);
    let main_cell = TableCell::new()
        .width(4_422, WidthType::Dxa)
        .add_paragraph(joint_closing_paragraph(&date, 360));
    match crate::export::joint_main_column(input) {
        Some(col) => {
            let mut cells: Vec<TableCell> = (0..2)
                .map(|_| {
                    TableCell::new()
                        .width(4_422, WidthType::Dxa)
                        .add_paragraph(joint_closing_paragraph("", 0))
                })
                .collect();
            cells[col] = main_cell;
            TableRow::new(cells).cant_split()
        }
        None => TableRow::new(vec![
            main_cell
                .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
                .grid_span(2),
        ])
        .cant_split(),
    }
}

fn joint_closing_paragraph(text: &str, before: u32) -> Paragraph {
    Paragraph::new()
        .add_run(body_run(text))
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .before(before)
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
}

fn joint_signature_cell_paragraph(value: &str, row_index: usize) -> Paragraph {
    Paragraph::new()
        .add_run(body_run(value))
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .before(if row_index == 0 { 360 } else { 0 })
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
}

fn record_run(text: &str) -> Run {
    Run::new()
        .add_text(text)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(FOOTER_SIZE)
}

fn automatic_print_copies(input: &DraftInput) -> usize {
    let responsible = if is_joint_mode_one(input) {
        &input.profile.joint_responsible_units
    } else {
        &input.profile.responsible_unit
    };
    split_units(&input.profile.recipient).len()
        + split_units(&input.profile.copies_to).len()
        + split_units(responsible).len()
}

/// 规格 §3.2：版记是一个带横线的表格——上、下横线粗（1pt），行间横线细（0.5pt），
/// 上下正好是中间的两倍宽。首行抄送跨三列并右对齐印数，其后每行是“承办单位/联系人/联系电话”。
fn add_footer_record(doc: Docx, input: &DraftInput, display: &UnitDisplay) -> Docx {
    let grid = vec![
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_PHONE_COLUMN_TWIPS,
    ];
    let mut rows = Vec::new();

    // 首行：抄送 + 右对齐印数，跨三列，其上边框即版记上横线。
    // 没有抄送单位时整行只留印数，不写空的“抄送：”标签（与 gonghan-gwa.cls 的 \FooterCopiesLine 一致）。
    let copies_text = display.join_hierarchical_for(
        &split_units(&input.profile.copies_to),
        input.uses_external_unit_names(),
    );
    let mut copies_paragraph = Paragraph::new().add_tab(
        Tab::new()
            .val(TabValueType::Right)
            .pos(TABLE_CONTENT_WIDTH_TWIPS),
    );
    if !copies_text.is_empty() {
        // “抄送：”占 3 字宽，回行时正文与首行的单位名称对齐。
        copies_paragraph = copies_paragraph
            .indent(Some(840), Some(SpecialIndentType::Hanging(840)), None, None)
            .add_run(record_run(&format!("抄送：{copies_text}")));
    }
    let copies_paragraph = copies_paragraph
        .add_run(Run::new().add_tab())
        .add_run(record_run(&format!(
            "（共印{}份）",
            automatic_print_copies(input)
        )))
        .line_spacing(
            LineSpacing::new()
                .line(420)
                .line_rule(LineSpacingType::Exact),
        );
    rows.push(TableRow::new(vec![
        TableCell::new()
            .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
            .grid_span(3)
            .add_paragraph(copies_paragraph),
    ]));

    let mut record_rows = if is_joint_mode_one(input) {
        joint_record_rows(input, display)
    } else if input.profile.responsible_unit.trim().is_empty()
        && input.profile.contact_person.trim().is_empty()
        && input.profile.contact_phone.trim().is_empty()
    {
        Vec::new()
    } else {
        vec![single_record_row(input, display)]
    };
    rows.append(&mut record_rows);

    let borders = TableBorders::new()
        .set(TableBorder::new(TableBorderPosition::Top).size(8))
        .set(TableBorder::new(TableBorderPosition::Bottom).size(8))
        .set(TableBorder::new(TableBorderPosition::InsideH).size(4));
    doc.add_table(
        Table::new(rows)
            .set_grid(grid)
            .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
            .margins(TableCellMargins::new().margin(0, 0, 0, 0))
            .layout(TableLayoutType::Fixed)
            .set_borders(borders),
    )
}

/// 版记单元格。`size` 按半磅计；`left_indent` 用于第 2 行起让名称与第一行标签后的位置对齐。
fn record_cell_sized(
    text: &str,
    width: usize,
    alignment: AlignmentType,
    size: usize,
    left_indent: i32,
) -> TableCell {
    let mut paragraph = Paragraph::new()
        .add_run(
            Run::new()
                .add_text(text)
                .fonts(chinese_fonts("仿宋_GB2312"))
                .size(size),
        )
        .align(alignment)
        .line_spacing(
            LineSpacing::new()
                .line(420)
                .line_rule(LineSpacingType::Exact),
        );
    if left_indent > 0 {
        paragraph = paragraph.indent(Some(left_indent), None, None, None);
    }
    TableCell::new()
        .width(width, WidthType::Dxa)
        .add_paragraph(paragraph)
}

fn record_cell(text: &str, width: usize, alignment: AlignmentType) -> TableCell {
    record_cell_sized(text, width, alignment, FOOTER_SIZE, 0)
}

/// 规格 §3.2/§6 姓名宽度：2 字姓名中间加全角空格占 3 字宽，4 字姓名用更小字号近似压缩到 3 字宽。
fn docx_name(value: &str, base_size: usize) -> (String, usize) {
    let chars = value.chars().collect::<Vec<_>>();
    match chars.len() {
        2 => (format!("{}\u{2003}{}", chars[0], chars[1]), base_size),
        4 => (value.to_string(), (base_size as f32 * 0.75) as usize),
        _ => (value.to_string(), base_size),
    }
}

/// 单独发文的版记行：承办单位（简称）/ 联系人 / 联系电话。
fn single_record_row(input: &DraftInput, display: &UnitDisplay) -> TableRow {
    let grid = [
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_PHONE_COLUMN_TWIPS,
    ];
    let responsible = display.abbr(&input.profile.responsible_unit);
    let (contact_text, contact_size) = docx_name(&input.profile.contact_person, FOOTER_SIZE);
    TableRow::new(vec![
        record_cell(
            &format!("承办单位：{responsible}"),
            grid[0],
            AlignmentType::Left,
        ),
        record_cell_sized(
            &format!("联系人：{contact_text}"),
            grid[1],
            AlignmentType::Center,
            contact_size,
            0,
        ),
        record_cell(
            &format!("联系电话：{}", input.profile.contact_phone.trim()),
            grid[2],
            AlignmentType::Right,
        ),
    ])
    .cant_split()
}

/// 联合发文的版记行：承办单位、联系人、联系电话一一对应；第 2 行起用左缩进对齐名称。
fn joint_record_rows(input: &DraftInput, display: &UnitDisplay) -> Vec<TableRow> {
    let grid = [
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_PHONE_COLUMN_TWIPS,
    ];
    let entries = crate::models::joint_responsible_entries(&input.profile);
    let row_count = entries.len().max(1);
    (0..row_count)
        .map(|index| {
            let entry = entries.get(index);
            let responsible_name = display.abbr(entry.map_or("", |value| value.unit.as_str()));
            let (contact_text, contact_size) =
                docx_name(entry.map_or("", |value| value.name.as_str()), FOOTER_SIZE);
            // “承办单位：”5 字 ≈ 1400 twips，“联系人：”4 字 ≈ 1120 twips。
            let unit_left = if index == 0 { 0 } else { 1400 };
            let contact_left = if index == 0 { 0 } else { 1120 };
            TableRow::new(vec![
                record_cell_sized(
                    &format!(
                        "{}{}",
                        if index == 0 { "承办单位：" } else { "" },
                        responsible_name
                    ),
                    grid[0],
                    AlignmentType::Left,
                    FOOTER_SIZE,
                    unit_left,
                ),
                record_cell_sized(
                    &format!(
                        "{}{}",
                        if index == 0 { "联系人：" } else { "" },
                        contact_text
                    ),
                    grid[1],
                    AlignmentType::Center,
                    contact_size,
                    contact_left,
                ),
                record_cell_sized(
                    &format!(
                        "{}{}",
                        if index == 0 { "联系电话：" } else { "" },
                        entry.map_or("", |value| value.phone.as_str())
                    ),
                    grid[2],
                    AlignmentType::Right,
                    FOOTER_SIZE,
                    0,
                ),
            ])
            // 行高交给 Word 自动撑开：承办单位没维护简称时会回落较长的规范名称，
            // 固定行高会把折行后的第二行直接裁掉。
            .cant_split()
        })
        .collect()
}

fn attachment_document_title_paragraph(text: &str) -> Paragraph {
    Paragraph::new()
        .add_run(
            Run::new()
                .add_text(plain_text(text))
                .fonts(chinese_fonts("方正小标宋简体"))
                .size(TITLE_SIZE),
        )
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .after(360)
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

/// 公文主标题 run：小标宋，字号按排布方案给出（半磅）。
fn title_run(text: &str, size: usize) -> Run {
    Run::new()
        .add_text(plain_text(text))
        .fonts(chinese_fonts("方正小标宋简体"))
        .size(size)
}

/// 公文主标题段，按排布方案渲染：
/// 单行保持二号；超出 1-2 字横向收窄字形（字高不变）保持单行；超出更多用 jieba
/// 换行（词不拆开、行长短均衡），各行仍用二号。调用方自行叠加行距与 keep_next。
fn document_title_paragraph(title: &str, plan: &TitlePlan) -> Paragraph {
    let mut paragraph = Paragraph::new().align(AlignmentType::Center);
    match plan {
        TitlePlan::SingleLine => {
            paragraph = paragraph.add_run(title_run(title, TITLE_SIZE));
        }
        TitlePlan::Compressed => {
            // 只横向缩放（w:w 字符缩放），字号保持二号、字高不变。
            let scale = title::compressed_scale_percent(title);
            paragraph = paragraph.add_run(title_run(title, TITLE_SIZE).stretch(scale as i32));
        }
        TitlePlan::Wrapped(lines) => {
            for (index, line) in lines.iter().enumerate() {
                paragraph = paragraph.add_run(title_run(line, TITLE_SIZE));
                if index + 1 < lines.len() {
                    paragraph = paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
                }
            }
        }
    }
    paragraph
}

fn red_approval_title_paragraph(title: &str, plan: &TitlePlan) -> Paragraph {
    let mut paragraph = Paragraph::new().align(AlignmentType::Center);
    let run = |text: &str| title_run(text, RED_APPROVAL_TITLE_SIZE);
    match plan {
        TitlePlan::SingleLine => paragraph = paragraph.add_run(run(title)),
        TitlePlan::Compressed => {
            let scale = title::compressed_scale_percent_for(
                title,
                title::RED_APPROVAL_TITLE_WIDTH_PT,
                title::RED_APPROVAL_TITLE_SIZE_PT,
            );
            paragraph = paragraph.add_run(run(title).stretch(scale as i32));
        }
        TitlePlan::Wrapped(lines) => {
            for (index, line) in lines.iter().enumerate() {
                paragraph = paragraph.add_run(run(line));
                if index + 1 < lines.len() {
                    paragraph = paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
                }
            }
        }
    }
    // 首页左栏约 10cm，右侧 5.6cm 留给批示栏。
    paragraph.indent(Some(0), None, Some(3_175), None)
}

fn red_approval_frame_table(input: &DraftInput) -> Table {
    let record_rows = crate::models::joint_responsible_entries(&input.profile)
        .len()
        .max(1);
    // 页面下边距为 35mm，承办表贴下边距上沿；每增加一行（正文字号固定
    // 28pt 行距），框底就随承办表上移，保持竖线与下横线相接。
    let frame_height = 10_028usize.saturating_sub(record_rows * 560).max(4_480) as f32;
    let borders = TableBorders::new()
        .set(
            TableBorder::new(TableBorderPosition::Top)
                .size(12)
                .color("FF0000"),
        )
        .set(
            TableBorder::new(TableBorderPosition::Left)
                .size(12)
                .color("FF0000"),
        );
    let paragraph = Paragraph::new()
        .add_run(
            Run::new()
                .add_text("批　示")
                .fonts(chinese_fonts("仿宋_GB2312"))
                .size(BODY_SIZE)
                .color("FF0000"),
        )
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .before(420)
                .line(560)
                .line_rule(LineSpacingType::Exact),
        );
    Table::new(vec![
        TableRow::new(vec![
            TableCell::new()
                .width(3_175, WidthType::Dxa)
                .add_paragraph(paragraph),
        ])
        .row_height(frame_height)
        .height_rule(HeightRule::AtLeast),
    ])
    .set_grid(vec![3_175])
    .width(3_175, WidthType::Dxa)
    .layout(TableLayoutType::Fixed)
    .clear_all_border()
    .set_borders(borders)
    .position(
        TablePositionProperty::new()
            .horizontal_anchor("margin")
            .vertical_anchor("margin")
            .position_x_alignment("right")
            .position_y(2_720)
            .left_from_text(0)
            .right_from_text(0),
    )
}

/// 首页红色横线要贯穿整个版心。批示栏自身只占右侧 5.6cm，另放一条
/// 极薄的浮动表格补足左栏横线，避免让 Word 把正文按整页宽度绕排。
fn red_approval_top_rule_table() -> Table {
    let borders = TableBorders::new().set(
        TableBorder::new(TableBorderPosition::Top)
            .size(12)
            .color("FF0000"),
    );
    Table::new(vec![
        TableRow::new(vec![
            TableCell::new()
                .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
                .add_paragraph(Paragraph::new()),
        ])
        .row_height(1.0)
        .height_rule(HeightRule::Exact),
    ])
    .set_grid(vec![TABLE_CONTENT_WIDTH_TWIPS])
    .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
    .layout(TableLayoutType::Fixed)
    .clear_all_border()
    .set_borders(borders)
    .position(
        TablePositionProperty::new()
            .horizontal_anchor("margin")
            .vertical_anchor("margin")
            .position_x_alignment("left")
            .position_y(2_720)
            .left_from_text(0)
            .right_from_text(0),
    )
}

/// 承办区一格：红色标签 + 黑色取值，整格不许换行。
///
/// 标签与取值的自然宽度超出栏宽时，两个 run 一起按同一比例横向压窄字形
/// （`w:w`，字高不变），与 LaTeX 侧的 `\RedFit` 和标题压缩同一套做法。
fn red_record_paragraph(
    label: &str,
    value: &str,
    alignment: AlignmentType,
    usable_twips: usize,
) -> Paragraph {
    let scale = crate::export::red_record_scale_percent(&format!("{label}{value}"), usable_twips);
    let mut label_run = Run::new()
        .add_text(label)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(BODY_SIZE)
        .color("FF0000");
    let mut value_run = body_run(value);
    if scale < 100 {
        label_run = label_run.stretch(scale as i32);
        value_run = value_run.stretch(scale as i32);
    }
    Paragraph::new()
        .add_run(label_run)
        .add_run(value_run)
        .align(alignment)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
}

fn red_approval_record_table(input: &DraftInput, display: &UnitDisplay) -> Table {
    let entries = crate::models::joint_responsible_entries(&input.profile);
    let fallback = crate::models::JointResponsibleEntry::default();
    let entries = if entries.is_empty() {
        std::slice::from_ref(&fallback)
    } else {
        entries.as_slice()
    };
    // 三栏不再等分：承办单位名最长，把版心余量都给它，联系人和电话按各自
    // 内容的自然宽度定死（栏宽与 LaTeX/预览同源，见 export::RED_RECORD_*）。
    let unit_width = crate::export::RED_RECORD_UNIT_TWIPS;
    let contact_width = crate::export::RED_RECORD_CONTACT_TWIPS;
    let phone_width = crate::export::RED_RECORD_PHONE_TWIPS;
    let rows = entries
        .iter()
        .map(|entry| {
            TableRow::new(vec![
                TableCell::new()
                    .width(unit_width, WidthType::Dxa)
                    .add_paragraph(red_record_paragraph(
                        "承办单位：",
                        &display.abbr(&entry.unit),
                        AlignmentType::Left,
                        crate::export::RED_RECORD_UNIT_USABLE_TWIPS,
                    )),
                TableCell::new()
                    .width(contact_width, WidthType::Dxa)
                    .add_paragraph(red_record_paragraph(
                        "联系人：",
                        &docx_name(&entry.name, BODY_SIZE).0,
                        AlignmentType::Left,
                        crate::export::RED_RECORD_CONTACT_USABLE_TWIPS,
                    )),
                TableCell::new()
                    .width(phone_width, WidthType::Dxa)
                    .add_paragraph(red_record_paragraph(
                        "电话：",
                        &entry.phone,
                        AlignmentType::Right,
                        crate::export::RED_RECORD_PHONE_USABLE_TWIPS,
                    )),
            ])
            .row_height(560.0)
            .height_rule(HeightRule::Exact)
            .cant_split()
        })
        .collect::<Vec<_>>();
    let borders = TableBorders::new().set(
        TableBorder::new(TableBorderPosition::Top)
            .size(12)
            .color("FF0000"),
    );
    Table::new(rows)
        .set_grid(vec![unit_width, contact_width, phone_width])
        .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .clear_all_border()
        .set_borders(borders)
        .position(
            TablePositionProperty::new()
                .horizontal_anchor("margin")
                .vertical_anchor("margin")
                .position_x_alignment("left")
                .position_y_alignment("bottom")
                .left_from_text(0)
                .right_from_text(0),
        )
}

fn table_run_sized(text: &str, header: bool, size: usize) -> Run {
    let mut run = Run::new()
        .add_text(plain_text(text))
        .fonts(chinese_fonts(if header {
            "黑体"
        } else {
            "仿宋_GB2312"
        }))
        .size(size);
    if header {
        run = run.bold();
    }
    run
}

fn table_runs_sized(text: &str, header: bool, size: usize) -> Vec<Run> {
    if header {
        return vec![table_run_sized(text, true, size)];
    }
    let segments = inline_segments(text);
    if segments.is_empty() {
        return vec![table_run_sized("", false, size)];
    }
    segments
        .into_iter()
        .map(|segment| {
            let mut run = table_run_sized(&segment.text, false, size);
            if segment.bold {
                run = run.bold();
            }
            run
        })
        .collect()
}

fn add_smart_table(mut doc: Docx, rows: &[Vec<String>], aligns: &[ColumnAlign]) -> Docx {
    if rows.is_empty() {
        return doc;
    }
    let (grid, alignments) = to_docx_grid(rows, aligns, TABLE_CONTENT_WIDTH_TWIPS, TABLE_SIZE * 10);
    if grid.is_empty() {
        return doc;
    }
    // 规格 §6：表头含“姓名/联系人”的列，非表头单元格按版记的方式处理姓名宽度。
    let name_column = rows.first().and_then(|header| {
        header
            .iter()
            .position(|cell| cell.contains("姓名") || cell.contains("联系人"))
    });

    let table_rows = rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            let cells = grid
                .iter()
                .enumerate()
                .map(|(column_index, width)| {
                    let text = row.get(column_index).map_or("", String::as_str);
                    // 表头一律居中；正文单元格按列对齐（分隔行写了冒号就以它为准）。
                    let alignment = if row_index == 0 {
                        AlignmentType::Center
                    } else {
                        match alignments.get(column_index) {
                            Some(ColumnAlignment::Center) => AlignmentType::Center,
                            Some(ColumnAlignment::Right) => AlignmentType::Right,
                            _ => AlignmentType::Left,
                        }
                    };
                    let runs = if name_column == Some(column_index) && row_index > 0 {
                        let segments = inline_segments(text);
                        let cleaned = segments
                            .iter()
                            .map(|segment| segment.text.as_str())
                            .collect::<String>();
                        let bold = segments.iter().any(|segment| segment.bold);
                        let (name_text, size) = docx_name(&cleaned, TABLE_SIZE);
                        let mut run = table_run_sized(&name_text, false, size);
                        if bold {
                            run = run.bold();
                        }
                        vec![run]
                    } else {
                        table_runs_sized(text, row_index == 0, TABLE_SIZE)
                    };
                    let mut paragraph = Paragraph::new();
                    for run in runs {
                        paragraph = paragraph.add_run(run);
                    }
                    let paragraph = paragraph.align(alignment).line_spacing(
                        LineSpacing::new()
                            .line(420)
                            .line_rule(LineSpacingType::Exact),
                    );
                    TableCell::new()
                        .width(*width, WidthType::Dxa)
                        .add_paragraph(paragraph)
                })
                .collect();
            TableRow::new(cells)
        })
        .collect::<Vec<_>>();

    let borders = TableBorders::new()
        .set(TableBorder::new(TableBorderPosition::Top).size(4))
        .set(TableBorder::new(TableBorderPosition::Left).size(4))
        .set(TableBorder::new(TableBorderPosition::Bottom).size(4))
        .set(TableBorder::new(TableBorderPosition::Right).size(4))
        .set(TableBorder::new(TableBorderPosition::InsideH).size(4))
        .set(TableBorder::new(TableBorderPosition::InsideV).size(4));
    let table = Table::new(table_rows)
        .set_grid(grid)
        .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .set_borders(borders);
    doc = doc.add_table(table);
    doc
}

fn add_official_content_block(
    mut doc: Docx,
    block: &MarkdownBlock,
    counters: &mut [usize; 4],
) -> Docx {
    match block {
        MarkdownBlock::Heading(level, text) => {
            if let Some(title) = official_heading_text(*level, text, counters) {
                doc = doc.add_paragraph(heading_paragraph(*level, &title));
            }
        }
        MarkdownBlock::Paragraph(text)
            if !text.trim().is_empty() && !text.contains("<div") && !text.contains("</div") =>
        {
            doc = doc.add_paragraph(body_paragraph(text));
        }
        MarkdownBlock::ListItem(text) => {
            doc = doc.add_paragraph(label_paragraph(text).indent(Some(420), None, None, None));
        }
        MarkdownBlock::Table { rows, aligns } => doc = add_smart_table(doc, rows, aligns),
        MarkdownBlock::Image { alt, src } => {
            if let Some(paragraph) = image_paragraph(alt, src) {
                doc = doc.add_paragraph(paragraph);
            }
        }
        _ => {}
    }
    doc
}

/// 图片段落：位图嵌入为 Word 图片（宽度=版心 156 mm，等比缩放，小图保持原尺寸）；
/// PDF 输出附件说明段落——docx-rs 无法把 PDF 作为图片嵌入。
fn image_paragraph(alt: &str, src: &str) -> Option<Paragraph> {
    let path = images::resolve(src).ok()?;
    let bytes = std::fs::read(&path).ok()?;
    let file_name = src.rsplit('/').next().unwrap_or(src).to_string();
    let is_pdf = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    image_paragraph_from_bytes(alt, &file_name, is_pdf, &bytes)
}

fn image_paragraph_from_bytes(
    alt: &str,
    file_name: &str,
    is_pdf: bool,
    bytes: &[u8],
) -> Option<Paragraph> {
    if is_pdf {
        let label = if alt.trim().is_empty() {
            format!("【附件】{file_name}（PDF 附件，请见原文）")
        } else {
            format!("【附件】{file_name}（{alt}，PDF 附件，请见原文）")
        };
        return Some(Paragraph::new().add_run(body_run(label)));
    }
    // 先完整解码一次：既取像素尺寸算缩放，也验证文件可被 docx-rs 的 Pic::new 转 PNG，
    // 避免 Pic::new 内部的 expect 在坏文件上 panic。
    let Ok(image) = image::load_from_memory(bytes) else {
        return None;
    };
    let (width_px, height_px) = image.dimensions();
    if width_px == 0 || height_px == 0 {
        return None;
    }
    // 版心 8844 twips = 156 mm；1 twip = 635 EMU，96 dpi 下 1 px = 9525 EMU。
    let max_width_emu = (TABLE_CONTENT_WIDTH_TWIPS as u64) * 635;
    let natural_width_emu = (width_px as u64) * 9525;
    let natural_height_emu = (height_px as u64) * 9525;
    let width_emu = natural_width_emu.min(max_width_emu);
    let height_emu = (natural_height_emu * width_emu / natural_width_emu) as u32;
    let pic = Pic::new(bytes).size(width_emu as u32, height_emu);
    Some(Paragraph::new().add_run(Run::new().add_image(pic)))
}

fn agenda_body_paragraph() -> Paragraph {
    Paragraph::new()
        .align(AlignmentType::Both)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .widow_control(true)
}

fn agenda_blank_line() -> Paragraph {
    Paragraph::new()
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

fn agenda_labeled_paragraph(line: &str) -> Paragraph {
    let (label, value) = line
        .split_once('：')
        .or_else(|| line.split_once(':'))
        .unwrap_or((line, ""));
    let mut paragraph = agenda_body_paragraph().add_run(heiti_run(format!("{label}：")));
    if !value.trim().is_empty() {
        for run in body_runs(value.trim()) {
            paragraph = paragraph.add_run(run);
        }
    }
    paragraph
}

fn agenda_numbering() -> AbstractNumbering {
    AbstractNumbering::new(AGENDA_NUMBERING_ID).add_level(
        Level::new(
            0,
            Start::new(1),
            NumberFormat::new("decimal"),
            LevelText::new("%1."),
            LevelJc::new("left"),
        )
        .suffix(LevelSuffixType::Space)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(BODY_SIZE),
    )
}

fn write_meeting_agenda_docx(path: &Path, input: &DraftInput, markdown: &str) -> Result<()> {
    let title = markdown
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(input.title_hint.as_str());
    let security_level = if input.profile.security_level.trim().is_empty() {
        "【待核实：密级】"
    } else {
        input.profile.security_level.trim()
    };
    let security_period = if input.profile.security_period.trim().is_empty() {
        "【待核实：保密期限】"
    } else {
        input.profile.security_period.trim()
    };
    let item_pattern = Regex::new(r"^\d+[.、．)]\s*(.+)$").expect("valid regex");

    let mut doc = Docx::new()
        .page_size(11906, 16838)
        .page_margin(PageMargin {
            top: 2098,
            bottom: 1984,
            left: 1587,
            right: 1474,
            header: 567,
            footer: 567,
            gutter: 0,
        })
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(BODY_SIZE)
        .default_line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .add_abstract_numbering(agenda_numbering())
        .add_numbering(Numbering::new(AGENDA_NUMBERING_ID, AGENDA_NUMBERING_ID));

    // 指人专办：勾选后在“密级★保密期限”后空一个全角空格，再以黑体标注“指人专办”。
    // 会议议程密级行整体黑体加粗；数字年限的保密期限数字部分用等宽西文字体。
    let special = if input.profile.special_handling {
        "　指人专办"
    } else {
        ""
    };
    let mut security = Paragraph::new();
    for run in security_runs(security_level, security_period, special, "黑体", true) {
        security = security.add_run(run);
    }
    doc = doc.add_paragraph(
        security
            .align(AlignmentType::Left)
            .line_spacing(
                LineSpacing::new()
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true),
    );
    doc = doc.add_paragraph(agenda_blank_line());
    let plan = title::title_plan(&plain_text(title), title::chars_per_line());
    doc = doc.add_paragraph(
        document_title_paragraph(title, &plan)
            .line_spacing(
                LineSpacing::new()
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true),
    );
    doc = doc.add_paragraph(agenda_blank_line());

    for line in markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("# ") {
            continue;
        }
        if line.starts_with("一、时间地点")
            || line.starts_with("二、参加人员")
            || line.starts_with("三、研讨内容")
        {
            doc = doc.add_paragraph(agenda_labeled_paragraph(line));
        } else if let Some(captures) = item_pattern.captures(line) {
            let content = captures.get(1).map(|value| value.as_str()).unwrap_or(line);
            let mut item = agenda_body_paragraph();
            for run in body_runs(content) {
                item = item.add_run(run);
            }
            doc = doc.add_paragraph(
                item.numbering(NumberingId::new(AGENDA_NUMBERING_ID), IndentLevel::new(0)),
            );
        } else {
            let mut item = agenda_body_paragraph();
            for run in body_runs(line) {
                item = item.add_run(run);
            }
            doc = doc.add_paragraph(item);
        }
    }

    let file =
        File::create(path).with_context(|| format!("无法创建 Word 文件：{}", path.display()))?;
    doc.build().pack(file).context("写入会议议程 DOCX 包失败")?;
    Ok(())
}

pub fn write_docx(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
) -> Result<()> {
    if input.kind == TemplateKind::MeetingAgenda {
        return write_meeting_agenda_docx(path, input, markdown);
    }

    let blocks = parse_markdown(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());

    let mut doc = Docx::new()
        .page_size(11906, 16838)
        .page_margin(PageMargin {
            top: 2098,
            bottom: 1984,
            left: 1587,
            right: 1474,
            header: 567,
            footer: 567,
            gutter: 0,
        })
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(BODY_SIZE)
        .default_line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        );

    if input.kind.uses_letter_layout() {
        doc = add_official_page_footers(doc, input.profile.duplex_printing);
        if input.kind == TemplateKind::RedHeadApproval {
            // 即使不标密也保留密级行的垂直槽位，使红头稳定落在参考稿约 67mm
            // 的位置；有密级时就在该槽位显示，不改变后续元素坐标。
            doc = doc.add_paragraph(
                letter_security_paragraph(input).line_spacing(
                    LineSpacing::new()
                        .line(560)
                        .line_rule(LineSpacingType::Exact)
                        .after(560),
                ),
            );
        } else if !input.profile.security_level.trim().is_empty() {
            doc = doc.add_paragraph(letter_security_paragraph(input));
        }
        if input.kind != TemplateKind::PlainDocument {
            let unit = main_issuing_unit(input, display);
            if !unit.is_empty() {
                let layout = issuing_unit_header(&unit);
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .add_run(
                            Run::new()
                                .add_text(&unit)
                                .fonts(chinese_fonts("方正小标宋简体"))
                                .size(layout.size)
                                .color("C00000"),
                        )
                        // 分散对齐让 Word 自己把字距均匀撑开；缩进决定撑开的范围。
                        .align(AlignmentType::Distribute)
                        .indent(
                            Some(layout.side_indent),
                            None,
                            Some(layout.side_indent),
                            None,
                        )
                        .line_spacing(LineSpacing::new().after(180)),
                );
            }
        }
        if input.kind.has_document_number()
            && let Some(number) = official_document_number(input)
        {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run(number))
                    .align(AlignmentType::Center)
                    .line_spacing(LineSpacing::new().after(240)),
            );
        }
        if input.kind == TemplateKind::RedHeadApproval {
            doc = doc
                .add_table(red_approval_top_rule_table())
                .add_table(red_approval_frame_table(input))
                .add_table(red_approval_record_table(input, display));
        }
    }

    let title_plain = plain_text(title);
    let title_capacity = if input.kind == TemplateKind::RedHeadApproval {
        title::red_approval_chars_per_line()
    } else {
        title::chars_per_line()
    };
    let plan = title::title_plan(&title_plain, title_capacity);
    let title_paragraph = if input.kind == TemplateKind::RedHeadApproval {
        red_approval_title_paragraph(title, &plan)
    } else {
        document_title_paragraph(title, &plan)
    };
    let title_before = if input.kind == TemplateKind::RedHeadApproval {
        480
    } else {
        120
    };
    // 红头呈批件首页版面：正文可用行数、正文是否跨页、表格图片是否要被赶出首页，
    // 三端共用 export::red_approval_* 的同一套估算，避免各端判据不一致。
    let red_wrap_lines = crate::export::red_approval_wrap_lines(
        match &plan {
            TitlePlan::Wrapped(lines) => lines.len().max(1),
            _ => 1,
        },
        crate::models::joint_responsible_entries(&input.profile)
            .len()
            .max(1),
    );
    let red_body = crate::export::red_approval_body_metrics(&blocks);
    let mut red_float_break_pending = input.kind == TemplateKind::RedHeadApproval
        && red_body.float_needs_page_break(red_wrap_lines);
    doc = doc.add_paragraph(
        title_paragraph
            .line_spacing(
                LineSpacing::new()
                    .before(title_before)
                    .after(360)
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true),
    );

    let addressee = match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => display.join_hierarchical_for(
            &split_units(&input.profile.recipient),
            input.uses_external_unit_names(),
        ),
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            display.reporting_leaders(&input.profile.reporting_leaders)
        }
        TemplateKind::PlainDocument | TemplateKind::MeetingAgenda => String::new(),
    };
    if !addressee.is_empty() && !markdown.contains(addressee.as_str()) {
        doc = doc.add_paragraph(label_paragraph(&format!(
            "{}：",
            addressee.trim_end_matches('：')
        )));
    }

    let mut attachment_blocks = Vec::new();
    if input.kind.uses_letter_layout() {
        let mut in_attachment = false;
        let mut seen_document_title = false;
        let mut counters = [0usize; 4];
        let compact = input.profile.style_mode == StyleMode::Compact;
        // 紧缩风格合并正文区 # 号最多的那一级标题；附件区标题不计入。
        let compact_heading_level = body_heading_max_level(&blocks);
        let mut index = 0usize;
        while index < blocks.len() {
            let block = &blocks[index];
            match block {
                MarkdownBlock::Title(_) if !seen_document_title && !in_attachment => {
                    seen_document_title = true;
                }
                MarkdownBlock::Marker(section) => {
                    in_attachment = matches!(section, MarkdownSection::Attachment);
                    counters = [0; 4];
                    if in_attachment {
                        attachment_blocks.push(block);
                    }
                }
                _ if in_attachment => attachment_blocks.push(block),
                _ => {
                    // 紧缩风格：正文区 # 号最多的那一级标题后紧跟正文段落时合并为一行。
                    let next_is_paragraph = blocks.get(index + 1).is_some_and(|next| {
                        matches!(next, MarkdownBlock::Paragraph(text)
                            if !text.trim().is_empty()
                                && !text.contains("<div")
                                && !text.contains("</div"))
                    });
                    if compact
                        && let MarkdownBlock::Heading(level, heading) = block
                        && *level == compact_heading_level
                        && next_is_paragraph
                    {
                        if let Some(title) = official_heading_text(*level, heading, &mut counters) {
                            let MarkdownBlock::Paragraph(body) = &blocks[index + 1] else {
                                unreachable!()
                            };
                            doc =
                                doc.add_paragraph(compact_heading_paragraph(*level, &title, body));
                        }
                        index += 1; // 跳过紧随的正文段落
                    } else {
                        // 红头呈批件首页右侧是批示栏，表格和图片按整幅版心排版会压
                        // 过去；正文区第一个表格/图片若落在首页就先换页。
                        if red_float_break_pending
                            && matches!(
                                block,
                                MarkdownBlock::Table { .. } | MarkdownBlock::Image { .. }
                            )
                        {
                            red_float_break_pending = false;
                            doc = doc.add_paragraph(
                                Paragraph::new().page_break_before(true).line_spacing(
                                    LineSpacing::new()
                                        .line(560)
                                        .line_rule(LineSpacingType::Exact),
                                ),
                            );
                        }
                        doc = add_official_content_block(doc, block, &mut counters);
                    }
                }
            }
            index += 1;
        }
    } else {
        for block in &blocks {
            match block {
                MarkdownBlock::Title(_) | MarkdownBlock::Html(_) | MarkdownBlock::Marker(_) => {}
                MarkdownBlock::Image { alt, src } => {
                    if let Some(paragraph) = image_paragraph(alt, src) {
                        doc = doc.add_paragraph(paragraph);
                    }
                }
                MarkdownBlock::Heading(level, text) => {
                    doc = doc.add_paragraph(heading_paragraph(*level, text));
                }
                MarkdownBlock::Paragraph(text) => {
                    if text.trim().is_empty() || text.contains("<div") || text.contains("</div") {
                        continue;
                    }
                    doc = doc.add_paragraph(body_paragraph(text));
                }
                MarkdownBlock::ListItem(text) => {
                    doc = doc.add_paragraph(label_paragraph(text).indent(
                        Some(420),
                        None,
                        None,
                        None,
                    ));
                }
                MarkdownBlock::Table { rows, aligns } => doc = add_smart_table(doc, rows, aligns),
            }
        }
    }

    // 附件概要：正文结束后、落款之前列出附件名称（红头呈批件同样支持）。
    if input.kind.uses_letter_layout() {
        let names = attachment_names(&blocks);
        if !names.is_empty() {
            doc = add_attachment_summary(doc, &names);
        }
    }

    if crate::models::is_joint_signature(input) {
        doc = add_joint_signature(doc, input, display);
    } else if input.kind == TemplateKind::WhitePaper {
        doc = add_white_paper_signature(doc, input, display, 0);
    } else if input.kind == TemplateKind::RedHeadApproval {
        // 落款最早从第二页开始。正文只有首页那点内容时另起一页标「（此页无正文）」；
        // 正文本来就跨页时不再额外制造空白页，落款接在正文之后。
        if !red_body.reaches_second_page(red_wrap_lines) {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run("（此页无正文）"))
                    .indent(Some(640), None, None, None)
                    .page_break_before(true)
                    .line_spacing(
                        LineSpacing::new()
                            .line(560)
                            .line_rule(LineSpacingType::Exact),
                    ),
            );
        }
        doc = add_white_paper_signature(doc, input, display, crate::export::SIGNATURE_ROOM_TWIPS);
    } else if matches!(
        input.kind,
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice
    ) {
        let raw_signature = if input.profile.signing_unit.trim().is_empty() {
            if is_joint_mode_one(input) {
                // 联合发文模式 1 只剩 1 个发文单位：回落右侧落款，单位取该唯一发文单位。
                split_units(&input.profile.joint_issuing_units)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| input.profile.issuing_unit.trim().to_string())
            } else {
                input.profile.issuing_unit.trim().to_string()
            }
        } else {
            input.profile.signing_unit.trim().to_string()
        };
        // 规格 §3.1：公函落款显示全称；电话通知落款显示简称（少于 5 字逐字加空格）。
        let signature = if input.kind == TemplateKind::PhoneNotice {
            display.abbr_spaced(&raw_signature)
        } else {
            display.full_name_for(&raw_signature, input.uses_external_unit_names())
        };
        if !signature.is_empty() {
            // 代章直接跟在落款单位后面同一行（如“星海省教育厅（代章）”），不另起一行。
            // 是否标注只由 seals_on_behalf 决定（仅公函；电话通知等其他文种不盖章）。
            let unit = if crate::export::seals_on_behalf(input, display) {
                format!("{signature}（代章）")
            } else {
                signature
            };
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run(unit))
                    .align(AlignmentType::Right)
                    .line_spacing(
                        LineSpacing::new()
                            .before(360)
                            .line(560)
                            .line_rule(LineSpacingType::Exact),
                    ),
            );
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run(official_signature_date(input)))
                    .align(AlignmentType::Right)
                    .line_spacing(
                        LineSpacing::new()
                            .line(560)
                            .line_rule(LineSpacingType::Exact),
                    ),
            );
        }
    }

    if input.kind.uses_letter_layout() && !attachment_blocks.is_empty() {
        doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));
        let mut counters = [0usize; 4];
        let attachment_count = attachment_blocks
            .iter()
            .filter(|block| matches!(block, MarkdownBlock::Marker(MarkdownSection::Attachment)))
            .count();
        let mut attachment_index = 0usize;
        for block in attachment_blocks {
            match block {
                MarkdownBlock::Marker(MarkdownSection::Attachment) => {
                    if attachment_index > 0 {
                        doc = doc.add_paragraph(
                            Paragraph::new().add_run(Run::new().add_break(BreakType::Page)),
                        );
                    }
                    attachment_index += 1;
                    counters = [0; 4];
                    let label = if attachment_count == 1 {
                        "附件".to_string()
                    } else {
                        format!("附件{attachment_index}")
                    };
                    doc = doc.add_paragraph(attachment_label_paragraph(&label));
                }
                MarkdownBlock::Title(text) => {
                    counters = [0; 4];
                    doc = doc.add_paragraph(attachment_document_title_paragraph(text));
                }
                _ => doc = add_official_content_block(doc, block, &mut counters),
            }
        }
    }

    if input.kind == TemplateKind::OfficialLetter {
        doc = add_footer_record(doc, input, display);
    }

    let file =
        File::create(path).with_context(|| format!("无法创建 Word 文件：{}", path.display()))?;
    doc.build().pack(file).context("写入 DOCX 包失败")?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::models::{JointContact, VocabularyCategory, VocabularyEntry};
    use std::io::Read;

    /// 测试大多使用无层级的扁平单位，空词库让 `UnitDisplay` 回落为规范名称。
    fn write_docx_ok(path: &Path, input: &DraftInput, markdown: &str) -> Result<()> {
        write_docx(path, input, markdown, &UnitDisplay::new(&[]))
    }

    /// 动态生成一张 1x1 红色 PNG 的字节。
    fn tiny_png() -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        cursor.into_inner()
    }

    #[test]
    fn image_paragraph_embeds_bitmap_and_docx_contains_media() {
        let temp = tempfile::tempdir().unwrap();
        let paragraph = image_paragraph_from_bytes("示意图", "示意图.png", false, &tiny_png())
            .expect("有效 PNG 应生成图片段落");
        let mut doc = Docx::new();
        doc = doc.add_paragraph(paragraph);
        let docx_path = temp.path().join("with-image.docx");
        doc.build().pack(File::create(&docx_path).unwrap()).unwrap();
        let file = File::open(&docx_path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        let media: Vec<String> = archive
            .file_names()
            .filter(|name| name.starts_with("word/media/"))
            .map(str::to_string)
            .collect();
        assert!(!media.is_empty(), "docx 应含 word/media/ 图片文件");
        let xml = zip_text(&docx_path, "word/document.xml");
        assert!(xml.contains("<w:drawing>"), "图片段落应含 drawing 元素");
    }

    #[test]
    fn image_paragraph_renders_pdf_as_attachment_note() {
        let temp = tempfile::tempdir().unwrap();
        let paragraph = image_paragraph_from_bytes("", "扫描件.pdf", true, b"%PDF-1.4")
            .expect("PDF 应生成附件说明段落");
        let mut doc = Docx::new();
        doc = doc.add_paragraph(paragraph);
        let docx_path = temp.path().join("with-pdf.docx");
        doc.build().pack(File::create(&docx_path).unwrap()).unwrap();
        let xml = zip_text(&docx_path, "word/document.xml");
        assert!(xml.contains("【附件】扫描件.pdf"));
        // PDF 不产生媒体文件。
        let file = File::open(&docx_path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        assert!(
            archive
                .file_names()
                .all(|name| !name.starts_with("word/media/")),
            "PDF 附件说明不应产生媒体文件"
        );
    }

    #[test]
    fn image_paragraph_ignores_undecodable_bytes() {
        assert!(
            image_paragraph_from_bytes("", "坏图.png", false, b"not an image").is_none(),
            "无法解码的字节应跳过而不是 panic"
        );
    }

    #[test]
    fn image_paragraph_skips_unresolvable_src() {
        // 非法路径（穿越）解析失败时跳过，不 panic。
        assert!(image_paragraph("", "../etc/passwd").is_none());
    }

    #[test]
    fn short_issuing_unit_keeps_its_size_and_stays_centered() {
        let layout = issuing_unit_header("国务院");
        assert_eq!(layout.size, HEADER_SIZE, "字数少时不得改变字号");
        // 三个字加两个一字宽的字距，居中摆放。
        let block = 3 * 480 + 2 * 480;
        assert_eq!(layout.side_indent, (HEADER_WIDTH_TWIPS - block) / 2);
        assert!(layout.side_indent > 0, "短名称应留出左右缩进而不是铺满版心");
    }

    #[test]
    fn medium_issuing_unit_fills_the_line_without_shrinking() {
        // 10 个字：一字宽的字距已经超出版心，因此正好撑满，字号不变。
        let layout = issuing_unit_header("中华人民共和国教育部");
        assert_eq!(layout.size, HEADER_SIZE);
        assert_eq!(layout.side_indent, 0);
    }

    #[test]
    fn long_issuing_unit_shrinks_instead_of_being_squashed() {
        let unit = "某某省人民政府政务服务和数字化建设管理局办公室综合处";
        let layout = issuing_unit_header(unit);
        assert!(layout.size < HEADER_SIZE, "排不下时应缩小字号");
        assert!(layout.size >= HEADER_MIN_SIZE, "不得小于三号");
        assert_eq!(layout.side_indent, 0);
        let count = unit.chars().count() as i32;
        assert!(
            count * layout.size as i32 * 10 <= HEADER_WIDTH_TWIPS,
            "缩小后必须能排进版心"
        );
    }

    #[test]
    fn very_long_issuing_unit_stops_at_the_minimum_size() {
        let layout = issuing_unit_header(&"某".repeat(60));
        assert_eq!(layout.size, HEADER_MIN_SIZE);
    }

    #[test]
    fn single_character_issuing_unit_is_centered() {
        let layout = issuing_unit_header("函");
        assert_eq!(layout.size, HEADER_SIZE);
        assert_eq!(layout.side_indent, (HEADER_WIDTH_TWIPS - 480) / 2);
    }

    #[test]
    fn hierarchical_units_expand_to_full_names_in_header_and_addressee() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hierarchy.docx");
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "新闻舆论处".into();
        input.profile.recipient = "新闻舆论处、信访接待处".into();
        input.profile.responsible_unit = "新闻舆论处".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "中央网信办".into(),
                category: VocabularyCategory::Unit,
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "新闻舆论处".into(),
                category: VocabularyCategory::Unit,
                parent: "中央网信办".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "信访接待处".into(),
                category: VocabularyCategory::Unit,
                parent: "中央网信办".into(),
                ..Default::default()
            },
        ];
        let display = UnitDisplay::new(&vocabulary);
        write_docx(&path, &input, "# 测试函\n\n正文。", &display).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 规格 §2.2：发文单位红头补全上级全称。
        assert!(xml.contains("中央网信办新闻舆论处"), "红头应补全上级全称");
        // 同属一个上级的主送单位：顿号连接且不重复上级。
        assert!(xml.contains("中央网信办新闻舆论处、信访接待处"));
        // 规格 §3.1：版记承办单位显示简称（无简称时回落规范名称）。
        assert!(xml.contains("承办单位：新闻舆论处"));
    }

    #[test]
    fn external_letter_uses_external_names_but_keeps_responsible_abbr() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("external.docx");
        let mut input = DraftInput::default();
        input.profile.correspondence_scope = crate::models::CorrespondenceScope::External;
        input.profile.issuing_unit = "新闻舆论处".into();
        input.profile.recipient = "信访接待处".into();
        input.profile.responsible_unit = "新闻舆论处".into();
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "中央网信办".into(),
                external_name: "国家互联网信息办公室".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "新闻舆论处".into(),
                external_name: "新闻传播管理处".into(),
                parent: "中央网信办".into(),
                abbr: "新舆处".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "信访接待处".into(),
                external_name: "公众服务处".into(),
                parent: "中央网信办".into(),
                ..Default::default()
            },
        ];
        write_docx(
            &path,
            &input,
            "# 外部函测试\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("国家互联网信息办公室新闻传播管理处"));
        assert!(xml.contains("国家互联网信息办公室公众服务处"));
        assert!(xml.contains("承办单位：新舆处"));
    }

    #[test]
    fn red_header_is_written_as_distributed_text_not_stretched_glyphs() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("letter.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "国务院".into();
        input.profile.recipient = "某某市教育局".into();
        write_docx_ok(&path, &input, "# 关于测试红头的函\n\n正文。\n").unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(
            xml.contains(r#"w:val="distribute""#),
            "红头应使用分散对齐，由 Word 均匀撑开字距"
        );
        let indent = (HEADER_WIDTH_TWIPS - (3 * 480 + 2 * 480)) / 2;
        assert!(
            xml.contains(&format!(r#"w:left="{indent}""#)),
            "短名称应靠左右缩进居中，实际 XML：{xml}"
        );
        assert!(
            xml.contains(&format!(r#"w:val="{HEADER_SIZE}""#)),
            "字数少时不得缩小字号"
        );
    }

    #[test]
    fn official_letter_docx_numbers_headings_and_starts_attachments_on_a_new_page() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("letter-with-attachment.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n<!-- [正文] -->\n## 一、总体要求\n### （一）具体事项\n正文。\n<!-- [附件] -->\n# 附件1\n## 统计表\n### 一、填报说明\n附件内容。\n# 附件2\n## 说明材料\n附件二内容。",
        )
        .unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("一、总体要求"));
        assert!(xml.contains("（一）具体事项"));
        assert!(xml.contains("附件1"));
        assert!(xml.contains("统计表"));
        assert!(xml.contains("附件2"));
        assert_eq!(xml.matches("一、填报说明").count(), 1);
        assert_eq!(xml.matches(r#"w:type="page""#).count(), 2, "{xml}");
        // 附件概要：正文结束后、落款前按顺序列出附件名称，与正文空两行、首行缩进两个汉字。
        assert!(xml.contains("附件1：统计表"), "{xml}");
        assert!(xml.contains("　　2：说明材料"), "{xml}");
        let summary = paragraph_containing(&xml, "附件1：统计表");
        assert!(summary.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(!summary.contains("w:eastAsia=\"黑体\""));
        assert!(summary.contains(r#"w:firstLine="640""#), "{summary}");
        let gap = gap_between(&xml, "正文。", "附件1：统计表");
        assert_eq!(gap.matches("<w:p ").count(), 2, "概要前应空两行：{gap}");
        assert!(!gap.contains("<w:t"));
        let body_heading = paragraph_containing(&xml, "一、总体要求");
        assert!(body_heading.contains(r#"w:firstLine="640""#));
        // 附件区真正的“附件1”黑体标签与“统计表”小标宋标题均位于概要之后。
        let after_summary = &xml[xml.find("附件1：统计表").unwrap() + "附件1：统计表".len()..];
        let attachment_label = paragraph_containing(after_summary, "附件1");
        assert!(attachment_label.contains("w:eastAsia=\"黑体\""));
        assert!(attachment_label.contains("w:ascii=\"SimHei\""));
        assert!(attachment_label.contains("w:hAnsi=\"SimHei\""));
        assert!(attachment_label.contains("w:val=\"left\""));
        assert!(!attachment_label.contains("w:firstLine"));
        let attachment_title = paragraph_containing(after_summary, "统计表");
        assert!(attachment_title.contains("w:eastAsia=\"方正小标宋简体\""));
        assert!(attachment_title.contains("w:val=\"center\""));
        let attachment_heading = paragraph_containing(&xml, "一、填报说明");
        assert!(attachment_heading.contains(r#"w:firstLine="640""#));
        assert!(!xml.contains("一、一、"));
    }

    #[test]
    fn official_letter_preview_masks_serial_and_day_but_formal_keeps_them() {
        let temp = tempfile::tempdir().unwrap();
        let formal_path = temp.path().join("formal.docx");
        let preview_path = temp.path().join("preview.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.department_code = "某政函".into();
        input.profile.document_number = "12".into();
        input.date = "2026年8月5日".into();

        write_docx_ok(&formal_path, &input, "# 测试函\n\n正文。").unwrap();
        let formal = zip_text(&formal_path, "word/document.xml");
        assert!(formal.contains("某政函〔2026〕12号"));
        assert!(formal.contains("2026年8月5日"));

        input.profile.letter_version = LetterVersion::Preview;
        write_docx_ok(&preview_path, &input, "# 测试函\n\n正文。").unwrap();
        let preview = zip_text(&preview_path, "word/document.xml");
        // 规格 §3.3：预览版占位统一 1em。
        assert!(preview.contains("某政函〔2026〕\u{2003}号"));
        assert!(preview.contains("2026年8月\u{2003}日"));
        assert!(!preview.contains("某政函〔2026〕12号"));
        assert!(!preview.contains("2026年8月5日"));
    }

    #[test]
    fn phone_notice_keeps_letter_layout_but_omits_number_and_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("phone-notice.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PhoneNotice;
        input.profile.issuing_unit = "某某省教育厅".into();
        input.profile.department_code = "某教函".into();
        input.profile.document_number = "12".into();
        input.profile.recipient = "某某市教育局".into();
        input.profile.copies_to = "某某市财政局".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-12345678".into();
        write_docx_ok(
            &path,
            &input,
            "# 关于测试事项的电话通知\n<!-- [正文] -->\n## 工作要求\n正文。\n<!-- [附件] -->\n# 附件标题\n附件内容。",
        )
        .unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("某某省教育厅"));
        assert!(xml.contains("某某市教育局"));
        assert!(xml.contains("一、工作要求"));
        assert!(xml.contains("附件"));
        assert!(!xml.contains("某教函"));
        assert!(!xml.contains("〔"));
        assert!(!xml.contains("抄送："));
        assert!(!xml.contains("承办单位："));
        assert!(!xml.contains("张三"));
        assert!(!xml.contains("010-12345678"));
    }

    #[test]
    fn phone_notice_signature_uses_spaced_abbreviation() {
        // 规格 §3.1：电话通知落款显示简称，少于 5 字时逐字加半角空格。
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("phone-notice-abbr.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PhoneNotice;
        input.profile.issuing_unit = "中央宣传部".into();
        let vocabulary = vec![VocabularyEntry {
            canonical: "中央宣传部".into(),
            category: VocabularyCategory::Unit,
            abbr: "中宣部".into(),
            ..Default::default()
        }];
        let display = UnitDisplay::new(&vocabulary);
        write_docx(&path, &input, "# 电话通知\n\n正文。", &display).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 红头保留全称，落款输出“中 宣 部”（简称逐字加半角空格）。
        assert!(xml.contains("中央宣传部"), "红头应保留全称");
        let signature = paragraph_containing(&xml, "中 宣 部");
        assert!(signature.contains("w:val=\"right\""), "落款应右对齐");
        assert!(!xml.contains("中宣部"), "不得输出未加空格的简称");
    }

    #[test]
    fn white_paper_signature_stacks_multiple_units_right_aligned_with_spread() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("white-paper-multi.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.signing_unit = "星海省教育厅、教师工作处".into();
        input.profile.use_short_name_for_signature = true;
        input.date = "2026年8月7日".into();
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "星海省教育厅".into(),
                category: VocabularyCategory::Unit,
                abbr: "省教育厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "教师工作处".into(),
                category: VocabularyCategory::Unit,
                abbr: "教师处".into(),
                ..Default::default()
            },
        ];
        let display = UnitDisplay::new(&vocabulary);
        write_docx(&path, &input, "# 标题\n\n正文。", &display).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 两个单位分行列出；简称逐字成 run，字间距 320 缇（1em，三号 16pt 下分散到 5 字宽）。
        for ch in ["省", "教", "厅", "教", "师", "处"] {
            assert!(
                xml.contains(&format!("<w:t xml:space=\"preserve\">{ch}</w:t>")),
                "应含“{ch}”：{xml}"
            );
        }
        assert!(
            xml.contains("w:spacing w:val=\"320\""),
            "3 字简称应有 1em 字符间距：{xml}"
        );
        // 单位与日期段落都右对齐。
        assert!(xml.contains("w:val=\"right\""), "落款应右对齐：{xml}");
        // 不出现未分散的整串简称。
        assert!(!xml.contains("省教育厅"), "简称不应整串出现：{xml}");
    }

    #[test]
    fn white_paper_signature_single_unit_keeps_full_name() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("white-paper-single.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.signing_unit = "星海省教育厅".into();
        input.date = "2026年8月7日".into();
        write_docx_ok(&path, &input, "# 标题\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(
            xml.contains("星海省教育厅"),
            "单单位未选简称应输出全称：{xml}"
        );
    }

    #[test]
    fn official_letter_seal_on_behalf_follows_the_unit_on_the_same_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("seal-on-behalf.docx");
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "星海省教育厅".into();
        input.date = "2026年8月7日".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(!xml.contains("（代章）"), "单位未配置时不得标注代章");

        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "星海省教育厅".into(),
            seal_on_behalf: true,
            ..Default::default()
        }];
        write_docx(
            &path,
            &input,
            "# 测试函\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 落款单位出现在红头与落款两处，最后一次出现是落款；“（代章）”直接跟在
        // 落款单位后面同一行（“星海省教育厅（代章）”），成文日期在其后另起一行。
        let unit_pos = xml.rfind("星海省教育厅").expect("落款单位");
        let seal_pos = xml.find("（代章）").expect("代章标注");
        let date_pos = xml.find("2026年8月7日").expect("成文日期");
        assert!(unit_pos < seal_pos && seal_pos < date_pos);
        assert!(
            xml.contains("星海省教育厅（代章）"),
            "代章应与落款单位同一行：{xml}"
        );
        let seal = paragraph_containing(&xml, "（代章）");
        assert!(seal.contains("w:val=\"right\""), "代章应与落款同侧对齐");
    }

    #[test]
    fn plain_document_keeps_body_and_attachments_but_omits_all_decorative_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plain-document.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PlainDocument;
        input.profile.issuing_unit = "不应出现的发文单位".into();
        input.profile.recipient = "不应出现的主送单位".into();
        input.profile.signing_unit = "不应出现的落款单位".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        input.date = "2026年8月7日".into();
        write_docx_ok(
            &path,
            &input,
            "# 普通公文测试\n<!-- [正文] -->\n## 工作要求\n正文内容。\n<!-- [附件] -->\n# 附件1\n## 附件标题\n附件内容。",
        )
        .unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(runs_text(&xml).contains("秘密★10年"));
        assert!(xml.contains("普通公文测试"));
        assert!(xml.contains("一、工作要求"));
        assert!(xml.contains("正文内容。"));
        assert!(xml.contains("附件：附件标题"));
        assert!(xml.contains("附件内容。"));
        assert!(!xml.contains("不应出现的发文单位"));
        assert!(!xml.contains("不应出现的主送单位"));
        assert!(!xml.contains("不应出现的落款单位"));
        assert!(!xml.contains("2026年8月7日"));
        assert!(!xml.contains("指人专办"));
        assert!(!xml.contains("C00000"), "普通公文不得出现红头颜色");
    }

    #[test]
    fn joint_mode_one_uses_main_header_two_column_signatures_and_multiline_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("joint-letter.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        input.profile.department_code = "乙函".into();
        input.profile.document_number = "8".into();
        input.profile.recipient = "收文单位".into();
        input.profile.copies_to = "丁单位、戊单位".into();
        input.profile.joint_responsible_units = "甲处室、乙处室".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "甲处室".into(),
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact {
                unit: "乙处室".into(),
                name: "李四".into(),
                phone: "010-22222222".into(),
            },
        ];
        input.date = "2026年8月5日".into();
        write_docx_ok(&path, &input, "# 联合发文测试函\n\n正文。").unwrap();

        let xml = zip_text(&path, "word/document.xml");
        let header = paragraph_containing(&xml, "乙单位");
        assert!(header.contains("C00000"), "主发文单位应写入红头：{header}");
        assert!(!paragraph_containing(&xml, "乙单位").contains("甲单位、乙单位、丙单位"));
        assert!(xml.contains("甲单位"));
        assert!(xml.contains("丙单位"));
        assert!(xml.contains(r#"w:trHeight w:val="2551""#));
        // 规格 §2.5：三个单位时最后一个单位跨两列居中。
        assert!(xml.contains(r#"w:gridSpan w:val="2""#));
        assert!(xml.contains("2026年8月5日"));
        // 日期压在主发文单位“乙单位”（右列）所在单元格，而不是整行居中。
        let date_at = xml.find("2026年8月5日").unwrap();
        let cell_start = xml[..date_at].rfind("<w:tc>").unwrap();
        let cell_end = xml[date_at..].find("</w:tc>").unwrap() + date_at + "</w:tc>".len();
        let row_start = xml[..cell_start].rfind("<w:tr>").unwrap();
        let row_end = xml[cell_end..].find("</w:tr>").unwrap() + cell_end + "</w:tr>".len();
        let closing_row = &xml[row_start..row_end];
        assert_eq!(closing_row.matches("<w:tc>").count(), 2, "收尾行应有两列");
        let first_cell_end = closing_row.find("</w:tc>").unwrap() + "</w:tc>".len();
        assert!(
            closing_row[first_cell_end..].contains("2026年8月5日"),
            "日期应在主发文单位（右列）单元格内"
        );
        assert_eq!(xml.matches("承办单位：").count(), 1);
        assert_eq!(xml.matches("联系人：").count(), 1);
        assert_eq!(xml.matches("联系电话：").count(), 1);
        // 联系电话列预留 11em（标签 5em + 11 位半角数字 5.5em + 0.5em 余量），
        // 其余两列均分剩余宽度，11 位电话号码不再换行。
        assert_eq!(xml.matches(r#"w:gridCol w:w="2882""#).count(), 2);
        assert_eq!(xml.matches(r#"w:gridCol w:w="3080""#).count(), 1);
        assert!(
            xml.contains(r#"w:tcW w:w="3080""#),
            "联系电话单元格宽度应能容纳 11 位数字"
        );
        assert!(xml.contains(r#"w:tab w:val="right" w:pos="8844""#));
        assert!(xml.contains("（共印5份）"));
        // 版记首行抄送跨三列，单元格带 gridSpan=3；回行时用悬挂缩进对齐单位名称。
        assert!(xml.contains(r#"w:gridSpan w:val="3""#));
        assert!(paragraph_containing(&xml, "抄送：").contains(r#"w:hanging="840""#));
        // 规格 §3.2：第 2 行起承办单位、联系人用左缩进对齐第一行标签后的位置。
        assert!(xml.contains(r#"w:left="1400""#));
        assert!(xml.contains(r#"w:left="1120""#));
        assert!(paragraph_containing(&xml, "承办单位：").contains(r#"w:sz w:val="28""#));
        assert!(paragraph_containing(&xml, "（共印5份）").contains(r#"w:sz w:val="28""#));
        assert!(xml.contains("甲处室"));
        assert!(xml.contains("乙处室"));
        // 规格 §3.2：2 字姓名中间加全角空格，占 3 字宽。
        assert!(xml.contains("张\u{2003}三"));
        assert!(xml.contains("李\u{2003}四"));
        assert!(xml.contains("010-11111111"));
        assert!(xml.contains("010-22222222"));
        // 版记行高必须交给 Word 自动撑开：承办单位没维护简称时会回落较长的
        // 规范全称，固定行高会把折行后的第二行直接裁掉。
        let record_at = xml.find("承办单位：").unwrap();
        let record_row_start = xml[..record_at].rfind("<w:tr>").unwrap();
        assert!(
            !xml[record_row_start..record_at].contains("hRule=\"exact\""),
            "联合发文版记行不得使用固定行高：{}",
            &xml[record_row_start..record_at]
        );
    }

    #[test]
    fn joint_mode_one_single_unit_falls_back_to_right_aligned_signature() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("joint-single.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        input.profile.department_code = "甲函".into();
        input.profile.document_number = "9".into();
        input.profile.recipient = "收文单位".into();
        input.profile.joint_responsible_units = "甲处室".into();
        input.profile.joint_contacts = vec![JointContact {
            unit: "甲处室".into(),
            name: "张三".into(),
            phone: "010-11111111".into(),
        }];
        input.date = "2026年8月5日".into();
        write_docx_ok(&path, &input, "# 单单位联合函\n\n正文。").unwrap();

        let xml = zip_text(&path, "word/document.xml");
        // 最后一个“甲单位”出现在落款（红头在前）：应是右对齐的单独段落，
        // 而不是联合发文的左列居中两列表格。
        let mut search_from = 0;
        let mut last_sig = "";
        while let Some(at) = xml[search_from..].find("甲单位") {
            let abs = search_from + at;
            let start = xml[..abs].rfind("<w:p ").unwrap();
            let end = xml[abs..].find("</w:p>").unwrap() + abs + "</w:p>".len();
            last_sig = &xml[start..end];
            search_from = end;
        }
        assert!(!last_sig.is_empty(), "落款应出现“甲单位”");
        assert!(
            last_sig.contains(r#"w:jc w:val="right""#),
            "单单位联合发文落款应右对齐：{last_sig}"
        );
        assert!(
            !last_sig.contains("<w:tbl>"),
            "不应再走联合落款两列表格：{last_sig}"
        );
        // 联合落款的两列表格（72mm 列）不再出现。
        assert!(!xml.contains(r#"w:gridCol w:w="4422""#));
    }

    #[test]
    fn official_letter_docx_supports_simplex_and_duplex_page_numbers() {
        let temp = tempfile::tempdir().unwrap();
        let simplex_path = temp.path().join("simplex.docx");
        let duplex_path = temp.path().join("duplex.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;

        write_docx_ok(&simplex_path, &input, "# 测试函\n\n正文。").unwrap();
        let simplex_document = zip_text(&simplex_path, "word/document.xml");
        let simplex_settings = zip_text(&simplex_path, "word/settings.xml");
        let simplex_footer = zip_text(&simplex_path, "word/footer1.xml");
        let simplex_first_footer = zip_text(&simplex_path, "word/footer2.xml");
        assert!(simplex_document.contains(r#"w:type="default""#));
        assert!(simplex_document.contains(r#"w:type="first""#));
        assert!(!simplex_document.contains(r#"w:type="even""#));
        assert!(!simplex_settings.contains("w:evenAndOddHeaders"));
        assert!(simplex_footer.contains(r#"w:val="center""#));
        assert!(simplex_footer.contains("<w:instrText>PAGE</w:instrText>"));
        assert!(!simplex_first_footer.contains("<w:instrText>PAGE</w:instrText>"));

        input.profile.duplex_printing = true;
        write_docx_ok(&duplex_path, &input, "# 测试函\n\n正文。").unwrap();
        let duplex_document = zip_text(&duplex_path, "word/document.xml");
        let duplex_settings = zip_text(&duplex_path, "word/settings.xml");
        let odd_footer = zip_text(&duplex_path, "word/footer1.xml");
        let even_footer = zip_text(&duplex_path, "word/footer2.xml");
        let first_footer = zip_text(&duplex_path, "word/footer3.xml");
        assert!(duplex_document.contains(r#"w:type="default""#));
        assert!(duplex_document.contains(r#"w:type="even""#));
        assert!(duplex_document.contains(r#"w:type="first""#));
        assert!(duplex_settings.contains("w:evenAndOddHeaders"));
        assert!(odd_footer.contains(r#"w:val="right""#));
        assert!(even_footer.contains(r#"w:val="left""#));
        assert!(odd_footer.contains("<w:instrText>PAGE</w:instrText>"));
        assert!(even_footer.contains("<w:instrText>PAGE</w:instrText>"));
        assert!(!first_footer.contains("<w:instrText>PAGE</w:instrText>"));
    }

    #[test]
    fn attachment_table_uses_smart_fixed_grid_and_official_fonts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("letter-table.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n<!-- [附件] -->\n# 附件1\n## 统计表\n| 序号 | 名称 | 详细说明 |\n| --- | --- | --- |\n| 1 | 甲 | 这是一段**重要说明**。 |\n| 2 | 乙 | 另一段较长的说明文字。 |",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains(r#"w:tblLayout w:type="fixed""#));
        assert!(xml.contains(r#"w:gridCol w:w="560""#));
        assert!(xml.contains("w:eastAsia=\"黑体\""));
        assert!(xml.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(xml.contains("w:insideH"));
        assert!(xml.contains("w:insideV"));
        let bold_cell = paragraph_containing(&xml, "重要说明");
        assert!(
            bold_cell.contains("<w:b"),
            "表格 Markdown 加粗应保留：{bold_cell}"
        );
        assert!(!bold_cell.contains("**"));
    }

    #[test]
    fn compact_style_merges_heading_and_paragraph_into_one_paragraph() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("compact.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.style_mode = StyleMode::Compact;
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        write_docx_ok(&path, &input, "# 测试函\n\n## 任务目标\n测试正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 标题与正文并入同一段：黑体标题 run + 仿宋正文 run。
        let merged = paragraph_containing(&xml, "任务目标");
        assert!(
            merged.contains("一、任务目标。"),
            "标题应带编号与句号：{merged}"
        );
        assert!(
            merged.contains("测试正文"),
            "紧缩风格应把正文并入标题段：{merged}"
        );
        assert!(merged.contains("黑体"), "标题部分应使用标题字体：{merged}");
    }

    #[test]
    fn compact_style_merges_every_deepest_heading_paragraph_pair() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("compact-deep.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.style_mode = StyleMode::Compact;
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        // 正文区 # 号最多的是 3 级（###）：每个“3 级标题+段落”都合并，2 级标题保持独立。
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n\n## 一、总体要求\n开头段落。\n### （一）任务一\n任务一正文。\n### （二）任务二\n任务二正文。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 2 级标题保持独立：正文段落单独成段，不并入标题段。
        let level_two = paragraph_containing(&xml, "总体要求");
        assert!(level_two.contains("一、总体要求"));
        assert!(
            !level_two.contains("开头段落"),
            "2 级标题不应合并正文：{level_two}"
        );
        // 每个 3 级标题都与紧随正文合并，用楷体标题字体。
        for (title, body) in [
            ("（一）任务一", "任务一正文"),
            ("（二）任务二", "任务二正文"),
        ] {
            let merged = paragraph_containing(&xml, title);
            assert!(
                merged.contains(&format!("{title}。")),
                "标题应带编号与句号：{merged}"
            );
            assert!(
                merged.contains(body),
                "每个最深层标题都应合并正文：{merged}"
            );
            assert!(
                merged.contains("楷体_GB2312"),
                "3 级标题应使用楷体标题字体：{merged}"
            );
        }
    }

    #[test]
    fn empty_copies_to_leaves_the_record_line_without_a_dangling_label() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("no-copies.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.recipient = "某部门".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 与 gonghan-gwa.cls 的 \FooterCopiesLine 一致：没有抄送单位时整行不写“抄送：”。
        assert!(!xml.contains("抄送："), "无抄送单位时不应留下空标签");
        assert!(xml.contains("（共印2份）"));
        assert!(xml.contains("承办单位：办公室"));
    }

    #[test]
    fn red_head_approval_docx_has_floating_frame_records_small_title_and_second_page_signature() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("red-approval.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::RedHeadApproval;
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.issuing_unit = "某某委员会办公室".into();
        input.profile.department_code = "某办呈".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "12".into();
        input.profile.reporting_leaders = "张三、李四".into();
        input.profile.signing_unit = "某某委员会".into();
        input.profile.joint_responsible_units = "综合处、业务处".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "综合处".into(),
                name: "王五".into(),
                phone: "010-12345678".into(),
            },
            JointContact {
                unit: "业务处".into(),
                name: "赵六".into(),
                phone: "010-87654321".into(),
            },
        ];
        input.date = "2026年8月12日".into();
        write_docx_ok(
            &path,
            &input,
            "# 关于认真做好网络安全与信息化重点工作的请示\n\n现将有关情况呈报如下。妥否，请指示。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("某办呈〔2026〕12号"));
        assert!(xml.contains("批　示"));
        assert!(xml.contains("w:tblpXSpec=\"right\""));
        assert!(xml.contains("w:tblpY=\"2720\""));
        assert!(xml.contains("w:tblpYSpec=\"bottom\""));
        assert!(xml.contains("承办单位："));
        assert!(xml.contains("综合处"));
        assert!(xml.contains("业务处"));
        let title = paragraph_containing(&xml, "关于认真做好网络安全与");
        assert!(title.contains("w:sz w:val=\"36\""));
        assert!(title.contains("w:right=\"3175\""));
        let notice = paragraph_containing(&xml, "（此页无正文）");
        assert!(notice.contains("w:pageBreakBefore"));
        assert!(xml.find("（此页无正文）").unwrap() < xml.rfind("某某委员会").unwrap());
        // 落款单位右对齐，但右缩进 4cm 让出签字空间。从「此页无正文」之后找起，
        // 免得匹配到红头里的「某某委员会办公室」。
        let closing = &xml[xml.find("（此页无正文）").unwrap()..];
        let signature = paragraph_containing(closing, "某某委员会");
        assert!(
            signature.contains(r#"w:val="right""#)
                && signature.contains(&format!(
                    r#"w:right="{}""#,
                    crate::export::SIGNATURE_ROOM_TWIPS
                )),
            "落款单位右侧应留出签字空间：{signature}"
        );
        // 成文日期居中于“落款单位 + 签字空间”：左缩进到最宽单位的左沿，
        // 段落右缘仍是版心右缘，居中位置正好是这一整段的中点。
        let date = paragraph_containing(&xml, "2026年8月12日");
        let expected_left = TABLE_CONTENT_WIDTH_TWIPS
            - crate::export::SIGNATURE_ROOM_TWIPS
            - crate::export::red_signature_unit_width_twips(&["某某委员会".to_string()]);
        assert!(
            date.contains(r#"w:val="center""#)
                && date.contains(&format!(r#"w:left="{expected_left}""#)),
            "成文日期应居中于单位与签字空间之间：{date}"
        );
        // 承办区三栏不再等分：承办单位栏最宽，另两栏按内容定宽。
        assert!(
            xml.contains(&format!(
                "<w:gridCol w:w=\"{}\" w:type=\"dxa\" /><w:gridCol w:w=\"{}\" w:type=\"dxa\" /><w:gridCol w:w=\"{}\" w:type=\"dxa\" />",
                crate::export::RED_RECORD_UNIT_TWIPS,
                crate::export::RED_RECORD_CONTACT_TWIPS,
                crate::export::RED_RECORD_PHONE_TWIPS
            )),
            "承办区栏宽应与 LaTeX/预览同源：{xml}"
        );
    }

    /// 承办单位一律不换行：栏内放不下时整格按同一比例横向压窄（`w:w`），
    /// 行高固定一行，与 LaTeX 的 \RedFit 和首页承办区高度对齐。
    #[test]
    fn red_head_approval_record_compresses_long_units_instead_of_wrapping() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("red-approval-long-unit.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::RedHeadApproval;
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.issuing_unit = "某某委员会办公室".into();
        input.profile.signing_unit = "某某委员会".into();
        input.profile.joint_responsible_units = "综合处、教师工作与师资管理处".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "综合处".into(),
                name: "王五".into(),
                phone: "010-12345678".into(),
            },
            JointContact {
                unit: "教师工作与师资管理处".into(),
                name: "赵六".into(),
                phone: "010-87654321".into(),
            },
        ];
        input.date = "2026年8月12日".into();
        write_docx_ok(&path, &input, "# 标题\n\n正文。妥否，请指示。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let scale = crate::export::red_record_scale_percent(
            "承办单位：教师工作与师资管理处",
            crate::export::RED_RECORD_UNIT_USABLE_TWIPS,
        );
        assert!(scale < 100, "长单位名应触发压缩");
        assert!(
            xml.contains(&format!("<w:w w:val=\"{scale}\"")),
            "长单位名应按 {scale}% 横向压窄：{xml}"
        );
        // 短单位名不压缩，也不得整行被裁：行高固定为一行 28pt。
        assert!(
            xml.contains("<w:trHeight w:val=\"560\" w:hRule=\"exact\" />"),
            "{xml}"
        );
    }

    /// 红头呈批件正文里的表格要被赶出首页，否则会压过右侧批示栏。
    #[test]
    fn red_head_approval_body_table_starts_on_a_new_page() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("red-approval-table.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::RedHeadApproval;
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.issuing_unit = "某某委员会办公室".into();
        input.profile.signing_unit = "某某委员会".into();
        input.profile.joint_responsible_units = "综合处".into();
        input.date = "2026年8月12日".into();
        write_docx_ok(
            &path,
            &input,
            "# 标题\n\n短正文。\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n\n妥否，请指示。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let table = xml.find("甲").expect("正文应含表格");
        let break_before = xml[..table]
            .rfind("w:pageBreakBefore")
            .expect("表格前应有换页");
        assert!(
            xml[break_before..table].find("短正文").is_none(),
            "换页要排在表格之前、短正文之后：{xml}"
        );
        // 正文因表格延续到第二页，落款接在正文之后，不再制造“此页无正文”空白页。
        assert!(!xml.contains("（此页无正文）"), "{xml}");
    }

    fn zip_text(path: &Path, entry: &str) -> String {
        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut text = String::new();
        archive
            .by_name(entry)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    fn paragraph_containing<'a>(xml: &'a str, needle: &str) -> &'a str {
        let needle_at = xml.find(needle).unwrap();
        let start = xml[..needle_at].rfind("<w:p ").unwrap();
        let end = xml[needle_at..].find("</w:p>").unwrap() + needle_at + "</w:p>".len();
        &xml[start..end]
    }

    /// 取一段 XML 里所有 `<w:t>` 文本拼接后的纯文本。密级行的数字年限拆成了
    /// “黑体 + 等宽数字 + 黑体”多个 run，整行断言用拼接后的文本更稳妥。
    fn runs_text(xml: &str) -> String {
        let re = Regex::new(r#"<w:t[^>]*>([^<]*)</w:t>"#).unwrap();
        re.captures_iter(xml)
            .map(|capture| capture[1].to_string())
            .collect()
    }

    fn gap_between<'a>(xml: &'a str, before: &str, after: &str) -> &'a str {
        let before_at = xml.find(before).unwrap();
        let before_end = xml[before_at..].find("</w:p>").unwrap() + before_at + "</w:p>".len();
        let after_at = xml.find(after).unwrap();
        let after_start = xml[..after_at].rfind("<w:p ").unwrap();
        &xml[before_end..after_start]
    }

    #[test]
    fn meeting_agenda_docx_has_required_fonts_indents_and_numbering() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agenda.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.title_hint = "专题研商会议议程".into();
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        let markdown = "# 专题研商会议议程\n\n一、时间地点：2026年8月5日（星期三）14:30，3C会议室。\n\n二、参加人员：张三同志、项目组成员。\n\n三、研讨内容：\n\n1. 汇报总体思路；\n2. 研究下一步工作。";

        write_docx_ok(&path, &input, markdown).unwrap();
        let document = zip_text(&path, "word/document.xml");
        let numberings = zip_text(&path, "word/numbering.xml");

        assert!(document.find("机密★").unwrap() < document.find("专题研商会议议程").unwrap());
        let security = paragraph_containing(&document, "机密★");
        assert!(security.contains("w:eastAsia=\"黑体\""));
        assert!(!security.contains("w:firstLine=\"640\""));
        let security_title_gap = gap_between(&document, "机密★", "专题研商会议议程");
        assert_eq!(
            security_title_gap.matches("<w:p ").count(),
            1,
            "{security_title_gap}"
        );
        assert!(!security_title_gap.contains("<w:t"));

        let title_body_gap = gap_between(&document, "专题研商会议议程", "一、时间地点：");
        assert_eq!(
            title_body_gap.matches("<w:p ").count(),
            1,
            "{title_body_gap}"
        );
        assert!(!title_body_gap.contains("<w:t"));

        let time = paragraph_containing(&document, "一、时间地点：");
        assert!(time.contains("w:eastAsia=\"黑体\""));
        assert!(time.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(time.contains("w:firstLine=\"640\""));

        let attendees = paragraph_containing(&document, "二、参加人员：");
        assert!(attendees.contains("w:eastAsia=\"黑体\""));
        assert!(attendees.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(attendees.contains("w:firstLine=\"640\""));

        let content = paragraph_containing(&document, "三、研讨内容：");
        assert!(content.contains("w:eastAsia=\"黑体\""));
        assert!(content.contains("w:firstLine=\"640\""));

        let first_item = paragraph_containing(&document, "汇报总体思路；");
        let second_item = paragraph_containing(&document, "研究下一步工作。");
        assert!(first_item.contains("<w:numPr>"));
        assert!(second_item.contains("<w:numPr>"));
        assert!(first_item.contains("w:firstLine=\"640\""));
        assert!(second_item.contains("w:firstLine=\"640\""));
        assert!(numberings.contains("w:val=\"%1.\""));
    }

    #[test]
    fn parenthesized_body_uses_kaiti_and_smaller_size() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("parens.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n\n他说：\"**重要事项**\"。现就（有关事项）及【特别说明】函告如下，参见附表（三）。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let paragraph = paragraph_containing(&xml, "现就");
        // 完整括号段用楷体_GB2312，字号为四号（28 半磅）；其余正文保持仿宋三号。
        assert!(
            paragraph.contains("w:eastAsia=\"楷体_GB2312\""),
            "{paragraph}"
        );
        assert!(
            paragraph.contains(&format!(r#"w:val="{PAREN_SIZE}""#)),
            "{paragraph}"
        );
        assert!(paragraph.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(paragraph.contains(&format!(r#"w:val="{BODY_SIZE}""#)));
        assert!(paragraph.contains("“"));
        assert!(paragraph.contains("”"));
        assert!(!paragraph.contains("**"));
        assert!(
            paragraph.contains("<w:b"),
            "Markdown 加粗应生成加粗属性：{paragraph}"
        );
        assert!(paragraph.contains("【特别说明】"));
    }

    #[test]
    fn special_handling_marks_after_security_level_in_black_font() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("special.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let security = paragraph_containing(&xml, "秘密");
        let text = runs_text(security);
        assert!(security.contains("指人专办"));
        assert!(text.contains("★10年"));
        // “指人专办”位于“密级★保密期限”之后，且以黑体单独成 run。
        assert!(
            text.find("★10年").unwrap() < text.find("指人专办").unwrap(),
            "指人专办应排在保密期限之后：{text}"
        );
        assert!(security.contains("w:eastAsia=\"黑体\""));
        assert!(security.contains("w:eastAsia=\"仿宋_GB2312\""));
    }

    #[test]
    fn numeric_security_period_digits_use_monospace_font() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("security-mono.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let security = paragraph_containing(&xml, "秘密");
        assert!(runs_text(security).contains("秘密★10年"));
        // 数字“10”单独成 run，用等宽西文字体（对应 LaTeX 的 ttfamily）。
        let digit_run = paragraph_containing(&xml, ">10<");
        assert!(
            digit_run.contains("Courier New"),
            "数字应用等宽西文字体：{digit_run}"
        );
        assert!(
            security.contains("w:eastAsia=\"仿宋_GB2312\""),
            "“年”等其余部分仍用行内基准字体"
        );

        // 非数字期限（“长期”）不使用等宽字体。
        input.profile.security_period = "长期".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(!xml.contains("Courier New"), "“长期”不应使用等宽字体");
    }

    #[test]
    fn meeting_agenda_special_handling_appends_to_heiti_security_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agenda-special.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        write_docx_ok(&path, &input, "# 专题会议议程\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let security = paragraph_containing(&xml, "机密");
        let text = runs_text(security);
        // 会议议程密级行整行为黑体，“指人专办”跟在“密级★保密期限”之后。
        assert!(text.contains("机密★10年"));
        assert!(text.contains("指人专办"));
        assert!(
            text.find("★10年").unwrap() < text.find("指人专办").unwrap(),
            "指人专办应排在保密期限之后：{text}"
        );
        assert!(security.contains("w:eastAsia=\"黑体\""));
    }

    #[test]
    fn document_title_compresses_small_overflow_and_wraps_large() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();

        // 22 字标题（超出一行 2 字）→ 字高不变（字号仍为二号），仅横向缩放字形。
        let compress_path = temp.path().join("compress.docx");
        let title_text = "关于认真做好网络安全与信息化重点工作验收的函";
        write_docx_ok(&compress_path, &input, &format!("# {title_text}\n\n正文。")).unwrap();
        let xml = zip_text(&compress_path, "word/document.xml");
        let title = paragraph_containing(&xml, title_text);
        assert!(
            title.contains(&format!(r#"w:sz w:val="{TITLE_SIZE}""#)),
            "字高应保持二号：{title}"
        );
        let scale = title::compressed_scale_percent(title_text);
        assert!(
            title.contains(&format!(r#"<w:w w:val="{scale}""#)),
            "仅横向缩放 {scale}%：{title}"
        );

        // 35 字标题 → jieba 换行，行间用文本换行符（不拆词）。
        let wrap_path = temp.path().join("wrap.docx");
        write_docx_ok(
            &wrap_path,
            &input,
            "# 关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知\n\n正文。",
        )
        .unwrap();
        let xml = zip_text(&wrap_path, "word/document.xml");
        let first_line = paragraph_containing(&xml, "关于转发国家互联网信息办公室");
        assert!(
            first_line.contains(r#"w:type="textWrapping""#),
            "长标题应换行：{first_line}"
        );
    }
}
