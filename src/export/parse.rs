//! Markdown 解析：块/节/表格/图片识别与源码定位。
//!
//! 由 src/export/mod.rs 拆分而来：本文件是模块 `export::parse`，与其它子模块共享
//! `export` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::{attachment_title_name, clean_heading_number, legacy_attachment_label};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkdownBlock {
    Title(String),
    Heading(u8, String),
    Paragraph(String),
    ListItem(String),
    /// 独立有序列表的一项。源码沿用标准 Markdown 的 `1. 内容`，`number`
    /// 是按同一组连续列表自动计算后的显示序号。
    OrderedListItem {
        number: usize,
        text: String,
    },
    /// GFM 表格。`aligns` 是分隔行里写明的列对齐，与列一一对应；
    /// 没写冒号的列是 `Auto`，交给智能列宽按内容判定。
    Table {
        rows: Vec<Vec<String>>,
        aligns: Vec<ColumnAlign>,
    },
    Marker(MarkdownSection),
    Html(String),
    /// 独占一行的图片引用 `![alt](src)`，`src` 为相对路径（如 `images/xxx.png`）。
    Image {
        alt: String,
        src: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownSection {
    Body,
    Attachment,
}

/// GFM 表格分隔行里写明的列对齐：`---`、`:---`、`:---:`、`---:`。
/// `Auto` 表示没写冒号，由 `table::analyze_table` 按内容判定（短数字列居中、
/// 长文本左对齐）；写了冒号就以冒号为准。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ColumnAlign {
    #[default]
    Auto,
    Left,
    Center,
    Right,
}

impl ColumnAlign {
    /// 分隔行里的一格。
    pub(crate) fn parse(cell: &str) -> Self {
        let cell = cell.trim();
        match (cell.starts_with(':'), cell.ends_with(':')) {
            (true, true) => Self::Center,
            (true, false) => Self::Left,
            (false, true) => Self::Right,
            (false, false) => Self::Auto,
        }
    }

    /// 分隔行这一格最少要多宽。GFM 的分隔行判定要求**去掉冒号之后**仍有三条
    /// 短横，写成 `:-:` 的那一格谁都不认，整张表会退化成普通段落。
    pub(crate) fn min_width(self) -> usize {
        match self {
            Self::Auto => 3,
            Self::Left | Self::Right => 4,
            Self::Center => 5,
        }
    }

    /// 按给定列宽写出分隔行的一格。
    pub(crate) fn render(self, width: usize) -> String {
        let width = width.max(self.min_width());
        match self {
            Self::Auto => "-".repeat(width),
            Self::Left => format!(":{}", "-".repeat(width - 1)),
            Self::Center => format!(":{}:", "-".repeat(width - 2)),
            Self::Right => format!("{}:", "-".repeat(width - 1)),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动（按内容判定）",
            Self::Left => "左对齐",
            Self::Center => "居中",
            Self::Right => "右对齐",
        }
    }
}

/// 一个 Markdown 块，外加它在源码中的字节范围。界面预览据此在点击版式时
/// 回跳到对应的 Markdown 原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocatedBlock {
    pub(crate) block: MarkdownBlock,
    pub(crate) range: std::ops::Range<usize>,
}

pub(crate) fn parse_markdown(markdown: &str) -> Vec<MarkdownBlock> {
    parse_markdown_located(markdown)
        .into_iter()
        .map(|located| located.block)
        .collect()
}

/// 与 `parse_markdown` 同一套块序列，另外给出每个块在源码里的 1-based 起始行号。
/// 行号会随 `\GwaTail{行号}` 写进 TeX，编译后由孤行探针原样回传，审校提示因此
/// 能直接点到 Markdown 的哪一段，不必再去猜段落序号。
pub(crate) fn parse_markdown_with_lines(markdown: &str) -> (Vec<MarkdownBlock>, Vec<usize>) {
    let located = parse_markdown_located(markdown);
    let starts: Vec<usize> = source_lines(markdown)
        .into_iter()
        .map(|(offset, _)| offset)
        .collect();
    let mut blocks = Vec::with_capacity(located.len());
    let mut lines = Vec::with_capacity(located.len());
    for item in located {
        lines.push(line_of(&starts, item.range.start));
        blocks.push(item.block);
    }
    (blocks, lines)
}

/// 字节偏移落在第几行（1-based）：最后一个不超过它的行首。
pub(crate) fn line_of(line_starts: &[usize], offset: usize) -> usize {
    line_starts.partition_point(|start| *start <= offset).max(1)
}

/// 反查：起始行号是 `line` 的那个块，占用源码的哪一段字节。
///
/// 孤行提示带着 `\GwaTail` 里回传的行号，要点击定位就得换回字节范围。这里直接
/// 复用 `parse_markdown_with_lines` 的切块结果，跟导出时的分段口径完全一致，
/// 不会出现"提示说第 5 行、选中的却是隔壁那段"。
pub(crate) fn block_span_for_line(markdown: &str, line: usize) -> Option<std::ops::Range<usize>> {
    let starts: Vec<usize> = source_lines(markdown)
        .into_iter()
        .map(|(offset, _)| offset)
        .collect();
    parse_markdown_located(markdown)
        .into_iter()
        .find(|located| line_of(&starts, located.range.start) == line)
        .map(|located| located.range)
}

/// 逐行切分并记录每行的起始字节；行内容与 `str::lines` 一致（去掉行尾 \r\n）。
pub(crate) fn source_lines(markdown: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for piece in markdown.split_inclusive('\n') {
        let line = piece.strip_suffix('\n').unwrap_or(piece);
        let line = line.strip_suffix('\r').unwrap_or(line);
        lines.push((offset, line));
        offset += piece.len();
    }
    lines
}

/// 与 `parse_markdown` 同一套规则，另外记录每个块占用的源码字节范围：
/// 段落是合并的那几行，表格是表头到最后一行数据，其余就是它自己那一行。
pub(crate) fn parse_markdown_located(markdown: &str) -> Vec<LocatedBlock> {
    let mut blocks: Vec<LocatedBlock> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut paragraph_range = 0..0;
    let mut in_html_block = false;
    let flush = |paragraph: &mut Vec<String>,
                 range: &mut std::ops::Range<usize>,
                 blocks: &mut Vec<LocatedBlock>| {
        if !paragraph.is_empty() {
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Paragraph(paragraph.join(" ")),
                range: range.clone(),
            });
            paragraph.clear();
        }
    };

    let lines = source_lines(markdown);
    let mut index = 0usize;
    while index < lines.len() {
        let (start, raw) = lines[index];
        let line = raw.trim();
        let span = start..start + raw.len();
        if in_html_block {
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Html(line.to_string()),
                range: span,
            });
            if line.starts_with("</div") {
                in_html_block = false;
            }
            index += 1;
            continue;
        }
        if line.is_empty() {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
        } else if let Some(section) = parse_section_marker(line) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Marker(section),
                range: span,
            });
        } else if let Some((level, text)) = parse_heading(line) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            let block = if level == 1 {
                MarkdownBlock::Title(text.to_string())
            } else {
                MarkdownBlock::Heading(level, clean_heading_number(text))
            };
            blocks.push(LocatedBlock { block, range: span });
        } else if let Some((alt, src)) = parse_image(line) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Image { alt, src },
                range: span,
            });
        } else if line.starts_with("<div") {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Html(line.to_string()),
                range: span,
            });
            in_html_block = true;
        } else if index + 1 < lines.len()
            && is_table_row(line)
            && is_table_separator(lines[index + 1].1.trim())
        {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            let mut rows = vec![parse_table_row(line)];
            // 分隔行不进正文，但它的冒号决定各列对齐。
            let mut aligns = parse_table_row(lines[index + 1].1.trim())
                .iter()
                .map(|cell| ColumnAlign::parse(cell))
                .collect::<Vec<_>>();
            let mut end = lines[index + 1].0 + lines[index + 1].1.len();
            index += 2;
            while index < lines.len() && is_table_row(lines[index].1.trim()) {
                rows.push(parse_table_row(lines[index].1.trim()));
                end = lines[index].0 + lines[index].1.len();
                index += 1;
            }
            let column_count = rows.first().map_or(0, Vec::len);
            for row in &mut rows {
                row.resize(column_count, String::new());
                row.truncate(column_count);
            }
            aligns.resize(column_count, ColumnAlign::Auto);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::Table { rows, aligns },
                range: start..end,
            });
            continue;
        } else if let Some((start_number, _)) = parse_ordered_item(line) {
            // 连续的有序列表行作为一组处理：既要统一末尾标点，也要只认首项
            // 的起始序号，后续源码可以像常见 Markdown 写法一样全部写 `1.`。
            let mut group_end = span.end;
            let mut items = Vec::new();
            let first_item = index;
            while index < lines.len() {
                let (item_start, raw_item) = lines[index];
                let Some((_, text)) = parse_ordered_item(raw_item.trim()) else {
                    break;
                };
                items.push(text.to_string());
                group_end = item_start + raw_item.len();
                index += 1;
            }
            let items = normalize_ordered_item_punctuation(&items);
            if !paragraph.is_empty() {
                // 正文之后没有空行：源码仍逐项换行，成文时把它们接回同一自然段，
                // 使用圈号且不额外插入空格。
                let tail = items
                    .iter()
                    .enumerate()
                    .map(|(offset, text)| {
                        format!("{}{text}", circled_number(start_number + offset))
                    })
                    .collect::<String>();
                paragraph
                    .last_mut()
                    .expect("paragraph is not empty")
                    .push_str(&tail);
                paragraph_range.end = group_end;
            } else {
                for (offset, text) in items.into_iter().enumerate() {
                    let (line_start, raw_item) = lines[first_item + offset];
                    blocks.push(LocatedBlock {
                        block: MarkdownBlock::OrderedListItem {
                            number: start_number + offset,
                            text,
                        },
                        range: line_start..line_start + raw_item.len(),
                    });
                }
            }
            continue;
        } else if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            flush(&mut paragraph, &mut paragraph_range, &mut blocks);
            blocks.push(LocatedBlock {
                block: MarkdownBlock::ListItem(format!("• {}", text.trim())),
                range: span,
            });
        } else {
            if paragraph.is_empty() {
                paragraph_range = span.clone();
            }
            paragraph_range.end = span.end;
            paragraph.push(line.to_string());
        }
        index += 1;
    }
    flush(&mut paragraph, &mut paragraph_range, &mut blocks);
    normalize_legacy_attachments(blocks)
}

/// 标准 Markdown 有序列表项：允许行首最多三个空格，点号后必须有空白。
/// 空内容仍返回，编辑器借此识别按第二次回车退出列表的占位行。
pub(crate) fn parse_ordered_item(line: &str) -> Option<(usize, &str)> {
    let leading = line.len() - line.trim_start_matches(' ').len();
    if leading > 3 {
        return None;
    }
    let line = &line[leading..];
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let number = line[..digits].parse().ok()?;
    let rest = line[digits..].strip_prefix('.')?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some((number, rest.trim_start()))
}

fn terminal_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ',' | '，'
            | ';'
            | '；'
            | '.'
            | '．'
            | '。'
            | '｡'
            | '!'
            | '！'
            | '?'
            | '？'
            | '、'
            | '､'
            | ':'
            | '：'
            | '…'
            | '—'
    )
}

/// 一组列表只允许两种标点风格：任一非末项以分号收尾时，中间统一分号、
/// 末项句号；否则所有项统一句号。只观察末尾标点串，不会把正文内部的分号
/// 当成风格信号。
pub(crate) fn normalize_ordered_item_punctuation(items: &[String]) -> Vec<String> {
    let semicolon_style = items
        .iter()
        .take(items.len().saturating_sub(1))
        .any(|item| {
            item.trim_end()
                .chars()
                .rev()
                .take_while(|ch| terminal_punctuation(*ch))
                .any(|ch| matches!(ch, ';' | '；'))
        });
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let trimmed = item.trim_end();
            if trimmed.is_empty() {
                return String::new();
            }
            let body = trimmed.trim_end_matches(terminal_punctuation);
            let punctuation = if semicolon_style && index + 1 < items.len() {
                '；'
            } else {
                '。'
            };
            format!("{body}{punctuation}")
        })
        .collect()
}

pub(crate) fn circled_number(number: usize) -> String {
    const CIRCLED: [char; 20] = [
        '①', '②', '③', '④', '⑤', '⑥', '⑦', '⑧', '⑨', '⑩', '⑪', '⑫', '⑬', '⑭', '⑮', '⑯', '⑰', '⑱',
        '⑲', '⑳',
    ];
    CIRCLED
        .get(number.saturating_sub(1))
        .map(char::to_string)
        .unwrap_or_else(|| format!("({number})"))
}

/// 把 Markdown 源码中的每组连续有序列表就地规范成同一种末尾标点风格。
/// 仅重写列表项正文，保留用户写下的序号、缩进、空行与换行结尾。
pub(crate) fn normalize_ordered_list_punctuation(markdown: &str) -> String {
    let lines = markdown.split('\n').collect::<Vec<_>>();
    let mut output = Vec::with_capacity(lines.len());
    let mut index = 0usize;
    while index < lines.len() {
        let raw = lines[index].strip_suffix('\r').unwrap_or(lines[index]);
        if parse_ordered_item(raw.trim_end()).is_none() {
            output.push(lines[index].to_string());
            index += 1;
            continue;
        }

        let first = index;
        let mut contents = Vec::new();
        let mut prefixes = Vec::new();
        let mut crlf = Vec::new();
        while index < lines.len() {
            let raw = lines[index].strip_suffix('\r').unwrap_or(lines[index]);
            let Some((_, content)) = parse_ordered_item(raw.trim_end()) else {
                break;
            };
            let content_start = content.as_ptr() as usize - raw.as_ptr() as usize;
            prefixes.push(raw[..content_start].to_string());
            contents.push(content.to_string());
            crlf.push(lines[index].ends_with('\r'));
            index += 1;
        }
        let normalized = normalize_ordered_item_punctuation(&contents);
        for offset in 0..normalized.len() {
            let suffix = if crlf[offset] { "\r" } else { "" };
            output.push(format!(
                "{}{}{suffix}",
                prefixes[offset], normalized[offset]
            ));
        }
        debug_assert_eq!(output.len(), index, "group beginning at line {first}");
    }
    output.join("\n")
}

/// 把旧版附件语法转换为统一的内部结构：
///
/// ```text
/// <!-- [附件] -->       <!-- [附件] -->
/// # 附件1          =>  # 附件正式标题
/// ## 附件正式标题      ## 附件内一级标题
/// ### 附件内一级标题
/// ```
///
/// 新版中每个附件标记就是一份附件的起点，附件标题和内容标题使用与正文相同的
/// Markdown 层级。这里只做读取期兼容，不改写用户原文；预览和导出因此只需维护
/// 一套规范结构。
pub(crate) fn normalize_legacy_attachments(blocks: Vec<LocatedBlock>) -> Vec<LocatedBlock> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum LegacyState {
        None,
        AwaitTitle,
        Content,
    }

    let mut out = Vec::with_capacity(blocks.len());
    let mut in_attachment = false;
    let mut at_attachment_start = false;
    let mut legacy = LegacyState::None;

    for mut located in blocks {
        match &located.block {
            MarkdownBlock::Marker(MarkdownSection::Attachment) => {
                in_attachment = true;
                at_attachment_start = true;
                legacy = LegacyState::None;
                out.push(located);
            }
            MarkdownBlock::Marker(MarkdownSection::Body) => {
                in_attachment = false;
                at_attachment_start = false;
                legacy = LegacyState::None;
                out.push(located);
            }
            MarkdownBlock::Title(text)
                if in_attachment && legacy_attachment_label(text).is_some() =>
            {
                // 旧格式只在第一份附件前放一个区段标记，后续 `# 附件N` 同时承担
                // 分隔作用；规范化时为后续附件补一个仅用于内部处理的标记。
                if !at_attachment_start {
                    out.push(LocatedBlock {
                        block: MarkdownBlock::Marker(MarkdownSection::Attachment),
                        range: located.range.start..located.range.start,
                    });
                }
                at_attachment_start = false;
                if let Some(name) = attachment_title_name(text) {
                    located.block = MarkdownBlock::Title(name);
                    out.push(located);
                    legacy = LegacyState::Content;
                } else {
                    legacy = LegacyState::AwaitTitle;
                }
            }
            MarkdownBlock::Heading(2, text)
                if in_attachment && legacy == LegacyState::AwaitTitle =>
            {
                located.block = MarkdownBlock::Title(text.clone());
                out.push(located);
                legacy = LegacyState::Content;
                at_attachment_start = false;
            }
            MarkdownBlock::Heading(level, text)
                if in_attachment && legacy == LegacyState::Content && *level > 2 =>
            {
                located.block = MarkdownBlock::Heading(*level - 1, text.clone());
                out.push(located);
                at_attachment_start = false;
            }
            _ => {
                if in_attachment && !matches!(located.block, MarkdownBlock::Html(_)) {
                    at_attachment_start = false;
                }
                out.push(located);
            }
        }
    }
    out
}

pub(crate) fn is_table_row(line: &str) -> bool {
    line.contains('|') && parse_table_row(line).len() >= 2
}

pub(crate) fn is_table_separator(line: &str) -> bool {
    let cells = parse_table_row(line);
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let value = cell.trim().trim_matches(':');
            value.len() >= 3 && value.chars().all(|ch| ch == '-')
        })
}

pub(crate) fn parse_table_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

pub(crate) fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes)
        || !line
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    Some((hashes as u8, line[hashes..].trim()))
}

/// 识别独占一行的图片引用 `![alt](src)`。`src` 不含空白与右括号（与
/// `images::image_refs` 的正则约束一致），返回 `(alt, src)`。
pub(crate) fn parse_image(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("![")?;
    let close = rest.find(']')?;
    let alt = rest[..close].to_string();
    let src = rest[close + 1..]
        .strip_prefix('(')?
        .strip_suffix(')')?
        .trim();
    if src.is_empty() || src.contains(char::is_whitespace) {
        return None;
    }
    Some((alt, src.to_string()))
}

/// 整行是否恰好是一张图片引用（与解析器的独占行识别一致）。
/// 实时排版编辑器据此把图片行高亮为占位提示。
pub(crate) fn is_image_line(line: &str) -> bool {
    parse_image(line.trim()).is_some()
}

/// 识别 `<!-- [正文] -->`、`<!-- [附件] -->`（含【】与英文变体）这类独占一行的区段标记。
/// 实时排版编辑器（draft_page / highlight）也复用它，以便在区段切换处重置公文标题计数器。
pub(crate) fn parse_section_marker(line: &str) -> Option<MarkdownSection> {
    let inner = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .trim_start_matches(['[', '【'])
        .trim_end_matches([']', '】'])
        .trim();
    match inner.to_ascii_lowercase().as_str() {
        "正文" | "body" => Some(MarkdownSection::Body),
        "附件" | "附录" | "attachment" | "attachments" | "appendix" => {
            Some(MarkdownSection::Attachment)
        }
        _ => None,
    }
}

/// 正文区最大标题层级（# 号最多的那一级），紧缩风格（规格 §4.2）据此把该级标题与正文合并。
/// 文档标题（`#`）与附件区标题不计入；附件区标题使用独立排版与计数器。
pub(crate) fn body_heading_max_level(blocks: &[MarkdownBlock]) -> u8 {
    let mut section = MarkdownSection::Body;
    let mut max_level = 0u8;
    for block in blocks {
        match block {
            MarkdownBlock::Marker(next) => section = *next,
            MarkdownBlock::Heading(level, _) if section == MarkdownSection::Body => {
                max_level = max_level.max(*level);
            }
            _ => {}
        }
    }
    max_level
}

#[cfg(test)]
mod ordered_list_tests {
    use super::*;

    #[test]
    fn adjacent_list_becomes_circled_inline_paragraph() {
        let blocks = parse_markdown(
            "正文内容：\n1. 第一项，\n1. 第二项。；\n1. 第三项，\n\n1. 独立甲，\n1. 独立乙；",
        );
        assert_eq!(
            blocks[0],
            MarkdownBlock::Paragraph("正文内容：①第一项；②第二项；③第三项。".into())
        );
        assert_eq!(
            blocks[1],
            MarkdownBlock::OrderedListItem {
                number: 1,
                text: "独立甲。".into(),
            }
        );
        assert_eq!(
            blocks[2],
            MarkdownBlock::OrderedListItem {
                number: 2,
                text: "独立乙。".into(),
            }
        );
    }

    #[test]
    fn only_terminal_middle_semicolon_selects_semicolon_style() {
        let internal = vec!["包括甲；乙，".into(), "最后一项；".into()];
        assert_eq!(
            normalize_ordered_item_punctuation(&internal),
            ["包括甲；乙。", "最后一项。"]
        );

        let terminal = vec!["第一项。；".into(), "第二项，".into(), "末项；".into()];
        assert_eq!(
            normalize_ordered_item_punctuation(&terminal),
            ["第一项；", "第二项；", "末项。"]
        );
    }

    #[test]
    fn source_normalization_preserves_markers_blank_lines_and_line_ending() {
        let source =
            "正文\r\n1. 第一项，\r\n1. 第二项。；\r\n1. 第三项\r\n\r\n3. 另一项,\r\n1. 末项;\r\n";
        assert_eq!(
            normalize_ordered_list_punctuation(source),
            "正文\r\n1. 第一项；\r\n1. 第二项；\r\n1. 第三项。\r\n\r\n3. 另一项。\r\n1. 末项。\r\n"
        );
    }

    #[test]
    fn first_source_number_controls_automatic_numbering() {
        let blocks = parse_markdown("\n3. 甲。\n1. 乙。");
        assert!(matches!(
            &blocks[0],
            MarkdownBlock::OrderedListItem { number: 3, .. }
        ));
        assert!(matches!(
            &blocks[1],
            MarkdownBlock::OrderedListItem { number: 4, .. }
        ));
    }
}

// ========== 红头呈批件首页几何 ==========
//
// 首页三块区域的宽度在预览、Word、LaTeX 三端必须一致，因此统一以缇（1/20 磅）
// 定义，另两端再换算成毫米/像素。基准字号为三号（16 pt），1 em = 320 缇。
