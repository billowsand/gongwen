//! 研究报告 → docx pipeline。
//!
//! 视觉布局对齐 `resources/research/md2tex.cls` 与 `template.tex`：
//! - 第 1 页 封面：左上"公开"（黑体4号）+ vfill + 居中一号小标宋标题 + vfill +
//!   居中三号黑体单位 + 16pt 间距 + 居中四号粗体日期；末尾翻页
//! - 第 2 页 目录：居中二号黑体"目录" + `TableOfContents`(dirty) + 翻页
//! - 之后 版本变更记录（不进 TOC，由 `<!-- [版本变更记录] -->` 触发）
//! - 之后 摘要 / 正文 / 附录，按 `<!-- [...] -->` 标记切换 emitter 模式
//!
//! 章节编号：H2 → "第X章 Y"、H3 → "X.Y Z"、H4 → "X.Y.Z W"，附录章节
//! 切换为 "附录 A / 附录 B / ..."。
//!
//! Heading1/2/3 样式在 styles.xml 中显式注册，并设 outline_lvl 0/1/2，让
//! Word 打开时 `TOC` 字段（`TOC \o "1-3"`）能扫描到全部条目；否则单独给段落
//! 挂 `pStyle="Heading1"` 但 styles.xml 里没有该样式定义，TOC 会是空的。

use anyhow::{Context, Result};
use chrono::{Datelike, Local};
use docx_rs::*;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::common::ast::{Block, Inline, MarkerKind};
use crate::common::front_matter::{self, Metadata};
use crate::common::numbering::{int_to_roman, number_to_uppercase_letter};
use crate::common::table_layout::{analyze_table, to_docx_grid};
use crate::parser;

// ===== 字体（与 LaTeX md2tex.cls 一致；用户须装相应字体，否则 Word 端字体回退） =====
const FONT_TITLE: &str = "FZXiaoBiaoSong-B05"; // 方正小标宋简体
const FONT_HEAD: &str = "FZHei-B01"; // 方正黑体
const FONT_BODY: &str = "FZShuSong-Z01"; // 方正书宋（仿宋类）
const FONT_KAI: &str = "FZKai-Z03"; // 方正楷体（行内强调用）

// ===== 字号（半磅 half-points，1pt = 2hp） =====
// LaTeX 模板中常用字号：
//   一号 = 26pt = 52hp（封面标题）
//   二号 = 22pt = 44hp（章 / 目录标题）
//   三号 = 16pt = 32hp（节标题 / 单位）
//   四号 = 14pt = 28hp（子节标题 / 日期 / "公开"）
//   小四 ≈ 12pt = 24hp
//   normalsize 14bp ≈ 14pt = 28hp（正文）
const SIZE_COVER_TITLE: usize = 52;
const SIZE_HEAD1: usize = 44; // chapter / 摘要 / 附录 / 目录 / 版本变更记录
const SIZE_HEAD2: usize = 32; // section
const SIZE_HEAD3: usize = 28; // subsection / 日期 / 公开
const SIZE_BODY: usize = 28; // 正文，对齐 LaTeX 14bp
const SIZE_COVER_INSTITUTION: usize = 32;
const SIZE_COVER_DATE: usize = 28;

// ===== 行距 / 间距 (单位 twips，1pt = 20twips) =====
// LaTeX 正文 24pt 行距 → 480twips；line_rule=AtLeast 保大字符不溢出
const LINE_BODY: i32 = 480;
const LINE_HEAD: i32 = 600; // 30pt，章标题留白
                            // 段前/段后 (twips)
const SPACING_BEFORE_CHAPTER: u32 = 480;
const SPACING_AFTER_CHAPTER: u32 = 480;
const SPACING_BEFORE_SECTION: u32 = 240;
const SPACING_AFTER_SECTION: u32 = 120;

// ===== 页面边距 (twips, 1mm = 56.6929 twips) =====
// LaTeX geometry: top=37mm, left=28mm, width=156mm, height=225mm（A4 = 210×297mm）
const PAGE_TOP: i32 = 2098; // 37 mm
const PAGE_BOTTOM: i32 = 1984; // 35 mm
const PAGE_LEFT: i32 = 1587; // 28 mm
const PAGE_RIGHT: i32 = 1474; // 26 mm
const MAX_IMAGE_WIDTH_EMU: u32 = 5_600_000; // 约 156 mm，限制在版心内
const MAX_INLINE_IMAGE_WIDTH_EMU: u32 = 1_800_000;
const TABLE_CONTENT_WIDTH_TWIPS: usize = 8_844; // 156 mm

// ===== research 模板的六级列表前缀 =====
const PAREN_CIRCLE_NUMBERS: &[&str] = &[
    "⑴", "⑵", "⑶", "⑷", "⑸", "⑹", "⑺", "⑻", "⑼", "⑽", "⑾", "⑿", "⒀", "⒁", "⒂", "⒃", "⒄", "⒅", "⒆",
    "⒇",
];
const CIRCLE_NUMBERS: &[&str] = &[
    "①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩", "⑪", "⑫", "⑬", "⑭", "⑮", "⑯", "⑰", "⑱", "⑲",
    "⑳",
];

fn font_set(name: &str) -> RunFonts {
    RunFonts::new()
        .ascii(name)
        .hi_ansi(name)
        .east_asia(name)
        .cs(name)
}

/// 入口：研究报告 docx 转换。
pub fn run(input: &Path, output: Option<&Path>) -> Result<()> {
    let input_kind = crate::input::classify(input)?;
    let raw = crate::input::collect_raw(input)?;
    let (metadata, content) = front_matter::parse(&raw);
    let content = crate::input::strip_horizontal_rules(&content);
    let output_path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| crate::input::default_output(input, "docx"));

    println!("正在转换: {}", input.display());

    let image_base_dir = match input_kind {
        crate::input::InputKind::Directory => input.to_path_buf(),
        crate::input::InputKind::File => input.parent().unwrap_or(Path::new(".")).to_path_buf(),
    };

    let blocks = parser::parse(&content);
    let split = split_blocks(&blocks);
    let cover_title = metadata.title.as_deref().or(split.title.as_deref());

    let mut docx = base_docx();
    docx = register_styles(docx);
    docx = add_cover(docx, cover_title, &metadata);
    docx = add_toc(docx);

    if !split.changelog.is_empty() {
        docx = add_changelog(docx, &split.changelog, &image_base_dir);
    }

    let mut emitter = MainEmitter::with_image_base(image_base_dir);
    docx = emitter.emit_all(docx, &split.main);

    if let Some(dir) = output_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("创建输出目录 {} 失败", dir.display()))?;
    }
    let file = File::create(&output_path)
        .with_context(|| format!("创建输出文件 {} 失败", output_path.display()))?;
    docx.build()
        .pack(file)
        .with_context(|| format!("写入 docx {} 失败", output_path.display()))?;

    println!("[完成] 转换完成: {}", output_path.display());
    Ok(())
}

// ============================================================
// 切分：标题 + 版本变更记录 + 主体
// ============================================================

struct SplitBlocks {
    title: Option<String>,
    changelog: Vec<Block>,
    main: Vec<Block>,
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum Bucket {
    Main,
    Changelog,
}

fn split_blocks(blocks: &[Block]) -> SplitBlocks {
    let mut title: Option<String> = None;
    let mut changelog = Vec::new();
    let mut main = Vec::new();
    let mut bucket = Bucket::Main;

    for b in blocks {
        match b {
            Block::Heading { level: 1, text } if title.is_none() => {
                title = Some(text.clone());
            }
            Block::Marker(MarkerKind::Changelog) => {
                bucket = Bucket::Changelog;
            }
            Block::Marker(MarkerKind::Body)
            | Block::Marker(MarkerKind::Abstract)
            | Block::Marker(MarkerKind::Appendix)
            | Block::Marker(MarkerKind::Reference) => {
                bucket = Bucket::Main;
                main.push(b.clone());
            }
            _ => match bucket {
                Bucket::Main => main.push(b.clone()),
                Bucket::Changelog => changelog.push(b.clone()),
            },
        }
    }
    SplitBlocks {
        title,
        changelog,
        main,
    }
}

// ============================================================
// 文档默认 / 页面 / 样式
// ============================================================

fn base_docx() -> Docx {
    Docx::new()
        .page_margin(
            PageMargin::new()
                .top(PAGE_TOP)
                .bottom(PAGE_BOTTOM)
                .left(PAGE_LEFT)
                .right(PAGE_RIGHT),
        )
        .default_fonts(font_set(FONT_BODY))
        .default_size(SIZE_BODY)
        .default_line_spacing(
            LineSpacing::new()
                .line(LINE_BODY)
                .line_rule(LineSpacingType::AtLeast),
        )
}

/// 注册 Heading1/2/3 样式到 styles.xml。
///
/// `TOC \o "1-3"` 字段会扫描 styleId 为 Heading1/2/3 的段落，或 outlineLvl
/// 为 0/1/2 的段落。两者同时设置最稳妥。
fn register_styles(docx: Docx) -> Docx {
    let h1 = Style::new("Heading1", StyleType::Paragraph)
        .name("heading 1")
        .based_on("Normal")
        .next("Normal")
        .fonts(font_set(FONT_HEAD))
        .size(SIZE_HEAD1)
        .bold()
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .before(SPACING_BEFORE_CHAPTER)
                .after(SPACING_AFTER_CHAPTER)
                .line(LINE_HEAD)
                .line_rule(LineSpacingType::AtLeast),
        )
        .outline_lvl(0)
        .ui_priority(9)
        .q_format(true);

    let h2 = Style::new("Heading2", StyleType::Paragraph)
        .name("heading 2")
        .based_on("Normal")
        .next("Normal")
        .fonts(font_set(FONT_HEAD))
        .size(SIZE_HEAD2)
        .bold()
        .align(AlignmentType::Left)
        // titlespacing{\section}{2em}{0pt}{0pt}：左缩进 2em
        .indent(None, None, None, Some(200))
        .line_spacing(
            LineSpacing::new()
                .before(SPACING_BEFORE_SECTION)
                .after(SPACING_AFTER_SECTION)
                .line(LINE_BODY)
                .line_rule(LineSpacingType::AtLeast),
        )
        .outline_lvl(1)
        .ui_priority(9)
        .q_format(true);

    let h3 = Style::new("Heading3", StyleType::Paragraph)
        .name("heading 3")
        .based_on("Normal")
        .next("Normal")
        .fonts(font_set(FONT_HEAD))
        .size(SIZE_HEAD3)
        .bold()
        .align(AlignmentType::Left)
        .indent(None, None, None, Some(200))
        .line_spacing(
            LineSpacing::new()
                .before(SPACING_BEFORE_SECTION)
                .after(SPACING_AFTER_SECTION)
                .line(LINE_BODY)
                .line_rule(LineSpacingType::AtLeast),
        )
        .outline_lvl(2)
        .ui_priority(9)
        .q_format(true);

    docx.add_style(h1).add_style(h2).add_style(h3)
}

// ============================================================
// 封面
// ============================================================

fn add_cover(mut docx: Docx, title: Option<&str>, metadata: &Metadata) -> Docx {
    let title_text = title.unwrap_or("研究报告");
    let security = match (
        metadata.security.as_deref().unwrap_or("公开"),
        metadata.security_years.as_deref(),
    ) {
        (security, Some(years)) => format!("{}★{}", security, years),
        (security, None) => security.to_string(),
    };
    let doc_type = metadata.doc_type.as_deref().unwrap_or("研究报告");
    let version = metadata.version.as_deref().unwrap_or("V1.0");
    let institution = metadata.institution.as_deref().unwrap_or("某某单位");
    let date = metadata
        .date
        .as_deref()
        .map(front_matter::normalize_date)
        .unwrap_or_else(|| {
            let now = Local::now();
            format!("{} 年 {} 月", now.year(), now.month())
        });

    // 顶部信息栏：密级/年限在左，编号在右。
    let mut top = Paragraph::new()
        .align(AlignmentType::Left)
        .add_tab(Tab::new().val(TabValueType::Right).pos(8800))
        .line_spacing(
            LineSpacing::new()
                .line(LINE_BODY)
                .line_rule(LineSpacingType::AtLeast),
        )
        .add_run(
            Run::new()
                .add_text(format!("密级：{}", security))
                .fonts(font_set(FONT_HEAD))
                .size(SIZE_BODY)
                .bold(),
        );
    if let Some(number) = metadata.doc_number.as_deref() {
        top = top.add_run(Run::new().add_tab()).add_run(
            Run::new()
                .add_text(format!("编号：{}", number))
                .fonts(font_set(FONT_HEAD))
                .size(SIZE_BODY)
                .bold(),
        );
    }
    docx = docx.add_paragraph(top);

    // 版本行下方用细线收束顶部信息栏。
    let mut version_line = Paragraph::new()
        .align(AlignmentType::Left)
        .line_spacing(
            LineSpacing::new()
                .after(80)
                .line(LINE_BODY)
                .line_rule(LineSpacingType::AtLeast),
        )
        .add_run(
            Run::new()
                .add_text(format!("版本：{}", version))
                .fonts(font_set(FONT_HEAD))
                .size(SIZE_BODY)
                .bold(),
        );
    version_line.property = version_line.property.clone().set_borders(
        ParagraphBorders::with_empty()
            .set(ParagraphBorder::new(ParagraphBorderPosition::Bottom).size(6)),
    );
    docx = docx.add_paragraph(version_line);

    // 文件类型：标题上方的封面角色标识。
    docx = docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(
                LineSpacing::new()
                    .before(1200)
                    .line(LINE_HEAD)
                    .line_rule(LineSpacingType::AtLeast),
            )
            .add_run(
                Run::new()
                    .add_text(doc_type)
                    .fonts(font_set(FONT_HEAD))
                    .size(SIZE_HEAD1)
                    .bold(),
            ),
    );

    // 标题（一号小标宋居中）
    docx = docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(
                LineSpacing::new()
                    .before(560)
                    .after(640)
                    .line(800)
                    .line_rule(LineSpacingType::AtLeast),
            )
            .add_run(
                Run::new()
                    .add_text(title_text)
                    .fonts(font_set(FONT_TITLE))
                    .size(SIZE_COVER_TITLE)
                    .bold(),
            ),
    );

    // 落款单位（黑体三号居中）。
    docx = docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(
                LineSpacing::new()
                    .before(4200)
                    .after(320)
                    .line(LINE_BODY)
                    .line_rule(LineSpacingType::AtLeast),
            )
            .add_run(
                Run::new()
                    .add_text(institution)
                    .fonts(font_set(FONT_HEAD))
                    .size(SIZE_COVER_INSTITUTION)
                    .bold(),
            ),
    );

    // 日期（四号书宋，统一到年月）。
    docx = docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(
                LineSpacing::new()
                    .line(LINE_BODY)
                    .line_rule(LineSpacingType::AtLeast),
            )
            .add_run(
                Run::new()
                    .add_text(&date)
                    .fonts(font_set(FONT_BODY))
                    .size(SIZE_COVER_DATE),
            ),
    );

    // 封面后强制翻页
    docx.add_paragraph(Paragraph::new().page_break_before(true))
}

// ============================================================
// 目录
// ============================================================

fn add_toc(mut docx: Docx) -> Docx {
    // "目录"标题（不带 Heading 样式，避免自引用）
    docx = docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(
                LineSpacing::new()
                    .before(0)
                    .after(SPACING_AFTER_CHAPTER)
                    .line(LINE_HEAD)
                    .line_rule(LineSpacingType::AtLeast),
            )
            .add_run(
                Run::new()
                    .add_text("目  录")
                    .fonts(font_set(FONT_HEAD))
                    .size(SIZE_HEAD1)
                    .bold(),
            ),
    );

    docx = docx.add_table_of_contents(TableOfContents::new().heading_styles_range(1, 3).dirty());

    // 目录后翻页
    docx.add_paragraph(Paragraph::new().page_break_before(true))
}

// ============================================================
// 版本变更记录
// ============================================================

fn add_changelog(mut docx: Docx, blocks: &[Block], image_base_dir: &Path) -> Docx {
    docx = docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .line_spacing(
                LineSpacing::new()
                    .before(0)
                    .after(SPACING_AFTER_CHAPTER)
                    .line(LINE_HEAD)
                    .line_rule(LineSpacingType::AtLeast),
            )
            .add_run(
                Run::new()
                    .add_text("版本变更记录")
                    .fonts(font_set(FONT_HEAD))
                    .size(SIZE_HEAD1)
                    .bold(),
            ),
    );

    let mut emitter = ChangelogEmitter::with_image_base(image_base_dir.to_path_buf());
    let mut skipped_repeated_title = false;
    for b in blocks {
        if !skipped_repeated_title
            && matches!(b, Block::Heading { level: 1, text } if text.trim() == "版本变更记录")
        {
            skipped_repeated_title = true;
            continue;
        }
        docx = emitter.emit(docx, b);
    }
    docx.add_paragraph(Paragraph::new().page_break_before(true))
}

// ============================================================
// 主 emitter（含模式：Body / Abstract / Appendix）
// ============================================================

#[derive(Copy, Clone, Eq, PartialEq)]
enum Mode {
    Body,
    Abstract,
    Appendix,
    Reference,
}

struct MainEmitter {
    mode: Mode,
    chapter: usize,
    section: usize,
    subsection: usize,
    appendix_idx: usize, // 0..N，对应附录 A..
    ref_counter: usize,  // 参考文献条目计数器 [1] [2] ...
    list: ListState,
    appendix_saw_h1: bool,
    suppress_next_heading: Option<&'static str>,
    table_counter: usize,
    figure_counter: usize,
    image_base_dir: PathBuf,
}

#[derive(Default)]
struct ListState {
    in_list: bool,
    level: u8,
    l1: usize,
    l2: usize,
    l3: usize,
    l4: usize,
    l5: usize,
    l6: usize,
}

impl MainEmitter {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_image_base(PathBuf::from("."))
    }

    fn with_image_base(image_base_dir: PathBuf) -> Self {
        Self {
            mode: Mode::Body,
            chapter: 0,
            section: 0,
            subsection: 0,
            appendix_idx: 0,
            ref_counter: 0,
            list: ListState::default(),
            appendix_saw_h1: false,
            suppress_next_heading: None,
            table_counter: 0,
            figure_counter: 0,
            image_base_dir,
        }
    }

    fn emit_all(&mut self, mut docx: Docx, blocks: &[Block]) -> Docx {
        for b in blocks {
            docx = self.emit(docx, b);
        }
        docx
    }

    fn emit(&mut self, docx: Docx, b: &Block) -> Docx {
        match b {
            Block::Marker(kind) => {
                self.list.reset();
                self.handle_marker(*kind);
                if matches!(kind, MarkerKind::Abstract) {
                    self.suppress_next_heading = Some("摘要");
                    return self.emit_abstract_header(docx);
                }
                if matches!(kind, MarkerKind::Reference) {
                    self.suppress_next_heading = Some("参考文献");
                    return self.emit_reference_header(page_break(docx));
                }
                docx
            }
            Block::Heading { level, text } => {
                self.list.reset();
                if self
                    .suppress_next_heading
                    .take()
                    .is_some_and(|expected| *level == 1 && text.trim() == expected)
                {
                    return docx;
                }
                self.emit_heading(docx, *level, text)
            }
            Block::Paragraph(inlines) => {
                self.list.reset();
                if let Some((alt, url)) = sole_image(inlines) {
                    self.add_figure(docx, alt, url)
                } else {
                    let base_dir = &self.image_base_dir;
                    add_body_paragraph(docx, |p| add_inlines(p, inlines, base_dir))
                }
            }
            Block::List {
                ordered: _,
                level,
                content,
            } => {
                let prefix = if self.mode == Mode::Reference && *level == 1 {
                    self.ref_counter += 1;
                    format!("[{}] ", self.ref_counter)
                } else {
                    self.list.next_prefix(*level)
                };
                add_list_paragraph(docx, *level, &prefix, content, &self.image_base_dir)
            }
            Block::Table { rows, caption } => {
                self.list.reset();
                let docx = if let Some(caption) = caption {
                    self.table_counter += 1;
                    let caption =
                        format!("表 {} {}", self.object_number(self.table_counter), caption);
                    add_table_caption(docx, &caption)
                } else {
                    docx
                };
                add_table(docx, rows)
            }
            Block::CodeBlock { content, .. } => {
                self.list.reset();
                add_code_block(docx, content)
            }
            Block::Empty => docx,
            Block::Label(_) => {
                // docx 暂不支持交叉引用锚点，忽略
                docx
            }
        }
    }

    fn handle_marker(&mut self, kind: MarkerKind) {
        match kind {
            MarkerKind::Abstract => self.mode = Mode::Abstract,
            MarkerKind::Appendix => {
                self.mode = Mode::Appendix;
                self.appendix_idx = 0;
                self.appendix_saw_h1 = false;
                self.table_counter = 0;
                self.figure_counter = 0;
            }
            MarkerKind::Body => {
                self.mode = Mode::Body;
                self.chapter = 0;
                self.section = 0;
                self.subsection = 0;
                self.table_counter = 0;
                self.figure_counter = 0;
            }
            MarkerKind::Reference => {
                self.mode = Mode::Reference;
                self.appendix_idx = 0;
                self.ref_counter = 0;
            }
            MarkerKind::Changelog => {
                self.mode = Mode::Body;
            }
        }
    }

    /// 摘要标题：与章同级（出现在 TOC 中），但文字固定为"摘要"。
    fn emit_abstract_header(&mut self, docx: Docx) -> Docx {
        docx.add_paragraph(heading_chapter_paragraph("摘要"))
    }

    /// 参考文献标题：与章同级（出现在 TOC 中），文字固定为"参考文献"。
    fn emit_reference_header(&mut self, docx: Docx) -> Docx {
        docx.add_paragraph(heading_chapter_paragraph("参考文献"))
    }

    fn emit_heading(&mut self, mut docx: Docx, level: u8, text: &str) -> Docx {
        if self.mode == Mode::Appendix {
            if level == 1 {
                self.appendix_saw_h1 = true;
            }
            return match (level, self.appendix_saw_h1) {
                (1, _) | (2, false) => {
                    self.appendix_idx += 1;
                    self.table_counter = 0;
                    self.figure_counter = 0;
                    let letter = (b'A' + (self.appendix_idx - 1) as u8) as char;
                    let label = format!("附录 {} {}", letter, text);
                    docx = page_break(docx);
                    docx.add_paragraph(heading_chapter_paragraph(&label))
                }
                (2, true) | (3, false) => docx.add_paragraph(heading_section_paragraph(text)),
                (3, true) | (4, false) => docx.add_paragraph(heading_subsection_paragraph(text)),
                _ => add_body_paragraph(docx, |p| {
                    p.align(AlignmentType::Left).add_run(
                        Run::new()
                            .add_text(text)
                            .fonts(font_set(FONT_HEAD))
                            .size(SIZE_BODY)
                            .bold(),
                    )
                }),
            };
        }

        match (level, self.mode) {
            (2, Mode::Body) => {
                self.chapter += 1;
                self.section = 0;
                self.subsection = 0;
                self.table_counter = 0;
                self.figure_counter = 0;
                let label = format!("第{}章 {}", chinese_chapter(self.chapter), text);
                docx = page_break(docx);
                docx.add_paragraph(heading_chapter_paragraph(&label))
            }
            (3, Mode::Body) => {
                self.section += 1;
                self.subsection = 0;
                let label = format!("{}.{} {}", self.chapter, self.section, text);
                docx.add_paragraph(heading_section_paragraph(&label))
            }
            (4, Mode::Body) => {
                self.subsection += 1;
                let label = format!(
                    "{}.{}.{} {}",
                    self.chapter, self.section, self.subsection, text
                );
                docx.add_paragraph(heading_subsection_paragraph(&label))
            }
            (2, Mode::Reference) => docx.add_paragraph(heading_section_paragraph(text)),
            (3, Mode::Reference) => docx.add_paragraph(heading_subsection_paragraph(text)),
            (_, Mode::Abstract) => {
                // 摘要里出现的子标题降级为加粗居中段落
                add_body_paragraph(docx, |p| {
                    p.align(AlignmentType::Center).add_run(
                        Run::new()
                            .add_text(text.to_string())
                            .fonts(font_set(FONT_HEAD))
                            .size(SIZE_HEAD2)
                            .bold(),
                    )
                })
            }
            (1, _) => {
                // 第二个 H1（在主文中再次出现）退化为加粗居中段落
                add_body_paragraph(docx, |p| {
                    p.align(AlignmentType::Center).add_run(
                        Run::new()
                            .add_text(text.to_string())
                            .fonts(font_set(FONT_TITLE))
                            .size(SIZE_HEAD1)
                            .bold(),
                    )
                })
            }
            _ => docx,
        }
    }

    fn object_number(&self, counter: usize) -> String {
        match self.mode {
            Mode::Appendix if self.appendix_idx > 0 => {
                format!(
                    "{}.{}",
                    number_to_uppercase_letter(self.appendix_idx),
                    counter
                )
            }
            Mode::Body if self.chapter > 0 => format!("{}.{}", self.chapter, counter),
            _ => counter.to_string(),
        }
    }

    fn add_figure(&mut self, docx: Docx, alt: &str, url: &str) -> Docx {
        match crate::common::docx_image::load(url, &self.image_base_dir, MAX_IMAGE_WIDTH_EMU) {
            Ok(pic) => {
                let has_caption = !alt.trim().is_empty();
                let figure = Paragraph::new()
                    .align(AlignmentType::Center)
                    .keep_next(has_caption)
                    .add_run(Run::new().add_image(pic));
                let docx = docx.add_paragraph(figure);
                if has_caption {
                    self.figure_counter += 1;
                    let caption = format!(
                        "图 {} {}",
                        self.object_number(self.figure_counter),
                        alt.trim()
                    );
                    add_figure_caption(docx, &caption)
                } else {
                    docx
                }
            }
            Err(error) => {
                eprintln!("  警告：{error:#}");
                add_image_error_paragraph(docx, alt, url)
            }
        }
    }
}

// ============================================================
// 版本变更记录的简化 emitter（无章节计数 / 无模式）
// ============================================================

struct ChangelogEmitter {
    list: ListState,
    table_counter: usize,
    image_base_dir: PathBuf,
}

impl ChangelogEmitter {
    fn with_image_base(image_base_dir: PathBuf) -> Self {
        Self {
            list: ListState::default(),
            table_counter: 0,
            image_base_dir,
        }
    }

    fn emit(&mut self, docx: Docx, b: &Block) -> Docx {
        match b {
            Block::Heading { level, text } => {
                self.list.reset();
                let size = match level {
                    1 | 2 => SIZE_HEAD2,
                    3 => SIZE_HEAD3,
                    _ => SIZE_BODY,
                };
                add_body_paragraph(docx, |p| {
                    p.align(AlignmentType::Left).add_run(
                        Run::new()
                            .add_text(text.to_string())
                            .fonts(font_set(FONT_HEAD))
                            .size(size)
                            .bold(),
                    )
                })
            }
            Block::Paragraph(inlines) => {
                self.list.reset();
                add_body_paragraph(docx, |p| add_inlines(p, inlines, &self.image_base_dir))
            }
            Block::List {
                ordered: _,
                level,
                content,
            } => {
                let prefix = self.list.next_prefix(*level);
                add_list_paragraph(docx, *level, &prefix, content, &self.image_base_dir)
            }
            Block::Table { rows, caption } => {
                self.list.reset();
                let docx = if let Some(caption) = caption {
                    self.table_counter += 1;
                    add_table_caption(docx, &format!("表 {} {}", self.table_counter, caption))
                } else {
                    docx
                };
                add_table(docx, rows)
            }
            Block::Marker(_) | Block::Empty | Block::CodeBlock { .. } | Block::Label(_) => docx,
        }
    }
}

// ============================================================
// 列表前缀状态机（对齐 research LaTeX 模板）
// ============================================================

impl ListState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn next_prefix(&mut self, level: u8) -> String {
        let prefix = match level {
            1 => {
                if !self.in_list || self.level > 1 {
                    self.l2 = 0;
                    self.l3 = 0;
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                if !self.in_list {
                    self.l1 = 0;
                }
                self.l1 += 1;
                PAREN_CIRCLE_NUMBERS
                    .get(self.l1 - 1)
                    .map(|prefix| format!("{} ", prefix))
                    .unwrap_or_else(|| format!("({}) ", self.l1))
            }
            2 => {
                if !self.in_list || self.level != 2 {
                    if self.level > 2 {
                        self.l3 = 0;
                        self.l4 = 0;
                        self.l5 = 0;
                        self.l6 = 0;
                    }
                    if !self.in_list || self.level < 2 {
                        self.l2 = 0;
                        self.l3 = 0;
                        self.l4 = 0;
                        self.l5 = 0;
                        self.l6 = 0;
                    }
                }
                self.l2 += 1;
                CIRCLE_NUMBERS
                    .get(self.l2 - 1)
                    .map(|prefix| format!("{} ", prefix))
                    .unwrap_or_else(|| format!("({}) ", self.l2))
            }
            3 => {
                if !self.in_list || self.level < 3 {
                    self.l3 = 0;
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l3 += 1;
                format!("({}) ", number_to_uppercase_letter(self.l3))
            }
            4 => {
                if !self.in_list || self.level < 4 {
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l4 += 1;
                let ch = (b'a' + ((self.l4 - 1) % 26) as u8) as char;
                format!("({}) ", ch)
            }
            5 => {
                if !self.in_list || self.level < 5 {
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l5 += 1;
                format!("{}. ", int_to_roman(self.l5))
            }
            6 => {
                if !self.in_list || self.level < 6 {
                    self.l6 = 0;
                }
                self.l6 += 1;
                format!("{}. ", int_to_roman(self.l6).to_lowercase())
            }
            _ => String::new(),
        };
        self.in_list = true;
        self.level = level;
        prefix
    }
}

// ============================================================
// 段落构造工具
// ============================================================

/// 章 / 摘要 / 附录 / 目录标题：用 Heading1 样式，居中、二号黑体。
fn heading_chapter_paragraph(text: &str) -> Paragraph {
    Paragraph::new()
        .style("Heading1")
        .align(AlignmentType::Center)
        .add_run(
            Run::new()
                .add_text(text)
                .fonts(font_set(FONT_HEAD))
                .size(SIZE_HEAD1)
                .bold(),
        )
}

/// 节标题：用 Heading2 样式，左缩进 2 字符、三号黑体。
fn heading_section_paragraph(text: &str) -> Paragraph {
    Paragraph::new()
        .style("Heading2")
        .align(AlignmentType::Left)
        .indent(None, None, None, Some(200))
        .add_run(
            Run::new()
                .add_text(text)
                .fonts(font_set(FONT_HEAD))
                .size(SIZE_HEAD2)
                .bold(),
        )
}

/// 子节标题：用 Heading3 样式，左缩进 2 字符、四号黑体。
fn heading_subsection_paragraph(text: &str) -> Paragraph {
    Paragraph::new()
        .style("Heading3")
        .align(AlignmentType::Left)
        .indent(None, None, None, Some(200))
        .add_run(
            Run::new()
                .add_text(text)
                .fonts(font_set(FONT_HEAD))
                .size(SIZE_HEAD3)
                .bold(),
        )
}

/// 普通正文段落：两端对齐、首行缩进 2 字符、AtLeast 24pt 行距。
fn add_body_paragraph<F>(docx: Docx, build: F) -> Docx
where
    F: FnOnce(Paragraph) -> Paragraph,
{
    // 首行缩进 2 字符 ≈ 2×14pt = 560 twips（与 ctex 的 \parindent=2em 等效）
    let p = Paragraph::new()
        .align(AlignmentType::Both)
        .indent(Some(0), Some(SpecialIndentType::FirstLine(560)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(LINE_BODY)
                .line_rule(LineSpacingType::AtLeast),
        );
    docx.add_paragraph(build(p))
}

/// 列表段落：左侧缩进为 0，特殊格式为首行缩进两个汉字。
fn add_list_paragraph(
    docx: Docx,
    _level: u8,
    prefix: &str,
    content: &[Inline],
    image_base_dir: &Path,
) -> Docx {
    // 正文为 14pt，一个汉字约 280 twips。列表左侧缩进固定为 0，特殊格式使用
    // 560 twips 的首行缩进，即两个正文汉字。
    const FIRST_LINE_INDENT: i32 = 560;
    let p = Paragraph::new()
        .align(AlignmentType::Both)
        .indent(
            Some(0),
            Some(SpecialIndentType::FirstLine(FIRST_LINE_INDENT)),
            None,
            None,
        )
        .line_spacing(
            LineSpacing::new()
                .line(LINE_BODY)
                .line_rule(LineSpacingType::AtLeast),
        );
    let p = p.add_run(
        Run::new()
            .add_text(prefix.to_string())
            .fonts(font_set(FONT_BODY))
            .size(SIZE_BODY),
    );
    let p = add_inlines(p, content, image_base_dir);
    docx.add_paragraph(p)
}

fn inline_run_style(ip: &Inline) -> (String, bool, bool) {
    match ip {
        Inline::Text(t) => (t.clone(), false, false),
        // docx 简化处理：粗体 / 斜体的子节点降级为纯文本（嵌套格式丢失）
        Inline::Bold(children) => (crate::common::inline::flatten(children), true, false),
        Inline::Italic(children) => (crate::common::inline::flatten(children), false, true),
        Inline::Code(t) => (t.clone(), false, false),
        Inline::Link { text, .. } => (text.clone(), false, false),
        // docx 暂不支持插图，降级为替代文本
        Inline::Image { alt, .. } => (alt.clone(), false, false),
        // docx 暂不支持交叉引用，降级为 id 文本
        Inline::CrossRef(id) => (id.clone(), false, false),
        // docx 暂不生成原生文献引用，保留 Pandoc 方括号标记
        Inline::Citation(keys) => (
            format!(
                "[{}]",
                keys.iter()
                    .map(|key| format!("@{key}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            false,
            false,
        ),
        // docx 暂不生成脚注部件，降级为全角括号内联注释
        Inline::Footnote(t) => (format!("（{}）", t), false, false),
    }
}

fn add_inlines(mut p: Paragraph, inlines: &[Inline], image_base_dir: &Path) -> Paragraph {
    for ip in inlines {
        if let Inline::Image { alt, url, .. } = ip {
            match crate::common::docx_image::load(url, image_base_dir, MAX_INLINE_IMAGE_WIDTH_EMU) {
                Ok(pic) => p = p.add_run(Run::new().add_image(pic)),
                Err(error) => {
                    eprintln!("  警告：{error:#}");
                    p = p.add_run(
                        Run::new()
                            .add_text(image_error_text(alt, url))
                            .fonts(font_set(FONT_BODY))
                            .size(SIZE_BODY),
                    );
                }
            }
            continue;
        }
        let (text, bold, italic) = inline_run_style(ip);
        let mut run = Run::new().add_text(&text).size(SIZE_BODY);
        // 斜体在中文里用楷体表达（与 LaTeX `ItalicFont={FZKai-Z03}` 一致）
        if italic {
            run = run.fonts(font_set(FONT_KAI)).italic();
        } else {
            run = run.fonts(font_set(FONT_BODY));
        }
        if bold {
            run = run.bold();
        }
        p = p.add_run(run);
    }
    p
}

fn sole_image(inlines: &[Inline]) -> Option<(&str, &str)> {
    let meaningful: Vec<&Inline> = inlines
        .iter()
        .filter(|inline| !matches!(inline, Inline::Text(text) if text.trim().is_empty()))
        .collect();
    match meaningful.as_slice() {
        [Inline::Image { alt, url, .. }] => Some((alt.as_str(), url.as_str())),
        _ => None,
    }
}

fn image_error_text(alt: &str, url: &str) -> String {
    let label = if alt.trim().is_empty() {
        url
    } else {
        alt.trim()
    };
    format!("[图片加载失败：{label}]")
}

fn add_image_error_paragraph(docx: Docx, alt: &str, url: &str) -> Docx {
    docx.add_paragraph(
        Paragraph::new().align(AlignmentType::Center).add_run(
            Run::new()
                .add_text(image_error_text(alt, url))
                .fonts(font_set(FONT_BODY))
                .size(SIZE_BODY),
        ),
    )
}

fn page_break(docx: Docx) -> Docx {
    docx.add_paragraph(Paragraph::new().page_break_before(true))
}

fn add_table_caption(docx: Docx, caption: &str) -> Docx {
    docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .keep_next(true)
            .line_spacing(
                LineSpacing::new()
                    .before(120)
                    .after(120)
                    .line(LINE_BODY)
                    .line_rule(LineSpacingType::AtLeast),
            )
            .add_run(
                Run::new()
                    .add_text(caption)
                    .fonts(font_set(FONT_BODY))
                    .size(SIZE_BODY),
            ),
    )
}

fn add_figure_caption(docx: Docx, caption: &str) -> Docx {
    add_table_caption(docx, caption)
}

fn add_code_block(mut docx: Docx, content: &str) -> Docx {
    for line in content.lines().filter(|line| !line.is_empty()) {
        docx = add_body_paragraph(docx, |p| {
            p.indent(Some(560), None, None, None).add_run(
                Run::new()
                    .add_text(line)
                    .fonts(font_set(FONT_BODY))
                    .size(SIZE_BODY),
            )
        });
    }
    docx
}

fn add_table(docx: Docx, rows: &[Vec<String>]) -> Docx {
    if rows.is_empty() {
        return docx;
    }
    let column_layout = analyze_table(rows);
    let max_cols = column_layout.len();
    if max_cols == 0 {
        return docx;
    }
    let grid = to_docx_grid(&column_layout, TABLE_CONTENT_WIDTH_TWIPS, SIZE_BODY * 10);

    let mut table_rows = Vec::new();
    for (row_idx, row) in rows.iter().enumerate() {
        let mut cells = Vec::new();
        for (col_idx, &column_width) in grid.iter().enumerate() {
            let cell_data = row.get(col_idx).map(String::as_str).unwrap_or("");
            let align = AlignmentType::Center;
            // 表头整行黑体加粗；表体解析 cell 内的 **加粗**/*斜体* 行内格式。
            let is_header = row_idx == 0;
            let mut p = Paragraph::new().align(align);
            for ip in crate::common::inline::parse(cell_data) {
                let (text, bold, italic) = inline_run_style(&ip);
                let mut run = Run::new().add_text(&text).size(SIZE_BODY);
                if is_header {
                    run = run.fonts(font_set(FONT_HEAD)).bold();
                } else if italic {
                    // 斜体在中文里用楷体表达（与正文 add_inlines 一致）
                    run = run.fonts(font_set(FONT_KAI)).italic();
                } else {
                    run = run.fonts(font_set(FONT_BODY));
                }
                if bold && !is_header {
                    run = run.bold();
                }
                p = p.add_run(run);
            }
            cells.push(
                TableCell::new()
                    .width(column_width, WidthType::Dxa)
                    .add_paragraph(p),
            );
        }
        table_rows.push(TableRow::new(cells));
    }

    let table_width = grid.iter().sum();
    let table = Table::new(table_rows)
        .set_grid(grid)
        .width(table_width, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .set_borders(
            TableBorders::new()
                .set(TableBorder::new(TableBorderPosition::Top).size(4))
                .set(TableBorder::new(TableBorderPosition::Left).size(4))
                .set(TableBorder::new(TableBorderPosition::Bottom).size(4))
                .set(TableBorder::new(TableBorderPosition::Right).size(4))
                .set(TableBorder::new(TableBorderPosition::InsideH).size(4))
                .set(TableBorder::new(TableBorderPosition::InsideV).size(4)),
        );
    docx.add_table(table)
}

// ============================================================
// 中文章节序号
// ============================================================

fn chinese_chapter(num: usize) -> String {
    use crate::common::numbering::number_to_chinese;
    number_to_chinese(num)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ast::Inline;

    fn paragraph_texts(docx: &Docx) -> Vec<String> {
        docx.document
            .children
            .iter()
            .filter_map(|child| match child {
                DocumentChild::Paragraph(paragraph) => Some(paragraph.raw_text()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn split_extracts_title_and_changelog() {
        let blocks = vec![
            Block::Heading {
                level: 1,
                text: "测试报告".into(),
            },
            Block::Marker(MarkerKind::Changelog),
            Block::Paragraph(vec![Inline::Text("v1.0 初版".into())]),
            Block::Marker(MarkerKind::Body),
            Block::Heading {
                level: 2,
                text: "引言".into(),
            },
            Block::Paragraph(vec![Inline::Text("正文".into())]),
        ];
        let split = split_blocks(&blocks);
        assert_eq!(split.title.as_deref(), Some("测试报告"));
        assert_eq!(split.changelog.len(), 1);
        assert!(split
            .main
            .iter()
            .any(|b| matches!(b, Block::Heading { level: 2, .. })));
    }

    #[test]
    fn main_emitter_numbers_chapters() {
        let mut e = MainEmitter::new();
        let docx = Docx::new();
        let docx = e.emit(
            docx,
            &Block::Heading {
                level: 2,
                text: "引言".into(),
            },
        );
        let docx = e.emit(
            docx,
            &Block::Heading {
                level: 3,
                text: "背景".into(),
            },
        );
        let _ = e.emit(
            docx,
            &Block::Heading {
                level: 4,
                text: "动机".into(),
            },
        );
        assert_eq!(e.chapter, 1);
        assert_eq!(e.section, 1);
        assert_eq!(e.subsection, 1);
    }

    #[test]
    fn main_emitter_appendix_letters() {
        let mut e = MainEmitter::new();
        e.handle_marker(MarkerKind::Appendix);
        let docx = Docx::new();
        let docx = e.emit(
            docx,
            &Block::Heading {
                level: 2,
                text: "数据集".into(),
            },
        );
        let _ = e.emit(
            docx,
            &Block::Heading {
                level: 2,
                text: "代码".into(),
            },
        );
        assert_eq!(e.appendix_idx, 2);
    }

    #[test]
    fn docx_fallback_reconstructs_citation_source() {
        let (text, bold, italic) =
            inline_run_style(&Inline::Citation(vec!["a".into(), "b".into()]));
        assert_eq!(text, "[@a; @b]");
        assert!(!bold);
        assert!(!italic);
    }

    #[test]
    fn cover_uses_front_matter_fields() {
        let metadata = Metadata {
            security: Some("机密".into()),
            security_years: Some("5年".into()),
            doc_type: Some("技术报告".into()),
            doc_number: Some("XX-2026-001".into()),
            version: Some("V2.1".into()),
            institution: Some("某研究所".into()),
            date: Some("2026-07".into()),
            title: Some("系统报告".into()),
            bibliography: None,
        };
        let docx = add_cover(Docx::new(), metadata.title.as_deref(), &metadata);
        let text = paragraph_texts(&docx).join("\n");

        for expected in [
            "密级：机密★5年",
            "编号：XX-2026-001",
            "版本：V2.1",
            "技术报告",
            "系统报告",
            "某研究所",
            "2026 年 7 月",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }
    }

    #[test]
    fn section_markers_avoid_duplicate_titles_and_support_h1_appendix() {
        let blocks = parser::parse(
            "<!-- [参考文献] -->\n# 参考文献\n- 条目\n<!-- [附录] -->\n# 数据集\n## 说明\n",
        );
        let mut emitter = MainEmitter::new();
        let docx = emitter.emit_all(Docx::new(), &blocks);
        let texts = paragraph_texts(&docx);

        assert_eq!(texts.iter().filter(|text| *text == "参考文献").count(), 1);
        assert!(texts.iter().any(|text| text == "[1] 条目"));
        assert!(texts.iter().any(|text| text == "附录 A 数据集"));
        assert!(texts.iter().any(|text| text == "说明"));
    }

    #[test]
    fn research_lists_use_two_character_first_line_indent() {
        let mut state = ListState::default();
        assert_eq!(state.next_prefix(1), "⑴ ");
        assert_eq!(state.next_prefix(2), "① ");
        assert_eq!(state.next_prefix(3), "(A) ");
        assert_eq!(state.next_prefix(4), "(a) ");
        assert_eq!(state.next_prefix(5), "I. ");
        assert_eq!(state.next_prefix(6), "i. ");

        let mut docx = Docx::new();
        for level in 1..=6 {
            docx = add_list_paragraph(
                docx,
                level,
                "⑴ ",
                &[Inline::Text(format!("第{level}级"))],
                Path::new("."),
            );
        }
        for child in &docx.document.children {
            let paragraph = match child {
                DocumentChild::Paragraph(paragraph) => paragraph,
                other => panic!("expected paragraph, got {other:?}"),
            };
            let indent = paragraph.property.indent.as_ref().expect("list indent");
            assert_eq!(indent.start, Some(0));
            assert_eq!(
                indent.special_indent,
                Some(SpecialIndentType::FirstLine(560))
            );
        }
        let first = match &docx.document.children[0] {
            DocumentChild::Paragraph(paragraph) => paragraph,
            other => panic!("expected paragraph, got {other:?}"),
        };
        assert_eq!(first.raw_text(), "⑴ 第1级");
    }

    #[test]
    fn third_chapter_numbers_table_and_embeds_numbered_figure() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("figure.png");
        image::DynamicImage::new_rgb8(80, 40)
            .save_with_format(&image_path, image::ImageFormat::Png)
            .unwrap();
        let blocks = parser::parse(
            "<!-- [正文] -->\n## 1 第一章\n## 2 第二章\n## 3 第三章标题\n\
             Table: 数据表\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n\
             ![结构图](figure.png)\n",
        );
        let mut emitter = MainEmitter::with_image_base(dir.path().to_path_buf());
        let docx = emitter.emit_all(Docx::new(), &blocks);
        let texts = paragraph_texts(&docx);

        assert!(texts.iter().any(|text| text == "第三章 第三章标题"));
        assert!(texts.iter().any(|text| text == "表 3.1 数据表"));
        assert!(texts.iter().any(|text| text == "图 3.1 结构图"));
        assert!(docx.document.children.iter().any(|child| match child {
            DocumentChild::Paragraph(paragraph) => paragraph.children.iter().any(|child| {
                matches!(child, ParagraphChild::Run(run) if run.children.iter().any(|child| matches!(child, RunChild::Drawing(_))))
            }),
            _ => false,
        }));
    }

    #[test]
    fn table_uses_shared_research_tex_column_widths() {
        let rows = vec![
            vec!["序号".into(), "名称".into(), "详细说明".into()],
            vec!["1".into(), "短项".into(), "这是一段很长的说明文字。".into()],
            vec!["2".into(), "另一项".into(), "另一段较长的说明文字。".into()],
        ];
        let docx = add_table(Docx::new(), &rows);
        let table = match docx.document.children.last() {
            Some(DocumentChild::Table(table)) => table,
            other => panic!("expected table, got {other:?}"),
        };

        assert_eq!(table.grid[0], 560); // 2em at the 14pt body font size
        assert_eq!(table.grid.iter().sum::<usize>(), TABLE_CONTENT_WIDTH_TWIPS);
        assert!(table.grid[2] > table.grid[1]);
    }
}
