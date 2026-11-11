//! 外部粘贴在后台读取并转换，主线程核对插入位置后一次写入正文。

use crate::draft_page::diff_editor::replace_with_undo;
use crate::draft_page::{
    DraftPage, blank_line_padding, editor_cursor, editor_id, editor_selection,
};
use crate::rich_paste::{self, Context, Source};
use eframe::egui;
use std::ops::Range;
use std::sync::mpsc::Receiver;

pub(crate) struct PendingPaste {
    markdown: String,
    range: Range<usize>,
    kind: crate::models::TemplateKind,
    pub(crate) receiver: Receiver<Result<PreparedPaste, String>>,
}

pub(crate) struct PreparedPaste {
    range: Range<usize>,
    inserted: String,
    cursor: usize,
    notice: String,
}

impl DraftPage<'_> {
    pub(crate) fn start_external_paste(&mut self, ctx: &egui::Context) {
        self.start_paste(ctx, rich_paste::clipboard::read_external);
    }

    fn start_paste(
        &mut self,
        ctx: &egui::Context,
        read: impl FnOnce() -> Result<Source, String> + Send + 'static,
    ) {
        if self.doc.read_only()
            || !self.candidates_available()
            || self.doc.ai_panel.workspace_view.is_some()
        {
            return;
        }
        if self.doc.pending_paste.is_some() {
            *self.status = "正在处理上一次外部粘贴。".into();
            return;
        }
        let markdown = self.doc.generated_markdown.clone();
        let range = paste_range(ctx, &markdown);
        let kind = self.doc.draft.kind;
        let (sender, receiver) = std::sync::mpsc::channel();
        self.doc.pending_paste = Some(PendingPaste {
            markdown: markdown.clone(),
            range: range.clone(),
            kind,
            receiver,
        });
        let key = self.doc.key;
        let tx = self.sender.clone();
        let wake = ctx.clone();
        #[cfg(test)]
        let worker_config_dir = crate::storage::config_dir().ok();
        std::thread::spawn(move || {
            #[cfg(test)]
            if let Some(dir) = worker_config_dir {
                crate::storage::set_test_config_dir_in_current_thread(dir);
            }
            let result = read().and_then(|source| prepare_paste(&markdown, range, kind, source));
            let _ = sender.send(result);
            let _ = tx.send(crate::app::WorkerResult::SmartPasteDone { key });
            wake.request_repaint();
        });
    }

    pub(crate) fn finish_paste(
        &mut self,
        ctx: &egui::Context,
        pending: PendingPaste,
        result: Result<PreparedPaste, String>,
    ) {
        if self.doc.read_only()
            || self.doc.ai_panel.workspace_view.is_some()
            || !self.candidates_available()
            || self.doc.draft.kind != pending.kind
            || self.doc.generated_markdown != pending.markdown
            || !ctx.memory(|memory| memory.has_focus(editor_id()))
            || paste_range(ctx, &self.doc.generated_markdown) != pending.range
        {
            *self.status = "粘贴位置已变化，本次外部粘贴已取消。".into();
            return;
        }
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                *self.status = error;
                return;
            }
        };
        let mut updated = pending.markdown;
        updated.replace_range(prepared.range, &prepared.inserted);
        replace_with_undo(
            ctx,
            &mut self.doc.generated_markdown,
            updated,
            prepared.cursor,
        );
        self.doc.pending_source_jump = Some(prepared.cursor);
        self.revalidate();
        *self.status = prepared.notice;
        if !self.doc.warnings.is_empty() || !self.doc.revisions.is_empty() {
            self.open_result_drawer();
        }
    }
}

fn paste_range(ctx: &egui::Context, markdown: &str) -> Range<usize> {
    editor_selection(ctx, markdown)
        .filter(|range| !range.is_empty())
        .unwrap_or_else(|| {
            let cursor = editor_cursor(ctx, markdown).unwrap_or(markdown.len());
            cursor..cursor
        })
}

fn prepare_paste(
    markdown: &str,
    mut range: Range<usize>,
    kind: crate::models::TemplateKind,
    source: Source,
) -> Result<PreparedPaste, String> {
    let line_start = markdown[..range.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let block = markdown[line_start..range.start].trim().is_empty();
    let context = Context {
        kind,
        draft_empty: markdown.trim().is_empty(),
        has_title: markdown
            .lines()
            .any(|line| line.trim_start().starts_with("# ")),
        section_level: section_level(markdown, range.start),
    };
    let outcome = if block {
        rich_paste::convert(&source, &context)
    } else {
        rich_paste::convert_inline(&source, &context)
    };
    let Some(outcome) = outcome else {
        let inserted = rich_paste::external_text(&source, false);
        if inserted.trim().is_empty() {
            return Err("清洗后没有可粘贴的内容。".into());
        }
        return Ok(PreparedPaste {
            cursor: range.start + inserted.len(),
            range,
            inserted,
            notice: "已清洗并粘贴外部文字。".into(),
        });
    };
    if !block {
        return Ok(PreparedPaste {
            cursor: range.start + outcome.markdown.len(),
            range,
            inserted: outcome.markdown,
            notice: outcome.stats.inline_notice(),
        });
    }
    range.start = line_start;
    let lead = blank_line_padding(markdown[..range.start].trim_end_matches(' '), true);
    let rest = markdown[range.end..].trim_start_matches(' ');
    let tail = if rest.trim_end_matches('\n').is_empty() {
        ""
    } else {
        blank_line_padding(rest, false)
    };
    let body = outcome.markdown.trim_end_matches('\n');
    Ok(PreparedPaste {
        cursor: range.start + lead.len() + body.len(),
        range,
        inserted: format!("{lead}{body}{tail}"),
        notice: outcome.stats.notice(),
    })
}

/// 插入点上方最近一个标题的级数（`# ` = 1）。附件区的附件标题同样作为上下文。
fn section_level(markdown: &str, at: usize) -> Option<u8> {
    for line in markdown[..at].lines().rev() {
        let trimmed = line.trim_start();
        let hashes = trimmed.bytes().take_while(|&byte| byte == b'#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            return Some(hashes as u8);
        }
    }
    None
}

#[cfg(test)]
mod ui_tests {
    use super::*;
    use crate::app::{DraftAction, VersionSwitchPrompt, WorkerResult};
    use crate::draft_page::{DraftSession, ExportLinks};
    use crate::models::AppConfig;
    use crate::theme;
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

        fn start(&mut self, source: Source) {
            let ctx = self.ctx.clone();
            ctx.memory_mut(|memory| memory.request_focus(editor_id()));
            self.with_page(|page| page.start_paste(&ctx, move || Ok(source)));
        }

        fn paste(&mut self, text: &str) {
            self.start(Source::Plain(text.to_string()));
            self.settle_paste();
        }

        fn settle_paste(&mut self) {
            let pending = self.doc.pending_paste.take().unwrap();
            let prepared = pending
                .receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("后台粘贴应及时完成");
            let ctx = self.ctx.clone();
            self.with_page(|page| page.finish_paste(&ctx, pending, prepared));
            self.frame(Vec::new(), egui::Modifiers::NONE);
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
    }

    #[test]
    fn multi_line_paste_is_converted_once_and_undoes_in_one_step() {
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
    }

    #[test]
    fn local_paste_preserves_markdown_and_external_fallback_cleans_text() {
        let mut harness = Harness::new("");
        harness.frame(
            vec![egui::Event::Paste(PLAIN.to_string())],
            egui::Modifiers::NONE,
        );
        assert_eq!(harness.doc.generated_markdown, PLAIN);

        let mut harness = Harness::new("");
        harness.paste("报\u{200b}  送  ");
        assert_eq!(harness.doc.generated_markdown, "报送");
    }

    #[test]
    fn async_paste_does_not_overwrite_edits_at_the_original_target() {
        let mut harness = Harness::new("某某单位：\n\n");
        harness.start(Source::Plain(PLAIN.to_string()));
        assert_eq!(harness.doc.generated_markdown, "某某单位：\n\n");
        harness.doc.generated_markdown.push_str("用户继续输入");
        harness.settle_paste();
        assert_eq!(harness.doc.generated_markdown, "某某单位：\n\n用户继续输入");
        assert!(harness.status.contains("已取消"));
    }

    /// 粘在句子中间：不建结构，压成一行行内文字，原文编号文字留着。
    #[test]
    fn mid_line_paste_becomes_one_line_of_inline_text() {
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

        // 表格塞不进句子时使用经过清洗的纯文本副本。
        let mut harness = Harness::new("某某单位：");
        harness.start(Source::Html {
            html:
                "<table><tr><th>序号</th><th>事项</th></tr><tr><td>1</td><td>报送</td></tr></table>"
                    .into(),
            plain: "序号\t事项".into(),
        });
        harness.settle_paste();
        assert_eq!(harness.doc.generated_markdown, "某某单位：序号事项");
    }

    /// 粘在 `##` 一节的下面：粘贴内容里的样式标题整体往下平移。
    #[test]
    fn style_headings_shift_under_the_current_section() {
        let html = "<html><body><h2>背景</h2><p>各单位加强巡查。</p><h3>现状</h3></body></html>";
        let mut harness = Harness::new("# 关于X的通知\n\n## 一、总体要求\n\n");
        // 光标放在文末空行上，上文最近的标题是 `## 一、总体要求`。
        harness.set_cursor(harness.doc.generated_markdown.chars().count());
        harness.start(Source::Html {
            html: html.into(),
            plain: "背景\n各单位加强巡查。\n现状".into(),
        });
        harness.settle_paste();
        let text = &harness.doc.generated_markdown;
        assert!(text.contains("### 背景"), "{text}");
        assert!(text.contains("#### 现状"), "{text}");
    }

    /// 单行带图也接管：图片写进临时配置目录的资源库，正文留引用，一次 Ctrl+Z 撤回。
    #[test]
    fn single_line_paste_with_an_image_takes_over_and_stores_the_picture() {
        let dir = std::env::temp_dir().join(format!("gongwen-paste-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::storage::set_test_config_dir(Some(dir.clone()));
        let html = "<html><body><p>见下图。<img src='data:image/png;base64,iVBORw0KGgo='></p></body></html>";
        let mut harness = Harness::new("");
        harness.start(Source::Html {
            html: html.into(),
            plain: "见下图。".into(),
        });
        harness.settle_paste();
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
        harness.frame(vec![undo], egui::Modifiers::COMMAND);
        assert_eq!(harness.doc.generated_markdown, "");

        crate::storage::set_test_config_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 粘在句子中间遇图不接管：行内这条路退回纯文本，不把图塞进句子，也不入库。
    #[test]
    fn mid_line_paste_with_an_image_uses_cleaned_text() {
        let html = "<html><body><p>见下图。<img src='data:image/png;base64,iVBORw0KGgo='></p></body></html>";
        let mut harness = Harness::new("某某单位：");
        harness.start(Source::Html {
            html: html.into(),
            plain: "见下图。".into(),
        });
        harness.settle_paste();
        assert_eq!(harness.doc.generated_markdown, "某某单位：见下图。");
        assert!(!harness.doc.generated_markdown.contains("images/"));
    }

    /// 只复制一个词，旁边带着网页的跟踪像素：照样原样粘贴，不当块级内容。
    #[test]
    fn single_line_paste_drops_a_tracking_pixel() {
        let html = r#"<html><body><p>通知<img src="https://stat.example.com/p.gif" width=1 height=1></p></body></html>"#;
        let mut harness = Harness::new("");
        harness.start(Source::Html {
            html: html.into(),
            plain: "通知".into(),
        });
        harness.settle_paste();
        assert_eq!(harness.doc.generated_markdown, "通知");
    }
}
