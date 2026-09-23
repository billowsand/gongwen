//! 排版基础：文本布局、段落/表格块、占位与版心度量辅助。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::layout`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::table::ColumnAlignment;
use crate::preview::gutter;
use crate::preview::{INDENT_CHARS, Metrics, PAREN_PT, TABLE_LINE_PT, TABLE_PT};
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

/// 一行文字里的一段：字面和字号可以与相邻段不同。
pub(crate) struct TextRun<'a> {
    pub(crate) text: &'a str,
    pub(crate) family: &'a str,
    pub(crate) size: f32,
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
    line_block_runs(ui, metrics, &[TextRun { text, family, size }], align);
}

/// 同 [`line_block`]，但一行里可以换字面：研究报告的表题就是黑体的"表 1.1"
/// 接宋体的题名，两段共用一行、一起居中。
pub(crate) fn line_block_runs(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    runs: &[TextRun<'_>],
    align: Align,
) {
    let mut job = job(metrics.content);
    job.halign = align;
    for run in runs {
        job.append(
            run.text,
            0.0,
            text_format(metrics.font(run.family, run.size), metrics.line),
        );
    }
    if align == Align::Center {
        // halign 只让 galley 内部每行相对自身宽度居中，经 Label 摆到光标处后，
        // 单行标题会偏左、多行才看似居中。这里把 galley 直接画在版心正中，
        // 让每一行都相对版心宽居中，与 Word 的居中段落一致。
        let galley = layout(ui, job);
        let height = galley.size().y;
        let rows = row_spans(&galley);
        place(ui, metrics, height, |painter, rect| {
            let origin = egui::pos2(rect.left() + metrics.content / 2.0, rect.top());
            push_galley_tints(metrics, &galley, origin);
            painter.galley(origin, galley, theme::paper::ink());
            mark_gutter_rows(metrics, rect, &rows);
        });
    } else {
        let galley = layout(ui, job);
        let rows = row_spans(&galley);
        let tints = galley.clone();
        let rect = ui.add(egui::Label::new(galley)).rect;
        push_galley_tints(metrics, &tints, rect.left_top());
        mark_gutter_rows(metrics, rect, &rows);
    }
}

/// 把一段 galley 每一行的底色矩形交给 [`Metrics::push_tint`]：横向从第一个有墨的字
/// 起到行尾最后一个字止（行首缩进的空白不压底色），纵向按 [`row_tint_offset`]
/// 让字坐在色块正中——与公文正文 `clickable_justified_job` 同一口径。
pub(crate) fn push_galley_tints(metrics: &Metrics, galley: &egui::Galley, origin: egui::Pos2) {
    for row in &galley.rows {
        let Some(ink) = first_ink(&row.glyphs, 0..row.glyphs.len()) else {
            continue;
        };
        let Some(last) = row.glyphs.last() else {
            continue;
        };
        let left = origin.x + row.pos.x + row.glyphs[ink].pos.x;
        let right = origin.x + row.pos.x + last.max_x();
        let top = origin.y + row.pos.y + row_tint_offset(row);
        metrics.push_tint(tint_rect(left, right, top, row.size.y));
    }
}

/// 一行底色的矩形：左右各外扩 3、上下各 1，与公文正文的行底色一致。
pub(crate) fn tint_rect(left: f32, right: f32, top: f32, height: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(left, top),
        egui::pos2(right.max(left + 1.0), top + height),
    )
    .expand2(egui::vec2(3.0, 1.0))
}

/// 一段文字里每一行相对段首的上沿、行高与基线。
fn row_spans(galley: &egui::Galley) -> Vec<(f32, f32, f32)> {
    galley
        .rows
        .iter()
        .map(|row| (row.pos.y, row.size.y, gutter::row_baseline(row, row.pos.y)))
        .collect()
}

/// 把一段已经排好的文字逐行记进行号刻度。`rect` 是这一段占住的版心区域，
/// 行的横坐标一律取版心左沿：正文怎么缩进、标题怎么居中，页边那一列都不跟着晃。
fn mark_gutter_rows(metrics: &Metrics, rect: egui::Rect, rows: &[(f32, f32, f32)]) {
    for &(top, height, baseline) in rows {
        metrics.mark_sourced_row(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + top),
                egui::vec2(rect.width(), height),
            ),
            rect.top() + baseline,
        );
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
    let normal = metrics.body_font();
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

/// 居中 / 居右区的一行：正文字体、不缩进，整行相对版心居中或靠右，与 Word 的
/// 居中 / 右对齐段落、TeX 的 `\centering` / `\raggedleft` 一致。行内加粗与括号
/// 楷体照正文；一行太长折行时，每一行各自居中或靠右。
pub(crate) fn aligned_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    align: export::LineAlign,
) {
    let mut job = job(metrics.content);
    job.halign = match align {
        export::LineAlign::Center => Align::Center,
        export::LineAlign::Right => Align::Max,
    };
    append_inline(&mut job, metrics, text, &metrics.body_font());
    // halign 让每行相对 galley 原点对齐，所以原点要放在版心中线或右沿上，
    // 道理同 `line_block_runs` 的居中分支。
    let galley = layout(ui, job);
    let height = galley.size().y;
    let rows = row_spans(&galley);
    place(ui, metrics, height, |painter, rect| {
        let x = match align {
            export::LineAlign::Center => rect.left() + metrics.content / 2.0,
            export::LineAlign::Right => rect.left() + metrics.content,
        };
        let origin = egui::pos2(x, rect.top());
        push_galley_tints(metrics, &galley, origin);
        painter.galley(origin, galley, theme::paper::ink());
        mark_gutter_rows(metrics, rect, &rows);
    });
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
    let normal = metrics.body_font();
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

/// 一段命中范围里第一个有墨的字所在的下标；整段都是空白时返回 `None`。
///
/// 首行缩进的两个全角空格、编号对齐用的占位空格都排在正文前面，它们只是版式留白，
/// 高亮底色压上去会在行首拖出一截空框，观感很差。命中/点击范围仍按整段算，
/// 只有画底色时从第一个实字起笔。
pub(crate) fn first_ink(
    glyphs: &[egui::epaint::text::Glyph],
    range: Range<usize>,
) -> Option<usize> {
    let start = range.start;
    glyphs
        .get(range)?
        .iter()
        .position(|glyph| !glyph.chr.is_whitespace())
        .map(|offset| start + offset)
}

/// 一行的底色相对行框要挪多少：负值表示往上提。
///
/// egui 把基线钉在字体 ascent 上，「行距减字高」的余量整块留在字的下面。底色若照
/// 行框铺满，字就贴着上沿、下面空出小半行的色带。这里按这一行真正的字框取中，
/// 把整块底色往上挪半格余量，字便坐在色块的垂直正中；每行挪的量一样，连着几行
/// 亮起来时上下仍是严丝合缝的。
pub(crate) fn row_tint_offset(row: &egui::epaint::text::Row) -> f32 {
    let band = row
        .glyphs
        .iter()
        .fold(None, |band: Option<(f32, f32)>, glyph| {
            let top = glyph.pos.y - glyph.font_ascent;
            let bottom = top + glyph.font_height;
            Some(match band {
                Some((above, below)) => (above.min(top), below.max(bottom)),
                None => (top, bottom),
            })
        });
    match band {
        Some((top, bottom)) => (top + bottom) / 2.0 - row.size.y / 2.0,
        None => 0.0,
    }
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
        // 行号认的是纸面上的行：一段排成几行就记几笔，跳转认第一段压在这一行上的
        // 源码行——一行文字横跨两条源码行时，报的号指向它起头的那一条。
        metrics.mark_row(
            egui::Rect::from_min_size(
                egui::pos2(block_rect.left(), block_rect.top() + placed.pos.y),
                egui::vec2(metrics.content, placed.size.y),
            ),
            gutter::row_baseline(placed, block_rect.top() + placed.pos.y),
            segments
                .iter()
                .find(|segment| segment.chars.start < row_end && row_start < segment.chars.end)
                .map(|segment| segment.source.clone()),
        );
        for segment in segments {
            let start = segment.chars.start.max(row_start);
            let end = segment.chars.end.min(row_end);
            if start >= end {
                continue;
            }
            let local_start = start - row_start;
            let local_end = end - row_start;
            let tint_top = row_tint_offset(placed);
            let row_rect = |from: usize, to: usize| {
                let left = row_galley
                    .pos_from_cursor(egui::text::CCursor::new(from))
                    .left();
                let right = row_galley
                    .pos_from_cursor(egui::text::CCursor::new(to))
                    .left()
                    .max(left + 1.0);
                egui::Rect::from_min_max(
                    block_rect.left_top() + placed.pos.to_vec2() + egui::vec2(left, tint_top),
                    block_rect.left_top()
                        + placed.pos.to_vec2()
                        + egui::vec2(right, tint_top + placed.size.y),
                )
                .expand2(egui::vec2(3.0, 1.0))
            };
            let ink_start = first_ink(&placed.glyphs, local_start..local_end);
            let rect = row_rect(local_start, local_end);
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
            if let Some(ink_start) = ink_start
                && (anchored || response.hovered())
            {
                ui.painter().rect_filled(
                    row_rect(ink_start, local_end),
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
            metrics.body_bold_font()
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

/// 一格字形的视觉中线在 galley 内的纵向位置：把每个字形按
/// (行顶 + 基线 − ascent) 取顶、按同式加 font_height 取底，再在所有字形里取最小
/// 顶、最大底求均值。
///
/// egui 把基线钉在字体 ascent 上，行距减字高的余量整块留在字下方；按 galley 几
/// 何居中字会贴着上沿。这里按字形真实框取中，让字坐在色块的正中（与
/// `row_tint_offset` 同思路，后者只对单行做这件事）。
pub(crate) fn galley_visual_midline(galley: &egui::Galley) -> f32 {
    let mut min_top: Option<f32> = None;
    let mut max_bottom: Option<f32> = None;
    for row in &galley.rows {
        for glyph in &row.glyphs {
            let top = row.pos.y + glyph.pos.y - glyph.font_ascent;
            let bottom = top + glyph.font_height;
            min_top = Some(min_top.map_or(top, |v| v.min(top)));
            max_bottom = Some(max_bottom.map_or(bottom, |v| v.max(bottom)));
        }
    }
    match (min_top, max_bottom) {
        (Some(top), Some(bottom)) => (top + bottom) / 2.0,
        _ => 0.0,
    }
}

/// 表格里排好的一格：锚点所在的行列、跨度与排好的字。
pub(crate) struct TableCellLayout {
    row: usize,
    column: usize,
    row_span: usize,
    column_span: usize,
    galley: Arc<egui::Galley>,
    padding: f32,
    align: ColumnAlignment,
}

/// 量好尺寸、还没落到纸上的表格：各列宽、各行高与每个锚点单元格。
///
/// 连续版式（公文预览）整张画在一处；红头呈批件要真分页，按行切成几段分别
/// 落在不同的页上（见 `red::red_place_table`），两处共用同一份量法与画法。
pub(crate) struct MeasuredTable {
    pub(crate) widths: Vec<f32>,
    pub(crate) row_heights: Vec<f32>,
    cells: Vec<TableCellLayout>,
}

/// 表格：四号字、行距 21 磅，表头黑体居中，列宽直接取导出器算好的智能列宽，
/// 因此预览的列宽与导出的 Word 表格一致。`content_width` 是表格占的版心宽度。
pub(crate) fn measure_table(
    ui: &egui::Ui,
    metrics: &Metrics,
    rows: &[Vec<String>],
    aligns: &[export::ColumnAlign],
    spans: &[export::TableSpan],
    numbered: bool,
    content_width: f32,
) -> Option<MeasuredTable> {
    let columns = export::table_columns(rows, aligns, spans);
    if columns.is_empty() || rows.is_empty() {
        return None;
    }
    let widths = columns
        .iter()
        .map(|column| content_width * column.fraction)
        .collect::<Vec<_>>();
    let column_alignments = columns
        .iter()
        .map(|column| column.alignment)
        .collect::<Vec<_>>();

    // 先排出所有锚点单元格，再由跨行单元格反推各物理行所需高度。
    let line = metrics.pt(TABLE_LINE_PT);
    let mut cells = Vec::new();
    let mut row_heights = vec![line; rows.len()];
    let bold_font = metrics.font(theme::FONT_BOLD, TABLE_PT);
    for (row_index, row) in rows.iter().enumerate() {
        let header = row_index == 0;
        let font = metrics.font(
            if header {
                theme::FONT_HEITI
            } else {
                theme::FONT_FANGSONG
            },
            TABLE_PT,
        );
        for column in 0..widths.len() {
            let span = export::table_span_at(spans, row_index, column);
            if span.is_some_and(|span| !span.is_anchor(row_index, column)) {
                continue;
            }
            // 跨度理应落在网格内（`parse_table_cells` 保证矩形），但 `TableSpan`
            // 是普通结构体，起草页那边也在手改跨度；夹一下，排版错位总好过 panic。
            let row_span = span
                .map_or(1, |span| span.row_span)
                .min(rows.len() - row_index);
            let column_span = span
                .map_or(1, |span| span.column_span)
                .min(widths.len() - column);
            let width = widths[column..column + column_span].iter().sum::<f32>();
            let padding = metrics.pt(3.0).min(width * 0.12);
            let text = row.get(column).map_or("", String::as_str);
            let mut cell_job = job((width - 2.0 * padding).max(1.0));
            // 横向合并格跨列统一判定对齐，导出的 Word/TeX 与预览走同一条规则。
            let align = crate::export::table::resolve_cell_alignment(
                rows,
                spans,
                &column_alignments,
                numbered,
                row_index,
                column,
            );
            cell_job.halign = match align {
                ColumnAlignment::Center => Align::Center,
                ColumnAlignment::Right => Align::RIGHT,
                ColumnAlignment::Left => Align::LEFT,
            };
            // 表头整行黑体，不再认单元格里的加粗；正文格按 `**` 换粗体字面，
            // 与 DOCX 的 table_runs_sized、TeX 的 \GwBold 一致。括号换楷体那条
            // 规则只管正文，表格三端都不用。
            if header {
                cell_job.append(
                    &export::plain_text(text),
                    0.0,
                    text_format(font.clone(), line),
                );
            } else {
                for segment in export::inline_segments(text) {
                    let segment_font = if segment.bold {
                        bold_font.clone()
                    } else {
                        font.clone()
                    };
                    cell_job.append(&segment.text, 0.0, text_format(segment_font, line));
                }
            }
            let galley = layout(ui, cell_job);
            if row_span == 1 {
                row_heights[row_index] =
                    row_heights[row_index].max(galley.size().y + 2.0 * padding);
            }
            cells.push(TableCellLayout {
                row: row_index,
                column,
                row_span,
                column_span,
                galley,
                padding,
                align,
            });
        }
    }

    for cell in cells.iter().filter(|cell| cell.row_span > 1) {
        let current = row_heights[cell.row..cell.row + cell.row_span]
            .iter()
            .sum::<f32>();
        let required = cell.galley.size().y + 2.0 * cell.padding;
        if required > current {
            let extra = (required - current) / cell.row_span as f32;
            for height in &mut row_heights[cell.row..cell.row + cell.row_span] {
                *height += extra;
            }
        }
    }

    Some(MeasuredTable {
        widths,
        row_heights,
        cells,
    })
}

impl MeasuredTable {
    pub(crate) fn width(&self) -> f32 {
        self.widths.iter().sum()
    }

    /// 第 `row` 行起、不能在中间断开的一段行：纵向合并格跨到哪一行，这一段
    /// 就至少到哪一行。返回这一段的结束行（不含）。
    pub(crate) fn unbreakable_end(&self, row: usize) -> usize {
        let mut end = row + 1;
        let mut index = row;
        while index < end {
            for cell in self.cells.iter().filter(|cell| cell.row == index) {
                end = end.max(cell.row + cell.row_span);
            }
            index += 1;
        }
        end.min(self.row_heights.len())
    }

    /// 把 `rows` 这几行自上而下紧挨着画在 `origin` 处，返回每行的
    /// `(行号, 行框, 基线)`，供页边行号与点击回跳用。
    ///
    /// `rows` 里每个锚点格跨到的行必须也在其中且紧挨着——按
    /// [`Self::unbreakable_end`] 切段就能保证；表头（第 0 行）不参与纵向合并，
    /// 续页时可以单独拼在前面重复一遍。
    pub(crate) fn paint_rows(
        &self,
        painter: &egui::Painter,
        metrics: &Metrics,
        origin: egui::Pos2,
        rows: &[usize],
    ) -> Vec<(usize, egui::Rect, f32)> {
        let stroke = Stroke::new(1.0_f32.max(metrics.scale), theme::paper::ink());
        let mut x_offsets = vec![origin.x];
        for width in &self.widths {
            x_offsets.push(x_offsets.last().copied().unwrap_or(origin.x) + width);
        }
        // 各行在这一段里的上沿；不在这一段里的行没有位置。
        let mut tops = vec![None; self.row_heights.len()];
        let mut y = origin.y;
        for &row in rows {
            tops[row] = Some(y);
            y += self.row_heights[row];
        }
        let cell_rect = |cell: &TableCellLayout| {
            let top = tops[cell.row]?;
            let bottom = top
                + self.row_heights[cell.row..cell.row + cell.row_span]
                    .iter()
                    .sum::<f32>();
            Some(egui::Rect::from_min_max(
                egui::pos2(x_offsets[cell.column], top),
                egui::pos2(x_offsets[cell.column + cell.column_span], bottom),
            ))
        };

        let mut marked = Vec::with_capacity(rows.len());
        for &row in rows {
            let Some(top) = tops[row] else {
                continue;
            };
            let row_rect = egui::Rect::from_min_max(
                egui::pos2(origin.x, top),
                egui::pos2(origin.x + self.width(), top + self.row_heights[row]),
            );
            // 表格一行就是纸面上的一行，哪怕某个单元格里的字折了两行：看稿的人指的
            // 是「表里第几行」，页边的号必须跟着表行走，不能跟着单元格里的折行走。
            // 与下方 `painter.galley` 的 top 同算法，按字形框取中，号才贴字。
            let baseline = self
                .cells
                .iter()
                .filter(|cell| cell.row == row)
                .find_map(|cell| {
                    let rect = cell_rect(cell)?;
                    let top = rect.center().y - galley_visual_midline(&cell.galley);
                    cell.galley
                        .rows
                        .first()
                        .map(|placed| gutter::row_baseline(placed, top + placed.pos.y))
                })
                .unwrap_or(row_rect.center().y);
            marked.push((row, row_rect, baseline));
        }

        for cell in &self.cells {
            let Some(rect) = cell_rect(cell) else {
                continue;
            };
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
            let anchor = match cell.align {
                ColumnAlignment::Center => rect.center().x,
                ColumnAlignment::Right => rect.right() - cell.padding,
                ColumnAlignment::Left => rect.left() + cell.padding,
            };
            // 按字形框居中，不用 galley 几何居中：行距 21 磅比字高多出来的余量整块
            // 留在字下方，几何居中字会贴着上沿偏上半格。
            let top = rect.center().y - galley_visual_midline(&cell.galley);
            painter.galley(
                egui::pos2(anchor, top),
                cell.galley.clone(),
                theme::paper::ink(),
            );
        }
        marked
    }
}

/// 连续版式里的表格：整张占满版心宽度，一次画完。
pub(crate) fn table_block(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    rows: &[Vec<String>],
    aligns: &[export::ColumnAlign],
    spans: &[export::TableSpan],
    numbered: bool,
) {
    let Some(table) = measure_table(ui, metrics, rows, aligns, spans, numbered, metrics.content)
    else {
        return;
    };
    let total_height = table.row_heights.iter().sum::<f32>();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(metrics.content, total_height),
        egui::Sense::hover(),
    );
    let all_rows = (0..table.row_heights.len()).collect::<Vec<_>>();
    for (_, row_rect, baseline) in table.paint_rows(ui.painter(), metrics, rect.min, &all_rows) {
        metrics.mark_sourced_row(row_rect, baseline);
    }
}

/// 画一个来自 Markdown 的块，并让它可以点：悬停时淡底提示，点击后把它在源码中的
/// 范围报给调用方；`anchor` 命中的块常亮，与编辑器里的高亮一一对应。
pub(crate) fn clickable(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    range: &Range<usize>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    // 底色要压在文字下面：先占一个空图形位，量出块的范围后再回填。
    let backdrop = ui.painter().add(egui::Shape::Noop);
    // 块里画出来的行都算在这段源码名下，行号据此只编能改的行；画完立刻还原，
    // 否则紧跟其后的落款、版记会顶着这一块的范围被编号。
    metrics.enter_source(Some(range.clone()));
    let inner = ui.scope(add_contents).response.rect;
    metrics.enter_source(None);
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

/// 同 [`clickable`]，但底色按行贴着文字画：块里的部件画字时经
/// [`Metrics::push_tint`] 交上每一行的底色矩形，悬停只亮鼠标下那一行，
/// 命中时整块逐行亮起，行首缩进不压底色——与公文正文的高亮观感一致。
/// 块里一行都没交（图片之类）时退回整块矩形。
pub(crate) fn clickable_rows(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    range: &Range<usize>,
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let backdrop = ui.painter().add(egui::Shape::Noop);
    metrics.enter_source(Some(range.clone()));
    metrics.begin_tints();
    let inner = ui.scope(add_contents).response.rect;
    let tints = metrics.take_tints();
    metrics.enter_source(None);
    if !inner.is_positive() {
        return;
    }
    let tints = if tints.is_empty() {
        vec![inner.expand2(egui::vec2(4.0, 1.0))]
    } else {
        tints
    };
    let anchored = anchor.is_some_and(|anchor| {
        !anchor.is_empty()
            && !range.is_empty()
            && anchor.start < range.end
            && range.start < anchor.end
    });
    if anchored && *scroll_to_anchor {
        let whole = tints
            .iter()
            .skip(1)
            .fold(tints[0], |whole, rect| whole.union(*rect));
        scroll_preview_to_rect(ui, whole);
        *scroll_to_anchor = false;
    }
    let mut shapes = Vec::new();
    for (index, rect) in tints.iter().enumerate() {
        let response = ui.interact(
            *rect,
            egui::Id::new(("gw-preview-row", range.start, range.end, index)),
            egui::Sense::click(),
        );
        if response.clicked() {
            *clicked = Some(range.clone());
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let fill = if anchored {
            theme::accent_soft()
        } else if response.hovered() {
            theme::paper::hover_tint()
        } else {
            continue;
        };
        shapes.push(egui::Shape::from(egui::epaint::RectShape::filled(
            *rect,
            egui::CornerRadius::same(3),
            fill,
        )));
    }
    ui.painter().set(backdrop, egui::Shape::Vec(shapes));
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
    // 又一张纸：页边的号栏按纸分段，不跨着纸缝连下去。
    metrics.next_page();
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

    /// 研究报告按行高亮：带首行缩进的单行，底色从第一个实字起笔、不压缩进，
    /// 且字形框在底色里垂直居中（上下余量相差不过 1px）。
    #[test]
    fn row_tints_skip_indent_and_center_the_glyphs() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let metrics = Metrics::research(1000.0, Some(1.0));
        let _ = ctx.run_ui(Default::default(), |ui| {
            let text = format!(
                "{}现在的任务就是很好的达到了所需要的效果",
                indent(INDENT_CHARS)
            );
            let mut probe = job(metrics.content);
            probe.append(&text, 0.0, text_format(metrics.body_font(), metrics.line));
            let galley = layout(ui, probe);
            let row = &galley.rows[0];
            let glyph = row.glyphs.iter().find(|g| !g.chr.is_whitespace()).unwrap();
            let origin = egui::pos2(10.0, 20.0);

            metrics.begin_tints();
            push_galley_tints(&metrics, &galley, origin);
            let tints = metrics.take_tints();
            assert_eq!(tints.len(), 1);
            let tint = tints[0];

            let ink_left = origin.x + row.pos.x + glyph.pos.x;
            assert!(
                (tint.left() - (ink_left - 3.0)).abs() < 0.5,
                "底色应从第一个实字起笔：{} vs {}",
                tint.left(),
                ink_left
            );
            let glyph_top = origin.y + row.pos.y + glyph.pos.y - glyph.font_ascent;
            let glyph_bottom = glyph_top + glyph.font_height;
            let above = glyph_top - tint.top();
            let below = tint.bottom() - glyph_bottom;
            assert!(
                (above - below).abs() < 1.0,
                "字形应在底色里垂直居中：上 {above} 下 {below}"
            );
        });
    }

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
