//! 「清洗」分区的纯文本处理：公文编号与 Markdown 编号互转、剥掉标记与段首编号。
//!
//! 这里只做确定性的文本替换，不碰 egui：输入输出都是一个 Markdown 字符串，
//! 按钮层只负责取稿子、调函数、写回并重新校验。逻辑能单独测。

use crate::models::{HeadingNumbering, NumberingConfig};
use regex::Regex;
use std::sync::OnceLock;

/// 行首的 Markdown 标题：`#` 到 `######` 后必须有一个空白，与导出器的认法一致。
///
/// 返回标题层级（`#` 的个数）与去掉标记后的标题文字。
fn heading_at(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    let text = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('\t'))?;
    Some((hashes, text.trim()))
}

/// 统计二级至五级 Markdown 标题（`##`–`#####`）的行数，用于状态栏报个数。
pub(crate) fn heading_line_count(markdown: &str) -> usize {
    markdown
        .split('\n')
        .filter(|line| heading_at(line).is_some_and(|(level, _)| (2..=5).contains(&level)))
        .count()
}

/// 一键把 Markdown 标题展开成公文编号的正文段落。
///
/// `# ` 是文档标题，原样保留；`##` 到 `#####` 各自连续编号，遇到更高级的标题
/// 就把低级计数清零（例如 `##` 之后的下一个 `###` 又从「（一）」开始）。
/// 标题文字先经 [`crate::export::clean_heading_number`] 去掉手写编号，编号文字按
/// 设置里的样式生成。列表行与正文行都不动。
///
/// 区段标记（`<!-- [附件] -->`、`<!-- [附录] -->` 与英文 attachment / appendix 的
/// 等价写法，以及 `<!-- [正文] -->`）处四级计数清零：附件里的编号从「一、」重新
/// 开始，与导出器 [`crate::export::HeadingCounters`] 的走法一致。
pub(crate) fn markdown_to_business_numbering(
    markdown: &str,
    numbering: &NumberingConfig,
) -> String {
    let mut counters = [0usize; 4];
    let mut out: Vec<String> = Vec::new();
    for line in markdown.split('\n') {
        // 区段标记本身不是标题，先把计数器清零再原样输出。
        if crate::export::parse_section_marker(line).is_some() {
            counters = [0usize; 4];
            out.push(line.to_string());
            continue;
        }
        let Some((level, text)) = heading_at(line) else {
            out.push(line.to_string());
            continue;
        };
        // `#` 是文档标题；`######` 导出器不认，两者都原样留着。
        if level == 1 || level > 5 {
            out.push(line.to_string());
            continue;
        }
        let Some(prefix) =
            crate::export::official_heading_prefix(level as u8, &mut counters, numbering)
        else {
            out.push(line.to_string());
            continue;
        };
        let text = crate::export::clean_heading_number(text);
        // DecimalDot 的「1.」落在行首会被 Markdown 当成有序列表，转义成「1\\.」。
        // 预览与导出的行内解析都认这个转义，印出来仍是「1.」。
        if numbering.heading(level as u8) == Some(HeadingNumbering::DecimalDot) {
            let number = prefix.trim_end_matches('.');
            out.push(format!("{}\\.{text}", number));
        } else {
            out.push(format!("{prefix}{text}"));
        }
    }
    out.join("\n")
}

/// 去掉行首的 Markdown 标记：标题变成正文段落，列表项只留文字。
///
/// 认的标记与导出器一致：`#` 到 `######`、`- `、`* `、`+ ` 与 `1. ` / `1) `。
/// 已转义成 `1\\.` 的编号不算列表标记，保持原样。
pub(crate) fn strip_markdown_markers(markdown: &str) -> String {
    markdown
        .split('\n')
        .map(strip_line_markers)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 去掉一整行开头的 Markdown 标记，返回剩下的文字。
fn strip_line_markers(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some((_, text)) = heading_at(trimmed) {
        return text.to_string();
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest.trim_start().to_string();
        }
    }
    if let Some(caps) = ordered_marker().captures(trimmed) {
        return caps
            .get(1)
            .map_or(String::new(), |text| text.as_str().trim_start().to_string());
    }
    line.to_string()
}

/// 去掉 Markdown 标记后，再剥掉段首的手打公文编号。
///
/// 认的编号：`一、`、`（一）`、`1.`、`1、`、`（1）`、`①`–`⑳`、
/// `第一章`/`第一节`/`第一部分`/`第一条`。像「一、二季度」这种两个季度并称的
/// 枚举不是编号，留着不动，免得把意思改坏。
pub(crate) fn strip_all_numbering(markdown: &str) -> String {
    markdown
        .split('\n')
        .map(|line| {
            let without_markers = strip_line_markers(line);
            strip_business_number(&without_markers).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 剥掉一段文字开头的一个公文编号。
fn strip_business_number(line: &str) -> &str {
    let text = line.trim_start();
    if text.is_empty() {
        return line;
    }
    if let Some(rest) = business_number_end(text) {
        let rest = rest.trim_start();
        if !rest.is_empty() {
            return rest;
        }
    }
    line
}

/// 若开头是一个公文编号，返回它后面的文字；不是编号返回 `None`。
fn business_number_end(text: &str) -> Option<&str> {
    // 圈号 ①–⑳，后面可能跟一个顿号。
    if let Some(ch) = text.chars().next()
        && ('①'..='⑳').contains(&ch)
    {
        let rest = &text[ch.len_utf8()..];
        return Some(rest.strip_prefix('、').unwrap_or(rest));
    }
    if let Some(caps) = chinese_numbering().captures(text) {
        let rest = caps.get(1).map_or("", |text| text.as_str());
        // 「一、二季度」是并列枚举，不是编号。
        if !looks_like_enumeration(rest) {
            return Some(rest);
        }
        return None;
    }
    if let Some(caps) = parenthesized_numbering().captures(text) {
        return Some(caps.get(1).map_or("", |text| text.as_str()));
    }
    if let Some(caps) = chapter_numbering().captures(text) {
        return Some(caps.get(1).map_or("", |text| text.as_str()));
    }
    if let Some(rest) = decimal_number_end(text)
        && !looks_like_enumeration(rest)
    {
        return Some(rest);
    }
    None
}

/// 编号后面紧跟的是不是同一组枚举的下一项，例如「一、二季度」「1、2月份」。
///
/// 这种「编号」其实是并列枚举的一部分（一、二季度 = 第一季度与第二季度），
/// 剥掉会改坏原意，所以认作正文而不是编号。
fn looks_like_enumeration(rest: &str) -> bool {
    let rest = rest.trim_start();
    let mut chars = rest.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_digit() && !is_chinese_numeral(first) {
        return false;
    }
    let tail = &rest[first.len_utf8()..];
    /// 枚举第二项后面的单位词或连接词。
    const UNITS: &[&str] = &[
        "季度", "月份", "月", "年", "期", "届", "批", "次", "号", "日", "天", "周", "时", "点",
        "个", "项", "名", "位", "家", "种", "类", "级", "层", "段", "步", "页", "、", "，", ",",
        "和", "及", "至", "到", "-", "—", "~", "～",
    ];
    UNITS.iter().any(|unit| tail.starts_with(unit))
}

/// 汉字数字，含大写与「〇」。
fn is_chinese_numeral(ch: char) -> bool {
    matches!(
        ch,
        '一' | '二'
            | '三'
            | '四'
            | '五'
            | '六'
            | '七'
            | '八'
            | '九'
            | '十'
            | '百'
            | '千'
            | '零'
            | '〇'
            | '两'
    )
}

fn ordered_marker() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\d+[.)]\s+(.*)$").expect("有序列表标记正则"))
}

fn chinese_numbering() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^[一二三四五六七八九十百千零〇两]+、\s*(.*)$").expect("汉字编号正则")
    })
}

fn parenthesized_numbering() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^[（(](?:[一二三四五六七八九十百千零〇两]+|\d+)[）)]\s*[、.．]?\s*(.*)$")
            .expect("括号编号正则")
    })
}

fn chapter_numbering() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^第[一二三四五六七八九十百千零〇两\d]+(?:章|节|部分|条)\s*[、.．:：]?\s*(.*)$")
            .expect("章节编号正则")
    })
}

/// 数字编号：`1.`、`1．`、`1、`、`1)` 或 `1）`，以及「Markdown→公文编号」
/// 生成的转义写法 `1\.`。
///
/// 「1.5 万元」这种小数不是编号：点号后面紧跟数字时不认。用逐字符判断而不是
/// 正则的前瞻——regex crate 不支持 look-ahead。
fn decimal_number_end(text: &str) -> Option<&str> {
    let digits = text.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let mut chars = text[digits..].chars();
    let mark = chars.next()?;
    // 反向转换生成的 `1\.`：反斜杠后面才是点号。
    if mark == '\\' {
        let escaped = chars.next()?;
        if !matches!(escaped, '.' | '．' | '、') {
            return None;
        }
        let after = chars.as_str();
        if after.starts_with(|ch: char| ch.is_ascii_digit()) {
            return None;
        }
        return Some(after);
    }
    let after = chars.as_str();
    match mark {
        '）' | ')' => Some(after),
        '.' | '．' | '、' => {
            if after.starts_with(|ch: char| ch.is_ascii_digit()) {
                return None;
            }
            Some(after)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 标题转公文编号时逐级连续并在上级标题处重置() {
        let numbering = NumberingConfig::default();
        let markdown = "# 关于做好工作的通知\n\n## 总体要求\n\n### 提高认识\n\n### 加强领导\n\n## 主要任务\n\n### 细化措施\n";
        let converted = markdown_to_business_numbering(markdown, &numbering);
        assert_eq!(
            converted,
            "# 关于做好工作的通知\n\n一、总体要求\n\n（一）提高认识\n\n（二）加强领导\n\n二、主要任务\n\n（一）细化措施\n"
        );
    }

    #[test]
    fn 小数编号样式在段首转义以免被当成列表() {
        let numbering = NumberingConfig::default();
        let markdown = "## 加强领导\n\n#### 具体措施\n";
        let converted = markdown_to_business_numbering(markdown, &numbering);
        assert_eq!(converted, "一、加强领导\n\n1\\.具体措施\n");
    }

    #[test]
    fn 标题已有的手写编号先剥掉再按样式生成() {
        let numbering = NumberingConfig::default();
        let markdown = "## 一、总体要求\n### （一）提高认识\n";
        let converted = markdown_to_business_numbering(markdown, &numbering);
        assert_eq!(converted, "一、总体要求\n（一）提高认识\n");
    }

    #[test]
    fn 附件标记后编号重新开始() {
        let numbering = NumberingConfig::default();
        for marker in ["<!-- [附件] -->", "<!-- [附录] -->", "<!-- [appendix] -->"] {
            let markdown =
                format!("## 总体要求\n\n{marker}\n\n# 物资清单\n\n## 品种\n\n### 规格\n");
            let converted = markdown_to_business_numbering(&markdown, &numbering);
            assert_eq!(
                converted,
                format!("一、总体要求\n\n{marker}\n\n# 物资清单\n\n一、品种\n\n（一）规格\n"),
                "{marker}"
            );
        }
    }

    #[test]
    fn 去掉标记后标题与列表都变正文() {
        let markdown =
            "# 标题\n\n## 一、总体要求\n\n- 第一项\n* 第二项\n+ 第三项\n1. 第四项\n正文\n";
        assert_eq!(
            strip_markdown_markers(markdown),
            "标题\n\n一、总体要求\n\n第一项\n第二项\n第三项\n第四项\n正文\n"
        );
    }

    #[test]
    fn 清除编号覆盖常见公文编号() {
        let markdown =
            "## 一、总体要求\n（一）提高认识\n1、具体措施\n（1）细节\n①第一步\n第一章　总则\n";
        assert_eq!(
            strip_all_numbering(markdown),
            "总体要求\n提高认识\n具体措施\n细节\n第一步\n总则\n"
        );
    }

    #[test]
    fn 清除编号时不误伤两个季度并称的枚举() {
        let markdown = "一、二季度工作安排\n一、加强领导\n二、二月份任务\n1、2月份工作\n";
        assert_eq!(
            strip_all_numbering(markdown),
            "一、二季度工作安排\n加强领导\n二、二月份任务\n1、2月份工作\n"
        );
    }

    #[test]
    fn 清除编号不误伤小数并认转义写法() {
        assert_eq!(strip_all_numbering("1.5万元\n"), "1.5万元\n");
        assert_eq!(strip_all_numbering("1\\.具体措施\n"), "具体措施\n");
    }

    #[test]
    fn 末尾换行与空行保持原样() {
        let markdown = "## 标题\n\n正文\n";
        for converted in [
            markdown_to_business_numbering(markdown, &NumberingConfig::default()),
            strip_markdown_markers(markdown),
            strip_all_numbering(markdown),
        ] {
            assert!(converted.ends_with("\n"), "{converted:?}");
            assert_eq!(converted.split('\n').count(), markdown.split('\n').count());
        }
    }
}
