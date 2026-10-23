//! 按插入点所在章节给「样式标题」定级。
//!
//! 从 Word、网页粘进来的 `<h1>`–`<h6>`（以及 `MsoTitle`）只带自己的层级，不知道
//! 插到哪儿：把一篇文章粘进稿子 `## 二、主要任务` 下面，它的 `<h2>` 应该落到
//! `###`，`<h1>` 也不该再是一个新的文档标题。
//!
//! 规则（`docs/rich-paste-design.md` 第六节）：
//!
//! - **有编号的标题不动**：编号样式本身就是层级（「一、」永远是 `##`），由
//!   `structure` 按编号重排。
//! - 没有编号、来自 `<h1>`–`<h6>` 的标题整体往下平移，使最浅的一个落在
//!   `max(插入点上方最近的标题级数 + 1, 1)`；稿子里已经有 `# ` 标题时不许再出现
//!   `# `，最浅落到 `##`。
//! - 平移后超过 `#####` 的转成加粗段落 `**标题**`。
//!
//! 只往下平移，不往上提：粘进来的内容自身层级比上下文深，就让它更深。

use super::Context;
use crate::doc_import::structure;

/// 把 Markdown 里的样式标题整体平移。逐行处理，编号标题与非标题行原样保留。
pub(crate) fn shift(markdown: &str, ctx: &Context) -> String {
    let scheme: structure::Scheme = structure::marker_scheme(markdown);
    let mut shallowest: Option<u8> = None;
    for line in markdown.lines() {
        if let Some(heading) = heading_line(line)
            && structure::marker_level(scheme, heading.text).is_none()
        {
            shallowest = Some(shallowest.map_or(heading.level, |level| level.min(heading.level)));
        }
    }
    let Some(shallowest) = shallowest else {
        return markdown.to_string();
    };
    let floor = floor(ctx);
    if shallowest >= floor {
        return markdown.to_string();
    }
    let delta = floor - shallowest;
    let mut out: Vec<String> = Vec::with_capacity(markdown.lines().count());
    for line in markdown.lines() {
        let Some(heading) = heading_line(line) else {
            out.push(line.to_string());
            continue;
        };
        // 有编号的标题交给 `structure`，这里只管样式标题。
        if structure::marker_level(scheme, heading.text).is_some() {
            out.push(line.to_string());
            continue;
        }
        let level = heading.level + delta;
        if level > 5 {
            out.push(bold_paragraph(heading.text));
        } else {
            out.push(format!(
                "{}{} {}",
                heading.indent,
                "#".repeat(level as usize),
                heading.text
            ));
        }
    }
    let mut shifted = out.join("\n");
    if markdown.ends_with('\n') {
        shifted.push('\n');
    }
    shifted
}

/// 允许出现的最浅一级：插入点上方最近标题的下一级，且稿子里已经有 `# ` 标题时
/// 不许再来一个。
fn floor(ctx: &Context) -> u8 {
    let base = if ctx.has_title { 2 } else { 1 };
    match ctx.section_level {
        Some(level) => (level + 1).max(base),
        None => base,
    }
}

/// 转成加粗段落；整行已经加粗的不再套一层。
fn bold_paragraph(text: &str) -> String {
    let text = text.trim();
    if text.starts_with("**") && text.ends_with("**") && text.len() > 4 {
        text.to_string()
    } else {
        format!("**{text}**")
    }
}

struct HeadingLine<'a> {
    indent: &'a str,
    level: u8,
    text: &'a str,
}

/// `## 标题` → 缩进、级数与标题文字。
fn heading_line(line: &str) -> Option<HeadingLine<'_>> {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|&byte| byte == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    let text = rest.strip_prefix(' ')?.trim();
    if text.is_empty() {
        return None;
    }
    Some(HeadingLine {
        indent: &line[..line.len() - trimmed.len()],
        level: hashes as u8,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateKind;

    fn context(kind: TemplateKind, has_title: bool, section_level: Option<u8>) -> Context {
        Context {
            kind,
            draft_empty: false,
            has_title,
            section_level,
        }
    }

    fn shifted(markdown: &str, ctx: &Context) -> String {
        shift(markdown, ctx)
    }

    #[test]
    fn style_headings_shift_under_the_current_section() {
        let markdown = "# 关于X的通知\n\n## 背景\n\n### 现状\n\n正文。";
        // 稿子里已有标题、插在 `##` 一级的章节下面：整体往下平移，最浅落到 `###`。
        assert_eq!(
            shifted(
                markdown,
                &context(TemplateKind::OfficialLetter, true, Some(2))
            ),
            "### 关于X的通知\n\n#### 背景\n\n##### 现状\n\n正文。"
        );
        // 没有上层章节时最浅落到 `##`。
        assert_eq!(
            shifted(markdown, &context(TemplateKind::OfficialLetter, true, None)),
            "## 关于X的通知\n\n### 背景\n\n#### 现状\n\n正文。"
        );
        // 空稿子允许 `# `。
        assert_eq!(
            shifted(
                markdown,
                &context(TemplateKind::OfficialLetter, false, None)
            ),
            "# 关于X的通知\n\n## 背景\n\n### 现状\n\n正文。"
        );
    }

    #[test]
    fn numbered_headings_are_left_to_structure() {
        let markdown = "# 1.1 背景\n\n## 1.2 现状\n\n### 没有编号\n";
        // 编号标题不动；没有编号的那一个已经在 `###`，不比上下文浅，也不动。
        assert_eq!(
            shifted(
                markdown,
                &context(TemplateKind::OfficialLetter, true, Some(2))
            ),
            markdown
        );
        // 插在 `#` 一级章节下面时，最浅的无编号标题落到 `##`，编号标题仍不动。
        let markdown = "# 一、背景\n\n# 没有编号\n";
        assert_eq!(
            shifted(
                markdown,
                &context(TemplateKind::OfficialLetter, true, Some(1))
            ),
            "# 一、背景\n\n## 没有编号\n"
        );
    }

    #[test]
    fn headings_below_the_fifth_level_become_bold_paragraphs() {
        let markdown = "## 一\n\n### 二\n\n#### 三\n\n##### 四";
        // 插在 `#####` 一节下面：最浅落到 `######`，全部超过 `#####` 的转加粗段落。
        assert_eq!(
            shifted(
                markdown,
                &context(TemplateKind::OfficialLetter, true, Some(5))
            ),
            "**一**\n\n**二**\n\n**三**\n\n**四**"
        );
    }

    #[test]
    fn already_deep_headings_are_not_pulled_up() {
        let markdown = "### 背景\n\n#### 现状";
        assert_eq!(
            shifted(markdown, &context(TemplateKind::OfficialLetter, true, None)),
            markdown
        );
    }
}
