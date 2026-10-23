//! 起草页编辑框的智能粘贴：接管 Ctrl+V，把富文本 / 多行纯文本转成公文体 Markdown。
//!
//! 判定与交接点都在这里（在 `TextEdit` 之前、同一帧内）：
//!
//! - 只在编辑框有焦点、可编辑、不在 AI 工作区视图里时接管；
//! - Ctrl+Shift+V（macOS 为 ⌘⇧V）原样粘贴：egui-winit 把 `command && V` 直接换成
//!   `Event::Paste`、不再发 V 的按键事件，只能看同一帧的 `input.modifiers.shift`
//!   来区分。Windows 的 Shift+Insert 因此也会走原样，这是已知限制；
//! - 内容只有一行、或光标前有非空白字符时不接管（复制一个词就粘贴一个词）；
//! - 接管时把 `Event::Paste` 从本帧的事件里删掉，用转换结果替换选区（没有选区就插在
//!   光标处），前后按 `blank_line_padding` 补空行，经 `replace_with_undo` 写回——
//!   只占一个撤销点，Ctrl+Z 一次回到粘贴前。
//!
//! 粘贴是用户的键盘输入，改的是正式稿 `generated_markdown`，不经过任何 AI 代码路径。

use crate::draft_page::diff_editor::replace_with_undo;
use crate::draft_page::{
    DraftPage, blank_line_padding, editor_cursor, editor_id, editor_selection,
};
use crate::rich_paste::{self, Context, Source};
use crate::theme;
use eframe::egui;

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

impl DraftPage<'_> {
    /// 在编辑框绘制之前接管本帧的 `Event::Paste`。
    pub(crate) fn smart_paste(&mut self, ui: &egui::Ui, editable: bool) {
        let ctx = ui.ctx();
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
        // 只粘贴一行（复制一个词、一个数字）不接管。
        if !text.trim_end_matches(['\r', '\n']).contains('\n') {
            return;
        }
        let markdown = &self.doc.generated_markdown;
        let cursor = editor_cursor(ctx, markdown).unwrap_or(markdown.len());
        let selection = editor_selection(ctx, markdown).filter(|range| !range.is_empty());
        // 插入点（有选区时是选区起点）前面必须只有空白：插在句子中间要等到第二期
        // （只保留行内格式）。选区从句子中间选起也一样，否则块级内容会把那句话拆开。
        let anchor = selection.as_ref().map_or(cursor, |range| range.start);
        let line_start = markdown[..anchor].rfind('\n').map_or(0, |index| index + 1);
        if !markdown[line_start..anchor].trim().is_empty() {
            return;
        }
        let source = match rich_paste::read_html() {
            Some(html) => Source::Html {
                html,
                plain: text.clone(),
            },
            None => Source::Plain(text.clone()),
        };
        let context = Context {
            research: self.doc.draft.kind.is_research(),
            draft_empty: markdown.trim().is_empty(),
            has_title: markdown
                .lines()
                .any(|line| line.trim_start().starts_with("# ")),
        };
        let Some(outcome) = rich_paste::convert(&source, &context) else {
            return;
        };
        // 接管：把这个事件从本帧删掉，编辑框就不会再插一遍纯文本。
        ui.input_mut(|input| {
            if index < input.events.len() {
                input.events.remove(index);
            }
        });
        // 插入点所在行行首的空白一起换掉，免得留下两个缩进空格。
        let range = line_start..selection.map_or(cursor, |range| range.end);
        let (start, end) = (range.start, range.end);
        let lead = blank_line_padding(markdown[..start].trim_end_matches(' '), true);
        let rest = markdown[end..].trim_start_matches(' ');
        // 后面只剩换行（或什么都不剩）时不必再补空行：那些换行已经够收尾。
        let tail = if rest.trim_end_matches('\n').is_empty() {
            ""
        } else {
            blank_line_padding(rest, false)
        };
        // 转换结果自带收尾换行，这里去掉再补前后空行，免得正文里白多几行。
        let body = outcome.markdown.trim_end_matches('\n');
        let inserted = format!("{lead}{body}{tail}");
        let mut updated = String::with_capacity(markdown.len() + inserted.len());
        updated.push_str(&markdown[..start]);
        updated.push_str(&inserted);
        updated.push_str(&markdown[end..]);
        let markdown_start = start + lead.len();
        let markdown_end = markdown_start + body.len();
        // 光标落在插入内容的末尾，`replace_with_undo` 前后各记一个撤销点。
        replace_with_undo(ctx, &mut self.doc.generated_markdown, updated, markdown_end);
        self.doc.pending_source_jump = Some(markdown_end);
        self.doc.recent_paste = Some(RecentPaste {
            range: markdown_start..markdown_end,
            inserted: body.to_string(),
            plain: source.plain_text().to_string(),
            expires_at: ctx.input(|input| input.time) + RECENT_PASTE_SECONDS,
        });
        self.revalidate();
        *self.status = outcome.stats.notice();
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
            )
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
        harness.paste("一、总体要求");
        assert_eq!(harness.doc.generated_markdown, "一、总体要求");

        // 光标前有非空白字符：不接管。
        let mut harness = Harness::new("某某单位：");
        harness.frame(
            vec![egui::Event::Paste(PLAIN.to_string())],
            egui::Modifiers::NONE,
        );
        assert_eq!(
            harness.doc.generated_markdown,
            "某某单位：一、总体要求\n（一）指导思想\n各单位要压实责任，确保各项措施落地见效。\n"
        );

        // 选区从句子中间选起：同样不接管，不能把那句话拆开。
        let mut harness = Harness::new("请各单位抓好落实。");
        harness.select(3, 5);
        harness.paste(PLAIN);
        assert!(
            !harness.doc.generated_markdown.contains("## 总体要求"),
            "{}",
            harness.doc.generated_markdown
        );
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
}
