//! 正文块排版（段落、列表、公式、插图与表格）。
//!
//! 由 src/preview/research.rs 拆分而来：本文件是模块 `preview::research::blocks`，与其它子模块共享
//! `preview::research` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::{LocatedBlock, MarkdownBlock};
use crate::models::NumberingConfig;
use crate::preview::layout::{clickable, clickable_rows, is_renderable_paragraph};
use crate::preview::render::{clickable_content_block, diagram_image_block, research_image_block};
use crate::preview::research::{
    CAPTION_GAP_PT, CHAPTER_GAP, FLOAT_SEP_PT, Kind, SECTION_GAP, caption_line, chapter_title,
    part_title, section_title, space_after_float, title_line,
};
use crate::preview::{INDENT_CHARS, Metrics, RESEARCH_BODY_PT, indent, math_flow, research_quote};
use crate::theme;
use egui::Align;
use std::ops::Range;

#[allow(clippy::too_many_arguments)]
pub(crate) fn body_item(
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
                    title_line(
                        ui,
                        metrics,
                        &text,
                        theme::FONT_BOLD,
                        RESEARCH_BODY_PT,
                        Align::LEFT,
                        &indent(INDENT_CHARS),
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
                    research_image_block(ui, metrics, alt, src);
                    // `\caption` 排在 figure 环境的图下方，居中，与表题同一套字面。
                    if let Some(number) = &number {
                        ui.add_space(metrics.pt(CAPTION_GAP_PT));
                        caption_line(ui, metrics, "图", number, alt);
                    }
                },
            );
            space_after_float(ui, metrics);
        }
        Kind::Diagram {
            number,
            caption,
            source: diagram,
        } => {
            clickable(
                ui,
                metrics,
                &source,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| {
                    diagram_image_block(ui, metrics, diagram, true);
                    if let Some(number) = &number {
                        ui.add_space(metrics.pt(CAPTION_GAP_PT));
                        caption_line(ui, metrics, "图", number, caption);
                    }
                },
            );
            space_after_float(ui, metrics);
        }
        Kind::Table { caption } => {
            // 表题排在表上方：longtblr 的 caption 就在表头之前。
            ui.add_space(metrics.pt(FLOAT_SEP_PT));
            if let Some(caption) = &caption {
                clickable_rows(
                    ui,
                    metrics,
                    &caption.source,
                    anchor,
                    scroll_to_anchor,
                    clicked,
                    |ui| caption_line(ui, metrics, "表", &caption.number, &caption.text),
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
            space_after_float(ui, metrics);
        }
        Kind::Citation => research_quote::citation(
            ui,
            metrics,
            &located.block,
            anchor,
            scroll_to_anchor,
            clicked,
        ),
        Kind::Box { heading, .. } => research_quote::boxed(
            ui,
            metrics,
            &heading,
            &located.block,
            &source,
            anchor,
            scroll_to_anchor,
            clicked,
        ),
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
pub(crate) fn plain(
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
                    Some(src) => {
                        math_flow::display_block(ui, metrics, src, math_flow::display_mark(text))
                    }
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
