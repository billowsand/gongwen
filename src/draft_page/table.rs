//! 表格编辑：定位、渲染、行列插入与对齐。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::table`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::theme;
use crate::app::accent;
use crate::export::{ColumnAlign};
use std::ops::{Range};
use eframe::egui;
use crate::draft_page::{DraftPage, display_width, editor_cursor, is_table_separator_line, is_table_source_line, line_at_byte, line_ranges, split_row};

/// 光标所在的那张 GFM 表格：解析出的单元格、列对齐，以及它在源码里的位置。
pub(crate) struct TableEdit {
    /// 表头也在内，但不含分隔行。
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) aligns: Vec<ColumnAlign>,
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
    let mut rows: Vec<Vec<String>> = Vec::new();
    // 每个源码行对应 `rows` 里的哪一行；分隔行算在表头上。
    let mut row_of_line: Vec<usize> = Vec::new();
    for (offset, line) in lines[first..=last].iter().enumerate() {
        if offset == 1 {
            row_of_line.push(0);
            continue;
        }
        let mut cells = split_row(line);
        cells.resize(columns, String::new());
        row_of_line.push(rows.len());
        rows.push(cells);
    }
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
        span: ranges[first].start..ranges[last].end,
        row: row_of_line[at - first],
        column,
    })
}

/// 把单元格重排成对齐好的 Markdown。列宽取该列最宽的一格，最少三格
/// （分隔行要放得下 `:-:`）。
pub(crate) fn render_table(rows: &[Vec<String>], aligns: &[ColumnAlign]) -> String {
    let widths = aligns
        .iter()
        .enumerate()
        .map(|(column, align)| {
            rows.iter()
                .map(|row| row.get(column).map_or(0, |cell| display_width(cell)))
                .max()
                .unwrap_or(0)
                // 列宽还要放得下这一列的分隔行：写了冒号的列去掉冒号后仍需三条短横，
                // 否则整张表在解析时就不成表了。
                .max(align.min_width())
        })
        .collect::<Vec<_>>();
    let mut lines: Vec<String> = Vec::with_capacity(rows.len() + 1);
    for (index, row) in rows.iter().enumerate() {
        let mut line = String::new();
        for (column, width) in widths.iter().copied().enumerate() {
            let cell = row.get(column).map_or("", String::as_str);
            line.push_str("| ");
            line.push_str(cell);
            line.push_str(&" ".repeat(width.saturating_sub(display_width(cell))));
            line.push(' ');
        }
        line.push('|');
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
    render_table(&cells, &vec![ColumnAlign::Auto; columns])
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
    Align(ColumnAlign),
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
                table.rows.insert(at, vec![String::new(); columns]);
                table.row = at;
                "已插入一行。".to_string()
            }
            TableOp::DeleteRow => {
                if table.row == 0 {
                    *self.status = "表头删不得；要去掉整张表就把那几行选中删掉。".into();
                    return;
                }
                table.rows.remove(table.row);
                table.row = table.row.min(table.rows.len() - 1);
                "已删除一行。".to_string()
            }
            TableOp::InsertColumnLeft | TableOp::InsertColumnRight => {
                let at = if op == TableOp::InsertColumnLeft {
                    table.column
                } else {
                    table.column + 1
                };
                for row in &mut table.rows {
                    row.insert(at, String::new());
                }
                table.aligns.insert(at, ColumnAlign::Auto);
                table.column = at;
                "已插入一列。".to_string()
            }
            TableOp::DeleteColumn => {
                // 只剩两列时不能再删：一列的表格不成表，导出会退化成普通段落。
                if columns <= 2 {
                    *self.status = "表格至少要两列，删不了。".into();
                    return;
                }
                for row in &mut table.rows {
                    row.remove(table.column);
                }
                table.aligns.remove(table.column);
                table.column = table.column.min(table.aligns.len() - 1);
                "已删除一列。".to_string()
            }
            TableOp::Align(align) => {
                table.aligns[table.column] = align;
                format!("第 {} 列已设为{}。", table.column + 1, align.label())
            }
        };
        let rendered = render_table(&table.rows, &table.aligns);
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
