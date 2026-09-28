//! 引用块 `>` 的行级识别：研究报告的引文与文框都从这里认。
//!
//! 公文助手的预览解析器也调这几个函数（`mdx::quote`），两边的认法只有一份。
//!
//! - 行首 `>` 后跟空格或直接到行尾才算引用，`>5%` 这种写法仍是正文；
//! - 要在行首写字面的 `>`，写成 `\>`（反斜杠转义，行内解析时去掉反斜杠）；
//! - 首行写 `[!名称] 标题` 的是文框（专栏、案例、例子……写什么印什么），其余是引文；
//! - 引文里以 `——` 开头的一行是出处。

/// 去掉行首的引用标记，返回其后的内容（已去首尾空白）；不是引用行返回 None。
///
/// 连写的多层 `> >`、`>>` 一律压成一层：研究报告不排嵌套引用。
pub fn strip_marker(line: &str) -> Option<&str> {
    let mut rest = line.trim();
    let mut matched = false;
    while let Some(after) = rest.strip_prefix('>') {
        if !(after.is_empty() || after.starts_with([' ', '\t', '>'])) {
            break;
        }
        matched = true;
        rest = after.trim_start();
    }
    matched.then_some(rest.trim_end())
}

/// 文框的首行 `[!名称] 标题`：返回（名称，标题）。名称写什么印什么——专栏、
/// 案例、例子……，每种名称各编各的号。标题可能带行尾锚点 `{#id}`，可能为空。
///
/// 方括号、感叹号的全角写法（`【！案例】`、`［!专栏］`）一并认，输入法切不切
/// 半角都能写出来。名称不能为空，也不能含方括号。
pub fn box_head(inner: &str) -> Option<(&str, &str)> {
    let rest = inner.strip_prefix(['[', '【', '［'])?;
    let rest = rest.strip_prefix(['!', '！'])?;
    let close = rest.find([']', '】', '］'])?;
    let name = rest[..close].trim();
    if name.is_empty() || name.contains(['[', '【', '［']) {
        return None;
    }
    let title = &rest[close..];
    let title = &title[title.chars().next()?.len_utf8()..];
    Some((name, title.trim()))
}

/// 引文的出处行：以破折号 `——` 开头。
pub fn is_source(inner: &str) -> bool {
    inner.starts_with("——")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_marker_followed_by_space_or_line_end_opens_a_quote() {
        assert_eq!(strip_marker("> 引文"), Some("引文"));
        assert_eq!(strip_marker("  >\t引文  "), Some("引文"));
        assert_eq!(strip_marker(">"), Some(""));
        assert_eq!(strip_marker("> > 嵌套"), Some("嵌套"));
        assert_eq!(strip_marker(">> 嵌套"), Some("嵌套"));
        assert_eq!(strip_marker(">5%的企业"), None);
        assert_eq!(strip_marker("\\> 字面"), None);
        assert_eq!(strip_marker("＞ 全角"), None);
        assert_eq!(strip_marker("正文 > 引文"), None);
    }

    #[test]
    fn box_head_takes_any_name_and_full_width_brackets() {
        assert_eq!(box_head("[!专栏] 国外经验"), Some(("专栏", "国外经验")));
        assert_eq!(
            box_head("【！案例】某市做法 {#case:a}"),
            Some(("案例", "某市做法 {#case:a}"))
        );
        assert_eq!(box_head("［! 例子 ］"), Some(("例子", "")));
        assert_eq!(box_head("[!NOTE] 提示"), Some(("NOTE", "提示")));
        assert_eq!(box_head("[!] 没有名称"), None);
        assert_eq!(box_head("[!专栏 没有收尾"), None);
        assert_eq!(box_head("专栏：国外经验"), None);
    }

    #[test]
    fn a_dash_line_is_the_source() {
        assert!(is_source("——《意见》"));
        assert!(!is_source("—《意见》"));
        assert!(!is_source("正文——插入语"));
    }
}
