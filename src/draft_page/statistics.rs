//! 当前稿件的可见中英文计数与后台实际分页；缓存不落盘、不改正文和导出记录。

use crate::export::{self, MarkdownBlock};
use crate::models::{AppConfig, DraftInput, FontConfig, NumberingConfig, VocabularyEntry};
use eframe::egui;
use regex::Regex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(650);
// 多篇稿件切换时也只允许一个统计排版运行，避免与编辑器争 CPU。
static LAYOUT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TextCounts {
    chinese: usize,
    english: usize,
    digits: usize,
}

impl TextCounts {
    fn from_text(text: &str) -> Self {
        static HAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Han}").unwrap());
        static LATIN: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\p{Latin}+(?:[\p{Latin}\p{M}0-9]|['‘’\-][\p{Latin}\p{M}0-9]+)*").unwrap()
        });
        Self {
            chinese: HAN.find_iter(text).count(),
            english: LATIN.find_iter(text).count(),
            digits: text.chars().filter(char::is_ascii_digit).count(),
        }
    }

    fn for_draft(input: &DraftInput, markdown: &str, numbering: &NumberingConfig) -> Self {
        if input.kind.is_research() {
            let expanded = crate::document_reference::References::read(markdown).expanded(markdown);
            // Mermaid 源码不落纸面；图题仍按研究报告的图片图题计数。
            let mut body = expanded.clone();
            for located in export::parse_markdown_located(&expanded).into_iter().rev() {
                if let MarkdownBlock::Diagram { caption, .. } = located.block {
                    let (caption, _) = export::crossref::split_label(&caption);
                    body.replace_range(located.range, &format!("![{caption}](diagram.png)\n"));
                }
            }
            let title = export::research::cover_title(input, &expanded).unwrap_or_default();
            return Self::from_text(&format!(
                "{}\n{}",
                export::plain_text(&title),
                mdx::typst_research::visible_body_text(&body)
            ));
        }
        let blocks = export::parse_markdown_with_numbering(markdown, numbering);
        let mut text = String::new();
        if !blocks
            .iter()
            .any(|block| matches!(block, MarkdownBlock::Title(_)))
        {
            text.push_str(&export::plain_text(&input.title_hint));
            text.push('\n');
        }
        for block in blocks {
            match block {
                MarkdownBlock::Title(value)
                | MarkdownBlock::Heading(_, value)
                | MarkdownBlock::Paragraph(value)
                | MarkdownBlock::OrderedListItem { text: value, .. }
                | MarkdownBlock::Aligned { text: value, .. } => {
                    text.push_str(&export::plain_text(&value));
                }
                MarkdownBlock::Table { rows, spans, .. } => {
                    for (row, cells) in rows.iter().enumerate() {
                        for (column, cell) in cells.iter().enumerate() {
                            if export::table_span_at(&spans, row, column)
                                .is_none_or(|span| span.is_anchor(row, column))
                            {
                                text.push_str(&export::plain_text(cell));
                                text.push('\n');
                            }
                        }
                    }
                }
                MarkdownBlock::Image { .. }
                | MarkdownBlock::Diagram { .. }
                | MarkdownBlock::Marker(_)
                | MarkdownBlock::Html(_)
                | MarkdownBlock::Quote { .. } => {}
            }
            text.push('\n');
        }
        Self::from_text(&text)
    }
}

#[derive(Clone)]
struct Snapshot {
    input: DraftInput,
    markdown: String,
    fonts: FontConfig,
    numbering: NumberingConfig,
    vocabulary: Vec<VocabularyEntry>,
}

impl Snapshot {
    fn matches(&self, input: &DraftInput, markdown: &str, config: &AppConfig) -> bool {
        self.input == *input
            && self.markdown == markdown
            && self.fonts == config.fonts
            && self.numbering == config.numbering
            && self.vocabulary == config.vocabulary
    }

    fn page_count(&self) -> anyhow::Result<usize> {
        if self.input.kind.is_research() {
            export::typst::research::page_count(&self.input, &self.markdown, &self.numbering)
        } else {
            export::typst::page_count(
                &self.input,
                &self.markdown,
                &crate::units::UnitDisplay::new(&self.vocabulary),
                &self.fonts,
                &self.numbering,
            )
        }
    }
}

#[derive(Default)]
enum Pages {
    #[default]
    Pending,
    Ready(usize),
    Failed(String),
}

type PageResult = Result<usize, String>;

/// 每篇打开的稿件单独持有状态；任务序号独立于 AI、保存和导出任务。
#[derive(Default)]
pub(crate) struct DocumentStatistics {
    snapshot: Option<Snapshot>,
    counts: TextCounts,
    pages: Pages,
    ready_at: Option<Instant>,
    generation: Arc<AtomicU64>,
    running: Option<(u64, mpsc::Receiver<PageResult>)>,
}

impl Drop for DocumentStatistics {
    fn drop(&mut self) {
        // 已关闭稿件的排队任务不再开始排版。
        self.generation.store(u64::MAX, Ordering::Relaxed);
    }
}

impl DocumentStatistics {
    fn observe(&mut self, input: &DraftInput, markdown: &str, config: &AppConfig, now: Instant) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.matches(input, markdown, config))
        {
            return;
        }
        self.counts = TextCounts::for_draft(input, markdown, &config.numbering);
        self.snapshot = Some(Snapshot {
            input: input.clone(),
            markdown: markdown.to_owned(),
            fonts: config.fonts.clone(),
            numbering: config.numbering,
            vocabulary: config.vocabulary.clone(),
        });
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.pages = Pages::Pending;
        self.ready_at = Some(now + DEBOUNCE);
    }

    fn poll(&mut self) {
        let Some((generation, receiver)) = &self.running else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("分页任务中断，点击统计重试".into()),
        };
        if *generation == self.generation.load(Ordering::Relaxed) {
            self.pages = match result {
                Ok(pages) => Pages::Ready(pages),
                Err(error) => Pages::Failed(error),
            };
            self.ready_at = None;
        }
        self.running = None;
    }

    /// 返回单行标签和悬停详情。字数只在内容变化时重算，分页等停笔后在后台执行。
    pub(crate) fn update(
        &mut self,
        input: &DraftInput,
        markdown: &str,
        config: &AppConfig,
        ctx: &egui::Context,
    ) -> (String, String) {
        let now = Instant::now();
        self.observe(input, markdown, config, now);
        self.poll();
        if let Some(ready_at) = self.ready_at {
            if now < ready_at {
                ctx.request_repaint_after(ready_at - now);
            } else if self.running.is_none() {
                let snapshot = self.snapshot.clone().expect("已观察到稿件");
                let generation = self.generation.load(Ordering::Relaxed);
                let current = self.generation.clone();
                let ctx = ctx.clone();
                let (sender, receiver) = mpsc::channel();
                match std::thread::Builder::new()
                    .name("稿件分页统计".into())
                    .spawn(move || {
                        let _guard = LAYOUT_LOCK
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if current.load(Ordering::Relaxed) != generation {
                            // 正文又变了或稿件已关闭：省去过期任务，唤醒界面调度新快照。
                            drop(sender);
                            ctx.request_repaint();
                            return;
                        }
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            snapshot.page_count().map_err(|error| format!("{error:#}"))
                        }))
                        .unwrap_or_else(|_| Err("分页任务异常，点击统计重试".into()));
                        let _ = sender.send(result);
                        ctx.request_repaint();
                    }) {
                    Ok(_) => self.running = Some((generation, receiver)),
                    Err(error) => {
                        self.pages = Pages::Failed(format!("无法启动分页任务：{error}"));
                        self.ready_at = None;
                    }
                }
            }
        }
        let pages = match &self.pages {
            Pages::Pending => "页数…".to_owned(),
            Pages::Ready(count) => format!("{} 页", grouped(*count)),
            Pages::Failed(_) => "页数 —".to_owned(),
        };
        let label = format!(
            "中文 {} 字 · 英文 {} 词 · {pages}",
            grouped(self.counts.chinese),
            grouped(self.counts.english)
        );
        let mut tip = format!(
            "统计当前稿件的标题、正文、表格、图题与附件正文。\n中文按汉字计，英文按单词计（连字符和撇号相连算一词）。\n数字 {} 个字符；不计空白、标点、Markdown 标记、图片内文字、自动编号与目录重复文字。\n研究报告公式只计文字说明，不计变量和公式命令。\n页数按单份完整定稿的 Typst 实际排版，含封面、目录和空白页；不含花脸稿与送批随行件。",
            grouped(self.counts.digits)
        );
        match &self.pages {
            Pages::Pending => tip.push_str("\n正在更新页数…"),
            Pages::Failed(error) => {
                tip.push_str(&format!("\n无法计算页数：{error}\n点击统计可重试。"))
            }
            Pages::Ready(_) => {}
        }
        (label, tip)
    }

    pub(crate) fn retry(&mut self) {
        if matches!(self.pages, Pages::Failed(_)) {
            self.pages = Pages::Pending;
            self.ready_at = Some(Instant::now());
        }
    }
}

fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut output = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(ch);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TemplateKind;

    fn counts(markdown: &str, kind: TemplateKind) -> TextCounts {
        TextCounts::for_draft(
            &DraftInput {
                kind,
                ..Default::default()
            },
            markdown,
            &NumberingConfig::default(),
        )
    }

    #[test]
    fn counts_chinese_and_english_words_instead_of_letters_or_punctuation() {
        assert_eq!(
            TextCounts::from_text("中文𠀀〇，Hello world! don't state-of-the-art Café GPT4 2026。"),
            TextCounts {
                chinese: 4,
                english: 6,
                digits: 5
            }
        );
        assert_eq!(
            TextCounts::from_text("  ，。!?\n\t🙂"),
            TextCounts::default()
        );
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(12_345_678), "12,345,678");
    }

    #[test]
    fn official_counts_visible_titles_tables_quotes_and_attachments() {
        let markdown = concat!(
            "# 标题\n\n## 一、要求\n\n**正文** Hello world 2026。\n\n",
            "| 甲 | 乙 |\n| --- | --- |\n| English | words |\n\n",
            "> 引文\n\n![图](images/should-not-count.png)\n\n",
            "<!-- [附件] -->\n# 附件\n\n附件内容。\n",
        );
        assert_eq!(
            counts(markdown, TemplateKind::OfficialLetter),
            TextCounts {
                chinese: 16,
                english: 4,
                digits: 4
            }
        );
        let input = DraftInput {
            title_hint: "表单标题".into(),
            ..Default::default()
        };
        assert_eq!(
            TextCounts::for_draft(&input, "正文", &Default::default()).chinese,
            6
        );
    }

    #[test]
    fn table_cells_do_not_join_english_words_and_merged_cells_count_once() {
        let markdown = "| 甲 | 乙 |\n| --- | --- |\n| hello ||\n| world | ^^ |\n";
        let stats = counts(markdown, TemplateKind::OfficialLetter);
        assert_eq!(stats.chinese, 2);
        assert_eq!(stats.english, 2);
    }

    #[test]
    fn research_counts_rendered_text_without_urls_keys_anchors_or_formula_commands() {
        let markdown = concat!(
            "# 报告\n\n<!-- [目录] -->\n\n## 背景 {#chap:background}\n\n",
            "中**文** [English words](https://should.not.count) [@citation-key] {@chap:background}\n\n",
            "[^note]:(脚注 words)\n\n",
            "$x+\\frac{a}{b}+\\text{为偶数}+\\text{for even}$\n\n",
            "![图题](images/file-name.png){#fig:ignored}\n\n",
            "表：表题 {#tab:ignored}\n| 甲 | 乙 |\n| --- | --- |\n| hello | world |\n\n",
        );
        assert_eq!(
            counts(markdown, TemplateKind::ResearchReport),
            TextCounts {
                chinese: 17,
                english: 7,
                digits: 0
            }
        );
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "封面题名".into(),
            ..Default::default()
        };
        assert_eq!(
            TextCounts::for_draft(&input, "# 原题\n\n正文", &Default::default()).chinese,
            6
        );
    }

    #[test]
    fn research_diagram_source_is_excluded_but_caption_counts() {
        let markdown = "# 报告\n\n```mermaid\nflowchart LR\n A[不算图中文字] --> B[不算]\n```\n图：流程 caption {#fig:flow}\n";
        let stats = counts(markdown, TemplateKind::ResearchReport);
        assert_eq!(stats.chinese, 4);
        assert_eq!(stats.english, 1);
    }

    #[test]
    fn formula_text_preserves_accented_words_without_counting_variables() {
        let stats = counts(
            "$x+\\text{Café 中文}+\\textrm{don't split}+为偶数$",
            TemplateKind::ResearchReport,
        );
        assert_eq!(
            stats,
            TextCounts {
                chinese: 5,
                english: 3,
                digits: 0
            },
            "{}",
            mdx::typst_research::visible_body_text(
                "$x+\\text{Café 中文}+\\textrm{don't split}+为偶数$"
            )
        );
    }

    #[test]
    fn unchanged_content_reuses_cache_and_all_layout_inputs_invalidate_pages() {
        let mut statistics = DocumentStatistics::default();
        let mut input = DraftInput::default();
        let mut config = AppConfig::default();
        let now = Instant::now();
        statistics.observe(&input, "正文", &config, now);
        let generation = statistics.generation.load(Ordering::Relaxed);
        let deadline = statistics.ready_at;
        statistics.pages = Pages::Ready(3);
        statistics.observe(&input, "正文", &config, now + Duration::from_millis(100));
        assert_eq!(statistics.generation.load(Ordering::Relaxed), generation);
        assert_eq!(statistics.ready_at, deadline);
        assert!(matches!(statistics.pages, Pages::Ready(3)));

        input.profile.number_copies = true;
        statistics.observe(&input, "正文", &config, now);
        assert!(matches!(statistics.pages, Pages::Pending));
        config.fonts.use_system_fonts = true;
        statistics.observe(&input, "正文", &config, now);
        config.numbering.heading1 = crate::models::HeadingNumbering::DecimalDot;
        statistics.observe(&input, "正文", &config, now);
        config.vocabulary.push(VocabularyEntry::default());
        statistics.observe(&input, "正文", &config, now);
        statistics.observe(&input, "正文修改", &config, now);
        assert_eq!(
            statistics.generation.load(Ordering::Relaxed),
            generation + 5
        );
        assert_eq!(statistics.ready_at, Some(now + DEBOUNCE));
    }

    #[test]
    fn old_worker_results_cannot_replace_new_content_and_documents_are_isolated() {
        let input = DraftInput::default();
        let config = AppConfig::default();
        let now = Instant::now();
        let mut first = DocumentStatistics::default();
        let mut second = DocumentStatistics::default();
        first.observe(&input, "第一篇", &config, now);
        second.observe(&input, "第二篇", &config, now);
        let generation = first.generation.load(Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        first.running = Some((generation, receiver));
        first.observe(&input, "第一篇改了", &config, now);
        sender.send(Ok(99)).unwrap();
        first.poll();
        assert!(matches!(first.pages, Pages::Pending));
        assert!(first.ready_at.is_some());
        assert!(first.running.is_none());
        assert_eq!(second.counts.chinese, 3);
        assert_eq!(second.generation.load(Ordering::Relaxed), generation);

        let (sender, receiver) = mpsc::channel();
        first.running = Some((first.generation.load(Ordering::Relaxed), receiver));
        sender.send(Ok(4)).unwrap();
        first.poll();
        assert!(matches!(first.pages, Pages::Ready(4)));
        assert!(first.ready_at.is_none());
    }

    #[test]
    fn failed_layout_keeps_word_counts_and_can_be_retried() {
        let mut statistics = DocumentStatistics::default();
        let input = DraftInput::default();
        let config = AppConfig::default();
        let ctx = egui::Context::default();
        statistics.observe(&input, "正文 hello", &config, Instant::now());
        let (sender, receiver) = mpsc::channel();
        statistics.running = Some((statistics.generation.load(Ordering::Relaxed), receiver));
        sender.send(Err("排版失败".into())).unwrap();
        let (label, tip) = statistics.update(&input, "正文 hello", &config, &ctx);
        assert_eq!(label, "中文 2 字 · 英文 1 词 · 页数 —");
        assert!(tip.contains("排版失败"));
        statistics.retry();
        assert!(matches!(statistics.pages, Pages::Pending));
        assert!(statistics.ready_at.is_some());
    }
}
