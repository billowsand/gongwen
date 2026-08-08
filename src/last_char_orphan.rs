//! 段落末挂单字检测。
//!
//! 公文正文每行装 28 字（三号仿宋），首行缩进 2 字后剩 26 字宽度当量；
//! 标点符号在版式上比汉字窄，按 0.5 倍宽度计入。Markdown 修饰符不占宽度。
//!
//! 输入是 Markdown 文本：换两行才算段内分段，软换行（在编辑器里按住 Shift+Enter
//! 写下的单个 `\n`）仍属于同一段，必须与段落拼接后再算宽度。本模块不依赖
//! 任何导出器，只做纯字符宽度模拟——既给 `validator::validate` 用，也给
//! 起草页审校提示用。
//!
//! 判定逻辑（保守）：
//! - 跳过标题行（`#` / `##` / `###` 等）、空行、Mardown 列表项前缀（`- ` / `* ` / `1. ` / `、`）。
//! - 段总宽度 ≤ 首行容量 → 不会换行，不报。
//! - 段总宽度 ≤ 首行 + 满行容量 → 装得下两行但末行很短，仅在末行剩余 < 1.5 字时报。
//! - 段总宽度 > 首行 + 满行容量 → 至少三行，按 28 - 28 - ... - 末行剩余模拟；末行剩余 < 1.5 字时报。
//! - 段末不是标点收尾（最后一个有效字符是汉字 / 数字 / 字母）→ 加重提示"段末未用标点收尾"。
//!   出现"section 区段标记"（`<!-- [附件] -->` 等）会切断前后段，避免把附件名也算进正文宽度。

const CHARS_PER_LINE_DEFAULT: f32 = 28.0;
const FIRST_LINE_CHARS_DEFAULT: f32 = 26.0;
const PUNCTUATION_FACTOR: f32 = 0.5;
const ORPHAN_THRESHOLD: f32 = 1.5;
const MIN_LINES_FOR_CHECK: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct OrphanWarning {
    /// 段落在 Markdown 里的 1-based 行号（首行）。
    pub start_line: usize,
    /// 段落宽度当量（已应用标点压缩）。
    pub display_width: f32,
    /// 模拟行数。
    pub lines: usize,
    /// 末行剩余宽度当量。
    pub tail_width: f32,
    /// 段末是否没有标点收尾。
    pub missing_terminator: bool,
    /// 段落前 40 字（用作提示预览）。
    pub preview: String,
}

/// 统计一段文本的宽度当量：汉字 1.0，标点 0.5，ASCII 0.5，空白 0，
/// Markdown 修饰符（`**` `__` 等）不计入；行内反引号代码同理。
fn display_width(text: &str) -> f32 {
    let mut total = 0.0f32;
    let mut chars = text.chars().peekable();
    let mut in_code = false;
    while let Some(ch) = chars.next() {
        match ch {
            '`' => {
                in_code = !in_code;
            }
            c if in_code => {
                // 反引号内的代码不算内容，仅占位。
                let _ = c;
            }
            '*' | '_' => {
                // 吞成对修饰符：连续相同字符成对；奇数个时按 Markdown 规则保留一个。
                if chars.peek().copied() == Some(ch) {
                    chars.next();
                } else {
                    // 奇数个修饰符保留字面，按 ASCII 处理。
                    total += PUNCTUATION_FACTOR;
                }
            }
            '\\' => {
                // Markdown 转义：吞掉下一个字符。
                chars.next();
            }
            ' ' | '\t' => {
                // 空白不计。
            }
            '\u{3000}' => {
                // 全角空格不计。
            }
            c if is_chinese_punctuation(c) => {
                total += PUNCTUATION_FACTOR;
            }
            c if is_chinese_char(c) => {
                total += 1.0;
            }
            c if c.is_ascii_digit() || c.is_ascii_alphabetic() => {
                total += PUNCTUATION_FACTOR;
            }
            _ => {
                // 其他（拉丁标点等）按 0.5 计入。
                total += PUNCTUATION_FACTOR;
            }
        }
    }
    total
}

fn is_chinese_char(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF)
}

fn is_chinese_punctuation(c: char) -> bool {
    // 公文常用标点（含全角与半角括号、引号、顿号、破折号、省略号等）。
    matches!(
        c,
        '。' | '，'
            | '；'
            | '：'
            | '？'
            | '！'
            | '、'
            | '·'
            | '—'
            | '…'
            | '（'
            | '）'
            | '【'
            | '】'
            | '《'
            | '》'
            | '\"'
            | '\''
            | '「'
            | '」'
            | '『'
            | '』'
            | '〈'
            | '〉'
            | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '-'
            | '~'
    )
}

/// 段末是否以标点收尾（决定是否给"未用标点收尾"提示）。
fn ends_with_terminator(text: &str) -> bool {
    let trimmed = text.trim_end().trim_end_matches(['*', '_', '`']);
    let Some(last) = trimmed.chars().last() else {
        return false;
    };
    is_chinese_punctuation(last)
}

/// 把 Markdown 文本切成段落（两换行才算分段；区段标记切断前后）。
/// 返回 `(段首行号, 段落文本)`。
fn split_paragraphs(markdown: &str) -> Vec<(usize, String)> {
    let mut paragraphs = Vec::new();
    let mut buf = String::new();
    let mut start_line: Option<usize> = None;
    let mut prev_blank = true;
    for (idx, raw_line) in markdown.lines().enumerate() {
        let line = raw_line.trim_end();
        let line_number = idx + 1;
        let is_blank = line.trim().is_empty();
        let is_section_marker = is_section_marker(line);
        if is_section_marker {
            if let Some(start) = start_line.take() {
                flush_paragraph(&mut buf, start, &mut paragraphs);
            }
            buf.clear();
            prev_blank = true;
            continue;
        }
        if is_blank && !prev_blank {
            // 单空行代表段落分隔；但若缓冲区为空（连续空行），跳过。
            if !buf.is_empty()
                && let Some(start) = start_line.take()
            {
                flush_paragraph(&mut buf, start, &mut paragraphs);
            }
            prev_blank = true;
            continue;
        }
        if !is_blank {
            if start_line.is_none() {
                start_line = Some(line_number);
            }
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(line);
            prev_blank = false;
        }
    }
    if let Some(start) = start_line.take() {
        flush_paragraph(&mut buf, start, &mut paragraphs);
    }
    paragraphs
}

fn flush_paragraph(buf: &mut String, start: usize, out: &mut Vec<(usize, String)>) {
    let trimmed = buf.trim().to_string();
    buf.clear();
    if trimmed.is_empty() {
        return;
    }
    out.push((start, trimmed));
}

fn is_section_marker(line: &str) -> bool {
    let trimmed = line.trim();
    if !(trimmed.starts_with("<!--") && trimmed.ends_with("-->")) {
        return false;
    }
    let inner = trimmed
        .trim_start_matches("<!--")
        .trim_end_matches("-->")
        .trim()
        .trim_start_matches(['[', '【'])
        .trim_end_matches([']', '】'])
        .trim();
    matches!(
        inner.to_ascii_lowercase().as_str(),
        "正文" | "body" | "附件" | "附录" | "attachment" | "attachments" | "appendix"
    )
}

/// 清理段落里的 Markdown 修饰符与列表项前缀，但保留段内换行换算成的空格。
fn clean_paragraph(text: &str) -> String {
    let mut out = String::new();
    let mut after_newline = true;
    for raw_line in text.lines() {
        let line = strip_list_prefix(raw_line);
        if !out.is_empty() && !after_newline {
            out.push('\n');
        }
        out.push_str(&line);
        after_newline = false;
    }
    out
}

/// 剥掉段首的列表项标记：`- ` `* ` `+ ` `1. ` `1）` `（一）` `一、` `㈠` 等。
fn strip_list_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return line.to_string();
    }
    // `-` `*` `+` 三种无序列表
    if matches!(bytes[0], b'-' | b'*' | b'+') && trimmed.get(1..2) == Some(" ") {
        return trimmed[2..].to_string();
    }
    // `1.` `2)` `10、` 数字列表
    let mut idx = 0;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx > 0 && idx < bytes.len() && matches!(bytes[idx], b'.' | b')' | b',') {
        let rest = trimmed[idx + 1..].trim_start();
        if rest.chars().next().is_some() {
            return rest.to_string();
        }
    }
    // `一、` `二、` 等中文数字列表（简化识别：以顿号结尾的前缀）
    if let Some(dun_pos) = trimmed.find('、') {
        let prefix = &trimmed[..dun_pos];
        if !prefix.is_empty() && prefix.chars().count() <= 4 {
            let rest = trimmed[dun_pos + 3..].trim_start();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    line.to_string()
}

/// 计算段落装载行数与末行剩余宽度。
/// 段首缩进 2 字已在 `first_line` 里预留（默认 26 < 28 = 28-2）。
fn simulate_layout(text: &str, chars_per_line: f32, first_line: f32) -> (f32, usize, f32) {
    let total = display_width(text);
    if total <= first_line {
        return (total, 1, total);
    }
    let mut remaining = total - first_line;
    let mut lines = 1;
    loop {
        if remaining < chars_per_line {
            let tail = remaining;
            return (total, lines + 1, tail);
        }
        remaining -= chars_per_line;
        lines += 1;
        // 防止极长段落永不收敛（理论不会，宽度是有限值）。
        if lines > 1000 {
            return (total, lines, remaining);
        }
    }
}

/// 公开入口：扫描 Markdown 文档，返回所有"末挂单字"的段落。
pub fn find_orphans(markdown: &str) -> Vec<OrphanWarning> {
    find_orphans_with(markdown, CHARS_PER_LINE_DEFAULT, FIRST_LINE_CHARS_DEFAULT)
}

/// 自定义每行字数的扫描版本。
pub fn find_orphans_with(
    markdown: &str,
    chars_per_line: f32,
    first_line: f32,
) -> Vec<OrphanWarning> {
    let mut warnings = Vec::new();
    for (start_line, raw) in split_paragraphs(markdown) {
        let cleaned = clean_paragraph(&raw);
        if is_skippable(&cleaned) {
            continue;
        }
        let (total, lines, tail) = simulate_layout(&cleaned, chars_per_line, first_line);
        if lines < MIN_LINES_FOR_CHECK {
            continue;
        }
        let missing_terminator = !ends_with_terminator(&cleaned);
        // 段末有标点收尾时，末行短不算孤儿，是公文体允许的。
        if !missing_terminator {
            continue;
        }
        // 末行剩余 < 1.5 字宽度才算"挂单字"。
        if tail >= ORPHAN_THRESHOLD {
            continue;
        }
        warnings.push(OrphanWarning {
            start_line,
            display_width: total,
            lines,
            tail_width: tail,
            missing_terminator,
            preview: preview_text(&raw),
        });
    }
    warnings
}

fn is_skippable(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with('#') {
        return true;
    }
    if trimmed.is_empty() {
        return true;
    }
    false
}

fn preview_text(raw: &str) -> String {
    let collapsed: String = raw
        .chars()
        .take(40)
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    collapsed
}

/// 给出对应用语，1 段最多 1 条提示。
pub fn format_warning(warning: &OrphanWarning) -> String {
    let mut msg = format!(
        "第 {} 行段落末行可能挂单字（{:.0} 字宽度，末行剩余 {:.1}）",
        warning.start_line, warning.display_width, warning.tail_width
    );
    if warning.missing_terminator {
        msg.push_str("，且末字未用标点收尾");
    }
    msg.push('：');
    msg.push_str(&warning.preview);
    msg.push('…');
    msg
}

/// 仅出于完整性校验：从 Markdown 里枚举段落，便于调试或导出预览。
#[cfg(test)]
fn enumerate_paragraphs(markdown: &str) -> Vec<(usize, String)> {
    split_paragraphs(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_orphan_count(text: &str, expected: usize) {
        let warnings = find_orphans(text);
        assert_eq!(
            warnings.len(),
            expected,
            "段落末挂单字检测数量不符：期望 {expected}，实际 {}；warnings = {warnings:?}",
            warnings.len()
        );
    }

    #[test]
    fn short_paragraphs_are_not_flagged() {
        // 单行段不会被报告：宽度 ≤ 首行 26。
        assert_orphan_count("现就有关事项函告如下。", 0);
    }

    #[test]
    fn two_line_paragraph_with_terminator_passes() {
        // 两行段，末行剩余约 12 字宽度，远大于阈值，不报。
        let text = "现就2026年度网络安全考核评估暨重点工作任务落实情况检查的有关事项函告如下，请各单位结合实际认真贯彻落实执行；如有问题请及时反馈。";
        assert_orphan_count(text, 0);
    }

    #[test]
    fn two_line_paragraph_ending_in_orphan_is_flagged() {
        // 模拟"两行后只余 1 个字"：前 50 宽（> 26 必断到第 2 行）后第 2 行只有 0.5 个字。
        let body = "为切实做好2026年度工作总结及2027年度工作谋划工作，请各部门按时报送相关材料";
        // 上面这段最后一个字是"料"，汉字 = 1.0；模拟：
        let (total, lines, tail) = simulate_layout(body, 28.0, 26.0);
        assert!(total > 26.0);
        assert!(lines == 2);
        // 末行剩余可能刚好 1.0，触发报告。
        let orphans = find_orphans(body);
        if tail < 1.5 {
            assert_eq!(orphans.len(), 1);
            assert!(orphans[0].missing_terminator, "末字未用标点收尾");
        }
    }

    #[test]
    fn hard_orphan_paragraph_is_flagged() {
        // 构造必挂单字：先填满首行 26，再填第二行 28，但末行只余 1 字且未用标点。
        let mut first_line = String::new();
        // 用汉字精确填充到宽度 26（不含 ASCII 与标点的复杂度，全部汉字）。
        for _ in 0..26 {
            first_line.push('的');
        }
        let second_full = "中".repeat(28);
        // 末尾一个孤字。
        let text = format!("{first_line}{second_full}应");
        let warnings = find_orphans(&text);
        assert_eq!(warnings.len(), 1, "应识别孤字段");
        assert!(warnings[0].missing_terminator);
        assert!(warnings[0].tail_width < 1.5);
    }

    #[test]
    fn orphan_with_terminator_passes() {
        // 段末用标点收尾时，即便末行短也不报（公文体允许）。
        let mut first_line = String::new();
        for _ in 0..26 {
            first_line.push('的');
        }
        let second_full = "中".repeat(28);
        let text = format!("{first_line}{second_full}。");
        let warnings = find_orphans(&text);
        assert!(warnings.is_empty(), "标点收尾时不应报告：{warnings:?}");
    }

    #[test]
    fn markdown_modifiers_are_ignored() {
        // 纯修饰符应不占宽度。
        let raw = "****";
        let width = display_width(raw);
        assert!(width < 0.01, "纯修饰符应占 0：{width}");
        // 包裹内部汉字仍按汉字计。
        let raw = "**重要通知**";
        let width = display_width(raw);
        assert!(
            (width - 4.0).abs() < 0.01,
            "修饰符包裹 4 个汉字应占 4.0：{width}"
        );
    }

    #[test]
    fn list_prefix_is_stripped() {
        let cleaned = clean_paragraph("- 第一项内容");
        assert!(cleaned.starts_with("第一项内容"));
        let cleaned = clean_paragraph("1. 第一项内容");
        assert!(cleaned.starts_with("第一项内容"));
        let cleaned = clean_paragraph("一、第一项内容");
        assert!(cleaned.starts_with("第一项内容"));
    }

    #[test]
    fn headings_are_skipped() {
        let text = "# 大标题\n\n正文内容";
        assert_orphan_count(text, 0);
    }

    #[test]
    fn section_marker_splits_paragraphs() {
        // 区段标记切断前后，避免把附件名前缀也算进正文末行。
        let text = "正文第一段。\n\n<!-- [附件] -->\n\n附件1.md";
        assert_orphan_count(text, 0);
    }

    #[test]
    fn punctuation_factor_compresses_correctly() {
        // 3 个汉字 = 3.0。
        assert_eq!(display_width("啊啊啊"), 3.0);
        // 7 个全角标点（含句号）= 7.0 × 0.5 = 3.5。
        assert_eq!(display_width("。；：？？！！"), 7.0 * PUNCTUATION_FACTOR);
    }

    #[test]
    fn ascii_is_half_width() {
        assert_eq!(display_width("ABCD"), 4.0 * PUNCTUATION_FACTOR);
        assert_eq!(display_width("1234"), 4.0 * PUNCTUATION_FACTOR);
    }

    #[test]
    fn layout_simulation_matches_hand_calc() {
        // 28 个汉字 = 28.0，应恰好占第二行整行。
        let (total, lines, tail) = simulate_layout(&"啊".repeat(54), 28.0, 26.0);
        assert_eq!(total, 54.0);
        assert_eq!(lines, 3);
        assert_eq!(tail, 0.0);
    }

    #[test]
    fn soft_newline_stays_in_same_paragraph() {
        // Markdown 段内单换行应拼成一段；这里手写两个物理行。
        let text = "现就有关事项\n函告如下。";
        let paragraphs = enumerate_paragraphs(text);
        assert_eq!(paragraphs.len(), 1);
    }

    #[test]
    fn hard_blank_splits_paragraphs() {
        let text = "第一段。\n\n第二段。";
        let paragraphs = enumerate_paragraphs(text);
        assert_eq!(paragraphs.len(), 2);
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(find_orphans("").is_empty());
        assert!(find_orphans("# 标题\n\n").is_empty());
    }

    #[test]
    fn real_world_paragraph_is_safe() {
        let text = "为切实做好2026年度网络安全工作，全面提升我省教育系统网络安全防护能力和水平，\
        根据《2026年度全国教育系统网络安全工作要点》部署要求，经研究决定在全省教育系统范围内\
        开展网络安全检查评估工作，现将有关事项通知如下。";
        let warnings = find_orphans(text);
        assert!(warnings.is_empty(), "常规段不应触发：{warnings:?}");
    }

    #[test]
    fn format_warning_includes_preview() {
        let warning = OrphanWarning {
            start_line: 12,
            display_width: 56.0,
            lines: 3,
            tail_width: 1.0,
            missing_terminator: true,
            preview: "为切实做好2026年度网络安全工作".into(),
        };
        let text = format_warning(&warning);
        assert!(text.contains("第 12 行"));
        assert!(text.contains("末行剩余 1.0"));
        assert!(text.contains("未用标点收尾"));
        assert!(text.contains("为切实做好"));
    }
}
