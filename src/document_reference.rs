//! 公文引用：稳定标记与自带快照的定义。定义是独占一行的 HTML 注释，随正文、
//! 历史版本及同步 ZIP 保存；渲染不依赖来源稿件是否在本机。

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::BTreeMap, ops::Range};

pub(crate) const PREFIX: &str = "{{公文:";
pub(crate) const DEFINITION: &str = "<!-- gongwen-reference ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reference {
    pub id: String,
    pub title: String,
    pub number: String,
    pub no_number: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_uuid: Option<String>,
}

impl Reference {
    pub fn manual(title: &str, number: &str, no_number: bool) -> Result<Self> {
        let reference = Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: title
                .trim()
                .trim_start_matches('《')
                .trim_end_matches('》')
                .trim()
                .into(),
            number: if no_number {
                String::new()
            } else {
                number.trim().into()
            },
            no_number,
            document_uuid: None,
            revision_uuid: None,
        };
        reference.validate()?;
        Ok(reference)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(uuid::Uuid::parse_str(&self.id).is_ok(), "引用编号无效");
        ensure!(!self.title.trim().is_empty(), "请填写公文名称");
        ensure!(
            !self.title.chars().any(char::is_control),
            "公文名称不能包含换行或控制字符"
        );
        ensure!(
            !self.number.chars().any(char::is_control),
            "发文字号不能包含换行或控制字符"
        );
        ensure!(
            (self.no_number && self.number.is_empty())
                || (!self.no_number && !self.number.trim().is_empty()),
            "请填写完整发文字号，或明确选择无文号"
        );
        for id in [&self.document_uuid, &self.revision_uuid]
            .into_iter()
            .flatten()
        {
            ensure!(uuid::Uuid::parse_str(id).is_ok(), "引用来源身份无效");
        }
        ensure!(
            self.revision_uuid.is_none() || self.document_uuid.is_some(),
            "引用版本缺少来源稿件身份"
        );
        Ok(())
    }

    pub fn token(&self) -> String {
        format!("{PREFIX}{}}}}}", self.id)
    }

    pub fn display(&self) -> String {
        if self.no_number {
            format!("《{}》", self.title)
        } else {
            format!("《{}》（{}）", self.title, self.number)
        }
    }

    pub fn definition(&self) -> String {
        // 避免用户文字提前关闭 HTML 注释，也避免把定义里的标记识别为正文引用。
        let json = serde_json::to_string(self)
            .expect("引用可序列化")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e");
        format!("{DEFINITION}{json} -->")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Issue {
    pub range: Range<usize>,
    pub message: String,
}

#[derive(Default)]
pub(crate) struct References {
    pub items: BTreeMap<String, Reference>,
    pub issues: Vec<Issue>,
}

impl References {
    pub fn read(markdown: &str) -> Self {
        let mut result = Self::default();
        for (start, line) in definition_lines(markdown) {
            if let Some(json) = line.trim().strip_prefix(DEFINITION) {
                let parsed = json
                    .strip_suffix(" -->")
                    .and_then(|json| serde_json::from_str::<Reference>(json).ok());
                let issue = match parsed {
                    Some(reference) => match reference.validate() {
                        Ok(()) if !result.items.contains_key(&reference.id) => {
                            result.items.insert(reference.id.clone(), reference);
                            None
                        }
                        Ok(()) => Some("引用定义重复，请修复后导出".into()),
                        Err(error) => Some(error.to_string()),
                    },
                    None => Some("公文引用定义损坏，请修复后导出".into()),
                };
                if let Some(message) = issue {
                    result.issues.push(Issue {
                        range: start..start + line.len(),
                        message,
                    });
                }
            }
        }
        for (range, id) in occurrences(markdown) {
            if !result.items.contains_key(&id) {
                result.issues.push(Issue {
                    range,
                    message: format!("公文引用未定义或编号损坏：{id}"),
                });
            }
        }
        result
    }

    pub fn check(markdown: &str) -> Result<()> {
        let references = Self::read(markdown);
        if !references.issues.is_empty() {
            bail!(
                "公文引用需要修复：{}",
                references
                    .issues
                    .iter()
                    .map(|issue| {
                        let line = markdown[..issue.range.start]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            + 1;
                        format!("第{line}行：{}", issue.message)
                    })
                    .collect::<Vec<_>>()
                    .join("；")
            );
        }
        Ok(())
    }

    /// 替换发生在源码切行以后，原始行长及块范围保持不变。插入普通文字时转义
    /// Markdown 语法，防止名称中的星号、竖线等被误认成加粗或表格。
    pub fn apply<'a>(
        &self,
        line: &'a str,
        start: usize,
        spans: &[(Range<usize>, String)],
    ) -> Cow<'a, str> {
        if line.trim().starts_with(DEFINITION) || !line.contains(PREFIX) {
            return Cow::Borrowed(line);
        }
        let mut text = String::new();
        let mut copied = 0;
        for (range, id) in spans
            .iter()
            .filter(|(range, _)| range.start >= start && range.end <= start + line.len())
        {
            let range = range.start - start..range.end - start;
            text.push_str(&line[copied..range.start]);
            let display = self
                .items
                .get(id)
                .map_or_else(|| format!("【公文引用待修复：{id}】"), Reference::display);
            text.push_str(&escape_markdown(&display));
            copied = range.end;
        }
        if copied == 0 {
            return Cow::Borrowed(line);
        }
        text.push_str(&line[copied..]);
        Cow::Owned(text)
    }

    /// 通用 Markdown / 对外复制使用展开后的正文，不携带内部定义。
    pub fn expanded(&self, markdown: &str) -> String {
        let spans = occurrences(markdown);
        let definitions = definition_lines(markdown)
            .into_iter()
            .map(|(start, _)| start)
            .collect::<std::collections::BTreeSet<_>>();
        let mut result = String::new();
        let mut start = 0;
        for line in markdown.split_inclusive('\n') {
            if !definitions.contains(&start) {
                if spans
                    .iter()
                    .any(|(range, _)| range.start >= start && range.start < start + line.len())
                {
                    result.push_str(&self.apply(line, start, &spans));
                } else {
                    result.push_str(line);
                }
            }
            start += line.len();
        }
        result
    }
}

pub(crate) fn escape_markdown(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '*' | '_' | '`' | '[' | ']' | '{' | '}' | '|' | '$' | '<' | '>'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 忽略定义、代码围栏、行内代码、公式与显式转义；破损标记也返回，不能静默印出。
pub(crate) fn occurrences(markdown: &str) -> Vec<(Range<usize>, String)> {
    let mut found = Vec::new();
    let mut offset = 0;
    let mut fence = None;
    let mut math_delimiter: Option<usize> = None;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !fence_line(trimmed, &mut fence) && fence.is_none() && !trimmed.starts_with(DEFINITION) {
            let mut index = 0;
            let mut code_delimiter: Option<usize> = None;
            while index < line.len() {
                let rest = &line[index..];
                let ch = rest.chars().next().unwrap();
                if ch == '\\' {
                    index += 1;
                    if index < line.len() {
                        index += line[index..].chars().next().unwrap().len_utf8();
                    }
                    continue;
                }
                if ch == '`' {
                    let length = rest.chars().take_while(|ch| *ch == '`').count();
                    if code_delimiter == Some(length) {
                        code_delimiter = None;
                    } else if code_delimiter.is_none() && math_delimiter.is_none() {
                        code_delimiter = Some(length);
                    }
                    index += length;
                    continue;
                }
                if ch == '$' && code_delimiter.is_none() {
                    let length = rest.chars().take_while(|ch| *ch == '$').count();
                    if math_delimiter == Some(length) {
                        math_delimiter = None;
                    } else if math_delimiter.is_none() {
                        math_delimiter = Some(length);
                    }
                    index += length;
                    continue;
                }
                if code_delimiter.is_none() && math_delimiter.is_none() && rest.starts_with(PREFIX)
                {
                    let end = rest
                        .find("}}")
                        .map_or(rest.trim_end_matches(['\r', '\n']).len(), |end| end + 2);
                    let id = rest[PREFIX.len()..end].trim_end_matches("}}").to_string();
                    found.push((offset + index..offset + index + end, id));
                    index += end;
                } else {
                    index += ch.len_utf8();
                }
            }
        }
        if math_delimiter == Some(1) {
            math_delimiter = None;
        }
        offset += line.len();
    }
    found
}

pub(crate) fn put(markdown: &str, reference: &Reference) -> String {
    let definitions = definition_lines(markdown)
        .into_iter()
        .map(|(start, _)| start)
        .collect::<std::collections::BTreeSet<_>>();
    let mut out = String::new();
    let mut replaced = false;
    let mut start = 0;
    for line in markdown.split_inclusive('\n') {
        let matches = definitions.contains(&start)
            && line
                .trim()
                .strip_prefix(DEFINITION)
                .and_then(|json| json.strip_suffix(" -->"))
                .and_then(|json| serde_json::from_str::<Reference>(json).ok())
                .is_some_and(|old| old.id == reference.id);
        if matches {
            out.push_str(&reference.definition());
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(line);
        }
        start += line.len();
    }
    if !replaced {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&reference.definition());
        out.push('\n');
    }
    out
}

/// 只移除引用定义，正文标记原样保留，供原生源码交换与逐条合并使用。
pub(crate) fn without_definitions(markdown: &str) -> String {
    remove_definitions(markdown, |_| true)
}

pub(crate) fn remove_definition(markdown: &str, id: &str) -> String {
    remove_definitions(markdown, |line| {
        line.trim()
            .strip_prefix(DEFINITION)
            .and_then(|json| json.strip_suffix(" -->"))
            .and_then(|json| serde_json::from_str::<Reference>(json).ok())
            .is_some_and(|reference| reference.id == id)
    })
}

/// 围栏中的源码示例不参与快照解析、替换或移除。
fn definition_lines(markdown: &str) -> Vec<(usize, &str)> {
    let mut fence = None;
    crate::export::source_lines(markdown)
        .into_iter()
        .filter(|(_, line)| {
            let trimmed = line.trim_start();
            !fence_line(trimmed, &mut fence) && fence.is_none() && trimmed.starts_with(DEFINITION)
        })
        .collect()
}

fn fence_line(line: &str, fence: &mut Option<(char, usize)>) -> bool {
    let Some(ch @ ('`' | '~')) = line.chars().next() else {
        return false;
    };
    let length = line.chars().take_while(|next| *next == ch).count();
    if length < 3 {
        return false;
    }
    if let Some((opening, minimum)) = *fence {
        if opening == ch && length >= minimum && line[length..].trim().is_empty() {
            *fence = None;
        }
    } else {
        *fence = Some((ch, length));
    }
    true
}

fn remove_definitions(markdown: &str, should_remove: impl Fn(&str) -> bool) -> String {
    let removals = definition_lines(markdown)
        .into_iter()
        .filter(|(_, line)| should_remove(line))
        .map(|(start, _)| start)
        .collect::<std::collections::BTreeSet<_>>();
    let mut start = 0;
    markdown
        .split_inclusive('\n')
        .filter(|line| {
            let keep = !removals.contains(&start);
            start += line.len();
            keep
        })
        .collect()
}

pub(crate) fn to_plain(markdown: &str, reference: &Reference) -> String {
    let mut result = markdown.to_string();
    let spans = occurrences(markdown)
        .into_iter()
        .filter(|(_, id)| id == &reference.id)
        .collect::<Vec<_>>();
    for (range, _) in spans.into_iter().rev() {
        result.replace_range(range, &escape_markdown(&reference.display()));
    }
    remove_definition(&result, &reference.id)
}

/// 文字校对屏蔽引用元数据与结构标记，字节长度保持不变，其他文字的定位不漂移。
/// 引用内容需要更正时通过引用面板，不能让普通词表替换把 UUID 或 JSON 改坏。
pub(crate) fn masked(markdown: &str) -> Cow<'_, str> {
    if !markdown.contains(PREFIX) && !markdown.contains(DEFINITION) {
        return Cow::Borrowed(markdown);
    }
    let mut bytes = markdown.as_bytes().to_vec();
    for (range, _) in occurrences(markdown) {
        bytes[range].fill(b' ');
    }
    for (start, line) in definition_lines(markdown) {
        bytes[start..start + line.len()].fill(b' ');
    }
    Cow::Owned(String::from_utf8(bytes).expect("只把完整 UTF-8 范围换成空格"))
}

/// 冲突面板展示人可读的引用信息，实际合并仍使用原定义，绝不丢失身份字段。
pub(crate) fn conflict_display(markdown: &str) -> String {
    let references = References::read(markdown);
    if !references.items.is_empty() && without_definitions(markdown).trim().is_empty() {
        references
            .items
            .values()
            .map(Reference::display)
            .collect::<Vec<_>>()
            .join("；")
    } else {
        markdown.to_string()
    }
}

/// AI 可以调整引用周围文字及位置，但不得修改定义或增删引用。规则修订也不能
/// 直接改写结构标记；用户通过引用面板执行这些操作。
pub(crate) fn ensure_preserved(before: &str, after: &str) -> Result<()> {
    let old = References::read(before);
    let new = References::read(after);
    let counts = |text: &str| {
        let mut counts = BTreeMap::<String, usize>::new();
        for (_, id) in occurrences(text) {
            *counts.entry(id).or_default() += 1;
        }
        counts
    };
    ensure!(
        old.items == new.items && counts(before) == counts(after),
        "该建议改变了公文引用，请通过「公文引用」面板手工核对与调整，当前正文未改变"
    );
    ensure!(
        new.issues.len() <= old.issues.len(),
        "该建议损坏了公文引用，已放弃采纳"
    );
    Ok(())
}

/// 从别篇粘贴时只带使用到的定义；同 ID 不同快照必须重新生成 ID，不覆盖目标篇。
pub(crate) fn transfer(
    fragment: &str,
    items: &[Reference],
    target: &str,
) -> (String, Vec<Reference>) {
    let destination = References::read(target);
    let mut text = fragment.to_string();
    let mut definitions = Vec::new();
    for reference in items {
        let mut reference = reference.clone();
        if destination
            .items
            .get(&reference.id)
            .is_some_and(|old| old != &reference)
        {
            let old_token = reference.token();
            reference.id = uuid::Uuid::new_v4().to_string();
            text = text.replace(&old_token, &reference.token());
        }
        definitions.push(reference);
    }
    (text, definitions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_self_contained_and_escapes_markup() {
        let reference = Reference::manual("关于 A*B 的函", "办函〔2026〕12号", false).unwrap();
        let source = put(
            &format!("# 本篇\n根据{}办理。", reference.token()),
            &reference,
        );
        References::check(&source).unwrap();
        let refs = References::read(&source);
        assert_eq!(refs.items[&reference.id], reference);
        let expanded = refs.expanded(&source);
        assert!(expanded.contains("《关于 A\\*B 的函》（办函〔2026〕12号）"));
        assert!(!expanded.contains(DEFINITION));
        let blocks = crate::export::parse_markdown_located(&source);
        let paragraph = &blocks[1];
        assert_eq!(
            &source[paragraph.range.clone()],
            format!("根据{}办理。", reference.token())
        );
        assert!(
            matches!(&paragraph.block, crate::export::MarkdownBlock::Paragraph(text) if text.contains("办函〔2026〕12号"))
        );
    }

    #[test]
    fn missing_duplicate_and_damaged_definitions_block_export() {
        assert!(References::check("根据{{公文:不存在}}办理。").is_err());
        assert!(References::check("根据{{公文:坏标记").is_err());
        let reference = Reference::manual("呈批件", "", true).unwrap();
        assert!(
            References::check(&format!(
                "{}\n{}",
                reference.definition(),
                reference.definition()
            ))
            .is_err()
        );
        assert!(References::check("<!-- gongwen-reference 损坏 -->").is_err());
        assert_eq!(reference.display(), "《呈批件》");
    }

    #[test]
    fn document_reference_transfer_does_not_overwrite_another_snapshot() {
        let reference = Reference::manual("旧函", "办函〔2026〕12号", false).unwrap();
        let mut other = reference.clone();
        other.number = "办函〔2026〕13号".into();
        let target = put(&format!("根据{}办理。", other.token()), &other);
        let (fragment, definitions) = transfer(
            &reference.token(),
            std::slice::from_ref(&reference),
            &target,
        );
        assert_ne!(definitions[0].id, reference.id);
        let merged = put(&format!("{fragment}\n{target}"), &definitions[0]);
        References::check(&merged).unwrap();
        assert_eq!(References::read(&merged).items[&reference.id], other);
        assert_eq!(
            References::read(&merged).items[&definitions[0].id].number,
            reference.number
        );
    }

    #[test]
    fn document_reference_ai_guard_preserves_definition_and_count() {
        let reference = Reference::manual("检查函", "办函〔2026〕12号", false).unwrap();
        let before = put(&format!("根据{}办理。", reference.token()), &reference);
        let after = before.replace("办理", "认真办理");
        ensure_preserved(&before, &after).unwrap();
        assert!(ensure_preserved(&before, &before.replace(&reference.token(), "该函")).is_err());
        let mut changed = reference.clone();
        changed.number = "办函〔2026〕13号".into();
        assert!(ensure_preserved(&before, &put(&before, &changed)).is_err());
        assert!(ensure_preserved("正文", &before).is_err());
    }

    #[test]
    fn document_reference_redline_compares_printed_words() {
        let reference = Reference::manual("检查函", "办函〔2026〕12号", false).unwrap();
        let before = put(
            &format!("# 情况报告\n\n根据{}办理。", reference.token()),
            &reference,
        );
        let mut changed = reference.clone();
        changed.number = "办函〔2026〕13号".into();
        let after = put(&before, &changed);
        let redline = crate::redline::build(&before, &after);
        assert!(!redline.is_empty());
        assert!(!redline.markdown.contains(PREFIX));
        assert!(!redline.markdown.contains(DEFINITION));
        assert!(redline.markdown.contains("12"));
        assert!(redline.markdown.contains("13"));
        changed.document_uuid = Some(uuid::Uuid::new_v4().to_string());
        assert!(crate::redline::build(&after, &put(&after, &changed)).is_empty());
    }

    #[test]
    fn document_reference_markdown_archive_has_plain_and_editable_documents() {
        use std::io::Read;
        let reference = Reference::manual("来函", "办函〔2026〕12号", false).unwrap();
        let markdown = put(
            &format!("# 本篇\n根据{}办理。", reference.token()),
            &reference,
        );
        let dir = tempfile::tempdir().unwrap();
        let output = crate::export::export_artifacts(
            dir.path(),
            &crate::models::DraftInput::default(),
            &markdown,
            &crate::models::ExportSelection {
                markdown: true,
                docx: false,
                pdf: false,
                ..Default::default()
            },
            &crate::units::UnitDisplay::new(&[]),
            &crate::models::FontConfig::default(),
            &crate::models::NumberingConfig::default(),
            None,
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&output.files[0]).unwrap()).unwrap();
        for index in 0..zip.len() {
            let mut file = zip.by_index(index).unwrap();
            let editable = file.name().ends_with("-可编辑.md");
            let mut body = String::new();
            file.read_to_string(&mut body).unwrap();
            if editable {
                References::check(&body).unwrap();
                assert_eq!(References::read(&body).items[&reference.id], reference);
            } else {
                assert!(body.contains(&reference.display()));
                assert!(!body.contains(PREFIX));
                assert!(!body.contains(DEFINITION));
            }
        }
        assert_eq!(zip.len(), 2);
    }

    #[test]
    fn definitions_cannot_close_comments_or_create_tokens() {
        let reference = Reference::manual("名称 --> {{公文:示例}}", "", true).unwrap();
        let source = put(&reference.token(), &reference);
        References::check(&source).unwrap();
        assert_eq!(References::read(&source).items[&reference.id], reference);
        assert_eq!(occurrences(&source).len(), 1);
    }

    #[test]
    fn literal_code_math_and_escaped_examples_are_not_references() {
        assert!(
            occurrences(
                "`{{公文:代码}}` ${{公文:公式}}$ \\{{公文:转义}}\n```\n{{公文:围栏}}\n```\n"
            )
            .is_empty()
        );
        assert!(occurrences("``字面 ` {{公文:代码}}``\n$$\n{{公文:跨行公式}}\n$$\n").is_empty());
    }

    #[test]
    fn document_reference_fenced_definitions_remain_literal_examples() {
        let reference = Reference::manual("示例函", "某函〔2026〕8号", false).unwrap();
        let example = format!(
            "````markdown\n```\n{}\n{}\n```\n````\n",
            reference.definition(),
            reference.token()
        );
        References::check(&example).unwrap();
        assert!(References::read(&example).items.is_empty());
        assert_eq!(References::read(&example).expanded(&example), example);
        assert_eq!(without_definitions(&example), example);
        assert_eq!(remove_definition(&example, &reference.id), example);
        let actual = put(
            &format!("{example}\n根据{}办理。", reference.token()),
            &reference,
        );
        References::check(&actual).unwrap();
        assert_eq!(occurrences(&actual).len(), 1);
        assert_eq!(References::read(&actual).items.len(), 1);
    }

    #[test]
    fn document_reference_merge_keeps_definitions_on_separate_lines() {
        let reference = Reference::manual("来函", "某函〔2026〕8号", false).unwrap();
        let base = format!(
            "{}\n根据{}办理。",
            reference.definition(),
            reference.token()
        );
        let mut updated = reference.clone();
        updated.title = "修订后的来函".into();
        let local = format!("{}\n根据{}办理。", updated.definition(), reference.token());
        let incoming = base.replace("办理", "认真办理");
        let input = crate::models::DraftInput::default();
        let proposal = crate::manuscript::merge::MergeProposal::build(
            (&input, &base, ""),
            (&input, &local, ""),
            (&input, &incoming, ""),
        )
        .unwrap();
        let (_, merged, _) = proposal.resolve(&[]).unwrap();
        References::check(&merged).unwrap();
        assert!(merged.contains("认真办理"));
        assert_eq!(References::read(&merged).items[&reference.id], updated);
    }

    #[test]
    fn document_reference_multiline_math_keeps_only_real_occurrences() {
        let reference = Reference::manual("来函", "某函〔2026〕8号", false).unwrap();
        let source = put(
            &format!(
                "$$\n{} $$ 根据{}办理。",
                reference.token(),
                reference.token()
            ),
            &reference,
        );
        assert_eq!(occurrences(&source).len(), 1);
        let expanded = References::read(&source).expanded(&source);
        assert!(expanded.contains(&reference.token()));
        assert_eq!(expanded.matches(&reference.display()).count(), 1);
        let lines = crate::export::parse_markdown_located(&source);
        assert!(lines.iter().any(|line| {
            matches!(&line.block, crate::export::MarkdownBlock::Paragraph(text)
                if text.contains(&reference.token()) && text.contains(&reference.display()))
        }));
    }

    #[test]
    fn document_reference_plain_conversion_and_proofreading_keep_source_offsets() {
        let reference = Reference::manual("有错别字的来函", "某函〔2026〕8号", false).unwrap();
        let source = put(
            &format!(
                "根据{}办理。错别字\n再次引用{}。\n`{}`",
                reference.token(),
                reference.token(),
                reference.token()
            ),
            &reference,
        );
        let mask = masked(&source);
        assert_eq!(mask.len(), source.len());
        assert_eq!(
            mask.find("错别字"),
            source.find("。错别字").map(|start| start + "。".len())
        );
        let lexicon = crate::proofread::Lexicon::parse(
            "条目编号\t错误写法\t建议写法\t级别\t命中条件\t分组\t说明\t启用\nTEST\t错别字\t正确词\t必错\t总是\t测试\t\t是\n",
        );
        let notes = lexicon.check(&source);
        assert_eq!(notes.len(), 1);
        assert_eq!(&source[notes[0].span.clone()], "错别字");
        let broken = source.replace(&reference.id, "损坏");
        let notes = crate::proofread_rules::check(&crate::models::DraftInput::default(), &broken);
        assert!(notes.iter().any(|note| note.group == "公文引用"
            && note.level == crate::proofread::Level::MustFix
            && note.replacement.is_none()));
        let plain = to_plain(&source, &reference);
        assert_eq!(plain.matches(&reference.display()).count(), 2);
        assert!(plain.contains(&format!("`{}`", reference.token())));
        assert!(!plain.contains(DEFINITION));
        References::check(&plain).unwrap();
    }

    #[test]
    fn document_reference_research_editable_source_preserves_numbered_table_tokens() {
        let reference = Reference::manual("来函", "某函〔2026〕8号", false).unwrap();
        let markdown = put(
            &format!(
                "# 报告\n\n<!-- [序号表] -->\n| 序号 | 依据 |\n| --- | --- |\n| | {} |\n",
                reference.token()
            ),
            &reference,
        );
        let source = crate::export::research::markdown_source_editable(
            &crate::models::DraftInput::default(),
            &markdown,
            false,
            &crate::models::NumberingConfig::default(),
        );
        assert!(source.contains(&reference.token()));
        assert!(!source.contains(&reference.display()));
        References::check(&source).unwrap();
    }

    #[test]
    fn preview_word_and_pdf_data_share_the_same_reference_text() {
        use std::io::Read;
        let reference = Reference::manual("关于开展检查的函", "某办函〔2026〕12号", false).unwrap();
        let markdown = put(
            &format!("# 办理情况报告\n\n根据{}办理。", reference.token()),
            &reference,
        );
        let input = crate::models::DraftInput::default();
        let json = crate::redline::typst_tests::typst_json(&input, &markdown);
        fn texts(value: &serde_json::Value, out: &mut String) {
            match value {
                serde_json::Value::Array(items) => {
                    for item in items {
                        texts(item, out);
                    }
                }
                serde_json::Value::Object(fields) => {
                    if let Some(text) = fields.get("t").and_then(serde_json::Value::as_str) {
                        out.push_str(text);
                    }
                    for (key, value) in fields {
                        if key != "t" {
                            texts(value, out);
                        }
                    }
                }
                _ => {}
            }
        }
        let mut rendered = String::new();
        texts(&serde_json::from_str(&json).unwrap(), &mut rendered);
        assert!(rendered.contains(&reference.display()), "{rendered}");
        assert!(!json.contains(PREFIX));
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("引用.docx");
        crate::export::write_docx(
            &path,
            &input,
            &markdown,
            &crate::units::UnitDisplay::new(&[]),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("《关于开展检查的函》"));
        assert!(xml.contains("某办函〔2026〕12号"));
        assert!(!xml.contains(PREFIX));
        assert!(!xml.contains(DEFINITION));
    }
}
