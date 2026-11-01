//! 空有序列表项与空标题的 Tab / Shift+Tab 层级编辑。
//!

use crate::export::{indent_columns, parse_indented_ordered_item};
use eframe::egui;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Edit {
    Changed { markdown: String, cursor: usize },
    Blocked(&'static str),
}

/// 在当前行处理 Tab / Shift+Tab。普通段落、非空列表项与代码围栏返回 None，
/// 交还 TextEdit 默认行为；空列表项或空标题到达层级边界时返回 Blocked，供界面
/// 消费按键并可显示说明，避免把字面 Tab 写进 Markdown。
pub(crate) fn edit_at_cursor(markdown: &str, cursor: usize, shift: bool) -> Option<Edit> {
    if cursor > markdown.len() || !markdown.is_char_boundary(cursor) {
        return None;
    }
    let ranges = line_ranges(markdown);
    let line_index = line_at_byte(&ranges, cursor);
    if in_fenced_code(markdown, line_index) {
        return None;
    }
    let line_range = ranges.get(line_index)?.clone();
    let line = markdown.get(line_range.clone())?;

    if let Some(edit) = edited_empty_heading(line, shift) {
        if cursor < line_range.start + line.trim_end().len() {
            return None;
        }
        return Some(match edit {
            Ok(new_line) => replace_line(markdown, line_index, &ranges, &new_line),
            Err(message) => Edit::Blocked(message),
        });
    }

    let (indent_bytes, _number, content) = parse_indented_ordered_item(line)?;
    if !content.trim().is_empty() || cursor < line_range.start + line.len() - content.len() {
        return None;
    }
    let group_start = ordered_group_start(markdown, &ranges, line_index);
    if is_inline_list_group(markdown, &ranges, group_start) {
        return None;
    }
    edit_empty_list_item(markdown, &ranges, line_index, indent_bytes, shift)
}

/// 按 Tab 键并应用变更。仅在编辑器有焦点且没有选区时接管。
/// 返回 Some(message) 表示已消费；成功编辑返回空串，边界无效返回说明。
pub(crate) fn handle_tab(
    ui: &egui::Ui,
    markdown: &mut String,
    editable: bool,
) -> Option<&'static str> {
    let ctx = ui.ctx();
    let focused = ctx.memory(|memory| memory.has_focus(crate::draft_page::editor_id()));
    if !editable || !focused || super::editor_keys::ime_composition_blocks_input(ctx, focused) {
        return None;
    }
    if !ui.input(|input| input.key_pressed(egui::Key::Tab)) {
        return None;
    }
    let selection = crate::draft_page::editor_selection(ctx, markdown)?;
    if !selection.is_empty() {
        return None;
    }
    let modifiers = ui.input(|input| input.modifiers);
    if modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
        return None;
    }
    let shift = modifiers.shift;
    let edit = crate::draft_page::editor_cursor(ctx, markdown)
        .and_then(|cursor| edit_at_cursor(markdown, cursor, shift))?;
    let key_modifiers = if shift {
        egui::Modifiers::SHIFT
    } else {
        egui::Modifiers::NONE
    };
    if !ui.input_mut(|input| input.consume_key(key_modifiers, egui::Key::Tab)) {
        return None;
    }
    match edit {
        Edit::Changed {
            markdown: updated,
            cursor,
        } => {
            crate::draft_page::diff_editor::replace_with_undo(ctx, markdown, updated, cursor);
            Some("")
        }
        Edit::Blocked(message) => Some(message),
    }
}

fn edit_empty_list_item(
    markdown: &str,
    ranges: &[Range<usize>],
    line_index: usize,
    indent_bytes: usize,
    shift: bool,
) -> Option<Edit> {
    let line_range = ranges.get(line_index)?.clone();
    let line = markdown.get(line_range.clone())?;
    let group_start = ordered_group_start(markdown, ranges, line_index);
    let base_line = markdown.get(ranges.get(group_start)?.clone())?;
    let (base_indent_bytes, _, _) = parse_indented_ordered_item(base_line)?;
    let base_indent = &base_line[..base_indent_bytes];
    let base_columns = indent_columns(base_indent);
    let current_indent_columns = indent_columns(&line[..indent_bytes]);

    if shift {
        if current_indent_columns < base_columns + 2 {
            return Some(Edit::Blocked("已是顶层列表项，不能继续提升层级。"));
        }
        let rest = &line[indent_bytes..];
        let new_line = format!("{base_indent}{rest}");
        Some(replace_list_line_and_renumber(
            markdown, ranges, line_index, new_line,
        ))
    } else {
        if current_indent_columns >= base_columns + 2 {
            return Some(Edit::Blocked("列表仅支持两级，当前项已是子项。"));
        }
        let parent_exists = (group_start..line_index).rev().any(|index| {
            let Some(previous) = markdown.get(ranges[index].clone()) else {
                return false;
            };
            let Some((indent, _, _)) = parse_indented_ordered_item(previous) else {
                return false;
            };
            indent_columns(&previous[..indent]) < base_columns + 2
        });
        if !parent_exists {
            return Some(Edit::Blocked("前面没有同级父项，不能把这一项降为子项。"));
        }
        let rest = &line[indent_bytes..];
        let new_line = format!("{base_indent}  {rest}");
        Some(replace_list_line_and_renumber(
            markdown, ranges, line_index, new_line,
        ))
    }
}

fn replace_list_line_and_renumber(
    markdown: &str,
    ranges: &[Range<usize>],
    line_index: usize,
    new_line: String,
) -> Edit {
    let group_start = ordered_group_start(markdown, ranges, line_index);
    let mut group_end = line_index;
    while group_end + 1 < ranges.len() {
        let next = &markdown[ranges[group_end + 1].clone()];
        if next.trim().is_empty() || parse_indented_ordered_item(next).is_none() {
            break;
        }
        group_end += 1;
    }
    let Some((base_indent_bytes, first_number, _)) =
        parse_indented_ordered_item(&markdown[ranges[group_start].clone()])
    else {
        return Edit::Blocked("无法识别当前列表组。");
    };
    let base_indent_columns =
        indent_columns(&markdown[ranges[group_start].clone()][..base_indent_bytes]);
    let mut root_number = first_number;
    let mut child_number: Option<usize> = None;
    let mut new_parent = false;
    let mut replacements = Vec::new();
    for (index, span) in ranges
        .iter()
        .enumerate()
        .take(group_end + 1)
        .skip(group_start)
    {
        let original = &markdown[span.clone()];
        let mut row = if index == line_index {
            new_line.clone()
        } else {
            original.to_string()
        };
        let Some((indent_bytes, original_number, _)) = parse_indented_ordered_item(&row) else {
            return Edit::Blocked("无法识别列表项编号。");
        };
        let indent = indent_columns(&row[..indent_bytes]);
        let assigned = if indent < base_indent_columns + 2 {
            let number = root_number;
            let Some(next) = root_number.checked_add(1) else {
                return Edit::Blocked("列表编号过大，无法继续编号。");
            };
            root_number = next;
            child_number = None;
            new_parent = index == line_index;
            number
        } else {
            let number = child_number.unwrap_or(if index == line_index || new_parent {
                1
            } else {
                original_number
            });
            let Some(next) = number.checked_add(1) else {
                return Edit::Blocked("列表编号过大，无法继续编号。");
            };
            child_number = Some(next);
            number
        };
        let rest = &row[indent_bytes..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let numbered = format!("{}{assigned}{}", &row[..indent_bytes], &rest[digits..]);
        row = numbered;
        if row != original {
            replacements.push((span.clone(), row));
        }
    }
    if replacements.is_empty() {
        return Edit::Blocked("列表层级没有变化。");
    }
    let mut updated = markdown.to_owned();
    for (span, replacement) in replacements.into_iter().rev() {
        updated.replace_range(span, &replacement);
    }
    let updated_ranges = line_ranges(&updated);
    let Some(updated_line_range) = updated_ranges.get(line_index) else {
        return Edit::Blocked("无法定位修改后的列表项。");
    };
    let Some(updated_line) = updated.get(updated_line_range.clone()) else {
        return Edit::Blocked("无法读取修改后的列表项。");
    };
    // 空项的内容为空，解析器返回的内容后缀位置就是序号及分隔空格之后。
    let cursor = parse_indented_ordered_item(updated_line)
        .map(|(_, _, content)| updated_line_range.start + updated_line.len() - content.len())
        .unwrap_or(updated_line_range.end);
    Edit::Changed {
        markdown: updated,
        cursor,
    }
}

fn ordered_group_start(markdown: &str, ranges: &[Range<usize>], current: usize) -> usize {
    let mut start = current;
    while start > 0 {
        let previous = &markdown[ranges[start - 1].clone()];
        if previous.trim().is_empty() || parse_indented_ordered_item(previous).is_none() {
            break;
        }
        start -= 1;
    }
    start
}

fn is_inline_list_group(markdown: &str, ranges: &[Range<usize>], start: usize) -> bool {
    if start == 0 {
        return false;
    }
    let previous = markdown[ranges[start - 1].clone()].trim();
    if previous.is_empty() || parse_indented_ordered_item(previous).is_some() {
        return false;
    }
    !previous.starts_with(['#', '|', '<', '>', '!', '$', '`'])
}

fn edited_empty_heading(line: &str, shift: bool) -> Option<Result<String, &'static str>> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let heading = &line[indent..];
    let level = heading.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) || !heading[level..].trim().is_empty() {
        return None;
    }
    if level == 1 {
        return Some(Err("主标题不参与标题层级快捷键。"));
    }
    let new_level = if shift {
        level.saturating_sub(1)
    } else {
        level + 1
    };
    if shift && level == 2 {
        return Some(Err("二级标题不能提升为主标题。"));
    }
    if !shift && level >= 5 {
        return Some(Err("标题层级已到五级上限。"));
    }
    let suffix = &heading[level..];
    let new_line = format!(
        "{}{new_hashes}{suffix}",
        &line[..indent],
        new_hashes = "#".repeat(new_level)
    );
    Some(Ok(new_line))
}

fn replace_line(
    markdown: &str,
    line_index: usize,
    ranges: &[Range<usize>],
    new_line: &str,
) -> Edit {
    let mut updated = markdown.to_owned();
    let span = ranges[line_index].clone();
    updated.replace_range(span.clone(), new_line);
    Edit::Changed {
        markdown: updated,
        cursor: span.start + new_line.len(),
    }
}

fn in_fenced_code(markdown: &str, target: usize) -> bool {
    let mut fence: Option<(u8, usize)> = None;
    for line in markdown.split('\n').take(target) {
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        if indent > 3 {
            continue;
        }
        let Some(&marker) = trimmed.as_bytes().first() else {
            continue;
        };
        if !matches!(marker, b'`' | b'~') {
            continue;
        }
        let count = trimmed.bytes().take_while(|byte| *byte == marker).count();
        if count < 3 {
            continue;
        }
        match fence {
            None => fence = Some((marker, count)),
            Some((active, width))
                if marker == active && count >= width && trimmed[count..].trim().is_empty() =>
            {
                fence = None
            }
            _ => {}
        }
    }
    fence.is_some()
}

/// 光标是否位于 Markdown 围栏代码块内，供输入事件按源码位置避开代码内容。
pub(super) fn cursor_is_in_fenced_code(markdown: &str, cursor: usize) -> bool {
    if cursor > markdown.len() || !markdown.is_char_boundary(cursor) {
        return false;
    }
    let ranges = line_ranges(markdown);
    in_fenced_code(markdown, line_at_byte(&ranges, cursor))
}

fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for piece in text.split_inclusive('\n') {
        let line = piece.strip_suffix('\n').unwrap_or(piece);
        let line = line.strip_suffix('\r').unwrap_or(line);
        ranges.push(start..start + line.len());
        start += piece.len();
    }
    if text.ends_with('\n') || ranges.is_empty() {
        ranges.push(text.len()..text.len());
    }
    ranges
}

fn line_at_byte(ranges: &[Range<usize>], byte: usize) -> usize {
    ranges
        .iter()
        .position(|range| byte <= range.end)
        .unwrap_or(ranges.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(text: &str, line: &str) -> usize {
        let mut start = 0;
        for source_line in text.split_inclusive('\n') {
            if source_line.trim_end_matches(['\r', '\n']) == line {
                return start + line.len();
            }
            start += source_line.len();
        }
        panic!("测试行不存在：{line}");
    }

    fn frame(
        ctx: &egui::Context,
        text: &mut String,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> Option<&'static str> {
        let id = crate::draft_page::editor_id();
        ctx.memory_mut(|memory| memory.request_focus(id));
        let mut events = events;
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            events,
            ..Default::default()
        };
        let mut message = None;
        crate::test_support::run_ui(ctx, raw, |ui| {
            message = handle_tab(ui, text, true);
            egui::TextEdit::multiline(text)
                .id(id)
                .desired_width(500.0)
                .show(ui);
        });
        message
    }

    fn place_cursor(ctx: &egui::Context, text: &str, byte: usize) {
        let mut state =
            egui::TextEdit::load_state(ctx, crate::draft_page::editor_id()).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(text[..byte].chars().count()),
            )));
        state.store(ctx, crate::draft_page::editor_id());
    }

    #[test]
    fn tab_demotes_empty_root_item_and_renumbers_both_levels() {
        let source = "1. 甲\n  1. 子项\n2. \n3. 后项";
        let edit = edit_at_cursor(source, cursor(source, "2. "), false).unwrap();
        let Edit::Changed { markdown, .. } = edit else {
            panic!("应当降为子项")
        };
        assert_eq!(markdown, "1. 甲\n  1. 子项\n  2. \n2. 后项");
    }

    #[test]
    fn renumbering_stays_inside_the_current_contiguous_group() {
        let source = "4. 父项\n5. \n6. 后项\n\n8. 独立列表";
        let edit = edit_at_cursor(source, cursor(source, "5. "), false).unwrap();
        let Edit::Changed { markdown, .. } = edit else {
            panic!("应当降为子项")
        };
        assert_eq!(markdown, "4. 父项\n  1. \n5. 后项\n\n8. 独立列表");
    }

    #[test]
    fn shift_tab_promotes_blank_child_and_renumbers_following_items() {
        let source = "1. 父项\n  1. \n  2. 后子项\n2. 后项";
        let edit = edit_at_cursor(source, cursor(source, "  1. "), true).unwrap();
        let Edit::Changed { markdown, .. } = edit else {
            panic!("应当提升为父项")
        };
        assert_eq!(markdown, "1. 父项\n2. \n  1. 后子项\n3. 后项");
    }

    #[test]
    fn only_two_list_levels_are_allowed_and_boundaries_are_consumable() {
        let root = "1. \n2. 后项";
        assert!(
            matches!(
                edit_at_cursor(root, cursor(root, "1. "), false),
                Some(Edit::Blocked(_))
            ),
            "首项没有父项时不能降级"
        );
        let child = "1. 父项\n  1. \n2. 后项";
        assert!(
            matches!(
                edit_at_cursor(child, cursor(child, "  1. "), false),
                Some(Edit::Blocked(_))
            ),
            "已有子项不能再降一级"
        );
        assert!(
            matches!(
                edit_at_cursor(root, cursor(root, "1. "), true),
                Some(Edit::Blocked(_))
            ),
            "顶层不能提升"
        );
        assert!(
            matches!(
                edit_at_cursor("###### ", "###### ".len(), false),
                Some(Edit::Blocked(_))
            ),
            "超出支持范围的标题不再降级"
        );
    }

    #[test]
    fn empty_headings_change_level_but_main_heading_is_out_of_scope() {
        let source = "## \n### \n##### \n# ";
        assert_eq!(
            edit_at_cursor(source, cursor(source, "## "), false),
            Some(Edit::Changed {
                markdown: "### \n### \n##### \n# ".into(),
                cursor: 4
            })
        );
        assert!(
            matches!(edit_at_cursor(source, cursor(source, "### "), true), Some(Edit::Changed { markdown, .. }) if markdown.starts_with("## "))
        );
        assert!(matches!(
            edit_at_cursor(source, cursor(source, "##### "), false),
            Some(Edit::Blocked(_))
        ));
        assert!(matches!(
            edit_at_cursor(source, cursor(source, "# "), false),
            Some(Edit::Blocked(_))
        ));
        assert!(matches!(
            edit_at_cursor("## ", 3, true),
            Some(Edit::Blocked(_))
        ));
    }

    #[test]
    fn nonempty_items_inline_lists_and_fenced_code_are_untouched() {
        let nonempty = "1. 有内容";
        assert!(edit_at_cursor(nonempty, cursor(nonempty, nonempty), false).is_none());
        let inline = "段落\n1. \n2. ";
        assert!(edit_at_cursor(inline, cursor(inline, "1. "), false).is_none());
        let code = "```md\n1. \n```";
        assert!(edit_at_cursor(code, cursor(code, "1. "), false).is_none());
        let ordinary = "普通正文";
        assert!(edit_at_cursor(ordinary, ordinary.len(), false).is_none());
        assert!(
            edit_at_cursor("1. ", 1, false).is_none(),
            "编号内部不接管层级快捷键"
        );
        let huge = format!("{}. 父项\n1. ", usize::MAX);
        assert!(
            edit_at_cursor(&huge, huge.len(), false).is_none(),
            "超出解析器九位上限的数字不是列表父项"
        );
    }

    #[test]
    fn changing_one_parent_preserves_other_children_start_numbers() {
        let source = "1. 甲\n  5. 原子项\n2. 乙\n3. ";
        let Some(Edit::Changed { markdown, .. }) = edit_at_cursor(source, source.len(), false)
        else {
            panic!("应生成子项");
        };
        assert_eq!(markdown, "1. 甲\n  5. 原子项\n2. 乙\n  1. ");
    }

    #[test]
    fn tab_key_demotes_and_one_undo_restores_the_exact_markdown() {
        let ctx = egui::Context::default();
        let source = "1. 父项\n2. \n3. 后项";
        let mut text = source.to_owned();
        frame(&ctx, &mut text, Vec::new(), egui::Modifiers::NONE);
        let byte = cursor(source, "2. ");
        place_cursor(&ctx, source, byte);
        let status = frame(
            &ctx,
            &mut text,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
        assert_eq!(status, Some(""));
        assert_eq!(text, "1. 父项\n  1. \n2. 后项");
        frame(
            &ctx,
            &mut text,
            vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            egui::Modifiers::COMMAND,
        );
        assert_eq!(text, source);
    }

    #[test]
    fn blocked_boundary_consumes_tab_without_inserting_a_literal_tab() {
        let ctx = egui::Context::default();
        let mut text = "1. \n2. 后项".to_owned();
        frame(&ctx, &mut text, Vec::new(), egui::Modifiers::NONE);
        let byte = cursor(&text, "1. ");
        place_cursor(&ctx, &text, byte);
        let message = frame(
            &ctx,
            &mut text,
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
        assert_eq!(message, Some("前面没有同级父项，不能把这一项降为子项。"));
        assert_eq!(text, "1. \n2. 后项");
    }

    #[test]
    fn ime_events_block_tab_interception_across_frames() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            events: vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "zhong".into(),
                active_range_chars: None,
            })],
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw, |_| {
            assert!(super::super::editor_keys::ime_composition_blocks_input(
                &ctx, true
            ));
        });
        crate::test_support::run_ui(&ctx, egui::RawInput::default(), |_| {
            assert!(super::super::editor_keys::ime_composition_blocks_input(
                &ctx, true
            ));
        });
        let raw = egui::RawInput {
            events: vec![egui::Event::Ime(egui::ImeEvent::Commit("中文".into()))],
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw, |_| {
            assert!(super::super::editor_keys::ime_composition_blocks_input(
                &ctx, true
            ));
        });
        crate::test_support::run_ui(&ctx, egui::RawInput::default(), |_| {
            assert!(!super::super::editor_keys::ime_composition_blocks_input(
                &ctx, true
            ));
        });
    }
}
