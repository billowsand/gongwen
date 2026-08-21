//! 红头呈批件：红头框线表、顶线表与红头版记表。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::red`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::docx::{
    BODY_SIZE, TABLE_CONTENT_WIDTH_TWIPS, chinese_fonts, docx_name, red_record_paragraph,
};
use crate::models::DraftInput;
use crate::units::UnitDisplay;
use docx_rs::*;

pub(crate) fn red_approval_frame_table(input: &DraftInput) -> Table {
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
pub(crate) fn red_approval_top_rule_table() -> Table {
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

pub(crate) fn red_approval_record_table(input: &DraftInput, display: &UnitDisplay) -> Table {
    let entries = crate::models::joint_responsible_entries(&input.profile);
    let fallback = crate::models::JointResponsibleEntry::default();
    let entries = if entries.is_empty() {
        std::slice::from_ref(&fallback)
    } else {
        entries.as_slice()
    };
    // 三栏宽度按各行实际内容一次算定（与 LaTeX/预览同源）：联系人栏固定 8 em
    // 永不压缩，电话栏按最长号码定宽，承办单位栏吃版心余量、超宽才压缩。
    // 多条承办条目时标签只出现在首行，续行取值缩到首行取值位置保持上下对齐。
    let rows = entries
        .iter()
        .map(|entry| {
            let (name, name_size) = docx_name(&entry.name, BODY_SIZE);
            (
                [display.abbr(&entry.unit), name, entry.phone.clone()],
                name_size,
            )
        })
        .collect::<Vec<_>>();
    let display_rows = rows.iter().map(|(row, _)| row.clone()).collect::<Vec<_>>();
    let columns = crate::export::red_record_columns(&display_rows);
    let rows = rows
        .iter()
        .enumerate()
        .map(|(index, (row, name_size))| {
            let first = index == 0;
            TableRow::new(vec![
                TableCell::new()
                    .width(columns.unit, WidthType::Dxa)
                    .add_paragraph(red_record_paragraph(
                        if first { "承办单位：" } else { "" },
                        &row[0],
                        BODY_SIZE,
                        crate::export::red_record_unit_twips(&row[0]),
                        AlignmentType::Left,
                        columns.unit_usable(),
                        if first {
                            0
                        } else {
                            crate::export::RED_RECORD_LABEL_UNIT_TWIPS
                        },
                    )),
                TableCell::new()
                    .width(columns.contact, WidthType::Dxa)
                    .add_paragraph(red_record_paragraph(
                        if first { "联系人：" } else { "" },
                        &row[1],
                        *name_size,
                        crate::export::RED_RECORD_LABEL_CONTACT_TWIPS
                            + crate::export::red_record_name_twips(&entries[index].name),
                        AlignmentType::Left,
                        columns.contact_usable(),
                        if first {
                            0
                        } else {
                            crate::export::RED_RECORD_LABEL_CONTACT_TWIPS
                        },
                    )),
                TableCell::new()
                    .width(columns.phone, WidthType::Dxa)
                    .add_paragraph(red_record_paragraph(
                        if first { "电话：" } else { "" },
                        &row[2],
                        BODY_SIZE,
                        crate::export::red_record_phone_twips(&row[2], first),
                        AlignmentType::Right,
                        columns.phone_usable(),
                        0,
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
        .set_grid(vec![columns.unit, columns.contact, columns.phone])
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
