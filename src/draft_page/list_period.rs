//! 行首数字后的全角点输入转换。

use eframe::egui;
use std::ops::Range;

/// 仅在正文源码编辑器确有焦点、正文可编辑且焦点位置可靠时改写本帧输入载荷。
pub(super) fn normalize_editor_events(ui: &egui::Ui, markdown: &str, editable: bool) {
    let ctx = ui.ctx();
    let focused = ctx.memory(|memory| memory.has_focus(crate::draft_page::editor_id()));
    if !editable || !focused || ui.input(|input| input.pointer.any_down()) {
        return;
    }
    if !ctx.input(|input| input.events.iter().any(contains_target_punctuation)) {
        return;
    }
    let Some(selection) = crate::draft_page::editor_selection(ctx, markdown) else {
        return;
    };
    if !eligible(markdown, &selection, editable, focused) {
        return;
    }
    ctx.input_mut(|input| normalize_events(markdown, selection, &mut input.events));
}

fn eligible(markdown: &str, selection: &Range<usize>, editable: bool, focused: bool) -> bool {
    editable
        && focused
        && selection.end <= markdown.len()
        && selection.start <= selection.end
        && markdown.is_char_boundary(selection.start)
        && markdown.is_char_boundary(selection.end)
        && !crate::draft_page::list_keys::cursor_is_in_fenced_code(markdown, selection.start)
}

fn contains_target_punctuation(event: &egui::Event) -> bool {
    match event {
        egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
            text.chars().any(|ch| matches!(ch, '。' | '．'))
        }
        _ => false,
    }
}

/// 只改输入载荷；模拟当前行前缀，不复制正文，让 TextEdit 管理光标和撤销。
fn normalize_events(markdown: &str, selection: Range<usize>, events: &mut [egui::Event]) {
    let has_commit = events
        .iter()
        .any(|event| matches!(event, egui::Event::Ime(egui::ImeEvent::Commit(_))));
    if events.iter().any(event_makes_cursor_unreliable)
        || (!has_commit
            && events
                .iter()
                .any(|event| matches!(event, egui::Event::Ime(egui::ImeEvent::Preedit { .. }))))
    {
        return;
    }
    let Some(mut prefix) = NumberPrefix::at(markdown, selection.start) else {
        return;
    };
    for event in events {
        match event {
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                let mut result = String::with_capacity(text.len());
                for character in text.chars() {
                    let converted = if matches!(character, '。' | '．')
                        && matches!(prefix, NumberPrefix::Digits(1..=9))
                    {
                        '.'
                    } else {
                        character
                    };
                    result.push(converted);
                    prefix.push(converted);
                }
                *text = result;
            }
            _ => {}
        }
    }
}

/// 只保留缩进/数字位数/其他三种状态，超长行也不需要额外分配。
#[derive(Clone, Copy)]
enum NumberPrefix {
    Indent,
    Digits(usize),
    Other,
}

impl NumberPrefix {
    fn at(markdown: &str, cursor: usize) -> Option<Self> {
        let before = markdown.get(..cursor)?;
        let line = before.rsplit('\n').next().unwrap_or_default();
        let mut prefix = Self::Indent;
        for character in line.chars() {
            prefix.push(character);
        }
        Some(prefix)
    }

    fn push(&mut self, character: char) {
        *self = match (*self, character) {
            (Self::Indent, ' ' | '\t') => Self::Indent,
            (Self::Indent, '0'..='9') => Self::Digits(1),
            (Self::Digits(count), '0'..='9') => Self::Digits((count + 1).min(10)),
            _ => Self::Other,
        };
    }
}

fn event_makes_cursor_unreliable(event: &egui::Event) -> bool {
    match event {
        egui::Event::Paste(_)
        | egui::Event::PointerButton { .. }
        | egui::Event::Ime(egui::ImeEvent::DeleteSurrounding { .. }) => true,
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            modifiers.ctrl
                || modifiers.command
                || modifiers.alt
                || matches!(
                    key,
                    egui::Key::ArrowDown
                        | egui::Key::ArrowLeft
                        | egui::Key::ArrowRight
                        | egui::Key::ArrowUp
                        | egui::Key::Backspace
                        | egui::Key::Delete
                        | egui::Key::End
                        | egui::Key::Enter
                        | egui::Key::Home
                        | egui::Key::PageDown
                        | egui::Key::PageUp
                        | egui::Key::Tab
                )
        }
        egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
            text.contains(['\n', '\r'])
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(markdown: &str, cursor: usize, events: &mut [egui::Event]) {
        normalize_events(markdown, cursor..cursor, events);
    }

    fn text(event: &egui::Event) -> &str {
        match event {
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => text,
            _ => panic!("期望文本事件"),
        }
    }

    #[test]
    fn converts_one_to_nine_digits_at_line_start_with_space_or_tab_indent() {
        for source in ["1", "123456789", "  12", "\t9"] {
            let mut events = vec![egui::Event::Text("。".into())];
            transform(source, source.len(), &mut events);
            assert_eq!(text(&events[0]), ".", "{source:?}");
        }
        let mut fullwidth = vec![egui::Event::Text("．".into())];
        transform("  12", 4, &mut fullwidth);
        assert_eq!(text(&fullwidth[0]), ".");
    }

    #[test]
    fn preserves_body_punctuation_decimal_dates_and_invalid_number_lengths() {
        for source in ["正文1", "2026年11月1日", "1.2", "1234567890"] {
            let mut events = vec![egui::Event::Text("。".into())];
            transform(source, source.len(), &mut events);
            assert_eq!(text(&events[0]), "。", "{source:?}");
        }
        let mut ascii = vec![egui::Event::Text(".".into())];
        transform("1", 1, &mut ascii);
        assert_eq!(text(&ascii[0]), ".");
    }

    #[test]
    fn handles_digit_and_dot_in_same_or_separate_text_and_ime_commit_events() {
        let mut grouped = vec![egui::Event::Text("1。".into())];
        transform("", 0, &mut grouped);
        assert_eq!(text(&grouped[0]), "1.");

        let mut separate = vec![
            egui::Event::Text("1".into()),
            egui::Event::Text("。".into()),
        ];
        transform("", 0, &mut separate);
        assert_eq!(text(&separate[0]), "1");
        assert_eq!(text(&separate[1]), ".");

        let mut commit = vec![egui::Event::Ime(egui::ImeEvent::Commit("1。".into()))];
        transform("", 0, &mut commit);
        assert_eq!(text(&commit[0]), "1.");
    }

    #[test]
    fn preedit_and_commit_in_one_frame_keep_preedit_and_convert_commit() {
        let preedit = egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "。".into(),
            active_range_chars: None,
        });
        let mut events = vec![
            preedit.clone(),
            egui::Event::Ime(egui::ImeEvent::Commit("。".into())),
        ];
        transform("1", 1, &mut events);
        assert_eq!(events[0], preedit);
        assert_eq!(text(&events[1]), ".");

        let mut hovering = vec![
            egui::Event::PointerMoved(egui::pos2(10.0, 10.0)),
            egui::Event::Text("。".into()),
        ];
        transform("1", 1, &mut hovering);
        assert_eq!(text(&hovering[1]), ".", "鼠标悬停不会移动文本光标");
        let mut multiline = vec![egui::Event::Ime(egui::ImeEvent::Commit(
            "```\n1。\n```".into(),
        ))];
        let before = multiline.clone();
        transform("", 0, &mut multiline);
        assert_eq!(multiline, before, "多行提交可能包含新代码围栏，保留原文");
    }

    #[test]
    fn editor_pipeline_converts_system_ime_commit_after_preedit() {
        let ctx = egui::Context::default();
        let id = crate::draft_page::editor_id();
        let mut source = "1".to_string();
        ctx.memory_mut(|memory| memory.request_focus(id));
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        crate::test_support::run_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            },
            |ui| {
                egui::TextEdit::multiline(&mut source).id(id).show(ui);
            },
        );
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(1),
            )));
        state.store(&ctx, id);
        let events = vec![
            egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "。".into(),
                active_range_chars: None,
            }),
            egui::Event::Ime(egui::ImeEvent::Commit("。".into())),
        ];
        crate::test_support::run_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(rect),
                events,
                ..Default::default()
            },
            |ui| {
                normalize_editor_events(ui, &source, true);
                egui::TextEdit::multiline(&mut source).id(id).show(ui);
            },
        );
        assert_eq!(source, "1.");
    }

    #[test]
    fn paste_preedit_and_cursor_moving_frames_are_left_unchanged() {
        for mut events in [
            vec![egui::Event::Paste("。".into())],
            vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "。".into(),
                active_range_chars: None,
            })],
            vec![
                egui::Event::Key {
                    key: egui::Key::ArrowLeft,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("。".into()),
            ],
        ] {
            let before = events.clone();
            transform("1", 1, &mut events);
            assert_eq!(events, before);
        }
    }

    #[test]
    fn selected_text_uses_its_start_and_multiple_events_do_not_reconvert() {
        let mut selected = vec![egui::Event::Text("。".into())];
        normalize_events("1xyz", 1..4, &mut selected);
        assert_eq!(text(&selected[0]), ".");

        let mut repeated = vec![
            egui::Event::Text("1。".into()),
            egui::Event::Text("1。".into()),
        ];
        transform("", 0, &mut repeated);
        assert_eq!(text(&repeated[0]), "1.");
        assert_eq!(text(&repeated[1]), "1。");
    }

    #[test]
    fn one_egui_undo_restores_original_source_after_textedit_handles_transformed_event() {
        let ctx = egui::Context::default();
        let mut source = "1".to_owned();
        let id = crate::draft_page::editor_id();
        ctx.memory_mut(|memory| memory.request_focus(id));
        crate::test_support::run_ui(
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::TextEdit::multiline(&mut source).id(id).show(ui);
            },
        );
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(1),
            )));
        state.store(&ctx, id);
        let mut events = vec![egui::Event::Text("。".into())];
        normalize_events("1", 1..1, &mut events);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 200.0),
            )),
            events,
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw, |ui| {
            egui::TextEdit::multiline(&mut source).id(id).show(ui);
        });
        assert_eq!(source, "1.");
        let undo = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, undo, |ui| {
            egui::TextEdit::multiline(&mut source).id(id).show(ui);
        });
        assert_eq!(source, "1");
    }

    #[test]
    fn editor_gate_requires_focus_editable_text_and_skips_fenced_code() {
        assert!(!eligible("1", &(1..1), true, false));
        assert!(!eligible("1", &(1..1), false, true));
        let code = "```text\n1\n```";
        let code_cursor = "```text\n1".len();
        assert!(!eligible(code, &(code_cursor..code_cursor), true, true));
        assert!(eligible("1", &(1..1), true, true));
        assert!(!eligible("1", &(2..2), true, true));
    }
}
