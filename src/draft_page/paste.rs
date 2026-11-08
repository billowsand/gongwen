//! 起草页编辑框的智能粘贴：接管 Ctrl+V，把富文本 / 多行纯文本转成公文体 Markdown。
//!
//! 判定与交接点都在这里（在 `TextEdit` 之前、同一帧内）：
//!
//! - 只在编辑框有焦点、可编辑、不在 AI 工作区视图里时接管；
//! - Ctrl+Shift+V（macOS 为 ⌘⇧V）原样粘贴：egui-winit 把 `command && V` 直接换成
//!   `Event::Paste`、不再发 V 的按键事件，只能看同一帧的 `input.modifiers.shift`
//!   来区分。Windows 的 Shift+Insert 因此也会走原样，这是已知限制；
//! - 内容只有一行且 HTML 里没有图片、或光标前有非空白字符时不接管（复制一个词就
//!   粘贴一个词；粘进带图的一段时哪怕纯文本只有一行也要接管）；
//! - 接管时把 `Event::Paste` 从本帧的事件里删掉，用转换结果替换选区（没有选区就插在
//!   光标处），前后按 `blank_line_padding` 补空行，经 `replace_with_undo` 写回——
//!   只占一个撤销点，Ctrl+Z 一次回到粘贴前。
//!
//! 粘贴是用户的键盘输入，改的是正式稿 `generated_markdown`，不经过任何 AI 代码路径。

use crate::draft_page::diff_editor::replace_with_undo;
use crate::draft_page::{
    DraftPage, blank_line_padding, editor_cursor, editor_id, editor_selection,
};
#[cfg(test)]
use crate::rich_paste::clipboard;
use crate::rich_paste::{self, Context, Source};
use crate::theme;
use eframe::egui;
use std::ops::Range;
use std::sync::mpsc::Receiver;

/// 「改为原样粘贴」按钮保留多久（秒）。再久用户已经按自己的意思改过正文了。
const RECENT_PASTE_SECONDS: f64 = 15.0;

/// 最近一次智能粘贴，用来给「改为原样粘贴」核对与还原。
pub(crate) struct RecentPaste {
    /// 插入内容在正文里的字节范围（不含前后补出来的空行）。
    pub(crate) range: std::ops::Range<usize>,
    /// 插入的是什么，用来核对那段文字还在原处。
    pub(crate) inserted: String,
    /// 剪贴板里的原始纯文本，点「改为原样粘贴」时用它替换。
    pub(crate) plain: String,
    /// 过了这个帧时间（秒）按钮就不画了。
    expires_at: f64,
}

/// 等待后台转换的粘贴。原文先照常插入，转换成功后只替换这段原文。
pub(crate) struct PendingPaste {
    pub(crate) range: Range<usize>,
    pub(crate) plain: String,
    pub(crate) receiver: Receiver<PreparedPaste>,
}

pub(crate) struct PreparedPaste {
    pub(crate) outcome: Option<rich_paste::Outcome>,
    pub(crate) markdown: String,
    pub(crate) line_start: usize,
    pub(crate) block: bool,
}

fn move_editor_cursor(ctx: &egui::Context, text: &str, mut byte: usize) {
    byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let mut state = egui::TextEdit::load_state(ctx, editor_id()).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(text[..byte].chars().count()),
        )));
    state.store(ctx, editor_id());
}

impl DraftPage<'_> {
    /// 在编辑框绘制之前接管本帧的 `Event::Paste`。
    pub(crate) fn smart_paste(&mut self, ui: &egui::Ui, editable: bool) {
        let ctx = ui.ctx();
        // 接管异步粘贴后，这一帧不再让 TextEdit 自己处理输入事件；否则
        // 刚写入的撤销器又会被同一个按键/粘贴事件推进一步。
        if self.doc.pending_paste.is_some() {
            ui.input_mut(|input| input.events.clear());
        }
        if !editable || self.doc.read_only() || self.doc.ai_panel.workspace_view.is_some() {
            return;
        }
        if !ctx.memory(|memory| memory.has_focus(editor_id())) {
            return;
        }
        // Ctrl+Shift+V / ⌘⇧V：交还编辑框原样粘贴纯文本。
        if ui.input(|input| input.modifiers.shift) {
            return;
        }
        let Some((index, text)) = paste_event(ui) else {
            return;
        };
        if self.doc.pending_paste.is_some() {
            return;
        }
        let markdown = &self.doc.generated_markdown;
        let cursor = editor_cursor(ctx, markdown).unwrap_or(markdown.len());
        let selection = editor_selection(ctx, markdown).filter(|range| !range.is_empty());
        // 插入点（有选区时是选区起点）前面只有空白：按块级内容粘，前后补空行。
        // 前面已经有字、或选区从句子中间选起：压成一行行内文字插进去，不能把原来
        // 那句话从中间拆成两段。
        let anchor = selection.as_ref().map_or(cursor, |range| range.start);
        let line_start = markdown[..anchor].rfind('\n').map_or(0, |index| index + 1);
        let block = markdown[line_start..anchor].trim().is_empty();
        let context = Context {
            kind: self.doc.draft.kind,
            draft_empty: markdown.trim().is_empty(),
            has_title: markdown
                .lines()
                .any(|line| line.trim_start().starts_with("# ")),
            section_level: section_level(markdown, anchor),
        };
        // 先原样插入：HTML 剪贴板读取、图片解码与结构转换都在后台做，不让
        // Wayland 事件循环停在这里。结果回来后只替换下面这一段原文。
        ui.input_mut(|input| {
            if index < input.events.len() {
                input.events.remove(index);
            }
        });
        let range = selection.clone().unwrap_or(cursor..cursor);
        let mut updated = String::with_capacity(markdown.len() + text.len());
        updated.push_str(&markdown[..range.start]);
        updated.push_str(&text);
        updated.push_str(&markdown[range.end..]);
        let start = range.start;
        let end = start + text.len();
        replace_with_undo(ctx, &mut self.doc.generated_markdown, updated, end);
        self.doc.pending_source_jump = Some(end);
        self.revalidate();

        let (sender, receiver) = std::sync::mpsc::channel();
        self.doc.pending_paste = Some(PendingPaste {
            range: start..end,
            plain: text.clone(),
            receiver,
        });
        let key = self.doc.key;
        let tx = self.sender.clone();
        let wake = ctx.clone();
        #[cfg(test)]
        let test_html = clipboard::test_html().flatten();
        #[cfg(test)]
        let worker_config_dir = crate::storage::config_dir().ok();
        std::thread::spawn(move || {
            #[cfg(test)]
            clipboard::set_test_html_in_current_thread(test_html);
            #[cfg(test)]
            if let Some(dir) = worker_config_dir {
                crate::storage::set_test_config_dir_in_current_thread(dir);
            }
            // 剪贴板是进程外的数据源：它可能等待另一个应用提供 HTML，绝不能
            // 在 UI 线程里等。失败时保持已经插入的纯文本，不覆盖用户的输入。
            let html = rich_paste::read_html();
            let single_line = !text.trim_end_matches(['\r', '\n']).contains('\n');
            let source = match html {
                Some(html) if !single_line || rich_paste::has_importable_image(&html) => {
                    Source::Html {
                        html,
                        plain: text.clone(),
                    }
                }
                _ => Source::Plain(text.clone()),
            };
            let outcome = if block {
                rich_paste::convert(&source, &context)
            } else {
                rich_paste::convert_inline(&source, &context)
            };
            let _ = sender.send(PreparedPaste {
                outcome,
                markdown: text,
                line_start,
                block,
            });
            let _ = tx.send(crate::app::WorkerResult::SmartPasteDone { key });
            wake.request_repaint();
        });
    }

    /// 收取一次后台粘贴。`None` 表示无需转换，保留已经插入的原文。
    pub(crate) fn finish_paste(
        &mut self,
        ui: &egui::Ui,
        pasted_range: Range<usize>,
        prepared: PreparedPaste,
    ) {
        let Some(outcome) = prepared.outcome else {
            return;
        };
        let ctx = ui.ctx();
        let markdown = &self.doc.generated_markdown;
        if prepared.block {
            // 插入点所在行行首的空白一起换掉，免得留下两个缩进空格。
            let range = prepared.line_start.min(pasted_range.start)..pasted_range.end;
            let (start, end) = (range.start, range.end);
            let lead = blank_line_padding(markdown[..start].trim_end_matches(' '), true);
            let rest = markdown[end..].trim_start_matches(' ');
            let tail = if rest.trim_end_matches('\n').is_empty() {
                ""
            } else {
                blank_line_padding(rest, false)
            };
            let body = outcome.markdown.trim_end_matches('\n');
            let inserted = format!("{lead}{body}{tail}");
            let mut updated = String::with_capacity(markdown.len() + inserted.len());
            updated.push_str(&markdown[..start]);
            updated.push_str(&inserted);
            updated.push_str(&markdown[end..]);
            let markdown_start = start + lead.len();
            let markdown_end = markdown_start + body.len();
            // 原文插入时已经记好「粘贴前 → 原文」的撤销点；这里只推进
            // 当前正文，不能再把临时原文变成额外的一步撤销。
            self.doc.generated_markdown = updated;
            move_editor_cursor(ctx, &self.doc.generated_markdown, markdown_end);
            self.doc.pending_source_jump = Some(markdown_end);
            self.doc.recent_paste = Some(RecentPaste {
                range: markdown_start..markdown_end,
                inserted: body.to_string(),
                plain: prepared.markdown,
                expires_at: ctx.input(|input| input.time) + RECENT_PASTE_SECONDS,
            });
            self.revalidate();
            *self.status = outcome.stats.notice();
        } else {
            let range = pasted_range;
            let body = outcome.markdown.as_str();
            let mut updated = String::with_capacity(markdown.len() + body.len());
            updated.push_str(&markdown[..range.start]);
            updated.push_str(body);
            updated.push_str(&markdown[range.end..]);
            let start = range.start;
            let end = start + body.len();
            self.doc.generated_markdown = updated;
            move_editor_cursor(ctx, &self.doc.generated_markdown, end);
            self.doc.pending_source_jump = Some(end);
            self.doc.recent_paste = Some(RecentPaste {
                range: start..end,
                inserted: body.to_string(),
                plain: prepared.markdown,
                expires_at: ctx.input(|input| input.time) + RECENT_PASTE_SECONDS,
            });
            self.revalidate();
            *self.status = outcome.stats.inline_notice();
        }
        if !self.doc.warnings.is_empty() || !self.doc.revisions.is_empty() {
            self.open_result_drawer();
        }
    }

    /// 编辑区上方那一行「改为原样粘贴」。按钮在下一次改动那段文字、换稿或超时后消失。
    pub(crate) fn recent_paste_strip(&mut self, ui: &mut egui::Ui) {
        if self.doc.read_only() {
            return;
        }
        let Some(paste) = self.doc.recent_paste.as_ref() else {
            return;
        };
        let now = ui.input(|input| input.time);
        // 那段文字被改过、换稿或超时：按钮就失效了，直接清掉。
        if now >= paste.expires_at
            || self.doc.generated_markdown.get(paste.range.clone()) != Some(paste.inserted.as_str())
        {
            self.doc.recent_paste = None;
            return;
        }
        let mut revert = false;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("已按格式转换")
                    .small()
                    .color(theme::text_muted()),
            );
            if ui
                .small_button("改为原样粘贴")
                .on_hover_text("用剪贴板里复制的原始文字替换刚才插入的内容")
                .clicked()
            {
                revert = true;
            }
        });
        if revert {
            self.revert_recent_paste(ui.ctx());
        }
    }

    /// 「改为原样粘贴」：核对那段文字还在原处，再用原始纯文本替换。
    fn revert_recent_paste(&mut self, ctx: &egui::Context) {
        let Some(paste) = self.doc.recent_paste.take() else {
            return;
        };
        let markdown = &self.doc.generated_markdown;
        if markdown.get(paste.range.clone()) != Some(paste.inserted.as_str()) {
            *self.status = "粘贴的内容已改动，无法还原。".into();
            return;
        }
        let mut updated = String::with_capacity(markdown.len());
        updated.push_str(&markdown[..paste.range.start]);
        updated.push_str(&paste.plain);
        updated.push_str(&markdown[paste.range.end..]);
        let cursor = paste.range.start + paste.plain.len();
        replace_with_undo(ctx, &mut self.doc.generated_markdown, updated, cursor);
        self.doc.pending_source_jump = Some(cursor);
        self.revalidate();
        *self.status = "已改为原样粘贴。".into();
    }
}

/// 插入点上方最近一个标题的级数（`# ` = 1）。附件区的附件标题同样是 `# `，
/// 粘进附件正文时以它为上下文。
fn section_level(markdown: &str, at: usize) -> Option<u8> {
    let head = &markdown[..at.min(markdown.len())];
    for line in head.lines().rev() {
        let trimmed = line.trim_start();
        let hashes = trimmed.bytes().take_while(|&byte| byte == b'#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            return Some(hashes as u8);
        }
    }
    None
}

/// 本帧第一个 `Event::Paste` 的下标与内容。
fn paste_event(ui: &egui::Ui) -> Option<(usize, String)> {
    ui.input(|input| {
        input.events.iter().enumerate().find_map(|(index, event)| {
            if let egui::Event::Paste(text) = event {
                Some((index, text.clone()))
            } else {
                None
            }
        })
    })
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use crate::app::{DraftAction, VersionSwitchPrompt, WorkerResult};
    use crate::draft_page::{DraftSession, ExportLinks};
    use crate::models::AppConfig;
    use crate::rich_paste::clipboard;
    use std::sync::mpsc::{Receiver, Sender};

    /// 多行纯文本：微信、记事本里复制出来的样子。
    const PLAIN: &str = "一、总体要求\n（一）指导思想\n各单位要压实责任，确保各项措施落地见效。\n";

    struct Harness {
        ctx: egui::Context,
        doc: DraftSession,
        config: AppConfig,
        sender: Sender<WorkerResult>,
        status: String,
        version_switch: Option<VersionSwitchPrompt>,
        revert_confirm: Option<(i64, i64)>,
        actions: Vec<DraftAction>,
        export_links: ExportLinks,
        metrics: crate::metrics::Metrics,
        _keep: Receiver<WorkerResult>,
    }

    impl Harness {
        fn new(markdown: &str) -> Self {
            let ctx = egui::Context::default();
            theme::configure_icons(&ctx);
            theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
            let config = AppConfig::default();
            let doc = DraftSession::with_markdown(1, &config, markdown.to_string());
            let (sender, _keep) = std::sync::mpsc::channel();
            Self {
                ctx,
                doc,
                config,
                sender,
                status: String::new(),
                version_switch: None,
                revert_confirm: None,
                actions: Vec::new(),
                export_links: ExportLinks::default(),
                metrics: crate::metrics::Metrics::default(),
                _keep,
            }
        }

        fn with_page<R>(&mut self, f: impl FnOnce(&mut DraftPage<'_>) -> R) -> R {
            let mut page = DraftPage {
                doc: &mut self.doc,
                config: &mut self.config,
                store: None,
                sender: &self.sender,
                status: &mut self.status,
                version_switch: &mut self.version_switch,
                revert_confirm: &mut self.revert_confirm,
                actions: &mut self.actions,
                export_links: &mut self.export_links,
                metrics: &mut self.metrics,
            };
            f(&mut page)
        }

        /// 画一帧编辑框，返回本帧画出来的所有文字。
        fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> Vec<String> {
            // 编辑框的识别号在整份应用里只有一个；每帧重新要一次焦点，
            // 免得某次点击把它挪走。
            self.ctx
                .memory_mut(|memory| memory.request_focus(editor_id()));
            // egui 0.36 的 `RawInput` 没有修饰键字段，修饰键状态由事件带进来。
            let mut events = events;
            events.insert(0, egui::Event::ModifiersChanged(modifiers));
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            };
            let ctx = self.ctx.clone();
            let output = crate::test_support::run_ui(&ctx, raw, |ui| {
                self.with_page(|page| page.markdown_editor(ui));
            });
            let mut texts = Vec::new();
            fn collect(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_string()),
                    egui::epaint::Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect(shape, out);
                        }
                    }
                    _ => {}
                }
            }
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut texts);
            }
            texts
        }

        fn paste(&mut self, text: &str) -> Vec<String> {
            self.frame(
                vec![egui::Event::Paste(text.to_string())],
                egui::Modifiers::NONE,
            );
            self.settle_paste()
        }

        fn settle_paste(&mut self) -> Vec<String> {
            let Some(pending) = self.doc.pending_paste.take() else {
                return self.frame(Vec::new(), egui::Modifiers::NONE);
            };
            let prepared = pending
                .receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("后台粘贴应及时完成");
            if self.doc.generated_markdown.get(pending.range.clone())
                == Some(pending.plain.as_str())
            {
                let raw = egui::RawInput::default();
                let ctx = self.ctx.clone();
                let mut prepared = Some(prepared);
                crate::test_support::run_ui(&ctx, raw, |ui| {
                    self.with_page(|page| {
                        page.finish_paste(ui, pending.range.clone(), prepared.take().unwrap())
                    });
                });
            }
            self.frame(Vec::new(), egui::Modifiers::NONE)
        }

        /// 把编辑框的光标放到第 `char_index` 个字符（0 起）。
        fn set_cursor(&mut self, char_index: usize) {
            let mut state = egui::TextEdit::load_state(&self.ctx, editor_id()).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(char_index),
                )));
            state.store(&self.ctx, editor_id());
        }

        /// 选中第 `start..end` 个字符。
        fn select(&mut self, start: usize, end: usize) {
            let mut state = egui::TextEdit::load_state(&self.ctx, editor_id()).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(start),
                    egui::text::CCursor::new(end),
                )));
            state.store(&self.ctx, editor_id());
        }
    }

    /// 空行分隔处插入：前后各留一个空行，别的正文一个字都不动。
    #[test]
    fn paste_between_paragraphs_keeps_the_surrounding_text() {
        clipboard::set_test_html(None);
        let mut harness = Harness::new(
            "某某单位：

请各单位抓好落实。
",
        );
        harness.set_cursor(6); // 空行上
        harness.paste(PLAIN);
        assert_eq!(
            harness.doc.generated_markdown,
            concat!(
                "某某单位：

",
                "## 总体要求

### 指导思想

各单位要压实责任，确保各项措施落地见效。",
                "

请各单位抓好落实。
"
            )
        );

        // 有选区：用转换结果替换选区。
        let mut harness = Harness::new(
            "某某单位：

请各单位抓好落实。
",
        );
        harness.select(7, 16);
        harness.paste(PLAIN);
        assert_eq!(
            harness.doc.generated_markdown,
            concat!(
                "某某单位：

",
                "## 总体要求

### 指导思想

各单位要压实责任，确保各项措施落地见效。
"
            )
        );
        clipboard::clear_test_html();
    }

    #[test]
    fn multi_line_paste_is_converted_once_and_undoes_in_one_step() {
        clipboard::set_test_html(None);
        let mut harness = Harness::new("某某单位：\n\n");
        harness.paste(PLAIN);
        let text = &harness.doc.generated_markdown;
        assert!(text.contains("## 总体要求"), "{text}");
        assert!(text.contains("### 指导思想"), "{text}");
        assert!(
            !text.contains("一、总体要求"),
            "粘贴的纯文本不应再插一遍：{text}"
        );
        assert!(
            harness.status.starts_with("已按格式粘贴"),
            "{}",
            harness.status
        );
        assert!(harness.doc.recent_paste.is_some(), "应当记下这次的粘贴");

        // Ctrl+Z 一次回到粘贴前。
        let undo = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        harness.frame(vec![undo.clone()], egui::Modifiers::COMMAND);
        assert_ne!(harness.doc.generated_markdown, "某某单位：\n\n");
        harness.frame(vec![undo], egui::Modifiers::COMMAND);
        assert_eq!(harness.doc.generated_markdown, "某某单位：\n\n");
        clipboard::clear_test_html();
    }

    #[test]
    fn shift_paste_and_single_line_paste_stay_verbatim() {
        clipboard::set_test_html(None);
        let mut harness = Harness::new("");
        // Shift 按着：交还编辑框，原样插入。
        harness.frame(
            vec![egui::Event::Paste(PLAIN.to_string())],
            egui::Modifiers::SHIFT,
        );
        assert!(harness.doc.generated_markdown.contains("一、总体要求"));
        assert!(!harness.doc.generated_markdown.contains("## 总体要求"));

        // 只有一行：不接管。
        let mut harness = Harness::new("");
        harness.frame(
            vec![egui::Event::Paste("一、总体要求".to_string())],
            egui::Modifiers::NONE,
        );
        harness.frame(Vec::new(), egui::Modifiers::NONE);
        assert_eq!(harness.doc.generated_markdown, "一、总体要求");

        clipboard::clear_test_html();
    }

    #[test]
    fn editing_the_plain_placeholder_keeps_the_users_text() {
        clipboard::set_test_html(None);
        let mut harness = Harness::new("某某单位：\n\n");
        harness.frame(
            vec![egui::Event::Paste(PLAIN.to_string())],
            egui::Modifiers::NONE,
        );
        let pending = harness.doc.pending_paste.take().unwrap();
        harness
            .doc
            .generated_markdown
            .replace_range(pending.range.start..pending.range.start, "用户继续输入");
        pending
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            harness
                .doc
                .generated_markdown
                .starts_with("某某单位：\n\n用户继续输入")
        );
        assert!(!harness.doc.generated_markdown.contains("## 总体要求"));
        clipboard::clear_test_html();
    }

    /// 粘在句子中间：不建结构，压成一行行内文字，原文编号文字留着。
    #[test]
    fn mid_line_paste_becomes_one_line_of_inline_text() {
        clipboard::set_test_html(None);
        let mut harness = Harness::new("某某单位：");
        harness.paste(PLAIN);
        assert_eq!(
            harness.doc.generated_markdown,
            "某某单位：一、总体要求（一）指导思想各单位要压实责任，确保各项措施落地见效。"
        );
        assert!(
            harness.status.starts_with("已按行内格式粘贴"),
            "{}",
            harness.status
        );

        // 选区从句子中间选起：同样压成一行替换选区，不能把那句话拆成两段。
        let mut harness = Harness::new("请各单位抓好落实。");
        harness.select(3, 5);
        harness.paste(PLAIN);
        assert_eq!(
            harness.doc.generated_markdown,
            "请各单一、总体要求（一）指导思想各单位要压实责任，确保各项措施落地见效。好落实。"
        );
        assert!(!harness.doc.generated_markdown.contains("## 总体要求"));

        // 含表格时塞不进一行：交还编辑框原样粘贴。
        let mut harness = Harness::new("某某单位：");
        clipboard::set_test_html(Some(
            "<html><body><p>甲</p><table><tr><th>序号</th><th>事项</th></tr>\
             <tr><td>1</td><td>报送</td></tr></table></body></html>",
        ));
        harness.frame(
            vec![egui::Event::Paste("甲\t序号\t事项".to_string())],
            egui::Modifiers::NONE,
        );
        assert!(
            harness.doc.generated_markdown.contains("甲\t序号\t事项"),
            "{}",
            harness.doc.generated_markdown
        );
        clipboard::clear_test_html();
    }

    /// 粘在 `##` 一节的下面：粘贴内容里的样式标题整体往下平移。
    #[test]
    fn style_headings_shift_under_the_current_section() {
        clipboard::set_test_html(Some(
            "<html><body><h2>背景</h2><p>各单位加强巡查。</p><h3>现状</h3></body></html>",
        ));
        let mut harness = Harness::new("# 关于X的通知\n\n## 一、总体要求\n\n");
        // 光标放在文末空行上，上文最近的标题是 `## 一、总体要求`。
        harness.set_cursor(harness.doc.generated_markdown.chars().count());
        harness.frame(
            vec![egui::Event::Paste(
                "背景\n各单位加强巡查。\n现状".to_string(),
            )],
            egui::Modifiers::NONE,
        );
        harness.settle_paste();
        let text = &harness.doc.generated_markdown;
        assert!(text.contains("### 背景"), "{text}");
        assert!(text.contains("#### 现状"), "{text}");
        clipboard::clear_test_html();
    }

    #[test]
    fn recent_paste_can_be_reverted_to_the_clipboard_text() {
        clipboard::set_test_html(None);
        let mut harness = Harness::new("");
        let texts = harness.paste(PLAIN);
        assert!(
            texts.iter().any(|text| text.contains("改为原样粘贴")),
            "粘贴后应当出现还原按钮：{texts:?}"
        );
        let ctx = harness.ctx.clone();
        harness.with_page(|page| page.revert_recent_paste(&ctx));
        assert_eq!(harness.doc.generated_markdown, PLAIN);
        assert_eq!(harness.status, "已改为原样粘贴。");
        assert!(harness.doc.recent_paste.is_none());

        // 那段文字被改过：按钮不再出现，也不能还原。
        let mut harness = Harness::new("");
        harness.paste(PLAIN);
        harness.doc.generated_markdown = harness
            .doc
            .generated_markdown
            .replace("## 总体要求", "改动过");
        let texts = harness.frame(Vec::new(), egui::Modifiers::NONE);
        assert!(!texts.iter().any(|text| text.contains("改为原样粘贴")));
        assert!(harness.doc.recent_paste.is_none());
        clipboard::clear_test_html();
    }

    /// 单行带图也接管：图片写进临时配置目录的资源库，正文留引用，一次 Ctrl+Z 撤回。
    #[test]
    fn single_line_paste_with_an_image_takes_over_and_stores_the_picture() {
        let dir = std::env::temp_dir().join(format!("gongwen-paste-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::storage::set_test_config_dir(Some(dir.clone()));
        clipboard::set_test_html(Some(
            "<html><body><p>见下图。<img src='data:image/png;base64,iVBORw0KGgo='></p></body></html>",
        ));
        let mut harness = Harness::new("");
        harness.paste("见下图。");
        let text = harness.doc.generated_markdown.clone();
        assert!(text.contains("![](images/"), "{text}");
        assert!(text.contains("见下图。"), "{text}");
        assert!(!text.contains("图片未导入"), "{text}");
        assert!(
            harness.status.contains("导入图片 1 张"),
            "{}",
            harness.status
        );
        let stored: Vec<_> = std::fs::read_dir(dir.join("images"))
            .expect("应把图片写进临时资源库")
            .collect();
        assert_eq!(stored.len(), 1);

        // 一次 Ctrl+Z 回到粘贴前（图片文件留在资源库，这是已知的遗留）。
        let undo = egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        harness.frame(vec![undo.clone()], egui::Modifiers::COMMAND);
        harness.frame(vec![undo], egui::Modifiers::COMMAND);
        assert_eq!(harness.doc.generated_markdown, "");

        clipboard::clear_test_html();
        crate::storage::set_test_config_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 粘在句子中间遇图不接管：行内这条路退回纯文本，不把图塞进句子，也不入库。
    #[test]
    fn mid_line_paste_with_an_image_stays_verbatim() {
        clipboard::set_test_html(Some(
            "<html><body><p>见下图。<img src='data:image/png;base64,iVBORw0KGgo='></p></body></html>",
        ));
        let mut harness = Harness::new("某某单位：");
        harness.paste("见下图。");
        assert_eq!(harness.doc.generated_markdown, "某某单位：见下图。");
        assert!(!harness.doc.generated_markdown.contains("images/"));
        clipboard::clear_test_html();
    }

    /// 只复制一个词，旁边带着网页的跟踪像素：照样原样粘贴，不当块级内容。
    #[test]
    fn single_line_paste_next_to_a_tracking_pixel_stays_verbatim() {
        clipboard::set_test_html(Some(
            r#"<html><body><p>通知<img src="https://stat.example.com/p.gif" width=1 height=1></p></body></html>"#,
        ));
        let mut harness = Harness::new("");
        harness.paste("通知");
        assert_eq!(harness.doc.generated_markdown, "通知");
        clipboard::clear_test_html();
    }
}
