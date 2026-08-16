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
