//! 页眉：发文机关标志、字号压缩与页码页脚。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::header`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::docx::{
    BODY_LINE_TWIPS, HEADER_SIZE, HEADER_WIDTH_TWIPS, PAGE_NUMBER_LINE_TWIPS, PAGE_NUMBER_SIZE,
    chinese_fonts,
};
use docx_rs::*;

/// 红头的排布方式。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HeaderLayout {
    /// 字号，半磅为单位。
    pub(crate) size: usize,
    /// 左右缩进，把拉开后的整块摆到版心中间。
    pub(crate) side_indent: i32,
    /// 长名称按 TeX resizebox 横向压缩，保持红头字高。
    pub(crate) scale: i32,
}

/// 发文机关标志分两种情形排布：
/// 1. 版心排得下——字号不变，用分散对齐把字距均匀撑开，字形不会变形；
///    字数很少时把字距限制在一个字宽以内，免得几个字散得满页都是。
/// 2. 版心排不下——与 TeX resizebox 一样压缩宽度，保持红头字高。
pub(crate) fn issuing_unit_header(unit: &str) -> HeaderLayout {
    // 汉字是等宽的：一个字的宽度 = 字号（半磅）× 10 twip。
    let char_width = |size: usize| size as i32 * 10;
    let count = unit
        .chars()
        .filter(|value| !value.is_whitespace())
        .count()
        .max(1) as i32;
    let natural = count * char_width(HEADER_SIZE);

    if natural >= HEADER_WIDTH_TWIPS {
        return HeaderLayout {
            size: HEADER_SIZE,
            side_indent: 0,
            scale: (HEADER_WIDTH_TWIPS * 100 / natural).max(1),
        };
    }

    // 字距拉到一个字宽时的总宽度，作为散开的上限。
    let widest = natural + (count - 1) * char_width(HEADER_SIZE);
    let block = widest.min(HEADER_WIDTH_TWIPS);
    HeaderLayout {
        size: HEADER_SIZE,
        side_indent: (HEADER_WIDTH_TWIPS - block) / 2,
        scale: 100,
    }
}

pub(crate) fn issuing_unit_paragraph(unit: &str) -> Paragraph {
    let layout = issuing_unit_header(unit);
    Paragraph::new()
        .add_run(
            Run::new()
                .add_text(unit)
                .fonts(chinese_fonts("方正小标宋简体"))
                .size(layout.size)
                .color("FF0000")
                .stretch(layout.scale),
        )
        .align(AlignmentType::Distribute)
        .indent(
            Some(layout.side_indent),
            None,
            Some(layout.side_indent),
            None,
        )
        // 红头字号大于正文，不能继承正文的固定行高，否则 Word 会裁掉字顶。
        .line_spacing(
            LineSpacing::new()
                .line(1000)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

/// 红头（含反线）浮动表：相对纸面顶端 27 mm、版心左边缘对齐，不参与正文流。
///
/// TeX 的 \DocumentHeader 用 \vspace*{-10mm} 把红头上提至上白边内（距纸顶
/// 37mm-10mm=27mm），正文仍从 37mm 的版心顶起排。Word 里若把红头作为页眉的
/// 普通内容，页眉下沿（约 44mm）会超过上边距（37mm），Word 会把首页正文整体下压；
/// 改成与红头呈批件（red.rs）相同的浮动表后，页眉自身的在流高度趋近于零，
/// 正文不再被压低。
pub(crate) fn letter_head_float_table(unit: &str) -> Table {
    // TeX：红头距纸顶 27mm = 1531 twips。
    const LETTER_HEAD_Y_TWIPS: i32 = 1531;
    Table::new(vec![TableRow::new(vec![
        TableCell::new().add_paragraph(issuing_unit_paragraph(unit)),
    ])])
    .set_grid(vec![HEADER_WIDTH_TWIPS as usize])
    .width(HEADER_WIDTH_TWIPS as usize, WidthType::Dxa)
    .layout(TableLayoutType::Fixed)
    .margins(TableCellMargins::new().margin(0, 0, 0, 0))
    .clear_all_border()
    // 表底边框即 0.53 mm 红色反线，与原先紧跟红头的反线表视觉一致。
    .set_borders(
        TableBorders::new().clear_all().set(
            TableBorder::new(TableBorderPosition::Bottom)
                .size(12)
                .color("FF0000"),
        ),
    )
    .position(
        TablePositionProperty::new()
            .horizontal_anchor("margin")
            .vertical_anchor("page")
            .position_x_alignment("left")
            .position_y(LETTER_HEAD_Y_TWIPS)
            .left_from_text(0)
            .right_from_text(0),
    )
}

pub(crate) fn letter_header(unit: &str) -> Header {
    Header::new()
        // 占位段落把页眉的在流高度压到最低，红头完全由浮动表定位。
        .add_paragraph(
            Paragraph::new()
                .add_run(Run::new().size(2))
                .line_spacing(LineSpacing::new().line(1).line_rule(LineSpacingType::Exact)),
        )
        .add_table(letter_head_float_table(unit))
}

pub(crate) fn blank_line() -> Paragraph {
    Paragraph::new().line_spacing(
        LineSpacing::new()
            .line(BODY_LINE_TWIPS as i32)
            .line_rule(LineSpacingType::Exact),
    )
}

pub(crate) fn page_number_footer(alignment: AlignmentType) -> Footer {
    let page_number = Run::new()
        .add_text("— ")
        .add_field_char(FieldCharType::Begin, false)
        .add_instr_text(InstrText::PAGE(InstrPAGE::new()))
        .add_field_char(FieldCharType::Separate, false)
        .add_text("1")
        .add_field_char(FieldCharType::End, false)
        .add_text(" —")
        .fonts(chinese_fonts("SimSun"))
        .size(PAGE_NUMBER_SIZE);
    Footer::new().add_paragraph(
        Paragraph::new()
            .add_run(page_number)
            .align(alignment)
            // 行高固定：页脚距边界是按「纸底 − 页脚距边界 − 这个行高 = 页码行上沿」
            // 反算出来的，行高一变，页码就不在 7 mm 的位置上了。
            .line_spacing(
                LineSpacing::new()
                    .line(PAGE_NUMBER_LINE_TWIPS)
                    .line_rule(LineSpacingType::Exact),
            ),
    )
}

pub(crate) fn add_official_page_footers(doc: Docx, duplex: bool) -> Docx {
    let doc = if duplex {
        doc.footer(page_number_footer(AlignmentType::Right))
            .even_footer(page_number_footer(AlignmentType::Left))
    } else {
        doc.footer(page_number_footer(AlignmentType::Center))
    };
    // 与函件 LaTeX 类保持一致：首页不显示页码。
    doc.first_footer(Footer::new())
}
