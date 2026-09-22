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
        .clear_all()
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
                .line(super::BODY_LINE_TWIPS as i32)
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
            // 正文绕排时与批示框左沿保持 4mm，否则每行末字都贴着红色竖线。
            // 与 TeX 的 \RedApprovalNarrowWidth、预览的窄栏同源。
            .left_from_text(crate::export::RED_APPROVAL_GUTTER_TWIPS as i32)
            .right_from_text(0),
    )
}

/// 首页红色横线要贯穿整个版心。批示栏自身只占右侧 5.6cm，另放一条
/// 极薄的浮动表格补足左栏横线，避免让 Word 把正文按整页宽度绕排。
///
/// 宽度只取到红色竖线（版心左缘往右 100mm），**不能**铺满整个版心：铺满时它与
/// 批示框在同一坐标上重叠，Word 为避免两个浮动表相压会把其中一个另挪位置，
/// 横线就跑到文号上方去了。两张表左右相接、互不重叠，才会都停在 48mm 处。
///
/// 行高取 20 缇而非理论上的 1 缇：Word 会把 1 缇高的浮动表当作不可见对象
/// 丢弃（左段横线整段消失，只剩批示框的右段），20 缇（0.35mm）即可稳定渲染，
/// 且远低于标题顶沿，不参与绕排、不影响版面。
pub(crate) fn red_approval_top_rule_table() -> Table {
    let borders = TableBorders::new().clear_all().set(
        TableBorder::new(TableBorderPosition::Top)
            .size(12)
            .color("FF0000"),
    );
    let width = crate::export::RED_APPROVAL_RULE_TWIPS;
    Table::new(vec![
        TableRow::new(vec![
            TableCell::new()
                .width(width, WidthType::Dxa)
                .add_paragraph(Paragraph::new()),
        ])
        .row_height(20.0)
        .height_rule(HeightRule::Exact),
    ])
    .set_grid(vec![width])
    .width(width, WidthType::Dxa)
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
    let borders = TableBorders::new().clear_all().set(
        TableBorder::new(TableBorderPosition::Top)
            .size(12)
            .color("FF0000"),
    );
    Table::new(rows)
        .set_grid(vec![columns.unit, columns.contact, columns.phone])
        .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .margins(TableCellMargins::new().margin(0, 0, 0, 0))
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
