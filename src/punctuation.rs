//! 保存时的整篇标点规范。
//!
//! 与输入层、预览层分开：
//! - 输入层的 `1。`→`1.` 是中文全角输入法的补偿，留在 `draft_page::list_period`；
//! - 预览/导出只读，不改源码；
//! - 这里只在用户显式保存时跑一次，整篇看上下文，改完写回源码。
//!
//! 这一层收「确定、可整篇判断」的几类：
//! 1. 发文字号年份括号与公文引用外层括号；
//! 2. 粘贴/导入带来的全角列表标记 `1。`/`1．`；
//! 3. 半角/全角、重复标点、省略号；
//! 4. 中文引号配对（代码围栏、行内代码、公文引用原样保留）；
//! 5. 有序列表末尾句号/分号（按每个连续列表组判断）。
//!
//! 空格归一还没进来：`normalize_spaces` 会删掉 `1. 内容` 后面的空格，得先给列表
//! 标记和代码加保护。

use crate::document_reference;
use regex::Regex;
use similar::TextDiff;
use std::sync::OnceLock;

/// 一处改动的说明，供状态栏/审校抽屉报数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Change {
    /// 给人看的短说明，如「某函[2026]12号 → 某函〔2026〕12号」。
    pub(crate) label: String,
}

/// 整篇保存规范的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Corrected {
    pub(crate) text: String,
    pub(crate) changes: Vec<Change>,
    pub(crate) map: SourceMap,
}

/// 一段源码到更正文本的坐标映射。相等段按偏移换算，替换段按整段换算。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SourceMap {
    segments: Vec<MapSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MapSegment {
    old: std::ops::Range<usize>,
    new: std::ops::Range<usize>,
}

impl SourceMap {
    pub(crate) fn identity(len: usize) -> Self {
        Self {
            segments: vec![MapSegment {
                old: 0..len,
                new: 0..len,
            }],
        }
    }

    pub(crate) fn new(old: &str, new: &str) -> Self {
        if old == new {
            return Self::identity(old.len());
        }
        let old_chars = old.char_indices().map(|(byte, _)| byte).collect::<Vec<_>>();
        let new_chars = new.char_indices().map(|(byte, _)| byte).collect::<Vec<_>>();
        let byte = |chars: &[usize], index: usize, len: usize| -> usize {
            chars.get(index).copied().unwrap_or(len)
        };
        let diff = TextDiff::from_chars(old, new);
        let mut segments = Vec::new();
        for op in diff.ops() {
            let old_range = op.old_range();
            let new_range = op.new_range();
            segments.push(MapSegment {
                old: byte(&old_chars, old_range.start, old.len())
                    ..byte(&old_chars, old_range.end, old.len()),
                new: byte(&new_chars, new_range.start, new.len())
                    ..byte(&new_chars, new_range.end, new.len()),
            });
        }
        Self { segments }
    }

    /// 源码范围 → 更正文本范围。替换段取整段；调用方拿到的是可用的粗范围。
    pub(crate) fn to_new(&self, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        self.map(&range, false)
    }

    /// 更正文本范围 → 源码范围。
    pub(crate) fn to_old(&self, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        self.map(&range, true)
    }

    fn map(&self, range: &std::ops::Range<usize>, reverse: bool) -> std::ops::Range<usize> {
        let mut start = None;
        let mut end = None;
        for segment in &self.segments {
            let (source_start, source_end, target_start, target_end) = if reverse {
                (
                    segment.new.start,
                    segment.new.end,
                    segment.old.start,
                    segment.old.end,
                )
            } else {
                (
                    segment.old.start,
                    segment.old.end,
                    segment.new.start,
                    segment.new.end,
                )
            };
            if source_end < range.start {
                continue;
            }
            if source_start > range.end {
                break;
            }
            let same_length = source_end - source_start == target_end - target_start;
            if start.is_none() && range.start < source_end {
                let at = range.start.max(source_start);
                start = Some(if same_length {
                    target_start + (at - source_start)
                } else {
                    target_start
                });
            }
            if range.end <= source_end {
                let at = range.end.max(source_start);
                end = Some(if same_length {
                    target_start + (at - source_start)
                } else {
                    target_end
                });
                break;
            }
        }
        let start = start.unwrap_or_else(|| {
            self.segments.last().map_or(0, |segment| {
                if reverse {
                    segment.old.end
                } else {
                    segment.new.end
                }
            })
        });
        let end = end.unwrap_or(start);
        start..end.max(start)
    }
}

impl Corrected {
    pub(crate) fn unchanged(text: String) -> Self {
        let map = SourceMap::identity(text.len());
        Self {
            text,
            changes: Vec::new(),
            map,
        }
    }

    pub(crate) fn changed(&self) -> bool {
        !self.changes.is_empty()
    }

    pub(crate) fn summary(&self) -> String {
        self.changes
            .iter()
            .map(|change| change.label.as_str())
            .collect::<Vec<_>>()
            .join("；")
    }
}

struct Edit {
    range: std::ops::Range<usize>,
    replacement: String,
    label: String,
}

/// 保存时对整篇公文正文做一次规范（测试与公文路径使用）。
#[cfg(test)]
pub(crate) fn normalize_for_save(markdown: &str) -> Corrected {
    normalize_for_save_for_kind(markdown, false)
}

/// 保存时对整篇正文做一次规范；研究报告不套公文列表末尾标点。
///
/// 只在显式保存调用；自动保存、失焦、切标签、复制全文都不调用。
pub(crate) fn normalize_for_save_for_kind(markdown: &str, research: bool) -> Corrected {
    normalize_impl(markdown, research, true)
}

/// 预览与导出用的同一套规范，但不做列表末尾整篇判断（那是保存的事）。
pub(crate) fn normalize_for_preview(markdown: &str, research: bool) -> Corrected {
    normalize_impl(markdown, research, false)
}

fn normalize_impl(markdown: &str, research: bool, list_tail: bool) -> Corrected {
    let mut changes = Vec::new();

    // 1. 公文引用与发文字号。两边的识别都在原文上做，互不重叠：
    //    `detect_loose_numbers` 会跳过已识别为引用的范围。
    let text = normalize_document_numbers(markdown, &mut changes);

    // 2. 先把网址、邮箱、裸域名、路径、带扩展名的文件摘出来，避免后续的
    //    半角/全角与空格规整把 `例子.中国`、`C:\我的 文件.docx` 改坏。
    let (mut text, protected) = mask_sensitive(&text);

    // 3. 粘贴/导入带来的全角列表标记 `1。`/`1．`：输入层只管手打，
    //    这里兜底，让后续列表末尾判断能认出它。
    let markers = normalize_list_markers(&text);
    if markers != text {
        changes.push(Change {
            label: "列表标记 1。→1.".to_string(),
        });
        text = markers;
    }

    // 4. 半角/全角、重复标点、省略号。代码、公式、链接、URL 由
    //    `normalize_punctuation` 自己跳过。
    let basic = crate::doc_import::clean::normalize_punctuation(&text);
    if basic != text {
        changes.push(Change {
            label: "规范半角标点、重复标点与省略号".to_string(),
        });
        text = basic;
    }

    // 5. 空格：删汉字/标点/数字之间的多余空格，保留代码与西文词间空格。
    let spaced = crate::doc_import::clean::normalize_spaces(&text);
    if spaced != text {
        changes.push(Change {
            label: "规范空格".to_string(),
        });
        text = spaced;
    }

    // 6. 引号配对。代码围栏与行内代码原样保留。
    let quoted = normalize_quotes_outside_code(&text);
    if quoted != text {
        changes.push(Change {
            label: "规范中文引号".to_string(),
        });
        text = quoted;
    }

    // 6. 有序列表末尾标点：整篇扫一遍，按每个连续列表组判断。
    //    研究报告不套公文的 `；`/`。` 收尾规则：研报列表常以单位、英文缩写或公式
    //    结尾，套上去反而是错的（与 `finalize_markdown` 同一口径）。
    let normalized = if list_tail && !research {
        crate::export::normalize_ordered_list_punctuation(&text)
    } else {
        text.clone()
    };
    if normalized != text {
        changes.push(Change {
            label: "统一列表末尾句号/分号".to_string(),
        });
    }

    let text = unmask_sensitive(&normalized, &protected);
    let map = SourceMap::new(markdown, &text);
    Corrected { text, changes, map }
}

/// 预览/导出排版用的 1:1 标点替换。
///
/// 只改字符本身、不增删字符，所以 `inline_atoms` 的源码范围仍然对得上。重复标点、
/// 省略号这类会改字符数的留给保存 pass。
pub(crate) fn style_chars(chars: &[char], protected: &[bool]) -> Vec<char> {
    let mut output = chars.to_vec();
    for index in 0..chars.len() {
        if protected.get(index).copied().unwrap_or(false) {
            continue;
        }
        let left = output[..index]
            .iter()
            .rev()
            .find(|ch| !ch.is_whitespace())
            .copied();
        let right = output[index + 1..]
            .iter()
            .find(|ch| !ch.is_whitespace())
            .copied();
        if let Some(fullwidth) = style_one(chars[index], left, right) {
            output[index] = fullwidth;
        }
    }
    // 括号要成对改，否则会出现「（API)」这种半边全角。`(1)` 这类编号不动：
    // 它由列表编号设置控制，生成出来的半角括号不能被这里改掉。
    for (open, close) in parenthesis_pairs(chars) {
        if protected.get(open).copied().unwrap_or(false)
            || protected.get(close).copied().unwrap_or(false)
        {
            continue;
        }
        if chars[open] != '(' || chars[close] != ')' {
            continue;
        }
        let digits_only = chars[open + 1..close]
            .iter()
            .all(|ch| ch.is_ascii_digit() || ch.is_whitespace());
        if digits_only {
            continue;
        }
        let left = chars[..open]
            .iter()
            .rev()
            .find(|ch| !ch.is_whitespace())
            .copied();
        let right = chars[close + 1..]
            .iter()
            .find(|ch| !ch.is_whitespace())
            .copied();
        if left.is_some_and(crate::doc_import::clean::is_cjk_side)
            || right.is_some_and(crate::doc_import::clean::is_cjk_side)
        {
            output[open] = '（';
            output[close] = '）';
        }
    }
    output
}

fn parenthesis_pairs(chars: &[char]) -> Vec<(usize, usize)> {
    let mut stack = Vec::new();
    let mut pairs = Vec::new();
    for (index, ch) in chars.iter().copied().enumerate() {
        match ch {
            '(' => stack.push(index),
            ')' => {
                if let Some(open) = stack.pop() {
                    pairs.push((open, index));
                }
            }
            _ => {}
        }
    }
    pairs
}

fn style_one(ch: char, left: Option<char>, right: Option<char>) -> Option<char> {
    let cjk = |ch: Option<char>| ch.is_some_and(crate::doc_import::clean::is_cjk_side);
    if !cjk(left) && !cjk(right) {
        return None;
    }
    match ch {
        ',' if left.is_some_and(|ch| ch.is_ascii_digit())
            && right.is_some_and(|ch| ch.is_ascii_digit()) =>
        {
            None
        }
        ',' => Some('，'),
        '.' if left.is_some_and(|ch| ch.is_ascii_alphanumeric())
            || right.is_some_and(|ch| ch.is_ascii_alphanumeric()) =>
        {
            None
        }
        '.' => Some('。'),
        ';' => Some('；'),
        ':' if left.is_some_and(|ch| ch.is_ascii_alphanumeric()) => None,
        ':' => Some('：'),
        '!' => Some('！'),
        // LaTeX 的悬空引用占位符 `??` 不是中文问号，留给预览/导出原样显示。
        '?' if left == Some('?') || right == Some('?') => None,
        '?' => Some('？'),
        // 括号成对才该转全角；预览的逐字符替换看不到配对，留给保存 pass。
        _ => None,
    }
}

/// 预览/导出时不能改的区段：代码围栏、行内代码、公文引用。
pub(crate) fn protected_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = code_ranges(text);
    ranges.extend(
        document_reference::detect_numbered(text)
            .into_iter()
            .map(|citation| citation.range),
    );
    ranges.extend(sensitive_ranges(text));
    merge_ranges(ranges)
}

/// 把网址、邮箱、裸域名、路径和带扩展名的文件换成占位符。
///
/// 这些片段的内部标点/空格不能参与中文标点规整；占位符用私用区字符，后续的
/// 半角全角、空格、引号、列表末尾判断都不会碰它，最后原样换回。
fn mask_sensitive(text: &str) -> (String, Vec<(String, String)>) {
    let ranges = sensitive_ranges(text);
    if ranges.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let mut output = String::with_capacity(text.len());
    let mut protected = Vec::with_capacity(ranges.len());
    let mut cursor = 0usize;
    for (index, range) in ranges.iter().enumerate() {
        output.push_str(&text[cursor..range.start]);
        let token = format!("\u{E100}{index}\u{E101}");
        output.push_str(&token);
        protected.push((token, text[range.clone()].to_string()));
        cursor = range.end;
    }
    output.push_str(&text[cursor..]);
    (output, protected)
}

fn unmask_sensitive(text: &str, protected: &[(String, String)]) -> String {
    let mut output = text.to_string();
    for (token, original) in protected {
        output = output.replace(token, original);
    }
    output
}

/// 网址、邮箱、裸域名、Windows 路径、带 ASCII/常见中文扩展名的文件名。
fn sensitive_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"(?i)(?:https?|ftp|file)://[^\s，。；：！？、）】》”’]+",
            r"(?i)www\.[^\s，。；：！？、）】》”’]+",
            r"(?i)[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}",
            r"(?i)[A-Z]:[\\/][^，。；：！？、\n]*",
            r"\\\\[^，。；：！？、\n]*",
            r"(?i)(?:[a-z0-9-]+\.)+(?:com|cn|net|org|gov|edu|io|ai|info|top|xyz)\b",
            r"(?i)[^\s，。；：！？、]+\.[A-Za-z0-9]{1,8}\b",
            r"[^\s，。；：！？、]+\.(?:中国|公司|网络|文档|表格|演示|图片|压缩包)",
        ]
        .into_iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
    });
    let mut ranges = Vec::new();
    for pattern in patterns {
        ranges.extend(pattern.find_iter(text).map(|found| found.range()));
    }
    merge_ranges(ranges)
}

fn normalize_document_numbers(markdown: &str, changes: &mut Vec<Change>) -> String {
    let mut edits = Vec::new();
    for citation in document_reference::detect_numbered(markdown) {
        if citation.standard(markdown) {
            continue;
        }
        let fixed = citation.text();
        edits.push(Edit {
            range: citation.range.clone(),
            replacement: fixed.clone(),
            label: format!("公文引用文号规范为「{fixed}」"),
        });
    }
    for number in document_reference::detect_loose_numbers(markdown) {
        edits.push(Edit {
            range: number.range.clone(),
            replacement: number.fixed.clone(),
            label: format!("{} → {}", number.written, number.fixed),
        });
    }
    // 按位置排序，重叠的只留第一个（引用优先于裸文号；识别层已经尽量互斥）。
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let mut kept: Vec<Edit> = Vec::with_capacity(edits.len());
    let mut cursor = 0usize;
    for edit in edits {
        if edit.range.start < cursor || edit.range.end > markdown.len() {
            continue;
        }
        cursor = edit.range.end;
        kept.push(edit);
    }
    if kept.is_empty() {
        return markdown.to_string();
    }
    for edit in &kept {
        changes.push(Change {
            label: edit.label.clone(),
        });
    }
    apply_edits(markdown, kept)
}

fn apply_edits(markdown: &str, edits: Vec<Edit>) -> String {
    let mut output = String::with_capacity(markdown.len());
    let mut cursor = 0usize;
    for edit in edits {
        output.push_str(&markdown[cursor..edit.range.start]);
        output.push_str(&edit.replacement);
        cursor = edit.range.end;
    }
    output.push_str(&markdown[cursor..]);
    output
}

/// 只对代码围栏与行内代码之外的文字做引号配对。
///
/// `normalize_chinese_quotes` 是逐字符一对一替换，本身不改长度；但 `inline_atoms`
/// 会把行内代码也当成普通字符，代码里的 `"` 会被改成中文引号。保存时先把代码段
/// 摘出来，避免把示例代码改坏。
fn normalize_quotes_outside_code(text: &str) -> String {
    let mut ranges = code_ranges(text);
    ranges.extend(
        document_reference::detect_numbered(text)
            .into_iter()
            .map(|citation| citation.range),
    );
    let ranges = merge_ranges(ranges);
    if ranges.is_empty() {
        return crate::export::normalize_chinese_quotes(text);
    }
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for range in ranges {
        if range.start > cursor {
            output.push_str(&crate::export::normalize_chinese_quotes(
                &text[cursor..range.start],
            ));
        }
        output.push_str(&text[range.clone()]);
        cursor = range.end;
    }
    if cursor < text.len() {
        output.push_str(&crate::export::normalize_chinese_quotes(&text[cursor..]));
    }
    output
}

fn merge_ranges(mut ranges: Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// 代码围栏与行内代码的字节范围，按位置排序、合并。
fn code_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut line_start = 0usize;
    let mut fence: Option<(&'static str, usize)> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some((open, start)) = fence {
            if trimmed.starts_with(open) {
                ranges.push(start..line_start + line.len());
                fence = None;
            }
            line_start += line.len();
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some(("```", line_start));
        } else if trimmed.starts_with("~~~") {
            fence = Some(("~~~", line_start));
        }
        line_start += line.len();
    }
    if let Some((_, start)) = fence {
        ranges.push(start..text.len());
    }

    // 行内代码：同一行里成对的 1～3 个反引号。
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }
        let start = index;
        let mut ticks = 0usize;
        while index < bytes.len() && bytes[index] == b'`' {
            ticks += 1;
            index += 1;
        }
        if ticks > 3 {
            continue;
        }
        let marker = "`".repeat(ticks);
        if let Some(offset) = text[index..].find(&marker) {
            let end = index + offset + ticks;
            ranges.push(start..end);
            index = end;
        }
    }

    merge_ranges(ranges)
}

/// 行首 `1。`/`1．` 改成 Markdown 列表标记 `1.`。
///
/// 只认 1～9 位数字、行首可有空格缩进、后面还有内容；不动 `1。5` 这类小数
/// （数字后直接是句号，但后面紧跟数字时不改）。
fn normalize_list_markers(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, ending) = match line.strip_suffix("\n") {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        let trimmed = body.trim_start();
        let indent_len = body.len() - trimmed.len();
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        let valid = (1..=9).contains(&digits)
            && matches!(trimmed[digits..].chars().next(), Some('。' | '．'))
            && trimmed[digits + '。'.len_utf8()..]
                .chars()
                .next()
                .is_some_and(|ch| !ch.is_ascii_digit());
        if valid {
            output.push_str(&body[..indent_len]);
            output.push_str(&trimmed[..digits]);
            output.push('.');
            output.push_str(&trimmed[digits + '。'.len_utf8()..]);
            output.push_str(ending);
        } else {
            output.push_str(line);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_document_number_brackets_are_normalized() {
        let corrected =
            normalize_for_save("按照某办函[2026]12号、某办发【2025】3号、某办字（2024）7号办理。");
        assert_eq!(
            corrected.text,
            "按照某办函〔2026〕12号、某办发〔2025〕3号、某办字〔2024〕7号办理。"
        );
        assert_eq!(corrected.changes.len(), 3);
    }

    #[test]
    fn angle_and_corner_brackets_are_normalized() {
        let corrected = normalize_for_save("见某办函〈2026〉12号和某办函「2025」3号。");
        assert_eq!(corrected.text, "见某办函〔2026〕12号和某办函〔2025〕3号。");
    }

    #[test]
    fn citation_outer_and_inner_brackets_are_normalized() {
        let corrected = normalize_for_save("根据《关于商洽的函》(某办函[2026]1号)办理。");
        assert_eq!(
            corrected.text,
            "根据《关于商洽的函》（某办函〔2026〕1号）办理。"
        );
    }

    #[test]
    fn already_standard_document_numbers_are_left_alone() {
        let text = "按照某办函〔2026〕12号办理。";
        let corrected = normalize_for_save(text);
        assert_eq!(corrected.text, text);
        assert!(!corrected.changed());
    }

    #[test]
    fn fullwidth_list_marker_is_normalized_at_save() {
        let corrected = normalize_for_save("1。 第一项\n2． 第二项\n3.5万元\n");
        assert_eq!(corrected.text, "1. 第一项。\n2. 第二项。\n3.5万元\n");
    }

    #[test]
    fn spaces_are_normalized_but_code_is_kept() {
        let corrected = normalize_for_save("2026 年   5 人，正文 `a  b`。");
        assert_eq!(corrected.text, "2026年5人，正文 `a  b`。");
    }

    #[test]
    fn preview_correction_maps_ranges_back_to_source() {
        let source = "第一项...第二项";
        let corrected = normalize_for_preview(source, false);
        assert_eq!(corrected.text, "第一项……第二项");
        let start = source.find("第二项").unwrap();
        let mapped = corrected.map.to_new(start..start + "第二项".len());
        assert_eq!(&corrected.text[mapped.clone()], "第二项");
        assert_eq!(corrected.map.to_old(mapped), start..start + "第二项".len());
    }

    #[test]
    fn preview_does_not_judge_list_tail() {
        let corrected = normalize_for_preview("1. 甲\n1. 乙\n", false);
        assert_eq!(corrected.text, "1. 甲\n1. 乙\n");
    }

    #[test]
    fn urls_emails_domains_and_paths_are_protected() {
        let corrected = normalize_for_save(
            "见www.例子.中国，邮箱a@b.com，路径C:\\我的 文件\\a.docx，正常,标点。",
        );
        assert!(corrected.text.contains("www.例子.中国"));
        assert!(corrected.text.contains("a@b.com"));
        assert!(corrected.text.contains("C:\\我的 文件\\a.docx"));
        assert!(corrected.text.contains("正常，标点。"));
    }

    #[test]
    fn preview_style_converts_paired_parentheses_but_not_numbering() {
        let text = "接口(API)调用 (1) 编号";
        let chars = text.chars().collect::<Vec<_>>();
        let styled = style_chars(&chars, &vec![false; chars.len()]);
        assert_eq!(
            styled.iter().collect::<String>(),
            "接口（API）调用 (1) 编号"
        );
    }

    #[test]
    fn preview_style_converts_halfwidth_but_keeps_numbering_and_placeholders() {
        let text = "工作,通知 见??。";
        let chars = text.chars().collect::<Vec<_>>();
        let styled = style_chars(&chars, &vec![false; chars.len()]);
        assert_eq!(styled.iter().collect::<String>(), "工作，通知 见??。");
    }

    #[test]
    fn halfwidth_repeats_and_ellipsis_are_normalized() {
        let corrected = normalize_for_save("工作,通知;时间:上午(附件)!!他说...");
        assert_eq!(corrected.text, "工作，通知；时间：上午（附件）！他说……");
    }

    #[test]
    fn quotes_are_normalized_but_code_is_left_alone() {
        let corrected = normalize_for_save("他说\"第一句\"，示例 `println!(\"x\")`。");
        assert_eq!(corrected.text, "他说“第一句”，示例 `println!(\"x\")`。");
    }

    #[test]
    fn fenced_code_is_not_touched() {
        let text = "```rust\nlet s = \"a,b\";\n```\n正文,\"引号\"";
        let corrected = normalize_for_save(text);
        assert!(corrected.text.contains("let s = \"a,b\";"));
        assert!(corrected.text.contains("正文，“引号”"));
    }

    #[test]
    fn clean_sample_is_idempotent() {
        let sample = include_str!("../examples/标准带附件函件测试样例.md");
        let once = normalize_for_save(sample);
        let twice = normalize_for_save(&once.text);
        assert!(
            !twice.changed(),
            "保存 pass 必须幂等：第二遍又改了 {}",
            twice.summary()
        );
        // 样例里 `接口(API)` 是中文语境里的半角括号，第一遍应规范成全角；
        // 但不应再牵动列表末尾或文号。
        assert!(once.changes.len() <= 1, "{}", once.summary());
    }

    #[test]
    fn research_list_tail_is_not_rewritten() {
        let corrected = normalize_for_save_for_kind("1. 甲\n1. 乙\n", true);
        assert_eq!(corrected.text, "1. 甲\n1. 乙\n");
    }

    #[test]
    fn list_tail_is_judged_once_for_the_whole_group() {
        let corrected = normalize_for_save("1. 第一项，\n1. 第二项。；\n1. 第三项\n");
        assert_eq!(corrected.text, "1. 第一项；\n1. 第二项；\n1. 第三项。\n");
        assert_eq!(corrected.changes.len(), 1);
    }

    #[test]
    fn list_tail_keeps_question_exclamation_ellipsis_and_dash() {
        let corrected =
            normalize_for_save("1. 是否同意？\n1. 请注意！\n1. 详见……\n1. 原文——\n1. 末项\n");
        assert_eq!(
            corrected.text,
            "1. 是否同意？\n1. 请注意！\n1. 详见……\n1. 原文——\n1. 末项。\n"
        );
    }

    #[test]
    fn quotes_inside_ellipsis_are_not_stripped() {
        let corrected = normalize_for_save("1. 他说“……”\n1. 末项\n");
        assert_eq!(corrected.text, "1. 他说“……”\n1. 末项。\n");
    }
}
