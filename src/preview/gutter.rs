//! 纸面行号：在公文预览的左页边铺一条号栏，给每一条**改得动的行**标一个号。
//!
//! 为什么要有它。预览投在会议室的屏幕上，敲字的人看左边的 Markdown，看稿的人看
//! 右边的纸面，两个人说的是同一份稿子，却没有同一套坐标：「这段中间那句」「再往下
//! 一点那行」全靠比划，一句话要说三遍。纸上有了号，一句「第 42 行」就落地了——
//! 法规草案的征求意见稿一向这么干，公文审稿要的也是同一件东西。
//!
//! 编的是**纸面上的行**，不是源码里的行。源码行号在左侧编辑区已经有一套
//! （`AppConfig::show_editor_line_numbers`），但那套号数的是源码：稿子里块与块之间
//! 都空着一行，标到纸上就是 1、3、5、7 一路单数，而且一个五行的长段只在头一行有号，
//! 中间四行仍然没法指。看稿的人指着屏幕说的「行」就是眼睛看到的那一行，页边的号
//! 必须与它一一对应，否则号就是假的。代价是改动会让后面的号重排——纸面行号的本性
//! 如此，投屏对稿本来也是改一处、对一处。
//!
//! 只有来自 Markdown 的行才有号。红头、密级、文号、主送、落款、版记出自表单，
//! 附件概要由附件标题生成，它们在纸上但改不动；给一行标号等于承诺「这行你可以改」，
//! 对着改不动的行报号只会白费一轮口舌。它们各自都有名字（「文号」「主送」「落款」），
//! 本来也不需要号来指认。实现上这条线划得很干净：记录只发生在带源码范围的块里，
//! 抬头、落款那些块走的是 `place` / `stacked`，一行都不记。
//!
//! ## 页边长什么样
//!
//! **一条号栏，不是一列裸数字。** 裸数字摆在页边留白里，看的人未必知道那是行号，
//! 也可能当成正文旁的批注或编号。铺一条定宽的淡底栏，再在靠版心那侧压一条极淡的
//! 竖线，页边立刻被划成两件东西：线右边是纸上的正文，线左边是给这份纸加的刻度。
//! 底色浅到几乎只是一层灰，投影仪压不出来时还有那条竖线兜底。栏是逐页铺的，
//! 附件另起一张纸就另起一条，不会跨着纸缝连成一根通天柱。
//!
//! **当前行的号是一枚实心牌子。** 光标停在哪一行，那一行的号就从淡灰翻成主色底、
//! 纸色字的小方牌——投屏时对面一眼就看见"现在说的是这一行"，不必再念一遍号码。
//! 指针扫过某一行时同样亮，因为对稿时鼠标本来就是拿来指的。平时一个牌子都不亮：
//! 常年亮着的高亮等于没有高亮。
//!
//! **号码与正文同一条基线。** 公文正文是固定行距 28 磅，比字高出一截，字实际上
//! 靠在行的上半段；号码若按行框居中就会整体偏下，看着像挂在两行之间。这里取行内
//! 首个字形的基线（`Glyph::pos.y` 就是基线），让号码与那一行的字踩同一条线。
//!
//! **字面用宋体。** 纸上的非正文数字在公文里本来就有自己的一副字面——页码用的
//! 宋体（`FontRole::PageNumber`，设置里换页码字体，行号跟着换）。正文是仿宋，
//! 号是宋体，字面本身就在说这两样东西不是一回事；界面无衬线字体则会让号码看着
//! 像贴在纸上的控件。深浅仍分两档：逢五重一档，眼睛先落到 40、45，再往回数两行。
//!
//! 号码可以点：点一下把光标带到那一行的源码。看稿的人报号，敲字的人点号，
//! 中间不必再数。
//!
//! 整块可以关掉：「视图 → 纸面行号」或设置页，对应
//! `AppConfig::show_preview_line_numbers`。默认关——页边有号还是没号，是投屏对稿
//! 时才需要的取舍，平时自己写稿不该被一列数字跟着走。

use crate::preview::Metrics;
use crate::theme;
use eframe::egui;
use std::ops::Range;

/// 号码字号，单位磅。正文三号（16 磅）配五号（10.5 磅）：小了三档，仍是公文
/// 用得上的字号，投到屏幕上认得出。
const NUMBER_PT: f32 = 10.5;
/// 号栏宽度，以及它的右缘与版心左沿之间的空当。栏宽按四位数留够——号码右对齐，
/// 长稿到三四位数时栏宽不能跟着变，否则整条栏的左缘会随着翻页晃。
const BAND_PT: f32 = 28.0;
const BAND_GAP_PT: f32 = 9.0;
/// 号码右缘与号栏右缘之间的内边距。比牌子的内缩多出一档，数字在牌子里
/// 左右上下才是一圈匀的空。
const NUMBER_PAD_PT: f32 = 6.5;
/// 逢几加重一档。五行一记是数行的老办法，十行太疏，三行太密。
const ACCENT_EVERY: usize = 5;
/// 号栏底色与右缘竖线的浓度。底色浅到只是一层灰；竖线稍重一点，投影仪压掉底色
/// 时它还在。
const BAND_TINT: f32 = 0.05;
const BAND_EDGE: f32 = 0.16;
/// 号栏与当前行牌子的圆角。
const BAND_RADIUS: u8 = 3;
const CHIP_RADIUS: u8 = 3;
/// 当前行那枚牌子相对号栏左右各收进去多少、上下各比数字的墨迹放宽多少（磅）。
/// 牌子是照着数字的墨迹长出来的，不是照着行框：行框比字高出一截（固定行距），
/// 按行框摆牌子，数字会顶在牌子上沿，下面空出一条。
const CHIP_INSET_PT: f32 = 3.0;
const CHIP_PAD_PT: f32 = 3.5;

/// 纸面上的一行：号码要标在哪儿，点下去又该跳到源码的哪一段。
pub(crate) struct GutterRow {
    /// 版心左沿的屏幕横坐标。号栏贴着它左边铺开，因此正文怎么缩进、标题怎么
    /// 居中，整条栏都在同一条竖线上。
    left: f32,
    /// 这一行在屏幕上的上下沿，以及版心右沿——指针落在这两条边之间的横带上，
    /// 就算"正指着这一行"，页边那个号跟着亮起来。
    top: f32,
    bottom: f32,
    right: f32,
    /// 这一行文字的基线。号码与正文踩同一条线，靠它对齐。
    baseline: f32,
    /// 这一行在第几张纸上。号栏逐页铺，纸与纸之间的空档不铺。
    page: usize,
    /// 这一行的文字在 Markdown 源码里的范围。点号码跳转用；
    /// 认不到源码的行仍然照编号，只是点不动。
    source: Option<Range<usize>>,
}

/// 本帧记下的全部行。`Metrics` 带着它走一遍版面，纸画完再统一标到页边。
#[derive(Default)]
pub(crate) struct Gutter {
    /// 总开关。关掉时一行不记，后面也就没得画。
    on: bool,
    /// 正在画的这一块来自源码的哪一段。`None` 表示当前画的是表单要素，
    /// 这期间记录的行一律丢掉——它们改不动，不该有号。
    sourced: Option<Range<usize>>,
    /// 正在画第几张纸。每铺一张纸加一。
    page: usize,
    rows: Vec<GutterRow>,
}

impl Gutter {
    pub(crate) fn enable(&mut self, on: bool) {
        self.on = on;
    }

    /// 取走本帧记下的全部行。取完就空，下一帧从头记起。
    pub(crate) fn take_rows(&mut self) -> Vec<GutterRow> {
        std::mem::take(&mut self.rows)
    }

    /// 又铺开一张纸。号栏按纸分段，靠它切开。
    pub(crate) fn next_page(&mut self) {
        self.page += 1;
    }

    /// 进入 / 离开一个来自源码的块。`clickable` 在画块之前后各调一次。
    pub(crate) fn set_source(&mut self, source: Option<Range<usize>>) {
        self.sourced = source;
    }

    /// 记下一行。`source` 给 `None` 时沿用当前块的源码范围。
    pub(crate) fn push(&mut self, rect: egui::Rect, baseline: f32, source: Option<Range<usize>>) {
        if !self.on || !rect.is_finite() || !baseline.is_finite() {
            return;
        }
        self.rows.push(GutterRow {
            left: rect.left(),
            top: rect.top(),
            bottom: rect.bottom(),
            right: rect.right(),
            baseline,
            page: self.page,
            source: source.or_else(|| self.sourced.clone()),
        });
    }

    /// 只在「正在画一个来自源码的块」时记。`line_block`、`table_block` 这些
    /// 抬头和落款也在用的部件走这条路，靠它把表单要素挡在外面。
    pub(crate) fn push_sourced(&mut self, rect: egui::Rect, baseline: f32) {
        if self.sourced.is_some() {
            self.push(rect, baseline, None);
        }
    }
}

/// 一行文字的基线：取行内第一个字形的基线（`Glyph::pos.y` 相对行顶就是基线），
/// 空行没有字形，退回按行高估一个。`top` 是这一行在屏幕上的上沿。
pub(crate) fn row_baseline(row: &egui::epaint::text::Row, top: f32) -> f32 {
    match row.glyphs.first() {
        Some(glyph) => top + glyph.pos.y,
        None => top + row.size.y * 0.75,
    }
}

/// 把本帧记下的行逐条标到页边。返回被点中的行在源码里的范围。
///
/// 必须在纸画完之后调用：号栏压在纸面留白上，晚画才不会被纸底盖住。
pub(crate) fn paint(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    anchor: Option<&Range<usize>>,
) -> Option<Range<usize>> {
    let rows = metrics.take_gutter_rows();
    if rows.is_empty() {
        return None;
    }
    let font = egui::FontId::new(
        metrics.pt(NUMBER_PT),
        theme::official_family(theme::FONT_SONGTI),
    );
    let band_width = metrics.pt(BAND_PT);
    let band_gap = metrics.pt(BAND_GAP_PT);
    let number_pad = metrics.pt(NUMBER_PAD_PT);
    let visible = ui.clip_rect();
    paint_bands(ui, &rows, band_width, band_gap, metrics.scale);
    // 指针落在哪一行上，那一行的号就亮：对稿时敲字的人把鼠标停在某一行，
    // 屏幕另一头的人立刻看得出在说第几行，不必再念一遍号码。
    let pointer = ui
        .ctx()
        .pointer_latest_pos()
        .filter(|pos| visible.contains(*pos));
    let chip_inset = metrics.pt(CHIP_INSET_PT);
    let chip_pad = metrics.pt(CHIP_PAD_PT);
    let mut clicked = None;
    for (index, row) in rows.iter().enumerate() {
        // 滚动区外的行照样编号，但不画也不登记热区：一份长稿有上千行，
        // 每帧为看不见的行排一次数字、登记一次热区纯属白费。
        if row.bottom < visible.top() || row.top > visible.bottom() {
            continue;
        }
        let number = index + 1;
        // 逢五加重：一列同样深浅的数字数起来要挨个看，有了重音就能跳着数。
        let ink = if number % ACCENT_EVERY == 0 {
            theme::paper::ink_muted()
        } else {
            theme::paper::ink_faint()
        };
        let galley = ui
            .painter()
            .layout_no_wrap(number.to_string(), font.clone(), ink);
        let band_right = row.left - band_gap;
        // 号码与正文踩同一条基线：galley 自己的基线相对它的顶在第一个字形上。
        let own_baseline = galley
            .rows
            .first()
            .map_or(galley.size().y * 0.75, |row| row_baseline(row, 0.0));
        let pos = egui::pos2(
            band_right - number_pad - galley.size().x,
            row.baseline - own_baseline,
        );
        // 牌子照数字的墨迹（`mesh_bounds` 是 galley 里真正有墨的范围）上下放宽一圈，
        // 横向铺满号栏：一列牌子的左右缘因此是齐的，只有高低随行走。
        let digits = galley.mesh_bounds.translate(pos.to_vec2());
        let chip = egui::Rect::from_min_max(
            egui::pos2(
                band_right - band_width + chip_inset,
                digits.top() - chip_pad,
            ),
            egui::pos2(band_right - chip_inset, digits.bottom() + chip_pad),
        );
        // 热区按整行给足：号码本身只有几个像素宽，照牌子点要瞄准。
        let hit = egui::Rect::from_min_max(
            egui::pos2(chip.left(), row.top),
            egui::pos2(chip.right(), row.bottom),
        );
        let response = ui.interact(
            hit,
            egui::Id::new(("gw-preview-line-number", number)),
            match &row.source {
                Some(_) => egui::Sense::click(),
                None => egui::Sense::hover(),
            },
        );
        let clickable = row.source.is_some();
        if clickable && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            clicked = row.source.clone();
        }
        // 亮牌子的三种情形：指针压在号上、指针停在这一行的正文上、左边编辑区的
        // 光标正落在这一行。平时一个都不亮——常年亮着的高亮等于没有高亮。
        let active = (clickable && response.hovered())
            || pointer.is_some_and(|at| {
                at.y >= row.top && at.y <= row.bottom && at.x >= chip.left() && at.x <= row.right
            })
            || matches!((&row.source, anchor), (Some(source), Some(anchor))
                if !anchor.is_empty() && anchor.start < source.end && source.start < anchor.end);
        if active {
            ui.painter()
                .rect_filled(chip, egui::CornerRadius::same(CHIP_RADIUS), theme::accent());
        }
        // 换色要走 `override_text_color`：galley 是连着颜色一起排好的，
        // `Painter::galley` 的那个颜色只是 galley 没带颜色时的兜底，改不动已排好的字。
        let mut text = egui::epaint::TextShape::new(pos, galley, ink);
        if active {
            text.override_text_color = Some(theme::paper::bg());
        }
        ui.painter().add(text);
    }
    clicked
}

/// 逐页铺号栏：淡底 + 靠版心那侧一条竖线。栏按各页第一条与最后一条号的上下沿
/// 收口，纸与纸之间的空档不铺——一条跨着纸缝连下去的柱子比没有栏还难看。
fn paint_bands(ui: &egui::Ui, rows: &[GutterRow], width: f32, gap: f32, scale: f32) {
    let painter = ui.painter();
    let mut start = 0usize;
    while start < rows.len() {
        let page = rows[start].page;
        let end = rows[start..]
            .iter()
            .position(|row| row.page != page)
            .map_or(rows.len(), |offset| start + offset);
        let page_rows = &rows[start..end];
        start = end;
        let Some(first) = page_rows.first() else {
            continue;
        };
        let right = first.left - gap;
        let top = page_rows.iter().fold(f32::MAX, |top, row| top.min(row.top));
        let bottom = page_rows
            .iter()
            .fold(f32::MIN, |bottom, row| bottom.max(row.bottom));
        let band =
            egui::Rect::from_min_max(egui::pos2(right - width, top), egui::pos2(right, bottom));
        if !band.is_positive() {
            continue;
        }
        painter.rect_filled(
            band,
            egui::CornerRadius::same(BAND_RADIUS),
            theme::paper::ink().gamma_multiply(BAND_TINT),
        );
        painter.line_segment(
            [band.right_top(), band.right_bottom()],
            egui::Stroke::new(
                1.0_f32.max(scale),
                theme::paper::ink().gamma_multiply(BAND_EDGE),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(top: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, top), egui::vec2(300.0, 20.0))
    }

    #[test]
    fn rows_are_only_recorded_while_a_sourced_block_is_being_drawn() {
        let mut gutter = Gutter::default();
        gutter.enable(true);
        // 红头、落款这些块不设源码范围，走 push_sourced 一行都不留。
        gutter.push_sourced(row(0.0), 15.0);
        gutter.set_source(Some(10..20));
        gutter.push_sourced(row(20.0), 35.0);
        gutter.set_source(None);
        gutter.push_sourced(row(40.0), 55.0);
        assert_eq!(gutter.rows.len(), 1);
        assert_eq!(gutter.rows[0].source, Some(10..20));
    }

    #[test]
    fn nothing_is_recorded_while_the_gutter_is_off() {
        let mut gutter = Gutter::default();
        gutter.set_source(Some(0..5));
        gutter.push(row(0.0), 15.0, None);
        gutter.push_sourced(row(20.0), 35.0);
        assert!(gutter.rows.is_empty());
    }

    #[test]
    fn an_explicit_source_wins_over_the_block_being_drawn() {
        let mut gutter = Gutter::default();
        gutter.enable(true);
        gutter.set_source(Some(0..5));
        gutter.push(row(0.0), 15.0, Some(7..9));
        gutter.push(row(20.0), 35.0, None);
        assert_eq!(gutter.rows[0].source, Some(7..9));
        assert_eq!(gutter.rows[1].source, Some(0..5));
    }

    /// 号栏按纸分段：换一张纸就换一条栏，否则附件与正文之间那道纸缝上会连出
    /// 一根通天柱。
    #[test]
    fn each_sheet_gets_its_own_band() {
        let mut gutter = Gutter::default();
        gutter.enable(true);
        gutter.set_source(Some(0..5));
        gutter.next_page();
        gutter.push_sourced(row(0.0), 15.0);
        gutter.push_sourced(row(20.0), 35.0);
        gutter.next_page();
        gutter.push_sourced(row(400.0), 415.0);
        let pages = gutter.rows.iter().map(|row| row.page).collect::<Vec<_>>();
        assert_eq!(pages, vec![1, 1, 2]);
    }
}
