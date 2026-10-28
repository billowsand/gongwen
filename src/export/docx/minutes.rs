//! 会议纪要的 Word 部件：版头（份号、密级、纪要标志、编号与红线）、名单与版记。
//!
//! 版式与 PDF（`assets/typst/gongwen.typ` 的 `minutes-*`）一一对应，规格见
//! `docs/meeting-minutes-design.md` 第二节；正文段落走公文正文链，只多一道「一是」楷体。

use crate::export::docx::runs::apply_redline;
use crate::export::docx::{
    BoldFont, FOOTER_EM_TWIPS, HEADER_WIDTH_TWIPS, SECURITY_FIRST_LINE_TWIPS,
    TABLE_CONTENT_WIDTH_TWIPS, apply_bold, body_line_twips, body_run, body_size, chinese_fonts,
    header, letter_security_paragraph, marked_runs, paren_size, record_run,
};
use crate::export::minutes::{
    RosterLine, enumeration_kai_spans, kai_pieces, minutes_heading, minutes_number_text, roster,
};
use crate::export::{inline_segments, redline_chunks};
use crate::models::{DraftInput, split_units};
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use docx_rs::*;

/// 纪要标志：方正小标宋 26pt（半磅 52）。
const HEADING_SIZE: usize = 52;
/// 纪要标志的行高。字号比正文网格的一行高，固定行高给足 34pt 免得 Word 裁掉字顶，
/// 多出来的 103 缇从它上面那个空行里扣回，后面的行仍落在网格上。
const HEADING_LINE_TWIPS: u32 = 680;
/// 编号行到红线：编号放在一格无框表里，红线是表的下边框；单元格下边距把线
/// 推到编号基线下约 4mm，同样从后面的空行里扣回。
const NUMBER_RULE_GAP_TWIPS: u32 = 113;
/// 名单左空二字、标签三字。
const ROSTER_INDENT_TWIPS: usize = 640;
const ROSTER_LABEL_TWIPS: usize = 960;
const ROSTER_COLUMNS: usize = 6;

fn exact_line(twips: u32) -> LineSpacing {
    LineSpacing::new()
        .line(twips as i32)
        .line_rule(LineSpacingType::Exact)
}

fn blank(twips: u32) -> Paragraph {
    Paragraph::new()
        .line_spacing(exact_line(twips))
        .keep_next(true)
}

/// 版头：份号、密级（版心第 1、2 行），空一行，纪要标志紧接编号与红线，空一行。
pub(crate) fn add_header(
    mut doc: Docx,
    input: &DraftInput,
    display: &UnitDisplay,
    elements: &ElementMarks,
) -> Docx {
    // 份号在版心第一行，行高同白头件密级的首行等效值；Word 版只出一份，固定 01。
    doc = doc.add_paragraph(
        Paragraph::new()
            .add_run(
                Run::new()
                    .add_text("01")
                    .fonts(chinese_fonts("黑体"))
                    .size(body_size()),
            )
            .line_spacing(exact_line(SECURITY_FIRST_LINE_TWIPS))
            .keep_next(true),
    );
    if !input.profile.security_level.trim().is_empty() || elements.security().changed() {
        doc = doc
            .add_paragraph(letter_security_paragraph(input, elements.security()).keep_next(true));
    }
    doc = doc.add_paragraph(blank(
        body_line_twips() - (HEADING_LINE_TWIPS - body_line_twips()),
    ));

    let heading = minutes_heading(input, display);
    // 汉字等宽：一个字宽 = 字号（半磅）× 10 缇。排不下就整体横向压缩，字高不变。
    let natural = heading.chars().count() as i32 * HEADING_SIZE as i32 * 10;
    let scale = if natural > HEADER_WIDTH_TWIPS {
        (HEADER_WIDTH_TWIPS * 100 / natural).max(1)
    } else {
        100
    };
    doc = doc.add_paragraph(
        Paragraph::new()
            .add_run(
                Run::new()
                    .add_text(heading)
                    .fonts(chinese_fonts("方正小标宋简体"))
                    .size(HEADING_SIZE)
                    .color("FF0000")
                    .stretch(scale),
            )
            .align(AlignmentType::Center)
            .line_spacing(exact_line(HEADING_LINE_TWIPS))
            .keep_next(true),
    );
    let mut number = Paragraph::new()
        .align(AlignmentType::Center)
        .line_spacing(exact_line(body_line_twips()))
        .keep_next(true);
    let [_, year_mark, serial_mark] = elements.number();
    if year_mark.changed() || serial_mark.changed() {
        for run in marked_runs(&elements.number_marked_line(""), |text| body_run(text)) {
            number = number.add_run(run);
        }
    } else {
        number = number.add_run(body_run(minutes_number_text(input)));
    }
    doc = doc.add_table(
        Table::new(vec![TableRow::new(vec![
            TableCell::new()
                .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
                .add_paragraph(number),
        ])])
        .set_grid(vec![TABLE_CONTENT_WIDTH_TWIPS])
        .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .margins(TableCellMargins::new().margin(0, 0, NUMBER_RULE_GAP_TWIPS as usize, 0))
        // 下边框即红线：12 = 1.5 磅，与函稿红头反线同粗。
        .set_borders(
            TableBorders::new().clear_all().set(
                TableBorder::new(TableBorderPosition::Bottom)
                    .size(12)
                    .color("FF0000"),
            ),
        ),
    );
    doc.add_paragraph(blank(body_line_twips() - NUMBER_RULE_GAP_TWIPS))
}

/// 会议纪要正文段：与 `body_paragraph` 相同，另把「一是……」那一截排成三号楷体。
pub(crate) fn body_paragraph(text: &str, bold: BoldFont<'_>) -> Paragraph {
    let mut paragraph = Paragraph::new()
        .align(AlignmentType::Both)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(exact_line(body_line_twips()))
        .widow_control(false);
    for run in body_runs(text, bold) {
        paragraph = paragraph.add_run(run);
    }
    paragraph
}

/// 切法同 `runs::body_runs`（花脸稿块 → 行内片段），楷体范围按全段可见文字算，
/// 与 PDF 的 `with_enumeration_kai` 同一套判定。
fn body_runs(text: &str, bold: BoldFont<'_>) -> Vec<Run> {
    let pieces: Vec<_> = redline_chunks(text)
        .into_iter()
        .flat_map(|chunk| {
            inline_segments(&chunk.text)
                .into_iter()
                .map(move |segment| (chunk.kind, segment))
        })
        .collect();
    let visible: String = pieces
        .iter()
        .map(|(_, segment)| segment.text.as_str())
        .collect();
    let spans = enumeration_kai_spans(&visible);
    let mut runs = Vec::new();
    let mut offset = 0;
    for (kind, segment) in pieces {
        // 片段内按「楷体 / 非楷体」再切一刀。
        for (text, kai) in kai_pieces(&segment.text, offset, &spans) {
            let mut run = if segment.parenthesized {
                Run::new()
                    .add_text(text)
                    .fonts(chinese_fonts("楷体_GB2312"))
                    .size(paren_size())
            } else if kai {
                Run::new()
                    .add_text(text)
                    .fonts(chinese_fonts("楷体_GB2312"))
                    .size(body_size())
            } else {
                body_run(text)
            };
            if segment.bold {
                run = apply_bold(run, bold);
            }
            runs.push(apply_redline(run, kind));
        }
        offset += segment.text.chars().count();
    }
    if runs.is_empty() {
        return vec![body_run("")];
    }
    runs
}

/// 名单：正文后空一行，出席、列席、请假依次排，空项不出。
pub(crate) fn add_roster(mut doc: Docx, input: &DraftInput) -> Docx {
    let lines = roster(input);
    if lines.is_empty() {
        return doc;
    }
    doc = doc.add_paragraph(header::blank_line().keep_next(true));
    for line in &lines {
        doc = if line.grid {
            doc.add_table(roster_grid(line))
        } else {
            doc.add_paragraph(roster_running(line))
        };
    }
    doc
}

fn label_run(label: &str) -> Run {
    Run::new()
        .add_text(format!("{label}："))
        .fonts(chinese_fonts("黑体"))
        .size(body_size())
}

fn roster_cell(width: usize, run: Option<Run>, align: AlignmentType) -> TableCell {
    let mut paragraph = Paragraph::new()
        .align(align)
        .line_spacing(exact_line(body_line_twips()));
    if let Some(run) = run {
        paragraph = paragraph.add_run(run);
    }
    TableCell::new()
        .width(width, WidthType::Dxa)
        .add_paragraph(paragraph)
}

/// 网格：无框表，左缩进二字，标签一列三字宽，其后 6 格均分；两字名中间空一字。
fn roster_grid(line: &RosterLine) -> Table {
    let cell =
        (TABLE_CONTENT_WIDTH_TWIPS - ROSTER_INDENT_TWIPS - ROSTER_LABEL_TWIPS) / ROSTER_COLUMNS;
    let mut grid = vec![ROSTER_LABEL_TWIPS];
    grid.extend(std::iter::repeat_n(cell, ROSTER_COLUMNS));
    let rows = line
        .names
        .chunks(ROSTER_COLUMNS)
        .enumerate()
        .map(|(index, chunk)| {
            let mut cells = vec![roster_cell(
                ROSTER_LABEL_TWIPS,
                (index == 0).then(|| label_run(line.label)),
                AlignmentType::Left,
            )];
            for column in 0..ROSTER_COLUMNS {
                let name = chunk.get(column).map(|name| body_run(padded_name(name)));
                cells.push(roster_cell(cell, name, AlignmentType::Center));
            }
            TableRow::new(cells).cant_split()
        })
        .collect();
    Table::new(rows)
        .set_grid(grid)
        .width(
            TABLE_CONTENT_WIDTH_TWIPS - ROSTER_INDENT_TWIPS,
            WidthType::Dxa,
        )
        .indent(ROSTER_INDENT_TWIPS as i32)
        .layout(TableLayoutType::Fixed)
        .margins(TableCellMargins::new().margin(0, 0, 0, 0))
        .clear_all_border()
}

/// 两字名中间空一字凑成三字宽（样例写法）。
fn padded_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() == 2 {
        format!("{}\u{3000}{}", chars[0], chars[1])
    } else {
        name.to_string()
    }
}

/// 连排：「出席：甲、乙、丙」，回行与冒号后首字对齐。
fn roster_running(line: &RosterLine) -> Paragraph {
    Paragraph::new()
        .add_run(label_run(line.label))
        .add_run(body_run(line.names.join("、")))
        .indent(
            Some((ROSTER_INDENT_TWIPS + ROSTER_LABEL_TWIPS) as i32),
            Some(SpecialIndentType::Hanging(ROSTER_LABEL_TWIPS as i32)),
            None,
            None,
        )
        .line_spacing(exact_line(body_line_twips()))
        .widow_control(false)
}

fn record_paragraph() -> Paragraph {
    Paragraph::new().line_spacing(exact_line(420))
}

/// 版记：顶粗线、分送、抄送（末行右端印数）、细线、印发机关与印发日期、底粗线。
/// 与函稿版记一样做成底边贴版心底的浮动表（定位与 1cm 间距共用 `record` 的实现）。
pub(crate) fn add_record(
    doc: Docx,
    input: &DraftInput,
    display: &UnitDisplay,
    elements: &ElementMarks,
) -> Docx {
    let hang = 3 * FOOTER_EM_TWIPS as i32;
    let copies_note = format!("（共印{}份）", crate::export::automatic_print_copies(input));
    let mut lines: Vec<Paragraph> = Vec::new();
    let distribute = split_units(&input.minutes.distribute);
    if !distribute.is_empty() {
        lines.push(
            record_paragraph()
                .indent(
                    Some(hang),
                    Some(SpecialIndentType::Hanging(hang)),
                    None,
                    None,
                )
                .add_run(record_run(&format!("分送：{}", distribute.join("、")))),
        );
    }
    let copies_text = crate::export::element_display::copies_to_display(input, display);
    if !copies_text.is_empty() || elements.copies_to().changed() {
        let mut paragraph = record_paragraph()
            .indent(
                Some(hang),
                Some(SpecialIndentType::Hanging(hang)),
                None,
                None,
            )
            .add_run(record_run("抄送："));
        if elements.copies_to().changed() {
            for run in marked_runs(&elements.copies_to().marked(), record_run) {
                paragraph = paragraph.add_run(run);
            }
        } else {
            paragraph = paragraph.add_run(record_run(&copies_text));
        }
        lines.push(paragraph);
    }
    // 印数挂在分送 / 抄送的最后一行右端；两行都没有就不印。
    if let Some(last) = lines.pop() {
        lines.push(
            last.add_tab(
                Tab::new()
                    .val(TabValueType::Right)
                    .pos(TABLE_CONTENT_WIDTH_TWIPS),
            )
            .add_run(Run::new().add_tab())
            .add_run(record_run(&copies_note)),
        );
    }
    let has_upper = !lines.is_empty();

    let em = FOOTER_EM_TWIPS as i32;
    let mut issue = record_paragraph()
        .indent(Some(em), None, Some(em), None)
        .add_tab(
            Tab::new()
                .val(TabValueType::Right)
                .pos(TABLE_CONTENT_WIDTH_TWIPS - 2 * FOOTER_EM_TWIPS),
        )
        .add_run(record_run(
            &display.full_name(input.minutes_printing_unit()),
        ))
        .add_run(Run::new().add_tab());
    if elements.date().changed() {
        for run in marked_runs(&elements.date().marked_line(), record_run) {
            issue = issue.add_run(run);
        }
    } else {
        issue = issue.add_run(record_run(
            &crate::export::element_display::date_display_line(input),
        ));
    }
    issue = issue.add_run(record_run("印发"));

    let full = |paragraph: Paragraph| {
        TableCell::new()
            .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
            .add_paragraph(paragraph)
    };
    let mut rows: Vec<TableRow> = lines
        .into_iter()
        .map(|paragraph| TableRow::new(vec![full(paragraph)]).cant_split())
        .collect();
    let mut issue_cell = full(issue);
    if has_upper {
        // 只在印发行上方画细线：分送与抄送之间不加线（样例写法）。单格边框要从空集
        // 起设，`set_border` 会先补上四边默认实线。
        issue_cell = issue_cell.set_borders(
            TableCellBorders::with_empty().set(
                TableCellBorder::new(TableCellBorderPosition::Top)
                    .size(7)
                    .color("000000"),
            ),
        );
    }
    rows.push(TableRow::new(vec![issue_cell]).cant_split());
    super::record::add_record_table(
        doc,
        Table::new(rows)
            .set_grid(vec![TABLE_CONTENT_WIDTH_TWIPS])
            .set_borders(
                TableBorders::new()
                    .clear_all()
                    .set(TableBorder::new(TableBorderPosition::Top).size(14))
                    .set(TableBorder::new(TableBorderPosition::Bottom).size(14)),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumeration_kai_runs_keep_bold_and_paren_styles() {
        let runs = body_runs("会议认为，形势向好。一是**统一**思想（附后）。二是", None);
        // 会议认为…… / 一是 / 统一（加粗） / 思想 / （附后）（括号四号） / 。 / 二是
        assert_eq!(runs.len(), 7);
    }
}
