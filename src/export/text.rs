//! 文本工具：表格列、中文数字、引号归一、内联段与附件名。
//!
//! 由 src/export/mod.rs 拆分而来：本文件是模块 `export::text`，与其它子模块共享
//! `export` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use super::docx;
use super::table;
use crate::export::{ColumnAlign, MarkdownBlock, MarkdownSection};

/// 表格的一列在版心中所占的比例，以及它的对齐方式。界面预览据此复用导出器的
/// 智能列宽，保证预览里的列宽、对齐与导出的 Word 表格一致。
pub(crate) struct TableColumn {
    /// 占版心宽度的比例，各列相加为 1。
    pub(crate) fraction: f32,
    pub(crate) alignment: table::ColumnAlignment,
}

pub(crate) fn table_columns(rows: &[Vec<String>], aligns: &[ColumnAlign]) -> Vec<TableColumn> {
    let (grid, alignments) = table::to_docx_grid(
        rows,
        aligns,
        docx::TABLE_CONTENT_WIDTH_TWIPS,
        docx::TABLE_SIZE * 10,
    );
    let total = grid.iter().sum::<usize>().max(1) as f32;
    grid.iter()
        .enumerate()
        .map(|(index, width)| TableColumn {
            fraction: *width as f32 / total,
            alignment: alignments
                .get(index)
                .copied()
                .unwrap_or(table::ColumnAlignment::Left),
        })
        .collect()
}

pub(crate) fn number_to_chinese(number: usize) -> String {
    const DIGITS: [&str; 11] = [
        "", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十",
    ];
    match number {
        0..=10 => DIGITS[number].to_string(),
        11..=19 => format!("十{}", DIGITS[number - 10]),
        20..=99 if number.is_multiple_of(10) => format!("{}十", DIGITS[number / 10]),
        20..=99 => format!("{}十{}", DIGITS[number / 10], DIGITS[number % 10]),
        _ => number.to_string(),
    }
}

pub(crate) fn plain_text(text: &str) -> String {
    inline_segments(text)
        .into_iter()
        .map(|segment| segment.text)
        .collect()
}

/// 把正文中的直引号、方向错误或风格混杂的引号统一为正确配对的中文引号。
/// 导出时本来就会做一遍；起草页「格式 → 规范引号」把它提到源码上，
/// 好让编辑区看到的和导出结果一致。
pub(crate) fn normalize_chinese_quotes(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut double_open = true;
    let mut single_open = true;
    let chars = text.chars().collect::<Vec<_>>();
    for (index, ch) in chars.iter().copied().enumerate() {
        if matches!(ch, '"' | '＂' | '“' | '”' | '„' | '‟' | '「' | '」') {
            output.push(if double_open { '“' } else { '”' });
            double_open = !double_open;
        } else if matches!(ch, '\'' | '＇' | '‘' | '’' | '‚' | '‛' | '『' | '』') {
            let apostrophe = index > 0
                && index + 1 < chars.len()
                && chars[index - 1].is_ascii_alphanumeric()
                && chars[index + 1].is_ascii_alphanumeric();
            if apostrophe {
                output.push('’');
            } else {
                output.push(if single_open { '‘' } else { '’' });
                single_open = !single_open;
            }
        } else {
            output.push(ch);
        }
    }
    output
}

pub(crate) fn parenthesized_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut outer_start = None;
    for (index, ch) in text.char_indices() {
        let expected = match ch {
            '（' => Some('）'),
            '(' => Some(')'),
            '【' => Some('】'),
            _ => None,
        };
        if let Some(expected) = expected {
            if stack.is_empty() {
                outer_start = Some(index);
            }
            stack.push((expected, index));
            continue;
        }
        if matches!(ch, '）' | ')' | '】') {
            if stack.last().is_some_and(|(expected, _)| *expected == ch) {
                stack.pop();
                if stack.is_empty()
                    && let Some(start) = outer_start.take()
                {
                    ranges.push((start, index + ch.len_utf8()));
                }
            } else {
                // 错配或孤立的右括号不参与字体切换；正在解析的外层也作废。
                stack.clear();
                outer_start = None;
            }
        }
    }
    ranges
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineSegment {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) parenthesized: bool,
}

// ── 花脸稿标记 ──────────────────────────────────────────────────────────────
//
// 增删用 Unicode 私用区的哨兵字符包起来，而不是另造一套 Markdown 语法：公文
// 正文里不可能出现这些码位，也就不会跟 `**`、括号、表格竖线抢解析；更要紧的是
// 它们在 `inline_segments` 之前就被剥掉，那套加粗/括号状态机一行都不用动。
//
// 没有哨兵的正常文档走的是同一条路，切出来就是一整块 `Same`，导出结果与从前
// 逐字节一致。

/// 删除段起始。
pub(crate) const REDLINE_DEL_OPEN: char = '\u{E000}';
/// 删除段结束。
pub(crate) const REDLINE_DEL_CLOSE: char = '\u{E001}';
/// 新增段起始。
pub(crate) const REDLINE_ADD_OPEN: char = '\u{E002}';
/// 新增段结束。
pub(crate) const REDLINE_ADD_CLOSE: char = '\u{E003}';

/// 一段文字在花脸稿里的身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedlineKind {
    /// 未改动，照常排。
    Same,
    /// 旧稿删掉的，画波浪线。
    Deleted,
    /// 新稿加上的，套方框。
    Added,
}

/// 一块同类文字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedlineChunk {
    pub(crate) kind: RedlineKind,
    pub(crate) text: String,
}

/// 用哨兵把一行切成若干块。没有哨兵时返回整行一块 `Same`。
///
/// 容错取"就近闭合"：遇到未配对的收尾哨兵就当普通结束，遇到没闭合的起始哨兵
/// 就一直标到行尾。花脸稿是程序生成的，正常不会出现这种情况，但宁可排得难看
/// 也不能把哨兵字符原样印到纸上。
pub(crate) fn redline_chunks(text: &str) -> Vec<RedlineChunk> {
    if !text.contains([
        REDLINE_DEL_OPEN,
        REDLINE_DEL_CLOSE,
        REDLINE_ADD_OPEN,
        REDLINE_ADD_CLOSE,
    ]) {
        return vec![RedlineChunk {
            kind: RedlineKind::Same,
            text: text.to_string(),
        }];
    }

    let mut chunks = Vec::new();
    let mut buffer = String::new();
    let mut kind = RedlineKind::Same;
    let flush = |chunks: &mut Vec<RedlineChunk>, buffer: &mut String, kind: RedlineKind| {
        if !buffer.is_empty() {
            chunks.push(RedlineChunk {
                kind,
                text: std::mem::take(buffer),
            });
        }
    };
    for ch in text.chars() {
        match ch {
            REDLINE_DEL_OPEN => {
                flush(&mut chunks, &mut buffer, kind);
                kind = RedlineKind::Deleted;
            }
            REDLINE_ADD_OPEN => {
                flush(&mut chunks, &mut buffer, kind);
                kind = RedlineKind::Added;
            }
            REDLINE_DEL_CLOSE | REDLINE_ADD_CLOSE => {
                flush(&mut chunks, &mut buffer, kind);
                kind = RedlineKind::Same;
            }
            _ => buffer.push(ch),
        }
    }
    flush(&mut chunks, &mut buffer, kind);
    chunks
}

/// 把一段文字包成删除标记。
pub(crate) fn mark_deleted(text: &str) -> String {
    format!("{REDLINE_DEL_OPEN}{text}{REDLINE_DEL_CLOSE}")
}

/// 把一段文字包成新增标记。
pub(crate) fn mark_added(text: &str) -> String {
    format!("{REDLINE_ADD_OPEN}{text}{REDLINE_ADD_CLOSE}")
}

/// 是不是花脸稿哨兵。各渲染路径用它做兜底过滤。
pub(crate) fn is_redline_sentinel(ch: char) -> bool {
    matches!(
        ch,
        REDLINE_DEL_OPEN | REDLINE_DEL_CLOSE | REDLINE_ADD_OPEN | REDLINE_ADD_CLOSE
    )
}

/// 去掉全部哨兵，还原成干净文本。导出 Markdown 或做标题提取时用。
pub(crate) fn strip_redline(text: &str) -> String {
    text.replace(
        [
            REDLINE_DEL_OPEN,
            REDLINE_DEL_CLOSE,
            REDLINE_ADD_OPEN,
            REDLINE_ADD_CLOSE,
        ],
        "",
    )
}

#[derive(Debug)]
struct InlineAtom {
    source: std::ops::Range<usize>,
    ch: char,
    bold: bool,
}

fn escaped_at(text: &str, index: usize) -> bool {
    text[..index]
        .chars()
        .rev()
        .take_while(|ch| *ch == '\\')
        .count()
        % 2
        == 1
}

fn paired_bold_markers(text: &str) -> std::collections::HashMap<usize, &'static str> {
    let mut paired = std::collections::HashMap::new();
    for marker in ["**", "__"] {
        let positions = text
            .match_indices(marker)
            .map(|(index, _)| index)
            .filter(|index| !escaped_at(text, *index))
            .collect::<Vec<_>>();
        for pair in positions.as_chunks::<2>().0 {
            paired.insert(pair[0], marker);
            paired.insert(pair[1], marker);
        }
    }
    paired
}

/// 把行内 Markdown 转成逐个可见字符；每个字符同时保留输入中的来源字节范围。
fn inline_atoms(text: &str) -> Vec<InlineAtom> {
    let paired_markers = paired_bold_markers(text);
    let mut atoms = Vec::new();
    let mut star_bold = false;
    let mut underscore_bold = false;
    let mut index = 0usize;
    while index < text.len() {
        if let Some(marker) = paired_markers.get(&index) {
            if *marker == "**" {
                star_bold = !star_bold;
            } else {
                underscore_bold = !underscore_bold;
            }
            index += marker.len();
            continue;
        }

        let ch = text[index..].chars().next().expect("valid char boundary");
        if ch == '`' && !escaped_at(text, index) {
            index += ch.len_utf8();
            continue;
        }
        if is_redline_sentinel(ch) {
            index += ch.len_utf8();
            continue;
        }
        if ch == '\\'
            && let Some(next) = text[index + ch.len_utf8()..].chars().next()
            && next.is_ascii_punctuation()
        {
            let end = index + ch.len_utf8() + next.len_utf8();
            atoms.push(InlineAtom {
                source: index..end,
                ch: next,
                bold: star_bold || underscore_bold,
            });
            index = end;
            continue;
        }

        let end = index + ch.len_utf8();
        atoms.push(InlineAtom {
            source: index..end,
            ch,
            bold: star_bold || underscore_bold,
        });
        index = end;
    }

    // 引号规范化是一进一出；处理后仍可沿用每个 atom 的来源范围。
    let visible = atoms.iter().map(|atom| atom.ch).collect::<String>();
    for (atom, normalized) in atoms
        .iter_mut()
        .zip(normalize_chinese_quotes(&visible).chars())
    {
        atom.ch = normalized;
    }
    atoms
}

/// 解析正文行内 Markdown：保留加粗语义、反斜杠转义，同时叠加括号字体规则。
/// 显示文本与字符来源映射共用 `inline_atoms`，不会各算一遍。
pub(crate) fn inline_segments(text: &str) -> Vec<InlineSegment> {
    let atoms = inline_atoms(text);
    let visible = atoms.iter().map(|atom| atom.ch).collect::<String>();
    let paren_ranges = parenthesized_ranges(&visible);
    let mut segments: Vec<InlineSegment> = Vec::new();
    let mut visible_byte = 0usize;
    for atom in atoms {
        let parenthesized = paren_ranges
            .iter()
            .any(|(start, end)| *start <= visible_byte && visible_byte < *end);
        if let Some(last) = segments.last_mut()
            && last.bold == atom.bold
            && last.parenthesized == parenthesized
        {
            last.text.push(atom.ch);
        } else {
            segments.push(InlineSegment {
                text: atom.ch.to_string(),
                bold: atom.bold,
                parenthesized,
            });
        }
        visible_byte += atom.ch.len_utf8();
    }
    segments
}

/// 行内 Markdown 排版后，原文某个字节位置对应到第几个可见字符。
///
/// 预览按源码行拆分点击区域时需要这层换算：反引号和成对的加粗标记不占版面，
/// 中文引号规范化则仍是一进一出。范围按字符计，正好可直接交给 egui 的光标坐标。
pub(crate) fn inline_visible_char_index(text: &str, byte_offset: usize) -> usize {
    inline_visible_char_indices(text, &[byte_offset])[0]
}

/// 批量把输入字节边界换成可见字符边界；整段只解析一次，供多行段落建立来源映射。
pub(crate) fn inline_visible_char_indices(text: &str, byte_offsets: &[usize]) -> Vec<usize> {
    let atoms = inline_atoms(text);
    byte_offsets
        .iter()
        .map(|byte_offset| {
            let mut boundary = (*byte_offset).min(text.len());
            while boundary > 0 && !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            atoms.partition_point(|atom| atom.source.end <= boundary)
        })
        .collect()
}

/// 从附件标题提取内嵌名称：`附件1：统计表` → `统计表`；`附件1` → None。
pub(crate) fn attachment_title_name(label: &str) -> Option<String> {
    let rest = label.strip_prefix("附件")?;
    let after_number = rest
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start();
    let name = after_number
        .strip_prefix('：')
        .or_else(|| after_number.strip_prefix(':'))
        .or_else(|| after_number.strip_prefix('、'))
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    Some(plain_text(name))
}

pub(crate) fn legacy_attachment_label(label: &str) -> Option<()> {
    let rest = label.strip_prefix("附件")?;
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let tail = rest[digits..].trim();
    if tail.is_empty() || tail.starts_with('：') || tail.starts_with(':') || tail.starts_with('、')
    {
        Some(())
    } else {
        None
    }
}

/// 提取各附件正式标题。解析器已把旧格式规范化，因此每个附件标记之后的首个
/// `#` 标题就是名称；没有正式标题的附件不进入正文附件清单。
pub(crate) fn attachment_names(blocks: &[MarkdownBlock]) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_attachment = false;
    let mut pending_name = false;
    for block in blocks {
        match block {
            MarkdownBlock::Marker(section) => {
                in_attachment = matches!(section, MarkdownSection::Attachment);
                pending_name = in_attachment;
            }
            MarkdownBlock::Title(text) if in_attachment && pending_name => {
                names.push(plain_text(text));
                pending_name = false;
            }
            _ => {}
        }
    }
    names
}

/// 解析界面使用的“YYYY年M月D日”日期，供 Word 与 LaTeX 使用同一套预览占位规则。
pub(crate) fn chinese_date_parts(value: &str) -> Option<(&str, &str, &str)> {
    let (year, remainder) = value.trim().split_once('年')?;
    let (month, day) = remainder.split_once('月')?;
    let day = day.trim().trim_end_matches('日').trim();
    Some((year.trim(), month.trim(), day))
}

#[cfg(test)]
mod source_mapping_tests {
    use super::*;

    #[test]
    fn visible_index_ignores_inline_markers_before_a_source_boundary() {
        let text = "**第一行**第二行";
        let boundary = text.find("第二行").unwrap();
        assert_eq!(inline_visible_char_index(text, boundary), 3);
        assert_eq!(inline_visible_char_index(text, text.len()), 6);
    }

    #[test]
    fn escaped_markdown_punctuation_is_one_visible_character() {
        let text = r"字段\_名称与\*号";
        assert_eq!(plain_text(text), "字段_名称与*号");
        let boundary = text.find("名称").unwrap();
        assert_eq!(inline_visible_char_index(text, boundary), 3);
        assert_eq!(inline_visible_char_index(text, text.len()), 8);
    }

    #[test]
    fn escaped_bold_markers_stay_literal() {
        let segments = inline_segments(r"\**不加粗\**，**加粗**");
        assert_eq!(
            segments,
            [
                InlineSegment {
                    text: "**不加粗**，".into(),
                    bold: false,
                    parenthesized: false,
                },
                InlineSegment {
                    text: "加粗".into(),
                    bold: true,
                    parenthesized: false,
                },
            ]
        );
    }
}
