//! 研究报告的纸面预览。
//!
//! 公文那套版式（红头、主送、落款、版记）对研究报告一条都不适用，因此这里不
//! 复用 `official_preview` 的版面，只共用画纸、画行和行号这些底层部件。版心、
//! 字号、行距、标题层级都对齐 mdx 的 `md2tex.cls`：
//!
//! - 版心 `left=28mm, top=37mm, width=156mm`，正文 14bp / 24pt；
//! - `#` 是报告题名（整篇至多一个）：题名归封面，正文纸上不排，`\chapter*`
//!   那种排法会在目录后多出一张只有一行标题的纸；`##` 是章（小二黑体居中，
//!   编号"第N章"），`###`、`####` 依次是节和小节，编号 `N.M`、`N.M.K`，
//!   由程序生成，正文里不写；
//! - 摘要、版本变更记录、参考文献这些区段由 `<!-- [...] -->` 标记切换，标题
//!   不编号；附录切到字母章号"附录A"；部分段（`<!-- [部分] -->`）里的 `#` 是
//!   部分（"第一部分"，小一黑体居中），章号跨部分连续；
//! - `<!-- [不编号] -->` 管紧随标题的整棵子树：不占章节号，不编号章里的图表
//!   题注改用全篇共用、不带章号的流水号。
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

use super::layout::{
    TextRun, clickable, clickable_rows, is_renderable_paragraph, line_block, line_block_runs, sheet,
};
use super::render::{PreviewOutput, clickable_content_block, image_block};
use super::{
    INDENT_CHARS, Metrics, PreviewScale, RESEARCH_BODY_PT, RESEARCH_CAPTION_PT,
    RESEARCH_CHAPTER_PT, RESEARCH_PART_PT, gutter, indent, math_flow,
};
use crate::export::crossref::{self, ResearchMarks};
use crate::export::{
    self, LocatedBlock, MarkdownBlock, ResearchSection, parse_research_marker,
    parse_unnumbered_marker,
};
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
    /// 部分：部分段里的 `#`。`heading` 是"第一部分"，`number` 是 `\thepart`
    /// （"一"）——`{@id}` 引的是后者。
    Part {
        heading: String,
        number: String,
        text: String,
    },
    /// 不编号的部分：`<!-- [不编号] -->` 下部分段里的 `#`。
    PartStar(String),
    /// 编号节，`number` 形如 `1.2`、`A.2.1`。
    Section { number: String, text: String },
    /// 不编号的章标题：摘要、版本变更记录、参考文献的首个标题，以及
    /// `<!-- [不编号] -->` 下的章。
    ChapterStar(String),
    /// 报告题名：正文区段的 `#`。题名印在封面上，正文纸上不再排第二遍，
    /// 所以它不落版面；只进导航大纲，并在文档要素的「文件名称」留空时
    /// 顶上封面题名（与 mdx 的 `TexResearchEmitter::report_title` 同一口径）。
    ReportTitle(String),
    /// 不编号的节标题：上面那些区段里后续的标题，以及不编号子树里的节。
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
    /// 部分序号：跨区段不清零，与 LaTeX 的 part 计数器一致。
    part: usize,
    /// 刚读到不编号标记、还在等它的标题（mdx `parser::parse` 的同名状态）。
    unnumbered_pending: bool,
    /// 不编号子树的根标题层级（源码里的 `#` 数）。
    unnumbered_root: Option<u8>,
    /// 当前在不编号章里：图表题注是不带章号的流水号（`\mdxfreenumbers`）。
    free: bool,
    /// 不编号章的图、表流水号，全篇共用。
    free_figure: usize,
    free_table: usize,
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
            part: 0,
            unnumbered_pending: false,
            unnumbered_root: None,
            free: false,
            free_figure: 0,
            free_table: 0,
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
        self.free = false;
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

    /// `\thefigure` / `\thetable`：都是"章号.序号"，跟着章归零。不编号章里
    /// 是全篇共用的流水号，不带章号（md2tex.cls 的 `\mdxfreenumbers`）。
    fn open_figure(&mut self) -> String {
        if self.free {
            self.free_figure += 1;
            return self.free_figure.to_string();
        }
        self.figure += 1;
        format!("{}.{}", self.chapter_number(), self.figure)
    }

    fn open_table(&mut self) -> String {
        if self.free {
            self.free_table += 1;
            return self.free_table.to_string();
        }
        self.table += 1;
        format!("{}.{}", self.chapter_number(), self.table)
    }

    /// 开新部分。返回（纸面前缀"第一部分"，`\thepart`"一"）。
    fn open_part(&mut self) -> (String, String) {
        self.part += 1;
        let number = export::number_to_chinese(self.part);
        (format!("第{number}部分"), number)
    }

    /// 这个标题是否落在不编号子树里，照 mdx `parser::parse` 的判定：标记交给
    /// 紧随的标题并以它为根，更深的标题都在子树里，同级或更高一级的结束子树。
    fn unnumbered(&mut self, level: u8) -> bool {
        if std::mem::take(&mut self.unnumbered_pending) {
            self.unnumbered_root = Some(level);
            return true;
        }
        match self.unnumbered_root {
            Some(root) if level > root => true,
            _ => {
                self.unnumbered_root = None;
                false
            }
        }
    }

    fn enter(&mut self, next: ResearchSection) {
        self.section = next;
        // 区段标记结束不编号子树，也把图表题注切回章号
        self.unnumbered_root = None;
        self.free = false;
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
            // 章号跨部分连续，不清计数器
            ResearchSection::Body | ResearchSection::Part => {}
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
        // 不编号标记只交给紧随的标题，中间隔了别的内容就作废（空行不成块）。
        if !matches!(
            located.block,
            MarkdownBlock::Title(_) | MarkdownBlock::Heading(..) | MarkdownBlock::Html(_)
        ) {
            walk.unnumbered_pending = false;
        }
        let kind = match &located.block {
            // 区段标记不落在纸上，只改后面各块的身份。公文解析器把
            // `<!-- [正文] -->`、`<!-- [附件] -->` 认成了它自己的区段，落到这里
            // 按原文重判一次，免得附录被当成公文附件。
            MarkdownBlock::Html(text) => {
                if parse_unnumbered_marker(text) {
                    walk.unnumbered_pending = true;
                    visit(located, Kind::Skip, None);
                    continue;
                }
                walk.unnumbered_pending = false;
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
            // `#` 与 `##` 起走同一套区段规则：正文区段里是报告题名（归封面，
            // 不落版面），附录段里是附录自己的章标题，摘要等区段里照 mdx
            // 降级——全在 heading() 里按 level 1 判定，与 `emit_heading` 一致。
            MarkdownBlock::Title(text) => heading(&mut walk, 1, text),
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
    // 报告题名在交给 mdx 之前就被拿掉了：它既不接收不编号标记，也不打断子树。
    if walk.section == ResearchSection::Body && level == 1 {
        return Kind::ReportTitle(text);
    }
    let unnumbered = walk.unnumbered(level);
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
                (1, _) | (2, false) if unnumbered => unnumbered_chapter(walk, text),
                (1, _) | (2, false) => chapter(walk, text),
                _ if unnumbered => Kind::SectionStar(text),
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
        // 正文段的 `#` 已在上面当报告题名处理，走到这里的 `#` 都是部分段的。
        ResearchSection::Body | ResearchSection::Part => match level {
            1 => {
                // 部分标题是原文（一级标题不经公文解析器去编号），手写的
                // "第一部分"在这里剥掉，与 mdx 的 `heading::clean` 一致。
                let text = export::clean_heading_number(&text);
                if unnumbered {
                    Kind::PartStar(text)
                } else {
                    let (heading, number) = walk.open_part();
                    Kind::Part {
                        heading,
                        number,
                        text,
                    }
                }
            }
            2 if unnumbered => unnumbered_chapter(walk, text),
            2 => chapter(walk, text),
            3..=5 if unnumbered => Kind::SectionStar(text),
            3..=5 => Kind::Section {
                number: walk.open_section(level),
                text,
            },
            // mdx 忽略六级以下标题。
            _ => Kind::Skip,
        },
    }
}

/// 不编号章：不占章号，其中的图表题注切到不带章号的流水号。
fn unnumbered_chapter(walk: &mut Walk, text: String) -> Kind<'static> {
    walk.free = true;
    Kind::ChapterStar(text)
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
        // 表题与表格之间可以夹一行序号表标记：它不落版面，mdx 也不把它当块。
        let mut before = index.checked_sub(1);
        while let Some(marker) = before.filter(|marker| {
            matches!(&blocks[*marker].block, MarkdownBlock::Html(line)
                if export::parse_numbered_table_marker(line))
        }) {
            before = marker.checked_sub(1);
        }
        let leading = before
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
                Kind::Chapter { number, .. }
                | Kind::Section { number, .. }
                | Kind::Part { number, .. } => Some(number.clone()),
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
    /// 与公文口径对齐的层级：1 是文档标题（研究报告里是正文区段的 `#` 报告
    /// 题名——它不落正文版面，但进大纲当根），2 是章（含摘要、参考文献、
    /// 附录），3 起是 `1.1`、`1.1.1`。
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
    // 大纲只看标题，序号表的分组编号样式无关紧要。
    let located =
        export::parse_markdown_located_research(markdown, &marks, &NumberingConfig::default());
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
            // 部分与章同级：导航只分到"章"这一层缩进，部分靠编号区分。
            Kind::Part { heading, text, .. } => (2, Some(format!("{heading}{CHAPTER_GAP}")), text),
            Kind::PartStar(text) => (2, None, text),
            // 报告题名是大纲的根，不占章号。
            Kind::ReportTitle(text) => (1, None, text),
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

/// 正文区段的第一个 `#`：报告题名。行尾锚点不算题名的一部分，剥掉。
pub(crate) fn report_title(markdown: &str) -> Option<String> {
    export::research_report_titles(markdown)
        .first()
        .map(|line| crossref::split_label(line).0.trim())
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn research_preview(
    ui: &mut egui::Ui,
    input: &DraftInput,
    markdown: &str,
    scale: PreviewScale,
    anchor: Option<&Range<usize>>,
    mut scroll_to_anchor: bool,
    numbering: &NumberingConfig,
    line_numbers: bool,
) -> PreviewOutput {
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|input| input.content_rect()));
    let metrics = Metrics::research(scale.viewport.unwrap_or(visible.width()), scale.zoom)
        .with_line_numbers(line_numbers);
    // 两遍：先给锚点和文献定号，再按纸面字面重新切块。研究报告的标题编号不跟
    // 设置里的公文编号样式走；序号表的分组编号跟设置，与导出一致。
    let marks = collect_marks(&export::parse_markdown_located(markdown), markdown);
    let located = export::parse_markdown_located_research(markdown, &marks, numbering);
    let mut clicked = None;

    cover_sheet(ui, &metrics, input, markdown);

    ui.add_space(14.0);
    sheet(ui, &metrics, |ui| {
        walk(&located, markdown, |located, kind, _| {
            body_item(
                ui,
                &metrics,
                markdown,
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

/// 封面：照 mdx `cover::layout` 的毫米坐标整页绝对定位，与 TeX 模板的 TikZ
/// 封面、Word 的图文框封面是同一张网格。题名换几行都不推动落款。
fn cover_sheet(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput, markdown: &str) {
    use mdx::cover::{self, Family, layout as l};

    let meta = &input.research;
    let family = Family::of(&meta.file_type);
    // 题名的正主是文档要素的「文件名称」（导出时写进 frontmatter）；留空
    // 才回退到正文区的 `#`，与 mdx 的 `cover.title.or(emitter.report_title())`
    // 一条口径。两处都空才出待核实占位。
    let body_title = report_title(markdown);
    let title = match (input.title_hint.trim(), body_title.as_deref()) {
        ("", Some(title)) => title,
        ("", None) => "【待核实：文件名称】",
        (hint, _) => hint,
    };
    // 密级★保密期限：公开件不标；年限为空时不出星。
    let security = match (
        cover::security_label(&meta.security),
        meta.security_years.trim(),
    ) {
        ("", _) => String::new(),
        (level, "") => level.to_string(),
        (level, years) => format!("{level}★{years}"),
    };
    let doc_type = match meta.file_type.trim() {
        "" => "研究报告",
        doc_type => doc_type,
    };

    sheet(ui, metrics, |ui| {
        let height = (metrics.page_height - metrics.margin_top * 2.0).max(0.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
        let page = rect.min - egui::vec2(metrics.margin_left, metrics.margin_top);
        let at = |x: f32, y: f32| page + egui::vec2(metrics.mm(x), metrics.mm(y));
        let painter = ui.painter();
        let ink = theme::paper::ink();
        let faint = theme::paper::ink_faint();
        let center_x = l::PAGE_WIDTH / 2.0;

        // 一段字：`x` 是锚点（居中时为中线，左对齐时为左边），`y` 是顶边。
        // 返回画完后的底边（mm），题名块靠它往下接。
        let text = |spec: CoverText<'_>, x: f32, y: f32| -> f32 {
            let font = metrics.font(spec.family, spec.pt);
            let size = font.size;
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = metrics.mm(spec.wrap);
            job.halign = spec.align;
            job.append(
                spec.text,
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: spec.color.unwrap_or(ink),
                    line_height: Some(size * spec.leading),
                    extra_letter_spacing: size * spec.tracking,
                    italics: spec.italic,
                    ..Default::default()
                },
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let bottom = y + galley.size().y / metrics.mm(1.0);
            painter.galley(at(x, y), galley, ink);
            bottom
        };
        let bar = |x: f32, y: f32, w: f32, h: f32, color: egui::Color32| {
            painter.rect_filled(
                egui::Rect::from_min_max(at(x, y), at(x + w, y + h)),
                0.0,
                color,
            );
        };

        // 密级（左）与编号（右）。
        if !security.is_empty() {
            text(
                CoverText::new(&security, theme::FONT_HEITI, l::META_PT).left(),
                l::SIDE,
                l::META_TOP,
            );
        }
        let number = meta.file_number.trim();
        if !number.is_empty() {
            text(
                CoverText::new(&format!("编号：{number}"), theme::FONT_HEITI, l::META_PT).right(),
                l::PAGE_WIDTH - l::SIDE,
                l::META_TOP,
            );
        }

        // 文种：黑体小二，字距半字。字距加在每个字后面，把整行往右推了半字，
        // 这里左移回来，让字面居中。
        let tracking = l::TYPE_PT * l::TYPE_TRACKING_EM * 0.3528 / 2.0;
        text(
            CoverText::new(doc_type, theme::FONT_HEITI, l::TYPE_PT).tracking(l::TYPE_TRACKING_EM),
            center_x - tracking,
            l::TYPE_TOP,
        );
        let ident = meta.ident.trim();
        if !ident.is_empty() {
            text(
                CoverText::new(ident, theme::FONT_SONGTI, l::IDENT_PT),
                center_x,
                l::IDENT_TOP,
            );
        }

        // 反线：项目类单线，研究类一粗一细。
        bar(l::SIDE, l::RULE_TOP, l::TEXT_WIDTH, l::RULE_THICK, ink);
        if !family.is_project() {
            bar(
                l::SIDE,
                l::RULE_TOP + l::RULE_THICK + l::RULE_GAP,
                l::TEXT_WIDTH,
                l::RULE_THIN,
                ink,
            );
        }

        // 题名、稿次、外文原题顺排。题名按公文标题的断行规矩分行，与导出的
        // PDF、Word 用同一个函数，断在同一处。
        let title = export::title::cover_title_lines(title).join("\n");
        let mut bottom = text(
            CoverText::new(&title, theme::FONT_BIAOSONG, l::TITLE_PT).leading(l::TITLE_LEADING),
            center_x,
            l::TITLE_TOP,
        );
        if let Some(version) = cover::version_mark(&meta.version) {
            bottom = text(
                CoverText::new(&version, theme::FONT_SONGTI, l::VERSION_PT),
                center_x,
                bottom + l::TITLE_GAP,
            );
        }
        let original = meta.original_title.trim();
        if !original.is_empty() && !family.is_project() {
            text(
                CoverText::new(original, theme::FONT_SONGTI, l::ORIGINAL_PT)
                    .leading(1.35)
                    .italic(),
                center_x,
                bottom + l::TITLE_GAP,
            );
        }

        match family.stage() {
            // 项目类：四格阶段条，当前格黑色粗线，其余灰色细线。
            Some(stage) => {
                let cell = (l::TEXT_WIDTH - 3.0 * l::STAGE_GAP) / 4.0;
                for (index, name) in cover::PROJECT_STAGES.iter().enumerate() {
                    let x = l::SIDE + index as f32 * (cell + l::STAGE_GAP);
                    let on = index == stage;
                    let (line, color) = if on {
                        (l::STAGE_LINE_ON, ink)
                    } else {
                        (l::STAGE_LINE, faint)
                    };
                    bar(x, l::STAGE_TOP, cell, line, color);
                    text(
                        CoverText::new(name, theme::FONT_HEITI, l::STAGE_PT)
                            .color(color)
                            .tracking(0.12),
                        x + cell / 2.0,
                        l::STAGE_TOP + l::STAGE_LINE_ON + 2.4,
                    );
                }
            }
            // 研究类：署名行。
            None => {
                let byline = meta.byline.trim();
                if !byline.is_empty() {
                    text(
                        CoverText::new(byline, theme::FONT_SONGTI, l::BYLINE_PT),
                        center_x,
                        l::BYLINE_TOP,
                    );
                }
            }
        }

        // 落款：单位黑体三号，日期宋体小三、汉字数字。
        let institution = match meta.institution.trim() {
            "" => "【待核实：撰写单位】",
            institution => institution,
        };
        let bottom = text(
            CoverText::new(institution, theme::FONT_HEITI, l::ORG_PT),
            center_x,
            l::ORG_TOP,
        );
        let date = meta.date.trim();
        if !date.is_empty() {
            text(
                CoverText::new(&cover::chinese_date(date), theme::FONT_SONGTI, l::DATE_PT),
                center_x,
                bottom + l::DATE_GAP,
            );
        }
    });
}

/// 封面上一段字的排法。缺省居中、宽度为版心、单倍行距、黑色。
struct CoverText<'a> {
    text: &'a str,
    family: &'a str,
    pt: f32,
    align: Align,
    wrap: f32,
    leading: f32,
    tracking: f32,
    italic: bool,
    color: Option<egui::Color32>,
}

impl<'a> CoverText<'a> {
    fn new(text: &'a str, family: &'a str, pt: f32) -> Self {
        Self {
            text,
            family,
            pt,
            align: Align::Center,
            wrap: mdx::cover::layout::TEXT_WIDTH - 10.0,
            leading: 1.3,
            tracking: 0.0,
            italic: false,
            color: None,
        }
    }

    fn left(mut self) -> Self {
        self.align = Align::LEFT;
        self
    }

    fn right(mut self) -> Self {
        self.align = Align::RIGHT;
        self
    }

    fn leading(mut self, leading: f32) -> Self {
        self.leading = leading;
        self
    }

    fn tracking(mut self, em: f32) -> Self {
        self.tracking = em;
        self
    }

    fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    fn color(mut self, color: egui::Color32) -> Self {
        self.color = Some(color);
        self
    }
}

#[allow(clippy::too_many_arguments)]
fn body_item(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    markdown: &str,
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
            clickable_rows(
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
            clickable_rows(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| chapter_title(ui, metrics, &text),
            );
        }
        Kind::Part { heading, text, .. } => {
            clickable_rows(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| part_title(ui, metrics, Some(&heading), &text),
            );
        }
        Kind::PartStar(text) => {
            clickable_rows(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| part_title(ui, metrics, None, &text),
            );
        }
        // 报告题名不落在正文纸上：它已经印在封面（`cover_sheet`）里了。
        Kind::ReportTitle(_) => {}
        Kind::Section { number, text } => {
            clickable_rows(
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
            clickable_rows(
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
            clickable_rows(
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
                clickable_rows(
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
            plain(
                ui,
                metrics,
                markdown,
                located,
                anchor,
                scroll_to_anchor,
                clicked,
            );
        }
        Kind::Plain => plain(
            ui,
            metrics,
            markdown,
            located,
            anchor,
            scroll_to_anchor,
            clicked,
        ),
    }
}

/// 段落、表格、列表都按共用部件画：字面与字号已经跟着 Metrics 走，这里拿到的
/// 就是研究报告的版式。标题在上面单独处理，走不到 content_block 里那套公文编号。
///
/// 高亮与公文一致：段落、列表按行贴着文字亮（`clickable_content_block`），
/// 表格等整块图形按块矩形亮。
///
/// 含 `$` 的段落是例外：`$$...$$` 独占一段居中，`$...$` 与文字混排，都交给
/// `math_flow`，同样按行高亮（`clickable_rows`）；不含 `$` 的段落保持原路径不动。
fn plain(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    markdown: &str,
    located: &LocatedBlock,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    if let MarkdownBlock::Paragraph(text) = &located.block
        && is_renderable_paragraph(text)
        && text.contains('$')
    {
        clickable_rows(
            ui,
            metrics,
            &located.range,
            anchor,
            scroll_to_anchor,
            clicked,
            |ui| {
                // 花脸稿里的独立公式：哨兵写在 `$$` 里面（`$$〔删:…〕$$`，见
                // visual_diff::serialize），旧稿里也可能整段包在外面。剥掉再认
                // `$$`，标记整块画。
                let bare = export::strip_redline(text);
                match math_flow::block_source(bare.trim()) {
                    Some(src) => math_flow::display_block(ui, metrics, src, display_mark(text)),
                    None => math_flow::paragraph(ui, metrics, text),
                }
            },
        );
        return;
    }
    let mut counters = [0usize; 4];
    clickable_content_block(
        ui,
        metrics,
        located,
        markdown,
        &mut counters,
        false,
        &NumberingConfig::default(),
        anchor,
        scroll_to_anchor,
        clicked,
    );
}

/// 独立公式段的整块标注：段里第一处增删标记的类型，没有就是未改动。
fn display_mark(text: &str) -> export::RedlineKind {
    export::redline_chunks(text)
        .iter()
        .map(|chunk| chunk.kind)
        .find(|kind| *kind != export::RedlineKind::Same)
        .unwrap_or(export::RedlineKind::Same)
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

/// 部分标题：小一黑体居中，"第一部分"一行、题目一行（ctex 的 part 格式）。
/// PDF 里部分独占一页；预览是一张连续的纸，上下各留三行空白示意。
fn part_title(ui: &mut egui::Ui, metrics: &Metrics, heading: Option<&str>, text: &str) {
    ui.add_space(metrics.line * 3.0);
    if let Some(heading) = heading {
        line_block(
            ui,
            metrics,
            heading,
            theme::FONT_HEITI,
            RESEARCH_PART_PT,
            Align::Center,
        );
        ui.add_space(metrics.line * 0.5);
    }
    line_block(
        ui,
        metrics,
        text,
        theme::FONT_HEITI,
        RESEARCH_PART_PT,
        Align::Center,
    );
    ui.add_space(metrics.line * 3.0);
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
        drawn_titled("", markdown)
    }

    /// 同 [`drawn`]，另给文档要素的「文件名称」——封面按它印，用来把封面上的
    /// 题名和正文纸上的内容区分开。
    fn drawn_titled(title_hint: &str, markdown: &str) -> String {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &FontConfig::default());
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: title_hint.to_string(),
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
                &NumberingConfig::default(),
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

    /// 同 [`drawn`]，但保留全部图形：公式纹理、占位虚线都要从 shape 里找。
    fn drawn_shapes(markdown: &str) -> Vec<egui::epaint::ClippedShape> {
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
        ctx.run_ui(raw, |ui| {
            let _ = research_preview(
                ui,
                &input,
                markdown,
                PreviewScale::zoom(Some(1.0)),
                None,
                false,
                &NumberingConfig::default(),
                false,
            );
        })
        .shapes
    }

    /// 递归展开：egui 会把纸面底色等一批 shape 包进 `Shape::Vec`。
    fn flatten(shapes: &[egui::epaint::ClippedShape]) -> Vec<&egui::epaint::Shape> {
        fn walk<'a>(shape: &'a egui::epaint::Shape, out: &mut Vec<&'a egui::epaint::Shape>) {
            match shape {
                egui::epaint::Shape::Vec(inner) => {
                    for shape in inner {
                        walk(shape, out);
                    }
                }
                shape => out.push(shape),
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// 一帧里画出来的全部文字（含 `Shape::Vec` 里包着的），按出现顺序拼起来。
    fn flat_text(shapes: &[egui::epaint::ClippedShape]) -> String {
        flatten(shapes)
            .iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 带纹理且尺寸有限的绘制项（公式贴图的画法：无圆角的贴图在 egui 里
    /// 直接落成 `Shape::Mesh`，带圆角的才是 `Shape::Rect`，两种都要认）。
    fn textured_rects(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
        flatten(shapes)
            .iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Rect(rect)
                    if rect.fill_texture_id() != egui::TextureId::default() =>
                {
                    Some(rect.rect)
                }
                egui::epaint::Shape::Mesh(mesh)
                    if mesh.texture_id != egui::TextureId::default()
                        && mesh.vertices.len() >= 4 =>
                {
                    Some(mesh.calc_bounds())
                }
                _ => None,
            })
            .filter(|rect| {
                rect.width().is_finite()
                    && rect.height().is_finite()
                    && rect.width() > 0.0
                    && rect.height() > 0.0
            })
            .collect()
    }

    /// 行内公式：段落里出现带纹理的有限尺寸矩形，两侧文字照常排出。
    #[test]
    fn an_inline_formula_is_drawn_as_a_textured_rect_among_the_text() {
        let shapes = drawn_shapes("## 章\n\n质能方程 $E=mc^2$ 揭示了质量与能量的关系。\n");
        let rects = textured_rects(&shapes);
        assert_eq!(rects.len(), 1, "应恰好有一幅公式纹理：{rects:?}");

        let text = flat_text(&shapes);
        assert!(text.contains("质能方程"), "{text}");
        assert!(text.contains("揭示了质量与能量的关系。"), "{text}");
        assert!(!text.contains("$E=mc^2$"), "源码符号不应印在纸上：{text}");
    }

    /// 独立块公式：独占一段，纹理矩形居中于版心。
    #[test]
    fn a_display_formula_is_centered_in_the_content_area() {
        let shapes = drawn_shapes("## 章\n\n$$E=mc^2$$\n");
        let rects = textured_rects(&shapes);
        assert_eq!(rects.len(), 1, "应恰好有一幅公式纹理：{rects:?}");

        // 版心中心：纸宽 595.28pt 居中于 1000px 视口，左页边 28mm，版心 156mm，
        // 缩放 1.0（与 Metrics::research 同一套常量）。
        let page = crate::preview::PAGE_PT * crate::preview::PT;
        let side = (1000.0 - page) / 2.0;
        let margin_left =
            crate::preview::RESEARCH_MARGIN_LEFT_MM * crate::preview::MM * crate::preview::PT;
        let content = crate::preview::RESEARCH_CONTENT_MM * crate::preview::MM * crate::preview::PT;
        let expected = side + margin_left + content / 2.0;
        assert!(
            (rects[0].center().x - expected).abs() <= 2.0,
            "公式中心 {} 应落在版心中心 {expected} 附近",
            rects[0].center().x
        );
    }

    /// 花脸稿里改过的独立公式：旧、新各成一块，旧的画删除线、新的套框。
    /// 第 ③ 期测试 F6：从前新旧两个公式挤在同一行，预览认不出整块。
    #[test]
    fn a_replaced_display_formula_draws_both_blocks_with_marks() {
        let doc = crate::redline::build("## 章\n\n$$E=mc^2$$\n", "## 章\n\n$$E=mc^3$$\n");
        let shapes = drawn_shapes(&doc.markdown);
        assert_eq!(
            textured_rects(&shapes).len(),
            2,
            "新旧两幅公式：{}",
            doc.markdown
        );
        let stroked = |color: egui::Color32| {
            shapes.iter().any(|clipped| match &clipped.shape {
                egui::Shape::LineSegment { stroke, .. } => stroke.color == color,
                egui::Shape::Rect(rect) => rect.stroke.color == color,
                _ => false,
            })
        };
        assert!(stroked(crate::preview::marks::DEL_COLOR), "旧公式画删除线");
        assert!(stroked(crate::preview::marks::ADD_COLOR), "新公式套框");
    }

    /// 渲染失败的公式（不支持的命令）降级为占位框：源码以灰色小字写出来，
    /// 不出纹理、不 panic。
    #[test]
    fn an_unsupported_formula_falls_back_to_a_placeholder() {
        for markdown in [
            "## 章\n\n$$\\notarealcommand{1}$$\n",
            "## 章\n\n公式 $\\notarealcommand$ 无效。\n",
        ] {
            let shapes = drawn_shapes(markdown);
            assert!(
                textured_rects(&shapes).is_empty(),
                "失败的公式不该出纹理：{markdown}"
            );
            let text = flat_text(&shapes);
            assert!(
                text.contains("\\notarealcommand"),
                "占位框应写出公式源码：{text}"
            );
        }
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

    /// 正文区段的 `#` 是报告题名：题名归封面，正文纸上一个字都不排，也不占
    /// 章号——随后的 `##` 仍是第1章。与 mdx 一致
    /// （`tex_research_emitter::tests::test_report_title_heading`）。
    #[test]
    fn a_body_h1_is_the_report_title_and_stays_off_the_page() {
        // 封面另给一个文件名称，好把封面上那行题名和正文纸上的内容分开看。
        let text = drawn_titled(
            "封面题名",
            concat!(
                "<!-- [正文] -->\n\n# 某某问题研究报告\n\n",
                "## 研究背景\n\n正文。\n",
            ),
        );
        assert!(text.contains("封面题名"), "{text}");
        assert!(!text.contains("某某问题研究报告"), "{text}");
        assert!(text.contains("第1章　研究背景"), "{text}");
        assert!(!text.contains("第2章"), "报告题名不应占用章号：{text}");
    }

    /// 封面题名的兜底：文档要素的「文件名称」留空时取正文区的 `#`，行尾锚点
    /// 不算题名的一部分。附录里的 `#` 不是题名。
    #[test]
    fn the_cover_falls_back_to_the_body_h1() {
        assert_eq!(
            report_title("<!-- [正文] -->\n\n# 某某问题研究报告 {#chap:t}\n\n## 研究背景\n"),
            Some("某某问题研究报告".to_string())
        );
        assert_eq!(report_title("## 研究背景\n\n正文。\n"), None);
        assert_eq!(
            report_title("<!-- [附录] -->\n\n# 调查问卷\n\n问卷。\n"),
            None
        );
    }

    /// 报告题名进大纲：level 1 的根节点，没有编号；章仍是 level 2。
    #[test]
    fn the_report_title_is_the_outline_root() {
        let entries =
            outline("<!-- [正文] -->\n\n# 某某问题研究报告\n\n## 研究背景\n\n### 研究方法\n");
        let shape: Vec<(u8, Option<String>, String)> = entries
            .iter()
            .map(|entry| (entry.level, entry.number.clone(), entry.text.clone()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (1, None, "某某问题研究报告".to_string()),
                (2, Some("第1章\u{3000}".to_string()), "研究背景".to_string()),
                (3, Some("1.1 ".to_string()), "研究方法".to_string()),
            ]
        );
    }

    /// 部分段的 `#` 印成"第一部分"+题目，章号跨部分连续；`{@part:x}` 引出"一"。
    #[test]
    fn parts_are_printed_and_chapters_keep_counting_across_them() {
        let text = drawn(concat!(
            "# 某某问题研究报告\n\n",
            "<!-- [部分] -->\n\n",
            "# 第一部分 现状分析 {#part:xz}\n\n",
            "## 研究背景\n\n见第{@part:xz}部分。\n\n",
            "# 对策建议\n\n",
            "## 总体思路\n\n正文。\n",
        ));
        assert!(text.contains("第一部分\n现状分析"), "{text}");
        assert!(text.contains("第二部分\n对策建议"), "{text}");
        assert!(text.contains("第1章　研究背景"), "{text}");
        assert!(text.contains("第2章　总体思路"), "{text}");
        assert!(text.contains("见第一部分。"), "{text}");
        assert!(!text.contains("某某问题研究报告\n第一部分"), "{text}");
    }

    /// 不编号子树：标题不带号、不占号；不编号章的表题用全篇共用的流水号。
    #[test]
    fn an_unnumbered_subtree_prints_without_numbers_and_captions_run_apart() {
        let table = "| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n";
        let text = drawn(&format!(
            "<!-- [不编号] -->\n## 前言\n\n表：前言表\n\n{table}\n### 编写说明\n\n\
             ## 研究背景\n\n表：背景表\n\n{table}\n### 基本情况\n\n\
             <!-- [不编号] -->\n### 附带说明\n\n### 主要问题\n\n\
             <!-- [不编号] -->\n## 结束语\n\n表：结束表\n\n{table}"
        ));
        assert!(text.contains("前言"), "{text}");
        assert!(text.contains("编写说明"), "{text}");
        assert!(!text.contains("1.1 编写说明"), "{text}");
        assert!(text.contains("第1章　研究背景"), "前言不占章号：{text}");
        assert!(text.contains("1.1 基本情况"), "{text}");
        assert!(!text.contains("1.2 附带说明"), "{text}");
        assert!(text.contains("1.2 主要问题"), "不编号的节不占节号：{text}");
        assert!(
            text.contains("表 1 \n前言表") || text.contains("表 1 前言表"),
            "{text}"
        );
        assert!(text.contains("表 1.1 "), "{text}");
        assert!(text.contains("表 2 "), "结束语接着前言的流水号：{text}");
    }

    /// 导航大纲：部分与章同级，带"第一部分"；不编号的章与节没有编号。
    #[test]
    fn the_outline_lists_parts_and_unnumbered_headings() {
        let entries = outline(concat!(
            "<!-- [不编号] -->\n## 前言\n\n",
            "<!-- [部分] -->\n\n# 现状分析\n\n## 研究背景\n\n### 研究方法\n",
        ));
        let shape: Vec<(u8, Option<String>, String)> = entries
            .iter()
            .map(|entry| (entry.level, entry.number.clone(), entry.text.clone()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (2, None, "前言".to_string()),
                (
                    2,
                    Some("第一部分\u{3000}".to_string()),
                    "现状分析".to_string()
                ),
                (2, Some("第1章\u{3000}".to_string()), "研究背景".to_string()),
                (3, Some("1.1 ".to_string()), "研究方法".to_string()),
            ]
        );
    }
}
