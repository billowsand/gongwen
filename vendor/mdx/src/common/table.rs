//! Markdown 表格识别与解析（GFM 风格 `| a | b |` + 分隔行 `|---|---|`）。
//!
//! 行为与 md_to_docx_rust 一致；分离出来是为了让公文 tex / 研报 docx 共用。

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

/// 从 `lines[start_index]` 起尝试解析连续表格块。
///
/// 返回 `(Some(rows), next_index)`；如果不是合法表格，则返回 `(None, start_index)`。
/// 行为：
/// - 允许块内最多一个空行（见原实现的"前看一行"逻辑）
/// - 必须含至少一行分隔线
/// - 输出已剥离首尾 `|`，cell 已 trim
pub fn parse_table(lines: &[String], start_index: usize) -> (Option<Vec<Vec<String>>>, usize) {
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

    let mut table_data = Vec::new();
    let mut separator_found = false;

    for line in &table_lines {
        if is_table_separator(line) {
            separator_found = true;
            continue;
        }
        let mut line = line.trim();
        if line.starts_with('|') {
            line = &line[1..];
        }
        if line.ends_with('|') {
            line = &line[..line.len() - 1];
        }
        let cells: Vec<String> = line.split('|').map(|s| s.trim().to_string()).collect();
        table_data.push(cells);
    }

    if !separator_found || table_data.is_empty() {
        return (None, start_index);
    }

    (Some(table_data), i)
}
