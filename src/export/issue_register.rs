//! 发文登记簿：把台账条目导出成 Excel，供存档、打印或再统计。

use crate::manuscript::ledger::IssueEntry;
use crate::models::VocabularyEntry;
use crate::platform::doc::NumberingScheme;
use crate::units::UnitDisplay;
use anyhow::Result;
use rust_xlsxwriter::{Format, FormatAlign, FormatBorder, Workbook};

const HEADERS: [&str; 10] = [
    "年度", "类型", "编号", "单位", "标题", "日期", "密级", "状态", "来源", "备注",
];
const WIDTHS: [f64; 10] = [8.0, 12.0, 24.0, 30.0, 44.0, 14.0, 8.0, 8.0, 8.0, 30.0];

fn scheme_label(entry: &IssueEntry) -> &'static str {
    match NumberingScheme::from_str(&entry.scheme) {
        Some(NumberingScheme::Letter) => "公函",
        Some(NumberingScheme::Approval) => "红头呈批件",
        Some(NumberingScheme::Minutes) => "会议纪要",
        None => "",
    }
}

pub(crate) fn xlsx_bytes(
    entries: &[IssueEntry],
    vocabulary: &[VocabularyEntry],
) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet.set_name("发文登记簿")?;
    let header_format = Format::new()
        .set_bold()
        .set_background_color("#DCE6F1")
        .set_align(FormatAlign::Center)
        .set_border(FormatBorder::Thin);
    for (column, header) in HEADERS.iter().enumerate() {
        sheet.write_string_with_format(0, column as u16, *header, &header_format)?;
        sheet.set_column_width(column as u16, WIDTHS[column])?;
    }
    sheet.set_freeze_panes(1, 0)?;

    let display = UnitDisplay::new(vocabulary);
    for (offset, entry) in entries.iter().enumerate() {
        let row = 1 + offset as u32;
        let unit = if entry.unit.trim().is_empty() {
            "未登记单位".to_string()
        } else {
            display.full_name(&entry.unit)
        };
        let source = if entry.manuscript_id.is_some() {
            "稿件"
        } else {
            "手工"
        };
        sheet.write_string(row, 0, entry.year.trim())?;
        sheet.write_string(row, 1, scheme_label(entry))?;
        sheet.write_string(row, 2, entry.number_label())?;
        sheet.write_string(row, 3, unit)?;
        sheet.write_string(row, 4, entry.title.trim())?;
        sheet.write_string(row, 5, entry.doc_date.trim())?;
        sheet.write_string(row, 6, entry.security.trim())?;
        sheet.write_string(row, 7, entry.status_label())?;
        sheet.write_string(row, 8, source)?;
        sheet.write_string(row, 9, entry.note.trim())?;
    }
    Ok(workbook.save_to_buffer()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{VocabularyCategory, VocabularyEntry};
    use calamine::Reader;

    fn entry(serial: &str, code: &str) -> IssueEntry {
        IssueEntry {
            id: 1,
            manuscript_id: Some(1),
            unit: "00".into(),
            scheme: "letter".into(),
            code: code.into(),
            year: "2026".into(),
            serial: serial.into(),
            title: "关于某事的通知".into(),
            kind: "OfficialLetter".into(),
            doc_date: "2026-09-30".into(),
            security: "内部".into(),
            status: "used".into(),
            note: String::new(),
        }
    }

    #[test]
    fn export_keeps_pool_columns_and_number_label() {
        let unit = VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "甲单位".into(),
            code: "00".into(),
            ..Default::default()
        };
        let bytes = xlsx_bytes(&[entry("12", "某函")], &[unit]).unwrap();
        let mut workbook = calamine::Xlsx::new(std::io::Cursor::new(bytes)).unwrap();
        let range = workbook.worksheet_range("发文登记簿").unwrap();
        let rows = range
            .rows()
            .map(|row| {
                row.iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows[0],
            [
                "年度", "类型", "编号", "单位", "标题", "日期", "密级", "状态", "来源", "备注"
            ]
        );
        assert_eq!(rows[1][0], "2026");
        assert_eq!(rows[1][1], "公函");
        assert_eq!(rows[1][2], "某函〔2026〕12号");
        assert_eq!(rows[1][3], "甲单位");
        assert_eq!(rows[1][8], "稿件");
    }
}
