//! 把粘贴出来的列表整理成程序认的样子。
//!
//! `anydoc` 渲染嵌套列表时项与项之间留空行（loose list），而公文助手的解析器靠
//! 「上一行是不是列表项」判断分组：中间一空行，整组就散了，子项还会跑成同级。
//! 技能包 `markdown-syntax.md` 第 5 节把要求写死了：
//!
//! - 列表前空一行（独立列表，逐项成段编「1.」）；
//! - 项与项之间不空行；
//! - 只分两级：第一级顶格 `- `，子项缩进两格 `  - `，第三级及更深的并进第二级；
//! - 白头件、红头呈批件不收列表（`validator` 会报「正文不宜使用项目符号」），
//!   列表项改成各自独立的普通段落，**不**自动补写「一是……二是……」。
//!
//! 只动 `- ` 开头的行：`1. ` 是 `structure` 自己排出来的列表（已经紧凑），也可能是
//! 它认别的编号用的写法，不能碰。

use crate::models::TemplateKind;

/// 整理一段 Markdown 里的列表。
pub(crate) fn shape(markdown: &str, kind: TemplateKind) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut index = 0;
    let mut fenced = false;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push(line.to_string());
            index += 1;
            continue;
        }
        if fenced {
            out.push(line.to_string());
            index += 1;
            continue;
        }
        if bullet_item(line).is_none() {
            out.push(line.to_string());
            index += 1;
            continue;
        }
        // 收一组列表项：中间的空行，只要后面还是列表项就并进这一组。
        let mut items: Vec<(usize, String)> = Vec::new();
        while index < lines.len() {
            if let Some(item) = bullet_item(lines[index]) {
                items.push(item);
                index += 1;
                continue;
            }
            if lines[index].trim().is_empty() && next_is_item(&lines, index + 1) {
                index += 1;
                continue;
            }
            break;
        }
        push_list(&mut out, &items, kind);
    }
    let mut shaped = out.join("\n");
    if markdown.ends_with('\n') {
        shaped.push('\n');
    }
    shaped
}

/// `- 内容` 的缩进列数与内容。`* ` 也认（anydoc 只用 `- `，手写的稿子里两种都有）。
fn bullet_item(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let content = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;
    Some((indent_columns(line), content.trim().to_string()))
}

fn indent_columns(line: &str) -> usize {
    line.chars()
        .take_while(|ch| matches!(ch, ' ' | '\t'))
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}

/// 从 `start` 往下第一个非空行还是不是列表项。
fn next_is_item(lines: &[&str], start: usize) -> bool {
    for line in &lines[start.min(lines.len())..] {
        if line.trim().is_empty() {
            continue;
        }
        return bullet_item(line).is_some();
    }
    false
}

fn push_list(out: &mut Vec<String>, items: &[(usize, String)], kind: TemplateKind) {
    if items.is_empty() {
        return;
    }
    // 列表前留一个空行，否则程序会把它当成「段内列表」接进上一段。
    if out.last().is_some_and(|line| !line.trim().is_empty()) {
        out.push(String::new());
    }
    if kind.is_approval() {
        // 白头件、红头呈批件不收列表：每项各自成段，子项也拉平成段。
        for (index, (_, content)) in items.iter().enumerate() {
            if index > 0 {
                out.push(String::new());
            }
            out.push(content.clone());
        }
        return;
    }
    if kind == TemplateKind::MeetingAgenda {
        // 会议议程的研讨内容只认「1. 事项」：从 1 起连续编号、每项一行、不分级
        // （`validator` 会报项目符号与断号）。子项拉平进同一组。
        for (index, (_, content)) in items.iter().enumerate() {
            out.push(format!("{}. {content}", index + 1));
        }
        return;
    }
    let base = items[0].0;
    for (indent, content) in items {
        // 只分两级：比组内首项深两列及以上的都是子项，再深也不再往下分。
        let child = *indent >= base + 2;
        let prefix = if child { "  " } else { "" };
        out.push(format!("{prefix}- {content}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaped(markdown: &str) -> String {
        shape(markdown, TemplateKind::OfficialLetter)
    }

    #[test]
    fn loose_lists_become_tight_two_level_lists() {
        let raw = "请各单位做好以下工作：\n\n- 第一项\n\n  - 子项一\n\n  - 子项二\n\n- 第二项\n";
        assert_eq!(
            shaped(raw),
            "请各单位做好以下工作：\n\n- 第一项\n  - 子项一\n  - 子项二\n- 第二项\n"
        );
    }

    #[test]
    fn deeper_levels_fold_into_the_second() {
        let raw = "- 第一项\n\n  - 子项\n\n    - 孙项\n\n- 第二项\n";
        assert_eq!(shaped(raw), "- 第一项\n  - 子项\n  - 孙项\n- 第二项\n");
    }

    #[test]
    fn a_blank_line_is_kept_before_the_list() {
        let raw = "正文。\n- 第一项\n- 第二项\n";
        assert_eq!(shaped(raw), "正文。\n\n- 第一项\n- 第二项\n");
    }

    #[test]
    fn approval_documents_get_paragraphs_instead_of_lists() {
        let raw = "- 第一项工作\n\n  - 子项\n\n- 第二项工作\n";
        assert_eq!(
            shape(raw, TemplateKind::WhitePaper),
            "第一项工作\n\n子项\n\n第二项工作\n"
        );
    }

    #[test]
    fn ordered_lists_and_code_fences_are_left_alone() {
        let raw = "1. 第一项\n2. 第二项\n\n```\n- 不是列表\n```\n";
        assert_eq!(shaped(raw), raw);
    }

    /// 整形后的列表要能被起草页解析器读成「一组两项、子项并进父项」。
    #[test]
    fn shaped_lists_parse_as_one_group_with_children() {
        use crate::export::{MarkdownBlock, parse_markdown};
        let markdown = shaped("- 第一项\n\n- 第二项\n\n  - 子项一\n\n  - 子项二\n");
        let blocks = parse_markdown(&markdown);
        let items: Vec<(usize, String)> = blocks
            .iter()
            .filter_map(|block| match block {
                MarkdownBlock::OrderedListItem { number, text } => Some((*number, text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 2, "{markdown}\n{blocks:#?}");
        assert_eq!(items[0].0, 1);
        assert_eq!(items[1].0, 2);
        // 子项并进父项，按段内列表编号接在父项后面。
        assert!(items[1].1.contains("子项一"), "{:?}", items);
        assert!(items[1].1.contains("子项二"), "{:?}", items);
    }

    /// 会议议程只认「1. 事项」：拉平成一级、从 1 起连续编号。
    #[test]
    fn meeting_agenda_lists_become_numbered_items() {
        let raw = "三、研讨内容：\n\n- 听取汇报\n\n- 审议方案\n\n  - 子项\n";
        assert_eq!(
            shape(raw, TemplateKind::MeetingAgenda),
            "三、研讨内容：\n\n1. 听取汇报\n2. 审议方案\n3. 子项\n"
        );
    }

    #[test]
    fn a_list_followed_by_a_paragraph_keeps_the_paragraph() {
        let raw = "- 第一项\n\n- 第二项\n\n以上材料请按时报送。\n";
        assert_eq!(shaped(raw), "- 第一项\n- 第二项\n\n以上材料请按时报送。\n");
    }
}
