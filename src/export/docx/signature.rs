//! 落款：白头件/联合发文落款、文号日期与附件概要。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::signature`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::chinese_date_parts;
use crate::export::docx::{
    CLOSING_GAP_TWIPS, JOINT_SIGNATURE_SEAL_GAP_TWIPS, PREVIEW_PLACEHOLDER,
    TABLE_CONTENT_WIDTH_TWIPS, body_run, body_runs, joint_closing_paragraph,
    joint_signature_cell_paragraph, spread_runs,
};
use crate::models::{DraftInput, JointIssuanceMode, LetterVersion, TemplateKind, split_units};
use crate::units::UnitDisplay;
use docx_rs::*;

pub(crate) fn official_document_number(input: &DraftInput) -> Option<String> {
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

pub(crate) fn official_signature_date(input: &DraftInput) -> String {
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
pub(crate) fn add_attachment_summary(mut doc: Docx, names: &[String]) -> Docx {
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

pub(crate) fn is_joint_mode_one(input: &DraftInput) -> bool {
    input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1
}

pub(crate) fn main_issuing_unit(input: &DraftInput, display: &UnitDisplay) -> String {
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
pub(crate) fn add_white_paper_signature(
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
                    .before(if index == 0 { CLOSING_GAP_TWIPS } else { 0 })
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

pub(crate) fn add_joint_signature(doc: Docx, input: &DraftInput, display: &UnitDisplay) -> Docx {
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
pub(crate) fn joint_unit_name(
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
pub(crate) fn joint_closing_row(input: &DraftInput) -> TableRow {
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
