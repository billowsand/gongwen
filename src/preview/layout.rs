//! 排版基础：文本布局、段落/表格块、占位与版心度量辅助。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::layout`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::table::ColumnAlignment;
use crate::preview::{BODY_PT, INDENT_CHARS, Metrics, PAREN_PT, TABLE_LINE_PT, TABLE_PT};
use crate::theme;
use eframe::egui;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Align, Color32, FontId, Stroke};
use std::ops::Range;
use std::sync::Arc;

/// 一段预览文字中，与一行 Markdown 源码对应的可见字符范围。
pub(crate) struct ClickableSourceSegment {
    pub(crate) source: Range<usize>,
    pub(crate) chars: Range<usize>,
}

/// 正文各级标题的字体：与 `export::docx::heading_paragraph` 保持一致。
pub(crate) fn heading_family(level: u8) -> &'static str {
    match level {
        2 => theme::FONT_HEITI,
        3 => theme::FONT_KAITI,
        _ => theme::FONT_FANGSONG,
    }
}

pub(crate) fn text_format(font: FontId, line: f32) -> TextFormat {
    TextFormat {
        font_id: font,
        color: theme::paper::ink(),
        line_height: Some(line),
        ..Default::default()
    }
}

pub(crate) fn job(width: f32) -> LayoutJob {
    LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: width,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 单行不换行的排版任务，用于量红头这类需要先测宽度的文字。
pub(crate) fn single_line(text: &str, format: TextFormat) -> LayoutJob {
    let mut job = job(f32::INFINITY);
    job.append(text, 0.0, format);
    job
}

/// 排版一段文字：先按中文避头尾（禁则）算好断点，再交给 egui 逐行排。
///
/// epaint 自带的断行只认它内置的那串「不能出现在行首」的字符
/// （`is_cjk_break_allowed`）：全角标点（`，` `；` `：` `？` `！` `）` 等）一个都
/// 不在列，而且那串字符只在前一个字是 CJK 时才被检查——数字、西文后面的标点
/// 根本不判，于是断行会把标点甩到下一行行首。中文公文的禁则比那串字符宽得多，
/// 这里先按自然宽度量一遍，用完整的避头尾规则重算断点，插成硬换行后再交给 epaint。
///
/// 换行符不产生字形，因此返回的 galley 逐行累积的 `glyphs.len()` 仍等于原文本的
/// 字符下标——`justified_rows` 与各处点击命中都依赖这个不变量切片。
pub(crate) fn layout(ui: &egui::Ui, job: LayoutJob) -> Arc<egui::Galley> {
    let job = kinsoku_wrap(ui, job);
    ui.ctx().fonts_mut(|fonts| fonts.layout_job(job))
}

/// 按避头尾规则把排版任务拆成硬换行；不需要重排时原样返回。
fn kinsoku_wrap(ui: &egui::Ui, job: LayoutJob) -> LayoutJob {
    let width = job.wrap.max_width;
    if !width.is_finite() || job.text.is_empty() || job.sections.is_empty() {
        return job;
    }
    // 不限宽排一遍：每个自然段摊成一行，字形位置就是自然宽度。
    let mut flat = job.clone();
    flat.wrap.max_width = f32::INFINITY;
    let measured = ui.ctx().fonts_mut(|fonts| fonts.layout_job(flat));

    let mut breaks = Vec::new();
    let mut rows = measured.rows.iter();
    let mut char_start = 0usize;
    // 不限宽时 galley 的行与 `\n` 切出的自然段一一对应。
    for paragraph in job.text.split('\n') {
        let Some(row) = rows.next() else { break };
        let count = paragraph.chars().count();
        if count > 0 {
            for line in kinsoku_lines(&row.glyphs, width) {
                if line.end < count {
                    breaks.push(char_start + line.end);
                }
            }
        }
        char_start += count + 1;
    }
    if breaks.is_empty() {
        return job;
    }
    hard_wrapped_job(&job, &breaks)
}

/// 取出字形流做断行：`(字符, 行内左缘, 前进宽度)`。
fn kinsoku_lines(glyphs: &[egui::epaint::text::Glyph], width: f32) -> Vec<Range<usize>> {
    let shaped = glyphs
        .iter()
        .map(|glyph| (glyph.chr, glyph.pos.x, glyph.advance_width))
        .collect::<Vec<_>>();
    break_lines(&shaped, width)
}

/// 避头尾断行的纯逻辑：给定逐字（字符 + 左缘 + 前进宽度）与行宽，返回每行的字符
/// 区间。贪心填满一行后，从行尾往前找最近的合法断点；避头点会被连同前一个字一起
/// 挤到下一行（追出），避尾点则自己挪到下一行。
fn break_lines(shaped: &[(char, f32, f32)], width: f32) -> Vec<Range<usize>> {
    let count = shaped.len();
    let mut lines = Vec::new();
    let mut start = 0usize;
    while start < count {
        let line_start = shaped[start].1;
        // `end` 是最后一个还能排进这一行的字的**下一个**下标。
        let mut end = start;
        while end < count && shaped[end].1 + shaped[end].2 - line_start <= width {
            end += 1;
        }
        if end == start {
            // 一个字的宽度就超过整行（畸形输入）：独占一行，保证循环前进。
            lines.push(start..start + 1);
            start += 1;
            continue;
        }
        if end >= count {
            lines.push(start..count);
            break;
        }
        // 从行尾往前找最近的合法断点。
        let mut break_at = None;
        let mut candidate = end;
        while candidate > start {
            if can_break_between(shaped[candidate - 1].0, shaped[candidate].0) {
                break_at = Some(candidate);
                break;
            }
            candidate -= 1;
        }
        let break_at = match break_at {
            Some(at) => at,
            None => {
                // 整段都找不到断点（长西文串、长数字）：向后让这一行溢出到下一个
                // 断点，宁可放宽一点也不把单词、数字拦腰切开。
                let mut at = end + 1;
                while at < count && !can_break_between(shaped[at - 1].0, shaped[at].0) {
                    at += 1;
                }
                at.min(count)
            }
        };
        lines.push(start..break_at);
        start = break_at;
    }
    lines
}

/// 行首禁则：这些字符不允许出现在一行的开头（后括号、后引号与句末点号）。
pub(super) fn is_no_line_start(ch: char) -> bool {
    const NO_START: &str =
        r#"、。，．：；？！,.:;?!%)]}）］｝〕〉》」』】〗〙〟｠»”’"'·ー々〻～〜–—―‐…‥%‰℃°"#;
    NO_START.contains(ch)
}

/// 行尾禁则：这些字符不允许出现在一行的末尾（前括号与前引号）。
pub(super) fn is_no_line_end(ch: char) -> bool {
    const NO_END: &str = "([{（［｛〔〈《「『【〖〘｟«“‘";
    NO_END.contains(ch)
}

/// 两个相邻的字之间能不能断行：空白处随便断；避头尾拦住；西文单词与数字串内部不断。
fn can_break_between(before: char, after: char) -> bool {
    if before.is_whitespace() || after.is_whitespace() {
        return true;
    }
    if is_no_line_end(before) || is_no_line_start(after) {
        return false;
    }
    !(before.is_ascii_alphanumeric() && after.is_ascii_alphanumeric())
}

/// 把断点插成硬换行，得到逐行排版的新任务。
///
/// 换行符归到它**后面那个字所属的 section**：epaint 在 section 文本里按 `\n`
/// 分段，段内后续字符沿用该 section 的字型；把 `\n` 塞进前一段，新一行的字型
/// 就会错成上一行的。
fn hard_wrapped_job(job: &LayoutJob, breaks: &[usize]) -> LayoutJob {
    // (字符所属 section 下标, 是否是原 section 的首字符)
    let mut owners: Vec<(usize, bool)> =
        Vec::with_capacity(job.text.chars().count() + breaks.len());
    let mut text = String::with_capacity(job.text.len() + breaks.len());
    let mut section_index = 0usize;
    let mut next_break = 0usize;
    for (char_index, (byte, ch)) in job.text.char_indices().enumerate() {
        while section_index + 1 < job.sections.len()
            && job.sections[section_index].byte_range.end.0 <= byte
        {
            section_index += 1;
        }
        if breaks.get(next_break) == Some(&char_index) {
            next_break += 1;
            owners.push((section_index, false));
            text.push('\n');
        }
        let first_of_section = job.sections[section_index].byte_range.start.0 == byte;
        owners.push((section_index, first_of_section));
        text.push(ch);
    }
    if owners.is_empty() {
        return job.clone();
    }

    let mut sections = Vec::with_capacity(job.sections.len() + breaks.len());
    let mut piece_owner = owners[0];
    let mut piece_start = 0usize;
    for (index, (byte, _)) in text.char_indices().enumerate() {
        if owners[index].0 != piece_owner.0 {
            sections.push(wrapped_section(job, piece_owner, piece_start..byte));
            piece_owner = owners[index];
            piece_start = byte;
        }
    }
    sections.push(wrapped_section(job, piece_owner, piece_start..text.len()));

    let mut wrapped = job.clone();
    wrapped.text = text;
    wrapped.sections = sections;
    // 断点已经全部落成硬换行，宽度限制只会把某行再切碎，必须放开。
    wrapped.wrap.max_width = f32::INFINITY;
    wrapped
}

/// 硬换行后的 section 片段：字型照旧，前导空隙只跟着原 section 的首字符走。
fn wrapped_section(
    job: &LayoutJob,
    owner: (usize, bool),
    range: Range<usize>,
) -> egui::text::LayoutSection {
    let source = &job.sections[owner.0];
    egui::text::LayoutSection {
        leading_space: if owner.1 { source.leading_space } else { 0.0 },
        byte_range: egui::text::ByteIndex(range.start)..egui::text::ByteIndex(range.end),
        format: source.format.clone(),
    }
}

pub(crate) fn draw(ui: &mut egui::Ui, job: LayoutJob) {
    let galley = layout(ui, job);
    ui.add(egui::Label::new(galley));
}

/// 两端对齐地画一段正文，与 Word 的 `w:jc=both`、TeX 的默认对齐一致：
/// 除末行外每行都撑满版心，末行保持自然宽度。
///
/// 不能直接用 egui 的 `LayoutJob::justify`：epaint 的 `halign_and_justify_row`
/// 会先数掉行首空白（`num_leading_spaces`）再把余下的字撑满整行宽，正文首行缩进
/// 的那两个全角空格既会被挤出版心，又会让首行多撑开两个字。这里换成自己逐行补
/// 字距：先按不对齐排一遍拿到断行位置，再逐行用 `extra_letter_spacing` 补足。
pub(crate) fn draw_justified(ui: &mut egui::Ui, job: LayoutJob) {
    let width = job.wrap.max_width;
    let base = layout(ui, job.clone());
    if !width.is_finite() || base.rows.len() < 2 {
        // 单行段落本来就是末行，不参与对齐。
        ui.add(egui::Label::new(base));
        return;
    }
    let rows = justified_rows(ui, &job, &base);
    let height = base.size().y;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    for (placed, galley) in base.rows.iter().zip(rows) {
        painter.galley(
            rect.left_top() + placed.pos.to_vec2(),
            galley,
            theme::paper::ink(),
        );
    }
}

/// 按 `base` 已经排好的断行位置，把整段拆成逐行的两端对齐 galley。
///
/// 返回的行数与 `base.rows` 一一对应，第 i 行画在 `base.rows[i].pos` 处即可。
/// 末行不撑开：公文和 Word 一样，段落最后一行保持自然宽度。
pub(crate) fn justified_rows(
    ui: &egui::Ui,
    job: &LayoutJob,
    base: &egui::Galley,
) -> Vec<Arc<egui::Galley>> {
    let width = job.wrap.max_width;
    let char_offsets = job
        .text
        .char_indices()
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    let row_count = base.rows.len();
    let mut galleys = Vec::with_capacity(row_count);
    let mut char_start = 0usize;
    for (index, row) in base.rows.iter().enumerate() {
        let char_end = char_start + row.glyphs.len();
        let byte_start = char_offsets
            .get(char_start)
            .copied()
            .unwrap_or(job.text.len());
        let byte_end = char_offsets
            .get(char_end)
            .copied()
            .unwrap_or(job.text.len())
            // 兜底：epaint 承诺一个字符一个字形，万一出现多字形簇也不能让切片反转。
            .max(byte_start);
        let extra = if index + 1 == row_count || !width.is_finite() {
            0.0
        } else {
            row_extra_spacing(row, width)
        };
        galleys.push(layout(ui, row_job(job, byte_start..byte_end, extra)));
        char_start = char_end;
    }
    galleys
}

/// 这一行要补多少字距才能撑满 `width`。行末空白不算进已用宽度，
/// 行首缩进算——缩进本来就占版心，只有它后面的字需要摊掉余量。
fn row_extra_spacing(row: &egui::epaint::text::PlacedRow, width: f32) -> f32 {
    let glyphs = &row.glyphs;
    let end = glyphs
        .iter()
        .rposition(|glyph| !glyph.chr.is_whitespace())
        .map_or(0, |index| index + 1);
    if end < 2 {
        return 0.0;
    }
    let natural = glyphs[end - 1].pos.x + glyphs[end - 1].advance_width - glyphs[0].pos.x;
    let slack = width - natural;
    // 补不动（整行已经排满甚至溢出）时就不动，避免负字距把字挤在一起。
    if slack <= 0.0 {
        return 0.0;
    }
    slack / (end as f32 - 1.0)
}

/// 从整段的排版任务里切出一行：按字节范围裁剪各 section，并统一设置字距。
fn row_job(job: &LayoutJob, range: Range<usize>, extra: f32) -> LayoutJob {
    let mut out = LayoutJob {
        text: job.text[range.clone()].to_string(),
        wrap: egui::text::TextWrapping {
            max_width: f32::INFINITY,
            ..Default::default()
        },
        ..Default::default()
    };
    for section in &job.sections {
        let start = section.byte_range.start.0.max(range.start);
        let end = section.byte_range.end.0.min(range.end);
        if start >= end {
            continue;
        }
        let mut format = section.format.clone();
        format.extra_letter_spacing = extra;
        out.sections.push(egui::text::LayoutSection {
            // 被上一行截断的 section 不再重复它的前导空白。
            leading_space: if section.byte_range.start.0 >= range.start {
                section.leading_space
            } else {
                0.0
            },
            byte_range: egui::text::ByteIndex(start - range.start)
                ..egui::text::ByteIndex(end - range.start),
            format,
        });
    }
    out
}

/// 占一块高 `height` 的版心宽区域，把绘制交给回调。抬头、落款、版记这些需要
/// 自己算横向位置的块都走这里。
pub(crate) fn place(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    height: f32,
    paint: impl FnOnce(&egui::Painter, egui::Rect),
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
    paint(ui.painter(), rect);
}

/// 把若干行叠放在版心内的一个块里。`left` 是块左沿相对版心左沿的偏移，
/// `align` 同时决定每行的对齐方式和锚点位置。
pub(crate) fn stacked(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    galleys: &[Arc<egui::Galley>],
    left: f32,
    width: f32,
    align: Align,
) {
    let height = galleys.iter().map(|galley| galley.size().y).sum::<f32>();
    place(ui, metrics, height, |painter, rect| {
        let anchor = match align {
            Align::Center => rect.left() + left + width / 2.0,
            Align::Max => rect.left() + left + width,
            _ => rect.left() + left,
        };
        let mut y = rect.top();
        for galley in galleys {
            // 右对齐按字形实际右缘（mesh_bounds.max.x）摆位，精确落在锚点上；
            // 用 size() 会算入行前 leading，产生 1~4px 错位。空行等无字形时兜底。
            let pos = match align {
                Align::Max => {
                    let right = if galley.mesh_bounds.max.x.is_finite() {
                        galley.mesh_bounds.max.x
                    } else {
                        galley.size().x
                    };
                    egui::pos2(anchor - right, y)
                }
                _ => egui::pos2(anchor, y),
            };
            painter.galley(pos, galley.clone(), theme::paper::ink());
            y += galley.size().y;
        }
    });
}

/// 排一行文字并返回 galley：`width` 为换行宽度，`align` 为行内对齐。
pub(crate) fn line_galley(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    font: FontId,
    width: f32,
    align: Align,
) -> Arc<egui::Galley> {
    let mut job = job(width);
    job.halign = align;
    job.append(text, 0.0, text_format(font, metrics.line));
    layout(ui, job)
}

/// 首行缩进用两个全角空格实现：仿宋的全角空格正好一个字宽，
/// 两个即 2 字，与 Word 的 640 缇一致。
pub(crate) fn indent(count: f32) -> String {
    "\u{3000}".repeat(count as usize)
}

/// 一行文字的整段渲染（居中标题、缩进标题等都走这里）。
pub(crate) fn line_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    align: Align,
) {
    let mut job = job(metrics.content);
    job.halign = align;
    job.append(
        text,
        0.0,
        text_format(metrics.font(family, size), metrics.line),
    );
    if align == Align::Center {
        // halign 只让 galley 内部每行相对自身宽度居中，经 Label 摆到光标处后，
        // 单行标题会偏左、多行才看似居中。这里把 galley 直接画在版心正中，
        // 让每一行都相对版心宽居中，与 Word 的居中段落一致。
        let galley = layout(ui, job);
        let height = galley.size().y;
        place(ui, metrics, height, |painter, rect| {
            painter.galley(
                egui::pos2(rect.left() + metrics.content / 2.0, rect.top()),
                galley,
                theme::paper::ink(),
            );
        });
    } else {
        draw(ui, job);
    }
}

/// 正文段落：仿宋三号、首行缩进 2 字，两端对齐；行内保留加粗与括号楷体。
pub(crate) fn body_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    first_line_indent: bool,
) {
    let mut job = job(metrics.content);
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    if first_line_indent {
        job.append(
            &indent(INDENT_CHARS),
            0.0,
            text_format(normal.clone(), metrics.line),
        );
    }
    append_inline(&mut job, metrics, text, &normal);
    draw_justified(ui, job);
}

/// 保持一个自然段的连续排版，同时把点击与高亮区域拆到每一行 Markdown 源码。
#[allow(clippy::too_many_arguments)]
pub(crate) fn clickable_body_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    first_line_indent: bool,
    segments: &[ClickableSourceSegment],
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let mut job = job(metrics.content);
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    let indent_chars = if first_line_indent {
        job.append(
            &indent(INDENT_CHARS),
            0.0,
            text_format(normal.clone(), metrics.line),
        );
        INDENT_CHARS as usize
    } else {
        0
    };
    append_inline(&mut job, metrics, text, &normal);
    let adjusted = segments
        .iter()
        .map(|segment| ClickableSourceSegment {
            source: segment.source.clone(),
            chars: if segment.chars.start == 0 {
                0..segment.chars.end + indent_chars
            } else {
                segment.chars.start + indent_chars..segment.chars.end + indent_chars
            },
        })
        .collect::<Vec<_>>();
    clickable_justified_job(
        ui,
        metrics,
        job,
        &adjusted,
        anchor,
        scroll_to_anchor,
        clicked,
    );
}

/// 为已经构造好的连续段落布局添加源码行级交互；紧缩段可借此保留标题/正文字体。
pub(crate) fn clickable_justified_job(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    job: LayoutJob,
    segments: &[ClickableSourceSegment],
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
) {
    let base = layout(ui, job.clone());
    let rows = justified_rows(ui, &job, &base);
    let height = base.size().y;
    let (block_rect, _) =
        ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());

    let mut row_start = 0usize;
    for (row_index, (placed, row_galley)) in base.rows.iter().zip(&rows).enumerate() {
        let row_end = row_start + placed.glyphs.len();
        for segment in segments {
            let start = segment.chars.start.max(row_start);
            let end = segment.chars.end.min(row_end);
            if start >= end {
                continue;
            }
            let local_start = start - row_start;
            let local_end = end - row_start;
            let left = row_galley
                .pos_from_cursor(egui::text::CCursor::new(local_start))
                .left();
            let right = row_galley
                .pos_from_cursor(egui::text::CCursor::new(local_end))
                .left()
                .max(left + 1.0);
            let rect = egui::Rect::from_min_max(
                block_rect.left_top() + placed.pos.to_vec2() + egui::vec2(left, 0.0),
                block_rect.left_top() + placed.pos.to_vec2() + egui::vec2(right, placed.size.y),
            )
            .expand2(egui::vec2(3.0, 1.0));
            let response = ui.interact(
                rect,
                egui::Id::new((
                    "gw-preview-source-line",
                    segment.source.start,
                    segment.source.end,
                    row_index,
                )),
                egui::Sense::click(),
            );
            if response.clicked() {
                *clicked = Some(segment.source.clone());
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let anchored = anchor.is_some_and(|anchor| {
                !anchor.is_empty()
                    && anchor.start < segment.source.end
                    && segment.source.start < anchor.end
            });
            if anchored && *scroll_to_anchor {
                scroll_preview_to_rect(ui, rect);
                *scroll_to_anchor = false;
            }
            if anchored || response.hovered() {
                ui.painter().rect_filled(
                    rect,
                    3.0,
                    if anchored {
                        theme::accent_soft()
                    } else {
                        theme::paper::hover_tint()
                    },
                );
            }
        }
        row_start = row_end;
    }

    for (placed, row) in base.rows.iter().zip(rows) {
        ui.painter().galley(
            block_rect.left_top() + placed.pos.to_vec2(),
            row,
            theme::paper::ink(),
        );
    }
}

/// 行内片段按导出规则上色：括号内容楷体四号，加粗走 `theme::FONT_BOLD`。
///
/// egui 不做合成加粗，所以「当前字体直接加粗」在预览里仍用黑体近似；设置里改选
/// 专用粗体字体后，`FONT_BOLD` 换成选定的字面，与 Word / TeX 同步。
pub(crate) fn append_inline(job: &mut LayoutJob, metrics: &Metrics, text: &str, normal: &FontId) {
    for segment in export::inline_segments(text) {
        let font = if segment.parenthesized {
            metrics.font(theme::FONT_KAITI, PAREN_PT)
        } else if segment.bold {
            metrics.font(theme::FONT_BOLD, BODY_PT)
        } else {
            normal.clone()
        };
        job.append(&segment.text, 0.0, text_format(font, metrics.line));
    }
}

/// 与导出一致：空段落和 HTML 包裹行不成段。
pub(crate) fn is_renderable_paragraph(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains("<div") && !text.contains("</div")
}

/// 表格：四号字、行距 21 磅，表头黑体居中，列宽直接取导出器算好的智能列宽，
/// 因此预览的列宽与导出的 Word 表格一致。
pub(crate) fn table_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    rows: &[Vec<String>],
    aligns: &[export::ColumnAlign],
) {
    let columns = export::table_columns(rows, aligns);
    if columns.is_empty() {
        return;
    }
    let widths = columns
        .iter()
        .map(|column| metrics.content * column.fraction)
        .collect::<Vec<_>>();

    // 单元格内边距不能超过列宽的一小部分，否则窄列会算出负的换行宽度。
    let line = metrics.pt(TABLE_LINE_PT);
    let stroke = Stroke::new(1.0_f32.max(metrics.scale), theme::paper::ink());
    for (index, row) in rows.iter().enumerate() {
        let header = index == 0;
        let font = metrics.font(
            if header {
                theme::FONT_HEITI
            } else {
                theme::FONT_FANGSONG
            },
            TABLE_PT,
        );
        let cells = widths
            .iter()
            .enumerate()
            .map(|(column, width)| {
                let padding = metrics.pt(3.0).min(width * 0.12);
                let text = export::plain_text(row.get(column).map_or("", String::as_str));
                let mut job = job((width - 2.0 * padding).max(1.0));
                // 表头一律居中；正文列按导出器判定的对齐方式。
                let align = if header {
                    ColumnAlignment::Center
                } else {
                    columns[column].alignment
                };
                job.halign = match align {
                    ColumnAlignment::Center => Align::Center,
                    ColumnAlignment::Right => Align::RIGHT,
                    ColumnAlignment::Left => Align::LEFT,
                };
                job.append(&text, 0.0, text_format(font.clone(), line));
                let galley = layout(ui, job);
                (galley, padding, align)
            })
            .collect::<Vec<_>>();
        let height = cells
            .iter()
            .map(|(galley, padding, _)| galley.size().y + 2.0 * padding)
            .fold(line, f32::max);

        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        let mut x = rect.left();
        for (column, (galley, padding, align)) in cells.iter().enumerate() {
            if column > 0 {
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    stroke,
                );
            }
            // halign 决定每行相对锚点的位置：居中时锚点取单元格中线，靠左时取内边距。
            let anchor = match align {
                ColumnAlignment::Center => x + widths[column] / 2.0,
                ColumnAlignment::Right => x + widths[column] - padding,
                ColumnAlignment::Left => x + padding,
            };
            let top = rect.top() + (height - galley.size().y) / 2.0;
            painter.galley(egui::pos2(anchor, top), galley.clone(), theme::paper::ink());
            x += widths[column];
        }
    }
}

/// 画一个来自 Markdown 的块，并让它可以点：悬停时淡底提示，点击后把它在源码中的
/// 范围报给调用方；`anchor` 命中的块常亮，与编辑器里的高亮一一对应。
pub(crate) fn clickable(
    ui: &mut egui::Ui,
    range: &Range<usize>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    // 底色要压在文字下面：先占一个空图形位，量出块的范围后再回填。
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let inner = ui.scope(add_contents).response.rect;
    if !inner.is_positive() {
        return;
    }
    let rect = inner.expand2(egui::vec2(4.0, 1.0));
    let response = ui.interact(
        rect,
        egui::Id::new(("gw-preview-block", range.start, range.end)),
        egui::Sense::click(),
    );
    if response.clicked() {
        *clicked = Some(range.clone());
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // 查找命中通常只是块内的一小段文字；与块范围相交也要标亮，这样在“公文预览”
    // 模式中仍能看出当前命中位于哪一段。
    let anchored = anchor.is_some_and(|anchor| {
        !anchor.is_empty()
            && !range.is_empty()
            && anchor.start < range.end
            && range.start < anchor.end
    });
    if anchored && *scroll_to_anchor {
        scroll_preview_to_rect(ui, rect);
        *scroll_to_anchor = false;
    }
    let fill = if anchored {
        theme::accent_soft()
    } else if response.hovered() {
        theme::paper::hover_tint()
    } else {
        return;
    };
    ui.painter().set(
        backdrop,
        egui::epaint::RectShape::filled(rect, egui::CornerRadius::same(3), fill),
    );
}

/// 把目标放在可视区中部略偏上（约 40% 高度），给下方正文留下更多阅读空间。
pub(crate) fn scroll_preview_to_rect(ui: &mut egui::Ui, rect: egui::Rect) {
    let offset = ui.clip_rect().height() * 0.1;
    ui.scroll_to_rect(
        rect.translate(egui::vec2(0.0, offset)),
        Some(egui::Align::Center),
    );
}

/// 一张“纸”：白底、细边、内含公文版心。
pub(crate) fn sheet(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        // 用量好的可见宽度而不是 available_width：滚动区里后者可能是无穷大。
        let side = ((metrics.viewport - metrics.page) / 2.0).max(0.0);
        ui.add_space(side);
        ui.vertical(|ui| {
            ui.set_max_width(metrics.page);
            egui::Frame::new()
                .fill(theme::paper::bg())
                .stroke(Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(3))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 2],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(theme::paper::shadow_alpha()),
                })
                .show(ui, |ui| {
                    // 页边距用 add_space 铺出来：`Margin` 是 i8，放大后会溢出。
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    ui.set_width(metrics.page);
                    ui.add_space(metrics.margin_top);
                    ui.horizontal_top(|ui| {
                        ui.add_space(metrics.margin_left);
                        ui.vertical(|ui| {
                            ui.set_width(metrics.content);
                            ui.set_min_height((metrics.page - metrics.margin_top * 2.0).max(0.0));
                            ui.style_mut().visuals.override_text_color = Some(theme::paper::ink());
                            // 预览是拿来看版式和点回源码的，不做文字选择，
                            // 否则 Label 会把点击当成拖选吃掉。
                            ui.style_mut().interaction.selectable_labels = false;
                            add_contents(ui);
                        });
                    });
                    ui.add_space(metrics.margin_top);
                });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 `(字符, 宽度)` 摊成断行器要的 `(字符, 左缘, 宽度)`。
    fn shaped(chars: &[(char, f32)]) -> Vec<(char, f32, f32)> {
        let mut x = 0.0f32;
        chars
            .iter()
            .map(|&(ch, width)| {
                let glyph = (ch, x, width);
                x += width;
                glyph
            })
            .collect()
    }

    fn hanzi(text: &str) -> Vec<(char, f32)> {
        text.chars().map(|ch| (ch, 1.0)).collect()
    }

    #[test]
    fn fullwidth_punctuation_is_pushed_off_the_line_start() {
        // 行宽 4 字。`，` 原本会落到第二行行首，必须连同前一个字一起挤过去。
        let glyphs = shaped(&hanzi("甲乙丙丁，戊己庚辛"));
        assert_eq!(break_lines(&glyphs, 4.0), vec![0..3, 3..7, 7..9]);
    }

    #[test]
    fn punctuation_after_digits_pushes_the_number_down() {
        // `，` 前面是数字：数字串不能切开，整串连标点一起下移。
        let mut chars = hanzi("甲乙丙");
        chars.push(('1', 0.5));
        chars.push(('2', 0.5));
        chars.push(('，', 1.0));
        chars.extend(hanzi("丁戊己庚"));
        let glyphs = shaped(&chars);
        assert_eq!(break_lines(&glyphs, 4.0), vec![0..3, 3..8, 8..10]);
    }

    #[test]
    fn opening_bracket_never_ends_a_line() {
        let glyphs = shaped(&hanzi("甲乙丙（丁戊己"));
        assert_eq!(break_lines(&glyphs, 4.0), vec![0..3, 3..7]);
    }

    #[test]
    fn latin_words_are_not_split() {
        let chars = [
            ('a', 0.5),
            ('b', 0.5),
            ('c', 0.5),
            (' ', 0.5),
            ('d', 0.5),
            ('e', 0.5),
            ('f', 0.5),
        ];
        let glyphs = shaped(&chars);
        assert_eq!(break_lines(&glyphs, 1.6).first(), Some(&(0..3)));
    }

    #[test]
    fn forbidden_characters_are_classified() {
        for ch in [
            '，', '。', '、', '；', '：', '？', '！', '）', '】', '》', '」', '”', '’',
        ] {
            assert!(is_no_line_start(ch), "“{ch}”不能起行");
        }
        for ch in ['（', '【', '《', '「', '“', '‘'] {
            assert!(is_no_line_end(ch), "“{ch}”不能收行");
        }
        // 开放类标点可以起行，收尾类标点也可以收行。
        assert!(!is_no_line_start('（'));
        assert!(!is_no_line_end('，'));
    }

    #[test]
    fn every_row_of_a_paragraph_obeys_kinsoku() {
        let text = "为进一步推进服务事项标准化、规范化、便利化，请各单位于2026年9月20日前报送材料（含附件1、附件2），逾期不再受理；材料编号ABC123，务必核对。";
        let chars = text
            .chars()
            .map(|ch| (ch, if ch.is_ascii_alphanumeric() { 0.5 } else { 1.0 }))
            .collect::<Vec<_>>();
        for width in [2.0, 3.0, 4.5, 7.0, 10.0, 13.5] {
            let glyphs = shaped(&chars);
            let lines = break_lines(&glyphs, width);
            assert_eq!(
                lines.first().map(|line| line.start),
                Some(0),
                "行宽 {width}"
            );
            assert_eq!(lines.last().map(|line| line.end), Some(glyphs.len()));
            for pair in lines.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "行区间必须首尾相接");
            }
            for line in &lines {
                assert!(
                    !is_no_line_start(glyphs[line.start].0),
                    "行宽 {width} 时“{}”被排到了行首：{lines:?}",
                    glyphs[line.start].0
                );
                assert!(
                    !is_no_line_end(glyphs[line.end - 1].0),
                    "行宽 {width} 时“{}”被排到了行尾：{lines:?}",
                    glyphs[line.end - 1].0
                );
            }
        }
    }
}
