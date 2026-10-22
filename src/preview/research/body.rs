//! 研究报告预览入口与参考文献段。
//!
//! 由 src/preview/research.rs 拆分而来：本文件是模块 `preview::research::body`，与其它子模块共享
//! `preview::research` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::crossref;
use crate::export::{LocatedBlock, MarkdownBlock, ResearchSection, parse_research_marker};
use crate::models::{DraftInput, NumberingConfig};
use crate::preview::cull::Cull;
use crate::preview::layout::{clickable_rows, sheet};
use crate::preview::render::{BlockShape, PreviewOutput, anchored};
use crate::preview::research::{Kind, body_item, chapter_title, collect_marks, cover_sheet, walk};
use crate::preview::{Metrics, PreviewScale, RESEARCH_BODY_PT, gutter};
use crate::theme;
use std::ops::Range;

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
    let terminal = input.research.template == crate::models::ResearchTemplate::Terminal;
    let _paper = terminal.then(|| {
        theme::paper::report_paper(input.research.palette == crate::models::ResearchPalette::Dark)
    });
    let mut metrics = Metrics::research(scale.viewport.unwrap_or(visible.width()), scale.zoom)
        .with_bold_style(ui.ctx())
        .with_line_numbers(line_numbers);
    metrics.terminal = terminal;
    // 两遍：先给锚点和文献定号，再按纸面字面重新切块。研究报告的标题编号不跟
    // 设置里的公文编号样式走；序号表的分组编号跟设置，与导出一致。
    // 两遍解析只取决于正文与编号样式，正文没动就复用上一次的结果（`preview::memo`）。
    let bib = &input.research.bibliography_content;
    let located = crate::preview::memo::memo(
        ui.ctx(),
        "research-parse",
        crate::preview::memo::key((markdown, numbering, bib)),
        || {
            let marks = collect_marks(
                &export::parse_markdown_located_report(markdown, numbering),
                markdown,
                bib,
            );
            export::parse_markdown_located_research(markdown, &marks, numbering)
        },
    );
    let mut clicked = None;
    let terminal_entries =
        terminal.then(|| crate::preview::research_terminal::structure(&located, markdown));
    let mut toc_done = false;

    if terminal {
        crate::preview::research_terminal::cover(ui, &metrics, input, markdown);
    } else {
        cover_sheet(ui, &metrics, input, markdown);
    }

    ui.add_space(14.0);
    // 远离视野的块只占位、不排版（见 `preview::cull`）。编号都在 `walk` 里算好、
    // 放进 `Kind`，跳过哪一块都不影响后面的编号。
    let cull = Cull::new(ui, &metrics, "research");
    // 参考文献表：PDF 里接在参考文献区段的标题下面，没有这个区段就排在全文末尾
    // （mdx `typst_research` 的 `close_reference` / `finish`）。正文没引文献就不排。
    let library = export::bibliography::library(&input.research.bibliography_content);
    let cited = crate::preview::research_citations(ui.ctx(), markdown, bib);
    let mut bib = BibPlacement::default();
    let bib_key = |titled: bool| {
        crate::preview::memo::key((
            "research-bib",
            titled,
            &*cited,
            &input.research.bibliography_content,
        ))
    };
    sheet(ui, &metrics, |ui| {
        walk(&located, markdown, |located, kind, _| {
            if let Some(entries) = &terminal_entries
                && !toc_done
                && crate::preview::research_terminal::is_toc(&markdown[located.range.clone()])
            {
                toc_done = true;
                cull.block(
                    ui,
                    &metrics,
                    crate::preview::memo::key(("terminal-toc", entries)),
                    &located.range,
                    false,
                    None,
                    |ui| {
                        crate::preview::research_terminal::toc(ui, &metrics, entries, &mut clicked);
                    },
                );
            }
            if let Some(next) = section_marker(markdown, located) {
                if bib.in_references && !bib.done && !cited.is_empty() {
                    bib.done = true;
                    let titled = !bib.titled;
                    let at = located.range.start..located.range.start;
                    cull.block(ui, &metrics, bib_key(titled), &at, false, None, |ui| {
                        bibliography(ui, &metrics, titled, &cited, &library);
                    });
                }
                bib.in_references = next == ResearchSection::References;
                bib.titled = false;
            }
            if bib.in_references && matches!(kind, Kind::ChapterStar(_)) {
                bib.titled = true;
            }
            if matches!(kind, Kind::Skip | Kind::ReportTitle(_)) {
                return;
            }
            let heading = matches!(
                kind,
                Kind::Chapter { .. }
                    | Kind::ChapterStar(_)
                    | Kind::Part { .. }
                    | Kind::PartStar(_)
                    | Kind::Section { .. }
                    | Kind::SectionStar(_)
            )
            .then_some(&located.range);
            let keep = anchored(anchor, &located.range)
                || matches!(&kind, Kind::Table { caption: Some(caption) }
                    if anchored(anchor, &caption.source));
            let part_structure = matches!(kind, Kind::Part { .. } | Kind::PartStar(_))
                .then_some(terminal_entries.as_ref());
            let key =
                crate::preview::memo::key((BlockShape(located), &kind, terminal, part_structure));
            cull.block(ui, &metrics, key, &located.range, keep, heading, |ui| {
                if let Some(entries) = &terminal_entries
                    && let Some(entry) = entries.iter().find(|e| {
                        e.kind == crate::preview::research_terminal::EntryKind::Part
                            && e.source == located.range
                    })
                {
                    clickable_rows(
                        ui,
                        &metrics,
                        &located.range,
                        anchor,
                        &mut scroll_to_anchor,
                        &mut clicked,
                        |ui| {
                            crate::preview::research_terminal::part(ui, &metrics, entry, entries);
                        },
                    );
                    return;
                }
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
        if !bib.done && !cited.is_empty() {
            let titled = !(bib.in_references && bib.titled);
            let at = markdown.len()..markdown.len();
            cull.block(ui, &metrics, bib_key(titled), &at, false, None, |ui| {
                bibliography(ui, &metrics, titled, &cited, &library);
            });
        }
    });

    let numbered = gutter::paint(ui, &metrics, anchor);
    PreviewOutput {
        scale: metrics.scale,
        clicked: clicked.or(numbered),
    }
}

/// 参考文献表排到哪了：当前是否在参考文献区段、区段里的标题排过没有、表排过没有。
#[derive(Default)]
pub(crate) struct BibPlacement {
    in_references: bool,
    /// 区段的首个标题已经排成不编号的章：表直接接在它下面，不再另起“参考文献”。
    titled: bool,
    done: bool,
}

/// 这一块若是区段标记，返回它切到的区段。
pub(crate) fn section_marker(markdown: &str, located: &LocatedBlock) -> Option<ResearchSection> {
    match located.block {
        MarkdownBlock::Html(_) | MarkdownBlock::Marker(_) => markdown
            .get(located.range.clone())
            .and_then(|raw| parse_research_marker(raw.trim())),
        _ => None,
    }
}

/// 条目之间比行距多出的空：`research.typ` 的 `bib-block` 实测 2.80 mm。
pub(crate) const BIB_ITEM_GAP_PT: f32 = 2.8 / 25.4 * 72.0;

/// 参考文献表：序号悬挂在左、著录排在右边一栏，宋体随正文字号。著录取自
/// `export::bibliography`（与 Typst 同一份 GB/T 7714 样式），号取自第一遍。
/// 文献库里没有的键照样占号，写明缺失——PDF 那边会中止导出。
pub(crate) fn bibliography(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    titled: bool,
    cited: &[crossref::CitedKey],
    library: &export::bibliography::Library,
) {
    if titled {
        chapter_title(ui, metrics, "参考文献");
    }
    let font = metrics.font(metrics.body_family, RESEARCH_BODY_PT);
    let format = crate::preview::text_format(font.clone(), metrics.line);
    let label = |number: usize| {
        let mut job = crate::preview::job(f32::INFINITY);
        job.append(&format!("[{number}]"), 0.0, format.clone());
        crate::preview::layout(ui, job)
    };
    let labels: Vec<_> = cited.iter().map(|cited| label(cited.number)).collect();
    // 序号一栏按最宽的号留，再空半个字。
    let column = labels
        .iter()
        .map(|galley| galley.size().x)
        .fold(0.0, f32::max)
        + font.size * 0.5;
    for (index, (cited, number)) in cited.iter().zip(labels).enumerate() {
        if index > 0 {
            ui.add_space(metrics.pt(BIB_ITEM_GAP_PT));
        }
        let (text, color) = match library.get(&cited.key) {
            Some(entry) if !entry.formatted.is_empty() => {
                (entry.formatted.clone(), theme::paper::ink())
            }
            Some(entry) => (
                format!("{}【待核实：BibTeX 有错，排不出著录】", entry.title),
                theme::warn(),
            ),
            None => (
                format!("【待核实：文献库里没有 {}】", cited.key),
                theme::warn(),
            ),
        };
        let mut job = crate::preview::job(metrics.content - column);
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                color,
                ..format.clone()
            },
        );
        let galley = crate::preview::layout(ui, job);
        let height = galley.size().y.max(number.size().y);
        crate::preview::place(ui, metrics, height, |painter, rect| {
            painter.galley(rect.left_top(), number, theme::paper::ink());
            painter.galley(
                rect.left_top() + egui::vec2(column, 0.0),
                galley,
                theme::paper::ink(),
            );
        });
    }
}
