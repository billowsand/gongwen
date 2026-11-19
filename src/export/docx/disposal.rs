//! 文件及光盘销毁审批表。两份表共用同一套 Word 表格版式。

use super::{TABLE_SIZE, TITLE_SIZE, chinese_fonts, table_run_sized, title_run};
use crate::manuscript::disposal::{FormKind, FormRow};
use anyhow::{Context, Result};
use docx_rs::*;
use std::io::Cursor;

const PAGE_WIDTH: u32 = 11906;
const PAGE_MARGIN: i32 = 1143;
const FILE_COLUMN_WIDTHS: [usize; 5] = [900, 2780, 4000, 1020, 920];
const DISC_COLUMN_WIDTHS: [usize; 5] = [900, 1840, 4940, 1020, 920];

pub(crate) fn build_disposal_docx(kind: FormKind, entries: &[FormRow]) -> Result<Vec<u8>> {
    let widths = match kind {
        FormKind::File => FILE_COLUMN_WIDTHS,
        FormKind::Disc => DISC_COLUMN_WIDTHS,
    };
    let mut rows = Vec::with_capacity(entries.len() + 1);
    rows.push(form_row(kind.columns(), widths, true));
    for (index, entry) in entries.iter().enumerate() {
        rows.push(form_row(
            [
                &(index + 1).to_string(),
                entry.number.trim(),
                entry.title.trim(),
                entry.security.trim(),
                &entry.copies.to_string(),
            ],
            widths,
            false,
        ));
    }

    let table = Table::new(rows)
        .set_grid(widths.to_vec())
        .width(widths.iter().sum(), WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .margins(TableCellMargins::new().margin(70, 100, 70, 100));
    let title = Paragraph::new()
        .style("Title")
        .add_run(title_run(kind.title(), TITLE_SIZE))
        .align(AlignmentType::Center)
        .keep_next(true)
        .line_spacing(
            LineSpacing::new()
                .after(300)
                .line(620)
                .line_rule(LineSpacingType::Exact),
        );
    let mut docx = Docx::new()
        .page_size(PAGE_WIDTH, 16838)
        .page_margin(PageMargin {
            top: 1420,
            bottom: 1420,
            left: PAGE_MARGIN,
            right: PAGE_MARGIN,
            header: 567,
            footer: 567,
            gutter: 0,
        })
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(TABLE_SIZE)
        .add_style(Style::new("Title", StyleType::Paragraph).name("Title"))
        .footer(super::header::page_number_footer(AlignmentType::Center))
        .add_paragraph(title)
        .add_table(table)
        .build();
    docx.document = super::receipt::with_repeat_header(docx.document);
    let mut bytes = Cursor::new(Vec::new());
    docx.pack(&mut bytes).context("生成审批表 DOCX 失败")?;
    Ok(bytes.into_inner())
}

fn form_row(values: [&str; 5], widths: [usize; 5], header: bool) -> TableRow {
    let cells = values
        .into_iter()
        .zip(widths)
        .enumerate()
        .map(|(column, (value, width))| {
            let align = if column == 2 && !header {
                AlignmentType::Left
            } else {
                AlignmentType::Center
            };
            let paragraph = Paragraph::new()
                .add_run(table_run_sized(value, header, false, TABLE_SIZE))
                .align(align)
                .line_spacing(
                    LineSpacing::new()
                        .line(420)
                        .line_rule(LineSpacingType::Exact),
                );
            TableCell::new()
                .width(width, WidthType::Dxa)
                .vertical_align(VAlignType::Center)
                .add_paragraph(paragraph)
        })
        .collect();
    TableRow::new(cells)
        .row_height(540.0)
        .height_rule(HeightRule::AtLeast)
        .cant_split()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn disposal_docx_uses_requested_columns_and_repeats_header() {
        for kind in FormKind::ALL {
            let rows = [FormRow {
                number: if kind == FormKind::Disc {
                    "GCB-0027"
                } else {
                    "某办函〔2026〕12号"
                }
                .into(),
                title: "关于召开工作会议并做好材料准备和有关事项的通知".into(),
                security: "秘密".into(),
                copies: if kind == FormKind::Disc { 1 } else { 4 },
            }];
            let bytes = build_disposal_docx(kind, &rows).unwrap();
            let mut docx = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut xml = String::new();
            docx.by_name("word/document.xml")
                .unwrap()
                .read_to_string(&mut xml)
                .unwrap();
            for text in kind.columns().into_iter().chain([
                kind.title(),
                &rows[0].number,
                &rows[0].title,
                "秘密",
            ]) {
                assert!(xml.contains(&format!(">{text}<")), "缺少 {text}");
            }
            assert_eq!(xml.matches("<w:tblHeader").count(), 1);
            assert_eq!(xml.matches("<w:tr>").count(), 2);
            assert_eq!(xml.matches("<w:tc>").count(), 10);
            assert!(xml.contains(&format!(">{}<", rows[0].copies)));
        }
    }

    #[test]
    #[ignore = "生成供人工目视检查的多页 Word 样张"]
    fn disposal_docx_samples() {
        let output = std::env::var_os("GONGWEN_DISPOSAL_SAMPLE_DIR").expect("请指定样张目录");
        for kind in FormKind::ALL {
            let rows: Vec<_> = (1..=34)
                .map(|index| FormRow {
                    number: if kind == FormKind::Disc {
                        format!("GCB-{index:04}")
                    } else {
                        format!("某办函〔2026〕{index}号")
                    },
                    title: if index % 4 == 0 {
                        "关于印发年度重点工作方案并做好跨部门协同、会议组织和材料归档等有关事项的通知".into()
                    } else {
                        format!("第{index}项重点工作通知")
                    },
                    security: "机密".into(),
                    copies: if kind == FormKind::Disc { 1 } else { 4 },
                })
                .collect();
            let path = std::path::Path::new(&output).join(format!("{}.docx", kind.title()));
            std::fs::write(path, build_disposal_docx(kind, &rows).unwrap()).unwrap();
        }
    }
}
