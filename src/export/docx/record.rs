//! 版记：三线表记录、印发份数与联合发文版记。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::record`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{DraftInput, split_units};
use crate::units::{UnitDisplay};
use crate::export::docx::{FOOTER_SIZE, RECORD_PHONE_COLUMN_TWIPS, RECORD_OTHER_COLUMN_TWIPS, TABLE_CONTENT_WIDTH_TWIPS, chinese_fonts, record_run, docx_name, is_joint_mode_one};
use docx_rs::*;

pub(crate) fn automatic_print_copies(input: &DraftInput) -> usize {
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
pub(crate) fn add_footer_record(doc: Docx, input: &DraftInput, display: &UnitDisplay) -> Docx {
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
pub(crate) fn record_cell_sized(
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

pub(crate) fn record_cell(text: &str, width: usize, alignment: AlignmentType) -> TableCell {
    record_cell_sized(text, width, alignment, FOOTER_SIZE, 0)
}

/// 单独发文的版记行：承办单位（简称）/ 联系人 / 联系电话。
pub(crate) fn single_record_row(input: &DraftInput, display: &UnitDisplay) -> TableRow {
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
pub(crate) fn joint_record_rows(input: &DraftInput, display: &UnitDisplay) -> Vec<TableRow> {
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
