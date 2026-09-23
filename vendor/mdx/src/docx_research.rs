//! 研究报告 → docx pipeline。
//!
//! 视觉布局对齐 `resources/research/md2tex.cls` 与 `template.tex`：
//! - 第 1 页 封面：各块用锚定页面的图文框按 `cover::layout` 的毫米数定位，
//!   与 `template.tex` 的 TikZ 封面同一张网格；末尾翻页
//! - 之后 版本变更记录（不进 TOC，由 `<!-- [版本变更记录] -->` 触发）
//! - 之后 摘要 / 正文 / 附录，按 `<!-- [...] -->` 标记切换 emitter 模式
//! - 目录：只在出现 `<!-- [目录] -->` 时排，排在标记所在的位置：居中二号黑体
//!   "目录" + TOC 域（dirty，打开时更新）；整篇只排一次
//!
//! 页码对齐 `md2tex.cls`：页脚居中"— N —"，封面不编页码。全文按目录切成
//! 几个分节（见 [`paginate`]），各节自带页脚与起始页码：
//! - 目录单用大写罗马页码 I、II、III；
//! - 目录插在摘要与正文之间时，摘要单用小写罗马页码 i、ii、iii，正文从 1 起；
//! - 其余情况正文用阿拉伯页码，目录前后接着数，目录页不占正文页号。
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
use crate::common::table::{span_at, TableSpan};
use crate::common::table_layout::{analyze_table, cell_alignment, to_docx_grid, ColumnAlignment};
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
const SIZE_HEAD1: usize = 44; // chapter / 摘要 / 附录 / 目录 / 版本变更记录
const SIZE_HEAD2: usize = 32; // section
const SIZE_HEAD3: usize = 28; // subsection / 日期 / 公开
const SIZE_BODY: usize = 28; // 正文，对齐 LaTeX 14bp

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
const PAGE_FOOTER: i32 = 1588; // 页脚距页面下沿 28 mm，与公文 docx 一致
const SIZE_FOOTER: usize = 28; // 页码四号，对齐 md2tex.cls
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

    let docx = build_docx(&parser::parse(&content), &metadata, image_base_dir);

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

/// 封面 → 版本变更记录 → 主体，最后按目录分节、配页码。
fn build_docx(blocks: &[Block], metadata: &Metadata, image_base_dir: PathBuf) -> Docx {
    let split = split_blocks(blocks);
    let cover_title = metadata.title.as_deref().or(split.title.as_deref());

    let mut docx = base_docx();
    docx = register_styles(docx);
    docx = add_cover(docx, cover_title, metadata);
    let cover_end = docx.document.children.len();

    if !split.changelog.is_empty() {
        docx = add_changelog(docx, &split.changelog, &image_base_dir);
    }

    let mut emitter = MainEmitter::with_image_base(image_base_dir);
    docx = emitter.emit_all(docx, &split.main);
    let front = crate::common::ast::toc_follows_abstract(blocks);
    paginate(docx, cover_end, emitter.toc_range, front)
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
            // 目录不属于任何区段，总在主体里按原位置排
            Block::Toc => main.push(b.clone()),
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
        .page_margin(page_margin())
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
    use crate::cover::{self, layout as l, Family};

    let title_text = title.unwrap_or("研究报告");
    let doc_type = metadata.doc_type.as_deref().unwrap_or("研究报告");
    let family = Family::of(doc_type);
    let security = cover::security_label(metadata.security.as_deref().unwrap_or("公开"));
    let security = match metadata.security_years.as_deref() {
        Some(years) if !security.is_empty() => format!("{security}★{years}"),
        _ => security.to_string(),
    };
    let version = metadata.version.as_deref().and_then(cover::version_mark);
    let institution = metadata.institution.as_deref().unwrap_or("某某单位");
    let date = metadata
        .date
        .as_deref()
        .map(cover::chinese_date)
        .unwrap_or_else(|| {
            let now = Local::now();
            cover::chinese_date(&format!("{}年{}月", now.year(), now.month()))
        });

    // 封面各块都是锚定页面的图文框（framePr），按距页顶的毫米数定位，与 TeX
    // 模板的 TikZ 坐标同一组数；题名换几行都不会把落款往下推。
    let text_run = |text: &str, font: &str, pt: f32| {
        Run::new()
            .add_text(text)
            .fonts(font_set(font))
            .size(half_points(pt))
    };
    let centered = |run: Run, y: f32, pt: f32| {
        framed(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(exact_line(pt * 1.3))
                .add_run(run),
            l::SIDE,
            y,
            l::TEXT_WIDTH,
        )
    };

    // 密级（左）与编号（右）：同一框内用右对齐制表位分开。
    let mut meta_line = Paragraph::new()
        .align(AlignmentType::Left)
        .line_spacing(exact_line(l::META_PT * 1.3))
        .add_tab(
            Tab::new()
                .val(TabValueType::Right)
                .pos(mm_to_twips(l::TEXT_WIDTH) as usize),
        );
    if !security.is_empty() {
        meta_line = meta_line.add_run(text_run(&security, FONT_HEAD, l::META_PT));
    }
    if let Some(number) = metadata.doc_number.as_deref() {
        meta_line = meta_line.add_run(Run::new().add_tab()).add_run(text_run(
            &format!("编号：{number}"),
            FONT_HEAD,
            l::META_PT,
        ));
    }
    docx = docx.add_paragraph(framed(meta_line, l::SIDE, l::META_TOP, l::TEXT_WIDTH));

    // 文种：黑体小二，字距半字。
    docx = docx.add_paragraph(centered(
        text_run(doc_type, FONT_HEAD, l::TYPE_PT)
            .character_spacing((l::TYPE_PT * l::TYPE_TRACKING_EM * 20.0).round() as i32),
        l::TYPE_TOP,
        l::TYPE_PT,
    ));

    if let Some(ident) = metadata.ident.as_deref() {
        docx = docx.add_paragraph(centered(
            text_run(ident, FONT_BODY, l::IDENT_PT),
            l::IDENT_TOP,
            l::IDENT_PT,
        ));
    }

    // 反线：项目类单线，研究类一粗一细。
    docx = docx.add_paragraph(rule(l::RULE_TOP, 12, "000000", l::TEXT_WIDTH, l::SIDE));
    if !family.is_project() {
        docx = docx.add_paragraph(rule(
            l::RULE_TOP + l::RULE_THICK + l::RULE_GAP,
            4,
            "000000",
            l::TEXT_WIDTH,
            l::SIDE,
        ));
    }

    // 题名、稿次、外文原题落在同一个框里，顺序排下，间距同 TeX。
    let title_frame = |p: Paragraph| framed(p, l::SIDE + 5.0, l::TITLE_TOP, l::TEXT_WIDTH - 10.0);
    // 题名按宿主算好的分行排，行间用行内换行，整段仍在同一个图文框里。
    let mut title_paragraph = Paragraph::new()
        .align(AlignmentType::Center)
        .line_spacing(exact_line(l::TITLE_PT * l::TITLE_LEADING));
    for (index, line) in cover::title_lines(title_text, metadata.title_lines.as_deref())
        .iter()
        .enumerate()
    {
        if index > 0 {
            title_paragraph =
                title_paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
        }
        title_paragraph = title_paragraph.add_run(text_run(line, FONT_TITLE, l::TITLE_PT));
    }
    docx = docx.add_paragraph(title_frame(title_paragraph));
    let gap = mm_to_twips(l::TITLE_GAP) as u32;
    if let Some(version) = version.as_deref() {
        docx = docx.add_paragraph(title_frame(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(exact_line(l::VERSION_PT * 1.3).before(gap))
                .add_run(text_run(version, FONT_BODY, l::VERSION_PT)),
        ));
    }
    if let Some(original) = metadata.original_title.as_deref() {
        docx = docx.add_paragraph(title_frame(
            Paragraph::new()
                .align(AlignmentType::Center)
                .line_spacing(exact_line(l::ORIGINAL_PT * 1.35).before(gap))
                .add_run(
                    Run::new()
                        .add_text(original)
                        .fonts(font_set("Times New Roman"))
                        .size(half_points(l::ORIGINAL_PT))
                        .italic(),
                ),
        ));
    }

    match family.stage() {
        // 项目类：四格阶段条，每格一个框，顶边框当横线。
        Some(stage) => {
            let cell = (l::TEXT_WIDTH - 3.0 * l::STAGE_GAP) / 4.0;
            for (index, name) in cover::PROJECT_STAGES.iter().enumerate() {
                let on = index == stage;
                let x = l::SIDE + index as f32 * (cell + l::STAGE_GAP);
                let (size, color) = if on { (18, "000000") } else { (4, "A6A6A6") };
                let mut run = text_run(name, FONT_HEAD, l::STAGE_PT).character_spacing(25);
                if !on {
                    run = run.color("8C8C8C");
                }
                let mut p = Paragraph::new()
                    .align(AlignmentType::Center)
                    .line_spacing(exact_line(l::STAGE_PT * 1.3))
                    .add_run(run);
                p.property = p.property.clone().set_borders(
                    ParagraphBorders::with_empty().set(
                        ParagraphBorder::new(ParagraphBorderPosition::Top)
                            .val(BorderType::Single)
                            .size(size)
                            .space(4)
                            .color(color),
                    ),
                );
                docx = docx.add_paragraph(framed(p, x, l::STAGE_TOP, cell));
            }
        }
        // 研究类：署名行。
        None => {
            if let Some(byline) = metadata.byline.as_deref() {
                // 编译、审校之间的空格统一成一字空（全角空格）。
                let byline = byline
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("\u{3000}");
                docx = docx.add_paragraph(centered(
                    text_run(&byline, FONT_BODY, l::BYLINE_PT),
                    l::BYLINE_TOP,
                    l::BYLINE_PT,
                ));
            }
        }
    }

    // 落款：单位黑体三号，日期宋体小三，同框上下排。
    docx = docx.add_paragraph(centered(
        text_run(institution, FONT_HEAD, l::ORG_PT).character_spacing(40),
        l::ORG_TOP,
        l::ORG_PT,
    ));
    let mut date_line = centered(
        text_run(&date, FONT_BODY, l::DATE_PT),
        l::ORG_TOP,
        l::DATE_PT,
    );
    date_line = date_line
        .line_spacing(exact_line(l::DATE_PT * 1.3).before(mm_to_twips(l::DATE_GAP) as u32));
    docx = docx.add_paragraph(date_line);

    // 封面自成一节，翻页靠分节符（见 [`paginate`]），这里不再另加分页符，
    // 否则封面后会多出一张白页。
    docx
}

fn mm_to_twips(mm: f32) -> i32 {
    (mm * 1440.0 / 25.4).round() as i32
}

fn half_points(pt: f32) -> usize {
    (pt * 2.0).round() as usize
}

fn exact_line(pt: f32) -> LineSpacing {
    LineSpacing::new()
        .before(0)
        .after(0)
        .line((pt * 20.0).round() as i32)
        .line_rule(LineSpacingType::Exact)
}

/// 把段落放进锚定页面的图文框：左上角距页面左、上边 `x`、`y` 毫米，宽 `w` 毫米。
/// 相邻段落的框属性完全相同时 Word 把它们并进同一个框，题名块就靠这一点顺排。
fn framed(mut p: Paragraph, x: f32, y: f32, w: f32) -> Paragraph {
    p.property = p.property.clone().frame_property(
        FrameProperty::new()
            .h_anchor("page")
            .v_anchor("page")
            .x(mm_to_twips(x))
            .y(mm_to_twips(y))
            .width(mm_to_twips(w) as u32)
            .wrap("around"),
    );
    p
}

/// 通栏横线：空段落的顶边框，行高压到 1 pt。`size` 以 1/8 pt 计。
fn rule(y: f32, size: usize, color: &str, width: f32, x: f32) -> Paragraph {
    let mut p = Paragraph::new()
        .line_spacing(exact_line(1.0))
        .add_run(Run::new().add_text("").size(2));
    p.property = p.property.clone().set_borders(
        ParagraphBorders::with_empty().set(
            ParagraphBorder::new(ParagraphBorderPosition::Top)
                .val(BorderType::Single)
                .size(size)
                .space(0)
                .color(color),
        ),
    );
    framed(p, x, y, width)
}

// ============================================================
// 目录
// ============================================================

/// 目录页：标题 + TOC 域。由 `<!-- [目录] -->` 触发，排在标记所在位置；
/// 前后翻页交给分节符（见 [`paginate`]）。
///
/// TOC 域直接写成段落里的域代码，不用 `TableOfContents`：后者是文档级
/// 元素，放不进 `Section`，目录又必须单独成节才能单用罗马页码。
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

    // 域标成 dirty：Word 打开时提示更新，条目与页码由 Word 按分节后的页码生成
    docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_field_char(FieldCharType::Begin, true))
            .add_run(Run::new().add_instr_text(InstrText::Unsupported(
                r#"TOC \o "1-3" \h \z \u"#.to_string(),
            )))
            .add_run(Run::new().add_field_char(FieldCharType::Separate, false))
            .add_run(
                Run::new()
                    .add_text("（打开文档时更新域即生成目录）")
                    .fonts(font_set(FONT_BODY))
                    .size(SIZE_BODY),
            )
            .add_run(Run::new().add_field_char(FieldCharType::End, false)),
    )
}

// ============================================================
// 分节与页码
// ============================================================

/// 目录前最后一段上的书签：目录在正文中间时，目录后的页码 = 本节页号 +
/// 该书签所在页的页号（见 [`page_footer`]）。
const BOOKMARK_BEFORE_TOC: &str = "_MdxBeforeToc";

/// 页脚里的页码写法。
enum PageField {
    /// 阿拉伯数字
    Arabic,
    /// 小写罗马数字 i、ii、iii（与目录分开编号的摘要）
    LowerRoman,
    /// 大写罗马数字 I、II、III（目录）
    UpperRoman,
    /// 目录在正文中间时，目录之后的正文：本节从 1 起，加上目录前最后一页的页号
    AfterToc,
}

/// 按目录把全文切成几节，各节配页脚与起始页码。
///
/// `cover_end`：封面在 `children` 里的终点；`toc`：目录的区间；`front`：
/// 目录插在摘要与正文之间（摘要单用小写罗马页码，正文从 1 起）。
///
/// 节与节之间靠分节符（下一页）翻页，所以各节首尾那种只为翻页而设的
/// 空段（段前分页的空段落）要去掉，否则会在节的开头或结尾多出一张白页。
fn paginate(mut docx: Docx, cover_end: usize, toc: Option<(usize, usize)>, front: bool) -> Docx {
    let mut children = std::mem::take(&mut docx.document.children);
    let mut rest = children.split_off(cover_end);
    let cover = children;

    // (内容, 页码写法)；最后一节用文档级的 sectPr，不进 Section
    let mut parts: Vec<(Vec<DocumentChild>, PageField)> = Vec::new();
    match toc {
        Some((start, end)) => {
            let after = rest.split_off(end - cover_end);
            let toc_part = rest.split_off(start - cover_end);
            let mut before = trim_page_breaks(rest);
            let before_is_empty = before.is_empty();
            if !before_is_empty {
                if !front {
                    mark_last_paragraph(&mut before, BOOKMARK_BEFORE_TOC);
                }
                let field = if front {
                    PageField::LowerRoman
                } else {
                    PageField::Arabic
                };
                parts.push((before, field));
            }
            parts.push((trim_page_breaks(toc_part), PageField::UpperRoman));
            let after_field = if front || before_is_empty {
                PageField::Arabic
            } else {
                PageField::AfterToc
            };
            parts.push((trim_page_breaks(after), after_field));
        }
        None => parts.push((trim_page_breaks(rest), PageField::Arabic)),
    }

    // 封面：自成一节，不带页脚
    docx = docx.add_section(section_of(cover, None));
    let (last, last_field) = parts.pop().expect("至少有正文一节");
    for (part, field) in parts {
        docx = docx.add_section(section_of(part, Some(field)));
    }
    docx.document.children.extend(last);
    docx.footer(page_footer(&last_field))
        .page_num_type(PageNumType::new().start(1))
}

/// 把一段顶层元素装进一个分节；`field` 为 `None` 时不带页脚（封面）。
fn section_of(children: Vec<DocumentChild>, field: Option<PageField>) -> Section {
    let mut section = Section::new().page_margin(page_margin());
    for child in children {
        section = match child {
            DocumentChild::Paragraph(p) => section.add_paragraph(*p),
            DocumentChild::Table(t) => section.add_table(*t),
            // 研究报告只产出段落与表格两种顶层元素
            other => unreachable!("研究报告 docx 不产出这种顶层元素：{other:?}"),
        };
    }
    match field {
        Some(field) => section
            .footer(page_footer(&field))
            .page_num_type(PageNumType::new().start(1)),
        None => section,
    }
}

/// 只为翻页而设的空段：段前分页、没有文字。
fn is_page_break_filler(child: &DocumentChild) -> bool {
    matches!(child, DocumentChild::Paragraph(p)
        if p.property.page_break_before == Some(true) && p.raw_text().trim().is_empty())
}

/// 去掉一节首尾的翻页空段：分节符本身就换页。
fn trim_page_breaks(mut children: Vec<DocumentChild>) -> Vec<DocumentChild> {
    while children.last().is_some_and(is_page_break_filler) {
        children.pop();
    }
    let lead = children
        .iter()
        .take_while(|c| is_page_break_filler(c))
        .count();
    children.drain(..lead);
    children
}

/// 在一节最后一个段落上打书签。
fn mark_last_paragraph(children: &mut [DocumentChild], name: &str) {
    if let Some(DocumentChild::Paragraph(p)) = children
        .iter_mut()
        .rev()
        .find(|c| matches!(c, DocumentChild::Paragraph(_)))
    {
        let marked = (**p)
            .clone()
            .add_bookmark_start(1, name)
            .add_bookmark_end(1);
        **p = marked;
    }
}

fn page_margin() -> PageMargin {
    PageMargin::new()
        .top(PAGE_TOP)
        .bottom(PAGE_BOTTOM)
        .left(PAGE_LEFT)
        .right(PAGE_RIGHT)
        .footer(PAGE_FOOTER)
}

/// 页脚：居中"— N —"，四号，对齐 md2tex.cls 的 `\cfoot`。
fn page_footer(field: &PageField) -> Footer {
    let run = || Run::new().fonts(font_set(FONT_BODY)).size(SIZE_FOOTER);
    let simple = |code: &str| {
        vec![
            run().add_field_char(FieldCharType::Begin, false),
            run().add_instr_text(InstrText::Unsupported(code.to_string())),
            run().add_field_char(FieldCharType::Separate, false),
            run().add_text("1"),
            run().add_field_char(FieldCharType::End, false),
        ]
    };
    let number = match field {
        PageField::Arabic => simple(" PAGE "),
        PageField::LowerRoman => simple(r" PAGE \* roman "),
        PageField::UpperRoman => simple(r" PAGE \* ROMAN "),
        // { = { PAGE } + { PAGEREF _MdxBeforeToc } }
        PageField::AfterToc => {
            let mut runs = vec![
                run().add_field_char(FieldCharType::Begin, false),
                run().add_instr_text(InstrText::Unsupported(" = ".into())),
            ];
            runs.extend(simple(" PAGE "));
            runs.push(run().add_instr_text(InstrText::Unsupported(" + ".into())));
            runs.extend(simple(&format!(" PAGEREF {BOOKMARK_BEFORE_TOC} ")));
            runs.push(run().add_field_char(FieldCharType::Separate, false));
            runs.push(run().add_text("1"));
            runs.push(run().add_field_char(FieldCharType::End, false));
            runs
        }
    };
    let mut p = Paragraph::new()
        .align(AlignmentType::Center)
        .line_spacing(LineSpacing::new().before(0).after(0))
        .add_run(run().add_text("\u{2014} "));
    for r in number {
        p = p.add_run(r);
    }
    Footer::new().add_paragraph(p.add_run(run().add_text(" \u{2014}")))
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
    /// 目录已经排过：`<!-- [目录] -->` 写了多处时只认第一处
    toc_done: bool,
    /// 目录在 `docx.document.children` 里占的区间，[`paginate`] 据此分节
    toc_range: Option<(usize, usize)>,
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
            toc_done: false,
            toc_range: None,
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
            Block::Toc => {
                self.list.reset();
                if self.toc_done {
                    return docx;
                }
                self.toc_done = true;
                // 目录自成一节：前后翻页都靠分节符，这里不加分页
                let start = docx.document.children.len();
                let docx = add_toc(docx);
                self.toc_range = Some((start, docx.document.children.len()));
                docx
            }
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
            Block::Table {
                rows,
                caption,
                spans,
                numbered,
            } => {
                self.list.reset();
                let docx = if let Some(caption) = caption {
                    self.table_counter += 1;
                    let caption =
                        format!("表 {} {}", self.object_number(self.table_counter), caption);
                    add_table_caption(docx, &caption)
                } else {
                    docx
                };
                add_table(docx, rows, spans, *numbered)
            }
            Block::CodeBlock { content, .. } => {
                self.list.reset();
                add_code_block(docx, content)
            }
            Block::Math(content) => {
                // docx 不支持公式：降级为源码原文段落
                self.list.reset();
                let base_dir = &self.image_base_dir;
                add_body_paragraph(docx, |p| {
                    add_inlines(p, &[Inline::Text(format!("$${}$$", content))], base_dir)
                })
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
            Block::Table {
                rows,
                caption,
                spans,
                numbered,
            } => {
                self.list.reset();
                let docx = if let Some(caption) = caption {
                    self.table_counter += 1;
                    add_table_caption(docx, &format!("表 {} {}", self.table_counter, caption))
                } else {
                    docx
                };
                add_table(docx, rows, spans, *numbered)
            }
            Block::Math(content) => {
                // docx 不支持公式：降级为源码原文段落
                self.list.reset();
                add_body_paragraph(docx, |p| {
                    add_inlines(
                        p,
                        &[Inline::Text(format!("$${}$$", content))],
                        &self.image_base_dir,
                    )
                })
            }
            Block::Marker(_)
            | Block::Toc
            | Block::Empty
            | Block::CodeBlock { .. }
            | Block::Label(_) => docx,
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
        // docx 不支持公式，降级为源码原文
        Inline::Math(t) => (format!("${t}$"), false, false),
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

fn add_table(docx: Docx, rows: &[Vec<String>], spans: &[TableSpan], numbered: bool) -> Docx {
    if rows.is_empty() {
        return docx;
    }
    let column_layout = analyze_table(rows, spans);
    let max_cols = column_layout.len();
    if max_cols == 0 {
        return docx;
    }
    let grid = to_docx_grid(&column_layout, TABLE_CONTENT_WIDTH_TWIPS, SIZE_BODY * 10);

    let mut table_rows = Vec::new();
    for (row_idx, row) in rows.iter().enumerate() {
        let mut cells = Vec::new();
        let mut col_idx = 0;
        while col_idx < grid.len() {
            let span = span_at(spans, row_idx, col_idx);
            // 横向合并格只在锚点列写一格（gridSpan），同行被它盖住的列跳过；
            // 纵向合并的续行照样要写一格 vMerge=continue 占位。
            if span.is_some_and(|span| span.column != col_idx) {
                col_idx += 1;
                continue;
            }
            let column_span = span
                .map_or(1, |span| span.column_span)
                .min(grid.len() - col_idx);
            let width = grid[col_idx..col_idx + column_span].iter().sum::<usize>();
            let continuation = span.is_some_and(|span| span.row != row_idx);
            let cell_data = if continuation {
                ""
            } else {
                row.get(col_idx).map(String::as_str).unwrap_or("")
            };
            // 表格一律居中；序号表整行合并的分组行靠左，与 TeX 一致。
            let group_row = numbered
                && column_span == grid.len()
                && cell_alignment(rows, spans, &column_layout, numbered, row_idx, col_idx)
                    == ColumnAlignment::Left;
            let align = if group_row {
                AlignmentType::Left
            } else {
                AlignmentType::Center
            };
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
            // 竖向居中与 TeX 一致；纵向合并格尤其看得出来。
            let mut cell = TableCell::new()
                .width(width, WidthType::Dxa)
                .vertical_align(VAlignType::Center)
                .add_paragraph(p);
            if column_span > 1 {
                cell = cell.grid_span(column_span);
            }
            if span.is_some_and(|span| span.row_span > 1) {
                cell = cell.vertical_merge(if continuation {
                    VMergeType::Continue
                } else {
                    VMergeType::Restart
                });
            }
            cells.push(cell);
            col_idx += column_span;
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

    fn toc_count(docx: &Docx) -> usize {
        paragraph_texts(docx)
            .iter()
            .filter(|t| t.as_str() == "目  录")
            .count()
    }

    /// 按文档顺序列出各分节的页码：(页脚里的域代码, 是否从 1 起)。
    /// 封面那一节没有页脚，域代码为空串。
    fn section_page_numbers(markdown: &str) -> Vec<(String, bool)> {
        let blocks = parser::parse(markdown);
        let xml = build_docx(&blocks, &Metadata::default(), PathBuf::from(".")).build();
        let document = String::from_utf8(xml.document).unwrap();
        let footers: Vec<String> = xml
            .footers
            .iter()
            .map(|f| String::from_utf8(f.clone()).unwrap())
            .collect();
        let instr = regex::Regex::new(r"<w:instrText[^>]*>([^<]*)</w:instrText>").unwrap();
        let footer_ref = regex::Regex::new(r#"footerReference[^>]*r:id="rIdFooter(\d+)""#).unwrap();
        document
            .split("<w:sectPr")
            .skip(1)
            .map(|sect| {
                let sect = &sect[..sect.find("</w:sectPr>").unwrap()];
                let code = footer_ref
                    .captures(sect)
                    .map(|c| {
                        let n: usize = c[1].parse().unwrap();
                        instr
                            .captures_iter(&footers[n - 1])
                            .map(|m| m[1].trim().to_string())
                            .collect::<Vec<_>>()
                            .join("|")
                    })
                    .unwrap_or_default();
                (code, sect.contains(r#"w:pgNumType w:start="1""#))
            })
            .collect()
    }

    /// 没有目录：封面不编页码，正文阿拉伯页码从 1 起。
    #[test]
    fn pages_without_toc() {
        assert_eq!(
            section_page_numbers(
                "<!-- [摘要] -->\n\n摘要。\n\n<!-- [正文] -->\n\n## 引言\n\n正文。\n"
            ),
            vec![(String::new(), false), ("PAGE".into(), true)]
        );
    }

    /// 目录在最前：目录大写罗马，其后正文从 1 起。
    #[test]
    fn pages_with_toc_first() {
        assert_eq!(
            section_page_numbers(
                "<!-- [目录] -->\n\n<!-- [摘要] -->\n\n摘要。\n\n## 引言\n\n正文。\n"
            ),
            vec![
                (String::new(), false),
                (r"PAGE \* ROMAN".into(), true),
                ("PAGE".into(), true),
            ]
        );
    }

    /// 目录插在摘要与正文之间：摘要小写罗马，目录大写罗马，正文从 1 起。
    #[test]
    fn pages_with_toc_between_abstract_and_body() {
        assert_eq!(
            section_page_numbers("<!-- [摘要] -->\n\n摘要。\n\n<!-- [目录] -->\n\n<!-- [正文] -->\n\n## 引言\n\n正文。\n"),
            vec![
                (String::new(), false),
                (r"PAGE \* roman".into(), true),
                (r"PAGE \* ROMAN".into(), true),
                ("PAGE".into(), true),
            ]
        );
    }

    /// 目录在正文中间：目录后的页码接着目录前的页号数（加上书签所在页的页号）。
    #[test]
    fn pages_with_toc_inside_body() {
        let md = "## 引言\n\n正文。\n\n<!-- [目录] -->\n\n## 结论\n\n正文。\n";
        assert_eq!(
            section_page_numbers(md),
            vec![
                (String::new(), false),
                ("PAGE".into(), true),
                (r"PAGE \* ROMAN".into(), true),
                (format!("=|PAGE|+|PAGEREF {BOOKMARK_BEFORE_TOC}"), true),
            ]
        );
        let xml = build_docx(&parser::parse(md), &Metadata::default(), PathBuf::from(".")).build();
        let document = String::from_utf8(xml.document).unwrap();
        assert!(document.contains(&format!(r#"w:name="{BOOKMARK_BEFORE_TOC}""#)));
    }

    /// 没写 `<!-- [目录] -->` 就不排目录；写了多处只排一次。
    #[test]
    fn toc_only_where_marked_and_only_once() {
        let heading = Block::Heading {
            level: 2,
            text: "引言".into(),
        };
        let mut e = MainEmitter::new();
        let docx = e.emit_all(Docx::new(), std::slice::from_ref(&heading));
        assert_eq!(toc_count(&docx), 0);

        let mut e = MainEmitter::new();
        let docx = e.emit_all(
            Docx::new(),
            &[
                Block::Marker(MarkerKind::Abstract),
                Block::Paragraph(vec![Inline::Text("摘要".into())]),
                Block::Toc,
                heading,
                Block::Toc,
            ],
        );
        assert_eq!(toc_count(&docx), 1);
        let texts = paragraph_texts(&docx);
        let abstract_at = texts.iter().position(|t| t == "摘要").unwrap();
        let toc_title_at = texts.iter().position(|t| t == "目  录").unwrap();
        let chapter_at = texts.iter().position(|t| t.contains("引言")).unwrap();
        assert!(
            abstract_at < toc_title_at && toc_title_at < chapter_at,
            "{texts:?}"
        );
    }

    /// 目录标记写在版本变更记录区段里，也留在主体按原位置排。
    #[test]
    fn split_keeps_toc_in_main() {
        let blocks = vec![
            Block::Marker(MarkerKind::Changelog),
            Block::Toc,
            Block::Paragraph(vec![Inline::Text("v1.0 初版".into())]),
        ];
        let split = split_blocks(&blocks);
        assert!(split.main.iter().any(|b| matches!(b, Block::Toc)));
        assert!(!split.changelog.iter().any(|b| matches!(b, Block::Toc)));
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
            doc_type: Some("技术实现方案".into()),
            doc_number: Some("XX-2026-001".into()),
            version: Some("V2.1".into()),
            institution: Some("某研究所".into()),
            date: Some("2026-07".into()),
            title: Some("系统报告".into()),
            ident: Some("项目编号：XM-2026-014".into()),
            ..Metadata::default()
        };
        let docx = add_cover(Docx::new(), metadata.title.as_deref(), &metadata);
        let text = paragraph_texts(&docx).join("\n");

        for expected in [
            "机密★5年",
            "编号：XX-2026-001",
            "（V2.1）",
            "技术实现方案",
            "项目编号：XM-2026-014",
            "系统报告",
            "立项论证",
            "技术实现",
            "某研究所",
            "二〇二六年七月",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }
    }

    #[test]
    fn research_cover_prints_byline_and_original_title_but_no_stage_strip() {
        let metadata = Metadata {
            security: Some("公开".into()),
            doc_type: Some("外文翻译".into()),
            byline: Some("编译：信息资源处".into()),
            original_title: Some("AI Risk Management Framework".into()),
            ..Metadata::default()
        };
        let docx = add_cover(Docx::new(), Some("人工智能风险管理框架"), &metadata);
        let text = paragraph_texts(&docx).join("\n");
        assert!(text.contains("编译：信息资源处"), "{text}");
        assert!(text.contains("AI Risk Management Framework"), "{text}");
        assert!(!text.contains("立项论证"), "研究类不印阶段条：{text}");
        assert!(!text.contains("公开"), "公开件不标密级：{text}");
    }

    #[test]
    fn cover_title_uses_the_host_line_breaks() {
        let metadata = Metadata {
            title_lines: Some(vec!["全市一体化政务数据共享".into(), "平台建设项目".into()]),
            ..Metadata::default()
        };
        let docx = add_cover(
            Docx::new(),
            Some("全市一体化政务数据共享平台建设项目"),
            &metadata,
        );
        let xml = String::from_utf8(docx.build().document).unwrap();
        let title = xml
            .split("<w:p>")
            .find(|p| p.contains("全市一体化政务数据共享"))
            .unwrap();
        assert!(title.contains("<w:br w:type=\"textWrapping\""), "{title}");
        assert!(title.contains("平台建设项目"), "{title}");
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
        let docx = add_table(Docx::new(), &rows, &[], false);
        let table = match docx.document.children.last() {
            Some(DocumentChild::Table(table)) => table,
            other => panic!("expected table, got {other:?}"),
        };

        assert_eq!(table.grid[0], 560); // 2em at the 14pt body font size
        assert_eq!(table.grid.iter().sum::<usize>(), TABLE_CONTENT_WIDTH_TWIPS);
        assert!(table.grid[2] > table.grid[1]);
    }
}
