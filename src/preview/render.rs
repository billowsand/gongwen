//! 正文渲染：分页入口 official_preview、正文各块与图片。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::render`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::images;
use crate::theme;
use crate::export::{LocatedBlock, MarkdownBlock, MarkdownSection};
use crate::export;
use crate::models::{DraftInput, StyleMode, TemplateKind};
use crate::units::{UnitDisplay};
use std::ops::{Range};
use eframe::egui::{Align, Color32, Stroke};
use eframe::egui;
use crate::preview::{Metrics, PreviewScale, BODY_PT, TITLE_PT, INDENT_CHARS, LIST_INDENT_PT, heading_family, text_format, job, draw, place, indent, line_block, body_block, append_inline, is_renderable_paragraph, table_block, clickable, sheet, header_block, addressee_block, signature_block, footer_record, BodyRun, red_approval_print_preview};

/// 预览的一次绘制结果。
pub struct PreviewOutput {
    /// 本帧实际使用的缩放倍率，供“适应宽度”状态下的加减档以它为起点。
    pub scale: f32,
    /// 本帧被点击的正文块在 Markdown 源码中的字节范围。
    pub clicked: Option<Range<usize>>,
}

/// 逐块画正文。
#[allow(clippy::too_many_arguments)]
pub(crate) fn body_blocks(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    body: &[&LocatedBlock],
    run: &BodyRun,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    counters: &mut [usize; 4],
) {
    let mut index = 0usize;
    while index < body.len() {
        let located = body[index];
        index += 1;
        match &located.block {
            // 文档标题已在上面按版式排过，正文区不再重复。
            MarkdownBlock::Title(_) => {}
            MarkdownBlock::Heading(level, heading)
                if run.compact
                    && *level == run.compact_level
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
                // 合并成一段的标题与正文，回跳时一并选中。
                let range = located.range.start..next.range.end;
                clickable(ui, &range, anchor, scroll_to_anchor, clicked, |ui| {
                    compact_block(ui, metrics, *level, heading, text, counters, run.numbered);
                });
            }
            _ => {
                let range = located.range.clone();
                clickable(ui, &range, anchor, scroll_to_anchor, clicked, |ui| {
                    content_block(ui, metrics, &located.block, counters, run.numbered);
                });
            }
        }
    }
}

/// 把 Markdown 连同表单锁定的行文要素按公文版式画在 `ui` 里；调用方负责套滚动区。
/// 返回本次实际使用的缩放倍率，供“适应宽度”状态下的加减档以它为起点。
pub(crate) fn official_preview(
    ui: &mut egui::Ui,
    input: &DraftInput,
    display: &UnitDisplay,
    markdown: &str,
    scale: PreviewScale,
    anchor: Option<&Range<usize>>,
    mut scroll_to_anchor: bool,
) -> PreviewOutput {
    // 自适应缩放要按“看得见的宽度”算：滚动方向上的 available_width 是无穷大，
    // 拿它算会把整页放大到上限。裁剪矩形就是滚动区的可视范围，再与窗口取交集兜底。
    let visible = ui
        .clip_rect()
        .intersect(ui.ctx().input(|input| input.content_rect()));
    // 居中用的宽度优先取调用方给的真实宽度（见 `PreviewScale::viewport`）。
    let metrics = Metrics::new(scale.viewport.unwrap_or(visible.width()), scale.zoom);
    // 六个文种的正文都走 export::latex::official_letter_sections_to_tex，标题一律
    // 自动编号为 一、（一）1.（1）；紧缩风格跟随模板配置。
    let numbered = true;
    let compact = input.profile.style_mode == StyleMode::Compact;
    let located = export::parse_markdown_located(markdown);
    let blocks = located
        .iter()
        .map(|block| block.block.clone())
        .collect::<Vec<_>>();
    let names = export::attachment_names(&blocks);

    // 先按正文/附件切分。红头呈批件正文还会在专用分页器里继续拆成真实页；
    // 每份附件仍从新纸开始。
    let mut body: Vec<&LocatedBlock> = Vec::new();
    let mut attachments: Vec<Vec<&LocatedBlock>> = Vec::new();
    let mut in_attachment = false;
    let mut seen_title = false;
    for located in &located {
        match &located.block {
            MarkdownBlock::Title(_) if !seen_title && !in_attachment => {
                seen_title = true;
                body.push(located);
            }
            MarkdownBlock::Marker(section) => {
                in_attachment = matches!(section, MarkdownSection::Attachment);
                if in_attachment {
                    attachments.push(Vec::new());
                }
            }
            _ if in_attachment => match attachments.last_mut() {
                Some(last) => last.push(located),
                None => attachments.push(vec![located]),
            },
            _ => body.push(located),
        }
    }
    // 紧缩风格合并的是正文区 # 号最多的那一级标题；附件区不参与。
    let compact_level = export::body_heading_max_level(&blocks);
    // 标题取正文首个 `# `，缺省回落表单里的标题提示，与导出器一致。
    let title = body
        .iter()
        .find_map(|located| match &located.block {
            MarkdownBlock::Title(text) => Some((export::plain_text(text), located.range.clone())),
            _ => None,
        })
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
            &names,
            anchor,
            &mut scroll_to_anchor,
            &mut clicked,
        );
        return PreviewOutput {
            scale: metrics.scale,
            clicked,
        };
    }
    sheet(ui, &metrics, |ui| {
        header_block(ui, &metrics, input, display);
        if !title.is_empty() {
            clickable(
                ui,
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
        addressee_block(ui, &metrics, input, display);

        body_blocks(
            ui,
            &metrics,
            &body,
            &BodyRun {
                compact,
                compact_level,
                numbered,
            },
            anchor,
            &mut scroll_to_anchor,
            &mut clicked,
            &mut counters,
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
        signature_block(ui, &metrics, input, display);
        if record_on_body {
            footer_record(ui, &metrics, input, display);
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
                let range = located.range.clone();
                clickable(
                    ui,
                    &range,
                    anchor,
                    &mut scroll_to_anchor,
                    &mut clicked,
                    |ui| {
                        match &located.block {
                            // 附件正式标题与正文标题使用同一层级编码。
                            MarkdownBlock::Title(text) => {
                                counters.fill(0);
                                line_block(
                                    ui,
                                    &metrics,
                                    &export::plain_text(text),
                                    theme::FONT_BIAOSONG,
                                    TITLE_PT,
                                    Align::Center,
                                );
                                ui.add_space(metrics.pt(18.0));
                            }
                            block => content_block(ui, &metrics, block, &mut counters, numbered),
                        }
                    },
                );
            }
            if sheet_index == last_attachment {
                footer_record(ui, &metrics, input, display);
            }
        });
    }
    PreviewOutput {
        scale: metrics.scale,
        clicked,
    }
}

/// 紧缩风格的一段：标题（带编号与句号，用该级标题字体）后面直接接正文。
pub(crate) fn compact_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    level: u8,
    heading: &str,
    body: &str,
    counters: &mut [usize; 4],
    numbered: bool,
) {
    let heading = match numbered {
        true => match export::official_heading_text(level, heading, counters) {
            Some(text) => text,
            None => return,
        },
        false => heading.to_string(),
    };
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    let mut job = job(metrics.content);
    job.append(
        &indent(INDENT_CHARS),
        0.0,
        text_format(normal.clone(), metrics.line),
    );
    job.append(
        &format!("{}。", export::plain_text(&heading)),
        0.0,
        text_format(metrics.font(heading_family(level), BODY_PT), metrics.line),
    );
    append_inline(&mut job, metrics, body, &normal);
    draw(ui, job);
}

pub(crate) fn heading_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    level: u8,
    text: &str,
    counters: &mut [usize; 4],
    numbered: bool,
) {
    let text = if numbered {
        match export::official_heading_text(level, text, counters) {
            Some(text) => text,
            None => return,
        }
    } else {
        text.to_string()
    };
    let mut job = job(metrics.content);
    let font = metrics.font(heading_family(level), BODY_PT);
    job.append(
        &indent(INDENT_CHARS),
        0.0,
        text_format(font.clone(), metrics.line),
    );
    job.append(
        &export::plain_text(&text),
        0.0,
        text_format(font, metrics.line),
    );
    draw(ui, job);
}

pub(crate) fn content_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    block: &MarkdownBlock,
    counters: &mut [usize; 4],
    numbered: bool,
) {
    match block {
        MarkdownBlock::Heading(level, text) => {
            heading_block(ui, metrics, *level, text, counters, numbered);
        }
        MarkdownBlock::Paragraph(text) if is_renderable_paragraph(text) => {
            body_block(ui, metrics, text, true);
        }
        MarkdownBlock::ListItem(text) => {
            ui.horizontal(|ui| {
                ui.add_space(metrics.pt(LIST_INDENT_PT));
                ui.vertical(|ui| {
                    ui.set_width(metrics.content - metrics.pt(LIST_INDENT_PT));
                    let mut job = job(metrics.content - metrics.pt(LIST_INDENT_PT));
                    job.append(
                        text,
                        0.0,
                        text_format(metrics.font(theme::FONT_FANGSONG, BODY_PT), metrics.line),
                    );
                    draw(ui, job);
                });
            });
        }
        MarkdownBlock::Table { rows, aligns } => table_block(ui, metrics, rows, aligns),
        MarkdownBlock::Image { alt, src } => image_block(ui, metrics, alt, src),
        MarkdownBlock::Title(_) | MarkdownBlock::Marker(_) | MarkdownBlock::Html(_) => {}
        MarkdownBlock::Paragraph(_) => {}
    }
}

/// 图片块：位图按版心宽度等比渲染，加载失败显示占位卡片；PDF 显示占位卡片
/// （预览不渲染 PDF 内容，导出时由导出器嵌入）。外层已由 clickable 包装。
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
        return image_placeholder(
            ui,
            metrics,
            alt,
            &file_name,
            "PDF 附件：预览暂不渲染，导出时嵌入",
        );
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

/// 图片占位卡片：细边框 + 文件名与说明，宽度=版心。
pub(crate) fn image_placeholder(ui: &mut egui::Ui, metrics: &Metrics, alt: &str, file_name: &str, note: &str) {
    let height = (metrics.line * 2.0).max(metrics.pt(24.0));
    place(ui, metrics, height, |painter, rect| {
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(2),
            Stroke::new(1.0_f32.max(metrics.scale), Color32::from_gray(150)),
            egui::StrokeKind::Inside,
        );
        let font = metrics.font(theme::FONT_FANGSONG, BODY_PT);
        let caption = if alt.is_empty() {
            file_name.to_string()
        } else {
            format!("{file_name}（{alt}）")
        };
        let text = format!("【图片】{caption}\n{note}");
        let galley = painter.layout(
            text,
            font,
            Color32::from_gray(90),
            rect.width() - metrics.pt(8.0),
        );
        let y = rect.top() + ((rect.height() - galley.size().y) / 2.0).max(metrics.pt(4.0));
        painter.galley(
            egui::pos2(rect.left() + metrics.pt(4.0), y),
            galley,
            Color32::WHITE,
        );
    });
}
