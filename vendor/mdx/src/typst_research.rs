//! 研究报告 → Typst 排版数据（公文助手的 Typst 引擎用）。
//!
//! 沿用原 TeX 输出（`md2tex.cls`）的解析结果与区段规则，把整篇
//! 报告整理成一份可序列化的 [`Doc`]，交给宿主程序的 Typst 模板去排。章节号、
//! 图表号、文框号、列表序号都在这里按原 `md2tex.cls` 的计数器规则算好，模板只负责排；
//! 只有交叉引用的落点（`{@id}`）与目录页码留给 Typst 自己解析。
//!
//! 与原 TeX 输出刻意不同的两处（TeX 那边是缺陷）：
//! - 列表从二级回到一级时序号接着往下数（TeX 会重开一个 asparaenum，从 ⑴ 再数）；
//! - 参考文献只排一个标题：`<!-- [参考文献] -->` 的标题下直接接文献表（TeX 另起
//!   一页再排一遍“参考文献”）。

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::common::ast::{Block, Inline, LineAlign, MarkerKind, QuoteItem, QuoteKind};
use crate::common::figure_size;
use crate::common::front_matter;
use crate::common::table::span_at;
use crate::common::table_layout::{analyze_table, cell_alignment, ColumnAlignment, ColumnWidth};

/// 一份排版数据。
#[derive(Debug, Serialize)]
pub struct Doc {
    pub cover: Cover,
    pub blocks: Vec<Item>,
    /// 正文有文献引用：模板据此装载 `references.bib`。
    pub bibliography: bool,
    /// 不阻断排版的提示（缺图之类）。
    #[serde(skip)]
    pub warnings: Vec<String>,
}

/// 封面（`template.tex` 的 titlepage）。空串表示那一行不排。
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Cover {
    /// 左上角密级（公开件为空）。
    pub security: String,
    /// 保密年限，非空时以“★”接在密级后。
    pub security_years: String,
    /// 右上角编号（不含“编号：”）。
    pub number: String,
    /// 文种（逐字隔半字排）。
    pub doc_type: String,
    pub ident: String,
    /// 题名各行。
    pub title: Vec<Vec<Run>>,
    /// 稿次（已带全角括号）。
    pub version: String,
    pub original: String,
    /// 项目类的当前阶段（0–3）；研究类为 `None`。
    pub stage: Option<usize>,
    /// 署名行，空格分隔的各项之间排一字空。
    pub byline: Vec<String>,
    pub institution: String,
    pub date: String,
}

/// 行内片段。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum Run {
    /// 文字。
    S {
        v: String,
    },
    B {
        c: Vec<Run>,
    },
    I {
        c: Vec<Run>,
    },
    Code {
        v: String,
    },
    Link {
        v: String,
        url: String,
    },
    /// 行内图片（未独占一段），满版心宽。
    Img {
        src: String,
        page: Option<u32>,
    },
    /// 交叉引用：印目标的编号，落点由模板查。
    Ref {
        id: String,
    },
    Cite {
        keys: Vec<String>,
    },
    /// 脚注（内容是纯文字）。
    Fn {
        v: String,
    },
    /// 公式源码；`d` 为展示样式。
    Math {
        v: String,
        d: bool,
    },
}

/// 版面上的一块。
#[derive(Debug, Serialize)]
#[serde(tag = "k", rename_all = "kebab-case")]
pub enum Item {
    /// 摘要单独编页（`\mdxfrontmatter`）：另起奇数页，小写罗马页码。
    Front,
    /// 摘要（`abstract` 环境）：标题“摘要”，进目录。
    Abstract {
        blocks: Vec<Item>,
    },
    /// 目录（`\mdxtableofcontents`）。
    Toc,
    /// 部分：`number` 是“一”，`None` 是不编号部分。
    Part {
        number: Option<String>,
        text: Vec<Run>,
        label: Option<String>,
    },
    /// 章。`number` 是 `\thechapter`（“1”“A”），`prefix` 是纸面上的“第1章”
    /// “附录A”；两者都为空是不编号章。`toc` 为假是版本变更记录、参考文献
    /// 那种 `\chapter*`。`numbered` 的章把脚注号归零。
    Chapter {
        number: Option<String>,
        prefix: Option<String>,
        text: Vec<Run>,
        label: Option<String>,
        toc: bool,
    },
    /// 节：`level` 1–3 对应 section / subsection / subsubsection。
    Section {
        level: u8,
        number: Option<String>,
        text: Vec<Run>,
        label: Option<String>,
        toc: bool,
    },
    Par {
        c: Vec<Run>,
    },
    Aligned {
        align: &'static str,
        c: Vec<Run>,
    },
    /// 列表段：一级条目各成一段（段落式，首行缩进），更深的条目接排在段内。
    List {
        items: Vec<ListEntry>,
    },
    Table(Table),
    Figure(Figure),
    Code {
        lang: String,
        text: String,
    },
    Math {
        v: String,
    },
    /// 引文。
    Quote {
        items: Vec<QuoteLine>,
    },
    /// 文框。
    Box {
        name: String,
        number: String,
        title: Vec<Run>,
        items: Vec<QuoteLine>,
        label: Option<String>,
    },
    /// 文献表。`titled` 为假时接在参考文献区段的标题下。
    Bib {
        titled: bool,
    },
}

#[derive(Debug, Serialize)]
pub struct ListEntry {
    pub level: u8,
    pub label: String,
    pub c: Vec<Run>,
}

/// 引文、文框里的一行。
#[derive(Debug, Serialize)]
#[serde(tag = "k", rename_all = "kebab-case")]
pub enum QuoteLine {
    Par {
        c: Vec<Run>,
    },
    /// 出处行，靠右。
    Source {
        c: Vec<Run>,
    },
    /// 列表项：`label` 是 ⑴ ⑵……
    List {
        label: String,
        c: Vec<Run>,
    },
    /// 整行独立公式，居中。
    Math {
        v: String,
    },
}

#[derive(Debug, Serialize)]
pub struct Table {
    pub caption: Option<Vec<Run>>,
    pub number: String,
    pub label: Option<String>,
    pub cols: Vec<Column>,
    /// 网格：被合并掉的格子为 `None`。
    pub rows: Vec<Vec<Option<Cell>>>,
}

#[derive(Debug, Serialize)]
pub struct Column {
    /// `l` / `c`。
    pub align: char,
    /// `fr`（X 列，`v` 是比例）或 `em`（定宽，`v` 是字数）。
    pub kind: &'static str,
    pub v: f64,
}

#[derive(Debug, Serialize)]
pub struct Cell {
    pub c: Vec<Run>,
    pub align: char,
    pub colspan: usize,
    pub rowspan: usize,
}

#[derive(Debug, Serialize)]
pub struct Figure {
    pub src: String,
    pub page: Option<u32>,
    /// 占版心宽的比例；`None` 时满宽、限高（读不出尺寸）。
    pub width: Option<f64>,
    pub caption: Option<String>,
    pub number: String,
    pub label: Option<String>,
}

/// 读一份带 frontmatter 的研究报告 Markdown，整理成排版数据。插图、文献按
/// 文件所在目录解析，校验（文献键、交叉引用）与 TeX 路径相同，不过就报错。
pub fn build(input: &Path) -> Result<Doc> {
    let content = std::fs::read_to_string(input)
        .with_context(|| format!("读取文件 {} 失败", input.display()))?;
    let content = content.trim_start_matches('\u{feff}');
    let base_dir = input.parent().unwrap_or(Path::new("."));
    let report_title = report_title_line(content).map(|(_, title)| title);
    let body = remove_report_title(content);
    let (cover, markdown) = front_matter::parse(&body);
    let title = cover.title.clone().or(report_title);

    let blocks = crate::parser::parse(&markdown);
    let citations =
        crate::common::citation::validate(&blocks, cover.bibliography.as_deref(), base_dir)?;
    crate::common::crossref::check_or_bail(&blocks, crate::common::crossref::Support::Full)?;

    let mut builder = Builder::new(base_dir);
    builder.has_citations = citations.has_citations;
    builder.emit_all(&blocks);
    let (items, warnings) = builder.finish();
    Ok(Doc {
        cover: cover_of(&cover, title.as_deref()),
        blocks: items,
        bibliography: citations.has_citations,
        warnings,
    })
}

/// 报告题名所在的行号：正文区段里的第一个 `#` 标题。
///
/// 只认正文区段（开头的默认区段或 `<!-- [正文] -->` 之后）：摘要、附录、部分等
/// 区段里的 `#` 各有归属（摘要标题、附录章、部分），拿去当题名就会从正文里丢掉
/// 一行。与公文助手的 `research_report_titles` 同一口径。
fn report_title_line(content: &str) -> Option<(usize, String)> {
    let heading_regex = regex::Regex::new(r"^#\s+(.+?)(?:\s*\{[^}]*\})?\s*$").ok()?;
    let mut in_body = true;
    for (index, line) in content.lines().enumerate() {
        if let Some(kind) = crate::common::markers::detect(line) {
            in_body = kind == MarkerKind::Body;
            continue;
        }
        if !in_body {
            continue;
        }
        if let Some(caps) = heading_regex.captures(line) {
            let title = crate::common::heading::clean(caps.get(1)?.as_str().trim());
            if !title.is_empty() {
                return Some((index, title));
            }
        }
    }
    None
}

/// 去掉报告题名那一行：题名归封面，正文版面不排。
fn remove_report_title(content: &str) -> String {
    let skip = report_title_line(content).map(|(index, _)| index);
    content
        .lines()
        .enumerate()
        .filter(|(index, _)| Some(*index) != skip)
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn cover_of(meta: &front_matter::Metadata, title: Option<&str>) -> Cover {
    let doc_type = meta
        .doc_type
        .as_deref()
        .unwrap_or("研究报告")
        .trim()
        .to_string();
    let date = meta
        .date
        .as_deref()
        .map(crate::cover::chinese_date)
        .unwrap_or_else(|| {
            use chrono::Datelike;
            let now = chrono::Local::now();
            format!("{}年{}月", now.year(), now.month())
        });
    let title = match title {
        Some(title) => crate::cover::title_lines(title, meta.title_lines.as_deref())
            .iter()
            .map(|line| heading_runs(line))
            .collect(),
        None => vec![vec![Run::S {
            v: "文档标题".into(),
        }]],
    };
    let opt = |value: &Option<String>| value.as_deref().unwrap_or("").trim().to_string();
    Cover {
        security: crate::cover::security_label(meta.security.as_deref().unwrap_or("公开"))
            .to_string(),
        security_years: opt(&meta.security_years),
        number: opt(&meta.doc_number),
        stage: crate::cover::Family::of(&doc_type).stage(),
        doc_type,
        ident: opt(&meta.ident),
        title,
        version: crate::cover::version_mark(meta.version.as_deref().unwrap_or(""))
            .unwrap_or_default(),
        original: opt(&meta.original_title),
        byline: opt(&meta.byline)
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        institution: meta
            .institution
            .as_deref()
            .unwrap_or("某某单位")
            .trim()
            .to_string(),
        date,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Abstract,
    Appendix,
    Changelog,
    Reference,
    Part,
}

/// LaTeX 计数器的对应物。
#[derive(Default)]
struct Counters {
    part: usize,
    chapter: usize,
    section: usize,
    subsection: usize,
    subsubsection: usize,
    figure: usize,
    table: usize,
    free_figure: usize,
    free_table: usize,
    /// 每种文框名称一对计数器：(随章, 全篇共用)。
    boxes: Vec<(String, usize, usize)>,
}

struct Builder {
    base_dir: PathBuf,
    items: Vec<Item>,
    warnings: Vec<String>,
    mode: Mode,
    pending_label: Option<String>,
    pending_unnumbered: bool,
    /// 摘要攒到区段结束整体输出。
    abstract_items: Vec<Item>,
    in_abstract: bool,
    abstract_skipped_heading: bool,
    abstract_numbered_apart: bool,
    toc_done: bool,
    /// 已经输出过 `\appendix`：章号改用字母。
    appendix: bool,
    appendix_saw_h1: bool,
    /// 当前在不编号章里：图表、文框用全篇共用的流水号。
    free: bool,
    star_heading_done: bool,
    counters: Counters,
    /// 正在攒的列表段，及其层级栈：(层级, 本层序号)。
    list: Vec<ListEntry>,
    list_stack: Vec<(u8, usize)>,
    /// 摘要里的列表按旧前缀排（`1.`、`(1)`），这里记各级计数。
    abstract_list: [usize; 6],
    abstract_list_level: u8,
    has_citations: bool,
    reference_open: bool,
    bib_done: bool,
}

impl Builder {
    fn new(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
            items: Vec::new(),
            warnings: Vec::new(),
            mode: Mode::Normal,
            pending_label: None,
            pending_unnumbered: false,
            abstract_items: Vec::new(),
            in_abstract: false,
            abstract_skipped_heading: false,
            abstract_numbered_apart: false,
            toc_done: false,
            appendix: false,
            appendix_saw_h1: false,
            free: false,
            star_heading_done: false,
            counters: Counters::default(),
            list: Vec::new(),
            list_stack: Vec::new(),
            abstract_list: [0; 6],
            abstract_list_level: 0,
            has_citations: false,
            reference_open: false,
            bib_done: false,
        }
    }

    fn finish(mut self) -> (Vec<Item>, Vec<String>) {
        self.flush_list();
        if self.in_abstract {
            self.finish_abstract();
        }
        self.close_reference();
        if self.has_citations && !self.bib_done {
            self.items.push(Item::Bib { titled: true });
        }
        (self.items, self.warnings)
    }

    fn push(&mut self, item: Item) {
        if self.in_abstract {
            self.abstract_items.push(item);
        } else {
            self.items.push(item);
        }
    }

    fn emit_all(&mut self, blocks: &[Block]) {
        self.abstract_numbered_apart = crate::common::ast::toc_follows_abstract(blocks);
        for block in blocks {
            self.emit_block(block);
        }
    }

    fn emit_block(&mut self, block: &Block) {
        match block {
            Block::List { level, content, .. } => {
                self.list_item(*level, content);
                return;
            }
            Block::Empty | Block::Label(_) | Block::Unnumbered => {}
            _ => self.flush_list(),
        }
        match block {
            Block::Heading { level, text } => self.heading(*level, text),
            Block::Paragraph(inlines) => self.paragraph(inlines),
            Block::Aligned { align, content } => {
                let c = runs(content);
                if !c.is_empty() {
                    let align = match align {
                        LineAlign::Center => "center",
                        LineAlign::Right => "right",
                    };
                    self.push(Item::Aligned { align, c });
                }
            }
            Block::List { .. } => unreachable!(),
            Block::Table {
                rows,
                caption,
                spans,
                numbered,
            } => self.table(rows, spans, *numbered, caption.as_deref()),
            Block::Marker(kind) => self.marker(*kind),
            Block::Label(id) => self.pending_label = Some(id.clone()),
            Block::CodeBlock { lang, content } => {
                if !self.in_abstract {
                    self.items.push(Item::Code {
                        lang: lang.clone().unwrap_or_default(),
                        text: content.clone(),
                    });
                }
            }
            Block::Math(source) => {
                if !self.in_abstract && !source.trim().is_empty() {
                    self.items.push(Item::Math { v: source.clone() });
                }
            }
            Block::Quote { kind, items } => self.quote(kind, items),
            Block::Toc => self.toc(),
            Block::Unnumbered => self.pending_unnumbered = true,
            Block::Empty => {}
        }
    }

    // ---------------- 区段 ----------------

    fn toc(&mut self) {
        if self.toc_done {
            return;
        }
        self.toc_done = true;
        if self.in_abstract {
            self.finish_abstract();
        }
        self.items.push(Item::Toc);
    }

    fn marker(&mut self, kind: MarkerKind) {
        self.free = false;
        if kind != MarkerKind::Abstract && self.in_abstract {
            self.finish_abstract();
        }
        self.close_reference();
        match kind {
            MarkerKind::Abstract => {
                self.mode = Mode::Abstract;
                self.in_abstract = true;
                self.abstract_skipped_heading = false;
            }
            MarkerKind::Appendix => {
                self.mode = Mode::Appendix;
                if !self.appendix {
                    self.appendix = true;
                    // \appendix：章号归零、改用字母。
                    self.counters.chapter = 0;
                    self.counters.section = 0;
                }
                self.appendix_saw_h1 = false;
            }
            MarkerKind::Changelog => {
                self.mode = Mode::Changelog;
                self.star_heading_done = false;
            }
            MarkerKind::Reference => {
                self.mode = Mode::Reference;
                self.star_heading_done = false;
                self.reference_open = true;
            }
            MarkerKind::Body => self.mode = Mode::Normal,
            MarkerKind::Part => self.mode = Mode::Part,
        }
    }

    /// 参考文献区段结束：文献表接在它的标题下面。
    fn close_reference(&mut self) {
        if !std::mem::take(&mut self.reference_open) {
            return;
        }
        if self.has_citations && !self.bib_done {
            self.bib_done = true;
            self.items.push(Item::Bib {
                titled: !self.star_heading_done,
            });
        }
    }

    fn finish_abstract(&mut self) {
        self.flush_list();
        let blocks = std::mem::take(&mut self.abstract_items);
        if !blocks.is_empty() {
            if self.abstract_numbered_apart {
                self.items.push(Item::Front);
            }
            self.items.push(Item::Abstract { blocks });
        }
        self.in_abstract = false;
        self.mode = Mode::Normal;
    }

    // ---------------- 标题 ----------------

    fn heading(&mut self, level: u8, text: &str) {
        let unnumbered = std::mem::take(&mut self.pending_unnumbered);
        let label = self.pending_label.take().filter(|_| !unnumbered);
        let text_runs = heading_runs(text);

        if self.in_abstract {
            if !self.abstract_skipped_heading {
                self.abstract_skipped_heading = true;
            } else {
                self.abstract_items.push(Item::Par {
                    c: vec![Run::B { c: text_runs }],
                });
            }
            return;
        }

        if matches!(self.mode, Mode::Changelog | Mode::Reference) {
            if !self.star_heading_done {
                self.star_heading_done = true;
                self.items.push(Item::Chapter {
                    number: None,
                    prefix: None,
                    text: text_runs,
                    label: None,
                    toc: false,
                });
            } else {
                self.items.push(Item::Section {
                    level: 1,
                    number: None,
                    text: text_runs,
                    label: None,
                    toc: false,
                });
            }
            return;
        }

        if self.mode == Mode::Appendix {
            if level == 1 {
                self.appendix_saw_h1 = true;
            }
            // 以 `#` 开章时整体下移一级：`#` 是章、`##` 是节。
            match level + u8::from(self.appendix_saw_h1) {
                0..=2 => self.chapter(text_runs, label, unnumbered),
                3 => self.section(1, text_runs, label, unnumbered),
                4 => self.section(2, text_runs, label, unnumbered),
                _ => self.section(3, text_runs, label, unnumbered),
            }
            return;
        }

        if self.mode == Mode::Part && level == 1 {
            let number = (!unnumbered).then(|| {
                self.counters.part += 1;
                crate::common::numbering::number_to_chinese(self.counters.part)
            });
            self.items.push(Item::Part {
                number,
                text: text_runs,
                label,
            });
            return;
        }

        match level {
            1 | 2 => self.chapter(text_runs, label, unnumbered),
            3 => self.section(1, text_runs, label, unnumbered),
            4 => self.section(2, text_runs, label, unnumbered),
            5 => self.section(3, text_runs, label, unnumbered),
            _ => {}
        }
    }

    fn chapter_number(&self) -> String {
        if self.appendix {
            crate::common::numbering::number_to_uppercase_letter(self.counters.chapter)
        } else {
            self.counters.chapter.to_string()
        }
    }

    fn chapter(&mut self, text: Vec<Run>, label: Option<String>, unnumbered: bool) {
        if unnumbered {
            self.free = true;
            self.items.push(Item::Chapter {
                number: None,
                prefix: None,
                text,
                label: None,
                toc: true,
            });
            return;
        }
        self.free = false;
        let c = &mut self.counters;
        c.chapter += 1;
        c.section = 0;
        c.subsection = 0;
        c.subsubsection = 0;
        c.figure = 0;
        c.table = 0;
        for (_, chapter, _) in &mut c.boxes {
            *chapter = 0;
        }
        let number = self.chapter_number();
        let prefix = if self.appendix {
            format!("附录{number}")
        } else {
            format!("第{number}章")
        };
        self.items.push(Item::Chapter {
            number: Some(number),
            prefix: Some(prefix),
            text,
            label,
            toc: true,
        });
    }

    fn section(&mut self, level: u8, text: Vec<Run>, label: Option<String>, unnumbered: bool) {
        let number = (!unnumbered).then(|| {
            let c = &mut self.counters;
            match level {
                1 => {
                    c.section += 1;
                    c.subsection = 0;
                    c.subsubsection = 0;
                }
                2 => {
                    c.subsection += 1;
                    c.subsubsection = 0;
                }
                _ => c.subsubsection += 1,
            }
            let mut parts = vec![self.chapter_number(), self.counters.section.to_string()];
            if level >= 2 {
                parts.push(self.counters.subsection.to_string());
            }
            if level >= 3 {
                parts.push(self.counters.subsubsection.to_string());
            }
            parts.join(".")
        });
        self.items.push(Item::Section {
            level,
            number,
            text,
            label,
            toc: true,
        });
    }

    // ---------------- 正文块 ----------------

    fn paragraph(&mut self, inlines: &[Inline]) {
        if let Some((alt, url, label)) = sole_image(inlines) {
            self.figure(alt, url, label);
            return;
        }
        let c = runs(inlines);
        if c.is_empty() {
            return;
        }
        self.push(Item::Par { c });
    }

    fn figure(&mut self, alt: &str, url: &str, label: Option<&str>) {
        let Some((src, page)) = self.local_image(url) else {
            return;
        };
        let width = figure_size::probe(&self.base_dir.join(&src), page)
            .map(|source| figure_size::width_fraction_for_path(source, Path::new(&src)));
        let (number, caption) = if alt.is_empty() {
            (String::new(), None)
        } else {
            (self.step_figure(), Some(alt.to_string()))
        };
        self.push(Item::Figure(Figure {
            src,
            page,
            width,
            caption,
            number,
            label: label.map(str::to_string),
        }));
    }

    /// 本地插图的相对路径与 PDF 页码；远程图片、找不到的文件记一条提示后略过。
    fn local_image(&mut self, url: &str) -> Option<(String, Option<u32>)> {
        let (path, page) = match crate::common::docx_image::split_pdf_page(url) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.warnings.push(error.to_string());
                return None;
            }
        };
        if path.starts_with("http://") || path.starts_with("https://") {
            self.warnings.push(format!("不支持远程插图，已略过：{url}"));
            return None;
        }
        let relative = Path::new(path);
        let escapes = relative.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        if escapes || !self.base_dir.join(relative).is_file() {
            self.warnings.push(format!("插图文件不存在，已略过：{url}"));
            return None;
        }
        Some((path.replace('\\', "/"), page))
    }

    fn step_figure(&mut self) -> String {
        if self.free {
            self.counters.free_figure += 1;
            self.counters.free_figure.to_string()
        } else {
            self.counters.figure += 1;
            format!("{}.{}", self.chapter_number(), self.counters.figure)
        }
    }

    fn step_table(&mut self) -> String {
        if self.free {
            self.counters.free_table += 1;
            self.counters.free_table.to_string()
        } else {
            self.counters.table += 1;
            format!("{}.{}", self.chapter_number(), self.counters.table)
        }
    }

    fn step_box(&mut self, name: &str) -> String {
        let index = match self.counters.boxes.iter().position(|(n, _, _)| n == name) {
            Some(index) => index,
            None => {
                self.counters.boxes.push((name.to_string(), 0, 0));
                self.counters.boxes.len() - 1
            }
        };
        let chapter = self.chapter_number();
        let entry = &mut self.counters.boxes[index];
        if self.free {
            entry.2 += 1;
            entry.2.to_string()
        } else {
            entry.1 += 1;
            format!("{chapter}.{}", entry.1)
        }
    }

    fn table(
        &mut self,
        rows: &[Vec<String>],
        spans: &[crate::common::table::TableSpan],
        numbered: bool,
        caption: Option<&str>,
    ) {
        let label = self.pending_label.take();
        if rows.is_empty() {
            return;
        }
        let columns = analyze_table(rows, spans);
        if columns.is_empty() {
            return;
        }
        let cols = columns
            .iter()
            .map(|column| {
                let (kind, v) = match column.width {
                    ColumnWidth::FixedEm(em) => ("em", em),
                    // TeX 写的是一位小数（`X[1.3,l]`），比例按同一精度取。
                    ColumnWidth::Relative(ratio) => ("fr", (ratio * 10.0).round() / 10.0),
                };
                Column {
                    align: column.alignment.latex(),
                    kind,
                    v,
                }
            })
            .collect::<Vec<_>>();
        let grid = rows
            .iter()
            .enumerate()
            .map(|(row, cells)| {
                cells
                    .iter()
                    .enumerate()
                    .map(|(column, text)| {
                        let span = span_at(spans, row, column);
                        if span.is_some_and(|span| !span.is_anchor(row, column)) {
                            return None;
                        }
                        let align = if span.is_some() {
                            cell_alignment(rows, spans, &columns, numbered, row, column)
                        } else if row == 0 {
                            ColumnAlignment::Center
                        } else {
                            columns
                                .get(column)
                                .map_or(ColumnAlignment::Left, |c| c.alignment)
                        };
                        Some(Cell {
                            c: cell_runs(&crate::common::inline::parse(text)),
                            align: align.latex(),
                            colspan: span.map_or(1, |s| s.column_span),
                            rowspan: span.map_or(1, |s| s.row_span),
                        })
                    })
                    .collect()
            })
            .collect();
        let number = self.step_table();
        self.push(Item::Table(Table {
            caption: caption.map(heading_runs),
            number,
            label,
            cols,
            rows: grid,
        }));
    }

    fn quote(&mut self, kind: &QuoteKind, items: &[QuoteItem]) {
        let label = self.pending_label.take();
        match kind {
            QuoteKind::Citation => {
                if items.is_empty() {
                    return;
                }
                let items = quote_lines(items, runs);
                self.push(Item::Quote { items });
            }
            QuoteKind::Box { name, title } => {
                let number = self.step_box(name);
                let items = quote_lines(items, cell_runs);
                self.push(Item::Box {
                    name: name.clone(),
                    number,
                    title: cell_runs(title),
                    items,
                    label,
                });
            }
        }
    }

    // ---------------- 列表 ----------------

    fn list_item(&mut self, level: u8, content: &[Inline]) {
        let c = runs(content);
        if self.in_abstract {
            let label = self.abstract_prefix(level);
            let mut line = vec![Run::S { v: label }];
            line.extend(c);
            self.abstract_items.push(Item::Par { c: line });
            return;
        }
        // 一级条目另起一段；更深的条目接在当前段里，每层各自计数。
        while let Some(&(top, _)) = self.list_stack.last() {
            if top > level {
                self.list_stack.pop();
            } else {
                break;
            }
        }
        if level == 1 {
            self.flush_paragraph_of_list();
        }
        let count = match self.list_stack.last_mut() {
            Some((top, count)) if *top == level => {
                *count += 1;
                *count
            }
            _ => {
                self.list_stack.push((level, 1));
                1
            }
        };
        self.list.push(ListEntry {
            level,
            label: list_label(level, count),
            c,
        });
    }

    /// 一级条目开始时，把上一段（上一个一级条目连同其下级）落下；序号栈保留。
    fn flush_paragraph_of_list(&mut self) {
        if !self.list.is_empty() {
            let items = std::mem::take(&mut self.list);
            self.items.push(Item::List { items });
        }
    }

    fn flush_list(&mut self) {
        self.flush_paragraph_of_list();
        self.list_stack.clear();
        self.abstract_list = [0; 6];
        self.abstract_list_level = 0;
    }

    /// 摘要里的列表前缀：`1.`、`(1)`、`a.`……（原 TeX 输出的口径）。
    fn abstract_prefix(&mut self, level: u8) -> String {
        let index = usize::from(level.clamp(1, 6)) - 1;
        if self.abstract_list_level < level || self.abstract_list_level == 0 {
            for count in &mut self.abstract_list[index..] {
                *count = 0;
            }
        } else {
            for count in &mut self.abstract_list[index + 1..] {
                *count = 0;
            }
        }
        self.abstract_list[index] += 1;
        self.abstract_list_level = level;
        let n = self.abstract_list[index];
        match level {
            1 => format!("{n}. "),
            2 => format!("({n}) "),
            3 => format!("{}. ", (b'a' + ((n - 1) % 26) as u8) as char),
            4 => format!("{}. ", crate::common::numbering::int_to_roman(n)),
            5 => format!("({}) ", (b'A' + ((n - 1) % 26) as u8) as char),
            _ => format!("{n}) "),
        }
    }
}

/// 正文列表各级的序号（`md2tex.cls` 的 `\setdefaultenum`）。
fn list_label(level: u8, n: usize) -> String {
    match level {
        1 => circled(n, "⑴⑵⑶⑷⑸⑹⑺⑻⑼⑽⑾⑿⒀⒁⒂⒃⒄⒅⒆⒇"),
        2 => circled(n, "①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳"),
        3 => format!(
            "({})",
            crate::common::numbering::number_to_uppercase_letter(n)
        ),
        4 => format!(
            "({})",
            crate::common::numbering::number_to_uppercase_letter(n).to_lowercase()
        ),
        5 => format!("{}.", crate::common::numbering::int_to_roman(n)),
        _ => format!(
            "{}.",
            crate::common::numbering::int_to_roman(n).to_lowercase()
        ),
    }
}

/// 前二十个用带圈 / 带括号数字，往后 TeX 印“Error”，这里退成 `(n)`。
fn circled(n: usize, glyphs: &str) -> String {
    glyphs
        .chars()
        .nth(n.wrapping_sub(1))
        .map_or_else(|| format!("({n})"), |ch| ch.to_string())
}

fn quote_lines(items: &[QuoteItem], render: fn(&[Inline]) -> Vec<Run>) -> Vec<QuoteLine> {
    let mut lines = Vec::with_capacity(items.len());
    let mut list_no = 0;
    for item in items {
        if let QuoteItem::Paragraph(inlines) = item {
            if let [Inline::DisplayMath(source)] = inlines.as_slice() {
                list_no = 0;
                lines.push(QuoteLine::Math { v: source.clone() });
                continue;
            }
        }
        match item {
            QuoteItem::Source(inlines) => {
                list_no = 0;
                lines.push(QuoteLine::Source { c: render(inlines) });
            }
            QuoteItem::List { content, .. } => {
                list_no += 1;
                lines.push(QuoteLine::List {
                    label: circled(list_no, "⑴⑵⑶⑷⑸⑹⑺⑻⑼⑽⑾⑿⒀⒁⒂⒃⒄⒅⒆⒇"),
                    c: render(content),
                });
            }
            QuoteItem::Paragraph(inlines) => {
                list_no = 0;
                lines.push(QuoteLine::Par { c: render(inlines) });
            }
        }
    }
    lines
}

fn sole_image(inlines: &[Inline]) -> Option<(&str, &str, Option<&str>)> {
    let meaningful: Vec<&Inline> = inlines
        .iter()
        .filter(|ip| !matches!(ip, Inline::Text(t) if t.trim().is_empty()))
        .collect();
    match meaningful.as_slice() {
        [Inline::Image { alt, url, label }] => Some((alt, url, label.as_deref())),
        _ => None,
    }
}

/// 正文行内片段。
fn runs(inlines: &[Inline]) -> Vec<Run> {
    let mut out = Vec::with_capacity(inlines.len());
    for ip in inlines {
        out.push(match ip {
            Inline::Text(t) => Run::S { v: t.clone() },
            Inline::Bold(children) => Run::B { c: runs(children) },
            Inline::Italic(children) => Run::I { c: runs(children) },
            Inline::Code(t) => Run::Code { v: t.clone() },
            Inline::Link { text, url } => Run::Link {
                v: text.clone(),
                url: url.clone(),
            },
            Inline::Image { url, .. } => {
                let (src, page) = crate::common::docx_image::split_pdf_page(url)
                    .map(|(p, page)| (p.to_string(), page))
                    .unwrap_or_else(|_| (url.clone(), None));
                Run::Img { src, page }
            }
            Inline::CrossRef(id) => Run::Ref { id: id.clone() },
            Inline::Citation(keys) => Run::Cite { keys: keys.clone() },
            Inline::Footnote(t) => Run::Fn { v: t.clone() },
            Inline::Math(t) => Run::Math {
                v: t.clone(),
                d: false,
            },
            Inline::DisplayMath(t) => Run::Math {
                v: t.clone(),
                d: true,
            },
        });
    }
    out
}

/// 表格单元格、文框：脚注降为全角括注，行内图片降为替代文字、链接只留文字。
fn cell_runs(inlines: &[Inline]) -> Vec<Run> {
    inlines
        .iter()
        .map(|ip| match ip {
            Inline::Footnote(t) => Run::S {
                v: format!("（{t}）"),
            },
            Inline::Bold(children) => Run::B {
                c: cell_runs(children),
            },
            Inline::Italic(children) => Run::I {
                c: cell_runs(children),
            },
            Inline::Image { alt, .. } => Run::S { v: alt.clone() },
            other => runs(std::slice::from_ref(other)).remove(0),
        })
        .collect()
}

/// 标题、题注、封面题名：只认行内公式，其余照字面印（与 `heading_latex` 同口径）。
fn heading_runs(text: &str) -> Vec<Run> {
    let inlines = crate::common::inline::parse(text);
    if !inlines.iter().any(|ip| matches!(ip, Inline::Math(_))) {
        return vec![Run::S {
            v: text.to_string(),
        }];
    }
    inlines
        .iter()
        .map(|ip| match ip {
            Inline::Math(source) => Run::Math {
                v: source.clone(),
                d: false,
            },
            other => Run::S {
                v: crate::common::inline::flatten(std::slice::from_ref(other)),
            },
        })
        .collect()
}
