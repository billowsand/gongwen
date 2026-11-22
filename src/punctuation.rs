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
}

impl Corrected {
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
    let mut changes = Vec::new();

    // 1. 公文引用与发文字号。两边的识别都在原文上做，互不重叠：
    //    `detect_loose_numbers` 会跳过已识别为引用的范围。
    let mut text = normalize_document_numbers(markdown, &mut changes);

    // 2. 粘贴/导入带来的全角列表标记 `1。`/`1．`：输入层只管手打，
    //    这里兜底，让后续列表末尾判断能认出它。
    let markers = normalize_list_markers(&text);
    if markers != text {
        changes.push(Change {
            label: "列表标记 1。→1.".to_string(),
        });
        text = markers;
    }

    // 3. 半角/全角、重复标点、省略号。代码、公式、链接、URL 由
    //    `normalize_punctuation` 自己跳过。
    let basic = crate::doc_import::clean::normalize_punctuation(&text);
    if basic != text {
        changes.push(Change {
            label: "规范半角标点、重复标点与省略号".to_string(),
        });
        text = basic;
    }

    // 4. 引号配对。代码围栏与行内代码原样保留。
    let quoted = normalize_quotes_outside_code(&text);
    if quoted != text {
        changes.push(Change {
            label: "规范中文引号".to_string(),
        });
        text = quoted;
    }

    // 5. 有序列表末尾标点：整篇扫一遍，按每个连续列表组判断。
    //    研究报告不套公文的 `；`/`。` 收尾规则：研报列表常以单位、英文缩写或公式
    //    结尾，套上去反而是错的（与 `finalize_markdown` 同一口径）。
    let normalized = if research {
        text.clone()
    } else {
        crate::export::normalize_ordered_list_punctuation(&text)
    };
    if normalized != text {
        changes.push(Change {
            label: "统一列表末尾句号/分号".to_string(),
        });
    }

    Corrected {
        text: normalized,
        changes,
    }
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
