//! 分阶段工作稿交付前的确定性来源检查，不把风格与结构样稿当作事实。

use crate::agent::writing::{SourceRole, WritingTask};
use crate::models::VocabularyEntry;
use std::sync::LazyLock;

static PENDING: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"【待核实[^】]*】").expect("固定待核实标记正则"));

pub(super) fn delivery_issues(task: &WritingTask, vocabulary: &[VocabularyEntry]) -> Vec<String> {
    let mut issues: Vec<_> = task
        .validate_sources()
        .into_iter()
        .map(|issue| issue.message)
        .collect();
    if task.final_markdown.trim().is_empty() {
        issues.push("尚无完成的统稿".into());
    }
    let source = task
        .sources
        .iter()
        .filter(|source| source.role == SourceRole::Content)
        .map(|source| source.text.as_str())
        .chain(std::iter::once(task.topic.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let text = crate::agent::evidence::strip_citations(&task.final_markdown);
    let text = PENDING.replace_all(&text, "");
    for fact in crate::ai_guard::extract_key_facts(&text, vocabulary) {
        let document_title = fact.value.trim_start_matches('《').trim_end_matches('》');
        let known_title = fact.kind == crate::ai_guard::FactKind::Document
            && task
                .sources
                .iter()
                .filter(|source| source.role == SourceRole::Content)
                .any(|source| {
                    std::path::Path::new(&source.title)
                        .file_stem()
                        .and_then(|name| name.to_str())
                        == Some(document_title)
                        || source.text.lines().any(|line| {
                            line.trim().trim_start_matches('#').trim() == document_title
                        })
                });
        if !source.contains(&fact.value) && !known_title {
            issues.push(format!(
                "{}「{}」未在内容材料或用户主题中找到，请补充依据或修订",
                fact.kind.label(),
                fact.value
            ));
        }
    }
    issues.sort();
    issues.dedup();
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::writing::SourceSnapshot;

    #[test]
    fn sample_facts_cannot_authorize_delivery() {
        let mut task = WritingTask::new(crate::models::TemplateKind::PlainDocument);
        task.add_source(
            SourceSnapshot::new("内容", SourceRole::Content, "已完成12项任务。").unwrap(),
        );
        task.add_source(
            SourceSnapshot::new("风格", SourceRole::Style, "已完成36项任务。").unwrap(),
        );
        task.final_markdown = "已完成36项任务。".into();
        assert!(
            delivery_issues(&task, &[])
                .iter()
                .any(|issue| issue.contains("36项"))
        );
        task.final_markdown = "已完成12项任务。".into();
        assert!(delivery_issues(&task, &[]).is_empty());
    }

    #[test]
    fn a_placeholder_elsewhere_does_not_authorize_an_asserted_number() {
        let mut task = WritingTask::new(crate::models::TemplateKind::PlainDocument);
        task.final_markdown = "【待核实：36项】。另已完成36项任务。".into();
        assert!(
            delivery_issues(&task, &[])
                .iter()
                .any(|issue| issue.contains("36项"))
        );
        task.final_markdown = "需完成【待核实：任务数量】。".into();
        assert!(delivery_issues(&task, &[]).is_empty());
    }
}
