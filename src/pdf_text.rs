//! PDF 页面的文字层：每个字落在哪、是什么字，供查看器显示文本光标、拖选与复制。
//!
//! 渲染用的是光栅图，本身不带文字。这里另走一遍页面内容流，用一个什么都不画的
//! 设备只记字形位置和 Unicode，按绘制顺序排好。内容流基本按阅读顺序出字，
//! 选区就用「第几个字」表示，跨行跨页都按这个顺序连起来，和常见阅读器一致。
//!
//! 坐标一律按页面宽高归一到 0..1（左上为原点），与显示缩放、渲染宽度都无关，
//! 每页只需取一次。

use eframe::egui;
use hayro::{
    hayro_interpret::{
        BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
        InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt, font::Glyph,
        hayro_cmap::BfString, hayro_syntax::page::Page, interpret_page,
    },
    vello_cpu::kurbo::{Affine, BezPath, Rect, Shape},
};

/// 文字命中框的竖向范围（字形单位，按 1000 的 em 计）：下探到降部、上到升部。
/// 按字体统一取值而不是按字形轮廓，同一行的高度才齐，「一」这种扁字也点得中。
const TEXT_DESCENT: f64 = -200.0;
const TEXT_ASCENT: f64 = 880.0;
/// 同一行相邻两字的横向间隙不超过行高的这么多倍就并成一段，词间空格和两端对齐
/// 拉开的字距都能连上，鼠标扫过一行时光标不会在字缝里来回闪。
const TEXT_JOIN_GAP: f64 = 1.5;
/// 竖排时上下相邻两字的间隙不超过字高的这么多倍就并成一列。标题字距拉得开，
/// 也要能连上；横排的行距通常更大，单字行才不会被误并。
const COLUMN_JOIN_GAP: f64 = 1.0;
/// 西文相邻两字的间隙超过行高的这么多倍就当作词间空格。TeX 这类排版器不画空格，
/// 词距全靠定位，复制时得按间隙补回来。
const WORD_GAP: f64 = 0.2;

/// 页面上的一个字。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextGlyph {
    /// 字形所占区域（归一化坐标）。
    pub(crate) rect: egui::Rect,
    /// 对应的文字。连字（fi）可能是多个字符；取不到 Unicode 时为空。
    pub(crate) text: String,
    /// 复制时前面要不要补一个空格。
    pub(crate) space_before: bool,
}

/// 一行（或同一行里连在一起的一段）文字；竖排时是一列。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextLine {
    pub(crate) rect: egui::Rect,
    /// 这一行的字在 `PageText::glyphs` 里的下标范围。
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// 竖排的一列：字自上而下排，插入位置和高亮都按纵向算。
    pub(crate) vertical: bool,
}

/// 一页的文字层。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PageText {
    pub(crate) glyphs: Vec<TextGlyph>,
    pub(crate) lines: Vec<TextLine>,
}

impl PageText {
    /// 这个点（归一化坐标）是否落在文字上。
    pub(crate) fn hit(&self, point: egui::Pos2) -> bool {
        self.lines.iter().any(|line| line.rect.contains(point))
    }

    /// 某点对应的插入位置：第几个字之前，取值 `0..=glyphs.len()`。
    /// 点在页面上方算页首、下方算页尾；落在行间或空白处就找最近的一行。
    pub(crate) fn caret_at(&self, point: egui::Pos2) -> usize {
        if point.y < 0.0 {
            return 0;
        }
        if point.y > 1.0 {
            return self.glyphs.len();
        }
        let line = self
            .lines
            .iter()
            .find(|line| line.rect.contains(point))
            .or_else(|| {
                self.lines.iter().min_by(|a, b| {
                    a.rect
                        .distance_sq_to_pos(point)
                        .total_cmp(&b.rect.distance_sq_to_pos(point))
                })
            });
        let Some(line) = line else {
            return 0;
        };
        // 越过字的中线算在它后面。
        (line.start..line.end)
            .find(|&index| {
                let center = self.glyphs[index].rect.center();
                if line.vertical {
                    point.y < center.y
                } else {
                    point.x < center.x
                }
            })
            .unwrap_or(line.end)
    }

    /// 选区 `start..end`（插入位置）的高亮框：每行把选中的字并成一块。
    pub(crate) fn highlight(&self, start: usize, end: usize) -> Vec<egui::Rect> {
        let end = end.min(self.glyphs.len());
        self.lines
            .iter()
            .filter_map(|line| {
                let from = line.start.max(start);
                let to = line.end.min(end);
                (from < to).then(|| {
                    // 行高方向按整行，不按单个字，选区边才齐。
                    let span = self.glyphs[from..to]
                        .iter()
                        .map(|glyph| glyph.rect)
                        .reduce(egui::Rect::union)
                        .unwrap_or(line.rect);
                    if line.vertical {
                        egui::Rect::from_x_y_ranges(line.rect.x_range(), span.y_range())
                    } else {
                        egui::Rect::from_x_y_ranges(span.x_range(), line.rect.y_range())
                    }
                })
            })
            .collect()
    }

    /// 选区 `start..end` 的文字。换行处补换行，同一行隔开的两段之间补空格。
    pub(crate) fn text(&self, start: usize, end: usize) -> String {
        let end = end.min(self.glyphs.len());
        let mut out = String::new();
        let mut previous_line: Option<&TextLine> = None;
        for line in &self.lines {
            let from = line.start.max(start);
            let to = line.end.min(end);
            if from >= to {
                continue;
            }
            if let Some(previous) = previous_line {
                let center = line.rect.center().y;
                let same_row = !line.vertical
                    && !previous.vertical
                    && center > previous.rect.top()
                    && center < previous.rect.bottom();
                out.push(if same_row { ' ' } else { '\n' });
            }
            for (offset, glyph) in self.glyphs[from..to].iter().enumerate() {
                if offset > 0 && glyph.space_before {
                    out.push(' ');
                }
                out.push_str(&glyph.text);
            }
            previous_line = Some(line);
        }
        out
    }
}

/// 取一页的文字层。
pub(crate) fn extract<'a>(page: &Page<'a>, cache: &InterpreterCache<'a>) -> PageText {
    let (width, height) = page.render_dimensions();
    let page_rect = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        page_rect,
        cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let mut device = GlyphCollector::default();
    interpret_page(page, &mut context, &mut device);
    build(device.glyphs, page_rect)
}

/// 设备收集到的一个字，坐标还是页面点。
struct RawGlyph {
    rect: Rect,
    text: String,
    /// 内容流里它前面紧挨着一个空格字形。
    space_before: bool,
}

/// 只记字形位置、什么都不画的设备。
#[derive(Default)]
struct GlyphCollector {
    glyphs: Vec<RawGlyph>,
    pending_space: bool,
}

impl<'a> Device<'a> for GlyphCollector {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _: &Paint<'a>,
        // 不可见文字（扫描件 OCR 叠的文字层）同样要收，阅读器在那里也能选字。
        _: &GlyphDrawMode,
    ) {
        let text = match glyph.as_unicode() {
            Some(BfString::Char(ch)) => ch.to_string(),
            Some(BfString::String(text)) => text,
            None => String::new(),
        };
        if !text.is_empty() && text.chars().all(char::is_whitespace) {
            self.pending_space = true;
            return;
        }
        // Type3 字形是一段绘图指令，没有现成的轮廓，公文导出也用不到，跳过。
        let Glyph::Outline(outline) = glyph else {
            return;
        };
        // 横向取轮廓的实际范围；空字形没有范围，直接略过。
        let bounds = outline.outline().bounding_box();
        if !bounds.is_finite() || bounds.width() <= 0.0 {
            return;
        }
        let em = Rect::new(bounds.x0, TEXT_DESCENT, bounds.x1, TEXT_ASCENT);
        let rect = (transform * glyph_transform).transform_rect_bbox(em);
        if !rect.is_finite() || rect.area() <= 0.0 {
            return;
        }
        self.glyphs.push(RawGlyph {
            rect,
            text,
            space_before: std::mem::take(&mut self.pending_space),
        });
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// 按绘制顺序把相邻的字并成行段，再归一到页面宽高。逐个和上一段比即可：
/// 竖向中心落在上一段里、横向间隙不超过行高的 `TEXT_JOIN_GAP` 倍，就并进去；
/// 单字段或竖排列的正下方紧挨着一个同列的字，就并成（或续上）竖排的一列。
fn build(raw: Vec<RawGlyph>, page: Rect) -> PageText {
    let mut glyphs: Vec<TextGlyph> = Vec::with_capacity(raw.len());
    let mut lines: Vec<(Rect, usize, bool)> = Vec::new();
    let mut previous: Option<(Rect, char)> = None;
    let normalize = |rect: Rect| {
        egui::Rect::from_min_max(
            egui::pos2(
                (rect.x0 / page.width()) as f32,
                (rect.y0 / page.height()) as f32,
            ),
            egui::pos2(
                (rect.x1 / page.width()) as f32,
                (rect.y1 / page.height()) as f32,
            ),
        )
    };
    for glyph in raw {
        // 裁剪框外的字看不见，也不该选得到。
        if glyph.rect.intersect(page).area() <= 0.0 {
            continue;
        }
        let first = glyph.text.chars().next().unwrap_or(' ');
        let last = glyph.text.chars().last().unwrap_or(' ');
        let mut space_before = glyph.space_before;
        let count = glyphs.len();
        let joined = lines.last_mut().is_some_and(|(line, start, vertical)| {
            let rect = glyph.rect;
            if !*vertical {
                let center = (rect.y0 + rect.y1) / 2.0;
                let height = line.height().max(rect.height());
                let gap = horizontal_gap(*line, rect);
                if center > line.y0 && center < line.y1 && gap <= height * TEXT_JOIN_GAP {
                    *line = line.union(rect);
                    return true;
                }
            }
            let center = (rect.x0 + rect.x1) / 2.0;
            let gap = rect.y0 - line.y1;
            let below = gap > -rect.height() * 0.5 && gap <= rect.height() * COLUMN_JOIN_GAP;
            if (*vertical || count - *start == 1) && center > line.x0 && center < line.x1 && below {
                *line = line.union(rect);
                *vertical = true;
                return true;
            }
            false
        });
        if joined && let Some((before, before_char)) = previous {
            let gap = horizontal_gap(before, glyph.rect);
            // 中文与中文、中文与西文之间的空隙是排版挤出来的，复制时不补空格。
            if gap > glyph.rect.height() * WORD_GAP && !is_cjk(before_char) && !is_cjk(first) {
                space_before = true;
            }
        }
        if !joined {
            lines.push((glyph.rect, glyphs.len(), false));
        }
        previous = Some((glyph.rect, last));
        glyphs.push(TextGlyph {
            rect: normalize(glyph.rect),
            text: glyph.text,
            space_before,
        });
    }
    let lines = lines
        .iter()
        .enumerate()
        .map(|(index, &(rect, start, vertical))| TextLine {
            rect: normalize(rect.intersect(page)),
            start,
            end: lines
                .get(index + 1)
                .map_or(glyphs.len(), |&(_, next, _)| next),
            vertical,
        })
        .collect();
    PageText { glyphs, lines }
}

/// 两个框的横向间隙，重叠时为 0。
fn horizontal_gap(a: Rect, b: Rect) -> f64 {
    if b.x0 >= a.x1 {
        b.x0 - a.x1
    } else if b.x1 <= a.x0 {
        a.x0 - b.x1
    } else {
        0.0
    }
}

/// 中日韩文字与全角标点。
fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{2E80}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE30}'..='\u{FE4F}'
        | '\u{FF00}'..='\u{FFEF}'
        | '\u{20000}'..='\u{2FA1F}')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在 100×100 的页面上按字排：每字宽 8、高 10。
    fn raw(x: f64, y: f64, text: &str) -> RawGlyph {
        RawGlyph {
            rect: Rect::new(x, y, x + 8.0, y + 10.0),
            text: text.into(),
            space_before: false,
        }
    }

    fn page(glyphs: Vec<RawGlyph>) -> PageText {
        build(glyphs, Rect::new(0.0, 0.0, 100.0, 100.0))
    }

    /// 同一行相邻的字并成一段，换行、隔得太远的另起一段。
    #[test]
    fn groups_glyphs_into_lines() {
        let text = page(vec![
            raw(0.0, 0.0, "a"),
            raw(8.0, 0.0, "b"),
            // 词间空格：隔开一些仍算同一行。
            raw(20.0, 0.0, "c"),
            // 下一行。
            raw(0.0, 20.0, "d"),
            // 同一行但隔得很远（表格另一栏）。
            raw(80.0, 20.0, "e"),
        ]);
        let ranges: Vec<_> = text.lines.iter().map(|l| (l.start, l.end)).collect();
        assert_eq!(ranges, vec![(0, 3), (3, 4), (4, 5)]);
        assert_eq!(
            text.lines[0].rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(0.28, 0.1))
        );
        assert_eq!(text.text(0, 5), "ab c\nd e");
    }

    /// 中文之间的排版空隙不补空格，显式的空格字形照样保留。
    #[test]
    fn copies_cjk_without_spurious_spaces() {
        let mut spaced = raw(40.0, 0.0, "B");
        spaced.space_before = true;
        let text = page(vec![
            raw(0.0, 0.0, "公"),
            raw(12.0, 0.0, "文"),
            raw(24.0, 0.0, "A"),
            spaced,
        ]);
        assert_eq!(text.text(0, 4), "公文A B");
        // 从中间截取时，选区开头的空格不带出来。
        assert_eq!(text.text(3, 4), "B");
    }

    /// 插入位置按字的中线分前后；行外找最近的一行，页外算页首页尾。
    #[test]
    fn caret_follows_pointer() {
        let text = page(vec![
            raw(0.0, 0.0, "a"),
            raw(8.0, 0.0, "b"),
            raw(0.0, 20.0, "c"),
        ]);
        assert_eq!(text.caret_at(egui::pos2(0.02, 0.05)), 0);
        assert_eq!(text.caret_at(egui::pos2(0.06, 0.05)), 1);
        assert_eq!(text.caret_at(egui::pos2(0.5, 0.05)), 2);
        // 第二行下方的空白：落到第二行。
        assert_eq!(text.caret_at(egui::pos2(0.5, 0.6)), 3);
        assert_eq!(text.caret_at(egui::pos2(0.5, -0.1)), 0);
        assert_eq!(text.caret_at(egui::pos2(0.5, 1.2)), 3);
    }

    /// 竖排：上下紧挨的字并成一列，列与列之间换行；插入位置按纵向算。
    #[test]
    fn stacks_vertical_text_into_columns() {
        // 自右向左两列，每列两字；第二列字距拉开（标题）。
        let text = page(vec![
            raw(80.0, 0.0, "竖"),
            raw(80.0, 10.0, "排"),
            raw(60.0, 0.0, "标"),
            raw(60.0, 18.0, "题"),
        ]);
        let ranges: Vec<_> = text
            .lines
            .iter()
            .map(|l| (l.start, l.end, l.vertical))
            .collect();
        assert_eq!(ranges, vec![(0, 2, true), (2, 4, true)]);
        assert_eq!(text.text(0, 4), "竖排\n标题");
        assert_eq!(text.caret_at(egui::pos2(0.84, 0.12)), 1);
        assert_eq!(
            text.highlight(1, 2),
            vec![egui::Rect::from_min_max(
                egui::pos2(0.8, 0.1),
                egui::pos2(0.88, 0.2)
            )]
        );
    }

    /// 高亮每行一块，竖向铺满整行。
    #[test]
    fn highlight_spans_selected_glyphs_per_line() {
        let text = page(vec![
            raw(0.0, 0.0, "a"),
            raw(8.0, 0.0, "b"),
            raw(16.0, 0.0, "c"),
            raw(0.0, 20.0, "d"),
        ]);
        let boxes = text.highlight(1, 4);
        assert_eq!(boxes.len(), 2);
        assert_eq!(
            boxes[0],
            egui::Rect::from_min_max(egui::pos2(0.08, 0.0), egui::pos2(0.24, 0.1))
        );
        assert_eq!(
            boxes[1],
            egui::Rect::from_min_max(egui::pos2(0.0, 0.2), egui::pos2(0.08, 0.3))
        );
        assert!(text.highlight(2, 2).is_empty());
    }
}
