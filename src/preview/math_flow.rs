//! 研究报告段落里的数学公式混排。
//!
//! 两种形态：`$$...$$` 独占一段、版心内居中；`$...$` 行内公式与文字按基线混排。
//! 渲染走 `math_render`（进程内排版 + 光栅化，微秒级），结果按
//! 「源码 + 块级标志 + 缩放 + 颜色」缓存进 `egui::Context`，每帧只贴纹理。
//! 渲染失败（不支持的命令、`\text{中文}` 缺字形等）降级为虚线框占位，
//! 框内用灰色小字写出公式源码，不 panic、不阻塞预览。

use super::layout::{
    indent, is_no_line_end, is_no_line_start, job, layout, place, row_tint_offset, text_format,
    tint_rect,
};
use super::math_render;
use super::{INDENT_CHARS, Metrics, RESEARCH_CAPTION_PT};
use crate::export;
use crate::theme;
use eframe::egui;
use egui::text::CCursor;
use std::ops::Range;

/// 光栅化超采样倍率：按 192dpi 出图、贴图缩小一半，高分屏不糊。
const OVERSAMPLE: f32 = 2.0;
/// 占位框内边距（磅）。
const PLACEHOLDER_PAD_PT: f32 = 2.0;
/// 占位框里源码小字最多占版心的几成宽，超出部分截断加省略号。
const PLACEHOLDER_TEXT_WIDTH: f32 = 0.8;

/// 一条公式的渲染缓存：成功是纹理加盒模型尺寸，失败也记一笔，免得每帧重排重画。
#[derive(Clone)]
enum Cached {
    Ready {
        texture: egui::TextureHandle,
        size: egui::Vec2,
        baseline: f32,
    },
    Failed,
}

/// 取一条公式的纹理；没有就同步渲染一次并缓存。渲染是微秒级调用，不必后台线程。
fn cached(ctx: &egui::Context, src: &str, display: bool, font_pt: f32) -> Cached {
    let color = theme::paper::ink();
    let quantized = (font_pt * 100.0).round() as i64;
    let id = egui::Id::new(("gw_math", src, display, quantized, color));
    if let Some(hit) = ctx.data(|data| data.get_temp::<Cached>(id)) {
        return hit;
    }
    let outcome = match math_render::render(src, display, font_pt, color, OVERSAMPLE) {
        Ok(rendered) => Cached::Ready {
            texture: ctx.load_texture(
                format!("gw_math/{display}/{quantized}/{src}"),
                rendered.image,
                egui::TextureOptions::LINEAR,
            ),
            size: rendered.size,
            baseline: rendered.baseline,
        },
        Err(_) => Cached::Failed,
    };
    ctx.data_mut(|data| data.insert_temp(id, outcome.clone()));
    outcome
}

/// trim 后整段被一对 `$$` 包住（长度大于 4）→ 独立块公式，内容是剥掉首尾 `$$` 的源码。
pub(crate) fn block_source(trimmed: &str) -> Option<&str> {
    if trimmed.len() <= 4 {
        return None;
    }
    trimmed
        .strip_prefix("$$")?
        .strip_suffix("$$")
        .map(str::trim)
        .filter(|inner| !inner.is_empty())
}

/// 独立块公式：独占一段，版心内居中，上下各空约 0.5 行。
pub(crate) fn display_block(ui: &mut egui::Ui, metrics: &Metrics, src: &str) {
    ui.add_space(metrics.line * 0.5);
    match cached(ui.ctx(), src, true, metrics.body_pt * metrics.scale) {
        Cached::Ready {
            texture,
            mut size,
            mut baseline,
        } => {
            // 比版心还宽的公式按比例压进版心，与图片块的口径一致。
            if size.x > metrics.content {
                let fit = metrics.content / size.x;
                size *= fit;
                baseline *= fit;
            }
            place(ui, metrics, size.y, |painter, rect| {
                let image = egui::Rect::from_center_size(rect.center(), size);
                metrics.push_tint(image.expand2(egui::vec2(3.0, 1.0)));
                metrics.mark_sourced_row(rect, rect.top() + baseline);
                painter.image(
                    texture.id(),
                    image,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            });
        }
        Cached::Failed => {
            placeholder_block(ui, metrics, src, metrics.line * 1.5, metrics.content);
        }
    }
    ui.add_space(metrics.line * 0.5);
}

/// 渲染失败的占位：虚线框 + 框内灰色小字写出公式源码。
fn placeholder_block(ui: &mut egui::Ui, metrics: &Metrics, src: &str, height: f32, width: f32) {
    place(ui, metrics, height, |painter, rect| {
        let rect = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(width.min(rect.width()), rect.height()),
        );
        dashed_rect(painter, rect, metrics.scale);
        let font = metrics.font(metrics.body_family, RESEARCH_CAPTION_PT);
        let label = truncate_to_fit(painter, font.clone(), src, rect.width());
        let galley = painter.layout(
            label,
            font,
            theme::paper::ink_muted(),
            rect.width() - 2.0 * metrics.pt(PLACEHOLDER_PAD_PT),
        );
        painter.galley(
            rect.center() - galley.size() / 2.0,
            galley,
            theme::paper::ink_muted(),
        );
    });
}

/// 画一圈虚线边框。
fn dashed_rect(painter: &egui::Painter, rect: egui::Rect, scale: f32) {
    let stroke = egui::Stroke::new(1.0_f32.max(scale), theme::paper::ink_faint());
    let dash = 4.0 * scale.max(1.0);
    let gap = 3.0 * scale.max(1.0);
    for points in [
        [rect.left_top(), rect.right_top()],
        [rect.right_top(), rect.right_bottom()],
        [rect.right_bottom(), rect.left_bottom()],
        [rect.left_bottom(), rect.left_top()],
    ] {
        painter.extend(egui::epaint::Shape::dashed_line(&points, stroke, dash, gap));
    }
}

/// 把源码截到能放进 `max_width`，截断了就在末尾补省略号。
fn truncate_to_fit(
    painter: &egui::Painter,
    font: egui::FontId,
    src: &str,
    max_width: f32,
) -> String {
    let width = |text: &str| {
        painter
            .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::BLACK)
            .size()
            .x
    };
    if width(src) <= max_width {
        return src.to_string();
    }
    let mut label = src.to_string();
    while !label.is_empty() && width(&format!("{label}…")) > max_width {
        label.pop();
    }
    format!("{label}…")
}

// ── 行内混排 ────────────────────────────────────────────────────────────────

/// 段落源码切成文本与行内公式两种片段。
enum Piece<'a> {
    Text(&'a str),
    Math(&'a str),
}

/// 词法：`\$` 是字面 `$`；`$...$` 结对要求内容非空、不含换行、不含 `$`；
/// 行内的 `$$` 不结对，两个字符都留在文本里（mdx 里 `$$` 也只有行首才是块级，
/// 预览与它对齐）。`$` 与 `\` 都是 ASCII，按字节扫不会切进 UTF-8 序列内部。
fn split_pieces(text: &str) -> Vec<Piece<'_>> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'$') {
            i += 2;
            continue;
        }
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        if bytes.get(i + 1) == Some(&b'$') {
            i += 2;
            continue;
        }
        // 找下一个未转义的 `$` 做闭合一侧。
        let mut j = i + 1;
        let close = loop {
            match bytes.get(j) {
                None => break None,
                Some(b'\\') if bytes.get(j + 1) == Some(&b'$') => j += 2,
                Some(b'$') => break Some(j),
                Some(_) => j += 1,
            }
        };
        let paired = close.is_some_and(|close| {
            let inner = &text[i + 1..close];
            !inner.is_empty() && !inner.contains(['$', '\n'])
        });
        if paired {
            let close = close.expect("paired 蕴含 close");
            if text_start < i {
                pieces.push(Piece::Text(&text[text_start..i]));
            }
            pieces.push(Piece::Math(&text[i + 1..close]));
            i = close + 1;
            text_start = i;
        } else {
            i += 1;
        }
    }
    if text_start < bytes.len() {
        pieces.push(Piece::Text(&text[text_start..]));
    }
    pieces
}

/// 混排的最小单位：文本原子是一个汉字、一个西文单词或一个空白字符；
/// 公式原子是一个排好版（或降级成占位框）的盒子。
enum Atom {
    Text {
        text: String,
        font: egui::FontId,
        width: f32,
    },
    Math(MathAtom),
}

/// 一个行内公式盒子。`texture` 为 None 表示渲染失败，画虚线占位框，
/// `label` 是框内要写的（可能截断过的）源码。
struct MathAtom {
    texture: Option<egui::TextureHandle>,
    label: String,
    size: egui::Vec2,
    baseline: f32,
}

impl Atom {
    fn width(&self) -> f32 {
        match self {
            Atom::Text { width, .. } => *width,
            Atom::Math(math) => math.size.x,
        }
    }

    fn first_char(&self) -> Option<char> {
        match self {
            Atom::Text { text, .. } => text.chars().next(),
            Atom::Math(_) => None,
        }
    }

    fn last_char(&self) -> Option<char> {
        match self {
            Atom::Text { text, .. } => text.chars().next_back(),
            Atom::Math(_) => None,
        }
    }
}

/// 含 `$` 的段落的混排入口：首行缩进 2 字，文本原子与公式盒子贪心流式排版。
///
/// 取舍：含公式的段落不做两端对齐，一律左对齐——公式是固定宽度的盒子，
/// 两端对齐只能拉伸文本原子之间的缝隙，盒子两侧的空白忽宽忽窄反而更难看。
pub(crate) fn paragraph(ui: &mut egui::Ui, metrics: &Metrics, text: &str) {
    let pieces = split_pieces(text);
    let normal = metrics.body_font();
    let indent_width = ui
        .ctx()
        .fonts_mut(|fonts| fonts.glyph_width(&normal, '\u{3000}'))
        * INDENT_CHARS;
    let band = line_band(ui, metrics);
    let mut atoms = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => text_atoms(ui.ctx(), metrics, text, &mut atoms),
            Piece::Math(src) => atoms.push(Atom::Math(math_atom(ui.ctx(), metrics, src, band))),
        }
    }
    if atoms.is_empty() {
        return;
    }
    let lines = break_lines(&atoms, metrics.content - indent_width, metrics.content);
    for (index, range) in lines.into_iter().enumerate() {
        draw_line(ui, metrics, &atoms[range], index == 0);
    }
}

/// 文本片段切原子：先过 `export::inline_segments` 保留加粗/括号的字体归属，
/// 再按 CJK 逐字、ASCII 按单词切开。`\$` 在这里折回字面的 `$`。
fn text_atoms(ctx: &egui::Context, metrics: &Metrics, text: &str, out: &mut Vec<Atom>) {
    let normal = metrics.body_font();
    ctx.fonts_mut(|fonts| {
        for segment in export::inline_segments(text) {
            let font = if segment.parenthesized {
                metrics.font(theme::FONT_KAITI, super::PAREN_PT)
            } else if segment.bold {
                metrics.body_bold_font()
            } else {
                normal.clone()
            };
            let mut word = String::new();
            let mut word_width = 0.0f32;
            let mut chars = segment.text.chars().peekable();
            while let Some(ch) = chars.next() {
                let ch = if ch == '\\' && chars.peek() == Some(&'$') {
                    chars.next();
                    '$'
                } else {
                    ch
                };
                if ch.is_ascii_alphanumeric() {
                    word_width += fonts.glyph_width(&font, ch);
                    word.push(ch);
                    continue;
                }
                if !word.is_empty() {
                    out.push(Atom::Text {
                        text: std::mem::take(&mut word),
                        font: font.clone(),
                        width: std::mem::take(&mut word_width),
                    });
                }
                let width = fonts.glyph_width(&font, ch);
                out.push(Atom::Text {
                    text: ch.to_string(),
                    font: font.clone(),
                    width,
                });
            }
            if !word.is_empty() {
                out.push(Atom::Text {
                    text: word,
                    font,
                    width: word_width,
                });
            }
        }
    });
}

/// 正文一行的垂直空间：`(基线以上, 基线以下)`，行高就是两者之和。
/// 基线位置按正文字体在固定行距下排一个全角空格量出来，与 `draw_line` 的口径一致。
fn line_band(ui: &egui::Ui, metrics: &Metrics) -> (f32, f32) {
    let mut probe = job(f32::INFINITY);
    probe.append(
        "\u{3000}",
        0.0,
        text_format(metrics.body_font(), metrics.line),
    );
    let above = layout(ui, probe)
        .rows
        .first()
        .and_then(|row| row.glyphs.first())
        .map_or(metrics.line * 0.75, |glyph| glyph.pos.y);
    (above, metrics.line - above)
}

/// 行内公式要压进多少倍才能按基线对齐塞进一行（不超过 1，不放大）。
/// 上下两侧分别看：分式、求和上下限这类竖向高的公式整体等比缩小，
/// 行距因此保持均匀，也不会顶出行外被相邻行盖住。
fn fit_to_band(size: egui::Vec2, baseline: f32, (above, below): (f32, f32)) -> f32 {
    let ascent = baseline.max(0.0);
    let descent = (size.y - baseline).max(0.0);
    let mut fit = 1.0f32;
    if ascent > above {
        fit = fit.min(above / ascent);
    }
    if descent > below {
        fit = fit.min(below / descent);
    }
    fit
}

/// 一个行内公式原子：渲染成功是纹理盒子，失败是虚线占位框盒子。
/// `band` 是正文一行在基线上下的空间，公式按它等比压缩，见 [`fit_to_band`]。
fn math_atom(ctx: &egui::Context, metrics: &Metrics, src: &str, band: (f32, f32)) -> MathAtom {
    match cached(ctx, src, false, metrics.body_pt * metrics.scale) {
        Cached::Ready {
            texture,
            mut size,
            mut baseline,
        } => {
            // 先压进一行的高度，再看宽度：比版心还宽的公式按比例压进版心，
            // 免得行内盒子直接溢出纸面。
            let mut fit = fit_to_band(size, baseline, band);
            if size.x * fit > metrics.content {
                fit = metrics.content / size.x;
            }
            size *= fit;
            baseline *= fit;
            MathAtom {
                texture: Some(texture),
                label: String::new(),
                size,
                baseline,
            }
        }
        Cached::Failed => {
            let font = metrics.font(metrics.body_family, RESEARCH_CAPTION_PT);
            let pad = metrics.pt(PLACEHOLDER_PAD_PT);
            let max_text = metrics.content * PLACEHOLDER_TEXT_WIDTH;
            let label = ctx.fonts_mut(|fonts| {
                let mut width = |text: &str| {
                    fonts
                        .layout_no_wrap(text.to_string(), font.clone(), theme::paper::ink_muted())
                        .size()
                        .x
                };
                if width(src) <= max_text {
                    return src.to_string();
                }
                let mut label = src.to_string();
                while !label.is_empty() && width(&format!("{label}…")) > max_text {
                    label.pop();
                }
                format!("{label}…")
            });
            let text_size = ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(label.clone(), font, theme::paper::ink_muted())
                    .size()
            });
            let size = egui::vec2(
                (text_size.x + 2.0 * pad).min(metrics.content),
                (text_size.y + 2.0 * pad).max(metrics.line * 0.8),
            );
            MathAtom {
                texture: None,
                label,
                size,
                baseline: size.y * 0.75,
            }
        }
    }
}

/// 两个相邻原子之间能不能断行：与 `layout.rs` 的 `can_break_between` 同一套
/// 避头尾规则，公式原子当作一个普通汉字看待——前后都可断，除非旁边是避头尾标点。
fn breakable(before: &Atom, after: &Atom) -> bool {
    let b = before.last_char();
    let a = after.first_char();
    if b.is_some_and(|c| c.is_whitespace()) || a.is_some_and(|c| c.is_whitespace()) {
        return true;
    }
    if b.is_some_and(is_no_line_end) || a.is_some_and(is_no_line_start) {
        return false;
    }
    !matches!((b, a), (Some(x), Some(y)) if x.is_ascii_alphanumeric() && y.is_ascii_alphanumeric())
}

/// 贪心流式断行：填满一行后从行尾往前找最近的合法断点；整行找不到断点
/// （长公式、长西文串）就向后溢出到下一个断点，不把原子拦腰切开——
/// 与 `layout.rs` 的 `break_lines` 同一取舍。首行宽度要减掉 2 字缩进。
fn break_lines(atoms: &[Atom], first_width: f32, width: f32) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut used = 0.0f32;
    let mut limit = first_width;
    let mut i = 0usize;
    while i < atoms.len() {
        if i > start && used + atoms[i].width() > limit {
            let mut at = i;
            while at > start && !breakable(&atoms[at - 1], &atoms[at]) {
                at -= 1;
            }
            if at > start {
                lines.push(start..at);
                used = atoms[at..i].iter().map(Atom::width).sum();
                start = at;
                limit = width;
                continue;
            }
            let mut at = (i + 1).max(start + 1);
            while at < atoms.len() && !breakable(&atoms[at - 1], &atoms[at]) {
                at += 1;
            }
            lines.push(start..at);
            start = at;
            i = at;
            used = 0.0;
            limit = width;
            continue;
        }
        used += atoms[i].width();
        i += 1;
    }
    if start < atoms.len() {
        lines.push(start..atoms.len());
    }
    lines
}

/// 画一行混排：文本原子拼成该行的一个 LayoutJob（逐原子带字体 section），
/// 公式按基线对齐贴入。公式在 LayoutJob 里占一个全角空格位，用 section 的
/// `leading_space` 把前进宽度补成盒宽——空格无墨迹，但它后面的文本因此让出
/// 公式位置；公式左缘 = 空格字形位置 − 补宽，用 `pos_from_cursor` 精确取回。
///
/// 不能用 `extra_letter_spacing`：epaint 把它加在字形「之前」且段落首字形不加，
/// 公式会被画到空位右侧、压住后文，行首公式则干脆没有空位。
fn draw_line(ui: &mut egui::Ui, metrics: &Metrics, atoms: &[Atom], first_line: bool) {
    let normal = metrics.body_font();
    let em = ui
        .ctx()
        .fonts_mut(|fonts| fonts.glyph_width(&normal, '\u{3000}'));
    let mut job = job(f32::INFINITY);
    if first_line {
        job.append(
            &indent(INDENT_CHARS),
            0.0,
            text_format(normal.clone(), metrics.line),
        );
    }
    let mut slots: Vec<(usize, f32, &MathAtom)> = Vec::new();
    for atom in atoms {
        match atom {
            Atom::Text { text, font, .. } => {
                job.append(text, 0.0, text_format(font.clone(), metrics.line));
            }
            Atom::Math(math) => {
                let pad = math.size.x - em;
                slots.push((job.text.chars().count(), pad, math));
                job.append("\u{3000}", pad, text_format(normal.clone(), metrics.line));
            }
        }
    }
    // 断行已经算好，不限宽排版就不会再被改切（kinsoku_wrap 对无穷宽原样返回）。
    let galley = layout(ui, job);

    // 行高 = max(正文行距, 各公式图高)；基线先让文本在行里垂直居中，
    // 再上抬到能兜住最深的公式顶部，行高随之兜底到最深的公式底部。
    let mut height = metrics.line;
    for math in atoms.iter().filter_map(|atom| match atom {
        Atom::Math(math) => Some(math),
        _ => None,
    }) {
        height = height.max(math.size.y);
    }
    let text_baseline = galley
        .rows
        .first()
        .and_then(|row| row.glyphs.first())
        .map_or(metrics.line * 0.75, |glyph| glyph.pos.y);
    let mut baseline = (height - metrics.line) / 2.0 + text_baseline;
    for math in atoms.iter().filter_map(|atom| match atom {
        Atom::Math(math) => Some(math),
        _ => None,
    }) {
        baseline = baseline.max(math.baseline);
        height = height.max(baseline + (math.size.y - math.baseline));
    }

    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
    let painter = ui.painter();
    let text_origin = rect.left_top() + egui::vec2(0.0, baseline - text_baseline);
    painter.galley(text_origin, galley.clone(), theme::paper::ink());
    let math_rects: Vec<egui::Rect> = slots
        .iter()
        .map(|&(char_index, pad, math)| {
            let x = rect.left() + galley.pos_from_cursor(CCursor::new(char_index)).left() - pad;
            let top = rect.top() + baseline - math.baseline;
            egui::Rect::from_min_size(egui::pos2(x, top), math.size)
        })
        .collect();
    if let Some(row) = galley.rows.first() {
        metrics.mark_sourced_row(rect, text_origin.y + row.pos.y + text_baseline);
        push_line_tint(metrics, row, text_origin, &math_rects);
    }
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    for ((_, _, math), math_rect) in slots.into_iter().zip(math_rects) {
        match &math.texture {
            Some(texture) => {
                painter.image(texture.id(), math_rect, uv, egui::Color32::WHITE);
            }
            None => {
                dashed_rect(painter, math_rect, metrics.scale);
                let font = metrics.font(metrics.body_family, RESEARCH_CAPTION_PT);
                let label =
                    painter.layout_no_wrap(math.label.clone(), font, theme::paper::ink_muted());
                painter.galley(
                    math_rect.center() - label.size() / 2.0,
                    label,
                    theme::paper::ink_muted(),
                );
            }
        }
    }
}

/// 一行混排的底色：横向从第一个有墨的字（或公式）到最后一个字（或公式），
/// 行首缩进不压底色；纵向以文字行按 `row_tint_offset` 居中，再兜住比行高的公式。
/// 公式在 galley 里只是全角空格占位，所以文字的墨迹与公式盒子分开取再合并。
fn push_line_tint(
    metrics: &Metrics,
    row: &egui::epaint::text::PlacedRow,
    origin: egui::Pos2,
    math_rects: &[egui::Rect],
) {
    let text_top = origin.y + row.pos.y + row_tint_offset(row);
    let mut span: Option<(f32, f32)> = None;
    let mut widen = |left: f32, right: f32| {
        span = Some(match span {
            Some((l, r)) => (l.min(left), r.max(right)),
            None => (left, right),
        });
    };
    let inked = row.glyphs.iter().filter(|glyph| !glyph.chr.is_whitespace());
    for glyph in inked {
        let left = origin.x + row.pos.x + glyph.pos.x;
        widen(left, origin.x + row.pos.x + glyph.max_x());
    }
    for math in math_rects {
        widen(math.left(), math.right());
    }
    let Some((left, right)) = span else {
        return;
    };
    let mut tint = tint_rect(left, right, text_top, row.size.y);
    for math in math_rects {
        tint = tint.union(egui::Rect::from_x_y_ranges(
            tint.x_range(),
            math.expand2(egui::vec2(0.0, 1.0)).y_range(),
        ));
    }
    metrics.push_tint(tint);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个指定宽度的文本原子（断行测试只看宽度与首尾字符，不用真字体）。
    fn text_atom(text: &str, width: f32) -> Atom {
        Atom::Text {
            text: text.to_string(),
            font: egui::FontId::default(),
            width,
        }
    }

    fn math_box(width: f32) -> Atom {
        Atom::Math(MathAtom {
            texture: None,
            label: String::new(),
            size: egui::vec2(width, 1.0),
            baseline: 0.75,
        })
    }

    #[test]
    fn dollar_pairs_split_text_and_math() {
        let pieces = split_pieces("质能方程 $E=mc^2$ 揭示了关系。");
        assert_eq!(pieces.len(), 3);
        assert!(matches!(&pieces[0], Piece::Text("质能方程 ")));
        assert!(matches!(&pieces[1], Piece::Math("E=mc^2")));
        assert!(matches!(&pieces[2], Piece::Text(" 揭示了关系。")));
    }

    #[test]
    fn escaped_dollar_is_literal_text() {
        let pieces = split_pieces(r"价格 \$5 元");
        assert_eq!(pieces.len(), 1);
        assert!(matches!(&pieces[0], Piece::Text(r"价格 \$5 元")));
    }

    /// 行内的 `$$` 不结对：两个字符都留在文本里，与 mdx 的「行首才块级」一致。
    #[test]
    fn double_dollar_inline_stays_text() {
        let pieces = split_pieces("见 $$x$$ 所示");
        assert_eq!(pieces.len(), 1);
        assert!(matches!(&pieces[0], Piece::Text("见 $$x$$ 所示")));
    }

    #[test]
    fn unpaired_or_empty_dollars_are_literal_text() {
        for text in ["见 $x 所示", "见 $$ 所示", "$x$ 与 $y"] {
            let pieces = split_pieces(text);
            let math = pieces
                .iter()
                .filter(|piece| matches!(piece, Piece::Math(_)))
                .count();
            match text {
                "$x$ 与 $y" => assert_eq!(math, 1, "{text} 只有第一对能结对"),
                _ => assert_eq!(math, 0, "{text} 不应切出公式"),
            }
        }
        // 含换行的不结对。
        assert!(
            split_pieces("$x\ny$")
                .iter()
                .all(|piece| matches!(piece, Piece::Text(_)))
        );
    }

    #[test]
    fn block_source_requires_both_delimiters() {
        assert_eq!(block_source("$$E=mc^2$$"), Some("E=mc^2"));
        assert_eq!(block_source("$$ E=mc^2 $$"), Some("E=mc^2"));
        assert_eq!(block_source("$$$$"), None);
        assert_eq!(block_source("$$x$"), None);
        assert_eq!(block_source("$x$"), None);
        assert_eq!(block_source("$$  $$"), None);
    }

    #[test]
    fn break_rules_match_kinsoku_with_math_as_a_hanzi() {
        // 公式与汉字之间可断。
        assert!(breakable(&math_box(1.0), &text_atom("后", 1.0)));
        assert!(breakable(&text_atom("前", 1.0), &math_box(1.0)));
        // 避头：公式后面跟逗号不能断。
        assert!(!breakable(&math_box(1.0), &text_atom("，", 1.0)));
        // 避尾：前括号后面不能断。
        assert!(!breakable(&text_atom("（", 1.0), &math_box(1.0)));
        // 空白两侧随便断；西文单词与数字串内部不断（单词本身是一个原子，
        // 这里验证相邻单词原子之间不因为都是 ASCII 就连死——中间有空格原子）。
        assert!(breakable(&text_atom(" ", 0.5), &text_atom("abc", 1.5)));
        assert!(!breakable(&text_atom("ab", 1.0), &text_atom("12", 1.0)));
    }

    #[test]
    fn greedy_lines_fill_first_then_wrap() {
        // 行宽 3.5：三个 1 宽的汉字 + 一个 1.5 宽的公式，公式放不下就换行。
        let atoms = vec![
            text_atom("甲", 1.0),
            text_atom("乙", 1.0),
            text_atom("丙", 1.0),
            math_box(1.5),
            text_atom("丁", 1.0),
        ];
        assert_eq!(break_lines(&atoms, 3.5, 3.5), vec![0..3, 3..5]);
    }

    #[test]
    fn first_line_is_narrower_by_the_indent() {
        // 首行宽 2.0（缩进吃掉 1.5），后续行 3.5。
        let atoms = vec![
            text_atom("甲", 1.0),
            text_atom("乙", 1.0),
            text_atom("丙", 1.0),
            text_atom("丁", 1.0),
        ];
        assert_eq!(break_lines(&atoms, 2.0, 3.5), vec![0..2, 2..4]);
    }

    /// 公式占位的几何：左缘 = 空格字形位置 − 补宽，恰好接在前文之后；
    /// 后文从左缘 + 盒宽起排。段首（行首公式）同样成立。
    #[test]
    fn math_slot_reserves_exactly_its_box_width() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        let font = egui::FontId::proportional(16.0);
        let box_width = 50.0;
        for prefix in ["ab", ""] {
            // 测试环境的默认字体没有全角空格，换个有字形的字符验证同一套几何。
            let em = ctx.fonts_mut(|fonts| fonts.glyph_width(&font, 'x'));
            let pad = box_width - em;
            let mut job = egui::text::LayoutJob::default();
            let format = egui::TextFormat::simple(font.clone(), egui::Color32::BLACK);
            job.append(prefix, 0.0, format.clone());
            let slot = job.text.chars().count();
            job.append("x", pad, format.clone());
            job.append("c", 0.0, format);
            let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
            let before = galley.pos_from_cursor(CCursor::new(slot)).left() - pad;
            let prefix_end = if prefix.is_empty() {
                0.0
            } else {
                galley.rows[0].glyphs[slot - 1].max_x()
            };
            let after = galley.pos_from_cursor(CCursor::new(slot + 1)).left();
            assert!(
                (before - prefix_end).abs() < 1.0,
                "{prefix:?}: {before} vs {prefix_end}"
            );
            assert!(
                (after - before - box_width).abs() < 1.0,
                "{prefix:?}: {after} - {before}"
            );
        }
    }

    /// 行内公式按基线上下两侧分别压进一行：矮的不动，高的等比缩小，不放大。
    #[test]
    fn inline_math_shrinks_into_the_line_band() {
        let band = (20.0, 8.0);
        // 普通 x^2：上 15 下 3，放得下，不缩。
        assert_eq!(fit_to_band(egui::vec2(10.0, 18.0), 15.0, band), 1.0);
        // \frac12：上 22 下 12，下侧更紧，按 8/12 缩。
        let fit = fit_to_band(egui::vec2(8.0, 34.0), 22.0, band);
        assert!((fit - 8.0 / 12.0).abs() < 1e-6, "{fit}");
        // 只有上侧超：按 20/30 缩。
        let fit = fit_to_band(egui::vec2(8.0, 32.0), 30.0, band);
        assert!((fit - 20.0 / 30.0).abs() < 1e-6, "{fit}");
    }

    #[test]
    fn an_unbreakable_run_overflows_instead_of_splitting() {
        // 行宽 2，一个 5 宽的公式原子：独占一行溢出，不切开。
        let atoms = vec![math_box(5.0), text_atom("甲", 1.0)];
        assert_eq!(break_lines(&atoms, 2.0, 2.0), vec![0..1, 1..2]);
    }
}
