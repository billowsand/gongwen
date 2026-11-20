//! 通讯录：从标准词库出 Excel，办公电话与手机分两列。
//!
//! 只列至少有一个号码的人员；单位用规范全称，顺序沿用词库树（单位、人员
//! 的层级顺序），方便直接打印或再排序。PDF 等排版层接入 `Render` 后再补。

use crate::models::{VocabularyCategory, VocabularyEntry};
use crate::units::UnitDisplay;
use anyhow::Result;
use rust_xlsxwriter::{Format, FormatAlign, FormatBorder, Workbook};

const HEADERS: [&str; 6] = ["单位", "姓名", "职务", "办公电话", "手机", "备注"];
const WIDTHS: [f64; 6] = [34.0, 14.0, 14.0, 18.0, 18.0, 40.0];

pub(crate) fn xlsx_bytes(vocabulary: &[VocabularyEntry]) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet.set_name("通讯录")?;
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
    let mut row = 1u32;
    for entry in vocabulary
        .iter()
        .filter(|entry| entry.category == VocabularyCategory::Person)
    {
        let office = entry.office_phone.trim();
        let mobile = entry.phone.trim();
        if office.is_empty() && mobile.is_empty() {
            continue;
        }
        let unit = if entry.unit.trim().is_empty() {
            "（未归属）".to_string()
        } else {
            display.full_name(&entry.unit)
        };
        sheet.write_string(row, 0, unit)?;
        sheet.write_string(row, 1, entry.canonical.trim())?;
        sheet.write_string(row, 2, entry.position.trim())?;
        sheet.write_string(row, 3, office)?;
        sheet.write_string(row, 4, mobile)?;
        sheet.write_string(row, 5, entry.note.trim())?;
        row += 1;
    }
    Ok(workbook.save_to_buffer()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{VocabularyCategory, VocabularyEntry};
    use calamine::Reader;

    fn person(unit: &str, name: &str, office: &str, mobile: &str) -> VocabularyEntry {
        VocabularyEntry {
            category: VocabularyCategory::Person,
            unit: unit.into(),
            canonical: name.into(),
            office_phone: office.into(),
            phone: mobile.into(),
            ..Default::default()
        }
    }

    #[test]
    fn contacts_keep_office_and_mobile_columns_and_skip_missing_numbers() {
        let mut unit = VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "甲单位".into(),
            code: "00".into(),
            ..Default::default()
        };
        unit.id = 1;
        let with_number = person("00", "张三", "010-1234", "13812345678");
        let without = person("00", "李四", "", "");
        let bytes = xlsx_bytes(&[unit, with_number, without]).unwrap();
        let mut workbook = calamine::Xlsx::new(std::io::Cursor::new(bytes)).unwrap();
        let range = workbook.worksheet_range("通讯录").unwrap();
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
            ["单位", "姓名", "职务", "办公电话", "手机", "备注"]
        );
        assert_eq!(rows.len(), 2, "没有号码的人不写进通讯录");
        assert_eq!(rows[1][0], "甲单位");
        assert_eq!(rows[1][1], "张三");
        assert_eq!(rows[1][3], "010-1234");
        assert_eq!(rows[1][4], "13812345678");
    }
}
