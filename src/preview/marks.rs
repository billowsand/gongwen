//! 花脸稿标记在预览里的画法（方案需求结论第 10 条，与 PDF / Word 一致）：
//!
//! - 删除：红色 `#C00000` 文字 + 同色直删除线穿过字身。线要排完版之后按字形
//!   位置自己画：egui 自带的 `strikethrough` 画在行盒中线上，固定行距下会落到
//!   字脚（见 [`LineMarks::paint_strike`]）。
//! - 新增：蓝色 `#1F4E9E` 细框，框内文字保持正文色。egui 没有「字符边框」，
//!   同样排完版之后按字形位置自己画。框**随正文断行**：跨行时在行尾开口、
//!   下一行接着画，左右竖边只在整段新增的首尾各一笔——与 `gonghan-gwa.cls`
//!   的 `\GwAddOpen` / `\GwAddMid` / `\GwAddClose` 同一规则。
//!
//! 标记在排版任务里靠一个不画出来的记号认出：`underline` 设成线宽 0、颜色为
//! 删除红或新增蓝。epaint 对线宽 0 的描边一律跳过（`Stroke::is_empty`），所以它
//! 不产生任何图形；而记号跟着 section 的格式走，避头尾硬换行、两端对齐逐行切片
//! 都会原样复制它，画线画框时从 galley 自带的排版任务里就能逐字取回。

use crate::export::{self, RedlineKind};
use crate::preview::Metrics;
use eframe::egui;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, Stroke};
use std::sync::Arc;

/// 删除红。
pub(crate) const DEL_COLOR: Color32 = Color32::from_rgb(0xC0, 0x00, 0x00);
/// 新增蓝。
pub(crate) const ADD_COLOR: Color32 = Color32::from_rgb(0x1F, 0x4E, 0x9E);

/// 新增块的记号：线宽 0 的「下划线」，不画，只供画框时认字。
const ADDED_MARKER: Stroke = Stroke {
    width: 0.0,
    color: ADD_COLOR,
};
/// 删除块的记号，同上。
const DELETED_MARKER: Stroke = Stroke {
    width: 0.0,
    color: DEL_COLOR,
};

/// 格式上的标记记号。
fn marker_kind(format: &TextFormat) -> RedlineKind {
    if format.underline == ADDED_MARKER {
        RedlineKind::Added
    } else if format.underline == DELETED_MARKER {
        RedlineKind::Deleted
    } else {
        RedlineKind::Same
    }
}

/// 框的上边在基线之上的高度、下边在基线之下的深度（`\GwBoxTop` / `\GwBoxBottom`）。
const BOX_TOP_EM: f32 = 0.96;
const BOX_BOTTOM_EM: f32 = 0.24;
/// 框线宽 0.5pt，删除线 0.6pt（`\GwStrikeUnit`）。
const BOX_LINE_PT: f32 = 0.5;
const STRIKE_PT: f32 = 0.6;
/// 一段删除里没有汉字可量时，删除线在基线之上的高度。
const STRIKE_FALLBACK_EM: f32 = 0.38;
/// 框的左右竖边离字的距离（`\GwAdd` 里的 `\hspace{1.5pt}`）。
const BOX_PAD_PT: f32 = 1.5;

/// 给一段文字的格式打上花脸稿标记。`Same` 原样返回。
pub(crate) fn mark_format(mut format: TextFormat, kind: RedlineKind) -> TextFormat {
    match kind {
        RedlineKind::Same => {}
        RedlineKind::Deleted => {
            format.color = DEL_COLOR;
            format.underline = DELETED_MARKER;
        }
        RedlineKind::Added => format.underline = ADDED_MARKER,
    }
    format
}

/// 从格式反推花脸稿标记。一致性测试据此把预览的排版任务还原成 `(文字, 类型)` 序列。
#[cfg(test)]
pub(crate) fn format_kind(format: &TextFormat) -> RedlineKind {
    marker_kind(format)
}

/// 新增块前后留的空隙：框的竖边画在这道缝里，不压到相邻的字。
pub(crate) fn box_gap(metrics: &Metrics) -> f32 {
    metrics.pt(BOX_PAD_PT + BOX_LINE_PT)
}

/// 把一段已是纯文本、只可能还带着哨兵的文字（标题、附件名这类整行文字）按
/// 花脸稿块追加进排版任务，与导出侧的 `marked_runs` 对应。没有哨兵时就是
/// 原样一段 `Same`，与从前逐字一致。
pub(crate) fn append_marked_text(
    job: &mut LayoutJob,
    metrics: &Metrics,
    text: &str,
    format: TextFormat,
) {
    let mut previous = RedlineKind::Same;
    for chunk in export::redline_chunks(text) {
        if chunk.text.is_empty() {
            continue;
        }
        let gap = chunk_gap(metrics, previous, chunk.kind);
        job.append(&chunk.text, gap, mark_format(format.clone(), chunk.kind));
        previous = chunk.kind;
    }
}

/// 带哨兵的行内 Markdown → 带哨兵的纯文本：逐块过 `plain_text`，哨兵原样保留。
/// 标题这类「先取纯文本、再整行排」的路径用它，排版时交给 [`append_marked_text`]。
pub(crate) fn plain_keep_marks(text: &str) -> String {
    export::redline_chunks(text)
        .into_iter()
        .map(|chunk| {
            let plain = export::plain_text(&chunk.text);
            match chunk.kind {
                RedlineKind::Same => plain,
                RedlineKind::Deleted => export::mark_deleted(&plain),
                RedlineKind::Added => export::mark_added(&plain),
            }
        })
        .collect()
}

/// 两块相接处要不要空出框的竖边位置：进出新增块时各留一道缝。
pub(crate) fn chunk_gap(metrics: &Metrics, previous: RedlineKind, current: RedlineKind) -> f32 {
    if (previous == RedlineKind::Added) != (current == RedlineKind::Added)
        && (previous == RedlineKind::Added || current == RedlineKind::Added)
    {
        box_gap(metrics)
    } else {
        0.0
    }
}

/// 一个排版任务里逐字（不含换行符）的花脸稿标记：`Some((类型, 字号))`。
/// 整段没有标记时 [`LineMarks::of`] 返回 `None`，调用方据此跳过。
pub(crate) struct LineMarks {
    chars: Vec<Option<(RedlineKind, f32)>>,
    /// 这段文字之前 / 之后紧邻的字是否也在新增块里。整段排版时都是 false；
    /// 逐行各自排版的路径（含公式的混排行）靠它知道框是否跨行接续。
    before: bool,
    after: bool,
}

impl LineMarks {
    /// 按排版任务的 section 格式逐字取回标记。换行符不产生字形，这里也跳过，
    /// 因此下标与 galley 逐行累计的字形下标一一对应。
    pub(crate) fn of(job: &LayoutJob) -> Option<Self> {
        if !job
            .sections
            .iter()
            .any(|section| marker_kind(&section.format) != RedlineKind::Same)
        {
            return None;
        }
        let mut chars = Vec::with_capacity(job.text.len() / 3);
        let mut section = 0usize;
        for (byte, ch) in job.text.char_indices() {
            while section + 1 < job.sections.len() && job.sections[section].byte_range.end.0 <= byte
            {
                section += 1;
            }
            if ch == '\n' {
                continue;
            }
            let format = &job.sections[section].format;
            let kind = marker_kind(format);
            chars.push((kind != RedlineKind::Same).then_some((kind, format.font_id.size)));
        }
        Some(Self {
            chars,
            before: false,
            after: false,
        })
    }

    /// 声明这段文字前后紧邻的字是否也在新增块里（见字段说明）。
    pub(crate) fn continuing(mut self, before: bool, after: bool) -> Self {
        self.before = before;
        self.after = after;
        self
    }

    fn kind(&self, index: usize) -> RedlineKind {
        self.chars
            .get(index)
            .copied()
            .flatten()
            .map_or(RedlineKind::Same, |(kind, _)| kind)
    }

    fn added(&self, index: usize) -> bool {
        match self.chars.get(index) {
            Some(mark) => matches!(mark, Some((RedlineKind::Added, _))),
            None => self.after,
        }
    }

    /// 画一行里的标记：新增框与删除线。`first_char` 是这一行首字在整段里的下标，
    /// `origin` 是这一行 `row` 的左上角（字形坐标都相对它）。
    pub(crate) fn paint_row(
        &self,
        painter: &egui::Painter,
        metrics: &Metrics,
        first_char: usize,
        origin: egui::Pos2,
        row: &egui::epaint::text::Row,
    ) {
        let glyphs = &row.glyphs;
        let baseline = glyphs
            .iter()
            .map(|glyph| glyph.pos.y)
            .fold(f32::MIN, f32::max);
        let mut index = 0usize;
        while index < glyphs.len() {
            let kind = self.kind(first_char + index);
            if kind == RedlineKind::Same {
                index += 1;
                continue;
            }
            let start = index;
            while index < glyphs.len() && self.kind(first_char + index) == kind {
                index += 1;
            }
            let run = start..index;
            match kind {
                RedlineKind::Added => {
                    self.paint_box(painter, metrics, first_char, origin, glyphs, run, baseline)
                }
                RedlineKind::Deleted => {
                    self.paint_strike(painter, metrics, first_char, origin, glyphs, run, baseline)
                }
                RedlineKind::Same => {}
            }
        }
    }

    /// 一行里一段连续新增的框。框高按本行最大的新增字号定死，括号里的小一号字
    /// 不把框压矮（TeX 侧的 `\GwBoxFreeze`）。
    #[allow(clippy::too_many_arguments)]
    fn paint_box(
        &self,
        painter: &egui::Painter,
        metrics: &Metrics,
        first_char: usize,
        origin: egui::Pos2,
        glyphs: &[egui::epaint::text::Glyph],
        run: std::ops::Range<usize>,
        baseline: f32,
    ) {
        let stroke = Stroke::new(metrics.pt(BOX_LINE_PT).max(1.0), ADD_COLOR);
        let pad = metrics.pt(BOX_PAD_PT);
        let em = (0..glyphs.len())
            .filter_map(|offset| match self.chars.get(first_char + offset) {
                Some(Some((RedlineKind::Added, size))) => Some(*size),
                _ => None,
            })
            .fold(0.0_f32, f32::max);
        let top = origin.y + baseline - em * BOX_TOP_EM;
        let bottom = origin.y + baseline + em * BOX_BOTTOM_EM;
        // 首尾竖边只画在整段新增真正的起止处；跨行接续的那一头开口。
        let global = first_char + run.start;
        let opens = if global == 0 {
            !self.before
        } else {
            !self.added(global - 1)
        };
        let closes = !self.added(first_char + run.end);
        let left = origin.x + glyphs[run.start].pos.x - if opens { pad } else { 0.0 };
        let right = origin.x + glyphs[run.end - 1].max_x() + if closes { pad } else { 0.0 };
        painter.line_segment([egui::pos2(left, top), egui::pos2(right, top)], stroke);
        painter.line_segment(
            [egui::pos2(left, bottom), egui::pos2(right, bottom)],
            stroke,
        );
        if opens {
            painter.line_segment([egui::pos2(left, top), egui::pos2(left, bottom)], stroke);
        }
        if closes {
            painter.line_segment([egui::pos2(right, top), egui::pos2(right, bottom)], stroke);
        }
    }

    /// 一行里一段连续删除的删除线：穿过字身中部。
    ///
    /// 不用 egui 自带的 `strikethrough`：它画在字形逻辑框（按行距撑高的整个行盒）
    /// 的中线上，公文是 28 磅固定行距、字挤在行盒上部，那条线就落到了字脚，
    /// 看上去是下划线。这里按汉字墨迹的实际上下沿取中；整段没有汉字（纯数字、
    /// 标点）时按字号的 0.38em 估（中文字身大约占基线上 0.88em、下 0.12em）。
    #[allow(clippy::too_many_arguments)]
    fn paint_strike(
        &self,
        painter: &egui::Painter,
        metrics: &Metrics,
        first_char: usize,
        origin: egui::Pos2,
        glyphs: &[egui::epaint::text::Glyph],
        run: std::ops::Range<usize>,
        baseline: f32,
    ) {
        let stroke = Stroke::new(metrics.pt(STRIKE_PT).max(1.0), DEL_COLOR);
        let em = run
            .clone()
            .filter_map(|offset| match self.chars.get(first_char + offset) {
                Some(Some((_, size))) => Some(*size),
                _ => None,
            })
            .fold(0.0_f32, f32::max);
        let (sum, count) = glyphs[run.clone()]
            .iter()
            .filter(|glyph| is_ideograph(glyph.chr) && !glyph.uv_rect.is_nothing())
            .map(|glyph| glyph.pos.y + glyph.uv_rect.offset.y + glyph.uv_rect.size.y / 2.0)
            .fold((0.0_f32, 0usize), |(sum, count), y| (sum + y, count + 1));
        let middle = if count > 0 {
            sum / count as f32
        } else {
            baseline - em * STRIKE_FALLBACK_EM
        };
        let y = origin.y + middle;
        let left = origin.x + glyphs[run.start].pos.x;
        let right = origin.x + glyphs[run.end - 1].max_x();
        painter.line_segment([egui::pos2(left, y), egui::pos2(right, y)], stroke);
    }

    /// 画一整个 galley 的标记：`origin` 是 galley 的左上角。
    pub(crate) fn paint_galley(
        &self,
        painter: &egui::Painter,
        metrics: &Metrics,
        origin: egui::Pos2,
        galley: &egui::Galley,
    ) {
        let mut first_char = 0usize;
        for placed in &galley.rows {
            self.paint_row(
                painter,
                metrics,
                first_char,
                origin + placed.pos.to_vec2(),
                &placed.row,
            );
            first_char += placed.glyphs.len();
        }
    }
}

/// 汉字（含扩展 A 与兼容区）：删除线按它们的墨迹定高度，标点、数字不参与。
fn is_ideograph(ch: char) -> bool {
    matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
}

/// 整块图形（公式、图片）的花脸稿标记：删除在正中画一道红线，新增套一个完整的框。
pub(crate) fn paint_block_mark(
    painter: &egui::Painter,
    metrics: &Metrics,
    rect: egui::Rect,
    kind: RedlineKind,
) {
    match kind {
        RedlineKind::Same => {}
        RedlineKind::Deleted => {
            let stroke = Stroke::new(metrics.pt(STRIKE_PT).max(1.0), DEL_COLOR);
            painter.line_segment([rect.left_center(), rect.right_center()], stroke);
        }
        RedlineKind::Added => {
            let stroke = Stroke::new(metrics.pt(BOX_LINE_PT).max(1.0), ADD_COLOR);
            painter.rect_stroke(
                rect.expand(metrics.pt(BOX_PAD_PT)),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
        }
    }
}

/// 画一个 galley 上的新增框与删除线；没有标记时什么也不做。各处 `painter.galley`
/// 之后顺手调一次。
pub(crate) fn paint_galley_marks(
    painter: &egui::Painter,
    metrics: &Metrics,
    origin: egui::Pos2,
    galley: &egui::Galley,
) {
    if let Some(marks) = LineMarks::of(&galley.job) {
        marks.paint_galley(painter, metrics, origin, galley);
    }
}

/// 一行带哨兵的文字排成 galley：与 [`append_marked_text`] 同一口径，供需要
/// 自行摆位（落款、文号、红头呈批件密级）的要素使用；绘制时记得调
/// [`paint_galley_marks`]。没有哨兵时与普通一行排版完全一致。
pub(crate) fn marked_line_galley(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    width: f32,
    align: egui::Align,
    format: TextFormat,
) -> Arc<egui::Galley> {
    let mut job = super::layout::job(width);
    job.halign = align;
    append_marked_text(&mut job, metrics, text, format);
    super::layout::layout(ui, job)
}

/// 把排版任务还原成 `(文字, 类型)` 序列，相邻同类合并。一致性测试用。
#[cfg(test)]
pub(crate) fn job_fragments(job: &LayoutJob) -> Vec<(String, RedlineKind)> {
    let mut out: Vec<(String, RedlineKind)> = Vec::new();
    for section in &job.sections {
        let text = &job.text[section.byte_range.start.0..section.byte_range.end.0];
        if text.is_empty() {
            continue;
        }
        let kind = format_kind(&section.format);
        match out.last_mut() {
            Some(last) if last.1 == kind => last.0.push_str(text),
            _ => out.push((text.to_string(), kind)),
        }
    }
    out
}

/// 预览正文的 `(文字, 类型)` 序列：与 `redline.rs` 一致性测试里的 DOCX / TeX
/// 序列同口径（段落、列表项、对齐行各自过 `append_inline`，相邻同类合并）。
#[cfg(test)]
pub(crate) fn body_sequence(markdown: &str) -> Vec<(String, RedlineKind)> {
    use crate::export::MarkdownBlock;
    let metrics = Metrics::new(1000.0, Some(1.0));
    let mut out: Vec<(String, RedlineKind)> = Vec::new();
    for block in export::parse_markdown(markdown) {
        let text = match &block {
            MarkdownBlock::Paragraph(text)
            | MarkdownBlock::OrderedListItem { text, .. }
            | MarkdownBlock::Aligned { text, .. } => text,
            _ => continue,
        };
        let mut job = LayoutJob::default();
        super::append_inline(&mut job, &metrics, text, &metrics.body_font());
        for (text, kind) in job_fragments(&job) {
            match out.last_mut() {
                Some(last) if last.1 == kind => last.0.push_str(&text),
                _ => out.push((text, kind)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DraftInput, NumberingConfig, TemplateKind};
    use crate::preview::{PreviewScale, official_preview};
    use crate::units::UnitDisplay;

    fn render(markdown: &str) -> egui::FullOutput {
        let ctx = egui::Context::default();
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let display = UnitDisplay::new(&[]);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 4000.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            let _ = official_preview(
                ui,
                &input,
                &display,
                markdown,
                PreviewScale::zoom(Some(1.0)),
                None,
                false,
                &NumberingConfig::default(),
                false,
                &crate::visual_diff::ElementMarks::default(),
            );
        })
    }

    /// 画出来的蓝色框线：(横线, 竖线)。
    fn box_lines(output: &egui::FullOutput) -> (Vec<[egui::Pos2; 2]>, Vec<[egui::Pos2; 2]>) {
        let mut horizontal = Vec::new();
        let mut vertical = Vec::new();
        for clipped in &output.shapes {
            if let egui::epaint::Shape::LineSegment { points, stroke } = &clipped.shape
                && stroke.color == ADD_COLOR
            {
                if (points[0].x - points[1].x).abs() < 0.01 {
                    vertical.push(*points);
                } else {
                    horizontal.push(*points);
                }
            }
        }
        (horizontal, vertical)
    }

    fn texts(output: &egui::FullOutput) -> String {
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

    /// 跨行的新增只在首尾各画一笔竖边，中间换行处开口（与 TeX 的
    /// `\GwAddOpen` / `\GwAddClose` 同一规则），上下边逐行都画。
    #[test]
    fn a_wrapped_insertion_gets_one_box_open_at_line_ends() {
        let added = "各单位要按照统一部署认真组织开展自查自纠工作并将有关情况形成书面材料及时报送"
            .repeat(2);
        let doc = crate::redline::build("第一段。", &format!("第一段。{added}"));
        let output = render(&doc.markdown);
        let (horizontal, vertical) = box_lines(&output);
        assert_eq!(vertical.len(), 2, "整段新增只有首尾两条竖边：{vertical:?}");
        // 至少跨了两行：每行上下两条边。
        assert!(
            horizontal.len() >= 4,
            "跨行新增逐行画上下边：{horizontal:?}"
        );
        let text = texts(&output);
        assert!(
            !text.chars().any(export::is_redline_sentinel),
            "哨兵字符不能印到纸上：{text}"
        );
    }

    /// 删除的字红色、带删除线；新增的字保持正文色。
    #[test]
    fn deleted_text_is_red_and_struck_through() {
        let doc = crate::redline::build(
            "同意你单位关于报送情况的请示。",
            "同意你单位关于开展检查的请示。",
        );
        let metrics = Metrics::new(1000.0, Some(1.0));
        let mut job = LayoutJob::default();
        super::super::append_inline(&mut job, &metrics, &doc.markdown, &metrics.body_font());
        let deleted = job
            .sections
            .iter()
            .find(|section| format_kind(&section.format) == RedlineKind::Deleted)
            .expect("有删除片段");
        assert_eq!(deleted.format.color, DEL_COLOR);
        // 删除线自己画（见 `paint_strike`），不能再借 egui 的 strikethrough：
        // 那条线落在行盒中线上，固定行距下会跑到字脚。
        assert_eq!(deleted.format.strikethrough, Stroke::NONE);
        let added = job
            .sections
            .iter()
            .find(|section| format_kind(&section.format) == RedlineKind::Added)
            .expect("有新增片段");
        assert_ne!(added.format.color, DEL_COLOR, "新增的字保持正文色");
        assert_eq!(
            job_fragments(&job),
            [
                ("同意你单位关于".to_string(), RedlineKind::Same),
                ("报送情况".to_string(), RedlineKind::Deleted),
                ("开展检查".to_string(), RedlineKind::Added),
                ("的请示。".to_string(), RedlineKind::Same),
            ]
        );
    }

    /// 删除线要穿过字身中部，而不是压在字脚当下划线（公文 28 磅固定行距下，
    /// egui 自带的 strikethrough 就是这个毛病）。取被删的那个字的墨迹上下沿，
    /// 线必须落在中间三分之一里。
    #[test]
    fn the_strike_crosses_the_middle_of_the_characters() {
        let doc = crate::redline::build(
            "同意你单位关于报送情况的请示。",
            "同意你单位关于开展检查的请示。",
        );
        let output = render(&doc.markdown);
        let strikes: Vec<f32> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::LineSegment { points, stroke }
                    if stroke.color == DEL_COLOR && (points[0].y - points[1].y).abs() < 0.01 =>
                {
                    Some(points[0].y)
                }
                _ => None,
            })
            .collect();
        assert_eq!(strikes.len(), 1, "一段删除一条线：{strikes:?}");
        // 找到「报」字的墨迹框。
        let ink = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => shape.galley.rows.iter().find_map(|placed| {
                    placed
                        .glyphs
                        .iter()
                        .find(|glyph| glyph.chr == '报')
                        .map(|glyph| {
                            let top =
                                shape.pos.y + placed.pos.y + glyph.pos.y + glyph.uv_rect.offset.y;
                            (top, top + glyph.uv_rect.size.y)
                        })
                }),
                _ => None,
            })
            .expect("画出了被删的字");
        let (top, bottom) = ink;
        let third = (bottom - top) / 3.0;
        assert!(
            strikes[0] > top + third && strikes[0] < bottom - third,
            "删除线 y={} 应落在字身中部 {}..{}",
            strikes[0],
            top + third,
            bottom - third
        );
    }

    /// 新增标题连编号一起加框（方案规则 8），与 DOCX / TeX 一致。
    #[test]
    fn a_new_heading_boxes_its_number_too() {
        let doc = crate::redline::build(
            "## 工作目标\n\n正文。",
            "## 工作目标\n\n正文。\n\n## 保障措施\n\n正文。",
        );
        let metrics = Metrics::new(1000.0, Some(1.0));
        let mut counters = [0usize; 4];
        let numbering = NumberingConfig::default();
        let mut sequences = Vec::new();
        for block in export::parse_markdown(&doc.markdown) {
            if let export::MarkdownBlock::Heading(level, text) = block {
                let job = super::super::render::heading_job(
                    &metrics,
                    level,
                    &text,
                    &mut counters,
                    true,
                    &numbering,
                )
                .expect("二级标题有编号");
                sequences.push(job_fragments(&job));
            }
        }
        assert_eq!(sequences.len(), 2);
        assert!(
            sequences[0]
                .iter()
                .all(|(_, kind)| *kind == RedlineKind::Same),
            "未改的标题不带标记：{:?}",
            sequences[0]
        );
        let added: Vec<_> = sequences[1]
            .iter()
            .filter(|(_, kind)| *kind == RedlineKind::Added)
            .collect();
        assert_eq!(added.len(), 1, "{:?}", sequences[1]);
        assert!(
            added[0].0.starts_with("二、"),
            "编号在框里：{:?}",
            sequences[1]
        );
    }
}
