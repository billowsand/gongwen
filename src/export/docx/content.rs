//! 正文与议程：智能表格、正文块排版与会议议程导出。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::content`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{DraftInput};
use std::fs::{File};
use std::path::{Path};
use crate::export::{ColumnAlign, MarkdownBlock, inline_segments, official_heading_text, plain_text};
use crate::export::table::{ColumnAlignment, to_docx_grid};
use crate::export::title;
use anyhow::{Context, Result};
use regex::Regex;
use crate::export::docx::{BODY_SIZE, AGENDA_NUMBERING_ID, TABLE_SIZE, TABLE_CONTENT_WIDTH_TWIPS, chinese_fonts, body_runs, security_runs, docx_name, table_run_sized, table_runs_sized, body_paragraph, label_paragraph, heading_paragraph, document_title_paragraph, image_paragraph, agenda_body_paragraph, agenda_blank_line, agenda_labeled_paragraph};
use docx_rs::*;

pub(crate) fn add_smart_table(mut doc: Docx, rows: &[Vec<String>], aligns: &[ColumnAlign]) -> Docx {
    if rows.is_empty() {
        return doc;
    }
    let (grid, alignments) = to_docx_grid(rows, aligns, TABLE_CONTENT_WIDTH_TWIPS, TABLE_SIZE * 10);
    if grid.is_empty() {
        return doc;
    }
    // 规格 §6：表头含“姓名/联系人”的列，非表头单元格按版记的方式处理姓名宽度。
    let name_column = rows.first().and_then(|header| {
        header
            .iter()
            .position(|cell| cell.contains("姓名") || cell.contains("联系人"))
    });

    let table_rows = rows
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            let cells = grid
                .iter()
                .enumerate()
                .map(|(column_index, width)| {
                    let text = row.get(column_index).map_or("", String::as_str);
                    // 表头一律居中；正文单元格按列对齐（分隔行写了冒号就以它为准）。
                    let alignment = if row_index == 0 {
                        AlignmentType::Center
                    } else {
                        match alignments.get(column_index) {
                            Some(ColumnAlignment::Center) => AlignmentType::Center,
                            Some(ColumnAlignment::Right) => AlignmentType::Right,
                            _ => AlignmentType::Left,
                        }
                    };
                    let runs = if name_column == Some(column_index) && row_index > 0 {
                        let segments = inline_segments(text);
                        let cleaned = segments
                            .iter()
                            .map(|segment| segment.text.as_str())
                            .collect::<String>();
                        let bold = segments.iter().any(|segment| segment.bold);
                        let (name_text, size) = docx_name(&cleaned, TABLE_SIZE);
                        let mut run = table_run_sized(&name_text, false, size);
                        if bold {
                            run = run.bold();
                        }
                        vec![run]
                    } else {
                        table_runs_sized(text, row_index == 0, TABLE_SIZE)
                    };
                    let mut paragraph = Paragraph::new();
                    for run in runs {
                        paragraph = paragraph.add_run(run);
                    }
                    let paragraph = paragraph.align(alignment).line_spacing(
                        LineSpacing::new()
                            .line(420)
                            .line_rule(LineSpacingType::Exact),
                    );
                    TableCell::new()
                        .width(*width, WidthType::Dxa)
                        .add_paragraph(paragraph)
                })
                .collect();
            TableRow::new(cells)
        })
        .collect::<Vec<_>>();

    let borders = TableBorders::new()
        .set(TableBorder::new(TableBorderPosition::Top).size(4))
        .set(TableBorder::new(TableBorderPosition::Left).size(4))
        .set(TableBorder::new(TableBorderPosition::Bottom).size(4))
        .set(TableBorder::new(TableBorderPosition::Right).size(4))
        .set(TableBorder::new(TableBorderPosition::InsideH).size(4))
        .set(TableBorder::new(TableBorderPosition::InsideV).size(4));
    let table = Table::new(table_rows)
        .set_grid(grid)
        .width(TABLE_CONTENT_WIDTH_TWIPS, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .set_borders(borders);
    doc = doc.add_table(table);
    doc
}

pub(crate) fn add_official_content_block(
    mut doc: Docx,
    block: &MarkdownBlock,
    counters: &mut [usize; 4],
) -> Docx {
    match block {
        MarkdownBlock::Heading(level, text) => {
            if let Some(title) = official_heading_text(*level, text, counters) {
                doc = doc.add_paragraph(heading_paragraph(*level, &title));
            }
        }
        MarkdownBlock::Paragraph(text)
            if !text.trim().is_empty() && !text.contains("<div") && !text.contains("</div") =>
        {
            doc = doc.add_paragraph(body_paragraph(text));
        }
        MarkdownBlock::ListItem(text) => {
            doc = doc.add_paragraph(label_paragraph(text).indent(Some(420), None, None, None));
        }
        MarkdownBlock::Table { rows, aligns } => doc = add_smart_table(doc, rows, aligns),
        MarkdownBlock::Image { alt, src } => {
            if let Some(paragraph) = image_paragraph(alt, src) {
                doc = doc.add_paragraph(paragraph);
            }
        }
        _ => {}
    }
    doc
}

pub(crate) fn agenda_numbering() -> AbstractNumbering {
    AbstractNumbering::new(AGENDA_NUMBERING_ID).add_level(
        Level::new(
            0,
            Start::new(1),
            NumberFormat::new("decimal"),
            LevelText::new("%1."),
            LevelJc::new("left"),
        )
        .suffix(LevelSuffixType::Space)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(BODY_SIZE),
    )
}

pub(crate) fn write_meeting_agenda_docx(path: &Path, input: &DraftInput, markdown: &str) -> Result<()> {
    let title = markdown
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(input.title_hint.as_str());
    let security_level = if input.profile.security_level.trim().is_empty() {
        "【待核实：密级】"
    } else {
        input.profile.security_level.trim()
    };
    let security_period = if input.profile.security_period.trim().is_empty() {
        "【待核实：保密期限】"
    } else {
        input.profile.security_period.trim()
    };
    let item_pattern = Regex::new(r"^\d+[.、．)]\s*(.+)$").expect("valid regex");

    let mut doc = Docx::new()
        .page_size(11906, 16838)
        .page_margin(PageMargin {
            top: 2098,
            bottom: 1984,
            left: 1587,
            right: 1474,
            header: 567,
            footer: 567,
            gutter: 0,
        })
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(BODY_SIZE)
        .default_line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .add_abstract_numbering(agenda_numbering())
        .add_numbering(Numbering::new(AGENDA_NUMBERING_ID, AGENDA_NUMBERING_ID));

    // 指人专办：勾选后在“密级★保密期限”后空一个全角空格，再以黑体标注“指人专办”。
    // 会议议程密级行整体黑体加粗；数字年限的保密期限数字部分用等宽西文字体。
    let special = if input.profile.special_handling {
        "　指人专办"
    } else {
        ""
    };
    let mut security = Paragraph::new();
    for run in security_runs(security_level, security_period, special, "黑体", true) {
        security = security.add_run(run);
    }
    doc = doc.add_paragraph(
        security
            .align(AlignmentType::Left)
            .line_spacing(
                LineSpacing::new()
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true),
    );
    doc = doc.add_paragraph(agenda_blank_line());
    let plan = title::title_plan(&plain_text(title), title::chars_per_line());
    doc = doc.add_paragraph(
        document_title_paragraph(title, &plan)
            .line_spacing(
                LineSpacing::new()
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true),
    );
    doc = doc.add_paragraph(agenda_blank_line());

    for line in markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("# ") {
            continue;
        }
        if line.starts_with("一、时间地点")
            || line.starts_with("二、参加人员")
            || line.starts_with("三、研讨内容")
        {
            doc = doc.add_paragraph(agenda_labeled_paragraph(line));
        } else if let Some(captures) = item_pattern.captures(line) {
            let content = captures.get(1).map(|value| value.as_str()).unwrap_or(line);
            let mut item = agenda_body_paragraph();
            for run in body_runs(content) {
                item = item.add_run(run);
            }
            doc = doc.add_paragraph(
                item.numbering(NumberingId::new(AGENDA_NUMBERING_ID), IndentLevel::new(0)),
            );
        } else {
            let mut item = agenda_body_paragraph();
            for run in body_runs(line) {
                item = item.add_run(run);
            }
            doc = doc.add_paragraph(item);
        }
    }

    let file =
        File::create(path).with_context(|| format!("无法创建 Word 文件：{}", path.display()))?;
    doc.build().pack(file).context("写入会议议程 DOCX 包失败")?;
    Ok(())
}
