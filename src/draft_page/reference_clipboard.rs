//! 应用内复制保留引用结构，对外剪贴板只输出展开后的文字。
//! 富数据仅暂存在当前 egui 上下文中，粘贴文本与上次复制一致时才取用。

use super::{diff_editor, editor_id};
use crate::document_reference::{self, Reference, References};
use eframe::egui;

#[derive(Clone)]
struct Clipboard {
    plain: String,
    raw: String,
    references: Vec<Reference>,
}

pub(crate) struct Paste {
    before: String,
    definitions: Vec<Reference>,
}

pub(crate) fn before_edit(ctx: &egui::Context, text: &str, editable: bool) -> Option<Paste> {
    if !editable || !ctx.memory(|memory| memory.has_focus(editor_id())) {
        return None;
    }
    let clipboard =
        ctx.data(|data| data.get_temp::<Clipboard>(egui::Id::new("reference_clipboard")))?;
    let mut definitions = Vec::new();
    ctx.input_mut(|input| {
        for event in &mut input.events {
            if let egui::Event::Paste(pasted) = event
                && *pasted == clipboard.plain
            {
                let (raw, items) =
                    document_reference::transfer(&clipboard.raw, &clipboard.references, text);
                *pasted = raw;
                definitions = items;
            }
        }
    });
    (!definitions.is_empty()).then(|| Paste {
        before: text.to_string(),
        definitions,
    })
}

pub(crate) fn after_edit(
    ctx: &egui::Context,
    text: &mut String,
    before: &str,
    paste: Option<Paste>,
) {
    if let Some(paste) = paste
        && *text != paste.before
    {
        let cursor = super::editor_cursor(ctx, text).unwrap_or(text.len());
        let mut updated = text.clone();
        for reference in &paste.definitions {
            updated = document_reference::put(&updated, reference);
        }
        if updated != *text {
            // 文本与定义必须在同一个撤销点里，避免撤销粘贴后留下孤立引用。
            *text = paste.before;
            diff_editor::replace_with_undo(ctx, text, updated, cursor);
        }
    }
    let references = References::read(before);
    let mut copied = None;
    let mut copy_command = false;
    ctx.output_mut(|output| {
        for command in &mut output.commands {
            if let egui::OutputCommand::CopyText(raw) = command {
                copy_command = true;
                let ids = document_reference::occurrences(raw);
                let items = ids
                    .iter()
                    .filter_map(|(_, id)| {
                        references
                            .items
                            .get(id)
                            .map(|reference| (id.clone(), reference.clone()))
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_values()
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    let plain = crate::export::plain_text(&references.expanded(raw));
                    copied = Some(Clipboard {
                        plain: plain.clone(),
                        raw: document_reference::without_definitions(raw),
                        references: items,
                    });
                    *raw = plain;
                }
            }
        }
    });
    if let Some(clipboard) = copied {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("reference_clipboard"), clipboard));
    } else if copy_command {
        ctx.data_mut(|data| data.remove::<Clipboard>(egui::Id::new("reference_clipboard")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_reference_clipboard_copy_paste_and_undo_are_self_contained() {
        let ctx = egui::Context::default();
        let reference = Reference::manual("原函", "某函〔2026〕8号", false).unwrap();
        let source = document_reference::put(&reference.token(), &reference);
        let mut copied_text = source.clone();
        let output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            ctx.copy_text(reference.token());
            after_edit(&ctx, &mut copied_text, &source, None);
        });
        assert!(output.platform_output.commands.iter().any(|command|
            matches!(command, egui::OutputCommand::CopyText(text) if text == &reference.display())));
        let mut different = reference.clone();
        different.number = "某函〔2026〕9号".into();
        let mut target = document_reference::put(&different.token(), &different);
        let before = target.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Paste(reference.display())],
                ..Default::default()
            },
            |_ui| {
                ctx.memory_mut(|memory| memory.request_focus(editor_id()));
                let paste = before_edit(&ctx, &target, true).expect("应用内富粘贴");
                ctx.input(|input| {
                    for event in &input.events {
                        if let egui::Event::Paste(text) = event {
                            target.insert_str(0, text);
                        }
                    }
                });
                after_edit(&ctx, &mut target, &before, Some(paste));
            },
        );
        References::check(&target).unwrap();
        let definitions = References::read(&target);
        assert_eq!(definitions.items.len(), 2);
        assert_eq!(definitions.items[&reference.id].number, different.number);
        assert!(
            definitions
                .items
                .values()
                .any(|item| item.id != reference.id && item.number == reference.number)
        );
        let state = egui::TextEdit::load_state(&ctx, editor_id()).unwrap();
        let mut undoer = state.undoer();
        let cursor = state.cursor.char_range().unwrap();
        assert_eq!(undoer.undo(&(cursor, target.clone())).unwrap().1, before);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {
            ctx.copy_text("普通文字".into());
            after_edit(&ctx, &mut target, &before, None);
        });
        assert!(
            ctx.data(|data| data.get_temp::<Clipboard>(egui::Id::new("reference_clipboard")))
                .is_none()
        );
    }
}
