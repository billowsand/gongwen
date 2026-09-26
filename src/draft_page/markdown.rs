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
    let cells = split_row(line);
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let value = cell.trim().trim_matches(':');
            value.len() >= 3 && value.chars().all(|ch| ch == '-')
        })
}

pub(crate) fn table_column_count(line: &str) -> usize {
    split_row(line).len().max(1)
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
    let mut value = line.trim();
    if let Some(rest) = value.strip_prefix('|') {
        value = rest;
    }
    if let Some(rest) = value.strip_suffix('|') {
        value = rest;
    }
    value
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

/// 列表项开关：已经是 `- ` / `* ` 开头就去掉，否则加上。公文不分有序无序，
/// `- ` 与 `1. ` 成文时都按设置里的列表编号样式排（见 `export::parse_list_item`）。
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

/// 在有序列表项中按回车：非空项按「当前序号+1」续写下一项，光标后方同一连续
/// 列表组里的已有项序号依次后移，避免与新行重号；空项再次回车则移除占位并
/// 结束列表。返回修改后的正文和新光标字节位置；非列表行返回 None，
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
    let (number, content) = export::parse_ordered_item(line)?;
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
    let marker = format!("\n{}{}. ", " ".repeat(indent_len.min(3)), number + 1);
    let mut updated = text.to_string();
    updated.insert_str(cursor, &marker);
    // 光标后方同一连续列表组（相同缩进、紧邻的有序列表行，遇空行或非列表行即止）
    // 里已有项的序号依次重排为新行序号 +1、+2……，只重写数字部分，
    // 各行的缩进、分隔符与内容原样保留。
    let mut expected = number + 2;
    let mut pos = line_end + marker.len();
    while pos < updated.len() && updated.as_bytes()[pos] == b'\n' {
        let item_start = pos + 1;
        let item_end = updated[item_start..]
            .find('\n')
            .map_or(updated.len(), |index| item_start + index);
        let item = &updated[item_start..item_end];
        let item_indent = item.len() - item.trim_start_matches(' ').len();
        if item_indent != indent_len || export::parse_ordered_item(item).is_none() {
            break;
        }
        let digits = item[item_indent..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let item_len = item.len();
        let number_start = item_start + item_indent;
        let replacement = expected.to_string();
        updated.replace_range(number_start..number_start + digits, &replacement);
        pos = item_start + item_len - digits + replacement.len();
        expected += 1;
    }
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
            export::MarkdownBlock::OrderedListItem { number, text } => {
                format!("{number}.{text}")
            }
            export::MarkdownBlock::Paragraph(text)
            | export::MarkdownBlock::Aligned { text, .. } => {
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

/// 把 `line` 插到 `pos` 所在行的行首，把原来那一行整体顶下去；返回插入内容
/// 本身（不含为独占一行补出来的换行）在新 `text` 里的字节范围。
///
/// 区段标记、表题这些「必须独占一行」的东西都走这里。插在行首而不是光标处，
/// 是为了让光标停在某一行上时，插进来的这行正好落到它上方：表题因此落到表格
/// 首行之上，正是解析器认表题的位置。所在行本来就空就不再补空行。
fn splice_own_line(text: &mut String, pos: usize, line: &str) -> Range<usize> {
    let pos = pos.min(text.len());
    let line_start = text[..pos].rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |index| line_start + index);
    // 所在行本来就空，或者它上面已经隔着一个空行，就不用再补——补出来是连着
    // 三个换行，源码上白多一行。
    let padded = !text[line_start..line_end].trim().is_empty()
        && line_start > 0
        && !text[..line_start].ends_with("\n\n");
    let insertion = if padded {
        format!("\n{line}\n")
    } else {
        format!("{line}\n")
    };
    // 补在前面的那个换行（有就一个字节）之后才是内容本身。
    let start = line_start + insertion.len() - line.len() - 1;
    text.insert_str(line_start, &insertion);
    start..start + line.len()
}

/// 居中 / 居右标记的源码写法。
pub(crate) fn align_marker(align: export::LineAlign) -> &'static str {
    match align {
        export::LineAlign::Center => "<!-- [居中] -->",
        export::LineAlign::Right => "<!-- [居右] -->",
    }
}

/// 管着第 `line` 行的那个对齐标记：从这一行往上找，空行之前遇到的第一个
/// 对齐标记（可以就是这一行本身）。返回标记所在行号与对齐方式。
pub(crate) fn governing_align_marker(
    text: &str,
    ranges: &[Range<usize>],
    line: usize,
) -> Option<(usize, export::LineAlign)> {
    (0..=line.min(ranges.len().saturating_sub(1)))
        .rev()
        .map(|index| (index, text[ranges[index].clone()].trim()))
        .take_while(|(_, source)| !source.is_empty())
        .find_map(|(index, source)| export::parse_align_marker(source).map(|align| (index, align)))
}

/// 对齐动作做了什么，状态栏据此报告。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlignEdit {
    Added,
    Switched,
    Removed,
}

/// 把选区覆盖的几行设成居中 / 居右，返回改后的全文、光标该落的位置与做了什么。
///
/// - 这几行已在同一种对齐区里：删掉那个标记，恢复正常排版（再点一次即取消）；
/// - 在另一种对齐区里：就地把标记换掉；
/// - 不在对齐区里：在第一行上方插一行标记；最后一行下面紧跟着非空行时补一个
///   空行，免得对齐区一路吞到后面的正文。
pub(crate) fn toggle_align_region(
    text: &str,
    selection: &Range<usize>,
    align: export::LineAlign,
) -> (String, usize, AlignEdit) {
    let ranges = line_ranges(text);
    let first = line_at_byte(&ranges, selection.start);
    let last = line_at_byte(&ranges, selection.end.max(selection.start));
    let marker = align_marker(align);
    let mut out = text.to_string();
    if let Some((line, current)) = governing_align_marker(text, &ranges, first) {
        let range = ranges[line].clone();
        if current == align {
            // 连同行尾换行一起删；标记在末行时删掉前面那个换行。
            let removal = if range.end < text.len() {
                range.start..range.end + text[range.end..].find('\n').map_or(0, |at| at + 1)
            } else {
                range.start.saturating_sub(1)..range.end
            };
            let removed = removal.len();
            out.replace_range(removal.clone(), "");
            let caret = ranges[last]
                .end
                .saturating_sub(removed)
                .max(removal.start)
                .min(out.len());
            return (out, caret, AlignEdit::Removed);
        }
        out.replace_range(range.clone(), marker);
        let caret = ranges[last].end + marker.len() - range.len();
        return (out, caret, AlignEdit::Switched);
    }
    // 先补后面的空行，再插前面的标记：前面的插入会把后面的偏移整体后移。
    let region_end = ranges[last].end;
    let next_is_text = ranges
        .get(last + 1)
        .is_some_and(|next| !text[next.clone()].trim().is_empty());
    if next_is_text {
        out.insert(region_end, '\n');
    }
    let start = ranges[first].start;
    out.insert_str(start, &format!("{marker}\n"));
    (out, region_end + marker.len() + 1, AlignEdit::Added)
}

impl DraftPage<'_> {
    /// 光标所在行受哪种对齐标记管，用来点亮「格式」分区里对应的按钮。
    pub(crate) fn align_at_cursor(&self, ctx: &egui::Context) -> Option<export::LineAlign> {
        let text = &self.doc.generated_markdown;
        let cursor = editor_cursor(ctx, text)?;
        let ranges = line_ranges(text);
        governing_align_marker(text, &ranges, line_at_byte(&ranges, cursor)).map(|(_, align)| align)
    }

    /// 把选区覆盖的几行设成居中 / 居右；已经是同一种对齐的再点一次取消。
    pub(crate) fn toggle_align(&mut self, ctx: &egui::Context, align: export::LineAlign) {
        if self.doc.read_only() {
            return;
        }
        let text = self.doc.generated_markdown.clone();
        let Some(range) = editor_selection(ctx, &text) else {
            *self.status = "先在审校稿里把光标放到要对齐的行上，或选中这几行。".into();
            return;
        };
        let ranges = line_ranges(&text);
        let lines = line_at_byte(&ranges, range.start)..=line_at_byte(&ranges, range.end);
        if lines
            .clone()
            .all(|line| text[ranges[line].clone()].trim().is_empty())
        {
            *self.status = "光标所在是空行：把光标放到要对齐的文字行上，或选中这几行。".into();
            return;
        }
        let (updated, caret, edit) = toggle_align_region(&text, &range, align);
        self.doc.generated_markdown = updated;
        self.doc.pending_source_jump = Some(caret);
        let label = match align {
            export::LineAlign::Center => "居中",
            export::LineAlign::Right => "居右",
        };
        *self.status = match edit {
            AlignEdit::Added => format!("已设为{label}：标记下方直到空行的各行都{label}排。"),
            AlignEdit::Switched => format!("已改为{label}。"),
            AlignEdit::Removed => format!("已取消{label}，恢复正常排版。"),
        };
    }

    /// 把区段标记（`<!-- [正文] -->` 等）插入审校稿：插到光标所在行的行首，
    /// 从没点进过编辑框时追加到文末。标记必须独占一行导出器才认，
    /// 正文等区段标记全篇只允许一个；附件与附录标记可重复插入，
    /// 每次都代表一份新材料；不编号标记每个不编号的标题前各写一个，部分标记
    /// 从别的区段切回部分段时也要再写一次，两者同样可重复。
    pub(crate) fn insert_section_marker(&mut self, ctx: &egui::Context, marker: &str, label: &str) {
        if self.doc.read_only() {
            return;
        }
        if ![
            "<!-- [附件] -->",
            "<!-- [附录] -->",
            "<!-- [部分] -->",
            "<!-- [不编号] -->",
        ]
        .contains(&marker)
            && self
                .doc
                .generated_markdown
                .lines()
                .any(|line| line.trim() == marker)
        {
            *self.status = format!("{label}已在稿中，不重复插入。");
            return;
        }
        let inserted = self.insert_own_line(ctx, marker);
        *self.status = format!("已插入{label}。");
        // 让编辑框下一次绘制时把光标挪到插入内容之后并滚动到位。
        // `+ 1` 跨过行尾那个换行，落到被顶下去的那一行行首。
        self.doc.pending_source_jump = Some(inserted.end + 1);
    }

    /// 研究报告的表题 `表：`：插到光标所在行的行首，把原来那一行顶下去，
    /// 光标停在冒号后面直接写题名。
    ///
    /// 表题必须独占一行、紧挨着表格，解析器才把它认成表题而不是正文
    /// （见 mdx 的 `take_leading_table_caption`）。插在行首而不是光标处，
    /// 是为了让光标落在表格首行时表题正好落到表格上方那一行。
    /// 表号由 LaTeX 的表格计数器生成，题名里不写。
    pub(crate) fn insert_table_caption(&mut self, ctx: &egui::Context) {
        if self.doc.read_only() {
            return;
        }
        let inserted = self.insert_own_line(ctx, "表：");
        *self.status = "已插入表题：写在表格上一行，冒号后填题名，不要自己编表号。".into();
        self.doc.pending_source_jump = Some(inserted.end);
    }

    /// 把一行内容插到光标所在行的行首；从没点进过编辑框时追加到文末。
    /// 返回插入内容本身（不含为独占一行补出来的换行）的字节范围。
    fn insert_own_line(&mut self, ctx: &egui::Context, line: &str) -> Range<usize> {
        let cursor = editor_cursor(ctx, &self.doc.generated_markdown);
        let text = &mut self.doc.generated_markdown;
        let pos = cursor.unwrap_or(text.len());
        splice_own_line(text, pos, line)
    }

    /// 研究报告的锚点 ` {#}`：追加到光标所在行的行尾，光标落进花括号里，
    /// 紧接着就能敲 id。锚点只在标题、表题或图片行的行尾生效，导出为
    /// LaTeX 的 `\label`，供 `{@id}` 交叉引用。
    pub(crate) fn insert_label(&mut self, ctx: &egui::Context) {
        if self.doc.read_only() {
            return;
        }
        let cursor = editor_cursor(ctx, &self.doc.generated_markdown);
        let text = &mut self.doc.generated_markdown;
        let pos = cursor.unwrap_or(text.len()).min(text.len());
        let line_start = text[..pos].rfind('\n').map_or(0, |index| index + 1);
        let line_end = text[pos..]
            .find('\n')
            .map_or(text.len(), |index| pos + index);
        if export::crossref::split_label(&text[line_start..line_end])
            .1
            .is_some()
        {
            *self.status = "本行已有锚点，直接改花括号里的 id 即可。".into();
            return;
        }
        text.insert_str(line_end, " {#}");
        // 光标落进花括号内，直接敲 id。
        self.doc.pending_source_jump = Some(line_end + " {#".len());
        *self.status = "已插入锚点：在花括号里写 id（字母开头，可含数字与 : . - _）。".into();
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
        // `toggle_ordered` 逐行加的都是 `1. `；整组一起重排成 1. 2. 3.，
        // 源码里才看得出第几项，和回车续写出来的编号也是同一套。
        self.doc.generated_markdown =
            export::renumber_ordered_groups(&export::normalize_ordered_list_punctuation(&updated));
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
mod align_tests {
    use super::*;
    use export::LineAlign::{Center, Right};

    #[test]
    fn marks_the_selected_lines_and_closes_the_region_before_following_text() {
        let text = "前文。\n居中甲\n居中乙\n后文。";
        let start = "前文。\n".len();
        let end = "前文。\n居中甲\n居中".len();
        let (updated, caret, edit) = toggle_align_region(text, &(start..end), Center);
        assert_eq!(edit, AlignEdit::Added);
        assert_eq!(updated, "前文。\n<!-- [居中] -->\n居中甲\n居中乙\n\n后文。");
        assert_eq!(caret, "前文。\n<!-- [居中] -->\n居中甲\n居中乙".len());
        // 解析出来正好是这两行，后文恢复正常段落。
        let blocks = export::parse_markdown(&updated);
        assert!(
            matches!(blocks.last(), Some(export::MarkdownBlock::Paragraph(p)) if p == "后文。")
        );
    }

    #[test]
    fn no_extra_blank_line_when_the_region_already_ends() {
        let text = "居右一行\n\n后文。";
        let (updated, _, _) = toggle_align_region(text, &(0..0), Right);
        assert_eq!(updated, "<!-- [居右] -->\n居右一行\n\n后文。");
        let (updated, _, _) = toggle_align_region("末行", &(0..0), Right);
        assert_eq!(updated, "<!-- [居右] -->\n末行");
    }

    #[test]
    fn same_alignment_again_removes_the_marker_and_other_one_switches_it() {
        let text = "前文。\n\n<!-- [居中] -->\n甲\n乙\n\n后文。";
        let on_second = "前文。\n\n<!-- [居中] -->\n甲\n".len();
        let (switched, _, edit) = toggle_align_region(text, &(on_second..on_second), Right);
        assert_eq!(edit, AlignEdit::Switched);
        assert_eq!(switched, "前文。\n\n<!-- [居右] -->\n甲\n乙\n\n后文。");
        let (removed, caret, edit) = toggle_align_region(&switched, &(on_second..on_second), Right);
        assert_eq!(edit, AlignEdit::Removed);
        assert_eq!(removed, "前文。\n\n甲\n乙\n\n后文。");
        assert_eq!(caret, "前文。\n\n甲\n乙".len());
    }

    #[test]
    fn governing_marker_stops_at_a_blank_line() {
        let text = "<!-- [居中] -->\n甲\n\n乙";
        let ranges = line_ranges(text);
        assert_eq!(governing_align_marker(text, &ranges, 1), Some((0, Center)));
        assert_eq!(governing_align_marker(text, &ranges, 3), None);
    }
}

#[cfg(test)]
mod ordered_list_tests {
    use super::*;

    #[test]
    fn enter_continues_ordered_item_with_incremented_marker() {
        let text = "正文\n1. 第一项";
        let (updated, cursor) = continue_ordered_list(text, text.len()).unwrap();
        assert_eq!(updated, "正文\n1. 第一项\n2. ");
        assert_eq!(cursor, updated.len());
    }

    #[test]
    fn enter_in_middle_renumbers_following_items() {
        let text = "1. 第一项\n2. 第二项\n3. 第三项";
        let cursor = "1. 第一项".len();
        let (updated, cursor) = continue_ordered_list(text, cursor).unwrap();
        assert_eq!(updated, "1. 第一项\n2. \n3. 第二项\n4. 第三项");
        assert_eq!(cursor, "1. 第一项\n2. ".len());
    }

    #[test]
    fn enter_at_last_item_only_appends() {
        // 列表中间被空行断开时，只重排光标后方紧邻的连续组，下一组不动。
        let text = "1. 第一项\n\n5. 另一组";
        let cursor = "1. 第一项".len();
        let (updated, _) = continue_ordered_list(text, cursor).unwrap();
        assert_eq!(updated, "1. 第一项\n2. \n\n5. 另一组");
    }

    #[test]
    fn enter_keeps_separator_and_content_of_renumbered_items() {
        let text = "2. 甲\n3.  乙\n4. 丙";
        let cursor = "2. 甲".len();
        let (updated, _) = continue_ordered_list(text, cursor).unwrap();
        assert_eq!(updated, "2. 甲\n3. \n4.  乙\n5. 丙");
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

/// 独占一行的插入（区段标记、表题）落点。
#[cfg(test)]
mod own_line_tests {
    use super::*;

    /// 表题要落在表格首行之上：光标停在表格里时，插进来的这行顶开它，
    /// 表题与表格紧邻——正是解析器认表题的位置。
    #[test]
    fn an_own_line_goes_above_the_line_the_cursor_sits_on() {
        let mut text = "前一段。\n\n| 甲 | 乙 |\n| --- | --- |\n".to_string();
        let cursor = text.find("| 甲").expect("表格首行") + "| 甲".len();
        let inserted = splice_own_line(&mut text, cursor, "表：");
        assert_eq!(text, "前一段。\n\n表：\n| 甲 | 乙 |\n| --- | --- |\n");
        assert_eq!(&text[inserted.clone()], "表：");
        // 光标停在冒号后面，接着就能写题名。
        assert_eq!(&text[..inserted.end], "前一段。\n\n表：");
    }

    /// 所在行本来就空就不再补空行；返回的范围仍精确框住插入的那串。
    #[test]
    fn an_own_line_does_not_pad_an_already_empty_line() {
        let mut text = "前一段。\n\n".to_string();
        let end = text.len();
        let inserted = splice_own_line(&mut text, end, "<!-- [正文] -->");
        assert_eq!(text, "前一段。\n\n<!-- [正文] -->\n");
        assert_eq!(&text[inserted.clone()], "<!-- [正文] -->");
        // 区段标记插完把光标送到下一行行首：跨过行尾那个换行就是文末。
        assert_eq!(inserted.end + 1, text.len());
    }

    /// 所在行有字、上面又紧挨着别的字时补一个空行，免得连成同一段；上面已经
    /// 隔着空行就不再补，省得源码里连着三个换行。
    #[test]
    fn an_own_line_pads_only_when_the_line_above_has_text() {
        let mut text = "前一段。\n下一段。".to_string();
        let cursor = text.find("下一段").expect("第二段");
        let inserted = splice_own_line(&mut text, cursor, "表：");
        assert_eq!(text, "前一段。\n\n表：\n下一段。");
        assert_eq!(&text[inserted], "表：");

        let mut spaced = "前一段。\n\n下一段。".to_string();
        let cursor = spaced.find("下一段").expect("第二段");
        splice_own_line(&mut spaced, cursor, "表：");
        assert_eq!(spaced, "前一段。\n\n表：\n下一段。");
    }

    /// 光标位置越界（改完正文还没重绘时编辑框记着的旧位置）不能 panic：
    /// 按文末算，插在末行之上。
    #[test]
    fn an_own_line_clamps_a_stale_cursor() {
        let mut text = "正文".to_string();
        let inserted = splice_own_line(&mut text, 9_999, "表：");
        assert_eq!(text, "表：\n正文");
        assert_eq!(&text[inserted], "表：");
    }
}
