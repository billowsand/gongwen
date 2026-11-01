//! 起草页源码编辑器的按键接管。

use super::{editor_id, editor_selection};
use crate::draft_page::table::source_cell_at;
use eframe::egui;
use std::ops::Range;

/// 处理普通 Enter：仅在可编辑的真实表格单元格内插入 `<br>`。
/// Shift+Enter、组合键、输入法组句/提交事件都留给 TextEdit 与输入法处理。
pub(crate) fn table_enter(ui: &egui::Ui, text: &mut String, editable: bool) {
    let ctx = ui.ctx();
    let focused = ctx.memory(|memory| memory.has_focus(editor_id()));
    let composing = ime_composition_blocks_enter(ctx, focused);
    if !editable || !focused || composing {
        return;
    }
    if !ui.input(|input| {
        input.key_pressed(egui::Key::Enter) && input.modifiers == egui::Modifiers::NONE
    }) {
        return;
    }
    let Some(selection) = editor_selection(ctx, text) else {
        return;
    };
    let Some((updated, cursor)) = table_enter_replacement(text, selection) else {
        return;
    };
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        super::diff_editor::replace_with_undo(ctx, text, updated, cursor);
        ctx.request_repaint();
    }
}

/// 生成单元格内 Enter 的一次编辑结果；选区只在完全落于同一源码单元格时接受。
pub(crate) fn table_enter_replacement(
    text: &str,
    selection: Range<usize>,
) -> Option<(String, usize)> {
    let cell = source_cell_at(text, selection.start)?;
    if selection.start < cell.start
        || selection.end > cell.end
        || selection.end < selection.start
        || !text.is_char_boundary(selection.end)
    {
        return None;
    }
    let mut updated = text.to_owned();
    updated.replace_range(selection.clone(), "<br>");
    Some((updated, selection.start + "<br>".len()))
}

/// egui 0.36 的 IME 组句光标状态不可读；用公开的 Event::Ime 在帧间保存组句状态。
/// 本帧出现提交事件也屏蔽 Enter，防止提交候选的 Enter 同时插入 `<br>`。
fn ime_composition_blocks_enter(ctx: &egui::Context, focused: bool) -> bool {
    let id = egui::Id::new("draft_editor_ime_composing");
    let previous = if focused {
        ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false))
    } else {
        false
    };
    let mut composing = previous;
    let mut ime_event = false;
    ctx.input(|input| {
        for event in &input.events {
            match event {
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    ime_event = true;
                    composing = !text.is_empty();
                }
                egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                    ime_event = true;
                    composing = false;
                }
                _ => {}
            }
        }
    });
    ctx.data_mut(|data| data.insert_temp(id, focused && composing));
    focused && (previous || composing || ime_event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, marker: &str) -> usize {
        text.find(marker).expect("测试标记存在")
    }

    fn frame(
        ctx: &egui::Context,
        text: &mut String,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        ctx.memory_mut(|memory| memory.request_focus(editor_id()));
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
        crate::test_support::run_ui(ctx, raw, |ui| {
            table_enter(ui, text, true);
            egui::TextEdit::multiline(text)
                .id(editor_id())
                .desired_width(500.0)
                .show(ui);
        });
    }

    #[test]
    fn enter_replaces_caret_or_same_cell_selection_once() {
        let source = "| 项目 | 内容 |\n| --- | --- |\n| 甲乙 | 正文 |";
        let cursor = at(source, "乙") + "乙".len();
        let (updated, next) = table_enter_replacement(source, cursor..cursor).unwrap();
        assert_eq!(updated, source.replacen("乙", "乙<br>", 1));
        assert_eq!(&updated[next - 4..next], "<br>");

        let selected = at(source, "甲")..at(source, "甲") + "甲乙".len();
        let (updated, _) = table_enter_replacement(source, selected).unwrap();
        assert!(updated.contains("| <br> |"));
    }

    #[test]
    fn source_cell_locator_ignores_escaped_and_code_pipes() {
        let source = "| 事项 | 说明 |\n| --- | --- |\n| 甲\\|乙 | `x|y` |";
        let escaped = at(source, "乙");
        let escaped_cell = crate::draft_page::table::source_cell_at(source, escaped).unwrap();
        assert!(source[escaped_cell].contains("甲\\|乙"));
        let code_pipe = at(source, "x|y") + 1;
        let code_cell = crate::draft_page::table::source_cell_at(source, code_pipe).unwrap();
        assert!(source[code_cell].contains("`x|y`"));
    }

    #[test]
    fn multiline_header_and_vertical_merge_map_to_real_source_cells() {
        let source = "| 总项 | 子项 |\n|  | 名称 |\n| --- | --- |\n| ^^ | 甲 |";
        let header = at(source, "名称");
        let range = crate::draft_page::table::source_cell_at(source, header).unwrap();
        assert!(source[range].contains("名称"));
        let merged_marker = at(source, "^^");
        assert!(crate::draft_page::table::source_cell_at(source, merged_marker).is_none());
        let anchor = at(source, "总项");
        assert!(crate::draft_page::table::source_cell_at(source, anchor).is_some());
    }

    #[test]
    fn enter_does_not_capture_separator_row_or_table_edges() {
        let source = "前言\n| 项目 | 内容 |\n| --- | --- |\n| 甲 | 正文 |\n后记";
        assert!(crate::draft_page::table::source_cell_at(source, at(source, "---")).is_none());
        assert!(crate::draft_page::table::source_cell_at(source, at(source, "前言")).is_none());
        let trailing_pipe = source.find("|\n后记").unwrap();
        assert!(crate::draft_page::table::source_cell_at(source, trailing_pipe).is_none());
        assert!(table_enter_replacement(source, at(source, "甲")..at(source, "后记")).is_none());
        let code = "```text\n| 项目 | 内容 |\n| --- | --- |\n| 甲 | 正文 |\n```";
        assert!(crate::draft_page::table::source_cell_at(code, at(code, "正文")).is_none());
    }

    #[test]
    fn normal_enter_replaces_in_cell_and_one_undo_restores_source() {
        let ctx = egui::Context::default();
        let source = "| 项目 | 内容 |\n| --- | --- |\n| 甲 | 正文 |";
        let mut text = source.to_owned();
        frame(&ctx, &mut text, Vec::new(), egui::Modifiers::NONE);
        let cursor_byte = at(source, "正文") + "正文".len();
        let char_index = source[..cursor_byte].chars().count();
        let mut state = egui::TextEdit::load_state(&ctx, editor_id()).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(char_index),
            )));
        state.store(&ctx, editor_id());

        frame(
            &ctx,
            &mut text,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
        assert_eq!(text, source.replacen("正文", "正文<br>", 1));

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
        assert_eq!(text, source, "一次撤销应恢复按 Enter 前的全文");
    }

    #[test]
    fn shift_enter_is_not_replaced_with_html_break() {
        let ctx = egui::Context::default();
        let mut text = "| 项目 | 内容 |\n| --- | --- |\n| 甲 | 正文 |".to_owned();
        frame(&ctx, &mut text, Vec::new(), egui::Modifiers::NONE);
        let cursor_byte = at(&text, "正文") + "正文".len();
        let char_index = text[..cursor_byte].chars().count();
        let mut state = egui::TextEdit::load_state(&ctx, editor_id()).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(char_index),
            )));
        state.store(&ctx, editor_id());
        frame(
            &ctx,
            &mut text,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
            egui::Modifiers::SHIFT,
        );
        assert!(!text.contains("<br>"));
        assert!(
            text.contains("正文\n"),
            "Shift+Enter 应由 TextEdit 插入源码换行：{text}"
        );
    }

    #[test]
    fn ime_preedit_and_commit_enter_are_left_to_text_edit() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            events: vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "zhong".into(),
                active_range_chars: None,
            })],
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw, |_| {
            assert!(ime_composition_blocks_enter(&ctx, true));
        });
        let raw = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw, |_| {
            assert!(ime_composition_blocks_enter(&ctx, true));
        });
        let raw = egui::RawInput {
            events: vec![egui::Event::Ime(egui::ImeEvent::Commit("中文".into()))],
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw, |_| {
            assert!(ime_composition_blocks_enter(&ctx, true));
        });
        crate::test_support::run_ui(&ctx, egui::RawInput::default(), |_| {
            assert!(!ime_composition_blocks_enter(&ctx, true));
        });
    }
}
