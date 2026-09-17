//! 研究报告的纸面预览。
//!
//! 公文那套版式（红头、主送、落款、版记）对研究报告一条都不适用，因此这里不
//! 复用 `official_preview` 的版面，只共用画纸、画行和行号这些底层部件。版心、
//! 字号、行距、标题层级都对齐 mdx 的 `md2tex.cls`：
//!
//! - 版心 `left=28mm, top=37mm, width=156mm`，正文 14bp / 24pt；
//! - `##` 是章（小二黑体居中，编号"第N章"），`###`、`####` 依次是节和小节，
//!   编号 `N.M`、`N.M.K`，由程序生成，正文里不写；
//! - 摘要、版本变更记录、参考文献这些区段由 `<!-- [...] -->` 标记切换，标题
//!   不编号；附录切到字母章号"附录A"。
//!
//! 排版规则一律照 `tex_research_emitter` 抄，不另立一套：预览与编译出的 PDF
//! 对不上，比预览简陋更糟——用户会照着预览改，改出来的 PDF 却是另一个样子。
//! `export::research` 里那条
//! `mdx_turns_the_marks_the_preview_resolves_into_label_ref_cite_and_caption`
//! 真跑一遍 mdx 转换，拿生成的 TeX 核对这里的假设。
//!
//! 字面是近似：预览只加载得到公文那五个字体，方正书宋、方正黑体分别用宋体、
//! 黑体顶替。字号、行距、版心是准的，字形要看编译出来的 PDF——表单底部那句
//! "最终版式以 TeX 编译 PDF 为准"说的就是这件事。

use super::layout::{TextRun, clickable, line_block, line_block_runs, sheet};
use super::render::{PreviewOutput, content_block, image_block};
use super::{
    INDENT_CHARS, Metrics, PreviewScale, RESEARCH_BODY_PT, RESEARCH_CAPTION_PT,
    RESEARCH_CHAPTER_PT, RESEARCH_COVER_PT, RESEARCH_COVER_TITLE_PT, RESEARCH_COVER_TYPE_PT,
    gutter, indent,
};
use crate::export::crossref::{self, ResearchMarks};
use crate::export::{self, LocatedBlock, MarkdownBlock, ResearchSection, parse_research_marker};
use crate::models::DraftInput;
use crate::models::NumberingConfig;
use crate::theme;
use eframe::egui;
use egui::Align;
use regex::Regex;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

/// 一块内容在纸面上的身份。编号在这里一次算好：印它要用，锚点表也要用。
enum Kind<'a> {
    /// 不落在纸上：文件名称、区段标记、已经并进表格的表题。
    Skip,
    /// 普通内容块，交给共用部件照原样画。
    Plain,
    /// 编号章。`heading` 是纸面上的前缀（正文"第 N 章"、附录"附录A"），
    /// `number` 是 `\thechapter` 本身（`1`、`A`）——`{@id}` 引的是后者，
    /// "第"和"章"这两个字由写正文的人自己写。
    Chapter {
        heading: String,
        number: String,
        text: String,
    },
    /// 编号节，`number` 形如 `1.2`、`A.2.1`。
    Section { number: String, text: String },
    /// 不编号的章标题：摘要、版本变更记录、参考文献的首个标题。
    ChapterStar(String),
    /// 不编号的节标题：上面那些区段里后续的标题。
    SectionStar(String),
    /// 摘要里的标题，mdx 排成一段加粗正文。
    AbstractHeading(String),
    /// 摘要开篇：`\begin{abstract}` 自带一个居中的"摘要"章标题。摘要里一块
    /// 内容都没有时 mdx 整个环境都不发，所以这一条挂在首块内容之前，而不是
    /// 挂在区段标记上。
    AbstractOpen,
    /// 有题注的图。没有替代文本的图片不进图号，与 LaTeX 一致（图号由
    /// `\caption` 递增，没有 caption 就不占号）。
    Figure {
        number: Option<String>,
        alt: &'a str,
        src: &'a str,
    },
    /// 表格，连同并进来的表题。
    Table { caption: Option<Caption> },
}

/// 一条并进表格的表题：编号、题名，以及它自己那一行源码的位置。
struct Caption {
    number: String,
    text: String,
    source: Range<usize>,
}

/// 走一遍块序列时的计数器与区段状态，规则全部照 `tex_research_emitter`。
struct Walk {
    section: ResearchSection,
    /// `\appendix` 发过没有。一旦发过，章号就一直是字母——后面再切回正文区段
    /// 也回不去数字，LaTeX 就是这样。
    appendix_started: bool,
    /// 附录段里是否已经用一级标题开过章：是则该段的 `##` 起整体下移一级。
    appendix_saw_h1: bool,
    /// 摘要里的首个标题已经跳过（摘要的标题由 `\begin{abstract}` 自己排）。
    abstract_skipped_heading: bool,
    /// 变更记录 / 参考文献里的首个标题已经排成不编号的章。
    star_heading_done: bool,
    /// 摘要环境已经开过（"摘要"那行标题已经排过）。
    abstract_opened: bool,
    chapter: usize,
    section_no: usize,
    subsection: usize,
    subsubsection: usize,
    figure: usize,
    table: usize,
}

impl Default for Walk {
    fn default() -> Self {
        Self {
            section: ResearchSection::Body,
            appendix_started: false,
            appendix_saw_h1: false,
            abstract_skipped_heading: false,
            star_heading_done: false,
            abstract_opened: false,
            chapter: 0,
            section_no: 0,
            subsection: 0,
            subsubsection: 0,
            figure: 0,
            table: 0,
        }
    }
}

impl Walk {
    /// `\thechapter`：正文是阿拉伯数字，附录是大写字母。
    fn chapter_number(&self) -> String {
        if self.appendix_started {
            let index = u32::from(b'A') + self.chapter.saturating_sub(1) as u32;
            char::from_u32(index).unwrap_or('A').to_string()
        } else {
            self.chapter.to_string()
        }
    }

    /// 开新章：节、图、表的计数器都跟着归零，与 LaTeX 的 `\chapter` 一致。
    /// 返回（纸面前缀，`\thechapter`）。
    fn open_chapter(&mut self) -> (String, String) {
        self.chapter += 1;
        self.section_no = 0;
        self.subsection = 0;
        self.subsubsection = 0;
        self.figure = 0;
        self.table = 0;
        let number = self.chapter_number();
        // ctex 的 `name = {第,章}` 是直接和 `\thechapter` 拼起来的，中间不插空格，
        // 数字两侧一空排出来就比 PDF 宽半个字。
        let heading = if self.appendix_started {
            format!("附录{number}")
        } else {
            format!("第{number}章")
        };
        (heading, number)
    }

    fn open_section(&mut self, level: u8) -> String {
        match level {
            3 => {
                self.section_no += 1;
                self.subsection = 0;
                self.subsubsection = 0;
                format!("{}.{}", self.chapter_number(), self.section_no)
            }
            4 => {
                self.subsection += 1;
                self.subsubsection = 0;
                format!(
                    "{}.{}.{}",
                    self.chapter_number(),
                    self.section_no.max(1),
                    self.subsection
                )
            }
            _ => {
                self.subsubsection += 1;
                format!(
                    "{}.{}.{}.{}",
                    self.chapter_number(),
                    self.section_no.max(1),
                    self.subsection.max(1),
                    self.subsubsection
                )
            }
        }
    }

    /// `\thefigure` / `\thetable`：都是"章号.序号"，跟着章归零。
    fn open_figure(&mut self) -> String {
        self.figure += 1;
        format!("{}.{}", self.chapter_number(), self.figure)
    }

    fn open_table(&mut self) -> String {
        self.table += 1;
        format!("{}.{}", self.chapter_number(), self.table)
    }

    fn enter(&mut self, next: ResearchSection) {
        self.section = next;
        match next {
            ResearchSection::Abstract => {
                self.abstract_skipped_heading = false;
                self.abstract_opened = false;
            }
            ResearchSection::Appendix => {
                // `\appendix` 只发一次；章号从这里开始改用字母，计数重新起算。
                if !self.appendix_started {
                    self.appendix_started = true;
                    self.chapter = 0;
                }
                self.appendix_saw_h1 = false;
            }
            ResearchSection::ChangeLog | ResearchSection::References => {
                self.star_heading_done = false;
            }
            ResearchSection::Body => {}
        }
    }
}

/// 按 mdx research 的规则走一遍块序列，给每一块定身份、算编号。
///
/// 预览要走两遍：第一遍收锚点与文献编号，第二遍才排版（`{@id}` 可以前引后面
/// 的章节，编号只有走完全文才定得下来）。两遍共用这一个函数，规则就不会各写
/// 一份、各错一处。
///
/// `visit` 的第三个参数是这一块定义的锚点 `{#id}`——只有第一遍用得上，第二遍
/// 拿到的源码里锚点已经被换掉了。
fn walk<'a>(
    blocks: &'a [LocatedBlock],
    markdown: &str,
    mut visit: impl FnMut(&'a LocatedBlock, Kind<'a>, Option<&str>),
) {
    let captions = table_captions(blocks);
    let merged: std::collections::HashSet<usize> = captions.values().copied().collect();
    let mut walk = Walk::default();

    for (index, located) in blocks.iter().enumerate() {
        let anchor = anchor_of(markdown, located);
        if merged.contains(&index) {
            visit(located, Kind::Skip, None);
            continue;
        }
        let kind = match &located.block {
            // 区段标记不落在纸上，只改后面各块的身份。公文解析器把
            // `<!-- [正文] -->`、`<!-- [附件] -->` 认成了它自己的区段，落到这里
            // 按原文重判一次，免得附录被当成公文附件。
            MarkdownBlock::Html(text) => {
                if let Some(next) = parse_research_marker(text) {
                    walk.enter(next);
                }
                Kind::Skip
            }
            MarkdownBlock::Marker(_) => {
                if let Some(next) = parse_research_marker(&markdown[located.range.clone()]) {
                    walk.enter(next);
                }
                Kind::Skip
            }
            // 附录段里的 `#` 是附录自己的章标题，与 mdx 一致；正文里的 `#` 是
            // 文件名称，由文档要素维护，不排进版面（校验那边已就此提示）。
            MarkdownBlock::Title(text) if walk.section == ResearchSection::Appendix => {
                heading(&mut walk, 1, text)
            }
            MarkdownBlock::Title(_) => Kind::Skip,
            MarkdownBlock::Heading(level, text) => heading(&mut walk, *level, text),
            MarkdownBlock::Image { alt, src } => Kind::Figure {
                number: (!alt.trim().is_empty()).then(|| walk.open_figure()),
                alt,
                src,
            },
            MarkdownBlock::Table { .. } => {
                let caption = captions.get(&index).map(|caption| {
                    let block = &blocks[*caption];
                    Caption {
                        number: walk.open_table(),
                        text: caption_text(block).unwrap_or_default(),
                        source: block.range.clone(),
                    }
                });
                Kind::Table { caption }
            }
            _ => Kind::Plain,
        };
        if walk.section == ResearchSection::Abstract
            && !walk.abstract_opened
            && !matches!(kind, Kind::Skip)
        {
            walk.abstract_opened = true;
            visit(located, Kind::AbstractOpen, None);
        }
        visit(located, kind, anchor);
    }
}

/// 一个标题在纸面上的身份，规则照 `TexResearchEmitter::emit_heading`。
fn heading(walk: &mut Walk, level: u8, text: &str) -> Kind<'static> {
    let text = text.trim().to_string();
    match walk.section {
        ResearchSection::Abstract => {
            if walk.abstract_skipped_heading {
                Kind::AbstractHeading(text)
            } else {
                // 摘要的标题由 `\begin{abstract}` 自己排，正文里的首个标题跳过，
                // 否则纸上会出现两行"摘要"。
                walk.abstract_skipped_heading = true;
                Kind::Skip
            }
        }
        ResearchSection::ChangeLog | ResearchSection::References => {
            if walk.star_heading_done {
                Kind::SectionStar(text)
            } else {
                walk.star_heading_done = true;
                Kind::ChapterStar(text)
            }
        }
        ResearchSection::Appendix => {
            if level == 1 {
                walk.appendix_saw_h1 = true;
            }
            let shifted = walk.appendix_saw_h1;
            match (level, shifted) {
                (1, _) | (2, false) => chapter(walk, text),
                (2, true) | (3, false) => Kind::Section {
                    number: walk.open_section(3),
                    text,
                },
                (3, true) | (4, false) => Kind::Section {
                    number: walk.open_section(4),
                    text,
                },
                _ => Kind::Section {
                    number: walk.open_section(5),
                    text,
                },
            }
        }
        ResearchSection::Body => match level {
            1 | 2 => chapter(walk, text),
            3..=5 => Kind::Section {
                number: walk.open_section(level),
                text,
            },
            // mdx 忽略六级以下标题。
            _ => Kind::Skip,
        },
    }
}

fn chapter(walk: &mut Walk, text: String) -> Kind<'static> {
    let (heading, number) = walk.open_chapter();
    Kind::Chapter {
        heading,
        number,
        text,
    }
}

/// 这一块定义的锚点 `{#id}`。锚点写在源码行尾，排版前已被剥掉，所以回原文去取。
fn anchor_of<'a>(markdown: &'a str, located: &LocatedBlock) -> Option<&'a str> {
    let raw = markdown.get(located.range.clone())?;
    crossref::split_label(raw).1
}

/// 「表格块下标 → 表题块下标」。表题写在表前或表后都认，表前优先——与 mdx 的
/// `take_leading_table_caption().or(parse_trailing_table_caption())` 一致。
fn table_captions(blocks: &[LocatedBlock]) -> HashMap<usize, usize> {
    let mut pairs = HashMap::new();
    let mut used = std::collections::HashSet::new();
    for (index, located) in blocks.iter().enumerate() {
        if !matches!(located.block, MarkdownBlock::Table { .. }) {
            continue;
        }
        let leading = index
            .checked_sub(1)
            .filter(|before| !used.contains(before))
            .filter(|before| caption_text(&blocks[*before]).is_some());
        let trailing = (index + 1 < blocks.len())
            .then_some(index + 1)
            .filter(|after| !used.contains(after))
            .filter(|after| caption_text(&blocks[*after]).is_some());
        if let Some(caption) = leading.or(trailing) {
            used.insert(caption);
            pairs.insert(index, caption);
        }
    }
    pairs
}

/// 这一块是不是一条表题；是就给出题名。
fn caption_text(located: &LocatedBlock) -> Option<String> {
    let MarkdownBlock::Paragraph(text) = &located.block else {
        return None;
    };
    // 第一遍解析拿到的文字还带着行尾锚点，剥掉再认。
    parse_table_caption(crossref::split_label(text).0)
}

/// 表题行：`表：题名`、`表 1.2 题名`、`Table: 题名`、`：题名`。
/// 编号由 LaTeX 的表格计数器生成，源码里写的旧编号一律剥掉，免得印两遍。
/// 写法与正则都照 mdx 的 `parser::parse_table_caption_marker`。
fn parse_table_caption(line: &str) -> Option<String> {
    static NUMBERED: OnceLock<Regex> = OnceLock::new();
    let numbered = NUMBERED.get_or_init(|| {
        Regex::new(
            r"^(?i:表|table)\s*(?:[A-Za-z]?\d+|[A-Za-z][.\-]\d+)(?:[.\-][A-Za-z0-9]+)*\s*(?:[:：]\s*|\s+)(.+)$",
        )
        .expect("带编号表题正则")
    });
    let trimmed = line.trim();
    if let Some(caps) = numbered.captures(trimmed) {
        let caption = strip_caption_number(caps[1].trim());
        if !caption.is_empty() {
            return Some(caption);
        }
    }
    let caption = trimmed
        .strip_prefix("Table:")
        .or_else(|| trimmed.strip_prefix("table:"))
        .or_else(|| trimmed.strip_prefix("TABLE:"))
        .or_else(|| trimmed.strip_prefix("表:"))
        .or_else(|| trimmed.strip_prefix("表："))
        .or_else(|| trimmed.strip_prefix(':'))?;
    let caption = strip_caption_number(caption.trim());
    (!caption.is_empty()).then_some(caption)
}

/// 剥掉表题开头残留的旧编号（`4.6 题名`、`E.1 题名`、`附录E 题名`）。
fn strip_caption_number(caption: &str) -> String {
    static APPENDIX: OnceLock<Regex> = OnceLock::new();
    let appendix = APPENDIX.get_or_init(|| {
        Regex::new(r"^附录\s*[A-Za-z0-9]+(?:[.\-][A-Za-z0-9]+)*\s*[、.．:：]?\s*")
            .expect("附录表题正则")
    });
    static LEADING_NUM: OnceLock<Regex> = OnceLock::new();
    let leading = LEADING_NUM.get_or_init(|| {
        Regex::new(r"^(?:\d+(?:\.\d+)*|[A-Za-z][.\-]\d+(?:[.\-]\d+)*)[.．、]?\s+([^\d\s])")
            .expect("表题编号正则")
    });
    let caption = appendix.replace(caption, "");
    leading.replace(&caption, "$1").trim().to_string()
}

/// 第一遍：给全文的锚点和文献引用编号。
fn collect_marks(blocks: &[LocatedBlock], markdown: &str) -> ResearchMarks {
    let mut marks = ResearchMarks::default();
    walk(blocks, markdown, |located, kind, anchor| {
        if let Some(anchor) = anchor {
            let number = match &kind {
                // 章锚点引的是 `\thechapter`（`1`、`A`），不是"第1章"整串。
                Kind::Chapter { number, .. } | Kind::Section { number, .. } => Some(number.clone()),
                Kind::Figure { number, .. } => number.clone(),
                Kind::Table { caption } => caption.as_ref().map(|caption| caption.number.clone()),
                _ => None,
            };
            if let Some(number) = number {
                marks.define(anchor, &number);
            }
        }
        // 表题自己那一块被并进了表格，锚点却写在它身上，所以要单独认一次。
        if let Kind::Table {
            caption: Some(caption),
        } = &kind
            && let Some(anchor) = markdown
                .get(caption.source.clone())
                .and_then(|raw| crossref::split_label(raw).1)
        {
            marks.define(anchor, &caption.number);
        }
        if let Some(raw) = markdown.get(located.range.clone()) {
            for key in crossref::citation_keys(raw) {
                marks.cite(key);
            }
        }
    });
    marks
}

/// 导航大纲里的一条标题。
///
/// 右缘导航原先一律走公文那套计数器，研究报告的 `##` 会被排成「一、」——纸上
/// 印的是「第1章」，导航里写的是「一、」，同一个标题两个号。这里把编号交回
/// [`walk`]，与版面用的是同一遍推算。
pub(crate) struct OutlineEntry {
    /// 与公文口径对齐的层级：1 留给不进刻度的文档标题（研究报告没有，题名在
    /// 封面上），2 是章（含摘要、参考文献、附录），3 起是 `1.1`、`1.1.1`。
    pub(crate) level: u8,
    /// 纸面上印在标题前的整串编号，连同它与标题之间那个间隔——导航直接把它和
    /// 标题拼起来，拼出来就该和纸上一模一样。不编号的章（摘要、参考文献、
    /// 版本变更记录）没有。
    pub(crate) number: Option<String>,
    pub(crate) text: String,
    /// 标题那一块在源码里的字节范围，导航靠它跳转和回查版面位置。
    pub(crate) line: Range<usize>,
}

/// 扫一遍源码，按研究报告的规则取出全部标题与编号。
pub(crate) fn outline(markdown: &str) -> Vec<OutlineEntry> {
    let marks = collect_marks(&export::parse_markdown_located(markdown), markdown);
    let located = export::parse_markdown_located_research(markdown, &marks);
    let mut entries = Vec::new();
    walk(&located, markdown, |located, kind, _| {
        let (level, number, text) = match kind {
            // "摘要"那行标题来自 `\begin{abstract}`，不对应任何一行源码，只能挂在
            // 摘要首块内容上——跳过去落在标题正下方，够用。
            Kind::AbstractOpen => (2, None, "摘要".to_string()),
            Kind::Chapter { heading, text, .. } => {
                (2, Some(format!("{heading}{CHAPTER_GAP}")), text)
            }
            Kind::ChapterStar(text) => (2, None, text),
            // 层级由编号自己说明：`1.1` 是节，`1.1.1` 是小节，附录的 `A.1` 同理。
            Kind::Section { number, text } => (
                2 + number.matches('.').count().min(3) as u8,
                Some(format!("{number}{SECTION_GAP}")),
                text,
            ),
            Kind::SectionStar(text) | Kind::AbstractHeading(text) => (3, None, text),
            _ => return,
        };
        if text.is_empty() && number.is_none() {
            return;
        }
        entries.push(OutlineEntry {
            level,
            number,
            text,
            line: located.range.clone(),
        });
    });
    entries
}

pub(crate) fn research_preview(
    ui: &mut egui::Ui,
    input: &DraftInput,
    markdown: &str,
    scale: PreviewScale,
    anchor: Option<&Range<usize>>,
    mut scroll_to_anchor: bool,
    line_numbers: bool,
) -> PreviewOutput {
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|input| input.content_rect()));
    let metrics = Metrics::research(scale.viewport.unwrap_or(visible.width()), scale.zoom)
        .with_line_numbers(line_numbers);
    // 两遍：先给锚点和文献定号，再按纸面字面重新切块。研究报告的标题编号不跟
    // 设置里的公文编号样式走，用默认值解析即可。
    let marks = collect_marks(&export::parse_markdown_located(markdown), markdown);
    let located = export::parse_markdown_located_research(markdown, &marks);
    let mut clicked = None;

    cover_sheet(ui, &metrics, input);

    ui.add_space(14.0);
    sheet(ui, &metrics, |ui| {
        walk(&located, markdown, |located, kind, _| {
            body_item(
                ui,
                &metrics,
                located,
                kind,
                anchor,
                &mut scroll_to_anchor,
                &mut clicked,
            );
        });
    });

    let numbered = gutter::paint(ui, &metrics, anchor);
    PreviewOutput {
        scale: metrics.scale,
        clicked: clicked.or(numbered),
    }
}

/// 简化封面：只排文字要素，不画 `md2tex.cls` 那圈 TikZ 双线页框——页框是纯
/// 装饰，编译出的 PDF 才是定稿版式，在预览里复刻它的收益抵不上维护成本。
fn cover_sheet(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput) {
    let meta = &input.research;
    sheet(ui, metrics, |ui| {
        ui.add_space(metrics.line);
        // 密级★保密年限：与 md2tex 模板的 \securitymark 一致，年限为空时不出星。
        let security = match (meta.security.trim(), meta.security_years.trim()) {
            ("", _) => String::new(),
            (level, "") => level.to_string(),
            (level, years) => format!("{level}★{years}"),
        };
        if !security.is_empty() {
            line_block(
                ui,
                metrics,
                &security,
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::RIGHT,
            );
        }
        if !meta.file_number.trim().is_empty() {
            line_block(
                ui,
                metrics,
                meta.file_number.trim(),
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::LEFT,
            );
        }

        ui.add_space(metrics.line * 6.0);
        let doc_type = meta.file_type.trim();
        if !doc_type.is_empty() {
            line_block(
                ui,
                metrics,
                doc_type,
                theme::FONT_HEITI,
                RESEARCH_COVER_TYPE_PT,
                Align::Center,
            );
            ui.add_space(metrics.line * 2.0);
        }

        let title = if input.title_hint.trim().is_empty() {
            "【待核实：文件名称】"
        } else {
            input.title_hint.trim()
        };
        line_block(
            ui,
            metrics,
            title,
            theme::FONT_BIAOSONG,
            RESEARCH_COVER_TITLE_PT,
            Align::Center,
        );

        if !meta.version.trim().is_empty() {
            ui.add_space(metrics.line * 2.0);
            line_block(
                ui,
                metrics,
                &format!("版本：{}", meta.version.trim()),
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::Center,
            );
        }

        ui.add_space(metrics.line * 6.0);
        for text in [meta.institution.trim(), meta.date.trim()] {
            if text.is_empty() {
                continue;
            }
            line_block(
                ui,
                metrics,
                text,
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::Center,
            );
            ui.add_space(metrics.line * 0.5);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn body_item(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    located: &LocatedBlock,
    kind: Kind<'_>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let source = located.range.clone();
    match kind {
        Kind::Skip => {}
        // 摘要那行标题来自 `\begin{abstract}`，不对应任何一行源码，所以不做成
        // 可点击块——点它没有可回跳的地方。
        Kind::AbstractOpen => chapter_title(ui, metrics, "摘要"),
        Kind::Chapter { heading, text, .. } => {
            clickable(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| chapter_title(ui, metrics, &format!("{heading}{CHAPTER_GAP}{text}")),
            );
        }
        Kind::ChapterStar(text) => {
            clickable(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| chapter_title(ui, metrics, &text),
            );
        }
        Kind::Section { number, text } => {
            clickable(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| section_title(ui, metrics, &format!("{number}{SECTION_GAP}{text}")),
            );
        }
        Kind::SectionStar(text) => {
            clickable(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| section_title(ui, metrics, &text),
            );
        }
        // mdx 把摘要里的标题排成一段加粗正文，不成标题。
        Kind::AbstractHeading(text) => {
            clickable(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| {
                    line_block_runs(
                        ui,
                        metrics,
                        &[
                            TextRun {
                                text: &indent(INDENT_CHARS),
                                family: theme::FONT_BOLD,
                                size: RESEARCH_BODY_PT,
                            },
                            TextRun {
                                text: &text,
                                family: theme::FONT_BOLD,
                                size: RESEARCH_BODY_PT,
                            },
                        ],
                        Align::LEFT,
                    );
                },
            );
        }
        Kind::Figure { number, alt, src } => {
            clickable(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| {
                    image_block(ui, metrics, alt, src);
                    // `\caption` 排在 figure 环境的图下方，居中，字号随正文。
                    if let Some(number) = &number {
                        line_block(
                            ui,
                            metrics,
                            &format!("图 {number} {alt}"),
                            metrics.body_family,
                            RESEARCH_BODY_PT,
                            Align::Center,
                        );
                    }
                },
            );
        }
        Kind::Table { caption } => {
            // 表题排在表上方：longtblr 的 caption 就在表头之前，标签黑体小四、
            // 题名宋体小四（md2tex.cls 的 caption-tag / caption-text）。
            if let Some(caption) = &caption {
                clickable(
                    ui,
                    metrics,
                    &caption.source,
                    anchor,
                    scroll_to_anchor,
                    clicked,
                    |ui| {
                        line_block_runs(
                            ui,
                            metrics,
                            &[
                                TextRun {
                                    text: &format!("表 {} ", caption.number),
                                    family: theme::FONT_HEITI,
                                    size: RESEARCH_CAPTION_PT,
                                },
                                TextRun {
                                    text: &caption.text,
                                    family: metrics.body_family,
                                    size: RESEARCH_CAPTION_PT,
                                },
                            ],
                            Align::Center,
                        );
                    },
                );
            }
            plain(ui, metrics, located, anchor, scroll_to_anchor, clicked);
        }
        Kind::Plain => plain(ui, metrics, located, anchor, scroll_to_anchor, clicked),
    }
}

/// 段落、表格、列表都按共用部件画：字面与字号已经跟着 Metrics 走，这里拿到的
/// 就是研究报告的版式。标题在上面单独处理，走不到 content_block 里那套公文编号。
fn plain(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    located: &LocatedBlock,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let mut counters = [0usize; 4];
    clickable(
        ui,
        metrics,
        &located.range,
        anchor,
        scroll_to_anchor,
        clicked,
        |ui| {
            content_block(
                ui,
                metrics,
                &located.block,
                &mut counters,
                false,
                &NumberingConfig::default(),
            );
        },
    );
}

/// 章号与章名之间的间隔：ctex 的 `chapter/aftername` 默认 `\quad`，一个汉字宽，
/// 用全角空格对得上。
const CHAPTER_GAP: &str = "\u{3000}";

/// 节号与节名之间的间隔：`\titleformat{\section}{...}{\thesection}{0.5em}{}`
/// 的第三个参数是 0.5em，只有章标题那档才是一个汉字。预览里的黑体西文是半角
/// 等宽（`.`、数字、空格都占 0.5em），所以一个 ASCII 空格正好是 0.5em；这里若
/// 用全角空格就宽出一倍——"1.1" 本身没有夹空格，看着宽是这个间隔撑的。
/// 不用 U+2002（en space）是因为 SimHei 没有这个码位，会掉进回退字体。
const SECTION_GAP: &str = " ";

/// 章标题：小二黑体居中，上下各空一行。
fn chapter_title(ui: &mut egui::Ui, metrics: &Metrics, text: &str) {
    ui.add_space(metrics.line);
    line_block(
        ui,
        metrics,
        text,
        theme::FONT_HEITI,
        RESEARCH_CHAPTER_PT,
        Align::Center,
    );
    ui.add_space(metrics.line);
}

/// 节标题：黑体，字号随正文，缩进 2 字（`\titlespacing` 的 2em）。
fn section_title(ui: &mut egui::Ui, metrics: &Metrics, text: &str) {
    line_block(
        ui,
        metrics,
        &format!("{}{text}", indent(INDENT_CHARS)),
        theme::FONT_HEITI,
        RESEARCH_BODY_PT,
        Align::LEFT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FontConfig, TemplateKind};

    /// 一帧里画出来的全部文字，按出现顺序拼起来。
    fn drawn(markdown: &str) -> String {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &FontConfig::default());
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..Default::default()
        };
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 6000.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            let _ = research_preview(
                ui,
                &input,
                markdown,
                PreviewScale::zoom(Some(1.0)),
                None,
                false,
                false,
            );
        });
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => Some(shape.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 交叉引用、文献引用、图表题注在纸上是编号，不是源码符号。
    #[test]
    fn marks_and_captions_are_printed_the_way_the_pdf_prints_them() {
        let text = drawn(concat!(
            "<!-- [正文] -->\n\n",
            "## 研究背景 {#chap:bg}\n\n",
            "见第{@chap:bg}章、表{@tbl:t}与图{@fig:a}，另见[@wang2020; @li2021]。\n\n",
            "表：样本分布 {#tbl:t}\n\n",
            "| 项 | 数 |\n| --- | --- |\n| 甲 | 1 |\n\n",
            "![总体架构](images/a.png){#fig:a}\n",
        ));

        assert!(text.contains("第1章　研究背景"), "{text}");
        assert!(
            text.contains("见第1章、表1.1与图1.1，另见[1,2]。"),
            "行内标记都应换成纸面编号：{text}"
        );
        assert!(text.contains("表 1.1 "), "表题应带自动编号：{text}");
        assert!(text.contains("样本分布"), "表题题名应排出：{text}");
        assert!(text.contains("图 1.1 总体架构"), "图题应带自动编号：{text}");
        for symbol in ["{@", "{#", "[@", "表：样本分布"] {
            assert!(
                !text.contains(symbol),
                "源码符号“{symbol}”不应印在纸上：{text}"
            );
        }
    }

    /// 版本变更记录、参考文献的标题只排一次：区段标记不自带标题，标题由区段
    /// 里的首个标题充当（与 `tex_research_emitter` 的 `\chapter*` 一致）。
    #[test]
    fn a_section_title_is_printed_once_not_once_per_marker_and_heading() {
        for (marker, title) in [
            ("<!-- [版本变更记录] -->", "版本变更记录"),
            ("<!-- [参考文献] -->", "参考文献"),
            ("<!-- [摘要] -->", "摘要"),
        ] {
            let text = drawn(&format!("{marker}\n\n## {title}\n\n内容一段。\n"));
            assert_eq!(
                text.matches(title).count(),
                1,
                "“{title}”应当只排一次：{text}"
            );
        }
    }

    /// 变更记录段里的后续标题降一级，不跟着排成居中的章标题。
    #[test]
    fn later_headings_in_an_unnumbered_section_become_sections() {
        let text = drawn("<!-- [版本变更记录] -->\n\n## 版本变更记录\n\n## V1.1 修订\n\n说明。\n");
        assert!(text.contains("版本变更记录"), "{text}");
        assert!(text.contains("V1.1 修订"), "{text}");
        assert!(
            !text.contains("第1章"),
            "不编号区段里的标题不该拿到章号：{text}"
        );
    }

    /// 附件标识与公文一致：每加一份附件写一个标识，附件依次拿到字母章号。
    #[test]
    fn each_attachment_marker_opens_its_own_lettered_appendix() {
        let text = drawn(concat!(
            "<!-- [正文] -->\n\n## 研究背景\n\n正文。\n\n",
            "<!-- [附件] -->\n\n## 调查问卷\n\n问卷。\n\n",
            "<!-- [附件] -->\n\n## 原始数据\n\n数据。\n",
        ));

        assert!(text.contains("第1章　研究背景"), "{text}");
        assert!(text.contains("附录A　调查问卷"), "{text}");
        assert!(text.contains("附录B　原始数据"), "{text}");
        assert!(!text.contains("第2章"), "附录不再占用正文章号：{text}");
    }

    /// 附录里的图表跟着字母章号走，与 LaTeX 的 `\thefigure` 一致。
    #[test]
    fn appendix_figures_are_numbered_against_the_letter_chapter() {
        let text = drawn(concat!(
            "<!-- [附件] -->\n\n## 调查问卷\n\n",
            "![问卷样张](images/a.png)\n",
        ));
        assert!(text.contains("图 A.1 问卷样张"), "{text}");
    }

    /// 引了不存在的锚点，就按 LaTeX 的办法印 `??`：预览里一眼看得出来，不必
    /// 等编译完再回头找。
    #[test]
    fn a_dangling_reference_prints_the_latex_placeholder() {
        let text = drawn("## 章\n\n见{@nope:x}。\n");
        assert!(text.contains("见??。"), "{text}");
    }

    #[test]
    fn a_table_caption_written_after_the_table_still_prints_above_it() {
        let text = drawn("## 章\n\n| 项 | 数 |\n| --- | --- |\n| 甲 | 1 |\n\n表：样本分布\n");
        assert!(text.contains("表 1.1 "), "{text}");
        let caption = text.find("样本分布").expect("表题");
        let cell = text.find('甲').expect("表格内容");
        assert!(caption < cell, "表题排在表格上方：{text}");
    }

    #[test]
    fn table_caption_prefixes_follow_mdx() {
        for line in [
            "表：样本分布",
            "表: 样本分布",
            "Table: 样本分布",
            "表 1.2 样本分布",
        ] {
            assert_eq!(
                parse_table_caption(line).as_deref(),
                Some("样本分布"),
                "“{line}”应当认成表题"
            );
        }
        assert_eq!(parse_table_caption("表格已经列出全部样本"), None);
    }
}
