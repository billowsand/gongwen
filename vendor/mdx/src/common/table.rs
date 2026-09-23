//! Markdown 表格识别与解析（GFM 风格 `| a | b |` + 分隔行 `|---|---|`）。
//!
//! 行为与 md_to_docx_rust 一致；分离出来是为了让公文 tex / 研报 docx 共用。
//! 合并单元格沿用 MultiMarkdown 写法：`||` 横向并入左格、`^^` 纵向并入上格。

/// 一行是否符合表格行外观（被 `|...|` 包围且至少含一个 `|`）。
pub fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    line.contains('|') && trimmed.starts_with('|') && trimmed.ends_with('|')
}

/// 一行是否是 markdown 表分隔行（仅包含 `-`、`:`、空格）。
pub fn is_table_separator(line: &str) -> bool {
    let line = line.trim();
    if !line.starts_with('|') || !line.ends_with('|') {
        return false;
    }
    let content = &line[1..line.len() - 1];
    for part in content.split('|') {
        let part = part.trim();
        if part.is_empty() || !part.chars().all(|c| c == '-' || c == ':' || c == ' ') {
            return false;
        }
    }
    true
}

/// 表格中的合并单元格。`row`/`column` 指向左上角锚点（`row` 从表头 0 起算），
/// 跨度至少为 1；只有横向或纵向跨度大于 1 的格子才会出现在 `spans` 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableSpan {
    pub row: usize,
    pub column: usize,
    pub row_span: usize,
    pub column_span: usize,
}

impl TableSpan {
    pub fn covers(self, row: usize, column: usize) -> bool {
        row >= self.row
            && row < self.row + self.row_span
            && column >= self.column
            && column < self.column + self.column_span
    }

    pub fn is_anchor(self, row: usize, column: usize) -> bool {
        self.row == row && self.column == column
    }
}

/// 返回覆盖指定网格位置的合并单元格；普通 1×1 单元格返回 None。
pub fn span_at(spans: &[TableSpan], row: usize, column: usize) -> Option<TableSpan> {
    spans.iter().copied().find(|span| span.covers(row, column))
}

/// 解析好的表格：`rows` 是矩形网格（每行列数相同，被合并掉的格子是空串），
/// `spans` 是其中的合并单元格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTable {
    pub rows: Vec<Vec<String>>,
    pub spans: Vec<TableSpan>,
}

/// 从 `lines[start_index]` 起尝试解析连续表格块。
///
/// 返回 `(Some(table), next_index)`；如果不是合法表格，则返回 `(None, start_index)`。
/// 行为：
/// - 允许块内最多一个空行（见原实现的"前看一行"逻辑）
/// - 必须含至少一行分隔线
/// - 输出已剥离首尾 `|`，cell 已 trim
/// - 合并单元格见 [`parse_cells`]
pub fn parse_table(lines: &[String], start_index: usize) -> (Option<ParsedTable>, usize) {
    let mut table_lines = Vec::new();
    let mut i = start_index;

    while i < lines.len() {
        let line = lines[i].trim();
        if is_table_line(line) {
            table_lines.push(line.to_string());
        } else if line.is_empty() && !table_lines.is_empty() {
            i += 1;
            if i < lines.len() && is_table_line(lines[i].trim()) {
                continue;
            } else {
                break;
            }
        } else {
            break;
        }
        i += 1;
    }

    if table_lines.len() < 2 {
        return (None, start_index);
    }

    let mut source_rows = Vec::new();
    let mut separator_found = false;

    for line in &table_lines {
        if is_table_separator(line) {
            separator_found = true;
            continue;
        }
        source_rows.push(line.as_str());
    }

    if !separator_found || source_rows.is_empty() {
        return (None, start_index);
    }

    (Some(parse_cells(&source_rows)), i)
}

/// 一行源码里的一格。两个竖线之间完全没有字符时（`||`）沿用 MultiMarkdown
/// 语义：本格并入左格。写成竖线、空格、竖线仍是一个普通空白单元格。
struct SourceCell {
    text: String,
    join_left: bool,
}

fn source_cells(line: &str) -> Vec<SourceCell> {
    let mut value = line.trim();
    if let Some(rest) = value.strip_prefix('|') {
        value = rest;
    }
    if let Some(rest) = value.strip_suffix('|') {
        value = rest;
    }
    value
        .split('|')
        .map(|cell| SourceCell {
            text: cell.trim().to_string(),
            join_left: cell.is_empty(),
        })
        .collect()
}

struct WorkingCell {
    row: usize,
    column: usize,
    text: String,
    row_span: usize,
    column_span: usize,
    active: bool,
}

/// 把表格各行（不含分隔行）归一化为矩形网格与合并单元格。
///
/// - 横向：`| 标题 |||` 里紧挨着的竖线表示本格并入左格；
/// - 纵向：单元格只写 `^^` 表示并入正上方的格子（表头不参与纵向合并）。
///   `^^` 右侧紧跟的 `||` 须与上方格子同宽，否则不合并、按原文保留，
///   免得手写出错时静默吞掉内容。
///
/// 列数取表头的列数（表头里的 `||` 也算列），其余行多截少补。规则与公文助手
/// 的 `export::parse::parse_table_cells` 一致，预览与成品才对得上。
pub fn parse_cells(source_rows: &[&str]) -> ParsedTable {
    let column_count = source_rows
        .first()
        .map_or(0, |header| source_cells(header).len());
    if column_count == 0 {
        return ParsedTable {
            rows: Vec::new(),
            spans: Vec::new(),
        };
    }

    let row_count = source_rows.len();
    let mut owners: Vec<Vec<Option<usize>>> = vec![vec![None; column_count]; row_count];
    let mut cells: Vec<WorkingCell> = Vec::new();

    for (row_index, source) in source_rows.iter().enumerate() {
        let mut slots = source_cells(source);
        slots.resize_with(column_count, || SourceCell {
            text: String::new(),
            join_left: false,
        });
        slots.truncate(column_count);

        for (column_index, slot) in slots.into_iter().enumerate() {
            if slot.join_left && column_index > 0 {
                if let Some(owner) = owners[row_index][column_index - 1] {
                    if cells[owner].row == row_index {
                        cells[owner].column_span += 1;
                        owners[row_index][column_index] = Some(owner);
                        continue;
                    }
                }
            }
            let owner = cells.len();
            cells.push(WorkingCell {
                row: row_index,
                column: column_index,
                text: slot.text,
                row_span: 1,
                column_span: 1,
                active: true,
            });
            owners[row_index][column_index] = Some(owner);
        }

        if row_index == 0 {
            continue;
        }
        let row_cells = owners[row_index].clone();
        for owner in row_cells.into_iter().flatten() {
            if !cells[owner].active || cells[owner].row != row_index || cells[owner].text != "^^" {
                continue;
            }
            let column = cells[owner].column;
            let column_span = cells[owner].column_span;
            let Some(above) = owners[row_index - 1][column] else {
                continue;
            };
            let valid = cells[above].active
                && cells[above].row != 0
                && cells[above].column == column
                && cells[above].column_span == column_span
                && cells[above].row + cells[above].row_span == row_index;
            if !valid {
                continue;
            }
            cells[above].row_span += 1;
            cells[owner].active = false;
            for slot in owners[row_index].iter_mut().skip(column).take(column_span) {
                *slot = Some(above);
            }
        }
    }

    let mut rows = vec![vec![String::new(); column_count]; row_count];
    let mut spans = Vec::new();
    for cell in cells.into_iter().filter(|cell| cell.active) {
        rows[cell.row][cell.column] = cell.text;
        if cell.row_span > 1 || cell.column_span > 1 {
            spans.push(TableSpan {
                row: cell.row,
                column: cell.column,
                row_span: cell.row_span,
                column_span: cell.column_span,
            });
        }
    }
    ParsedTable { rows, spans }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(row: usize, column: usize, row_span: usize, column_span: usize) -> TableSpan {
        TableSpan {
            row,
            column,
            row_span,
            column_span,
        }
    }

    #[test]
    fn plain_table_has_no_spans() {
        let table = parse_cells(&["| A | B |", "| 1 |  |"]);
        assert_eq!(table.rows, vec![vec!["A", "B"], vec!["1", ""]]);
        assert!(table.spans.is_empty());
    }

    #[test]
    fn adjacent_pipes_merge_into_the_left_cell() {
        let table = parse_cells(&["| A | B | C |", "| 分组 |||", "| 1 | x ||"]);
        assert_eq!(table.rows[1], vec!["分组", "", ""]);
        assert_eq!(table.spans, vec![span(1, 0, 1, 3), span(2, 1, 1, 2)]);
    }

    #[test]
    fn carets_merge_with_the_cell_above() {
        let table = parse_cells(&["| A | B |", "| 甲 | 1 |", "| ^^ | 2 |", "| ^^ | 3 |"]);
        assert_eq!(table.spans, vec![span(1, 0, 3, 1)]);
        assert_eq!(table.rows[2][0], "");
    }

    #[test]
    fn invalid_carets_stay_as_text() {
        // 表头不参与纵向合并；宽度对不上的 `^^` 也不合并。
        let table = parse_cells(&[
            "| A | B | C |",
            "| ^^ | 1 | 2 |",
            "| 甲 |||",
            "| ^^ | x | y |",
        ]);
        assert_eq!(table.rows[1][0], "^^");
        assert_eq!(table.rows[3][0], "^^");
        assert_eq!(table.spans, vec![span(2, 0, 1, 3)]);
    }
}
