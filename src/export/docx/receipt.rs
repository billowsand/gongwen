//! 公函签收记录单的 Word 生成器：独立成文件，绝不拼进公函正文。
//!
//! 数据来自 `export::typst::receipt`（与 PDF 端同一份），两端字段、签收单位顺序与
//! 去重规则、表格列宽不会各说各话；这里只负责按 Word 版式排：题名小标宋二号居中，
//! 要素行黑体标签 + 仿宋取值，签收表六列、表头黑体四号、内容仿宋四号，外框粗内部
//! 细线，跨页重复表头、行不拆分，每页都印页码，手写处是带下划线的全角空格。

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use docx_rs::*;

use crate::export::docx::{
    BODY_BOTTOM_MARGIN_TWIPS, FOOTER_DISTANCE_TWIPS, TABLE_SIZE, TITLE_SIZE, chinese_fonts,
    table_run_sized, title_run,
};
use crate::export::typst::receipt::{
    COLUMN_NAMES, COLUMN_WIDTHS_MM, CONTENT_WIDTH_TWIPS, sign_method_text,
};
use crate::models::DraftInput;
use crate::units::UnitDisplay;

/// 表格内容的行距（缇）：21 pt，与公文表格同。
const ROW_LINE_TWIPS: i32 = 420;
/// 签收内容行的最小行高（缇）：约 16 mm，给手写签名与日期留足空间。
const RECEIPT_ROW_HEIGHT_TWIPS: f32 = 907.0;
/// 要素行的固定行高（缇）：21 pt。
const FIELD_LINE_TWIPS: i32 = 420;
/// 题名行的固定行高（缇）：35 pt，二号字不至于贴住上一行。
const TITLE_LINE_TWIPS: i32 = 700;
/// 题名与要素行之间的空行高度（缇）。
const TITLE_GAP_TWIPS: i32 = 240;
/// 要素行与表格之间、表格与送文信息之间的空行高度（缇）。
const BLOCK_GAP_TWIPS: i32 = 160;
/// 表格左右内边距（缇）：1 mm。
const CELL_MARGIN_TWIPS: usize = 57;
/// 外框线宽（八分之一磅）：1.5 pt。
const OUTER_BORDER_SIZE: usize = 12;
/// 内部线宽（八分之一磅）：0.5 pt。
const INNER_BORDER_SIZE: usize = 4;

/// 排一份《公文签收记录单》Word 写到 `path`。
pub(crate) fn write(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
) -> Result<()> {
    let doc = crate::export::typst::receipt::build(input, markdown, display);
    let widths = column_widths();

    let mut rows: Vec<TableRow> = Vec::new();
    // 表头：黑体四号居中，跨页时重复（重复标记在打包前补，见 `with_repeat_header`）。
    let header: Vec<TableCell> = COLUMN_NAMES
        .iter()
        .enumerate()
        .map(|(index, name)| cell(name, widths[index], true, AlignmentType::Center))
        .collect();
    rows.push(TableRow::new(header).cant_split());
    // 内容行：仿宋四号；签收日期与签收人留白，密级随该份公文。
    for row in &doc.rows {
        let cells = vec![
            cell(&row.index, widths[0], false, AlignmentType::Center),
            cell(&row.unit, widths[1], false, AlignmentType::Left),
            cell(&sign_method_text(), widths[2], false, AlignmentType::Center),
            cell("", widths[3], false, AlignmentType::Center),
            cell("", widths[4], false, AlignmentType::Center),
            cell(
                row.security.as_deref().unwrap_or(""),
                widths[5],
                false,
                AlignmentType::Center,
            ),
        ];
        rows.push(
            TableRow::new(cells)
                .row_height(RECEIPT_ROW_HEIGHT_TWIPS)
                .height_rule(HeightRule::AtLeast)
                .cant_split(),
        );
    }

    let borders = TableBorders::new()
        .clear_all()
        .set(
            TableBorder::new(TableBorderPosition::Top)
                .size(OUTER_BORDER_SIZE)
                .color("000000"),
        )
        .set(
            TableBorder::new(TableBorderPosition::Bottom)
                .size(OUTER_BORDER_SIZE)
                .color("000000"),
        )
        .set(
            TableBorder::new(TableBorderPosition::Left)
                .size(OUTER_BORDER_SIZE)
                .color("000000"),
        )
        .set(
            TableBorder::new(TableBorderPosition::Right)
                .size(OUTER_BORDER_SIZE)
                .color("000000"),
        )
        .set(
            TableBorder::new(TableBorderPosition::InsideH)
                .size(INNER_BORDER_SIZE)
                .color("000000"),
        )
        .set(
            TableBorder::new(TableBorderPosition::InsideV)
                .size(INNER_BORDER_SIZE)
                .color("000000"),
        );
    let table = Table::new(rows)
        .set_grid(widths.clone())
        .width(CONTENT_WIDTH_TWIPS, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .margins(TableCellMargins::new().margin(
            CELL_MARGIN_TWIPS,
            CELL_MARGIN_TWIPS,
            CELL_MARGIN_TWIPS,
            CELL_MARGIN_TWIPS,
        ))
        .set_borders(borders);

    let mut docx = Docx::new()
        .page_size(11906, 16838)
        .page_margin(PageMargin {
            top: 2098,
            bottom: BODY_BOTTOM_MARGIN_TWIPS,
            left: 1587,
            right: 1474,
            header: 567,
            footer: FOOTER_DISTANCE_TWIPS,
            gutter: 0,
        })
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(TABLE_SIZE)
        .default_line_spacing(
            LineSpacing::new()
                .line(ROW_LINE_TWIPS)
                .line_rule(LineSpacingType::Exact),
        )
        // 签收单每页都印页码，所以不用 `add_official_page_footers`（它会清空首页页脚）。
        .footer(super::header::page_number_footer(AlignmentType::Center));

    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(title_run(&doc.title, TITLE_SIZE))
            .align(AlignmentType::Center)
            .line_spacing(
                LineSpacing::new()
                    .line(TITLE_LINE_TWIPS)
                    .line_rule(LineSpacingType::Exact),
            ),
    );
    docx = docx.add_paragraph(spacer(TITLE_GAP_TWIPS));
    for row in &doc.fields {
        docx = docx.add_paragraph(field_line(row));
    }
    docx = docx.add_paragraph(spacer(BLOCK_GAP_TWIPS));
    docx = docx.add_table(table);
    docx = docx.add_paragraph(spacer(BLOCK_GAP_TWIPS));
    for row in &doc.footer {
        docx = docx.add_paragraph(field_line(row));
    }

    let file =
        File::create(path).with_context(|| format!("无法创建 Word 文件：{}", path.display()))?;
    let mut xml = docx.build();
    xml.document = with_repeat_header(xml.document);
    xml.pack(file).context("写入 DOCX 包失败")?;
    Ok(())
}

/// 签收表各列宽（缇）：毫米换算后合计正好一个版心（156mm），余量给最后一列。
fn column_widths() -> Vec<usize> {
    let mut widths: Vec<usize> = COLUMN_WIDTHS_MM
        .iter()
        .map(|mm| (mm * 1440.0 / 25.4).round() as usize)
        .collect();
    let sum: usize = widths.iter().sum();
    if let Some(last) = widths.last_mut() {
        *last += CONTENT_WIDTH_TWIPS.saturating_sub(sum);
    }
    widths
}

/// 要素行：标签黑体四号，取值仿宋四号；取值为空就留出手写空间。
fn field_line(fields: &[crate::export::typst::receipt::ReceiptField]) -> Paragraph {
    let mut paragraph = Paragraph::new();
    for (index, field) in fields.iter().enumerate() {
        paragraph = paragraph.add_run(table_run_sized(&field.label, true, false, TABLE_SIZE));
        if field.value.is_empty() {
            paragraph = paragraph.add_run(blank_run(field.blank_chars));
        } else {
            paragraph = paragraph.add_run(table_run_sized(&field.value, false, false, TABLE_SIZE));
        }
        if index + 1 < fields.len() {
            paragraph = paragraph.add_run(table_run_sized("　", false, false, TABLE_SIZE));
        }
    }
    paragraph.align(AlignmentType::Left).line_spacing(
        LineSpacing::new()
            .line(FIELD_LINE_TWIPS)
            .line_rule(LineSpacingType::Exact),
    )
}

/// 手写空间：带下划线的全角空格（PDF 侧是一条等长的横线，两端一样长）。
fn blank_run(chars: usize) -> Run {
    Run::new()
        .add_text("　".repeat(chars))
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(TABLE_SIZE)
        .underline("single")
}

/// 表格单元格：单行四号字，垂直居中。
fn cell(text: &str, width: usize, header: bool, align: AlignmentType) -> TableCell {
    TableCell::new()
        .add_paragraph(
            Paragraph::new()
                .add_run(table_run_sized(text, header, false, TABLE_SIZE))
                .align(align)
                .line_spacing(
                    LineSpacing::new()
                        .line(ROW_LINE_TWIPS)
                        .line_rule(LineSpacingType::Exact),
                ),
        )
        .width(width, WidthType::Dxa)
        .vertical_align(VAlignType::Center)
}

/// 空行：只用来撑开块之间的距离。
fn spacer(twips: i32) -> Paragraph {
    Paragraph::new().add_run(Run::new().size(2)).line_spacing(
        LineSpacing::new()
            .line(twips)
            .line_rule(LineSpacingType::Exact),
    )
}

/// 给表格首行补上 `w:tblHeader`（跨页重复表头）。
///
/// docx-rs 没有这个属性，打包前在 XML 上补：首行的 `w:trPr` 若自闭合就展开，
/// 否则把标记插进已有的 `w:trPr` 里。文档里没有表格时原样返回。
pub(crate) fn with_repeat_header(document: Vec<u8>) -> Vec<u8> {
    let mut xml = match String::from_utf8(document) {
        Ok(xml) => xml,
        Err(err) => return err.into_bytes(),
    };
    let Some(row) = xml.find("<w:tr>") else {
        return xml.into_bytes();
    };
    let rest = &xml[row..];
    if let Some(pos) = rest.find("<w:trPr />") {
        let at = row + pos;
        xml.replace_range(
            at..at + "<w:trPr />".len(),
            "<w:trPr><w:tblHeader /></w:trPr>",
        );
    } else if let Some(pos) = rest.find("<w:trPr>") {
        let at = row + pos + "<w:trPr>".len();
        xml.insert_str(at, "<w:tblHeader />");
    } else {
        xml.insert_str(row + "<w:tr>".len(), "<w:trPr><w:tblHeader /></w:trPr>");
    }
    xml.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TemplateKind, TemplateProfile};

    fn letter() -> DraftInput {
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            profile: TemplateProfile::for_kind(TemplateKind::OfficialLetter),
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.department_code = "某民函".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "18".into();
        input.profile.recipient = "甲单位、乙单位".into();
        input.profile.copies_to = "丙单位、甲单位".into();
        input.date = "2026年9月30日".into();
        input
    }

    /// 读出 docx 包里的 `word/document.xml`。
    fn document_xml(path: &Path) -> String {
        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name("word/document.xml").unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut entry, &mut xml).unwrap();
        xml
    }

    fn write_receipt(dir: &Path) -> String {
        let path = dir.join("receipt.docx");
        write(
            &path,
            &letter(),
            "# 关于商洽的函\n\n正文。",
            &UnitDisplay::new(&[]),
        )
        .unwrap();
        document_xml(&path)
    }

    #[test]
    fn receipt_docx_has_title_elements_and_signing_units() {
        let dir = tempfile::tempdir().unwrap();
        let xml = write_receipt(dir.path());
        assert!(xml.contains("公文签收记录单"), "缺题名");
        assert!(xml.contains("文件名称"), "缺要素行");
        assert!(xml.contains("某民函〔2026〕18号"), "缺函号");
        assert!(xml.contains("某某市民政局"), "缺发函单位");
        assert!(xml.contains("2026年9月30日"), "缺成文日期");
        // 表头六列与签收单位按主送在前、抄送在后的顺序出现。
        for name in COLUMN_NAMES {
            assert!(xml.contains(name), "缺表头 {name}");
        }
        let positions: Vec<usize> = ["甲单位", "乙单位", "丙单位"]
            .iter()
            .map(|unit| {
                xml.find(unit)
                    .unwrap_or_else(|| panic!("缺签收单位 {unit}"))
            })
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        // 签收方式给出可勾选项，日期与签收人留白。
        assert!(xml.contains("□电子 □纸质"), "缺签收方式");
    }

    #[test]
    fn receipt_docx_repeats_the_header_row_and_keeps_rows_whole() {
        let dir = tempfile::tempdir().unwrap();
        let xml = write_receipt(dir.path());
        assert!(xml.contains("<w:tblHeader />"), "首行没有重复表头标记");
        assert!(xml.contains("<w:cantSplit />"), "行没有设不拆分");
        assert!(
            xml.contains("w:val=\"907\" w:hRule=\"atLeast\""),
            "签收行应保留约 16mm 的最小手写高度"
        );
        // 重复表头只加在首行上。
        assert_eq!(xml.matches("<w:tblHeader />").count(), 1);
    }

    #[test]
    fn column_widths_add_up_to_the_content_width() {
        let widths = column_widths();
        assert_eq!(widths.len(), COLUMN_NAMES.len());
        assert_eq!(widths.iter().sum::<usize>(), CONTENT_WIDTH_TWIPS);
    }

    #[test]
    fn with_repeat_header_leaves_documents_without_tables_alone() {
        let xml = b"<w:document><w:body><w:p /></w:body></w:document>".to_vec();
        assert_eq!(with_repeat_header(xml.clone()), xml);
    }
}
