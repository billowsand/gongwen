//! 工作区内的复制留在内存。外部粘贴和外发各有独立入口。

use super::{GongwenApp, TabRef, WorkerResult};
use crate::draft_page::editor_id;
use eframe::egui;
use zeroize::Zeroizing;

#[derive(Default)]
pub(super) struct Clipboard {
    text: Option<Zeroizing<String>>,
    approved_copy: Option<egui::OutputCommand>,
    pending_paste: Option<PasteTarget>,
    paste_sequence: u64,
}

/// 在 egui 的最终平台输出上隔离复制，覆盖绘制结束后才生成命令的可选中文本。
#[derive(Default)]
struct ClipboardBoundary {
    workspace: bool,
    copy_to_system: bool,
    approved_copy: Option<egui::OutputCommand>,
    captured_copy: Option<CapturedCopy>,
}

struct CapturedCopy {
    commands: Vec<egui::OutputCommand>,
    external: bool,
    pointer_clicked: bool,
}

impl egui::Plugin for ClipboardBoundary {
    fn debug_name(&self) -> &'static str {
        "GongwenClipboard"
    }

    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        let mut copies = Vec::new();
        output
            .platform_output
            .commands
            .retain_mut(|command| match command {
                egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_) => {
                    copies.push(std::mem::replace(
                        command,
                        egui::OutputCommand::CopyText(String::new()),
                    ));
                    false
                }
                egui::OutputCommand::TextSelectionSettled(_) => false,
                _ => true,
            });
        let approved = self.approved_copy.take();
        if !std::mem::take(&mut self.workspace) {
            return;
        }
        let external = std::mem::take(&mut self.copy_to_system);
        if external || !copies.is_empty() {
            self.captured_copy = Some(CapturedCopy {
                commands: copies,
                external,
                pointer_clicked: ctx.input(|input| input.pointer.any_click()),
            });
            ctx.request_repaint();
        }
        if let Some(command) = approved {
            output.platform_output.commands.push(command);
        }
    }
}

struct PasteTarget {
    sequence: u64,
    tab: Option<TabRef>,
    focus: egui::Id,
    cursor: Option<egui::text::CCursorRange>,
    #[cfg(feature = "ai-workbench")]
    material: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    Paste,
    PasteExternal,
    CopyExternal,
}

pub(super) fn configure(ctx: &egui::Context) {
    ctx.add_plugin(ClipboardBoundary::default());
    ctx.with_plugin(|boundary: &mut ClipboardBoundary| *boundary = ClipboardBoundary::default());
    // 对应 vendored egui-winit。快捷键留给应用，系统 PRIMARY 选区始终关闭。
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("gongwen_application_paste"), true));
}

fn shortcut(event: &egui::Event) -> Option<Shortcut> {
    let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.command && !modifiers.alt && modifiers.shift {
        match key {
            egui::Key::V => return Some(Shortcut::PasteExternal),
            egui::Key::C => return Some(Shortcut::CopyExternal),
            _ => {}
        }
    }
    if *key == egui::Key::Paste
        || (*key == egui::Key::V && modifiers.command_only())
        || (cfg!(windows) && *key == egui::Key::Insert && modifiers.shift_only())
    {
        return Some(Shortcut::Paste);
    }
    None
}

pub(crate) fn request_local_paste(ctx: &egui::Context, key: crate::draft_page::DocKey) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("gongwen_local_paste"), key));
    ctx.request_repaint();
}

pub(crate) fn request_system_copy(ctx: &egui::Context, text: String) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("gongwen_system_copy"), text));
    ctx.request_repaint();
}

impl GongwenApp {
    pub(super) fn handle_clipboard_input(&mut self, ctx: &egui::Context) {
        let previous = ctx
            .with_plugin(|boundary: &mut ClipboardBoundary| {
                boundary.workspace = true;
                boundary.captured_copy.take()
            })
            .flatten();
        if let Some(mut captured) = previous {
            if captured.external {
                // 同帧同时点击复制按钮时无法认定哪份内容来自快捷键。
                if captured.commands.len() == 1 && !captured.pointer_clicked {
                    self.request_clipboard_export(captured.commands.pop().unwrap());
                } else {
                    self.status = "请先选中要复制到系统剪贴板的内容。".into();
                }
            } else if let Some(egui::OutputCommand::CopyText(text)) = captured.commands.pop() {
                self.clipboard.text = Some(Zeroizing::new(text));
            }
        }
        if let Some(text) =
            ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::unique("gongwen_system_copy")))
        {
            self.request_clipboard_export(egui::OutputCommand::CopyText(text));
        }
        let mut paste_external = false;
        let mut copy_to_system = false;
        let mut empty = false;
        let paste_from_menu = ctx
            .data_mut(|data| {
                data.remove_temp::<crate::draft_page::DocKey>(egui::Id::unique(
                    "gongwen_local_paste",
                ))
            })
            .is_some_and(|key| {
                self.tabs.get(self.active_tab) == Some(&TabRef::Doc(key))
                    && ctx.memory(|memory| memory.has_focus(editor_id()))
            });
        ctx.input_mut(|input| {
            input.events.retain_mut(|event| match shortcut(event) {
                Some(Shortcut::Paste) => {
                    if let Some(text) = &self.clipboard.text {
                        *event = egui::Event::Paste(text.to_string());
                        true
                    } else {
                        empty = true;
                        false
                    }
                }
                Some(Shortcut::PasteExternal) => {
                    paste_external = true;
                    false
                }
                Some(Shortcut::CopyExternal) => {
                    copy_to_system = true;
                    *event = egui::Event::Copy;
                    true
                }
                None => !matches!(event, egui::Event::MiddleClickPaste { .. }),
            });
            if paste_from_menu {
                if let Some(text) = &self.clipboard.text {
                    input.events.push(egui::Event::Paste(text.to_string()));
                } else {
                    empty = true;
                }
            }
        });
        if copy_to_system {
            ctx.with_plugin(|boundary: &mut ClipboardBoundary| boundary.copy_to_system = true);
        }
        if empty {
            self.status = format!(
                "应用内剪贴板为空。用 {} 从外部粘贴。",
                crate::theme::primary_shortcut("Shift+V")
            );
        }
        if self.clipboard.pending_paste.is_some()
            && ctx.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::PointerButton { pressed: true, .. }
                            | egui::Event::Ime(_)
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Cut
                    )
                })
            })
        {
            self.clipboard.pending_paste = None;
        }
        if paste_external {
            self.start_external_paste(ctx);
        }
    }

    fn start_external_paste(&mut self, ctx: &egui::Context) {
        let Some(focus) = ctx.memory(|memory| memory.focused()) else {
            self.status = "请先将光标放到要粘贴的输入框。".into();
            return;
        };
        if self.showing_doc() && focus == editor_id() {
            self.draft_page().start_external_paste(ctx);
            return;
        }
        let Some(state) = egui::TextEdit::load_state(ctx, focus) else {
            self.status = "请先将光标放到要粘贴的输入框。".into();
            return;
        };
        let cursor = state.cursor.char_range();
        #[cfg(feature = "ai-workbench")]
        let material = self.showing_doc()
            && self
                .docs
                .get(self.active_doc)
                .is_some_and(|doc| focus == crate::ai_panel::composer_input_id(doc.key));
        #[cfg(not(feature = "ai-workbench"))]
        let material = false;
        self.clipboard.paste_sequence += 1;
        let sequence = self.clipboard.paste_sequence;
        self.clipboard.pending_paste = Some(PasteTarget {
            sequence,
            tab: self.tabs.get(self.active_tab).copied(),
            focus,
            cursor,
            #[cfg(feature = "ai-workbench")]
            material,
        });
        let tx = self.sender.clone();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let result = crate::rich_paste::clipboard::read_external()
                .map(|source| crate::rich_paste::external_text(&source, material));
            let _ = tx.send(WorkerResult::ExternalPaste { sequence, result });
            wake.request_repaint();
        });
    }

    pub(super) fn finish_external_paste(
        &mut self,
        ctx: &egui::Context,
        sequence: u64,
        result: Result<String, String>,
    ) {
        if !self
            .clipboard
            .pending_paste
            .as_ref()
            .is_some_and(|target| target.sequence == sequence)
        {
            return;
        }
        let Some(target) = self.clipboard.pending_paste.take() else {
            return;
        };
        let cursor = egui::TextEdit::load_state(ctx, target.focus)
            .and_then(|state| state.cursor.char_range());
        if self.tabs.get(self.active_tab).copied() != target.tab
            || !ctx.memory(|memory| memory.has_focus(target.focus))
            || target.cursor != cursor
            || ctx.input(|input| !input.focused)
        {
            self.status = "粘贴位置已变化，本次外部粘贴已取消。".into();
            return;
        }
        match result {
            Ok(text) if !text.is_empty() => {
                #[cfg(feature = "ai-workbench")]
                if target.material && self.draft_page().add_external_paste_material(&text) {
                    self.status = "已清洗并添加粘贴材料。".into();
                    return;
                }
                ctx.input_mut(|input| input.events.push(egui::Event::Paste(text)));
            }
            Ok(_) => self.status = "清洗后没有可粘贴的文字。".into(),
            Err(error) => self.status = error,
        }
    }

    pub(super) fn request_clipboard_export(&mut self, command: egui::OutputCommand) {
        self.request_output_with_warning(
            "复制内容到系统剪贴板",
            super::output_guard::CLIPBOARD_WARNING,
            move |app| app.clipboard.approved_copy = Some(command),
        );
    }

    pub(super) fn publish_approved_copy(&mut self, ctx: &egui::Context) {
        if let Some(command) = self.clipboard.approved_copy.take() {
            ctx.with_plugin(|boundary: &mut ClipboardBoundary| {
                boundary.approved_copy = Some(command);
            });
            self.status = "已复制到系统剪贴板。".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        }
    }

    #[test]
    fn copy_at_end_of_pass_stays_internal_and_can_be_pasted_next_frame() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = super::super::management_samples::fixture();
        ctx.on_end_pass(
            "late_copy",
            std::sync::Arc::new(|ui| {
                if ui.input(|input| input.events.contains(&egui::Event::Copy)) {
                    ui.copy_text("应用内  原文".into());
                    ui.ctx().send_cmd(egui::OutputCommand::TextSelectionSettled(
                        "系统主选区".into(),
                    ));
                }
            }),
        );
        let copied = crate::test_support::run_ui(
            &ctx,
            egui::RawInput {
                events: vec![egui::Event::Copy],
                ..Default::default()
            },
            |_| app.handle_clipboard_input(&ctx),
        );
        assert!(copied.platform_output.commands.is_empty());
        let raw = egui::RawInput {
            events: vec![key(egui::Key::V, egui::Modifiers::COMMAND)],
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw.clone(), |_| {
            app.handle_clipboard_input(&ctx);
            assert!(ctx.input(|input| {
                input
                    .events
                    .contains(&egui::Event::Paste("应用内  原文".into()))
            }));
        });
        assert!(app.pending_output.is_none());
        // 锁定会重置插件、清掉尚未收取的内容，然后重建整个工作区。
        configure(&ctx);
        let mut app = super::super::management_samples::fixture();
        crate::test_support::run_ui(&ctx, raw, |_| {
            app.handle_clipboard_input(&ctx);
            assert!(ctx.input(|input| {
                input
                    .events
                    .iter()
                    .all(|event| !matches!(event, egui::Event::Paste(_)))
            }));
        });
        assert!(app.status.contains("应用内剪贴板为空"));
    }

    #[test]
    fn explicit_system_copy_requires_confirmation_in_a_plain_workspace() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = super::super::management_samples::fixture();
        let shifted = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        let raw = egui::RawInput {
            events: vec![key(egui::Key::C, shifted)],
            ..Default::default()
        };
        let captured = crate::test_support::run_ui(&ctx, raw, |_| {
            app.handle_clipboard_input(&ctx);
            assert!(ctx.input(|input| input.events.contains(&egui::Event::Copy)));
            ctx.copy_text("明确选中的内容".into());
        });
        assert!(captured.platform_output.commands.is_empty());
        let pending = crate::test_support::run_ui(&ctx, egui::RawInput::default(), |_| {
            app.handle_clipboard_input(&ctx);
        });
        assert!(app.pending_output.is_some());
        assert!(pending.platform_output.commands.is_empty());
        assert!(app.clipboard.text.is_none());
    }

    #[test]
    fn external_shortcuts_have_no_internal_fallback() {
        let shifted = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        assert_eq!(
            shortcut(&key(egui::Key::V, shifted)),
            Some(Shortcut::PasteExternal)
        );
        assert_eq!(
            shortcut(&key(egui::Key::C, shifted)),
            Some(Shortcut::CopyExternal)
        );
        assert_eq!(
            shortcut(&key(egui::Key::V, egui::Modifiers::COMMAND)),
            Some(Shortcut::Paste)
        );
        assert_eq!(
            shortcut(&key(egui::Key::V, shifted | egui::Modifiers::ALT)),
            None
        );
    }
}
