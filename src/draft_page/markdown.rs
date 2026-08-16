//! Markdown/表格编辑工具函数与编辑器插入动作。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::markdown`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::draft_page::{DraftPage, blank_line_padding, editor_id};
use crate::export;
use eframe::egui;
use std::ops::Range;

pub(crate) fn markdown_heading_level(line: &str) -> Option<u8> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    ((1..=6).contains(&hashes)
        && trimmed
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace))
    .then_some(hashes as u8)
}

pub(crate) fn is_table_source_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && trimmed.matches('|').count() >= 2
}

pub(crate) fn is_table_separator_line(line: &str) -> bool {
    let cells = line
        .trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .collect::<Vec<_>>();
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let value = cell.trim().trim_matches(':');
            value.len() >= 3 && value.chars().all(|ch| ch == '-')
        })
}

pub(crate) fn table_column_count(line: &str) -> usize {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .count()
        .max(1)
}

/// 每一行在源码中的字节范围，不含行尾的换行符。空文本也返回一行，
/// 免得调用方到处判空。
pub(crate) fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for piece in text.split_inclusive('\n') {
        let line = piece.strip_suffix('\n').unwrap_or(piece);
        let line = line.strip_suffix('\r').unwrap_or(line);
        ranges.push(start..start + line.len());
        start += piece.len();
    }
    // 文本以换行结尾时，`split_inclusive` 不会再给出末尾那个空行；光标停在
    // 那里同样要有行可落，所以补一行。
    if text.ends_with('\n') || ranges.is_empty() {
        ranges.push(text.len()..text.len());
    }
    ranges
}

/// 给定字节位置落在第几行。越界时归到最后一行。
pub(crate) fn line_at_byte(ranges: &[Range<usize>], byte: usize) -> usize {
    ranges
        .iter()
        .position(|range| byte <= range.end)
        .unwrap_or(ranges.len() - 1)
}

/// 单元格在等宽显示下占几格：ASCII 一格，中日韩字符两格。只用于把源码里的
/// 竖线对齐，不影响导出后的列宽。
pub(crate) fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| if ch.is_ascii() { 1 } else { 2 })
        .sum()
}

/// 拆一行表格：去掉首尾竖线后按竖线切开，每格去空白。
pub(crate) fn split_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// 把某一行改成指定层级的标题；`level` 为 0 表示降回正文。
pub(crate) fn set_heading(line: &str, level: u8) -> String {
    let body = line.trim_start().trim_start_matches('#').trim_start();
    if level == 0 {
        body.to_string()
    } else {
        format!("{} {body}", "#".repeat(level as usize))
    }
}

/// 项目符号开关：已经是 `- ` / `* ` 开头就去掉，否则加上。
pub(crate) fn toggle_bullet(line: &str) -> String {
    let body = line.trim_start();
    match body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")) {
        Some(rest) => rest.to_string(),
        None if body.is_empty() => body.to_string(),
        None => format!("- {body}"),
    }
}

/// 有序列表开关。源码统一写成 `1. `，实际编号由解析器按连续列表组计算。
pub(crate) fn toggle_ordered(line: &str) -> String {
    if let Some((_, text)) = export::parse_ordered_item(line) {
        text.to_string()
    } else if line.trim().is_empty() {
        "1. ".to_string()
    } else {
        format!("1. {}", line.trim_start())
    }
}

/// 在有序列表项中按回车：非空项续写一个 `1. ` 占位；空项再次回车则移除
/// 占位并结束列表。返回修改后的正文和新光标字节位置；非列表行返回 None，
/// 交还给 TextEdit 做普通换行。
pub(crate) fn continue_ordered_list(text: &str, cursor: usize) -> Option<(String, usize)> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index);
    let line = &text[line_start..line_end];
    let (_, content) = export::parse_ordered_item(line)?;
    let content_start = content.as_ptr() as usize - line.as_ptr() as usize;
    if cursor < line_start + content_start {
        return None;
    }

    if content.trim().is_empty() {
        let mut updated = text.to_string();
        updated.replace_range(line_start..line_end, "");
        updated = export::normalize_ordered_list_punctuation(&updated);
        // 标点规范化可能改变前面列表项的字节长度；按行数重新找当前空行。
        let line_number = text[..line_start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let new_cursor = if line_number == 0 {
            0
        } else {
            updated
                .match_indices('\n')
                .nth(line_number - 1)
                .map_or(updated.len(), |(index, _)| index + 1)
        };
        return Some((updated, new_cursor));
    }

    let indent_len = line.len() - line.trim_start_matches(' ').len();
    let marker = format!("\n{}1. ", " ".repeat(indent_len.min(3)));
    let mut updated = text.to_string();
    updated.insert_str(cursor, &marker);
    Some((updated, cursor + marker.len()))
}

/// 把选区覆盖到的每一行都过一遍 `edit`，返回改过之后的全文与新的选区。
/// 选区落在行中间也按整行处理——标题层级、项目符号本来就是整行的事。
pub(crate) fn map_lines(
    text: &str,
    range: &Range<usize>,
    edit: impl Fn(&str) -> String,
) -> (String, Range<usize>) {
    let ranges = line_ranges(text);
    let first = line_at_byte(&ranges, range.start);
    let last = line_at_byte(&ranges, range.end.max(range.start));
    let span = ranges[first].start..ranges[last].end;
    let replaced = text[span.clone()]
        .split('\n')
        .map(&edit)
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = text.to_string();
    out.replace_range(span.clone(), &replaced);
    let end = span.start + replaced.len();
    (out, span.start..end)
}

/// 给选区加粗；选区自身或紧挨着的两侧已经带 `**` 就去掉标记。
/// 返回新正文与新选区。空选区会插入一对空标记，光标落在中间。
pub(crate) fn toggle_bold(text: &str, range: &Range<usize>) -> (String, Range<usize>) {
    let selected = &text[range.clone()];
    let mut out = text.to_string();
    if selected.len() >= 4 && selected.starts_with("**") && selected.ends_with("**") {
        let inner = selected[2..selected.len() - 2].to_string();
        out.replace_range(range.clone(), &inner);
        let end = range.start + inner.len();
        return (out, range.start..end);
    }
    if range.start >= 2
        && text.get(range.start - 2..range.start) == Some("**")
        && text.get(range.end..range.end + 2) == Some("**")
    {
        // 先删后面那对，前面的字节位置才不会跟着移动。
        out.replace_range(range.end..range.end + 2, "");
        out.replace_range(range.start - 2..range.start, "");
        return (out, range.start - 2..range.end - 2);
    }
    out.replace_range(range.clone(), &format!("**{selected}**"));
    (out, range.start + 2..range.end + 2)
}

/// 连续空行压成一行，行尾空格去掉，文末只留一个换行。
pub(crate) fn tidy_blank_lines(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut previous_blank = false;
    for line in text.split('\n') {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            if previous_blank {
                continue;
            }
            previous_blank = true;
        } else {
            previous_blank = false;
        }
        lines.push(trimmed.to_string());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let mut joined = lines.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

/// 正文字数与段落数。按导出器的分块统计：Markdown 标记、表格竖线、图片引用
/// 与区段标记都不计入，所以数出来的是「排到纸上有多少字」。
pub(crate) fn body_stats(markdown: &str) -> (usize, usize) {
    let mut characters = 0usize;
    let mut paragraphs = 0usize;
    for block in export::parse_markdown(markdown) {
        let text = match &block {
            export::MarkdownBlock::Title(text) | export::MarkdownBlock::Heading(_, text) => {
                text.clone()
            }
            export::MarkdownBlock::ListItem(text) => text.trim_start_matches('•').to_string(),
            export::MarkdownBlock::OrderedListItem { number, text } => {
                format!("{number}.{text}")
            }
            export::MarkdownBlock::Paragraph(text) => {
                paragraphs += 1;
                text.clone()
            }
            export::MarkdownBlock::Table { rows, .. } => rows
                .iter()
                .flat_map(|row| row.iter().cloned())
                .collect::<Vec<_>>()
                .join(""),
            export::MarkdownBlock::Image { .. }
            | export::MarkdownBlock::Marker(_)
            | export::MarkdownBlock::Html(_) => continue,
        };
        characters += export::plain_text(&text)
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .count();
    }
    (characters, paragraphs)
}

/// 今天的中文数字日期，即公文成文日期的写法：二〇二五年八月十一日。
pub(crate) fn chinese_today() -> String {
    const DIGITS: [char; 10] = ['〇', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    let now = chrono::Local::now();
    let year = now
        .format("%Y")
        .to_string()
        .chars()
        .filter_map(|ch| ch.to_digit(10))
        .map(|digit| DIGITS[digit as usize])
        .collect::<String>();
    let month = now.format("%m").to_string().parse::<usize>().unwrap_or(1);
    let day = now.format("%d").to_string().parse::<usize>().unwrap_or(1);
    format!(
        "{year}年{}月{}日",
        export::number_to_chinese(month),
        export::number_to_chinese(day)
    )
}

/// 编辑框记着的光标位置（字节）。这里**不看焦点**：点功能区按钮时焦点已经被
/// 按钮抢走了，但用户的意思显然是「对我刚才编辑的地方动手」，所以认 TextEdit
/// 自己存着的那个光标。从没点进过编辑框时返回 None，调用方按文末处理。
pub(crate) fn editor_cursor(ctx: &egui::Context, text: &str) -> Option<usize> {
    let range = egui::TextEdit::load_state(ctx, editor_id())?
        .cursor
        .char_range()?;
    Some(byte_at_char(text, range.primary.index.0))
}

/// 编辑框当前的选区（字节范围）。没有选区时首尾相同。
pub(crate) fn editor_selection(ctx: &egui::Context, text: &str) -> Option<Range<usize>> {
    let range = egui::TextEdit::load_state(ctx, editor_id())?
        .cursor
        .char_range()?;
    let primary = byte_at_char(text, range.primary.index.0);
    let secondary = byte_at_char(text, range.secondary.index.0);
    Some(primary.min(secondary)..primary.max(secondary))
}

/// 字符下标换算成字节位置；越界时取文末。
pub(crate) fn byte_at_char(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

impl DraftPage<'_> {
    /// 把区段标记（`<!-- [正文] -->` 等）插入审校稿：插到光标所在行的行首，
    /// 从没点进过编辑框时追加到文末。标记必须独占一行导出器才认，
    /// 正文标记只允许一个；附件标记可重复插入，每次都代表一份新附件。
    pub(crate) fn insert_section_marker(&mut self, ctx: &egui::Context, marker: &str, label: &str) {
        if self.doc.read_only() {
            return;
        }
        if marker != "<!-- [附件] -->"
            && self
                .doc
                .generated_markdown
                .lines()
                .any(|line| line.trim() == marker)
        {
            *self.status = format!("{label}已在稿中，不重复插入。");
            return;
        }
        let cursor = editor_cursor(ctx, &self.doc.generated_markdown);
        let text = &mut self.doc.generated_markdown;
        let pos = cursor.unwrap_or(text.len()).min(text.len());
        let line_start = text[..pos].rfind('\n').map_or(0, |index| index + 1);
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |index| line_start + index);
        let line_empty = text[line_start..line_end].trim().is_empty();
        let insertion = if line_empty {
            format!("{marker}\n")
        } else {
            format!("\n{marker}\n")
        };
        text.insert_str(line_start, &insertion);
        *self.status = format!("已插入{label}。");
        // 让编辑框下一次绘制时把光标挪到插入内容之后并滚动到位。
        self.doc.pending_source_jump = Some(line_start + insertion.len());
    }

    /// 把一段块级 Markdown 插进审校稿，返回插入内容自身的起始字节。
    ///
    /// 插入点取编辑框记着的光标（没有就用文末），前后按需补空行——插的是标题、
    /// 表格这类块级内容，紧贴着上一行会被 markdown 当成同一段，表格更是不成表。
    pub(crate) fn insert_block(&mut self, ctx: &egui::Context, markdown: &str) -> usize {
        let cursor = editor_cursor(ctx, &self.doc.generated_markdown);
        let text = &mut self.doc.generated_markdown;
        let pos = cursor.unwrap_or(text.len()).min(text.len());
        let lead = blank_line_padding(text[..pos].trim_end_matches(' '), true);
        let tail = blank_line_padding(text[pos..].trim_start_matches(' '), false);
        let insertion = format!("{lead}{markdown}{tail}");
        text.insert_str(pos, &insertion);
        // 光标落到插入内容的末尾，方便接着往下写。
        self.doc.pending_source_jump = Some(pos + lead.len() + markdown.len());
        pos + lead.len()
    }

    /// 在光标处插入一段行内文字（词库词条、日期、符号）。`back` 是插完之后
    /// 光标要往回退几个字节，用来把光标放进成对符号的中间。
    pub(crate) fn insert_inline(
        &mut self,
        ctx: &egui::Context,
        snippet: &str,
        back: usize,
        label: &str,
    ) {
        if self.doc.read_only() {
            return;
        }
        let cursor = editor_cursor(ctx, &self.doc.generated_markdown);
        let text = &mut self.doc.generated_markdown;
        let pos = cursor.unwrap_or(text.len()).min(text.len());
        text.insert_str(pos, snippet);
        self.doc.pending_source_jump = Some(pos + snippet.len() - back);
        *self.status = format!("已插入{label}。");
    }

    /// 光标所在行的标题层级，用来点亮「格式」分区里对应的那枚按钮。
    pub(crate) fn heading_level_at_cursor(&self, ctx: &egui::Context) -> Option<u8> {
        let text = &self.doc.generated_markdown;
        let cursor = editor_cursor(ctx, text)?;
        let ranges = line_ranges(text);
        let line = &text[ranges[line_at_byte(&ranges, cursor)].clone()];
        markdown_heading_level(line)
    }

    /// 把选区覆盖到的行设成指定标题层级；`level` 为 0 是降回正文。
    pub(crate) fn apply_heading(&mut self, ctx: &egui::Context, level: u8, label: &str) {
        self.apply_line_edit(
            ctx,
            |line| set_heading(line, level),
            &if level == 0 {
                "已降为正文。".to_string()
            } else {
                format!("已设为「{label}」这一级标题。")
            },
        );
    }

    /// 对选区覆盖到的每一行做同一件事（标题层级、项目符号）。
    /// 改完把光标放到改动范围的末尾，而不是选中整段——选中状态下随手一打字
    /// 就会把刚改好的几行整个替换掉。
    pub(crate) fn apply_line_edit(
        &mut self,
        ctx: &egui::Context,
        edit: impl Fn(&str) -> String,
        done: &str,
    ) {
        if self.doc.read_only() {
            return;
        }
        let text = self.doc.generated_markdown.clone();
        let range = editor_selection(ctx, &text).unwrap_or(text.len()..text.len());
        let (updated, span) = map_lines(&text, &range, edit);
        self.doc.generated_markdown = updated;
        self.doc.pending_source_jump = Some(span.end);
        *self.status = done.to_string();
    }

    /// 把选区覆盖的行切换为有序列表，并用列表前是否保留空行确定段内/独立模式。
    pub(crate) fn apply_ordered_list(&mut self, ctx: &egui::Context, inline: bool) {
        if self.doc.read_only() {
            return;
        }
        let text = self.doc.generated_markdown.clone();
        let range = editor_selection(ctx, &text).unwrap_or(text.len()..text.len());
        let ranges = line_ranges(&text);
        let first = line_at_byte(&ranges, range.start);
        let was_ordered = export::parse_ordered_item(&text[ranges[first].clone()]).is_some();
        let (mut updated, mut span) = map_lines(&text, &range, toggle_ordered);

        if !was_ordered && span.start > 0 {
            if inline {
                // 多个空行一律压回一个换行，让列表紧接上面的正文源码行。
                let mut run_start = span.start;
                while run_start > 0 && updated.as_bytes()[run_start - 1] == b'\n' {
                    run_start -= 1;
                }
                if span.start - run_start > 1 {
                    let removed = span.start - run_start - 1;
                    updated.replace_range(run_start..span.start - 1, "");
                    span = span.start - removed..span.end - removed;
                }
            } else if !updated[..span.start].ends_with("\n\n") {
                updated.insert(span.start, '\n');
                span = span.start + 1..span.end + 1;
            }
        }

        let affected_line = line_at_byte(&line_ranges(&updated), span.end);
        self.doc.generated_markdown = export::normalize_ordered_list_punctuation(&updated);
        let normalized_ranges = line_ranges(&self.doc.generated_markdown);
        self.doc.pending_source_jump = Some(
            normalized_ranges
                .get(affected_line)
                .map_or(self.doc.generated_markdown.len(), |range| range.end),
        );
        *self.status = if was_ordered {
            "已取消有序列表。".into()
        } else if inline {
            "已设为段内有序列表；圈号将在排版视图中自动生成。".into()
        } else {
            "已设为独立有序列表。".into()
        };
    }

    /// 给选中的文字加粗；已经加粗的再来一次就是取消。加粗之后保持选中，
    /// 与各家编辑器的主快捷键+B 一致。
    pub(crate) fn toggle_bold(&mut self, ctx: &egui::Context) {
        if self.doc.read_only() {
            return;
        }
        let text = self.doc.generated_markdown.clone();
        let Some(range) = editor_selection(ctx, &text) else {
            *self.status = "先在审校稿里选中要加粗的文字。".into();
            return;
        };
        let (updated, selection) = toggle_bold(&text, &range);
        let removed = updated.len() < text.len();
        self.doc.generated_markdown = updated;
        self.doc.pending_source_selection = Some(selection);
        *self.status = if range.is_empty() {
            "已插入一对加粗标记。".into()
        } else if removed {
            "已取消加粗。".into()
        } else {
            "已加粗。".into()
        };
    }
}

#[cfg(test)]
mod ordered_list_tests {
    use super::*;

    #[test]
    fn enter_continues_ordered_item_with_placeholder_marker() {
        let text = "正文\n1. 第一项";
        let (updated, cursor) = continue_ordered_list(text, text.len()).unwrap();
        assert_eq!(updated, "正文\n1. 第一项\n1. ");
        assert_eq!(cursor, updated.len());
    }

    #[test]
    fn enter_on_empty_item_exits_and_normalizes_the_group() {
        let text = "正文\n1. 第一项；\n1. 第二项，\n1. ";
        let (updated, cursor) = continue_ordered_list(text, text.len()).unwrap();
        assert_eq!(updated, "正文\n1. 第一项；\n1. 第二项。\n");
        assert_eq!(cursor, updated.len());
    }

    #[test]
    fn ordinary_lines_are_left_to_text_edit() {
        assert!(continue_ordered_list("普通正文", "普通正文".len()).is_none());
    }
}
