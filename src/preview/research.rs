//! 研究报告的纸面预览。
//!
//! 版心、字号、行距与标题层级对齐 mdx 的 `md2tex.cls`，排版规则照
//! `typst_research` 抄，保证预览与导出 PDF 一致。各功能域已拆分到
//! `preview/research/` 子模块（编号、锚点、正文、封面、块排版、标题），
//! 这里保留 `Kind` / `Caption` 与测试。

#[cfg(test)]
use crate::export::crossref;
#[cfg(test)]
use crate::models::{DraftInput, NumberingConfig};
#[cfg(test)]
use crate::preview::{Metrics, PreviewScale, RESEARCH_CAPTION_PT};
#[cfg(test)]
use crate::theme;
use std::ops::Range;

mod blocks;
mod body;
mod cover;
mod headings;
mod marks;
mod numbering;

pub(crate) use blocks::body_item;
pub(crate) use body::research_preview;
pub(crate) use cover::cover_sheet;
pub(crate) use headings::{
    CAPTION_GAP_PT, CHAPTER_GAP, FLOAT_SEP_PT, SECTION_GAP, caption_line, chapter_title,
    part_title, section_title, space_after_float, title_line,
};
#[cfg(test)]
pub(crate) use marks::parse_table_caption;
pub(crate) use marks::{
    LabelKind, LabelTarget, anchor_of, caption_text, citations, collect_marks, label_targets,
    outline, report_title, table_captions,
};
pub(crate) use numbering::walk;

/// 一块内容在纸面上的身份。编号在这里一次算好：印它要用，锚点表也要用。
/// 可哈希：它是块排版指纹的一部分（`preview::cull`）。
#[derive(Hash)]
pub(super) enum Kind<'a> {
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
    Diagram {
        number: Option<String>,
        caption: &'a str,
        source: &'a str,
    },
    /// 表格，连同并进来的表题。
    Table { caption: Option<Caption> },
    /// 引文：`>` 引用块，排法见 `research_quote`。
    Citation,
    /// 文框：首行 `> [!名称] 标题` 的引用块（专栏、案例……）。`number` 是这种
    /// 名称自己的编号（"2.1"，不编号章里是流水号），`{@id}` 引的就是它，名称
    /// 由写正文的人自己写；`heading` 是纸面上的整串前缀（"案例 2.1"）。
    Box {
        number: String,
        heading: String,
        name: &'a str,
        title: &'a str,
    },
}

/// 一条并进表格的表题：编号、题名，以及它自己那一行源码的位置。
#[derive(Hash)]
pub(super) struct Caption {
    pub(super) number: String,
    pub(super) text: String,
    pub(super) source: Range<usize>,
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
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: title_hint.to_string(),
            ..Default::default()
        };
        drawn_input(&input, markdown)
    }

    /// 同 [`drawn`]，文档要素整份给出（文献库等）。
    fn drawn_input(input: &DraftInput, markdown: &str) -> String {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &FontConfig::default());
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
                input,
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

    #[test]
    fn terminal_preview_preserves_report_semantics_and_scopes_paper_color() {
        use crate::models::{ResearchPalette, ResearchTemplate};
        let original = theme::paper::bg();
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..Default::default()
        };
        input.research.template = ResearchTemplate::Terminal;
        let markdown = "# 研究标题\n\n<!-- [目录] -->\n\n<!-- [部分] -->\n\n# 现状与问题\n\n## 研究背景\n\n### 数据来源\n\n这是正文。\n";
        for palette in [ResearchPalette::Dark, ResearchPalette::Bright] {
            input.research.palette = palette;
            let text = drawn_input(&input, markdown);
            assert!(text.contains("研究终端"));
            assert!(text.contains("研究标题"));
            assert!(text.contains("BOM") && text.contains("PART 1"));
            assert!(text.contains("章节明细") && text.contains("P1"));
            assert!(text.contains("第1章"));
            // 终端的节号在独立方框里，绘制文字不再与标题合成同一个 galley。
            assert!(text.contains("1.1") && text.contains("数据来源"));
            assert!(text.contains("这是正文。"));
            assert_eq!(
                theme::paper::bg(),
                original,
                "报告绘制不能污染其他预览的纸色"
            );
        }
    }

    /// 同 [`drawn`]，但保留全部图形：公式纹理、占位虚线都要从 shape 里找。
    fn drawn_shapes(markdown: &str) -> Vec<egui::epaint::ClippedShape> {
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..Default::default()
        };
        drawn_shapes_input(&input, markdown)
    }

    fn drawn_shapes_input(input: &DraftInput, markdown: &str) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &FontConfig::default());
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
                input,
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

    #[test]
    fn terminal_nested_heading_numbers_are_framed_and_quotes_share_card_style() {
        use crate::models::ResearchTemplate;
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..Default::default()
        };
        input.research.template = ResearchTemplate::Terminal;
        let shapes = drawn_shapes_input(
            &input,
            "## 方法\n\n### 一节\n\n#### 小节\n\n##### 细目\n\n> 普通引文。\n> ——研究材料\n\n> [!引理] 样本\n>\n> 引理内容。\n\n> [!推论] 结论\n>\n> 推论内容。\n",
        );
        let flat = flatten(&shapes);
        for number in ["1.1", "1.1.1", "1.1.1.1"] {
            let text = flat
                .iter()
                .find_map(|s| match s {
                    egui::Shape::Text(t) if t.galley.text() == number => Some(t),
                    _ => None,
                })
                .expect("编号独立绘制");
            assert!(flat.iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.rect.contains(text.pos + text.galley.size() / 2.0))), "{number} 应位于编号框内");
        }
        let text = flat_text(&shapes);
        for value in [
            "引文",
            "QUOTE",
            "引理",
            "LEMMA",
            "推论",
            "COROLLARY",
            "研究材料",
        ] {
            assert!(text.contains(value), "缺 {value}：{text}");
        }
        let corners = flat.iter().filter(|s| matches!(s, egui::Shape::Path(p) if p.points.len() == 3 && p.stroke.width > 0.0)).count();
        assert!(corners >= 6, "三个引用块各有首尾角标");
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

    /// `[@…]` 印成上标：序号缩小、基线抬高，哨兵不落到纸上。普通段落与公式混排的
    /// 段落各走一条排版路径，两条都要管。
    #[test]
    fn bracket_citations_are_drawn_as_superscripts() {
        let shapes = drawn_shapes(concat!(
            "<!-- [正文] -->\n\n## 背景\n\n",
            "研究表明[@a; @b]。\n\n",
            "公式$x$之后又引[@a]。\n",
        ));
        let texts = drawn_texts(&shapes);
        for text in &texts {
            assert!(
                !text
                    .galley
                    .text()
                    .contains([crossref::SUPER_OPEN, crossref::SUPER_CLOSE]),
                "哨兵不该印出来：{}",
                text.galley.text()
            );
        }
        for (before, cited) in [("明", "[1,2]"), ("引", "[1]")] {
            let glyphs: Vec<_> = texts
                .iter()
                .flat_map(|text| text.galley.rows.iter())
                .flat_map(|row| row.glyphs.iter())
                .collect();
            let at = glyphs
                .windows(2)
                .position(|pair| pair[0].chr.to_string() == before && pair[1].chr == '[')
                .unwrap_or_else(|| panic!("找不到“{before}{cited}”"));
            let (body, number) = (glyphs[at], glyphs[at + 1]);
            assert!(
                number.font_height < body.font_height * 0.8,
                "{cited} 应缩小：{} vs {}",
                number.font_height,
                body.font_height
            );
            assert!(
                number.pos.y < body.pos.y,
                "{cited} 的基线应高于正文：{} vs {}",
                number.pos.y,
                body.pos.y
            );
        }
    }

    /// 版本变更记录、参考文献的标题只排一次：区段标记不自带标题，标题由区段
    /// 里的首个标题充当（与原 TeX 输出的 `\chapter*` 一致）。
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

    /// 纸面上画出来的文字块，连同它们的 galley（要看字面、字号与位置）。
    fn drawn_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<&egui::epaint::TextShape> {
        flatten(shapes)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::epaint::Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    /// 图题与表题同一套字面（`md2tex.cls` 的 caption 包与 tabularray 两处一致）：
    /// 标签黑体、题名宋体，都是小四；表格正文也是宋体小四，不是公文的仿宋四号。
    #[test]
    fn figure_and_table_captions_share_one_typeface() {
        let shapes = drawn_shapes(concat!(
            "<!-- [正文] -->\n\n## 章\n\n",
            "![总体架构](images/a.png)\n\n",
            "表：样本分布\n| 地区 | 样本 |\n| --- | --- |\n| 华东 | 12 |\n",
        ));
        let texts = drawn_texts(&shapes);
        let caption_fonts = |prefix: &str| {
            let galley = &texts
                .iter()
                .find(|text| text.galley.text().starts_with(prefix))
                .unwrap_or_else(|| panic!("没画出“{prefix}”"))
                .galley;
            let sections = &galley.job.sections;
            (
                sections[0].format.font_id.clone(),
                sections[sections.len() - 1].format.font_id.clone(),
            )
        };
        let figure = caption_fonts("图 1.1");
        let table = caption_fonts("表 1.1");
        assert_eq!(figure, table, "图题与表题应同一套字面");
        let metrics = Metrics::research(1000.0, Some(1.0));
        assert_eq!(
            figure.0,
            metrics.font(theme::FONT_HEITI, RESEARCH_CAPTION_PT)
        );
        assert_eq!(
            figure.1,
            metrics.font(theme::FONT_SONGTI, RESEARCH_CAPTION_PT)
        );
        let cell = texts
            .iter()
            .find(|text| text.galley.text() == "华东")
            .expect("表格正文");
        assert_eq!(
            cell.galley.job.sections[0].format.font_id, figure.1,
            "表格正文与题名同为宋体小四"
        );
    }

    /// 插图宽度照 `mdx::figure_size` 取档、居中，与导出的 PDF / Word 一致：
    /// 方图是 0.8 档，不再像公文那样按像素原样铺到版心宽。
    #[test]
    fn figures_take_the_export_width_step_and_are_centered() {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &FontConfig::default());
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_research_width_{}.png", std::process::id());
        let path = dir.join(&name);
        image::RgbaImage::from_pixel(1200, 1200, image::Rgba([0, 0, 255, 255]))
            .save(&path)
            .unwrap();
        let markdown = format!("<!-- [正文] -->\n\n## 章\n\n![方图](images/{name})\n");
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
        let frame = || {
            ctx.run_ui(raw.clone(), |ui| {
                let _ = research_preview(
                    ui,
                    &input,
                    &markdown,
                    PreviewScale::zoom(Some(1.0)),
                    None,
                    false,
                    &NumberingConfig::default(),
                    false,
                );
            })
            .shapes
        };
        // 首帧交给后台解码，等解码完再排。
        let mut figure = None;
        for _ in 0..20 {
            let shapes = frame();
            figure = flatten(&shapes).into_iter().find_map(|shape| match shape {
                // `Image::paint_at` 画成带纹理笔刷的矩形。
                egui::epaint::Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect),
                _ => None,
            });
            if figure.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = std::fs::remove_file(&path);
        let figure = figure.expect("图片应在几帧内解码画出");
        let metrics = Metrics::research(1000.0, Some(1.0));
        assert!(
            (figure.width() - metrics.content * 0.8).abs() < 1.0,
            "方图应占版心宽 0.8：{figure:?}，版心 {}",
            metrics.content
        );
        assert!((figure.width() - figure.height()).abs() < 1.0, "{figure:?}");
        // 居中：左右留白相等。纸的版心左沿 = 图左沿 − 一侧留白。
        let side = (metrics.content - figure.width()) / 2.0;
        let shapes = frame();
        let caption = drawn_texts(&shapes)
            .into_iter()
            .find(|text| text.galley.text().starts_with("图 1.1"))
            .map(|text| text.pos.x)
            .expect("图题");
        // 图题是画在版心正中的 galley，原点横坐标就是版心中线。
        assert!(
            (figure.center().x - caption).abs() < 1.0,
            "图应居中：图 {figure:?}，版心中线 {caption}，一侧留白 {side}"
        );
    }

    /// 图题不贴着图，表格后面的正文不贴着表格下边框：预览的行距留白全在字
    /// 下方，这两处原先一点空都没有。
    #[test]
    fn captions_and_following_text_keep_clear_of_figures_and_tables() {
        let shapes = drawn_shapes(concat!(
            "<!-- [正文] -->\n\n## 章\n\n",
            "![总体架构](images/a.png)\n\n",
            "表：样本分布\n| 地区 | 样本 |\n| --- | --- |\n| 华东 | 12 |\n\n",
            "表后正文。\n",
        ));
        let texts = drawn_texts(&shapes);
        let top_of = |needle: &str| {
            texts
                .iter()
                .find(|text| text.galley.text().contains(needle))
                .unwrap_or_else(|| panic!("没画出“{needle}”"))
                .pos
                .y
        };
        let figure_caption = top_of("图 1.1");
        let table_caption = top_of("表 1.1");
        let after = top_of("表后正文");
        // 图片找不到时画占位框，表格画格线：都是描边矩形，按位置区分。
        let bottom_of_rects_in = |from: f32, to: f32| {
            flatten(&shapes)
                .into_iter()
                .filter_map(|shape| match shape {
                    egui::epaint::Shape::Rect(rect) => Some(rect.rect),
                    _ => None,
                })
                .filter(|rect| rect.min.y > from && rect.max.y <= to)
                .map(|rect| rect.max.y)
                .fold(f32::NEG_INFINITY, f32::max)
        };
        let metrics = Metrics::research(1000.0, Some(1.0));
        let figure_bottom = bottom_of_rects_in(0.0, figure_caption);
        assert!(
            figure_caption - figure_bottom >= metrics.pt(CAPTION_GAP_PT) - 1.0,
            "图题与图之间应留 \\abovecaptionskip：{figure_bottom} → {figure_caption}"
        );
        let table_bottom = bottom_of_rects_in(table_caption, after);
        assert!(
            after - table_bottom >= metrics.pt(FLOAT_SEP_PT) + metrics.pt(8.0),
            "表后正文应离开表格下边框：{table_bottom} → {after}"
        );
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
    /// （`export::research` 的 `typst_data_keeps_a_body_h1_off_the_page_and_out_of_the_chapter_count`）。
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

    /// 封面题名里的行内公式与 PDF 一样排成公式：纸上只剩题名文字，不印源码。
    #[test]
    fn inline_math_in_the_cover_title_is_typeset() {
        for (hint, markdown) in [
            (
                "",
                "<!-- [正文] -->\n\n# 这时白头件的题目$\\sum_a^b$\n\n## 背景\n",
            ),
            (
                "这时白头件的题目$\\sum_a^b$",
                "<!-- [正文] -->\n\n## 背景\n",
            ),
        ] {
            let text = drawn_titled(hint, markdown);
            assert!(text.contains("这时白头件的题目"), "{text}");
            assert!(!text.contains('$'), "{text}");
            assert!(!text.contains("\\sum"), "{text}");
        }
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

    fn targets(markdown: &str) -> Vec<(String, LabelKind, String, String)> {
        label_targets(markdown)
            .into_iter()
            .map(|t| (t.id, t.kind, t.number, t.title))
            .collect()
    }

    /// 交叉引用菜单按锚点挂的位置分类，编号与纸面同源，id 怎么起名不影响判断。
    #[test]
    fn label_targets_classify_by_position_and_carry_the_printed_number() {
        let table = "| 项 | 数 |\n| --- | --- |\n| 甲 | 1 |\n\n";
        let markdown = format!(
            "# 某某问题研究报告\n\n<!-- [部分] -->\n\n\
             # 第一部分 现状分析 {{#a}}\n\n\
             ## 研究背景 {{#b}}\n\n\
             ### 研究方法，见第{{@b}}章 {{#c}}\n\n\
             #### 数据来源 {{#d}}\n\n\
             ![总体架构](images/a.png){{#e}}\n\n\
             表：样本分布 {{#f}}\n\n{table}\
             <!-- [附录] -->\n\n## 调研问卷 {{#g}}\n\n### 问卷说明 {{#h}}\n"
        );
        let s = |v: &str| v.to_string();
        assert_eq!(
            targets(&markdown),
            vec![
                (s("a"), LabelKind::Part, s("第一部分"), s("现状分析")),
                (s("b"), LabelKind::Chapter, s("第1章"), s("研究背景")),
                (s("c"), LabelKind::Section, s("1.1"), s("研究方法，见第1章")),
                (s("d"), LabelKind::Subsection, s("1.1.1"), s("数据来源")),
                (s("e"), LabelKind::Figure, s("图 1.1"), s("总体架构")),
                (s("f"), LabelKind::Table, s("表 1.1"), s("样本分布")),
                (s("g"), LabelKind::Chapter, s("附录A"), s("调研问卷")),
                (s("h"), LabelKind::Section, s("A.1"), s("问卷说明")),
            ]
        );
    }

    /// 没有号可引的锚点（不编号标题、无图注图片、普通段落）不进菜单；重名只留第一次。
    #[test]
    fn label_targets_skip_what_cannot_be_numbered_and_keep_the_first_duplicate() {
        let markdown = "<!-- [正文] -->\n\n\
             <!-- [不编号] -->\n## 前言 {#pre}\n\n\
             ## 研究背景 {#dup}\n\n\
             ![](images/a.png){#bare}\n\n\
             ## 现状 {#dup}\n";
        assert_eq!(
            targets(markdown),
            vec![(
                "dup".to_string(),
                LabelKind::Chapter,
                "第1章".to_string(),
                "研究背景".to_string()
            )]
        );
    }

    /// 引文与文框：文框名称写什么印什么，每种名称各编各的号，口径与图表相同
    /// （章号.序号，不编号章里是流水号，附录带字母章号）；`{@id}` 印的是号，
    /// `>` 与 `[!名称]` 不印在纸上。
    #[test]
    fn inline_math_is_typeset_everywhere_the_pdf_typesets_it() {
        let text = drawn(concat!(
            "<!-- [摘要] -->\n\n",
            "## 摘要里的$\\alpha$\n\n",
            "<!-- [正文] -->\n\n",
            "## 我们的$\\sum$\n\n",
            "### 小节$x^2$\n\n",
            "> 引文 $\\sum{}$ 照排。\n\n",
            "> [!例子] 标题 $a_i$\n>\n> 内容 $\\sum_a=A$ 认证\n\n",
            "| 公式 | 说明 |\n| --- | --- |\n| $\\sum{}$ | 求和 |\n\n",
            "见“$\\sum$”。\n\n",
            // 居中 / 居右区、引文、文框里整行的 `$$` 是独立公式。
            "<!-- [居中] -->\n$\\sum{}$,**我们**\n$$\\sum_a^b$$\n\n",
            "<!-- [居右] -->\n$\\sum{}$，**还是**\n$$\\sum_a^b$$\n\n",
            "> [!例子] 标题\n>\n> $$\\sum_a^b$$\n\n",
            "> $$\\sum_a^b$$\n",
        ));
        assert!(text.contains("我们"), "{text}");
        assert!(text.contains("还是"), "{text}");
        assert!(!text.contains("**"), "{text}");
        // 公式都贴成图，纸面上不该再印出源码。
        for source in ["$", "\\sum", "\\alpha", "x^2", "a_i"] {
            assert!(!text.contains(source), "{source} 应排成公式：{text}");
        }
        assert!(text.contains("我们的"), "{text}");
        assert!(text.contains("内容"), "{text}");
        assert!(text.contains("求和"), "{text}");
        // 公式两侧的引号照常配对：一左一右。
        assert!(text.contains("见“"), "{text}");
        assert!(text.contains("”。"), "{text}");
    }

    #[test]
    fn quotes_print_as_citations_and_separately_numbered_boxes() {
        let text = drawn(concat!(
            "<!-- [正文] -->\n\n",
            "<!-- [不编号] -->\n## 前言\n\n",
            "> [!专栏] 名词解释\n>\n> 授权运营是指……\n\n",
            "## 地方实践\n\n",
            "见案例{@case:city}与专栏{@box:sg}。\n\n",
            "> 坚持统筹发展和安全。\n> - 明确授权条件；\n> ——《意见》\n\n",
            "> [!案例] 某市做法 {#case:city}\n>\n> 一是先定场景。\n\n",
            "> [!专栏] 新加坡经验 {#box:sg}\n>\n> ……\n\n",
            "> [!例子]\n>\n> 没有标题的例子。\n\n",
            "> [!案例] 某省做法\n\n",
            "<!-- [附录] -->\n\n## 附录的标题\n\n",
            "> [!案例] 附录里的案例\n",
        ));
        assert!(text.contains("专栏 1\u{3000}名词解释"), "{text}");
        assert!(text.contains("见案例1.1与专栏1.1。"), "{text}");
        assert!(text.contains("案例 1.1\u{3000}某市做法"), "{text}");
        assert!(text.contains("专栏 1.1\u{3000}新加坡经验"), "{text}");
        assert!(
            text.contains("例子 1.1\n"),
            "没有标题只印名称与编号：{text}"
        );
        assert!(text.contains("案例 1.2\u{3000}某省做法"), "{text}");
        assert!(text.contains("案例 A.1\u{3000}附录里的案例"), "{text}");
        assert!(text.contains("坚持统筹发展和安全。"), "{text}");
        assert!(text.contains("⑴ 明确授权条件；"), "{text}");
        assert!(text.contains("——《意见》"), "{text}");
        for symbol in ["> ", "[!", "{#", "{@"] {
            assert!(
                !text.contains(symbol),
                "源码符号“{symbol}”不应印在纸上：{text}"
            );
        }
    }

    fn with_bib(bib: &str) -> DraftInput {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..Default::default()
        };
        input.research.bibliography_content = bib.to_string();
        input
    }

    const BIB: &str = "@article{wang2020, author={王明}, title={数字政府}, journal={中国行政管理}, year={2020}}
@book{li2021, author={李华}, title={治理现代化}, publisher={人民出版社}, address={北京}, year={2021}}
@book{unused, author={赵磊}, title={未引用}, publisher={某社}, year={2022}}
";

    /// 参考文献表接在区段标题下：按首次引用先后编号，只列引过的，缺键写明待核实；
    /// 区段的标题已经排过，不再另起“参考文献”。
    #[test]
    fn the_bibliography_follows_the_reference_heading_in_citation_order() {
        let text = drawn_input(
            &with_bib(BIB),
            concat!(
                "<!-- [正文] -->

## 背景

先引[@li2021]，再引[@nope; @wang2020]。

",
                "<!-- [参考文献] -->

## 参考文献

<!-- [附录] -->

## 附表

附录正文。
",
            ),
        );
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("缺「{needle}」：{text}"))
        };
        assert!(at("[1]") < at("李华. 治理现代化[M]"), "{text}");
        assert!(at("[2]") < at("【待核实：文献库里没有 nope】"), "{text}");
        assert!(at("[3]") < at("王明. 数字政府[J]"), "{text}");
        assert!(
            at("王明. 数字政府[J]") < at("附录正文"),
            "表在附录之前：{text}"
        );
        assert!(!text.contains("未引用"), "没引的不进表：{text}");
        assert_eq!(text.matches("参考文献").count(), 1, "标题只排一次：{text}");
    }

    /// 没有参考文献区段时表排在全文末尾，自带“参考文献”标题；正文没引就不排。
    #[test]
    fn without_a_reference_section_the_bibliography_closes_the_report() {
        let text = drawn_input(
            &with_bib(BIB),
            "<!-- [正文] -->

## 背景

见[@wang2020]。

结尾一段。
",
        );
        let heading = text.find("参考文献").expect("自带标题");
        assert!(text.find("结尾一段").unwrap() < heading, "{text}");
        assert!(
            heading < text.find("王明. 数字政府[J]").expect("著录"),
            "{text}"
        );

        let uncited = drawn_input(
            &with_bib(BIB),
            "<!-- [正文] -->

## 背景

没有引用。
",
        );
        assert!(!uncited.contains("参考文献"), "{uncited}");
    }

    /// 文框进交叉引用菜单，编号带自己的名称；引文没有号，挂了锚点也不列。
    #[test]
    fn boxes_are_label_targets_and_citations_are_not() {
        let s = |v: &str| v.to_string();
        assert_eq!(
            targets(
                "## 地方实践\n\n> [!案例] 某市做法 {#case:city}\n>\n> 一是……\n\n\
                 > [!专栏] 新加坡经验 {#box:sg}\n\n> 引文 {#q}\n"
            ),
            vec![
                (s("case:city"), LabelKind::Box, s("案例 1.1"), s("某市做法")),
                (s("box:sg"), LabelKind::Box, s("专栏 1.1"), s("新加坡经验")),
            ]
        );
    }
}
