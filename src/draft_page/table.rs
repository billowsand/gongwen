//! 表格编辑：定位、渲染、行列插入与对齐。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::table`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::accent;
use crate::draft_page::{
    DraftPage, display_width, editor_cursor, is_table_separator_line, is_table_source_line,
    line_at_byte, line_ranges, split_row,
};
use crate::export::{ColumnAlign, TableSpan, parse_table_cells, table_span_at};
use crate::theme;
use eframe::egui;
use std::ops::Range;

/// 光标所在的那张 GFM 表格：解析出的单元格、列对齐，以及它在源码里的位置。
pub(crate) struct TableEdit {
    /// 表头也在内，但不含分隔行。
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) aligns: Vec<ColumnAlign>,
    pub(crate) spans: Vec<TableSpan>,
    /// 整张表在源码中的字节范围（表头行首到最后一行数据的行尾）。
    span: Range<usize>,
    /// 光标所在的行在 `rows` 中的下标（落在分隔行上时算表头）与列下标。
    pub(crate) row: usize,
    pub(crate) column: usize,
}

impl TableEdit {
    pub(crate) fn columns(&self) -> usize {
        self.aligns.len()
    }
}

/// 光标落在表格里就把它解析出来；不在表格里、或那几行不是合法的 GFM 表格
/// （表头之后必须紧跟分隔行）时返回 None。
pub(crate) fn table_at(text: &str, cursor: usize) -> Option<TableEdit> {
    let ranges = line_ranges(text);
    let lines = ranges
        .iter()
        .map(|range| &text[range.clone()])
        .collect::<Vec<_>>();
    let at = line_at_byte(&ranges, cursor);
    if !is_table_source_line(lines[at]) {
        return None;
    }
    let mut first = at;
    while first > 0 && is_table_source_line(lines[first - 1]) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < lines.len() && is_table_source_line(lines[last + 1]) {
        last += 1;
    }
    if last <= first || !is_table_separator_line(lines[first + 1]) {
        return None;
    }
    let mut aligns = split_row(lines[first + 1])
        .iter()
        .map(|cell| ColumnAlign::parse(cell))
        .collect::<Vec<_>>();
    let columns = aligns.len().max(split_row(lines[first]).len()).max(1);
    aligns.resize(columns, ColumnAlign::Auto);
    let mut source_rows: Vec<String> = Vec::new();
    // 每个源码行对应 `rows` 里的哪一行；分隔行算在表头上。
    let mut row_of_line: Vec<usize> = Vec::new();
    for (offset, line) in lines[first..=last].iter().enumerate() {
        if offset == 1 {
            row_of_line.push(0);
            continue;
        }
        row_of_line.push(source_rows.len());
        source_rows.push((*line).to_string());
    }
    let (rows, spans) = parse_table_cells(&source_rows, columns);
    // 列：数一数光标之前有几根竖线。行首那根不算一列的开始。
    let line_start = ranges[at].start;
    let mut cut = cursor.clamp(line_start, ranges[at].end);
    while cut > line_start && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let before = &text[line_start..cut];
    let column = before
        .matches('|')
        .count()
        .saturating_sub(1)
        .min(columns - 1);
    Some(TableEdit {
        rows,
        aligns,
        spans,
        span: ranges[first].start..ranges[last].end,
        row: row_of_line[at - first],
        column,
    })
}

/// 把单元格重排成对齐好的 Markdown。列宽取该列最宽的一格，最少三格
/// （分隔行要放得下 `:-:`）。
pub(crate) fn render_table(
    rows: &[Vec<String>],
    aligns: &[ColumnAlign],
    spans: &[TableSpan],
) -> String {
    let widths = aligns
        .iter()
        .enumerate()
        .map(|(column, align)| {
            rows.iter()
                .enumerate()
                .map(|(row_index, row)| {
                    let span = table_span_at(spans, row_index, column);
                    if span.is_some_and(|span| !span.is_anchor(row_index, column)) {
                        return 0;
                    }
                    let cell = row.get(column).map_or("", String::as_str);
                    let columns = span.map_or(1, |span| span.column_span);
                    display_width(cell).div_ceil(columns)
                })
                .max()
                .unwrap_or(0)
                // 列宽还要放得下这一列的分隔行：写了冒号的列去掉冒号后仍需三条短横，
                // 否则整张表在解析时就不成表了。
                .max(align.min_width())
        })
        .collect::<Vec<_>>();
    let mut lines: Vec<String> = Vec::with_capacity(rows.len() + 1);
    for (index, row) in rows.iter().enumerate() {
        let mut line = String::from("|");
        for (column, _width) in widths.iter().copied().enumerate() {
            let span = table_span_at(spans, index, column);
            if span.is_some_and(|span| span.column != column) {
                line.push('|');
                continue;
            }
            let cell = match span {
                Some(span) if span.row != index => "^^",
                _ => row.get(column).map_or("", String::as_str),
            };
            let column_span = span.map_or(1, |span| span.column_span);
            let combined_width = widths[column..column + column_span].iter().sum::<usize>()
                + 3 * column_span.saturating_sub(1);
            line.push(' ');
            line.push_str(cell);
            line.push_str(&" ".repeat(combined_width.saturating_sub(display_width(cell))));
            line.push_str(" |");
        }
        lines.push(line);
        if index == 0 {
            let mut separator = String::new();
            for (column, width) in widths.iter().copied().enumerate() {
                separator.push_str("| ");
                separator.push_str(&aligns[column].render(width));
                separator.push(' ');
            }
            separator.push('|');
            lines.push(separator);
        }
    }
    lines.join("\n")
}

/// 空白的 `rows` 行 `columns` 列表格，行数含表头。
///
/// 最少两列：GFM 的表格判定要求分隔行至少切出两格，一列的「表格」谁都不认，
/// 导出时会退化成普通段落。
pub(crate) fn blank_table(rows: usize, columns: usize) -> String {
    let columns = columns.max(2);
    let cells = vec![vec![String::new(); columns]; rows.max(1)];
    render_table(&cells, &vec![ColumnAlign::Auto; columns], &[])
}

/// 「插入」分区里对光标所在表格的编辑动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableOp {
    InsertRowAbove,
    InsertRowBelow,
    DeleteRow,
    InsertColumnLeft,
    InsertColumnRight,
    DeleteColumn,
    MergeRight,
    MergeDown,
    SplitCell,
    Align(ColumnAlign),
}

fn effective_span(spans: &[TableSpan], row: usize, column: usize) -> TableSpan {
    table_span_at(spans, row, column).unwrap_or(TableSpan {
        row,
        column,
        row_span: 1,
        column_span: 1,
    })
}

fn retain_real_spans(spans: &mut Vec<TableSpan>) {
    spans.retain(|span| span.row_span > 1 || span.column_span > 1);
    spans.sort_by_key(|span| (span.row, span.column));
}

fn insert_table_row(table: &mut TableEdit, at: usize) {
    table.rows.insert(at, vec![String::new(); table.columns()]);
    for span in &mut table.spans {
        if at <= span.row {
            span.row += 1;
        } else if at < span.row + span.row_span {
            span.row_span += 1;
        }
    }
}

fn delete_table_row(table: &mut TableEdit, at: usize) {
    let mut transfers = Vec::new();
    let mut next = Vec::new();
    for mut span in table.spans.drain(..) {
        if at < span.row {
            span.row -= 1;
            next.push(span);
        } else if at >= span.row + span.row_span {
            next.push(span);
        } else if span.row_span > 1 {
            if at == span.row {
                transfers.push((
                    span.row,
                    span.column,
                    table.rows[span.row][span.column].clone(),
                ));
            }
            span.row_span -= 1;
            next.push(span);
        }
    }
    table.rows.remove(at);
    table.spans = next;
    for (row, column, text) in transfers {
        if let Some(cell) = table.rows.get_mut(row).and_then(|row| row.get_mut(column)) {
            *cell = text;
        }
    }
    retain_real_spans(&mut table.spans);
}

fn insert_table_column(table: &mut TableEdit, at: usize) {
    for row in &mut table.rows {
        row.insert(at, String::new());
    }
    table.aligns.insert(at, ColumnAlign::Auto);
    for span in &mut table.spans {
        if at <= span.column {
            span.column += 1;
        } else if at < span.column + span.column_span {
            span.column_span += 1;
        }
    }
}

fn delete_table_column(table: &mut TableEdit, at: usize) {
    let mut transfers = Vec::new();
    let mut next = Vec::new();
    for mut span in table.spans.drain(..) {
        if at < span.column {
            span.column -= 1;
            next.push(span);
        } else if at >= span.column + span.column_span {
            next.push(span);
        } else if span.column_span > 1 {
            if at == span.column {
                transfers.push((
                    span.row,
                    span.column,
                    table.rows[span.row][span.column].clone(),
                ));
            }
            span.column_span -= 1;
            next.push(span);
        }
    }
    for row in &mut table.rows {
        row.remove(at);
    }
    table.aligns.remove(at);
    table.spans = next;
    for (row, column, text) in transfers {
        if let Some(cell) = table.rows.get_mut(row).and_then(|row| row.get_mut(column)) {
            *cell = text;
        }
    }
    retain_real_spans(&mut table.spans);
}

fn extend_span(table: &mut TableEdit, span: TableSpan, down: bool) -> Result<(), &'static str> {
    let (target_rows, target_columns) = if down {
        if span.row == 0 {
            return Err("表头不能与正文纵向合并。");
        }
        let target = span.row + span.row_span;
        if target >= table.rows.len() {
            return Err("下方没有可合并的单元格。");
        }
        (
            target..target + 1,
            span.column..span.column + span.column_span,
        )
    } else {
        let target = span.column + span.column_span;
        if target >= table.columns() {
            return Err("右侧没有可合并的单元格。");
        }
        (span.row..span.row + span.row_span, target..target + 1)
    };

    for row in target_rows.clone() {
        for column in target_columns.clone() {
            if !table.rows[row][column].trim().is_empty()
                || table_span_at(&table.spans, row, column).is_some()
            {
                return Err("目标单元格含有内容或已经合并，请先清空或拆分。");
            }
        }
    }

    if let Some(existing) = table
        .spans
        .iter_mut()
        .find(|item| item.row == span.row && item.column == span.column)
    {
        if down {
            existing.row_span += 1;
        } else {
            existing.column_span += 1;
        }
    } else {
        table.spans.push(TableSpan {
            row_span: span.row_span + usize::from(down),
            column_span: span.column_span + usize::from(!down),
            ..span
        });
    }
    retain_real_spans(&mut table.spans);
    Ok(())
}

/// 「表格」下拉里的网格选择器：8×8 的小方格，鼠标划到哪就亮到哪，
/// 点一下插入对应大小的表格。返回 `(行数, 列数)`，行数含表头。
pub(crate) fn table_grid_picker(ui: &mut egui::Ui) -> Option<(usize, usize)> {
    const MAX_ROWS: usize = 8;
    const MAX_COLUMNS: usize = 8;
    const CELL: f32 = 18.0;
    const GAP: f32 = 3.0;

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(
            MAX_COLUMNS as f32 * (CELL + GAP),
            MAX_ROWS as f32 * (CELL + GAP),
        ),
        egui::Sense::click(),
    );
    let pointer = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos());
    let picked = pointer.map(|pos| {
        let column = (((pos.x - rect.left()) / (CELL + GAP)) as usize).min(MAX_COLUMNS - 1);
        let row = (((pos.y - rect.top()) / (CELL + GAP)) as usize).min(MAX_ROWS - 1);
        // 一列的表格不成表（见 `blank_table`），所以最少亮两列。
        (row + 1, (column + 1).max(2))
    });
    {
        let painter = ui.painter();
        for row in 0..MAX_ROWS {
            for column in 0..MAX_COLUMNS {
                let cell = egui::Rect::from_min_size(
                    rect.min + egui::vec2(column as f32 * (CELL + GAP), row as f32 * (CELL + GAP)),
                    egui::vec2(CELL, CELL),
                );
                let lit = picked.is_some_and(|(rows, columns)| row < rows && column < columns);
                painter.rect(
                    cell,
                    egui::CornerRadius::same(2),
                    if lit {
                        theme::accent_soft()
                    } else {
                        theme::surface_sunk()
                    },
                    egui::Stroke::new(1.0, if lit { accent() } else { theme::border() }),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
    ui.label(
        egui::RichText::new(match picked {
            Some((rows, columns)) => format!("{rows} 行 × {columns} 列（首行为表头）"),
            None => "在网格上划出表格大小".to_string(),
        })
        .color(theme::text_muted()),
    );
    response.clicked().then_some(picked).flatten()
}

impl DraftPage<'_> {
    /// 在光标处插入一张空白表格。
    pub(crate) fn insert_table(&mut self, ctx: &egui::Context, rows: usize, columns: usize) {
        if self.doc.read_only() {
            return;
        }
        let markdown = blank_table(rows, columns);
        let position = self.insert_block(ctx, &markdown);
        // 光标落进表头第一格，接着就能打字。
        self.doc.pending_source_jump = Some(position + 2);
        *self.status = format!("已插入 {rows} 行 {columns} 列表格。");
    }

    /// 光标所在的那张表格；不在表格里返回 None。
    pub(crate) fn table_at_cursor(&self, ctx: &egui::Context) -> Option<TableEdit> {
        let cursor = editor_cursor(ctx, &self.doc.generated_markdown)?;
        table_at(&self.doc.generated_markdown, cursor)
    }

    /// 表格的行列增删与列对齐。改完整张表按最宽的单元格重新对齐竖线，
    /// 让源码保持能读——手工维护的表格几行之后就会歪得没法看。
    pub(crate) fn apply_table_op(&mut self, ctx: &egui::Context, op: TableOp) {
        if self.doc.read_only() {
            return;
        }
        let Some(mut table) = self.table_at_cursor(ctx) else {
            *self.status = "把光标放进表格里再用这几个按钮。".into();
            return;
        };
        let columns = table.columns();
        let message = match op {
            TableOp::InsertRowAbove | TableOp::InsertRowBelow => {
                // 表头之上插不了数据行——第一行就是表头。
                let at = if op == TableOp::InsertRowAbove {
                    table.row.max(1)
                } else {
                    table.row + 1
                };
                insert_table_row(&mut table, at);
                table.row = at;
                "已插入一行。".to_string()
            }
            TableOp::DeleteRow => {
                if table.row == 0 {
                    *self.status = "表头删不得；要去掉整张表就把那几行选中删掉。".into();
                    return;
                }
                let row = table.row;
                delete_table_row(&mut table, row);
                table.row = table.row.min(table.rows.len() - 1);
                "已删除一行。".to_string()
            }
            TableOp::InsertColumnLeft | TableOp::InsertColumnRight => {
                let at = if op == TableOp::InsertColumnLeft {
                    table.column
                } else {
                    table.column + 1
                };
                insert_table_column(&mut table, at);
                table.column = at;
                "已插入一列。".to_string()
            }
            TableOp::DeleteColumn => {
                // 只剩两列时不能再删：一列的表格不成表，导出会退化成普通段落。
                if columns <= 2 {
                    *self.status = "表格至少要两列，删不了。".into();
                    return;
                }
                let column = table.column;
                delete_table_column(&mut table, column);
                table.column = table.column.min(table.aligns.len() - 1);
                "已删除一列。".to_string()
            }
            TableOp::MergeRight | TableOp::MergeDown => {
                let span = effective_span(&table.spans, table.row, table.column);
                let down = op == TableOp::MergeDown;
                if let Err(message) = extend_span(&mut table, span, down) {
                    *self.status = message.into();
                    return;
                }
                if down {
                    "已向下合并一个单元格。".to_string()
                } else {
                    "已向右合并一个单元格。".to_string()
                }
            }
            TableOp::SplitCell => {
                let span = effective_span(&table.spans, table.row, table.column);
                let before = table.spans.len();
                table
                    .spans
                    .retain(|item| item.row != span.row || item.column != span.column);
                if table.spans.len() == before {
                    *self.status = "当前单元格没有合并。".into();
                    return;
                }
                table.row = span.row;
                table.column = span.column;
                "已拆分单元格。".to_string()
            }
            TableOp::Align(align) => {
                table.aligns[table.column] = align;
                format!("第 {} 列已设为{}。", table.column + 1, align.label())
            }
        };
        let rendered = render_table(&table.rows, &table.aligns, &table.spans);
        // 光标回到原来那一行的第一格里，接着改。分隔行在表头之后占一行。
        let target_line = if table.row == 0 { 0 } else { table.row + 1 };
        let offset = rendered
            .split('\n')
            .take(target_line)
            .map(|line| line.len() + 1)
            .sum::<usize>();
        let start = table.span.start;
        self.doc
            .generated_markdown
            .replace_range(table.span.clone(), &rendered);
        self.doc.pending_source_jump = Some(start + offset + 2);
        *self.status = message;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editable_table() -> TableEdit {
        TableEdit {
            rows: vec![
                vec!["类别".into(), "项目".into(), "说明".into()],
                vec!["综合".into(), String::new(), String::new()],
                vec![String::new(), String::new(), String::new()],
            ],
            aligns: vec![ColumnAlign::Auto; 3],
            spans: Vec::new(),
            span: 0..0,
            row: 1,
            column: 0,
        }
    }

    #[test]
    fn merge_survives_insert_and_delete_inside_span() {
        let mut table = editable_table();
        let cell = effective_span(&table.spans, 1, 0);
        extend_span(&mut table, cell, false).unwrap();
        let cell = effective_span(&table.spans, 1, 0);
        extend_span(&mut table, cell, true).unwrap();
        assert_eq!(
            table.spans[0],
            TableSpan {
                row: 1,
                column: 0,
                row_span: 2,
                column_span: 2,
            }
        );

        insert_table_column(&mut table, 1);
        insert_table_row(&mut table, 2);
        assert_eq!(table.spans[0].column_span, 3);
        assert_eq!(table.spans[0].row_span, 3);

        delete_table_column(&mut table, 1);
        delete_table_row(&mut table, 2);
        assert_eq!(table.spans[0].column_span, 2);
        assert_eq!(table.spans[0].row_span, 2);
        assert_eq!(table.rows[1][0], "综合");
    }

    #[test]
    fn merge_refuses_content_and_header_body_crossing() {
        let mut table = editable_table();
        table.rows[1][1] = "已有内容".into();
        let cell = effective_span(&table.spans, 1, 0);
        assert!(extend_span(&mut table, cell, false).is_err());

        let header = effective_span(&table.spans, 0, 0);
        assert_eq!(
            extend_span(&mut table, header, true),
            Err("表头不能与正文纵向合并。")
        );
    }
}
