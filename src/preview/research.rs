//! 研究报告的纸面预览。
//!
//! 公文那套版式（红头、主送、落款、版记）对研究报告一条都不适用，因此这里不
//! 复用 `official_preview` 的版面，只共用画纸、画行和行号这些底层部件。版心、
//! 字号、行距、标题层级都对齐 mdx 的 `md2tex.cls`：
//!
//! - 版心 `left=28mm, top=37mm, width=156mm`，正文 14bp / 24pt；
//! - `##` 是章（二号黑体居中，编号"第 N 章"），`###`、`####` 依次是节和小节，
//!   编号 `N.M`、`N.M.K`，由程序生成，正文里不写；
//! - 摘要、参考文献这类区段用不编号的章标题。
//!
//! 字面是近似：预览只加载得到公文那五个字体，方正书宋、方正黑体分别用宋体、
//! 黑体顶替。字号、行距、版心是准的，字形要看编译出来的 PDF——表单底部那句
//! "最终版式以 TeX 编译 PDF 为准"说的就是这件事。

use super::layout::{clickable, line_block, sheet};
use super::render::{PreviewOutput, content_block};
use super::{
    Metrics, PreviewScale, RESEARCH_BODY_PT, RESEARCH_CHAPTER_PT, RESEARCH_COVER_PT,
    RESEARCH_COVER_TITLE_PT, gutter,
};
use crate::export::{self, LocatedBlock, MarkdownBlock};
use crate::models::DraftInput;
use crate::models::NumberingConfig;
use crate::theme;
use eframe::egui;
use egui::Align;
use std::ops::Range;

/// 研究报告里不参与编号的区段。正文区段（`<!-- [正文] -->`）只是恢复编号，
/// 不单独起标题。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResearchSection {
    Abstract,
    Body,
    Appendix,
    ChangeLog,
    References,
}

impl ResearchSection {
    /// 这一段在纸面上的标题；正文段没有标题。
    fn heading(self) -> Option<&'static str> {
        match self {
            Self::Abstract => Some("摘要"),
            Self::Body => None,
            Self::Appendix => Some("附录"),
            Self::ChangeLog => Some("版本变更记录"),
            Self::References => Some("参考文献"),
        }
    }

    /// 附录的章用字母编号，其余不编号段落里的章标题一律不编号。
    fn numbers_chapters(self) -> bool {
        matches!(self, Self::Body)
    }
}

/// 识别 mdx research 的区段标记。`export::parse_section_marker` 只认公文那两种，
/// 研究报告多出摘要、版本变更记录和参考文献。
fn parse_research_marker(line: &str) -> Option<ResearchSection> {
    let inner = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .trim_start_matches(['[', '【'])
        .trim_end_matches([']', '】'])
        .trim()
        .to_ascii_lowercase();
    match inner.as_str() {
        "摘要" | "abstract" => Some(ResearchSection::Abstract),
        "正文" | "body" => Some(ResearchSection::Body),
        "附录" | "附件" | "appendix" | "attachment" => Some(ResearchSection::Appendix),
        "版本变更记录" | "changelog" | "version" => Some(ResearchSection::ChangeLog),
        "参考文献" | "references" | "bibliography" => Some(ResearchSection::References),
        _ => None,
    }
}

/// 章节编号。研究报告的编号一律由程序生成，正文里手工写的编号在解析阶段就被
/// `clean_heading_number` 剥掉了，这里不会重复。
#[derive(Default)]
struct Counters {
    chapter: usize,
    section: usize,
    subsection: usize,
}

impl Counters {
    fn chapter(&mut self) -> String {
        self.chapter += 1;
        self.section = 0;
        self.subsection = 0;
        format!("第 {} 章", self.chapter)
    }

    fn section(&mut self) -> String {
        self.section += 1;
        self.subsection = 0;
        format!("{}.{}", self.chapter.max(1), self.section)
    }

    fn subsection(&mut self) -> String {
        self.subsection += 1;
        format!(
            "{}.{}.{}",
            self.chapter.max(1),
            self.section.max(1),
            self.subsection
        )
    }
}

pub(crate) fn research_preview(
    ui: &mut egui::Ui,
    input: &DraftInput,
    markdown: &str,
    scale: PreviewScale,
    anchor: Option<&Range<usize>>,
    mut scroll_to_anchor: bool,
    line_numbers: bool,
) -> PreviewOutput {
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|input| input.content_rect()));
    let metrics = Metrics::research(scale.viewport.unwrap_or(visible.width()), scale.zoom)
        .with_line_numbers(line_numbers);
    // 研究报告的标题编号不跟设置里的公文编号样式走，用默认值解析即可。
    let located = export::parse_markdown_located(markdown);
    let mut clicked = None;

    cover_sheet(ui, &metrics, input);

    ui.add_space(14.0);
    sheet(ui, &metrics, |ui| {
        let mut counters = Counters::default();
        let mut section = ResearchSection::Body;
        for located in &located {
            body_item(
                ui,
                &metrics,
                located,
                &mut section,
                &mut counters,
                anchor,
                &mut scroll_to_anchor,
                &mut clicked,
                markdown,
            );
        }
    });

    let numbered = gutter::paint(ui, &metrics, anchor);
    PreviewOutput {
        scale: metrics.scale,
        clicked: clicked.or(numbered),
    }
}

/// 简化封面：只排文字要素，不画 `md2tex.cls` 那圈 TikZ 双线页框——页框是纯
/// 装饰，编译出的 PDF 才是定稿版式，在预览里复刻它的收益抵不上维护成本。
fn cover_sheet(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput) {
    let meta = &input.research;
    sheet(ui, metrics, |ui| {
        ui.add_space(metrics.line);
        // 密级★保密年限：与 md2tex 模板的 \securitymark 一致，年限为空时不出星。
        let security = match (meta.security.trim(), meta.security_years.trim()) {
            ("", _) => String::new(),
            (level, "") => level.to_string(),
            (level, years) => format!("{level}★{years}"),
        };
        if !security.is_empty() {
            line_block(
                ui,
                metrics,
                &security,
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::RIGHT,
            );
        }
        if !meta.file_number.trim().is_empty() {
            line_block(
                ui,
                metrics,
                meta.file_number.trim(),
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::LEFT,
            );
        }

        ui.add_space(metrics.line * 6.0);
        let doc_type = meta.file_type.trim();
        if !doc_type.is_empty() {
            line_block(
                ui,
                metrics,
                doc_type,
                theme::FONT_HEITI,
                RESEARCH_CHAPTER_PT,
                Align::Center,
            );
            ui.add_space(metrics.line * 2.0);
        }

        let title = if input.title_hint.trim().is_empty() {
            "【待核实：文件名称】"
        } else {
            input.title_hint.trim()
        };
        line_block(
            ui,
            metrics,
            title,
            theme::FONT_BIAOSONG,
            RESEARCH_COVER_TITLE_PT,
            Align::Center,
        );

        if !meta.version.trim().is_empty() {
            ui.add_space(metrics.line * 2.0);
            line_block(
                ui,
                metrics,
                &format!("版本：{}", meta.version.trim()),
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::Center,
            );
        }

        ui.add_space(metrics.line * 6.0);
        for text in [meta.institution.trim(), meta.date.trim()] {
            if text.is_empty() {
                continue;
            }
            line_block(
                ui,
                metrics,
                text,
                theme::FONT_HEITI,
                RESEARCH_COVER_PT,
                Align::Center,
            );
            ui.add_space(metrics.line * 0.5);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn body_item(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    located: &LocatedBlock,
    section: &mut ResearchSection,
    counters: &mut Counters,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    markdown: &str,
) {
    match &located.block {
        // 研究报告的文件名称由文档要素维护，正文里的 `#` 不排进版面；
        // 校验那边已经就此给出提示。
        MarkdownBlock::Title(_) => {}
        MarkdownBlock::Html(text) => {
            if let Some(next) = parse_research_marker(text) {
                enter_section(ui, metrics, next, section, counters);
            }
        }
        MarkdownBlock::Marker(_) => {
            // 公文解析器把 `<!-- [正文] -->`、`<!-- [附录] -->` 识别成了它自己的
            // 区段，落到这里按原文重新判定，免得附录被当成公文附件。
            let text = &markdown[located.range.clone()];
            if let Some(next) = parse_research_marker(text) {
                enter_section(ui, metrics, next, section, counters);
            }
        }
        MarkdownBlock::Heading(level, text) => {
            let heading = match level {
                2 if section.numbers_chapters() => {
                    format!("{}　{text}", counters.chapter())
                }
                2 => text.clone(),
                3 => format!("{}　{text}", counters.section()),
                _ => format!("{}　{text}", counters.subsection()),
            };
            let centered = *level == 2;
            clickable(
                ui,
                metrics,
                &located.range,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| {
                    if centered {
                        ui.add_space(metrics.line);
                    }
                    line_block(
                        ui,
                        metrics,
                        &heading,
                        theme::FONT_HEITI,
                        if centered {
                            RESEARCH_CHAPTER_PT
                        } else {
                            RESEARCH_BODY_PT
                        },
                        if centered { Align::Center } else { Align::LEFT },
                    );
                    if centered {
                        ui.add_space(metrics.line);
                    }
                },
            );
        }
        // 段落、表格、图片、列表都按共用部件画：字面与字号已经跟着 Metrics
        // 走，这里拿到的就是研究报告的版式。标题在上面单独处理，走不到
        // content_block 里那套公文编号。
        _ => {
            let mut counters = [0usize; 4];
            clickable(
                ui,
                metrics,
                &located.range,
                anchor,
                scroll_to_anchor,
                clicked,
                |ui| {
                    content_block(
                        ui,
                        metrics,
                        &located.block,
                        &mut counters,
                        false,
                        &NumberingConfig::default(),
                    );
                },
            );
        }
    }
}

/// 切到新区段：不编号的区段先排它自己的标题，再把章节计数归零。
fn enter_section(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    next: ResearchSection,
    section: &mut ResearchSection,
    counters: &mut Counters,
) {
    *section = next;
    counters.section = 0;
    counters.subsection = 0;
    if let Some(heading) = next.heading() {
        ui.add_space(metrics.line);
        line_block(
            ui,
            metrics,
            heading,
            theme::FONT_HEITI,
            RESEARCH_CHAPTER_PT,
            Align::Center,
        );
        ui.add_space(metrics.line);
    }
}
