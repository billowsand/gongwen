//! 锚点、图表题注、交叉引用、大纲与报告题名。
//!
//! 由 src/preview/research.rs 拆分而来：本文件是模块 `preview::research::marks`，与其它子模块共享
//! `preview::research` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::crossref;
use crate::export::crossref::ResearchMarks;
use crate::export::{LocatedBlock, MarkdownBlock};
use crate::models::NumberingConfig;
use crate::preview::research::{CHAPTER_GAP, Kind, SECTION_GAP, walk};
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

/// 这一块定义的锚点 `{#id}`。锚点写在源码行尾，排版前已被剥掉，所以回原文去取。
/// 文框的锚点写在首行行尾，不在块尾。
pub(crate) fn anchor_of<'a>(markdown: &'a str, located: &LocatedBlock) -> Option<&'a str> {
    let raw = markdown.get(located.range.clone())?;
    let raw = match located.block {
        MarkdownBlock::Quote { .. } => raw.lines().next()?,
        MarkdownBlock::Diagram { .. } => raw.lines().last()?,
        _ => raw,
    };
    crossref::split_label(raw).1
}

/// 「表格块下标 → 表题块下标」。表题写在表前或表后都认，表前优先——与 mdx 的
/// `take_leading_table_caption().or(parse_trailing_table_caption())` 一致。
pub(crate) fn table_captions(blocks: &[LocatedBlock]) -> HashMap<usize, usize> {
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
pub(crate) fn caption_text(located: &LocatedBlock) -> Option<String> {
    let MarkdownBlock::Paragraph(text) = &located.block else {
        return None;
    };
    // 第一遍解析拿到的文字还带着行尾锚点，剥掉再认。
    parse_table_caption(crossref::split_label(text).0)
}

/// 表题行：`表：题名`、`表 1.2 题名`、`Table: 题名`、`：题名`。
/// 编号由 LaTeX 的表格计数器生成，源码里写的旧编号一律剥掉，免得印两遍。
/// 写法与正则都照 mdx 的 `parser::parse_table_caption_marker`。
pub(crate) fn parse_table_caption(line: &str) -> Option<String> {
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
pub(crate) fn strip_caption_number(caption: &str) -> String {
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

/// 第一遍：给全文的锚点和文献引用编号。`bib` 是文献库（`.bib` 内容）：叙述式
/// `@key` 只有键在库里才算引用，与 mdx 排 PDF 时一致。
pub(crate) fn collect_marks(blocks: &[LocatedBlock], markdown: &str, bib: &str) -> ResearchMarks {
    let library = export::bibliography::library(bib);
    collect_marks_with(blocks, markdown, &|key| library.contains(key), |_, _| {})
}

/// 一个锚点挂在什么东西上：`{@id}` 印的号，以及交叉引用菜单要的分类、纸面前缀和题名。
pub(crate) struct Anchored<'k> {
    kind: LabelKind,
    /// `{@id}` 印出来的那串：章是 `\thechapter`（`1`、`A`），不是"第1章"整串。
    reference: &'k str,
    /// 章、部分、文框在纸面上的整串前缀（"第1章""附录A""第一部分""案例 2.1"）；
    /// 节、图、表没有。
    heading: Option<&'k str>,
    title: &'k str,
}

impl<'k> Anchored<'k> {
    pub(crate) fn of(kind: &'k Kind<'_>) -> Option<Self> {
        let (kind, reference, heading, title) = match kind {
            Kind::Part {
                heading,
                number,
                text,
            } => (
                LabelKind::Part,
                number,
                Some(heading.as_str()),
                text.as_str(),
            ),
            Kind::Chapter {
                heading,
                number,
                text,
            } => (
                LabelKind::Chapter,
                number,
                Some(heading.as_str()),
                text.as_str(),
            ),
            // 节的深浅由编号自己说明：`1.2` 是节，`1.2.3` 起是小节，附录 `A.1` 同理。
            Kind::Section { number, text } => {
                let kind = if number.matches('.').count() > 1 {
                    LabelKind::Subsection
                } else {
                    LabelKind::Section
                };
                (kind, number, None, text.as_str())
            }
            Kind::Figure {
                number: Some(number),
                alt,
                ..
            } => (LabelKind::Figure, number, None, *alt),
            Kind::Diagram {
                number: Some(number),
                caption,
                ..
            } => (LabelKind::Figure, number, None, *caption),
            Kind::Table {
                caption: Some(caption),
            } => (
                LabelKind::Table,
                &caption.number,
                None,
                caption.text.as_str(),
            ),
            Kind::Box {
                number,
                heading,
                title,
                ..
            } => (LabelKind::Box, number, Some(heading.as_str()), *title),
            _ => return None,
        };
        Some(Self {
            kind,
            reference,
            heading,
            title,
        })
    }
}

/// 同 [`collect_marks`]，每登记一个锚点就把它交给 `on_anchor`。
///
/// 交叉引用菜单与排版共用这一遍：分类、编号的口径只有一份，菜单里写的号就是纸上印的号。
pub(crate) fn collect_marks_with(
    blocks: &[LocatedBlock],
    markdown: &str,
    known: &dyn Fn(&str) -> bool,
    mut on_anchor: impl FnMut(&str, Anchored<'_>),
) -> ResearchMarks {
    let mut marks = ResearchMarks::default();
    walk(blocks, markdown, |located, kind, anchor| {
        if let Some(anchor) = anchor
            && let Some(target) = Anchored::of(&kind)
        {
            marks.define(anchor, target.reference);
            on_anchor(anchor, target);
        }
        // 表题自己那一块被并进了表格，锚点却写在它身上，所以要单独认一次。
        if let Kind::Table {
            caption: Some(caption),
        } = &kind
            && let Some(anchor) = markdown
                .get(caption.source.clone())
                .and_then(|raw| crossref::split_label(raw).1)
            && let Some(target) = Anchored::of(&kind)
        {
            marks.define(anchor, target.reference);
            on_anchor(anchor, target);
        }
        if let Some(raw) = markdown.get(located.range.clone()) {
            for key in crossref::citation_keys(raw, known) {
                marks.cite(key);
            }
        }
    });
    marks
}

/// 交叉引用目标的分类，由锚点挂在哪一块上判定，不看 id 怎么起名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LabelKind {
    Part,
    Chapter,
    Section,
    Subsection,
    Figure,
    Table,
    Box,
}

impl LabelKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Part => "部分",
            Self::Chapter => "章",
            Self::Section => "节",
            Self::Subsection => "小节",
            Self::Figure => "图",
            Self::Table => "表",
            Self::Box => "文框",
        }
    }
}

/// 交叉引用菜单里的一条可引目标。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LabelTarget {
    pub(crate) id: String,
    pub(crate) kind: LabelKind,
    /// 纸面上印在标题或题注前的编号：第一部分、第1章、附录A、1.2、图 1.3、表 2、
    /// 专栏 2.1、案例 1.1。
    pub(crate) number: String,
    /// 标题、图注或表题，行内标记已换成纸面字面。
    pub(crate) title: String,
}

/// 全文可被 `{@id}` 引用的目标，按出现顺序；同一个 id 重复定义只留第一次，
/// 与 [`ResearchMarks::define`] 一致。
///
/// 只走第一遍解析（不排版、不做第二遍切块），比导航大纲还省；起草页只在
/// 交叉引用下拉展开时才调它。挂在不编号标题或普通段落上的锚点没有号可引，
/// 不在其列——引了也只会印成 `??`。
pub(crate) fn label_targets(markdown: &str) -> Vec<LabelTarget> {
    let blocks = export::parse_markdown_located(markdown);
    let mut targets: Vec<LabelTarget> = Vec::new();
    // 菜单只要题名，不看文献库：叙述式 `@key` 不登记，题名里原样留着。
    let marks = collect_marks_with(&blocks, markdown, &|_| false, |id, target| {
        if targets.iter().any(|seen| seen.id == id) {
            return;
        }
        let number = match target.kind {
            LabelKind::Figure => format!("图 {}", target.reference),
            LabelKind::Table => format!("表 {}", target.reference),
            _ => target.heading.unwrap_or(target.reference).to_string(),
        };
        targets.push(LabelTarget {
            id: id.to_string(),
            kind: target.kind,
            number,
            // 第一遍的题名还带着行尾锚点和行内标记，等编号全定下来再换。
            title: target.title.to_string(),
        });
    });
    for target in &mut targets {
        if let Cow::Owned(title) = marks.apply(&target.title) {
            target.title = crossref::strip_superscript(&title).into_owned();
        }
    }
    targets
}

/// 正文引过的文献：序号、引用处数，按序号排。与排版共用第一遍，序号就是纸上印的号
/// （GB/T 7714 顺序编码制：按首次引用的先后）。
pub(crate) fn citations(markdown: &str, bib: &str) -> Vec<crossref::CitedKey> {
    collect_marks(&export::parse_markdown_located(markdown), markdown, bib).cited()
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
    // 大纲只看标题，不看文献库。
    let marks = collect_marks(&export::parse_markdown_located(markdown), markdown, "");
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
            text: crossref::strip_superscript(&text).into_owned(),
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
