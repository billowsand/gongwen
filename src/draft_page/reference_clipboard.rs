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
    if references.items.is_empty() {
        return;
    }
    let mut copied = None;
    ctx.output_mut(|output| {
        for command in &mut output.commands {
            if let egui::OutputCommand::CopyText(raw) = command {
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
    }
}
