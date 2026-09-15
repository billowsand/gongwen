//! Markdown → IR 解析器。
//!
//! 输出 [`Vec<Block>`]，供 `tex_official` / `docx_research` / `docx_official` 等 emitter 消费。
//!
//! 处理流程：
//! 1. 全局做一遍 [`common::quotes::convert_quotes`] 把所有引号正规化为中文圆引号；
//! 2. 行扫描，按"标题 / 表格 / 列表 / 标记 / 段落 / 空行"分派；
//! 3. 标题文本经 [`common::heading::clean`] 去除旧编号；
//! 4. 段落与列表内容用 [`common::inline::parse`] 拆成 [`Inline`] 序列。

#![allow(dead_code)]

use regex::Regex;
use std::sync::OnceLock;

use crate::common::ast::{Block, Inline};
use crate::common::{heading, inline, markers, quotes, table};

/// 把整段 markdown 解析成 IR。
pub fn parse(content: &str) -> Vec<Block> {
    let lines: Vec<String> = normalize_lines_except_code(content);

    let mut blocks: Vec<Block> = Vec::new();
    let mut list_indents: Vec<(usize, u8)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let raw = &lines[i];
        let line = raw.trim();

        // 1) 区段标记 `<!-- [...] -->`
        if let Some(kind) = markers::detect(line) {
            list_indents.clear();
            blocks.push(Block::Marker(kind));
            i += 1;
            continue;
        }

        // 2) 标题 `#` ~ `######`（尾部 `{#id}` 剥离为交叉引用锚点）
        if line.starts_with('#') {
            list_indents.clear();
            let mut level: u8 = 0;
            let mut rest = line;
            while rest.starts_with('#') && level < 6 {
                level += 1;
                rest = &rest[1..];
            }
            let body = heading::clean(rest.trim());
            let (body, label) = strip_label_attr(&body);
            if let Some(id) = label {
                blocks.push(Block::Label(id));
            }
            blocks.push(Block::Heading { level, text: body });
            i += 1;
            continue;
        }

        // 3) 代码块 ```...```
        if line.starts_with("```") {
            list_indents.clear();
            let (parsed, new_i) = parse_code_block(&lines, i);
            if let Some((lang, content)) = parsed {
                blocks.push(Block::CodeBlock { lang, content });
                i = new_i;
                continue;
            }
            // 退化：当作普通段落
            blocks.push(Block::Paragraph(inline::parse(line)));
            i += 1;
            continue;
        }

        // 4) 表格 (must check for table first to avoid confusing | with text)
        if table::is_table_line(line) {
            list_indents.clear();
            let leading_caption = take_leading_table_caption(&mut blocks);
            let (parsed, new_i) = table::parse_table(&lines, i);
            if let Some(rows) = parsed {
                let (trailing_caption, final_i) = parse_trailing_table_caption(&lines, new_i);
                let (caption, label) = match leading_caption.or(trailing_caption) {
                    Some((c, l)) => (Some(c), l),
                    None => (None, None),
                };
                // 表题尾部 `{#id}` 锚点挂在随后的表格块上
                if let Some(id) = label {
                    blocks.push(Block::Label(id));
                }
                blocks.push(Block::Table { rows, caption });
                i = final_i;
                continue;
            }
            if let Some((caption, _)) = leading_caption {
                blocks.push(Block::Paragraph(inline::parse(&format!(
                    "Table: {}",
                    caption
                ))));
            }
            // 退化：当作普通段落
            blocks.push(Block::Paragraph(inline::parse(line)));
            i += 1;
            continue;
        }

        // 4) 列表项
        if let Some((ordered, indent, content)) = detect_list(raw) {
            let level = resolve_list_level(&mut list_indents, indent);
            blocks.push(Block::List {
                ordered,
                level,
                content: inline::parse(&content),
            });
            i += 1;
            continue;
        }

        // 5) 空行
        if line.is_empty() {
            blocks.push(Block::Empty);
            i += 1;
            continue;
        }

        // 6) 普通段落
        list_indents.clear();
        blocks.push(Block::Paragraph(inline::parse(line)));
        i += 1;
    }
    blocks
}

fn take_leading_table_caption(blocks: &mut Vec<Block>) -> Option<(String, Option<String>)> {
    let mut empty_tail = Vec::new();
    while matches!(blocks.last(), Some(Block::Empty)) {
        empty_tail.push(blocks.pop().expect("last block"));
    }

    let caption = match blocks.last() {
        Some(Block::Paragraph(inlines)) => {
            let text = inline::flatten(inlines);
            parse_table_caption_marker(&text)
        }
        _ => None,
    };

    if caption.is_some() {
        blocks.pop();
    } else {
        while let Some(block) = empty_tail.pop() {
            blocks.push(block);
        }
    }

    caption
}

fn parse_trailing_table_caption(
    lines: &[String],
    start: usize,
) -> (Option<(String, Option<String>)>, usize) {
    let mut i = start;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }

    if i < lines.len() {
        if let Some(caption) = parse_table_caption_marker(lines[i].trim()) {
            return (Some(caption), i + 1);
        }
    }

    (None, start)
}

fn parse_table_caption_marker(line: &str) -> Option<(String, Option<String>)> {
    let trimmed = line.trim();

    // 带编号的表格标记（"表1：标题" / "表 1.2 标题" / "表E.1：标题" / "Table 1: 标题"）。
    // 编号必须由 LaTeX 表格计数器自动生成，此处剥除以避免重复编号。
    // 编号段必须含数字，避免把 "Table of contents" 这类正文误判为表题。
    static NUMBERED: OnceLock<Regex> = OnceLock::new();
    let numbered = NUMBERED.get_or_init(|| {
        Regex::new(
            r"^(?i:表|table)\s*(?:[A-Za-z]?\d+|[A-Za-z][.\-]\d+)(?:[.\-][A-Za-z0-9]+)*\s*(?:[:：]\s*|\s+)(.+)$",
        )
        .expect("invalid numbered table caption pattern")
    });
    if let Some(caps) = numbered.captures(trimmed) {
        let caption = strip_caption_number(caps[1].trim());
        if !caption.is_empty() {
            return Some(strip_label_attr(&caption));
        }
    }

    let caption = trimmed
        .strip_prefix("Table:")
        .or_else(|| trimmed.strip_prefix("table:"))
        .or_else(|| trimmed.strip_prefix("TABLE:"))
        .or_else(|| trimmed.strip_prefix("表:"))
        .or_else(|| trimmed.strip_prefix("表："))
        .or_else(|| trimmed.strip_prefix(':'))?;
    let caption = strip_caption_number(caption.trim());
    if caption.is_empty() {
        None
    } else {
        Some(strip_label_attr(&caption))
    }
}

/// 剥除文本尾部的 `{#id}` 交叉引用锚点，返回 (剩余文本, label)。
fn strip_label_attr(text: &str) -> (String, Option<String>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\s*\{#([A-Za-z][\w:.-]*)\}\s*$").expect("invalid label attr regex")
    });
    match re.captures(text) {
        Some(caps) => {
            let m = caps.get(0).expect("label attr whole group");
            (
                text[..m.start()].trim_end().to_string(),
                Some(caps[1].to_string()),
            )
        }
        None => (text.to_string(), None),
    }
}

/// 剥除表题开头残留的旧编号（": 4.6 CH-06 标题" / "Table: 3 标题" / ": E.1 标题" /
/// ": 附录E 标题"），规则与 [`heading::clean`] 的数字编号部分一致；编号由 LaTeX 自动生成。
/// 字母编号仅限单字母+分隔符+数字（如 "E.1"），避免误伤 "CH-06" 这类产品代号。
fn strip_caption_number(caption: &str) -> String {
    static APPENDIX: OnceLock<Regex> = OnceLock::new();
    let appendix = APPENDIX.get_or_init(|| {
        Regex::new(r"^附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*")
            .expect("invalid appendix caption pattern")
    });
    let caption = appendix.replace(caption, "");

    static LEADING_NUM: OnceLock<Regex> = OnceLock::new();
    let leading_num = LEADING_NUM.get_or_init(|| {
        Regex::new(r"^(?:\d+(?:\.\d+)*|[A-Za-z][.\-]\d+(?:[.\-]\d+)*)[.．、]?\s+([^\d\s])")
            .expect("invalid caption number pattern")
    });
    // 标题首字符用捕获组保留，避免随编号一起被替换掉
    leading_num.replace(&caption, "$1").trim().to_string()
}

fn normalize_lines_except_code(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_code = false;

    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            lines.push(line.to_string());
            in_code = !in_code;
            continue;
        }

        if in_code {
            lines.push(line.to_string());
        } else {
            lines.push(quotes::convert_quotes(line));
        }
    }

    lines
}

/// 列表行识别：返回 `(ordered, 前导空白列数, 去前缀的文本)`。
fn detect_list(raw: &str) -> Option<(bool, usize, String)> {
    let trimmed_start = raw.trim_start();
    if trimmed_start.is_empty() {
        return None;
    }

    let indent = raw.len() - trimmed_start.len();
    if let Some(rest) = trimmed_start.strip_prefix("- ") {
        return Some((false, indent, rest.trim().to_string()));
    }
    if let Some(rest) = trimmed_start.strip_prefix("* ") {
        return Some((false, indent, rest.trim().to_string()));
    }
    let re = numbered_list_regex();
    if re.is_match(trimmed_start) {
        let stripped = re.replace(trimmed_start, "").to_string();
        return Some((true, indent, stripped));
    }
    None
}

/// 把连续列表中的实际缩进变化映射为最多六级层级。
///
/// 相邻列表项只要缩进增加，就进入下一层，因此 2 空格和 4 空格两种常见写法
/// 都能稳定表达多层列表。列表从缩进位置开始且缺少父项时，保留旧规则作为回退，
/// 兼容既有文档中独立书写的二级、三级列表片段。
fn resolve_list_level(indents: &mut Vec<(usize, u8)>, indent: usize) -> u8 {
    if let Some(position) = indents.iter().position(|(known, _)| *known == indent) {
        let level = indents[position].1;
        indents.truncate(position + 1);
        return level;
    }

    if let Some(&(current_indent, current_level)) = indents.last() {
        if indent > current_indent {
            let level = current_level.saturating_add(1).min(6);
            indents.push((indent, level));
            return level;
        }

        while indents.last().is_some_and(|(known, _)| *known > indent) {
            indents.pop();
        }
    }

    let level = indent_to_level(indent);
    indents.push((indent, level));
    level
}

fn indent_to_level(indent: usize) -> u8 {
    match indent {
        0 => 1,
        1..=4 => 2,
        5..=8 => 3,
        9..=12 => 4,
        13..=16 => 5,
        _ => 6,
    }
}

/// 解析以三个反引号包围、可带语言名称的代码块。
/// 返回 (lang, content) 和结束行之后的索引。
fn parse_code_block(lines: &[String], start: usize) -> (Option<(Option<String>, String)>, usize) {
    let first = &lines[start];
    let first_trimmed = first
        .trim_start()
        .trim_start_matches('`')
        .split_whitespace()
        .next()
        .unwrap_or("");
    let lang = if first_trimmed.is_empty() {
        None
    } else {
        Some(first_trimmed.to_string())
    };

    let mut content = String::new();
    let mut i = start + 1;
    while i < lines.len() {
        let line = &lines[i];
        if line.trim() == "```" {
            // 找到结束标记
            return (Some((lang, content.trim_matches('\n').to_string())), i + 1);
        }
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(line);
        i += 1;
    }
    // 没有找到结束标记，把剩下的都当作内容
    (Some((lang, content.trim_matches('\n').to_string())), i)
}

fn numbered_list_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d+\.\s*").expect("invalid numbered list regex"))
}

/// 把行内 Inline 序列原样拼回字符串（debug 用）。
#[allow(dead_code)]
pub fn render_text(inlines: &[Inline]) -> String {
    inline::flatten(inlines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ast::MarkerKind;

    fn paragraph_text(b: &Block) -> String {
        match b {
            Block::Paragraph(inlines) => inline::flatten(inlines),
            _ => panic!("not a paragraph: {:?}", b),
        }
    }

    #[test]
    fn parses_headings_and_strips_numbering() {
        let blocks = parse("# 一、引言\n## 1.1 背景\n### （一）问题\n");
        let titles: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(titles, vec!["引言", "背景", "问题"]);
    }

    #[test]
    fn parses_paragraphs_with_bold_italic() {
        let blocks = parse("一段**加粗**和*斜体*文字。\n");
        let para = blocks
            .iter()
            .find(|b| matches!(b, Block::Paragraph(_)))
            .expect("paragraph");
        let inlines = match para {
            Block::Paragraph(v) => v,
            _ => unreachable!(),
        };
        assert!(matches!(&inlines[0], Inline::Text(t) if t == "一段"));
        assert!(
            matches!(&inlines[1], Inline::Bold(children) if matches!(children.as_slice(), [Inline::Text(t)] if t == "加粗"))
        );
        assert!(matches!(&inlines[2], Inline::Text(t) if t == "和"));
        assert!(
            matches!(&inlines[3], Inline::Italic(children) if matches!(children.as_slice(), [Inline::Text(t)] if t == "斜体"))
        );
    }

    #[test]
    fn detects_markers() {
        let blocks = parse(
            "<!-- [摘要] -->\n<!-- [附录] -->\n<!-- [版本变更记录] -->\n<!-- [正文] -->\n<!-- [参考文献] -->\n",
        );
        let kinds: Vec<MarkerKind> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Marker(k) => Some(*k),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                MarkerKind::Abstract,
                MarkerKind::Appendix,
                MarkerKind::Changelog,
                MarkerKind::Body,
                MarkerKind::Reference,
            ]
        );
    }

    #[test]
    fn parses_table_block() {
        let md = "| 列A | 列B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        let blocks = parse(md);
        let table = blocks
            .iter()
            .find(|b| matches!(b, Block::Table { .. }))
            .expect("table");
        if let Block::Table { rows, caption } = table {
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0], vec!["列A".to_string(), "列B".to_string()]);
            assert_eq!(rows[2], vec!["3".to_string(), "4".to_string()]);
            assert_eq!(caption, &None);
        }
    }

    #[test]
    fn parses_leading_table_caption() {
        let md = "Table: 测试表格\n\n| 列A | 列B |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(md);
        let tables: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Table { caption, .. } => Some(caption.as_deref()),
                _ => None,
            })
            .collect();
        assert_eq!(tables, vec![Some("测试表格")]);
        assert!(!blocks.iter().any(|b| match b {
            Block::Paragraph(inlines) => inline::flatten(inlines).contains("Table:"),
            _ => false,
        }));
    }

    #[test]
    fn parses_trailing_table_caption() {
        let md = "| 列A | 列B |\n|---|---|\n| 1 | 2 |\n\n: 测试表格\n";
        let blocks = parse(md);
        let caption = blocks
            .iter()
            .find_map(|b| match b {
                Block::Table { caption, .. } => caption.as_deref(),
                _ => None,
            })
            .expect("caption");
        assert_eq!(caption, "测试表格");
    }

    #[test]
    fn parses_chinese_table_caption_marker() {
        let md = "表：中文表题\n\n| 列A | 列B |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(md);
        let caption = blocks
            .iter()
            .find_map(|b| match b {
                Block::Table { caption, .. } => caption.as_deref(),
                _ => None,
            })
            .expect("caption");
        assert_eq!(caption, "中文表题");
    }

    #[test]
    fn strips_number_from_table_caption_markers() {
        // 编号交给 LaTeX 表格计数器自动生成，标记里的旧编号必须剥除
        let cases = [
            ("表1：中文表题", "中文表题"),
            ("表 1: 中文表题", "中文表题"),
            ("表1.2 中文表题", "中文表题"),
            ("表 1-1 中文表题", "中文表题"),
            ("表E.1：中文表题", "中文表题"),
            ("表 E-1 中文表题", "中文表题"),
            ("Table 1: 测试表格", "测试表格"),
            ("table 2.3：测试表格", "测试表格"),
        ];
        for (marker, expected) in cases {
            let md = format!("{}\n\n| 列A | 列B |\n|---|---|\n| 1 | 2 |\n", marker);
            let blocks = parse(&md);
            let caption = blocks
                .iter()
                .find_map(|b| match b {
                    Block::Table { caption, .. } => caption.as_deref(),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("caption for {:?}", marker));
            assert_eq!(caption, expected, "marker {:?}", marker);
            // 带编号的标记行不得残留为普通段落，否则与自动编号重复
            assert!(
                !blocks.iter().any(|b| match b {
                    Block::Paragraph(inlines) => inline::flatten(inlines).contains(marker),
                    _ => false,
                }),
                "marker {:?} left as paragraph",
                marker
            );
        }
    }

    #[test]
    fn strips_number_from_caption_text() {
        // 表题文本开头残留的旧编号（"4.6" / "3"）也必须剥除
        let cases = [
            (
                ": 4.6 CH-06 产品清单、任务筹划与任务分配",
                "CH-06 产品清单、任务筹划与任务分配",
            ),
            (": E.1 附录表题", "附录表题"),
            (": 附录E 集成任务清单", "集成任务清单"),
            ("Table: 3 测试表格", "测试表格"),
            ("表：1.2 中文表题", "中文表题"),
        ];
        for (marker, expected) in cases {
            let md = format!("| 列A | 列B |\n|---|---|\n| 1 | 2 |\n\n{}\n", marker);
            let blocks = parse(&md);
            let caption = blocks
                .iter()
                .find_map(|b| match b {
                    Block::Table { caption, .. } => caption.as_deref(),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("caption for {:?}", marker));
            assert_eq!(caption, expected, "marker {:?}", marker);
        }
    }

    #[test]
    fn plain_table_words_not_treated_as_caption() {
        // "Table of contents" 这类正文不含数字编号，不得被误判为表题
        let md = "Table of contents\n\n| 列A | 列B |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(md);
        assert!(blocks.iter().any(|b| match b {
            Block::Paragraph(inlines) => inline::flatten(inlines) == "Table of contents",
            _ => false,
        }));
        let caption = blocks.iter().find_map(|b| match b {
            Block::Table { caption, .. } => caption.as_deref(),
            _ => None,
        });
        assert_eq!(caption, None);
    }

    #[test]
    fn lists_track_indent_levels() {
        let md = "- 一级\n  - 二级\n      - 三级\n";
        let blocks = parse(md);
        let levels: Vec<u8> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::List { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 3]);
    }

    #[test]
    fn lists_accept_two_space_multilevel_indentation() {
        let md = "- 一级\n  - 二级\n    - 三级\n      - 四级\n        - 五级\n          - 六级\n";
        let blocks = parse(md);
        let levels: Vec<u8> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::List { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn independent_indented_list_keeps_legacy_level() {
        let md = "说明文字\n      - 独立三级列表\n";
        let blocks = parse(md);
        assert!(blocks
            .iter()
            .any(|block| matches!(block, Block::List { level: 3, .. })));
    }

    #[test]
    fn quotes_normalized_globally() {
        let blocks = parse("说\"你好\"。\n");
        let p = paragraph_text(&blocks[0]);
        assert!(
            p.contains('\u{201c}') && p.contains('\u{201d}'),
            "got {:?}",
            p
        );
    }

    #[test]
    fn ordered_list_distinguished() {
        let blocks = parse("1. 第一\n- 第二\n");
        let mut iter = blocks.iter().filter_map(|b| match b {
            Block::List { ordered, .. } => Some(*ordered),
            _ => None,
        });
        assert_eq!(iter.next(), Some(true));
        assert_eq!(iter.next(), Some(false));
    }

    #[test]
    fn code_block_preserves_ascii_quotes() {
        let blocks = parse("```rust\nlet s = \"hello\";\n```\n");
        let code = blocks
            .iter()
            .find_map(|b| match b {
                Block::CodeBlock { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .expect("code block");
        assert_eq!(code, "let s = \"hello\";");
    }

    #[test]
    fn fenced_code_does_not_create_citations() {
        let blocks = parse("```text\n[@key]\n```\n");
        assert!(matches!(
            &blocks[0],
            Block::CodeBlock { content, .. } if content == "[@key]"
        ));
    }

    #[test]
    fn code_block_uses_first_info_word_as_language() {
        let blocks = parse("```rust {#id}\nfn main() {}\n```\n");
        let lang = blocks
            .iter()
            .find_map(|b| match b {
                Block::CodeBlock { lang, .. } => lang.as_deref(),
                _ => None,
            })
            .expect("language");
        assert_eq!(lang, "rust");
    }

    #[test]
    fn heading_label_attr_becomes_label_block() {
        let blocks = parse("## 第一章 概述 {#chap:overview}\n");
        assert!(matches!(&blocks[0], Block::Label(id) if id == "chap:overview"));
        assert!(matches!(&blocks[1], Block::Heading { level: 2, text } if text == "概述"));
    }

    #[test]
    fn heading_without_label_attr_has_no_label_block() {
        let blocks = parse("## 概述\n");
        assert!(!blocks.iter().any(|b| matches!(b, Block::Label(_))));
        assert!(matches!(&blocks[0], Block::Heading { text, .. } if text == "概述"));
    }

    #[test]
    fn table_caption_label_attr_becomes_label_block() {
        let md = "表：产品清单 {#tbl:products}\n\n| 列A | 列B |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(md);
        let label_pos = blocks
            .iter()
            .position(|b| matches!(b, Block::Label(id) if id == "tbl:products"))
            .expect("label block");
        let table_pos = blocks
            .iter()
            .position(|b| matches!(b, Block::Table { .. }))
            .expect("table");
        assert!(label_pos < table_pos);
        let caption = blocks.iter().find_map(|b| match b {
            Block::Table { caption, .. } => caption.as_deref(),
            _ => None,
        });
        assert_eq!(caption, Some("产品清单"));
    }
}
