//! 「清洗」分区的纯文本处理：把不合公文助手 Markdown 标准的写法改成合规写法。
//!
//! 清洗只做正向转换：目标一律是「更符合公文助手 Markdown 标准的正文」。所以这里
//! 没有「Markdown 退回公文编号」「剥掉 Markdown 标记」「清除全部编号」这类反向
//! 操作——那些方向把已经成形的结构拆散，与清洗的目的正相反。
//!
//! 判定口径跟 `skills/gongwen-markdown` 的《公文助手 Markdown 语法完整参考》和自检
//! 脚本 `scripts/check_gongwen_md.py` 对齐：标题不手写编号、层级不跳级、正文区只留
//! 一个一级标题、列表只用 `1. ` / `- ` / `* `、表格必须有分隔行。
//!
//! 这里只做确定性的文本替换，不碰 egui：输入输出都是一个 Markdown 字符串，按钮层
//! 只负责取稿子、调函数、写回并重新校验。逻辑能单独测。

use crate::export::{MarkdownSection, ResearchSection};
use std::collections::HashSet;

/// 正文标题最多写到五级（`#####`）：六级在公文版式里整条不排，内容会消失。
const MAX_HEADING_LEVEL: usize = 5;

// ---------------------------------------------------------------- 入口

/// 结构总纲：把全文改成合规的公文助手 Markdown 标注。
///
/// 顺序固定：
/// 1. [`restructure`] 把段首手打的公文编号（「一、」「（一）」「1.」「（1）」「①」）
///    还原成 Markdown 标题与列表；
/// 2. [`normalize_headings`] 修标题：剥掉手写编号、层级不跳级、正文区不留第二个
///    一级标题；
/// 3. [`normalize_lists`] 把程序不认的列表写法改成 `1. ` / `- `；
/// 4. [`normalize_tables`] 给表格补分隔行并对齐列数。
pub(crate) fn normalize_structure(markdown: &str, research: bool) -> String {
    let text = restructure(markdown, research);
    let text = normalize_headings(&text, research);
    let text = normalize_lists(&text);
    normalize_tables(&text)
}

/// 段首手打公文编号 → Markdown 标题与列表。
///
/// 复用导入端同一套结构还原规则（[`crate::doc_import::structure::restructure`]）：
/// 它按整篇文字定编号方案，再按上下文判断某个编号是标题还是干条。还原会顺带把
/// 段落之间的空行摆正，原稿里标题与正文本来就紧贴着的那一行（紧缩排版）要按原样
/// 贴回去。
///
/// 研究报告有自己的 Markdown 扩展（锚点、交叉引用、文框、公式），结构还原那套规则
/// 是给公文写的，对研报只修不合规的写法，不动层级。
fn restructure(markdown: &str, research: bool) -> String {
    if research {
        return markdown.to_string();
    }
    let tight = tight_pairs(markdown);
    let rewritten = crate::doc_import::structure::restructure(
        markdown,
        &crate::doc_import::structure::Options::restructure_only(),
    );
    let rewritten = restore_tight_pairs(&tight, &rewritten);
    crate::doc_import::clean::tidy(&rewritten)
}

/// 原稿里标题与正文紧贴着的那几对（紧缩排版：标题后不空行）。
fn tight_pairs(markdown: &str) -> HashSet<(String, String)> {
    let mut pairs = HashSet::new();
    let lines: Vec<&str> = markdown.split('\n').collect();
    for window in lines.windows(2) {
        let (head, body) = (window[0].trim(), window[1].trim());
        if head.is_empty() || body.is_empty() {
            continue;
        }
        if heading_at(head).is_some() {
            pairs.insert((head.to_string(), body.to_string()));
        }
    }
    pairs
}

/// 把结构还原时插到标题与正文之间的空行去掉：原稿里这两行本来就紧贴着。
fn restore_tight_pairs(pairs: &HashSet<(String, String)>, rewritten: &str) -> String {
    if pairs.is_empty() {
        return rewritten.to_string();
    }
    let lines: Vec<&str> = rewritten.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let head = lines[index].trim().to_string();
        let joined = index + 2 < lines.len()
            && lines[index + 1].trim().is_empty()
            && !lines[index + 2].trim().is_empty()
            && pairs.contains(&(head, lines[index + 2].trim().to_string()));
        out.push(lines[index].to_string());
        if joined {
            index += 1;
        }
        index += 1;
    }
    out.join("\n")
}

// ---------------------------------------------------------------- 标题

/// 修标题：剥掉行首手写的编号、层级不跳级、正文区不留第二个一级标题、末尾不加句号。
///
/// 一级标题是文档标题、附件标题与研究报告的部分标题，正文区只该有一个；多出来的
/// 降成二级标题，内容不丢。二级到五级标题是正文层级：程序排版时本来就会剥掉行首
/// 手写的编号，源码里留着只会多一个号，这里一并剥掉。
pub(crate) fn normalize_headings(markdown: &str, research: bool) -> String {
    let mut out: Vec<String> = Vec::new();
    // 是否在等一个正式标题（文档标题、附件标题）。
    let mut expecting_title = true;
    // 这一区段是否允许多个一级标题：研究报告的部分区与附录区可以，公文不行。
    let mut many_titles = false;
    // 上一个正文标题的层级，用来判跳级。文档标题不算一级，附件与部分另起一层。
    let mut last_level: Option<usize> = None;
    let mut fence: Option<&'static str> = None;
    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            out.push(line.to_string());
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            out.push(line.to_string());
            continue;
        }
        if let Some(section) = section_of(line, research) {
            match section {
                Section::Body => {}
                Section::Attachment => {
                    // 公文的附件区只容一个一级标题（附件正式标题）；研究报告的附录区
                    // 可以有几个章标题。
                    many_titles = research;
                    expecting_title = true;
                    last_level = None;
                }
                Section::Part => {
                    many_titles = true;
                    expecting_title = true;
                    last_level = None;
                }
            }
            out.push(line.to_string());
            continue;
        }
        let Some((level, text)) = heading_at(line) else {
            out.push(line.to_string());
            continue;
        };
        let (level, text) = if level == 1 {
            if many_titles {
                // 研究报告的部分标题、附录章标题：原样保留。
                (1, text.to_string())
            } else if expecting_title {
                // 文档标题与附件标题：原样保留，不剥编号。
                expecting_title = false;
                (1, text.to_string())
            } else {
                // 正文区里多出来的一级标题：降成二级标题。
                (2, clean_heading_text(text))
            }
        } else {
            let level = level.min(MAX_HEADING_LEVEL);
            // 跳级：`##` 下面直接写 `####` 会被排成「一、」后直接「1.」，降一级。
            let level = match last_level {
                Some(previous) if level > previous + 1 => previous + 1,
                _ => level,
            };
            (level, clean_heading_text(text))
        };
        if level > 1 {
            last_level = Some(level);
        }
        out.push(format!("{} {text}", "#".repeat(level)));
    }
    out.join("\n")
}

/// 标题文字去掉两处不合规：行首手写的编号与末尾的句号。
///
/// 剥编号用导出器同一套规则（[`crate::export::clean_heading_number`]），保证清洗后
/// 的源码与排版结果一致。剥完一个字都不剩时保留原文：那样的标题（整段就是一个
/// 编号，如「第一节」）剥了就成了空标题，源码比原来还难看。
fn clean_heading_text(text: &str) -> String {
    let cleaned = crate::export::clean_heading_number(text);
    if cleaned.is_empty() {
        return text.trim().to_string();
    }
    cleaned
        .strip_suffix('。')
        .unwrap_or(cleaned.as_str())
        .trim_end()
        .to_string()
}

/// 区段标记的分类：决定这一区段里的一级标题算不算「正式标题」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// 正文区。
    Body,
    /// 附件 / 附录区。
    Attachment,
    /// 研究报告的部分区。
    Part,
}

/// 这一行是区段标记吗？是的话属于哪一段。
fn section_of(line: &str, research: bool) -> Option<Section> {
    if let Some(section) = crate::export::parse_section_marker(line) {
        return Some(match section {
            MarkdownSection::Body => Section::Body,
            MarkdownSection::Attachment => Section::Attachment,
        });
    }
    if research && let Some(section) = crate::export::parse_research_marker(line) {
        return Some(match section {
            ResearchSection::Part => Section::Part,
            ResearchSection::Appendix => Section::Attachment,
            _ => Section::Body,
        });
    }
    None
}

// ---------------------------------------------------------------- 列表

/// 把程序不认的列表写法改成它认的三种：`1. 内容`、`- 内容`、`* 内容`。
///
/// 程序只认这三种行首写法（《公文助手 Markdown 语法完整参考》第 5.1 节），
/// `+ 内容`、`-内容`、`*内容`、`1.内容`、`1)内容` 都会当普通段落印出来。这里改成
/// 标准写法，缩进与文字都不动。
///
/// 公文编号（「一、」「（一）」「1、」「（1）」「①」）不动：那是标题层的写法，由
/// [`normalize_structure`] 按上下文还原成标题或列表。
pub(crate) fn normalize_lists(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<&'static str> = None;
    for line in markdown.split('\n') {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            out.push(line.to_string());
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            out.push(line.to_string());
            continue;
        }
        out.push(normalize_list_line(line));
    }
    out.join("\n")
}

/// 改一行列表的写法；不是列表样子的行原样返回。
fn normalize_list_line(line: &str) -> String {
    let trimmed = line.trim_start();
    // 表格行与标记行不动。
    if trimmed.is_empty() || trimmed.starts_with('|') || trimmed.starts_with("<!--") {
        return line.to_string();
    }
    // 已经是标准写法。
    if is_standard_list_item(trimmed) {
        return line.to_string();
    }
    let Some((marker, rest)) = rewrite_list_marker(trimmed) else {
        return line.to_string();
    };
    let indent = &line[..line.len() - trimmed.len()];
    format!("{indent}{marker}{rest}")
}

/// 已经是程序认的列表写法吗（`1. 内容`、`- 内容`、`* 内容`）。
fn is_standard_list_item(trimmed: &str) -> bool {
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return true;
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    (1..=9).contains(&digits) && trimmed[digits..].starts_with(". ")
}

/// 把差一点的列表写法改成标准写法，返回新的标记与剩下的文字。
fn rewrite_list_marker(trimmed: &str) -> Option<(String, &str)> {
    // `+ 内容` / `+内容` → `- 内容`
    if let Some(rest) = trimmed.strip_prefix('+') {
        return Some(("- ".to_string(), rest.trim_start()));
    }
    // `-内容` → `- 内容`；`---` 是分隔线，不动。
    if let Some(rest) = trimmed.strip_prefix('-')
        && !rest.starts_with('-')
    {
        return Some(("- ".to_string(), rest.trim_start()));
    }
    // `*内容` → `* 内容`；`**加粗**` 不是列表，不动。
    if let Some(rest) = trimmed.strip_prefix('*')
        && !rest.starts_with('*')
    {
        return Some(("* ".to_string(), rest.trim_start()));
    }
    // `1.内容` → `1. 内容`；`1.5万元` 是小数，不是列表。
    if let Some((number, rest)) = split_decimal_marker(trimmed) {
        return Some((format!("{number}. "), rest));
    }
    // `1)内容` → `1. 内容`
    if let Some((number, rest)) = split_paren_marker(trimmed) {
        return Some((format!("{number}. "), rest));
    }
    None
}

/// `1.内容` → `("1", "内容")`。`1.5万元` 这种小数不认。
fn split_decimal_marker(trimmed: &str) -> Option<(&str, &str)> {
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let rest = &trimmed[digits..];
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix('．'))?;
    let rest = rest.trim_start();
    if rest.is_empty() || rest.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    Some((&trimmed[..digits], rest))
}

/// `1)内容` → `("1", "内容")`。
fn split_paren_marker(trimmed: &str) -> Option<(&str, &str)> {
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let rest = &trimmed[digits..];
    let rest = rest.strip_prefix(')').or_else(|| rest.strip_prefix('）'))?;
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    Some((&trimmed[..digits], rest))
}

// ---------------------------------------------------------------- 表格

/// 表格按分隔行补齐列数、规范单元格；缺分隔行的补上分隔行。
pub(crate) fn normalize_tables(markdown: &str) -> String {
    let text = insert_table_separators(markdown);
    crate::doc_import::clean::normalize_tables(&text)
}

/// 给缺分隔行的表格补上分隔行。
///
/// 程序认表的条件是「表头行 + 分隔行 + 数据行」，分隔行每格至少三个短横、格数与
/// 表头一致；缺了分隔行，这一组带 `|` 的行就整组当正文印出。这里按组里最宽的
/// 一行定列数补出分隔行，列数对齐交给 [`crate::doc_import::clean::normalize_tables`]。
fn insert_table_separators(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<&'static str> = None;
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim_start();
        if let Some(marker) = fence {
            out.push(lines[index].to_string());
            if trimmed.starts_with(marker) {
                fence = None;
            }
            index += 1;
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            out.push(lines[index].to_string());
            index += 1;
            continue;
        }
        if !crate::export::is_table_row(lines[index]) {
            out.push(lines[index].to_string());
            index += 1;
            continue;
        }
        // 找出这一组连续的表格行。
        let start = index;
        while index < lines.len() && crate::export::is_table_row(lines[index]) {
            index += 1;
        }
        let group = &lines[start..index];
        // 组里已经有合法分隔行：列数对齐那一步会处理。
        if group
            .iter()
            .any(|line| crate::export::is_table_separator(line))
        {
            out.extend(group.iter().map(|line| line.to_string()));
            continue;
        }
        // 一组只有一行：正文里写字面的 `|` 也可能是这个形状，不动。
        if group.len() < 2 {
            out.push(group[0].to_string());
            continue;
        }
        // 表头要写得住竖线、格数不少于两格，才当成一张缺分隔行的表。
        if !looks_like_table_header(group[0]) {
            out.extend(group.iter().map(|line| line.to_string()));
            continue;
        }
        let columns = group
            .iter()
            .map(|line| crate::export::parse_table_row(line).len())
            .max()
            .unwrap_or(2)
            .max(2);
        // 第二行像分隔行但不合格（短横不够三根）：换成合格的分隔行。
        if looks_like_separator(group[1]) {
            out.push(group[0].to_string());
            out.push(separator_row(columns));
            out.extend(group[2..].iter().map(|line| line.to_string()));
            continue;
        }
        // 没有分隔行：在表头后面补一行。
        out.push(group[0].to_string());
        out.push(separator_row(columns));
        out.extend(group[1..].iter().map(|line| line.to_string()));
    }
    out.join("\n")
}

/// 这一行像一张表的表头吗：首尾都有竖线、格数不少于两格。
fn looks_like_table_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|')
        && trimmed.ends_with('|')
        && crate::export::parse_table_row(trimmed).len() >= 2
}

/// 像分隔行但不是合格分隔行的行：只有 `|`、`-`、`:` 和空白。
fn looks_like_separator(line: &str) -> bool {
    let value = line.trim();
    value.contains('-')
        && value.contains('|')
        && value
            .chars()
            .all(|ch| matches!(ch, '|' | '-' | ':' | ' ' | '\t'))
}

/// 造一行分隔行：每格三个短横。
fn separator_row(columns: usize) -> String {
    let mut row = String::from("|");
    for _ in 0..columns {
        row.push_str(" --- |");
    }
    row
}

// ---------------------------------------------------------------- 杂项

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

/// 代码围栏的记号（``` 或 ~~~）。
fn fence_marker(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 结构总纲把段首公文编号还原成标题与列表() {
        let markdown = "# 标题\n\n一、总体要求\n\n（一）提高认识\n\n1. 具体措施\n";
        let converted = normalize_structure(markdown, false);
        assert_eq!(
            converted,
            "# 标题\n\n## 总体要求\n\n### 提高认识\n\n1. 具体措施\n"
        );
    }

    #[test]
    fn 结构总纲对合规稿子不改内容() {
        let markdown = "# 关于××工作的通知\n\n<!-- [正文] -->\n\n开头一段。\n\n## 总体要求\n\n### 提高认识\n\n- 第一项\n- 第二项\n\n| 序号 | 单位 |\n| --- | --- |\n| 1 | 甲 |\n\n<!-- [附件] -->\n\n# 情况统计表\n\n附件内容。\n";
        assert_eq!(normalize_structure(markdown, false), markdown);
    }

    #[test]
    fn 结构总纲不把两个季度并称的枚举当编号() {
        let markdown = "# 标题\n\n一、二季度工作安排\n\n二、二月份任务\n";
        assert_eq!(normalize_structure(markdown, false), markdown);
    }

    #[test]
    fn 结构总纲保留标题与正文紧贴着的紧缩排版() {
        let markdown = "# 标题\n\n## 总体要求\n紧贴正文。\n";
        assert_eq!(
            normalize_structure(markdown, false),
            "# 标题\n\n## 总体要求\n紧贴正文。\n"
        );
    }

    #[test]
    fn 标题剥掉手写编号与末尾句号() {
        let markdown = "## 一、总体要求。\n\n### （一）提高认识\n";
        assert_eq!(
            normalize_headings(markdown, false),
            "## 总体要求\n\n### 提高认识\n"
        );
    }

    #[test]
    fn 标题层级不跳级时降一级() {
        let markdown = "## 总体要求\n\n#### 具体措施\n";
        assert_eq!(
            normalize_headings(markdown, false),
            "## 总体要求\n\n### 具体措施\n"
        );
    }

    #[test]
    fn 六级标题降成五级免得内容不排() {
        let markdown = "#### 具体措施\n\n###### 细节\n";
        assert_eq!(
            normalize_headings(markdown, false),
            "#### 具体措施\n\n##### 细节\n"
        );
    }

    #[test]
    fn 正文区第二个一级标题降成二级() {
        let markdown = "# 标题\n\n## 第一节\n\n# 又一个标题\n";
        assert_eq!(
            normalize_headings(markdown, false),
            "# 标题\n\n## 第一节\n\n## 又一个标题\n"
        );
    }

    #[test]
    fn 附件标题与附件里的二级标题都留着() {
        let markdown = "# 标题\n\n<!-- [附件] -->\n\n# 情况统计表\n\n## 品种\n";
        assert_eq!(normalize_headings(markdown, false), markdown);
    }

    #[test]
    fn 研究报告的部分标题可以多个() {
        let markdown = "# 报告\n\n<!-- [部分] -->\n\n# 现状分析\n\n<!-- [部分] -->\n\n# 对策建议\n";
        assert_eq!(normalize_headings(markdown, true), markdown);
    }

    #[test]
    fn 列表标记改成程序认的三种写法() {
        let markdown = "-内容\n*内容\n1.内容\n1)内容\n+ 第四项\n";
        assert_eq!(
            normalize_lists(markdown),
            "- 内容\n* 内容\n1. 内容\n1. 内容\n- 第四项\n"
        );
    }

    #[test]
    fn 列表改写不动加粗与小数() {
        assert_eq!(normalize_lists("**重点**\n"), "**重点**\n");
        assert_eq!(normalize_lists("1.5万元\n"), "1.5万元\n");
        assert_eq!(
            normalize_lists("正文里有个 | 符号\n"),
            "正文里有个 | 符号\n"
        );
    }

    #[test]
    fn 列表已经合规时原样返回() {
        let markdown = "- 第一项\n* 第二项\n1. 第三项\n";
        assert_eq!(normalize_lists(markdown), markdown);
    }

    #[test]
    fn 表格缺分隔行时补上并对齐列数() {
        let markdown = "| 序号 | 单位名称 |\n| 1 | 甲单位 |\n| 2 | 乙单位 |\n";
        assert_eq!(
            normalize_tables(markdown),
            "| 序号 | 单位名称 |\n| --- | --- |\n| 1 | 甲单位 |\n| 2 | 乙单位 |\n"
        );
    }

    #[test]
    fn 表格分隔行短横不够时换成合格的() {
        let markdown = "| 序号 | 单位 |\n|--|--|\n| 1 | 甲 |\n";
        assert_eq!(
            normalize_tables(markdown),
            "| 序号 | 单位 |\n| --- | --- |\n| 1 | 甲 |\n"
        );
    }

    #[test]
    fn 一行带竖线的正文不当成表格() {
        let markdown = "正文里有个 | 符号\n另一段\n";
        assert_eq!(normalize_tables(markdown), markdown);
    }

    #[test]
    fn 末尾换行与空行保持原样() {
        let markdown = "## 标题\n\n正文\n";
        for converted in [
            normalize_structure(markdown, false),
            normalize_headings(markdown, false),
            normalize_lists(markdown),
            normalize_tables(markdown),
        ] {
            assert!(converted.ends_with('\n'), "{converted:?}");
            assert_eq!(converted.split('\n').count(), markdown.split('\n').count());
        }
    }
}
