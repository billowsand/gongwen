//! 原始 Markdown 与行文要素的保守三方合并；花脸稿仅用于展示。
use crate::models::DraftInput;
use anyhow::{Context, Result};
use serde_json::Value;
use similar::{DiffTag, TextDiff};
use std::ops::Range;

#[derive(Debug, Clone)]
pub struct FieldConflict {
    pub path: Vec<String>,
    pub local: Value,
    pub incoming: Value,
}

#[derive(Debug, Clone)]
pub enum MarkdownChunk {
    Text(String),
    Conflict {
        base: String,
        local: String,
        incoming: String,
    },
}

#[derive(Debug, Clone)]
pub struct MergeProposal {
    merged_fields: Value,
    pub field_conflicts: Vec<FieldConflict>,
    pub markdown: Vec<MarkdownChunk>,
    pub notes: String,
    pub notes_conflict: Option<(String, String)>,
}

impl MergeProposal {
    pub fn build(
        base: (&DraftInput, &str, &str),
        local: (&DraftInput, &str, &str),
        incoming: (&DraftInput, &str, &str),
    ) -> Result<Self> {
        let base_json = serde_json::to_value(base.0)?;
        let local_json = serde_json::to_value(local.0)?;
        let incoming_json = serde_json::to_value(incoming.0)?;
        let mut conflicts = Vec::new();
        let merged_fields = merge_value(
            &base_json,
            &local_json,
            &incoming_json,
            &mut Vec::new(),
            &mut conflicts,
        );
        let notes_conflict = if local.2 != incoming.2 && local.2 != base.2 && incoming.2 != base.2 {
            Some((local.2.to_string(), incoming.2.to_string()))
        } else {
            None
        };
        let notes = if local.2 == base.2 {
            incoming.2
        } else {
            local.2
        }
        .to_string();
        Ok(Self {
            merged_fields,
            field_conflicts: conflicts,
            markdown: merge_markdown(base.1, local.1, incoming.1),
            notes,
            notes_conflict,
        })
    }

    /// 无可信共同祖先时，整份要素和正文都交由用户决定。
    pub fn manual(
        local: (&DraftInput, &str, &str),
        incoming: (&DraftInput, &str, &str),
    ) -> Result<Self> {
        let local_json = serde_json::to_value(local.0)?;
        let incoming_json = serde_json::to_value(incoming.0)?;
        Ok(Self {
            merged_fields: local_json.clone(),
            field_conflicts: if local_json == incoming_json {
                Vec::new()
            } else {
                vec![FieldConflict {
                    path: Vec::new(),
                    local: local_json,
                    incoming: incoming_json,
                }]
            },
            markdown: if local.1 == incoming.1 {
                vec![MarkdownChunk::Text(local.1.to_string())]
            } else {
                vec![MarkdownChunk::Conflict {
                    base: String::new(),
                    local: local.1.to_string(),
                    incoming: incoming.1.to_string(),
                }]
            },
            notes: local.2.to_string(),
            notes_conflict: (local.2 != incoming.2)
                .then(|| (local.2.to_string(), incoming.2.to_string())),
        })
    }

    pub fn conflict_count(&self) -> usize {
        self.field_conflicts.len()
            + self
                .markdown
                .iter()
                .filter(|chunk| matches!(chunk, MarkdownChunk::Conflict { .. }))
                .count()
            + usize::from(self.notes_conflict.is_some())
    }

    /// choices 中 true 取导入侧，false 取本机侧；顺序是要素、备注、正文冲突。
    pub fn resolve(&self, choices: &[bool]) -> Result<(DraftInput, String, String)> {
        anyhow::ensure!(choices.len() == self.conflict_count(), "冲突选择数量不匹配");
        let mut fields = self.merged_fields.clone();
        let mut index = 0;
        for conflict in &self.field_conflicts {
            if choices[index] {
                set_path(&mut fields, &conflict.path, conflict.incoming.clone());
            }
            index += 1;
        }
        let snapshot = serde_json::from_value(fields).context("合并后的行文要素无效")?;
        let notes = if let Some((_, incoming)) = &self.notes_conflict {
            let take_incoming = choices[index];
            index += 1;
            if take_incoming {
                incoming.clone()
            } else {
                self.notes.clone()
            }
        } else {
            self.notes.clone()
        };
        let mut markdown = String::new();
        for chunk in &self.markdown {
            match chunk {
                MarkdownChunk::Text(text) => markdown.push_str(text),
                MarkdownChunk::Conflict {
                    local, incoming, ..
                } => {
                    markdown.push_str(if choices[index] { incoming } else { local });
                    index += 1;
                }
            }
        }
        Ok((snapshot, markdown, notes))
    }
}

fn set_path(root: &mut Value, path: &[String], value: Value) {
    if path.is_empty() {
        *root = value;
    } else if path.len() == 1 && path[0] == "$kind_profile" {
        root["kind"] = value["kind"].clone();
        root["profile"] = value["profile"].clone();
    } else if path.len() == 1 && path[0] == "$date" {
        root["date"] = value["date"].clone();
        root["date_is_auto"] = value["date_is_auto"].clone();
    } else if let Some((first, rest)) = path.split_first() {
        if rest.is_empty() {
            root[first] = value;
        } else {
            set_path(&mut root[first], rest, value);
        }
    }
}

fn merge_value(
    base: &Value,
    local: &Value,
    incoming: &Value,
    path: &mut Vec<String>,
    conflicts: &mut Vec<FieldConflict>,
) -> Value {
    if local == incoming {
        return local.clone();
    }
    if local == base {
        return incoming.clone();
    }
    if incoming == base {
        return local.clone();
    }
    if let (Some(b), Some(l), Some(r)) = (base.as_object(), local.as_object(), incoming.as_object())
    {
        let mut merged = serde_json::Map::new();
        if path.is_empty() {
            for (label, keys) in [
                ("$kind_profile", ["kind", "profile"]),
                ("$date", ["date", "date_is_auto"]),
            ] {
                let grouped = |object: &serde_json::Map<String, Value>| {
                    Value::Object(
                        keys.iter()
                            .filter_map(|key| {
                                object
                                    .get(*key)
                                    .map(|value| ((*key).to_string(), value.clone()))
                            })
                            .collect(),
                    )
                };
                let old = grouped(b);
                let left = grouped(l);
                let right = grouped(r);
                let value = if left == right {
                    left.clone()
                } else if left == old {
                    right.clone()
                } else if right == old {
                    left.clone()
                } else {
                    conflicts.push(FieldConflict {
                        path: vec![label.to_string()],
                        local: left.clone(),
                        incoming: right,
                    });
                    left
                };
                for key in keys {
                    if let Some(value) = value.get(key) {
                        merged.insert(key.to_string(), value.clone());
                    }
                }
            }
        }
        for (key, old) in b {
            if path.is_empty()
                && matches!(key.as_str(), "kind" | "profile" | "date" | "date_is_auto")
            {
                continue;
            }
            if let (Some(left), Some(right)) = (l.get(key), r.get(key)) {
                path.push(key.clone());
                merged.insert(key.clone(), merge_value(old, left, right, path, conflicts));
                path.pop();
            }
        }
        return Value::Object(merged);
    }
    conflicts.push(FieldConflict {
        path: path.clone(),
        local: local.clone(),
        incoming: incoming.clone(),
    });
    local.clone()
}

#[derive(Debug, Clone)]
struct Edit {
    old: Range<usize>,
    replacement: String,
}

fn edits(base: &str, side: &str) -> Vec<Edit> {
    let diff = TextDiff::from_lines(base, side);
    let lines = side.split_inclusive('\n').collect::<Vec<_>>();
    diff.ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| {
            let replacement = lines[op.new_range()].concat();
            Edit {
                old: op.old_range(),
                replacement,
            }
        })
        .collect()
}

fn touches(edit: &Edit, start: usize, end: usize) -> bool {
    if start == end {
        edit.old.start == start
    } else if edit.old.start == edit.old.end {
        edit.old.start >= start && edit.old.start <= end
    } else {
        edit.old.start < end && edit.old.end > start
    }
}

fn apply(base: &[&str], start: usize, end: usize, edits: &[Edit]) -> String {
    let mut result = String::new();
    let mut cursor = start;
    for edit in edits {
        result.push_str(&base[cursor..edit.old.start].concat());
        result.push_str(&edit.replacement);
        cursor = edit.old.end;
    }
    result.push_str(&base[cursor..end].concat());
    result
}

pub fn merge_markdown(base: &str, local: &str, incoming: &str) -> Vec<MarkdownChunk> {
    if local == incoming {
        return vec![MarkdownChunk::Text(local.to_string())];
    }
    if local == base {
        return vec![MarkdownChunk::Text(incoming.to_string())];
    }
    if incoming == base {
        return vec![MarkdownChunk::Text(local.to_string())];
    }
    let lines = base.split_inclusive('\n').collect::<Vec<_>>();
    let local_edits = edits(base, local);
    let incoming_edits = edits(base, incoming);
    let (mut li, mut ri, mut cursor) = (0, 0, 0);
    let mut out = Vec::new();
    while li < local_edits.len() || ri < incoming_edits.len() {
        let next = local_edits
            .get(li)
            .map(|e| e.old.start)
            .unwrap_or(usize::MAX)
            .min(
                incoming_edits
                    .get(ri)
                    .map(|e| e.old.start)
                    .unwrap_or(usize::MAX),
            );
        if next > cursor {
            out.push(MarkdownChunk::Text(lines[cursor..next].concat()));
            cursor = next;
        }
        let start = cursor;
        let mut end = start;
        let first_li = li;
        let first_ri = ri;
        loop {
            let before = (li, ri, end);
            while let Some(edit) = local_edits.get(li) {
                if touches(edit, start, end) {
                    end = end.max(edit.old.end);
                    li += 1;
                } else {
                    break;
                }
            }
            while let Some(edit) = incoming_edits.get(ri) {
                if touches(edit, start, end) {
                    end = end.max(edit.old.end);
                    ri += 1;
                } else {
                    break;
                }
            }
            if (li, ri, end) == before {
                break;
            }
        }
        let left = &local_edits[first_li..li];
        let right = &incoming_edits[first_ri..ri];
        let base_chunk = lines[start..end].concat();
        let local_chunk = apply(&lines, start, end, left);
        let incoming_chunk = apply(&lines, start, end, right);
        if left.is_empty() {
            out.push(MarkdownChunk::Text(incoming_chunk));
        } else if right.is_empty() || local_chunk == incoming_chunk {
            out.push(MarkdownChunk::Text(local_chunk));
        } else {
            out.push(MarkdownChunk::Conflict {
                base: base_chunk,
                local: local_chunk,
                incoming: incoming_chunk,
            });
        }
        cursor = end;
    }
    if cursor < lines.len() {
        out.push(MarkdownChunk::Text(lines[cursor..].concat()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_line_edits_merge_without_losing_whitespace() {
        let base = "甲。\n\n乙。\n";
        let local = "甲改。\n\n乙。\n";
        let incoming = "甲。\n\n乙改。\n";
        let chunks = merge_markdown(base, local, incoming);
        assert!(
            !chunks
                .iter()
                .any(|c| matches!(c, MarkdownChunk::Conflict { .. }))
        );
        let text = chunks
            .into_iter()
            .map(|c| match c {
                MarkdownChunk::Text(v) => v,
                _ => unreachable!(),
            })
            .collect::<String>();
        assert_eq!(text, "甲改。\n\n乙改。\n");
    }

    #[test]
    fn same_line_edit_conflicts() {
        let chunks = merge_markdown("甲。\n", "甲改。\n", "甲补。\n");
        assert!(
            chunks
                .iter()
                .any(|c| matches!(c, MarkdownChunk::Conflict { .. }))
        );
    }

    fn numbered_input(document_number: &str, date: &str) -> DraftInput {
        let mut input = DraftInput::default();
        input.profile.document_number = document_number.to_string();
        input.date = date.to_string();
        input
    }

    #[test]
    fn independent_element_changes_merge_without_conflict() {
        let base = numbered_input("1", "2026-01-01");
        let mut local = base.clone();
        local.profile.document_number = "2".into();
        let mut incoming = base.clone();
        incoming.date = "2026-02-02".into();
        let proposal = MergeProposal::build(
            (&base, "正文", ""),
            (&local, "正文", ""),
            (&incoming, "正文", ""),
        )
        .unwrap();
        assert_eq!(proposal.conflict_count(), 0);
        let (merged, markdown, _) = proposal.resolve(&[]).unwrap();
        assert_eq!(merged.profile.document_number, "2");
        assert_eq!(merged.date, "2026-02-02");
        assert_eq!(markdown, "正文");
    }

    #[test]
    fn same_element_changed_differently_conflicts_and_resolves_per_choice() {
        let base = numbered_input("1", "2026-01-01");
        let mut local = base.clone();
        local.profile.document_number = "2".into();
        let mut incoming = base.clone();
        incoming.profile.document_number = "9".into();
        let proposal = MergeProposal::build(
            (&base, "正文", ""),
            (&local, "正文", ""),
            (&incoming, "正文", ""),
        )
        .unwrap();
        assert_eq!(proposal.conflict_count(), 1);
        let (keep_local, _, _) = proposal.resolve(&[false]).unwrap();
        assert_eq!(keep_local.profile.document_number, "2");
        let (take_incoming, _, _) = proposal.resolve(&[true]).unwrap();
        assert_eq!(take_incoming.profile.document_number, "9");
    }

    #[test]
    fn kind_and_profile_are_merged_as_one_group() {
        let base = DraftInput::default();
        let mut local = base.clone();
        local.kind = crate::models::TemplateKind::ResearchReport;
        let mut incoming = base.clone();
        incoming.profile.profile_name = "另一方案".into();
        let proposal = MergeProposal::build(
            (&base, "正文", ""),
            (&local, "正文", ""),
            (&incoming, "正文", ""),
        )
        .unwrap();
        // kind 与 profile 相互依赖，任一侧变化都整组冲突。
        assert_eq!(proposal.field_conflicts.len(), 1);
        assert_eq!(
            proposal.field_conflicts[0].path,
            vec!["$kind_profile".to_string()]
        );
        let (merged, _, _) = proposal.resolve(&[true]).unwrap();
        assert_eq!(merged.kind, incoming.kind);
        assert_eq!(merged.profile, incoming.profile);
    }

    #[test]
    fn date_and_auto_flag_are_merged_as_one_group() {
        let base = DraftInput::default();
        let mut local = base.clone();
        local.date = "2026-03-03".into();
        let mut incoming = base.clone();
        incoming.date_is_auto = !base.date_is_auto;
        let proposal = MergeProposal::build(
            (&base, "正文", ""),
            (&local, "正文", ""),
            (&incoming, "正文", ""),
        )
        .unwrap();
        assert_eq!(proposal.field_conflicts.len(), 1);
        assert_eq!(proposal.field_conflicts[0].path, vec!["$date".to_string()]);
        let (merged, _, _) = proposal.resolve(&[false]).unwrap();
        assert_eq!(merged.date, local.date);
        assert_eq!(merged.date_is_auto, local.date_is_auto);
    }

    #[test]
    fn notes_merge_like_a_field_and_conflict_only_when_both_sides_change() {
        let base = DraftInput::default();
        let local = base.clone();
        let incoming = base.clone();
        let proposal = MergeProposal::build(
            (&base, "正文", "旧备注"),
            (&local, "正文", "本机备注"),
            (&incoming, "正文", "导入备注"),
        )
        .unwrap();
        assert!(proposal.notes_conflict.is_some());
        let (_, _, notes) = proposal.resolve(&[true]).unwrap();
        assert_eq!(notes, "导入备注");

        let incoming_same_notes = (&incoming, "正文", "旧备注");
        let proposal = MergeProposal::build(
            (&base, "正文", "旧备注"),
            (&local, "正文", "本机备注"),
            incoming_same_notes,
        )
        .unwrap();
        assert!(proposal.notes_conflict.is_none());
        let (_, _, notes) = proposal.resolve(&[]).unwrap();
        assert_eq!(notes, "本机备注");
    }

    #[test]
    fn whitespace_and_markup_only_changes_merge_without_conflict() {
        let base = "甲。**乙**\n\n丙。\n";
        // 本机只动空白（行尾加一个空格），导入侧只动另一行的标记。
        let local = "甲。**乙** \n\n丙。\n";
        let incoming = "甲。**乙**\n\n**丙改。**\n";
        let chunks = merge_markdown(base, local, incoming);
        assert!(
            !chunks
                .iter()
                .any(|c| matches!(c, MarkdownChunk::Conflict { .. }))
        );
        let text = chunks
            .into_iter()
            .map(|c| match c {
                MarkdownChunk::Text(v) => v,
                _ => unreachable!(),
            })
            .collect::<String>();
        assert_eq!(text, "甲。**乙** \n\n**丙改。**\n");
    }

    #[test]
    fn resolve_maps_choices_in_field_notes_markdown_order() {
        let base = DraftInput::default();
        let mut local = base.clone();
        local.profile.document_number = "2".into();
        let mut incoming = base.clone();
        incoming.profile.document_number = "9".into();
        let proposal = MergeProposal::build(
            (&base, "甲。\n", "旧备注"),
            (&local, "甲改。\n", "本机备注"),
            (&incoming, "甲补。\n", "导入备注"),
        )
        .unwrap();
        assert_eq!(proposal.conflict_count(), 3);
        let (snapshot, markdown, notes) = proposal.resolve(&[true, false, true]).unwrap();
        assert_eq!(snapshot.profile.document_number, "9");
        assert_eq!(notes, "本机备注");
        assert_eq!(markdown, "甲补。\n");
    }
}
