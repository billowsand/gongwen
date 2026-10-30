//! 版记：三线表记录、印发份数与联合发文版记。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::record`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::docx::{
    FOOTER_SIZE, RECORD_OTHER_COLUMN_TWIPS, RECORD_PHONE_COLUMN_TWIPS, TABLE_CONTENT_WIDTH_TWIPS,
    chinese_fonts, docx_name, is_joint_mode_one, marked_runs, record_run,
};
use crate::models::{DraftInput, split_units};
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use docx_rs::*;

pub(crate) fn automatic_print_copies(input: &DraftInput) -> usize {
    if input.kind == crate::models::TemplateKind::MeetingMinutes {
        return minutes_recipients(input)
            .iter()
            .map(|(_, value)| split_units(value).len())
            .sum();
    }
    let responsible = if is_joint_mode_one(input) {
        &input.profile.joint_responsible_units
    } else {
        &input.profile.responsible_unit
    };
    split_units(&input.profile.recipient).len()
        + split_units(&input.profile.copies_to).len()
        + split_units(responsible).len()
}

/// 会议纪要每人每单位各印一份，依次是出席、列席、分送、抄送；份号也按这个顺序编。
/// 请假的人不在会上，不另印。
pub(crate) fn minutes_recipients(input: &DraftInput) -> [(&'static str, &str); 4] {
    [
        ("出席", input.attendees.as_str()),
        ("列席", input.minutes.nonvoting.as_str()),
        ("分送", input.minutes.distribute.as_str()),
        ("抄送", input.profile.copies_to.as_str()),
    ]
}

/// 规格 §3.2：版记是一个带横线的表格——上、下横线粗（1pt），行间横线细（0.5pt），
/// 上下正好是中间的两倍宽。首行抄送跨三列并右对齐印数，其后每行是“承办单位/联系人/联系电话”。
pub(crate) fn add_footer_record(
    doc: Docx,
    input: &DraftInput,
    display: &UnitDisplay,
    elements: &ElementMarks,
) -> Docx {
    let grid = vec![
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_OTHER_COLUMN_TWIPS,
        RECORD_PHONE_COLUMN_TWIPS,
    ];
    let mut rows = Vec::new();

    // 首行：抄送 + 右对齐印数，跨三列，其上边框即版记上横线。
    // 没有抄送单位时整行只留印数；仅在抄送字段有删除标注时保留“抄送：”行。
    let copies_text = crate::export::element_display::copies_to_display(input, display);
    let mut copies_paragraph = Paragraph::new().add_tab(
        Tab::new()
            .val(TabValueType::Right)
            .pos(TABLE_CONTENT_WIDTH_TWIPS),
    );
    if !copies_text.is_empty() || elements.copies_to().changed() {
        // 抄送变了：旧值删除线、新值加框，「抄送：」标签不进标注；没变原样单 run。
        copies_paragraph =
            copies_paragraph.indent(Some(840), Some(SpecialIndentType::Hanging(840)), None, None);
        match elements.copies_to().change() {
            Some(_) => {
                copies_paragraph = copies_paragraph.add_run(record_run("抄送："));
                for run in marked_runs(&elements.copies_to().marked(), record_run) {
                    copies_paragraph = copies_paragraph.add_run(run);
                }
            }
            None => {
                copies_paragraph =
                    copies_paragraph.add_run(record_run(&format!("抄送：{copies_text}")));
            }
        }
    }
    let copies_paragraph = copies_paragraph
        .keep_next(true)
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
        .clear_all()
        .set(TableBorder::new(TableBorderPosition::Top).size(14))
        .set(TableBorder::new(TableBorderPosition::Bottom).size(14))
        .set(TableBorder::new(TableBorderPosition::InsideH).size(7));
    // 版记贴末页版心底（`add_record_table`）：在流里接在内容之后只能碰巧落到页底，所以做成浮动表，
    // 底边对齐版心底（与红头呈批件承办区同法）。浮动表锚在其后的段落上，锚段
    // 压到最矮；末页剩余放不下版记时 Word 会连锚段一起挪到下一页底部。
    // 与上方内容至少隔 1cm 由 `w:topFromText` 保证，docx-rs 不写这个属性，
    // 打包前由 `with_record_gap` 补上。
    add_record_table(doc, Table::new(rows).set_grid(grid).set_borders(borders))
}

/// 把版记表做成底边贴版心底的浮动表，再接一个压到最矮的锚段。会议纪要的
/// 版记共用这一段。
pub(crate) fn add_record_table(doc: Docx, table: Table) -> Docx {
    doc.add_table(
        table
            .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
            .margins(TableCellMargins::new().margin(0, 0, 0, 0))
            .layout(TableLayoutType::Fixed)
            .position(record_position()),
    )
    .add_paragraph(
        Paragraph::new()
            .add_run(Run::new().size(2))
            .line_spacing(LineSpacing::new().line(1).line_rule(LineSpacingType::Exact)),
    )
}

/// 版记浮动表的定位：水平居版心，垂直底边贴版心底。
fn record_position() -> TablePositionProperty {
    TablePositionProperty::new()
        .horizontal_anchor("margin")
        .vertical_anchor("margin")
        .position_x_alignment("center")
        .position_y_alignment("bottom")
        .left_from_text(0)
        .right_from_text(0)
}

/// 版记与上方内容的最小间距：1cm。
const RECORD_GAP_TWIPS: i32 = 567;

/// 给版记浮动表补上 `w:topFromText`（docx-rs 的 `tblpPr` 不支持该属性）。
/// 只认 `record_position` 写出的那一组属性，文档里没有版记时原样返回。
pub(crate) fn with_record_gap(document: Vec<u8>) -> Vec<u8> {
    let xml = match String::from_utf8(document) {
        Ok(xml) => xml,
        Err(err) => return err.into_bytes(),
    };
    let marker = r#"<w:tblpPr w:leftFromText="0" w:rightFromText="0" w:vertAnchor="margin" w:horzAnchor="margin" w:tblpXSpec="center" w:tblpYSpec="bottom""#;
    let patched = format!(
        r#"<w:tblpPr w:leftFromText="0" w:rightFromText="0" w:topFromText="{RECORD_GAP_TWIPS}" w:vertAnchor="margin" w:horzAnchor="margin" w:tblpXSpec="center" w:tblpYSpec="bottom""#
    );
    xml.replacen(marker, &patched, 1).into_bytes()
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
    let (contact_text, contact_size) = docx_name(
        &display.person_name(&input.profile.contact_person),
        FOOTER_SIZE,
    );
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
