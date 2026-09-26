//! 正文渲染：分页入口 official_preview、正文各块与图片。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::render`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::RedlineKind;
use crate::export::{LocatedBlock, MarkdownBlock, MarkdownSection};
use crate::images;
use crate::models::{DraftInput, NumberingConfig, TemplateKind};
use crate::preview::cull::Cull;
use crate::preview::gutter;
use crate::preview::marks;
use crate::preview::pdf_figure;
use crate::preview::{
    BODY_PT, BodyRun, ClickableSourceSegment, INDENT_CHARS, Metrics, PreviewScale, TITLE_PT,
    addressee_block, aligned_block, append_inline, body_block, clickable, clickable_body_block,
    clickable_justified_job, draw_justified, footer_record, header_block, heading_family, indent,
    is_renderable_paragraph, job, line_block, place, red_approval_print_preview, sheet,
    signature_block, table_block, text_format,
};
use crate::theme;
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use eframe::egui;
use eframe::egui::text::LayoutJob;
use eframe::egui::{Align, Stroke};
use std::ops::Range;

/// 预览的一次绘制结果。
pub struct PreviewOutput {
    /// 本帧实际使用的缩放倍率，供“适应宽度”状态下的加减档以它为起点。
    pub scale: f32,
    /// 本帧被点击的正文块在 Markdown 源码中的字节范围。
    pub clicked: Option<Range<usize>>,
}

/// 块的排版指纹：块内容、源码长度与段内各源码行的相对位置。不含块在源码中的
/// 绝对位置——前面增删几个字，后面的块整体挪位，排版结果照样能复用。
pub(crate) struct BlockShape<'a>(pub(crate) &'a LocatedBlock);

impl std::hash::Hash for BlockShape<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let located = self.0;
        let base = located.range.start;
        located.block.hash(state);
        located.range.len().hash(state);
        for segment in &located.source_segments {
            (segment.source.start.saturating_sub(base)).hash(state);
            (segment.source.end.saturating_sub(base)).hash(state);
            segment.chars.hash(state);
        }
        located.generated_prefixes.hash(state);
    }
}

/// 单块（非紧缩标题）的缓存键。只有标题的排版取决于编号计数器——编号字数
/// 不同，折行就可能不同；段落、表格不带计数器，前面加一个标题不连累它们重排。
fn block_key(
    located: &LocatedBlock,
    counters: &[usize; 4],
    numbered: bool,
    numbering: &NumberingConfig,
) -> u64 {
    let counters = matches!(located.block, MarkdownBlock::Heading(..)).then_some(*counters);
    super::memo::key(("block", BlockShape(located), counters, numbered, numbering))
}

/// 锚点是否落在 `range` 里：落在里面的块必须现排（要铺底色、要滚过去）。
pub(crate) fn anchored(anchor: Option<&Range<usize>>, range: &Range<usize>) -> bool {
    anchor.is_some_and(|anchor| anchor.start <= range.end && range.start <= anchor.end)
}

/// 逐块画正文。远离视野的块交给 `cull` 只占位（见 `preview::cull`）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn body_blocks(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    cull: &Cull,
    body: &[&LocatedBlock],
    run: &BodyRun,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    counters: &mut [usize; 4],
    numbering: &NumberingConfig,
    markdown: &str,
) {
    let mut index = 0usize;
    while index < body.len() {
        let located = body[index];
        index += 1;
        match &located.block {
            // 文档标题已在上面按版式排过，正文区不再重复。
            MarkdownBlock::Title(_) => {}
            MarkdownBlock::Heading(level, heading)
                if run.compact_headings[index - 1]
                    && matches!(body.get(index), Some(next)
                        if matches!(&next.block, MarkdownBlock::Paragraph(text)
                            if is_renderable_paragraph(text))) =>
            {
                let Some(next) = body.get(index) else {
                    unreachable!("已在守卫里确认过后面还有块")
                };
                let MarkdownBlock::Paragraph(text) = &next.block else {
                    unreachable!("已在守卫里确认过是正文段落")
                };
                index += 1;
                let range = located.range.start..next.range.end;
                let key = (
                    "compact",
                    BlockShape(located),
                    BlockShape(next),
                    next.range.start - located.range.start,
                    *counters,
                    run.numbered,
                    numbering,
                );
                let drawn = cull.block(
                    ui,
                    metrics,
                    key,
                    &located.range,
                    anchored(anchor, &range),
                    Some(&located.range),
                    |ui| {
                        let body_segments = paragraph_source_segments(markdown, next, text);
                        clickable_compact_block(
                            ui,
                            metrics,
                            *level,
                            heading,
                            text,
                            located.range.clone(),
                            &body_segments,
                            counters,
                            run.numbered,
                            numbering,
                            anchor,
                            scroll_to_anchor,
                            clicked,
                        );
                    },
                );
                // 跳过排版的标题照样占号，后面的编号才接得上。
                if !drawn && run.numbered {
                    let _ = export::official_heading_text(*level, heading, counters, numbering);
                }
            }
            block => {
                let key = block_key(located, counters, run.numbered, numbering);
                let heading = matches!(block, MarkdownBlock::Heading(..)).then_some(&located.range);
                let drawn = cull.block(
                    ui,
                    metrics,
                    key,
                    &located.range,
                    anchored(anchor, &located.range),
                    heading,
                    |ui| {
                        clickable_content_block(
                            ui,
                            metrics,
                            located,
                            markdown,
                            counters,
                            run.numbered,
                            numbering,
                            anchor,
                            scroll_to_anchor,
                            clicked,
                        )
                    },
                );
                if !drawn
                    && run.numbered
                    && let MarkdownBlock::Heading(level, _) = block
                {
                    let _ = export::official_heading_prefix(*level, counters, numbering);
                }
            }
        }
    }
}

/// 一个正文块的可点击渲染：段落、标题、列表这些成行的块按行贴着文字高亮，
/// 行首缩进的空白不会被底色压住；表格、图片等整块图形仍按块矩形高亮。
#[allow(clippy::too_many_arguments)]
pub(crate) fn clickable_content_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    located: &LocatedBlock,
    markdown: &str,
    counters: &mut [usize; 4],
    numbered: bool,
    numbering: &NumberingConfig,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let source = located.range.clone();
    match &located.block {
        MarkdownBlock::Paragraph(text) if is_renderable_paragraph(text) => {
            let segments = paragraph_source_segments(markdown, located, text);
            clickable_body_block(
                ui,
                metrics,
                text,
                true,
                &segments,
                anchor,
                scroll_to_anchor,
                clicked,
            );
        }
        MarkdownBlock::Heading(level, text) => {
            let Some(job) = heading_job(metrics, *level, text, counters, numbered, numbering)
            else {
                return;
            };
            let segments = [ClickableSourceSegment {
                source,
                chars: 0..job.text.chars().count(),
            }];
            clickable_justified_job(
                ui,
                metrics,
                job,
                &segments,
                anchor,
                scroll_to_anchor,
                clicked,
            );
        }
        MarkdownBlock::OrderedListItem { number, text } => {
            let prefix = export::render_list_number(numbering.list2, *number);
            clickable_text_block(
                ui,
                metrics,
                &format!("{prefix}{text}"),
                true,
                source,
                anchor,
                scroll_to_anchor,
                clicked,
            );
        }
        block => clickable(
            ui,
            metrics,
            &source,
            anchor,
            scroll_to_anchor,
            clicked,
            |ui| {
                content_block(ui, metrics, block, counters, numbered, numbering);
            },
        ),
    }
}

/// 整块对应一行源码的正文块（列表项等）：命中范围就是整段可见文字。
#[allow(clippy::too_many_arguments)]
fn clickable_text_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    first_line_indent: bool,
    source: Range<usize>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let segments = [ClickableSourceSegment {
        source,
        chars: 0..export::inline_visible_char_index(text, text.len()),
    }];
    clickable_body_block(
        ui,
        metrics,
        text,
        first_line_indent,
        &segments,
        anchor,
        scroll_to_anchor,
        clicked,
    );
}

/// 公文预览每帧都要的解析与切块结果，按正文内容缓存（见 `preview::memo`）。
struct OfficialParse {
    located: Vec<LocatedBlock>,
    /// 正文区的块（`located` 下标）：首个标题加上附件标记之前的内容。
    body: Vec<usize>,
    /// 各份附件的块，每份附件从新纸开始。
    attachments: Vec<Vec<usize>>,
    /// 附件概要里列出的附件名。
    names: Vec<String>,
    /// 正文区每个块是否与后文合成紧缩标题，下标与 `body` 对应。
    compact_headings: Vec<bool>,
    /// 正文首个 `# ` 的文字（保留花脸稿标记）与源码范围。
    title: Option<(String, Range<usize>)>,
}

impl OfficialParse {
    fn new(
        markdown: &str,
        numbering: &NumberingConfig,
        style_mode: crate::models::StyleMode,
    ) -> Self {
        let located = export::parse_markdown_located_with_numbering(markdown, numbering);
        let blocks = located
            .iter()
            .map(|block| block.block.clone())
            .collect::<Vec<_>>();
        let names = export::attachment_names(&blocks);

        // 先按正文/附件切分。红头呈批件正文还会在专用分页器里继续拆成真实页；
        // 每份附件仍从新纸开始。
        let mut body = Vec::new();
        let mut attachments: Vec<Vec<usize>> = Vec::new();
        let mut in_attachment = false;
        let mut seen_title = false;
        for (index, located) in located.iter().enumerate() {
            match &located.block {
                MarkdownBlock::Title(_) if !seen_title && !in_attachment => {
                    seen_title = true;
                    body.push(index);
                }
                MarkdownBlock::Marker(section) => {
                    in_attachment = matches!(section, MarkdownSection::Attachment);
                    if in_attachment {
                        attachments.push(Vec::new());
                    }
                }
                _ if in_attachment => match attachments.last_mut() {
                    Some(last) => last.push(index),
                    None => attachments.push(vec![index]),
                },
                _ => body.push(index),
            }
        }
        let body_plain = body
            .iter()
            .map(|&index| blocks[index].clone())
            .collect::<Vec<_>>();
        let compact_headings = export::compact_heading_flags(&body_plain, style_mode);
        let title = body.iter().find_map(|&index| match &located[index].block {
            MarkdownBlock::Title(text) => {
                Some((marks::plain_keep_marks(text), located[index].range.clone()))
            }
            _ => None,
        });
        Self {
            located,
            body,
            attachments,
            names,
            compact_headings,
            title,
        }
    }
}

/// 把 Markdown 连同表单锁定的行文要素按公文版式画在 `ui` 里；调用方负责套滚动区。
/// 返回本次实际使用的缩放倍率，供“适应宽度”状态下的加减档以它为起点。
#[allow(clippy::too_many_arguments)]
pub(crate) fn official_preview(
    ui: &mut egui::Ui,
    input: &DraftInput,
    display: &UnitDisplay,
    markdown: &str,
    scale: PreviewScale,
    anchor: Option<&Range<usize>>,
    mut scroll_to_anchor: bool,
    numbering: &NumberingConfig,
    line_numbers: bool,
    elements: &ElementMarks,
) -> PreviewOutput {
    super::layout::clear_hovered(ui.ctx());
    // 研究报告不是公文：版心、字号、标题层级和封面全都另一套，没有红头、主送、
    // 落款和版记可言，因此整张纸交给专用版式画，不在下面的公文流程里打补丁。
    if input.kind.is_research() {
        return super::research::research_preview(
            ui,
            input,
            markdown,
            scale,
            anchor,
            scroll_to_anchor,
            numbering,
            line_numbers,
        );
    }
    // 自适应缩放要按“看得见的宽度”算：滚动方向上的 available_width 是无穷大，
    // 拿它算会把整页放大到上限。裁剪矩形就是滚动区的可视范围，再与窗口取交集兜底。
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|input| input.content_rect()));
    // 居中用的宽度优先取调用方给的真实宽度（见 `PreviewScale::viewport`）。
    let metrics = Metrics::new(scale.viewport.unwrap_or(visible.width()), scale.zoom)
        .with_line_numbers(line_numbers);
    // 六个文种的正文都走 export::latex::official_letter_sections_to_tex，标题编号
    // 跟随设置里的编号样式；紧缩风格跟随模板配置。
    let numbered = true;
    // 解析与切块只取决于正文、编号样式和紧缩风格，正文没动就复用上一次的结果。
    let parsed = super::memo::memo(
        ui.ctx(),
        "official-parse",
        super::memo::key((
            markdown,
            numbering,
            std::mem::discriminant(&input.profile.style_mode),
        )),
        || OfficialParse::new(markdown, numbering, input.profile.style_mode),
    );
    let body = parsed
        .body
        .iter()
        .map(|&index| &parsed.located[index])
        .collect::<Vec<_>>();
    let attachments = parsed
        .attachments
        .iter()
        .map(|sheet| {
            sheet
                .iter()
                .map(|&index| &parsed.located[index])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let names = &parsed.names;
    let compact_headings = parsed.compact_headings.clone();
    // 标题取正文首个 `# `，缺省回落表单里的标题提示，与导出器一致。
    let title = parsed
        .title
        .clone()
        .unwrap_or_else(|| (export::plain_text(input.title_hint.trim()), 0..0));
    let mut clicked = None;
    let mut counters = [0usize; 4];
    // 版记排在全文最后：有附件时跟在最后一份附件后面，没有附件时跟在落款后面。
    let record_on_body = attachments.is_empty();

    let (title, title_range) = title;
    if input.kind == TemplateKind::RedHeadApproval {
        red_approval_print_preview(
            ui,
            &metrics,
            input,
            display,
            &body,
            &attachments,
            &(title, title_range),
            names,
            anchor,
            &mut scroll_to_anchor,
            &mut clicked,
            numbering,
            markdown,
            elements,
        );
        // 行号每帧都要画，不能写成 `clicked.or_else(…)`：那样点中正文的那一帧
        // 会连带把页边整列号码漏掉，看上去就是闪一下。
        let numbered = gutter::paint(ui, &metrics, anchor);
        return PreviewOutput {
            scale: metrics.scale,
            clicked: clicked.or(numbered),
        };
    }
    let cull = Cull::new(ui, &metrics, "official");
    sheet(ui, &metrics, |ui| {
        header_block(ui, &metrics, input, display, elements);
        if !title.is_empty() {
            clickable(
                ui,
                &metrics,
                &title_range,
                anchor,
                &mut scroll_to_anchor,
                &mut clicked,
                |ui| {
                    line_block(
                        ui,
                        &metrics,
                        &title,
                        theme::FONT_BIAOSONG,
                        TITLE_PT,
                        Align::Center,
                    );
                },
            );
        }
        // 类里标题与主送（或正文）之间固定空一行。
        ui.add_space(metrics.line);
        addressee_block(ui, &metrics, input, display, elements);

        body_blocks(
            ui,
            &metrics,
            &cull,
            &body,
            &BodyRun {
                compact_headings,
                numbered,
            },
            anchor,
            &mut scroll_to_anchor,
            &mut clicked,
            &mut counters,
            numbering,
            markdown,
        );
        // 正文之后的附件概要：空两行再逐条列出（与导出一致）。
        if !names.is_empty() {
            ui.add_space(metrics.line * 2.0);
            for (index, name) in names.iter().enumerate() {
                // 多个附件只有第一行保留“附件”二字，其余行用两个全角空格占位对齐。
                let label = if names.len() == 1 {
                    format!("附件：{name}")
                } else if index == 0 {
                    format!("附件{}：{name}", index + 1)
                } else {
                    format!("　　{}：{name}", index + 1)
                };
                body_block(ui, &metrics, &label, true);
            }
        }
        signature_block(ui, &metrics, input, display, elements);
        if record_on_body {
            footer_record(ui, &metrics, input, display, elements);
        }
    });

    let last_attachment = attachments.len().saturating_sub(1);
    let attachment_count = attachments.len();
    for (sheet_index, attachment) in attachments.into_iter().enumerate() {
        ui.add_space(14.0);
        sheet(ui, &metrics, |ui| {
            let mut counters = [0usize; 4];
            let label = if attachment_count == 1 {
                "附件".to_string()
            } else {
                format!("附件{}", sheet_index + 1)
            };
            line_block(
                ui,
                &metrics,
                &label,
                theme::FONT_HEITI,
                BODY_PT,
                Align::LEFT,
            );
            for located in attachment {
                // 附件正式标题与正文标题使用同一层级编码。
                if let MarkdownBlock::Title(text) = &located.block {
                    counters.fill(0);
                    let range = located.range.clone();
                    clickable(
                        ui,
                        &metrics,
                        &range,
                        anchor,
                        &mut scroll_to_anchor,
                        &mut clicked,
                        |ui| {
                            line_block(
                                ui,
                                &metrics,
                                &marks::plain_keep_marks(text),
                                theme::FONT_BIAOSONG,
                                TITLE_PT,
                                Align::Center,
                            );
                            ui.add_space(metrics.pt(18.0));
                        },
                    );
                    continue;
                }
                let key = block_key(located, &counters, numbered, numbering);
                let heading =
                    matches!(located.block, MarkdownBlock::Heading(..)).then_some(&located.range);
                let drawn = cull.block(
                    ui,
                    &metrics,
                    key,
                    &located.range,
                    anchored(anchor, &located.range),
                    heading,
                    |ui| {
                        clickable_content_block(
                            ui,
                            &metrics,
                            located,
                            markdown,
                            &mut counters,
                            numbered,
                            numbering,
                            anchor,
                            &mut scroll_to_anchor,
                            &mut clicked,
                        )
                    },
                );
                // 跳过排版的标题照样占号，后面的编号才接得上。
                if !drawn && let MarkdownBlock::Heading(level, _) = &located.block {
                    let _ = export::official_heading_prefix(*level, &mut counters, numbering);
                }
            }
            if sheet_index == last_attachment {
                footer_record(ui, &metrics, input, display, elements);
            }
        });
    }
    // 行号压在纸面留白上，等纸和附件都画完再标：晚画才不会被纸底盖住。
    // 点中的号与点中的正文块是同一件事——都是「把光标带到这一行」。
    let numbered = gutter::paint(ui, &metrics, anchor);
    PreviewOutput {
        scale: metrics.scale,
        clicked: clicked.or(numbered),
    }
}

/// 一个自然段可以由多行 Markdown 软换行组成；成文仍连续排版，交互范围按源码行拆开。
pub(crate) fn paragraph_source_segments(
    _markdown: &str,
    located: &LocatedBlock,
    rendered_text: &str,
) -> Vec<ClickableSourceSegment> {
    if !located.source_segments.is_empty() {
        let segments = located
            .source_segments
            .iter()
            .map(|segment| ClickableSourceSegment {
                source: segment.source.clone(),
                chars: segment.chars.clone(),
            })
            .collect::<Vec<_>>();
        debug_assert_eq!(
            segments.last().map(|segment| segment.chars.end),
            Some(export::inline_visible_char_index(
                rendered_text,
                rendered_text.len()
            )),
            "解析器来源映射必须覆盖完整的段落可见文本"
        );
        return segments;
    }

    vec![ClickableSourceSegment {
        source: located.range.clone(),
        chars: 0..export::inline_visible_char_index(rendered_text, rendered_text.len()),
    }]
}

#[allow(clippy::too_many_arguments)]
fn clickable_compact_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    level: u8,
    heading: &str,
    body: &str,
    heading_source: Range<usize>,
    body_segments: &[ClickableSourceSegment],
    counters: &mut [usize; 4],
    numbered: bool,
    numbering: &NumberingConfig,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let heading = match numbered {
        true => match export::official_heading_text(level, heading, counters, numbering) {
            Some(text) => text,
            None => return,
        },
        false => heading.to_string(),
    };
    let normal = metrics.body_font();
    let mut job = job(metrics.content);
    job.append(
        &indent(INDENT_CHARS),
        0.0,
        text_format(normal.clone(), metrics.line),
    );
    // 与 DOCX 的 `compact_heading_paragraph` 一样，标题连同句号按花脸稿块标注。
    let heading_text = marks::plain_keep_marks(&format!("{heading}。"));
    marks::append_marked_text(
        &mut job,
        metrics,
        &heading_text,
        text_format(metrics.font(heading_family(level), BODY_PT), metrics.line),
    );
    let body_start = INDENT_CHARS as usize + export::strip_redline(&heading_text).chars().count();
    append_inline(&mut job, metrics, body, &normal);
    let mut segments = vec![ClickableSourceSegment {
        source: heading_source,
        chars: 0..body_start,
    }];
    segments.extend(body_segments.iter().map(|segment| ClickableSourceSegment {
        source: segment.source.clone(),
        chars: body_start + segment.chars.start..body_start + segment.chars.end,
    }));
    clickable_justified_job(
        ui,
        metrics,
        job,
        &segments,
        anchor,
        scroll_to_anchor,
        clicked,
    );
}

pub(crate) fn heading_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
    numbered: bool,
    numbering: &NumberingConfig,
) {
    if let Some(job) = heading_job(metrics, level, text, counters, numbered, numbering) {
        draw_justified(ui, metrics, job);
    }
}

/// 各级标题的排版任务：首行缩进 2 字，字体按层级取。
/// 编号规则跳过这一级（`official_heading_text` 返回 None）时不成段。
pub(super) fn heading_job(
    metrics: &Metrics,
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
    numbered: bool,
    numbering: &NumberingConfig,
) -> Option<LayoutJob> {
    let number = if numbered {
        Some(export::official_heading_prefix(level, counters, numbering)?)
    } else {
        None
    };
    let mut job = job(metrics.content);
    let font = metrics.font(heading_family(level), BODY_PT);
    job.append(
        &indent(INDENT_CHARS),
        0.0,
        text_format(font.clone(), metrics.line),
    );
    // 花脸稿：新增的标题连编号一起加框（方案规则 8），与 DOCX 的
    // `heading_paragraph_with_number`、TeX 的 `marked_heading_tex` 一致；
    // 其余情况编号照常排，只标文字。
    let text = marks::plain_keep_marks(text);
    let text = match number {
        Some(number) if export::whole_chunk_kind(&text) == Some(RedlineKind::Added) => {
            export::mark_added(&format!("{number}{}", export::strip_redline(&text)))
        }
        Some(number) => format!("{number}{text}"),
        None => text,
    };
    marks::append_marked_text(&mut job, metrics, &text, text_format(font, metrics.line));
    Some(job)
}

pub(crate) fn content_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    block: &MarkdownBlock,
    counters: &mut [usize; 4],
    numbered: bool,
    numbering: &NumberingConfig,
) {
    match block {
        MarkdownBlock::Heading(level, text) => {
            heading_block(ui, metrics, *level, text, counters, numbered, numbering);
        }
        MarkdownBlock::Paragraph(text) if is_renderable_paragraph(text) => {
            body_block(ui, metrics, text, true);
        }
        MarkdownBlock::OrderedListItem { number, text } => {
            let prefix = export::render_list_number(numbering.list2, *number);
            body_block(ui, metrics, &format!("{prefix}{text}"), true);
        }
        MarkdownBlock::Table {
            rows,
            aligns,
            spans,
            numbered,
        } => table_block(ui, metrics, rows, aligns, spans, *numbered),
        MarkdownBlock::Image { alt, src } => image_block(ui, metrics, alt, src),
        MarkdownBlock::Aligned { align, text } => aligned_block(ui, metrics, text, *align),
        MarkdownBlock::Title(_) | MarkdownBlock::Marker(_) | MarkdownBlock::Html(_) => {}
        MarkdownBlock::Paragraph(_) => {}
    }
}

/// 图片块：位图按版心宽度等比渲染，加载失败显示占位卡片；PDF 取第一页光栅化
/// 后同样按版心宽度画（见 `preview::pdf_figure`）。外层已由 clickable 包装。
pub(crate) fn image_block(ui: &mut egui::Ui, metrics: &Metrics, alt: &str, src: &str) {
    let file_name = src.rsplit('/').next().unwrap_or(src).to_string();
    let path = match images::resolve(src) {
        Ok(path) => path,
        Err(error) => return image_placeholder(ui, metrics, alt, &file_name, &error.to_string()),
    };
    let is_pdf = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        return pdf_block(ui, metrics, alt, src, &path, &file_name);
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return image_placeholder(ui, metrics, alt, &file_name, &format!("无法读取：{error}"));
        }
    };
    // 每张图片一个固定 uri，egui 的 ImageCache 按 uri 缓存解码结果，避免每帧重读重解码。
    // src 形如 `images/xxx.png`，直接用其做 uri 后缀保证唯一。
    let uri = format!("bytes://{src}");
    // 用 fit_to_original_size 按纹理尺寸布局，再受 max_size 限制（宽=版心、高不设限）。
    // 不能用默认的 Fraction 适配：滚动区里 available_size.y 是无穷大，会把图片高度
    // 撑成无穷大，矩形超出可视区域而不绘制。
    ui.add(
        egui::Image::from_bytes(uri, bytes)
            .fit_to_original_size(1.0)
            .max_size(egui::vec2(metrics.content, f32::INFINITY)),
    );
}

/// PDF 插图：第一页占满版心宽，与导出的 `\includegraphics[width=\textwidth]`
/// 一致。首帧还在后台渲染，先画占位卡片顶上。
fn pdf_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    alt: &str,
    src: &str,
    path: &std::path::Path,
    file_name: &str,
) {
    match pdf_figure::page(ui.ctx(), src, path) {
        pdf_figure::PdfPage::Ready(texture) => {
            let size = texture.size_vec2();
            let height = metrics.content * size.y / size.x.max(1.0);
            ui.add(
                egui::Image::from_texture(egui::load::SizedTexture::from_handle(&texture))
                    .fit_to_exact_size(egui::vec2(metrics.content, height)),
            );
        }
        pdf_figure::PdfPage::Rendering => {
            image_placeholder(ui, metrics, alt, file_name, "PDF 附件：正在渲染第一页…");
        }
        pdf_figure::PdfPage::Failed(error) => {
            image_placeholder(ui, metrics, alt, file_name, &error);
        }
    }
}

/// 图片占位卡片：细边框 + 文件名与说明，宽度=版心。
pub(crate) fn image_placeholder(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    alt: &str,
    file_name: &str,
    note: &str,
) {
    let height = (metrics.line * 2.0).max(metrics.pt(24.0));
    place(ui, metrics, height, |painter, rect| {
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(2),
            Stroke::new(1.0_f32.max(metrics.scale), theme::paper::ink_faint()),
            egui::StrokeKind::Inside,
        );
        let font = metrics.body_font();
        let caption = if alt.is_empty() {
            file_name.to_string()
        } else {
            format!("{file_name}（{alt}）")
        };
        let text = format!("【图片】{caption}\n{note}");
        let galley = painter.layout(
            text,
            font,
            theme::paper::ink_muted(),
            rect.width() - metrics.pt(8.0),
        );
        let y = rect.top() + ((rect.height() - galley.size().y) / 2.0).max(metrics.pt(4.0));
        painter.galley(
            egui::pos2(rect.left() + metrics.pt(4.0), y),
            galley,
            theme::paper::bg(),
        );
    });
}

#[cfg(test)]
mod source_segment_tests {
    use super::*;

    #[test]
    fn a_soft_wrapped_paragraph_keeps_one_click_target_per_markdown_line() {
        let markdown = "第一行\n**第二行**";
        let located = export::parse_markdown_located(markdown).remove(0);
        let MarkdownBlock::Paragraph(text) = &located.block else {
            panic!("expected paragraph")
        };
        let segments = paragraph_source_segments(markdown, &located, text);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].source, 0.."第一行".len());
        let second_start = "第一行\n".len();
        assert_eq!(segments[1].source, second_start..markdown.len());
        assert_eq!(segments[0].chars, 0..3);
        assert_eq!(segments[1].chars, 3..6);
    }

    #[test]
    fn adjacent_ordered_items_keep_exact_source_line_boundaries() {
        let markdown = "正文内容：\n1. 第一项，\n1. 第二项。；\n1. 第三项，";
        let located = export::parse_markdown_located(markdown).remove(0);
        let MarkdownBlock::Paragraph(text) = &located.block else {
            panic!("expected paragraph")
        };
        assert_eq!(text, "正文内容：①第一项；②第二项；③第三项。");
        let segments = paragraph_source_segments(markdown, &located, text);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.chars.clone())
                .collect::<Vec<_>>(),
            [0..5, 5..10, 10..15, 15..20]
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| &markdown[segment.source.clone()])
                .collect::<Vec<_>>(),
            ["正文内容：", "1. 第一项，", "1. 第二项。；", "1. 第三项，"]
        );
    }

    #[test]
    fn escaped_characters_and_cross_line_bold_use_visible_boundaries() {
        let markdown = "字段\\_名**称\n继续**填写";
        let located = export::parse_markdown_located(markdown).remove(0);
        let MarkdownBlock::Paragraph(text) = &located.block else {
            panic!("expected paragraph")
        };
        let segments = paragraph_source_segments(markdown, &located, text);
        assert_eq!(export::plain_text(text), "字段_名称继续填写");
        assert_eq!(segments[0].chars, 0..5);
        assert_eq!(segments[1].chars, 5..9);
    }

    #[test]
    fn latin_soft_wrap_space_belongs_to_the_following_source_line() {
        let markdown = "the quick brown\nfox jumps";
        let located = export::parse_markdown_located(markdown).remove(0);
        let MarkdownBlock::Paragraph(text) = &located.block else {
            panic!("expected paragraph")
        };
        assert_eq!(text, "the quick brown fox jumps");
        let segments = paragraph_source_segments(markdown, &located, text);
        assert_eq!(segments[0].chars, 0..15);
        assert_eq!(segments[1].chars, 15..25);
    }
}
