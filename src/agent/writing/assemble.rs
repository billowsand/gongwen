//! 按确认提纲把各节拼成全文：节标题由程序给出且不带手写编号，节内标题整体降到节标题之下。

use super::{Section, WritingTask};

/// 多节时节标题占 `##`，节内小标题从 `###` 起；只有一节时正文直接从 `##` 起。
fn body_base_level(sections: usize) -> usize {
    if sections > 1 { 3 } else { 2 }
}

/// 写节与统稿提示词里的标题约定，与 [`assemble`] 的降级规则一致。
pub(super) fn section_heading_rule(sections: usize) -> &'static str {
    if sections > 1 {
        "本节标题由程序生成，正文开头不要重复本节标题，也不要输出全文标题；本节内需要再分层时只用“### 小标题”及以下，标题不写编号。"
    } else {
        "不要输出全文标题；需要分层时用“## 标题”“### 标题”，标题不写编号。"
    }
}

/// 节标题去掉 Markdown 标记与手写编号；提纲里写了「一、」也只留名称，编号由导出程序生成。
pub(super) fn section_heading(title: &str) -> String {
    let plain = title.trim().trim_start_matches('#').trim();
    let cleaned = crate::export::clean_heading_number(plain);
    if cleaned.is_empty() {
        plain.to_string()
    } else {
        cleaned
    }
}

/// 拼接全文：各节优先用统稿建议；没有建议（锁定、人工稿）或审阅时选了保留原稿，用本节正文。
pub(super) fn assemble(task: &WritingTask) -> String {
    let count = task.sections.len();
    task.sections
        .iter()
        .map(|section| assemble_section(section, count))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn assemble_section(section: &Section, count: usize) -> String {
    let body = section
        .review_suggestion
        .as_deref()
        .filter(|_| !section.review_kept_draft)
        .unwrap_or(&section.markdown);
    let title = section_heading(&section.title);
    let body = normalize_body(body, &title, body_base_level(count));
    if count == 1 {
        body
    } else if body.is_empty() {
        format!("## {title}")
    } else {
        format!("## {title}\n\n{body}")
    }
}

/// 一行若是 ATX 标题，返回层级与标题文字。
fn heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.len() - trimmed.trim_start_matches('#').len();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    rest.starts_with(' ').then(|| (level, rest.trim()))
}

/// 去掉开头重复的全文标题或本节标题，把节内标题整体平移到 `base` 级起，并剥掉手写编号。
/// 附件标记之后的内容按附件自己的规则排，原样保留。
fn normalize_body(body: &str, title: &str, base: usize) -> String {
    let lines: Vec<&str> = body.trim().lines().collect();
    let mut start = 0;
    if let Some((level, text)) = lines.first().and_then(|line| heading(line))
        && (level == 1 || section_heading(text) == title)
    {
        start = 1;
        while lines.get(start).is_some_and(|line| line.trim().is_empty()) {
            start += 1;
        }
    }
    let lines = &lines[start..];
    let attachment = lines
        .iter()
        .position(|line| line.trim() == "<!-- [附件] -->")
        .unwrap_or(lines.len());
    let mut fenced = false;
    let mut levels = Vec::new();
    for line in &lines[..attachment] {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && let Some((level, _)) = heading(line) {
            levels.push(level);
        }
    }
    let Some(top) = levels.iter().min().copied() else {
        return lines.join("\n");
    };
    let mut fenced = false;
    let mut out = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if index >= attachment {
            out.push((*line).to_string());
            continue;
        }
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        match heading(line).filter(|_| !fenced) {
            Some((level, text)) => {
                let level = (level + base).saturating_sub(top).clamp(base, 6);
                out.push(format!("{} {}", "#".repeat(level), section_heading(text)));
            }
            None => out.push((*line).to_string()),
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateKind;

    fn task(sections: &[(&str, &str)]) -> WritingTask {
        let mut task = WritingTask::new(TemplateKind::PlainDocument);
        task.sections = sections
            .iter()
            .map(|(title, markdown)| Section {
                title: (*title).into(),
                markdown: (*markdown).into(),
                ..Default::default()
            })
            .collect();
        task
    }

    #[test]
    fn section_headings_drop_manual_numbers_and_body_headings_sit_below_them() {
        let task = task(&[
            (
                "一、精简检查考核",
                "## 一、精简检查考核\n\n导语。\n\n## 统筹计划\n\n正文。\n\n### 细化要求\n\n细则。",
            ),
            ("（二）规范报表", "# 关于减负的通知\n\n报表正文。"),
        ]);
        let text = assemble(&task);
        assert_eq!(
            text,
            "## 精简检查考核\n\n导语。\n\n### 统筹计划\n\n正文。\n\n#### 细化要求\n\n细则。\n\n## 规范报表\n\n报表正文。"
        );
        let blocks = crate::export::parse_markdown(&text);
        let headings: Vec<_> = blocks
            .iter()
            .filter_map(|block| match block {
                crate::export::MarkdownBlock::Heading(level, text) => Some((*level, text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            [
                (2, "精简检查考核"),
                (3, "统筹计划"),
                (4, "细化要求"),
                (2, "规范报表")
            ]
        );
    }

    #[test]
    fn a_single_unit_keeps_body_headings_at_the_top_body_level() {
        let task = task(&[(
            "工作要求",
            "### 第一部分 总体要求\n\n正文。\n\n#### 分项\n\n细则。",
        )]);
        assert_eq!(
            assemble(&task),
            "## 总体要求\n\n正文。\n\n### 分项\n\n细则。"
        );
    }

    #[test]
    fn attachments_and_fenced_lines_are_left_alone() {
        let task = task(&[
            ("措施", "```\n## 不是标题\n```"),
            (
                "背景",
                "## 情况\n\n正文。\n\n<!-- [附件] -->\n# 情况表\n\n## 表内说明",
            ),
        ]);
        let text = assemble(&task);
        assert!(text.contains("```\n## 不是标题\n```"));
        assert!(text.ends_with(
            "## 背景\n\n### 情况\n\n正文。\n\n<!-- [附件] -->\n# 情况表\n\n## 表内说明"
        ));
    }

    #[test]
    fn review_suggestions_replace_drafts_when_present() {
        let mut task = task(&[("背景", "原稿。"), ("措施", "原稿。")]);
        task.sections[1].review_suggestion = Some("统稿建议。".into());
        assert_eq!(
            assemble(&task),
            "## 背景\n\n原稿。\n\n## 措施\n\n统稿建议。"
        );
    }
}
