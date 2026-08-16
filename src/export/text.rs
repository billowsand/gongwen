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
    text.replace("**", "").replace("__", "").replace('`', "")
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

/// 解析正文行内 Markdown：保留 `**…**` / `__…__` 的加粗语义，同时叠加括号字体规则。
/// 加粗标记跨越括号边界时仍可正确切分为多个字体一致、粗细一致的片段。
pub(crate) fn inline_segments(text: &str) -> Vec<InlineSegment> {
    let text = normalize_chinese_quotes(&text.replace('`', ""));
    let paren_ranges = parenthesized_ranges(&text);

    // 只移除成对出现的 Markdown 加粗标记；孤立的 `**` / `__` 保留原文。
    let mut paired_markers: std::collections::HashMap<usize, &'static str> =
        std::collections::HashMap::new();
    for (marker, literal) in [("**", "**"), ("__", "__")] {
        let positions = text
            .match_indices(marker)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for pair in positions.chunks_exact(2) {
            paired_markers.insert(pair[0], literal);
            paired_markers.insert(pair[1], literal);
        }
    }

    let mut segments: Vec<InlineSegment> = Vec::new();
    let mut buffer = String::new();
    let mut star_bold = false;
    let mut underscore_bold = false;
    let mut current_state: Option<(bool, bool)> = None;
    let mut index = 0usize;
    while index < text.len() {
        if let Some(marker) = paired_markers.get(&index) {
            if !buffer.is_empty()
                && let Some((bold, parenthesized)) = current_state
            {
                segments.push(InlineSegment {
                    text: std::mem::take(&mut buffer),
                    bold,
                    parenthesized,
                });
            }
            if *marker == "**" {
                star_bold = !star_bold;
            } else {
                underscore_bold = !underscore_bold;
            }
            current_state = None;
            index += marker.len();
            continue;
        }

        let ch = text[index..].chars().next().expect("valid char boundary");
        let parenthesized = paren_ranges
            .iter()
            .any(|(start, end)| *start <= index && index < *end);
        let state = (star_bold || underscore_bold, parenthesized);
        if current_state.is_some_and(|current| current != state) && !buffer.is_empty() {
            let (bold, parenthesized) = current_state.expect("state exists");
            segments.push(InlineSegment {
                text: std::mem::take(&mut buffer),
                bold,
                parenthesized,
            });
        }
        current_state = Some(state);
        buffer.push(ch);
        index += ch.len_utf8();
    }
    if !buffer.is_empty()
        && let Some((bold, parenthesized)) = current_state
    {
        segments.push(InlineSegment {
            text: buffer,
            bold,
            parenthesized,
        });
    }
    segments
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
