//! 正文与附件分区 → 模板块。逐条对应 TeX 路径的
//! `official_letter_sections_to_tex_with_barrier_with_numbering`：同一套标题编号、
//! 紧缩合并、附件分区与横页判定，只是产出模板数据。

use super::data::{Attachment, Block, Runs, Title};
use super::runs::{body_runs, heading_runs, marked_runs};
use crate::export::latex::attachment_landscape_flags;
use crate::export::title::{self, TitlePlan};
use crate::export::{
    LineAlign, MarkdownBlock, MarkdownSection, attachment_names, official_heading_prefix,
    plain_text, redline_slice_lines, render_list_number,
};
use crate::models::{NumberingConfig, StyleMode};

/// 正文区与附件区的块。
pub(crate) struct Sections {
    pub body: Vec<Block>,
    pub attachments: Vec<Attachment>,
}

/// 标题排布（主标题、附件标题、红头呈批件首页标题共用）：单行、压缩或按词换行。
pub(crate) fn title_data(
    text: &str,
    chars_per_line: usize,
    scale_for: impl Fn(&str) -> usize,
) -> Title {
    let plain = plain_text(text);
    match title::title_plan(&plain, chars_per_line) {
        TitlePlan::SingleLine => Title {
            lines: vec![marked_runs(text)],
            scale: 1.0,
        },
        TitlePlan::Compressed => Title {
            lines: vec![marked_runs(text)],
            scale: scale_for(&plain) as f64 / 100.0,
        },
        TitlePlan::Wrapped(lines) => Title {
            lines: redline_slice_lines(text, &lines)
                .iter()
                .map(|line| marked_runs(line))
                .collect(),
            scale: 1.0,
        },
    }
}

/// 公文主标题与附件标题（二号、版心宽）。
pub(crate) fn main_title(text: &str) -> Title {
    title_data(
        text,
        title::chars_per_line(),
        title::compressed_scale_percent,
    )
}

/// 附件说明各行：单个附件「：名称」，多个附件「N：名称」；「附件」二字由模板补。
pub(crate) fn attachment_summary(blocks: &[MarkdownBlock]) -> Vec<Runs> {
    let names = attachment_names(blocks);
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let label = if names.len() == 1 {
                format!("：{name}")
            } else {
                format!("{}：{name}", index + 1)
            };
            body_runs(&label)
        })
        .collect()
}

fn is_plain_paragraph(text: &str) -> bool {
    !text.contains("<div") && !text.contains("</div")
}

/// `barrier`：红头呈批件在正文区第一个表格 / 图片之前插一道屏障（附件区不插）。
pub(crate) fn sections(
    blocks: &[MarkdownBlock],
    lines: &[usize],
    style_mode: StyleMode,
    barrier: bool,
    numbering: &NumberingConfig,
) -> Sections {
    let mut body_barrier = barrier;
    let mut body: Vec<Block> = Vec::new();
    let mut attachments: Vec<Attachment> = Vec::new();
    let landscape = attachment_landscape_flags(blocks);
    let attachment_count = blocks
        .iter()
        .filter(|block| matches!(block, MarkdownBlock::Marker(MarkdownSection::Attachment)))
        .count();
    let mut section = MarkdownSection::Body;
    let mut seen_document_title = false;
    let mut counters = [0usize; 4];
    let compact_headings = crate::export::compact_heading_flags(blocks, lines, style_mode);
    let mut table_serial = 0usize;

    // 当前分区的块序列：正文直接写进 body，附件写进最后一个附件。
    fn target<'a>(
        section: MarkdownSection,
        body: &'a mut Vec<Block>,
        attachments: &'a mut [Attachment],
    ) -> &'a mut Vec<Block> {
        match (section, attachments.last_mut()) {
            (MarkdownSection::Attachment, Some(attachment)) => &mut attachment.blocks,
            _ => body,
        }
    }

    let mut index = 0usize;
    while index < blocks.len() {
        let block = &blocks[index];
        let line = lines.get(index).copied();
        match block {
            MarkdownBlock::Title(_) if !seen_document_title && section == MarkdownSection::Body => {
                seen_document_title = true;
            }
            MarkdownBlock::Title(text) if section == MarkdownSection::Attachment => {
                counters = [0; 4];
                if let Some(attachment) = attachments.last_mut() {
                    attachment.title = main_title(text);
                }
            }
            MarkdownBlock::Title(_) => {}
            MarkdownBlock::Marker(MarkdownSection::Attachment) => {
                section = MarkdownSection::Attachment;
                counters = [0; 4];
                let ordinal = attachments.len();
                let label = if attachment_count == 1 {
                    "附件".to_string()
                } else {
                    format!("附件{}", ordinal + 1)
                };
                attachments.push(Attachment {
                    label,
                    landscape: landscape.get(ordinal).copied().unwrap_or(false),
                    title: Title {
                        lines: Vec::new(),
                        scale: 1.0,
                    },
                    blocks: Vec::new(),
                });
            }
            MarkdownBlock::Marker(MarkdownSection::Body) => {
                section = MarkdownSection::Body;
                counters = [0; 4];
            }
            MarkdownBlock::Heading(level, text) => {
                let next_is_paragraph = blocks.get(index + 1).is_some_and(|next| {
                    matches!(next, MarkdownBlock::Paragraph(p)
                        if !p.trim().is_empty() && is_plain_paragraph(p))
                });
                if compact_headings[index]
                    && section == MarkdownSection::Body
                    && next_is_paragraph
                    && let Some(number) = official_heading_prefix(*level, &mut counters, numbering)
                {
                    let MarkdownBlock::Paragraph(body_text) = &blocks[index + 1] else {
                        unreachable!()
                    };
                    let mut head = heading_runs(&number, text);
                    head.push(super::data::Run::text("。"));
                    target(section, &mut body, &mut attachments).push(Block::Compact {
                        level: *level,
                        head,
                        runs: body_runs(body_text),
                        line: lines.get(index + 1).copied(),
                    });
                    index += 1;
                } else if let Some(number) =
                    official_heading_prefix(*level, &mut counters, numbering)
                    && (2..=5).contains(level)
                {
                    target(section, &mut body, &mut attachments).push(Block::Heading {
                        level: *level,
                        runs: heading_runs(&number, text),
                        line,
                    });
                }
            }
            MarkdownBlock::Paragraph(text) => {
                if is_plain_paragraph(text) {
                    target(section, &mut body, &mut attachments).push(Block::Par {
                        runs: body_runs(text),
                        line,
                    });
                }
            }
            MarkdownBlock::Aligned { align, text } => {
                target(section, &mut body, &mut attachments).push(Block::Aligned {
                    align: match align {
                        LineAlign::Center => "center",
                        LineAlign::Right => "right",
                    },
                    runs: body_runs(text),
                });
            }
            MarkdownBlock::OrderedListItem { number, text } => {
                let prefix = render_list_number(numbering.list2, *number);
                let mut runs = super::runs::plain_runs(&prefix);
                runs.extend(body_runs(text));
                target(section, &mut body, &mut attachments).push(Block::Par {
                    runs: super::runs::with_cjk_latin_gaps(runs),
                    line,
                });
            }
            MarkdownBlock::Table {
                rows,
                aligns,
                spans,
                numbered,
            } => {
                table_serial += 1;
                if let Some(table) = crate::export::table::to_typst_table(
                    rows,
                    aligns,
                    spans,
                    *numbered,
                    format!("t{table_serial}"),
                ) {
                    if section == MarkdownSection::Body && std::mem::take(&mut body_barrier) {
                        body.push(Block::Barrier);
                    }
                    target(section, &mut body, &mut attachments).push(Block::Table(table));
                }
            }
            MarkdownBlock::Image { alt: _, src } => {
                if section == MarkdownSection::Body && std::mem::take(&mut body_barrier) {
                    body.push(Block::Barrier);
                }
                target(section, &mut body, &mut attachments).push(Block::Image {
                    src: crate::images::normalize_ref(src),
                });
            }
            // 引用块已由 `flatten_quotes` 拆成段落；Mermaid 已在调用方物化成图片引用。
            MarkdownBlock::Html(_)
            | MarkdownBlock::Diagram { .. }
            | MarkdownBlock::Quote { .. } => {}
        }
        index += 1;
    }
    Sections { body, attachments }
}
