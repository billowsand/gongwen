//! 公文 → docx pipeline。
//!
//! 消费 [`crate::parser`] 产出的 [`Block`] AST，按公文风格输出 DOCX。
//!
//! 字体 / 字号 / 编号体系与原 md_to_docx_rust 保持一致：
//! - H1：方正小标宋简体，二号(44hp)，居中，后跟空行
//! - H2：黑体，三号(32hp)，"一、"
//! - H3：楷体_GB2312，三号(32hp)，"（一）"
//! - H4：仿宋_GB2312，三号(32hp)，"1."
//! - H5：仿宋_GB2312，三号(32hp)，加粗，"(1)"
//! - 正文：仿宋_GB2312，三号(32hp)，首行缩进 2 字符，固定行距 29pt
//! - 列表：6 级前缀循环 ①②③ → ⑴⑵⑶ → a.b. → I.II. → (A)(B) → 1)2)
//!
//! 共享逻辑（引号正规化、标题去编号、行内格式拆分、表格解析、编号转换）
//! 统一由 `common/` 与 `parser` 提供，本模块只负责 DOCX 渲染。

use anyhow::{Context, Result};
use docx_rs::*;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::common::ast::{Block, Inline, MarkerKind};
use crate::common::front_matter;
use crate::common::inline;
use crate::common::numbering::{int_to_roman, number_to_chinese, number_to_uppercase_letter};
use crate::common::table_layout::{analyze_table, to_docx_grid};
use crate::parser;

// ===== 圆圈数字（列表前缀用） =====
const CIRCLE_NUMBERS_1: &[&str] = &[
    "⑴", "⑵", "⑶", "⑷", "⑸", "⑹", "⑺", "⑻", "⑼", "⑽", "⑾", "⑿", "⒀", "⒁", "⒂", "⒃", "⒄", "⒅", "⒆",
    "⒇",
];
const CIRCLE_NUMBERS_2: &[&str] = &[
    "①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩", "⑪", "⑫", "⑬", "⑭", "⑮", "⑯", "⑰", "⑱", "⑲",
    "⑳",
];

// ===== 字体 =====
const FONT_TITLE: &str = "方正小标宋简体";
const FONT_HEAD: &str = "黑体";
const FONT_KAI: &str = "楷体_GB2312";
const FONT_BODY: &str = "仿宋_GB2312";
const FONT_SONG: &str = "宋体";

// ===== 字号（半磅 half-points，1pt = 2hp） =====
const SIZE_TITLE: usize = 44; // 二号
const SIZE_BODY: usize = 32; // 三号
const SIZE_TABLE: usize = 28; // 四号
const SIZE_FOOTER: usize = 28; // 四号

// ===== 行距 / 缩进 (twips, 1pt = 20twips) =====
const LINE_BODY: i32 = 580; // 29pt 固定行距
const INDENT_FIRST_LINE: i32 = 640; // 首行缩进 2 字符 (2 × 16pt × 20twips)
const MAX_IMAGE_WIDTH_EMU: u32 = 5_600_000;
const MAX_INLINE_IMAGE_WIDTH_EMU: u32 = 1_800_000;
const TABLE_CONTENT_WIDTH_TWIPS: usize = 8_844; // 156 mm

fn font_set(name: &str) -> RunFonts {
    RunFonts::new().ascii(name).hi_ansi(name).east_asia(name)
}

// ===== 公共入口 =====

pub fn run(input: &Path, output: Option<&Path>) -> Result<()> {
    let input_kind = crate::input::classify(input)?;
    let raw = crate::input::collect_raw(input)?;
    let (_metadata, content) = front_matter::parse(&raw);
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
    let mut emitter = OfficialEmitter::with_image_base(image_base_dir);
    let docx = emitter.base_docx();
    let docx = emitter.emit_all(docx, &blocks);

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
// 转换器
// ============================================================

struct OfficialEmitter {
    h2: usize,
    h3: usize,
    h4: usize,
    h5: usize,
    list: ListState,
    reference_mode: bool,
    reference_counter: usize,
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

impl OfficialEmitter {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_image_base(PathBuf::from("."))
    }

    fn with_image_base(image_base_dir: PathBuf) -> Self {
        Self {
            h2: 0,
            h3: 0,
            h4: 0,
            h5: 0,
            list: ListState::default(),
            reference_mode: false,
            reference_counter: 0,
            suppress_next_heading: None,
            table_counter: 0,
            figure_counter: 0,
            image_base_dir,
        }
    }

    /// 构建文档基础：页面边距 + 页脚（页码 "— N —" 格式）。
    fn base_docx(&self) -> Docx {
        let song_fonts = font_set(FONT_SONG);
        let footer = Footer::new().add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(LineSpacing::new().before(0).after(0))
                .add_run(
                    Run::new()
                        .add_text("\u{2014} ")
                        .fonts(song_fonts.clone())
                        .size(SIZE_FOOTER),
                )
                .add_run(
                    Run::new()
                        .add_field_char(FieldCharType::Begin, false)
                        .fonts(song_fonts.clone())
                        .size(SIZE_FOOTER),
                )
                .add_run(
                    Run::new()
                        .add_instr_text(InstrText::PAGE(InstrPAGE {}))
                        .fonts(song_fonts.clone())
                        .size(SIZE_FOOTER),
                )
                .add_run(
                    Run::new()
                        .add_field_char(FieldCharType::Separate, false)
                        .fonts(song_fonts.clone())
                        .size(SIZE_FOOTER),
                )
                .add_run(
                    Run::new()
                        .add_text("1")
                        .fonts(song_fonts.clone())
                        .size(SIZE_FOOTER),
                )
                .add_run(
                    Run::new()
                        .add_field_char(FieldCharType::End, false)
                        .fonts(song_fonts.clone())
                        .size(SIZE_FOOTER),
                )
                .add_run(
                    Run::new()
                        .add_text(" \u{2014}")
                        .fonts(song_fonts)
                        .size(SIZE_FOOTER),
                ),
        );

        Docx::new()
            .page_margin(
                PageMargin::new()
                    .top(2100) // 3.7cm
                    .bottom(1985) // 3.5cm
                    .left(1588) // 2.8cm
                    .right(1474) // 2.6cm
                    .footer(1588), // footer_distance = 2.8cm
            )
            .footer(footer)
    }

    fn emit_all(&mut self, mut docx: Docx, blocks: &[Block]) -> Docx {
        for b in blocks {
            docx = self.emit(docx, b);
        }
        docx
    }

    fn emit(&mut self, docx: Docx, b: &Block) -> Docx {
        match b {
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
                    self.add_body_paragraph(docx, inlines)
                }
            }
            Block::List { level, content, .. } => {
                let prefix = if self.reference_mode && *level == 1 {
                    self.reference_counter += 1;
                    format!("[{}] ", self.reference_counter)
                } else {
                    self.list.next_prefix(*level)
                };
                self.add_list_paragraph(docx, &prefix, content)
            }
            Block::Table { rows, caption } => {
                self.list.reset();
                let docx = if let Some(caption) = caption {
                    self.table_counter += 1;
                    add_caption_paragraph(docx, &format!("表 {} {}", self.table_counter, caption))
                } else {
                    docx
                };
                self.add_table(docx, rows)
            }
            Block::CodeBlock { content, .. } => {
                self.list.reset();
                self.add_code_block(docx, content)
            }
            Block::Marker(kind) => {
                self.list.reset();
                self.emit_marker(docx, *kind)
            }
            // 公文不使用区段标记和交叉引用锚点；空行沿用旧行为，不截断列表。
            Block::Empty | Block::Label(_) => docx,
        }
    }

    fn emit_marker(&mut self, docx: Docx, kind: MarkerKind) -> Docx {
        self.reference_mode = matches!(kind, MarkerKind::Reference);
        if self.reference_mode {
            self.reference_counter = 0;
        }
        match kind {
            MarkerKind::Abstract => {
                self.suppress_next_heading = Some("摘要");
                self.add_section_heading(docx, "摘要")
            }
            MarkerKind::Changelog => {
                self.suppress_next_heading = Some("版本变更记录");
                self.add_section_heading(docx, "版本变更记录")
            }
            MarkerKind::Reference => {
                self.suppress_next_heading = Some("参考文献");
                self.add_section_heading(docx, "参考文献")
            }
            MarkerKind::Body | MarkerKind::Appendix => {
                self.h2 = 0;
                self.h3 = 0;
                self.h4 = 0;
                self.h5 = 0;
                docx
            }
        }
    }

    fn add_section_heading(&self, docx: Docx, text: &str) -> Docx {
        docx.add_paragraph(heading_paragraph(
            text,
            FONT_TITLE,
            SIZE_TITLE,
            AlignmentType::Center,
            false,
        ))
    }

    fn emit_heading(&mut self, docx: Docx, level: u8, text: &str) -> Docx {
        match level {
            1 => {
                self.h2 = 0;
                self.h3 = 0;
                self.h4 = 0;
                self.h5 = 0;
                let p =
                    heading_paragraph(text, FONT_TITLE, SIZE_TITLE, AlignmentType::Center, false);
                docx.add_paragraph(p).add_paragraph(Paragraph::new())
            }
            2 => {
                self.h2 += 1;
                self.h3 = 0;
                self.h4 = 0;
                self.h5 = 0;
                let title = format!("{}、{}", number_to_chinese(self.h2), text);
                let p = heading_paragraph(&title, FONT_HEAD, SIZE_BODY, AlignmentType::Left, false);
                docx.add_paragraph(p)
            }
            3 => {
                self.h3 += 1;
                self.h4 = 0;
                self.h5 = 0;
                let title = format!("（{}）{}", number_to_chinese(self.h3), text);
                let p = heading_paragraph(&title, FONT_KAI, SIZE_BODY, AlignmentType::Left, false);
                docx.add_paragraph(p)
            }
            4 => {
                self.h4 += 1;
                self.h5 = 0;
                let title = format!("{}.{}", self.h4, text);
                let p = heading_paragraph(&title, FONT_BODY, SIZE_BODY, AlignmentType::Left, false);
                docx.add_paragraph(p)
            }
            5 => {
                self.h5 += 1;
                let title = format!("({}){}", self.h5, text);
                let p = heading_paragraph(&title, FONT_BODY, SIZE_BODY, AlignmentType::Left, true);
                docx.add_paragraph(p)
            }
            _ => docx,
        }
    }

    fn add_body_paragraph(&self, docx: Docx, inlines: &[Inline]) -> Docx {
        let p = body_base();
        let p = add_inlines(
            p,
            inlines,
            FONT_BODY,
            SIZE_BODY,
            false,
            Some(&self.image_base_dir),
        );
        docx.add_paragraph(p)
    }

    fn add_list_paragraph(&self, docx: Docx, prefix: &str, content: &[Inline]) -> Docx {
        let p = body_base();
        let p = p.add_run(
            Run::new()
                .add_text(prefix)
                .fonts(font_set(FONT_BODY))
                .size(SIZE_BODY),
        );
        let p = add_inlines(
            p,
            content,
            FONT_BODY,
            SIZE_BODY,
            false,
            Some(&self.image_base_dir),
        );
        docx.add_paragraph(p)
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
                    add_caption_paragraph(
                        docx,
                        &format!("图 {} {}", self.figure_counter, alt.trim()),
                    )
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

    /// 公文 DOCX 暂无专用代码样式，至少逐行保留代码内容，避免 AST 迁移后静默丢失。
    fn add_code_block(&self, mut docx: Docx, content: &str) -> Docx {
        for line in content.lines().filter(|line| !line.is_empty()) {
            docx = self.add_body_paragraph(docx, &[Inline::Text(line.to_string())]);
        }
        docx
    }

    fn add_table(&self, docx: Docx, rows: &[Vec<String>]) -> Docx {
        if rows.is_empty() {
            return docx;
        }
        let column_layout = analyze_table(rows);
        let max_cols = column_layout.len();
        if max_cols == 0 {
            return docx;
        }
        let grid = to_docx_grid(&column_layout, TABLE_CONTENT_WIDTH_TWIPS, SIZE_TABLE * 10);

        let mut table_rows = Vec::new();
        for (row_idx, row_data) in rows.iter().enumerate() {
            let mut cells = Vec::new();
            for (col_idx, &column_width) in grid.iter().enumerate() {
                let cell_data = row_data.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                let align = if row_idx == 0 {
                    AlignmentType::Center
                } else {
                    AlignmentType::Left
                };
                // 首行黑体、其余行仿宋_GB2312；字号统一四号(28hp)。
                // cell 内的 **加粗**/*斜体* 行内格式通过 inline::parse 解析。
                let font = if row_idx == 0 { FONT_HEAD } else { FONT_BODY };
                let p = Paragraph::new().align(align);
                let p = add_inlines(p, &inline::parse(cell_data), font, SIZE_TABLE, false, None);
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
}

// ============================================================
// 列表前缀状态机
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
                if self.l1 <= CIRCLE_NUMBERS_2.len() {
                    CIRCLE_NUMBERS_2[self.l1 - 1].to_string()
                } else {
                    format!("({})", self.l1)
                }
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
                if self.l2 <= CIRCLE_NUMBERS_1.len() {
                    CIRCLE_NUMBERS_1[self.l2 - 1].to_string()
                } else {
                    format!("({})", self.l2)
                }
            }
            3 => {
                if !self.in_list || self.level < 3 {
                    self.l3 = 0;
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l3 += 1;
                let ch = (b'a' + ((self.l3 - 1) % 26) as u8) as char;
                format!("{}.", ch)
            }
            4 => {
                if !self.in_list || self.level < 4 {
                    self.l4 = 0;
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l4 += 1;
                format!("{}.", int_to_roman(self.l4))
            }
            5 => {
                if !self.in_list || self.level < 5 {
                    self.l5 = 0;
                    self.l6 = 0;
                }
                self.l5 += 1;
                format!("({})", number_to_uppercase_letter(self.l5))
            }
            6 => {
                if !self.in_list || self.level < 6 {
                    self.l6 = 0;
                }
                self.l6 += 1;
                format!("{})", self.l6)
            }
            _ => String::new(),
        };
        self.in_list = true;
        self.level = level;
        prefix
    }
}

// ============================================================
// 段落 / 行内格式构造工具
// ============================================================

/// 公文段落基础：两端对齐、固定行距 29pt、首行缩进 2 字符。
fn body_base() -> Paragraph {
    Paragraph::new()
        .align(AlignmentType::Both)
        .line_spacing(
            LineSpacing::new()
                .line_rule(LineSpacingType::Exact)
                .line(LINE_BODY)
                .before(0)
                .after(0),
        )
        .indent(
            None,
            Some(SpecialIndentType::FirstLine(INDENT_FIRST_LINE)),
            None,
            None,
        )
}

/// 标题段落：固定行距 29pt、首行缩进 2 字符，字体 / 字号 / 对齐 / 加粗由参数控制。
fn heading_paragraph(
    text: &str,
    font: &str,
    size: usize,
    align: AlignmentType,
    force_bold: bool,
) -> Paragraph {
    let p = Paragraph::new()
        .align(align)
        .line_spacing(
            LineSpacing::new()
                .line_rule(LineSpacingType::Exact)
                .line(LINE_BODY)
                .before(0)
                .after(0),
        )
        .indent(
            None,
            Some(SpecialIndentType::FirstLine(INDENT_FIRST_LINE)),
            None,
            None,
        );
    add_inlines(p, &inline::parse(text), font, size, force_bold, None)
}

/// 把 Inline 序列渲染为 Run 并添加到段落。
///
/// 与原 `process_text_formatting` 行为一致：
/// - `Inline::Bold` → 加粗 Run（扁平化子节点为纯文本）
/// - `Inline::Italic` → 斜体 Run
/// - 其他类型 → 普通 Run，`force_bold` 时附加加粗
fn add_inlines(
    mut p: Paragraph,
    inlines: &[Inline],
    font: &str,
    size: usize,
    force_bold: bool,
    image_base_dir: Option<&Path>,
) -> Paragraph {
    for ip in inlines {
        if let (Inline::Image { alt, url, .. }, Some(base_dir)) = (ip, image_base_dir) {
            match crate::common::docx_image::load(url, base_dir, MAX_INLINE_IMAGE_WIDTH_EMU) {
                Ok(pic) => p = p.add_run(Run::new().add_image(pic)),
                Err(error) => {
                    eprintln!("  警告：{error:#}");
                    p = p.add_run(
                        Run::new()
                            .add_text(image_error_text(alt, url))
                            .fonts(font_set(font))
                            .size(size),
                    );
                }
            }
            continue;
        }
        let (text, bold, italic) = match ip {
            Inline::Text(t) => (t.clone(), false, false),
            Inline::Bold(children) => (inline::flatten(children), true, false),
            Inline::Italic(children) => (inline::flatten(children), false, true),
            Inline::Code(t) => (t.clone(), false, false),
            Inline::Link { text, .. } => (text.clone(), false, false),
            Inline::Image { alt, .. } => (alt.clone(), false, false),
            Inline::CrossRef(id) => (id.clone(), false, false),
            Inline::Citation(keys) => (
                format!(
                    "[{}]",
                    keys.iter()
                        .map(|k| format!("@{k}"))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                false,
                false,
            ),
            Inline::Footnote(t) => (format!("（{}）", t), false, false),
        };
        let mut run = Run::new().add_text(&text).fonts(font_set(font)).size(size);
        if bold || force_bold {
            run = run.bold();
        }
        if italic {
            run = run.italic();
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

fn add_caption_paragraph(docx: Docx, caption: &str) -> Docx {
    docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .keep_next(true)
            .line_spacing(
                LineSpacing::new()
                    .before(80)
                    .after(80)
                    .line(LINE_BODY)
                    .line_rule(LineSpacingType::Exact),
            )
            .add_run(
                Run::new()
                    .add_text(caption)
                    .fonts(font_set(FONT_BODY))
                    .size(SIZE_TABLE),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraphs(docx: &Docx) -> Vec<&Paragraph> {
        docx.document
            .children
            .iter()
            .filter_map(|child| match child {
                DocumentChild::Paragraph(p) => Some(p.as_ref()),
                _ => None,
            })
            .collect()
    }

    fn run_text(run: &Run) -> String {
        run.children
            .iter()
            .filter_map(|child| match child {
                RunChild::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn paragraph_text(paragraph: &Paragraph) -> String {
        paragraph
            .children
            .iter()
            .filter_map(|child| match child {
                ParagraphChild::Run(run) => Some(run_text(run)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn table_caption_is_preserved_before_table() {
        let blocks = parser::parse("Table: 产品清单\n\n| 名称 | 数量 |\n|---|---|\n| A | 1 |\n");
        let mut emitter = OfficialEmitter::new();
        let docx = emitter.emit_all(Docx::new(), &blocks);

        assert!(matches!(
            docx.document.children.as_slice(),
            [DocumentChild::Paragraph(_), DocumentChild::Table(_)]
        ));
        let captions: Vec<String> = paragraphs(&docx)
            .into_iter()
            .map(paragraph_text)
            .filter(|text| !text.is_empty())
            .collect();
        assert_eq!(captions, vec!["表 1 产品清单"]);
    }

    #[test]
    fn table_uses_shared_research_tex_column_widths() {
        let rows = vec![
            vec!["序号".into(), "名称".into(), "详细说明".into()],
            vec!["1".into(), "短项".into(), "这是一段很长的说明文字。".into()],
            vec!["2".into(), "另一项".into(), "另一段较长的说明文字。".into()],
        ];
        let docx = OfficialEmitter::new().add_table(Docx::new(), &rows);
        let table = match docx.document.children.last() {
            Some(DocumentChild::Table(table)) => table,
            other => panic!("expected table, got {other:?}"),
        };

        assert_eq!(table.grid[0], 560); // 2em at the 14pt table font size
        assert_eq!(table.grid.iter().sum::<usize>(), TABLE_CONTENT_WIDTH_TWIPS);
        assert!(table.grid[2] > table.grid[1]);
    }

    #[test]
    fn code_block_content_is_preserved_and_resets_list() {
        let blocks = parser::parse("- 第一项\n```rust\nlet s = \"hello\";\n```\n- 新列表\n");
        let mut emitter = OfficialEmitter::new();
        let docx = emitter.emit_all(Docx::new(), &blocks);
        let texts: Vec<String> = paragraphs(&docx).into_iter().map(paragraph_text).collect();

        assert_eq!(
            texts,
            vec!["①第一项", "let s = &quot;hello&quot;;", "①新列表"]
        );
    }

    #[test]
    fn heading_renders_inline_emphasis_without_markers() {
        let blocks = parser::parse("## **重点**和*说明*\n");
        let mut emitter = OfficialEmitter::new();
        let docx = emitter.emit_all(Docx::new(), &blocks);
        let heading = paragraphs(&docx)[0];
        let runs: Vec<&Run> = heading
            .children
            .iter()
            .filter_map(|child| match child {
                ParagraphChild::Run(run) => Some(run.as_ref()),
                _ => None,
            })
            .collect();

        assert_eq!(paragraph_text(heading), "一、重点和说明");
        assert!(runs
            .iter()
            .any(|run| { run_text(run) == "重点" && run.run_property.bold.is_some() }));
        assert!(runs
            .iter()
            .any(|run| { run_text(run) == "说明" && run.run_property.italic.is_some() }));
    }

    #[test]
    fn section_markers_render_headings_and_reference_numbers() {
        let blocks = parser::parse(
            "<!-- [摘要] -->\n# 摘要\n摘要正文\n<!-- [参考文献] -->\n# 参考文献\n- 条目\n",
        );
        let mut emitter = OfficialEmitter::new();
        let docx = emitter.emit_all(Docx::new(), &blocks);
        let texts: Vec<String> = paragraphs(&docx).into_iter().map(paragraph_text).collect();

        assert_eq!(texts.iter().filter(|text| *text == "摘要").count(), 1);
        assert_eq!(texts.iter().filter(|text| *text == "参考文献").count(), 1);
        assert!(texts.iter().any(|text| text == "[1] 条目"));
    }

    #[test]
    fn tables_and_figures_are_numbered_and_image_is_embedded() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("figure.png");
        image::DynamicImage::new_rgb8(80, 40)
            .save_with_format(&image_path, image::ImageFormat::Png)
            .unwrap();
        let blocks = parser::parse(
            "Table: 数据表\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n![结构图](figure.png)\n",
        );
        let mut emitter = OfficialEmitter::with_image_base(dir.path().to_path_buf());
        let docx = emitter.emit_all(Docx::new(), &blocks);
        let texts: Vec<String> = paragraphs(&docx).into_iter().map(paragraph_text).collect();

        assert!(texts.iter().any(|text| text == "表 1 数据表"));
        assert!(texts.iter().any(|text| text == "图 1 结构图"));
        assert!(docx.document.children.iter().any(|child| match child {
            DocumentChild::Paragraph(paragraph) => paragraph.children.iter().any(|child| {
                matches!(child, ParagraphChild::Run(run) if run.children.iter().any(|child| matches!(child, RunChild::Drawing(_))))
            }),
            _ => false,
        }));
    }
}
