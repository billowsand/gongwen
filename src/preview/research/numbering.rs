//! 章节、图表与文框的编号状态机，以及标题块的身份构造。
//!
//! 由 src/preview/research.rs 拆分而来：本文件是模块 `preview::research::numbering`，与其它子模块共享
//! `preview::research` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::{
    LocatedBlock, MarkdownBlock, ResearchSection, parse_research_marker, parse_unnumbered_marker,
};
use crate::preview::research::{Caption, Kind, anchor_of, caption_text, table_captions};
use std::collections::HashMap;

/// 走一遍块序列时的计数器与区段状态，规则全部照 mdx 的 `typst_research`。
pub(crate) struct Walk {
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
    /// 文框序号，每种名称各编各的，与图表一样跟着章归零（md2tex.cls 的
    /// `\mdxboxstep`）。
    boxes: HashMap<String, usize>,
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
    free_boxes: HashMap<String, usize>,
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
            boxes: HashMap::new(),
            part: 0,
            unnumbered_pending: false,
            unnumbered_root: None,
            free: false,
            free_figure: 0,
            free_table: 0,
            free_boxes: HashMap::new(),
        }
    }
}

impl Walk {
    /// `\thechapter`：正文是阿拉伯数字，附录是大写字母。
    pub(crate) fn chapter_number(&self) -> String {
        if self.appendix_started {
            let index = u32::from(b'A') + self.chapter.saturating_sub(1) as u32;
            char::from_u32(index).unwrap_or('A').to_string()
        } else {
            self.chapter.to_string()
        }
    }

    /// 开新章：节、图、表的计数器都跟着归零，与 LaTeX 的 `\chapter` 一致。
    /// 返回（纸面前缀，`\thechapter`）。
    pub(crate) fn open_chapter(&mut self) -> (String, String) {
        self.free = false;
        self.chapter += 1;
        self.section_no = 0;
        self.subsection = 0;
        self.subsubsection = 0;
        self.figure = 0;
        self.table = 0;
        self.boxes.clear();
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

    pub(crate) fn open_section(&mut self, level: u8) -> String {
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
    pub(crate) fn open_figure(&mut self) -> String {
        if self.free {
            self.free_figure += 1;
            return self.free_figure.to_string();
        }
        self.figure += 1;
        format!("{}.{}", self.chapter_number(), self.figure)
    }

    pub(crate) fn open_table(&mut self) -> String {
        if self.free {
            self.free_table += 1;
            return self.free_table.to_string();
        }
        self.table += 1;
        format!("{}.{}", self.chapter_number(), self.table)
    }

    /// 文框编号：与图表同一口径，每种名称各编各的。
    pub(crate) fn open_box(&mut self, name: &str) -> String {
        if self.free {
            let counter = self.free_boxes.entry(name.to_string()).or_default();
            *counter += 1;
            return counter.to_string();
        }
        let counter = self.boxes.entry(name.to_string()).or_default();
        *counter += 1;
        let counter = *counter;
        format!("{}.{}", self.chapter_number(), counter)
    }

    /// 开新部分。返回（纸面前缀"第一部分"，`\thepart`"一"）。
    pub(crate) fn open_part(&mut self) -> (String, String) {
        self.part += 1;
        let number = export::number_to_chinese(self.part);
        (format!("第{number}部分"), number)
    }

    /// 这个标题是否落在不编号子树里，照 mdx `parser::parse` 的判定：标记交给
    /// 紧随的标题并以它为根，更深的标题都在子树里，同级或更高一级的结束子树。
    pub(crate) fn unnumbered(&mut self, level: u8) -> bool {
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

    pub(crate) fn enter(&mut self, next: ResearchSection) {
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
pub(crate) fn walk<'a>(
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
            MarkdownBlock::Diagram { source, caption } => Kind::Diagram {
                number: (!caption.trim().is_empty()).then(|| walk.open_figure()),
                caption,
                source,
            },
            MarkdownBlock::Quote {
                boxed: Some(boxed), ..
            } => {
                let number = walk.open_box(&boxed.name);
                Kind::Box {
                    heading: format!("{} {number}", boxed.name),
                    number,
                    name: &boxed.name,
                    title: &boxed.title,
                }
            }
            MarkdownBlock::Quote { .. } => Kind::Citation,
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
pub(crate) fn heading(walk: &mut Walk, level: u8, text: &str) -> Kind<'static> {
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
pub(crate) fn unnumbered_chapter(walk: &mut Walk, text: String) -> Kind<'static> {
    walk.free = true;
    Kind::ChapterStar(text)
}

pub(crate) fn chapter(walk: &mut Walk, text: String) -> Kind<'static> {
    let (heading, number) = walk.open_chapter();
    Kind::Chapter {
        heading,
        number,
        text,
    }
}
