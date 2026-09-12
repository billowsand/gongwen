//! 公文预览右缘的导航刻度：常驻一列细刻度反映全文结构，靠近右缘则在刻度左侧
//! 铺开一列标题，指针最近的那条隆起放大。
//!
//! 为什么不做成侧栏。起草页左右已经排满——左边公文要素、右边版本与审校提示两个
//! 抽屉，中间的版式预览还要按纸张宽度自适应缩放：中间一窄，纸上的字就跟着变小。
//! **宽度是这一页最稀缺的资源**，再切一栏给导航，代价是全程看不清正文。悬浮层
//! 不占宽度，代价只有"平时看不见"，而这个代价可以用常驻刻度抵掉。
//!
//! 为什么不是纯悬停唤出。不显示的功能等于不存在，而且"我现在读到第几节"这个
//! 最该一直在线的信息，一旦藏起来就只有主动去找它时才告诉你——那时候你已经知道了。
//! 所以常驻一列极淡的刻度：一条一个标题，按层级定长短，当前所在那条高亮。它顺带
//! 还回答了一个公文很在意的问题——各节长短是否均衡，某节明显长出一截通常意味着
//! 结构没拆开。
//!
//! 为什么标题是就地铺开而不是另开一个大纲框。框一展开就盖掉一块版面，视线也得
//! 离开正文横移过去。这里让标题直接贴着刻度长出来：指针最近的那条最大最亮，
//! 往外逐档缩小变淡，像 Dock 被鼠标顶起的那一段。底衬从右往左化开、没有边框，
//! 所以整列看着是浮在纸上，而不是压在一块板子上。
//!
//! 说明白一点：底衬不是真正的毛玻璃。egui 的渲染管线取不到已经画好的画面去做
//! 高斯模糊，这里用的是渐变的半透明薄色——底下的字透得出来但不会糊。
//!
//! 编号一律来自 [`export::HeadingCounters`]，与 DOCX/LaTeX 导出和版式预览共用同一套
//! 计数器。导航里写「三、」而预览里排出「四、」是最难查的那类 bug，共用计数器
//! 从根上排除它。
//!
//! 整块可以关掉：「视图 → 导航」或设置页里那一项，对应
//! `AppConfig::show_preview_navigator`。默认开——刻度只占右缘十几个点，
//! 又不吃点击，代价小到不值得让人先去找开关；但右缘要绝对干净时关得掉。

use crate::draft_page::{DraftPage, PreviewAnchor};
use crate::export;
use crate::models::NumberingConfig;
use crate::theme;
use eframe::egui;
use std::ops::Range;

/// 刻度带宽度。它浮在纸张右侧的留白上，让开滚动条。
const RAIL_WIDTH: f32 = 14.0;
/// 标题列连同刻度带一共占多宽。也是"靠近右缘"的判定宽度——
/// 热区必须把标题列圈进去，否则指针往左挪到标题上就判成离开，整列当场缩回去。
const COLUMN_WIDTH: f32 = 300.0;
/// 标题与刻度带之间的空隙，以及每行文字上下留白。
const LABEL_GAP: f32 = 10.0;
const LABEL_PAD_X: f32 = 12.0;
const LABEL_ROW_PAD: f32 = 7.0;
/// 焦点往外各铺几条。再多就挤，而且离得远的本来也看不清。
const FOCUS_RANKS: usize = 4;
/// 按名次递减的字号与不透明度：正中最大最实，往外逐档化进纸里。
const RANK_FONT: [f32; FOCUS_RANKS + 1] = [15.5, 13.0, 11.5, 10.5, 10.0];
const RANK_ALPHA: [f32; FOCUS_RANKS + 1] = [1.0, 0.72, 0.45, 0.26, 0.13];
/// 整列淡入淡出的时长，以及焦点换条时锚点滑过去的时长。
const REVEAL_TIME: f32 = 0.14;
const ANCHOR_GLIDE: f32 = 0.09;
/// 底衬最实处的不透明度，以及它比文字向外多铺出去的余量。
const SCRIM_ALPHA: f32 = 0.82;
const SCRIM_BLEED: f32 = 16.0;

/// 导航里的一条标题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavEntry {
    /// 公文层级：1 是正式标题（文档标题或附件标题），2 起是「一、」「（一）」……
    pub(crate) level: u8,
    /// 自动编号前缀。正式标题没有编号。
    pub(crate) number: Option<String>,
    /// 去掉 `#` 之后的标题文字。带编号的标题还会清掉人工写入的旧编号，
    /// 正式标题不清——与解析器和版式预览的处理一致。
    pub(crate) text: String,
    /// 标题行在源码中的字节范围。跳转、回查版面位置都用它。
    pub(crate) line: Range<usize>,
}

/// 公文预览滚动区上一帧的量度。导航靠它把版面位置换算成刻度条上的位置。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct PreviewScroll {
    pub(crate) offset_y: f32,
    pub(crate) content_height: f32,
    pub(crate) viewport_top: f32,
    pub(crate) viewport_height: f32,
}

/// 扫一遍源码，取出全部标题。
///
/// 编号由 [`export::HeadingCounters`] 推进，因此区段标记 `<!--附件-->` 处的计数器
/// 重置、附件区的独立编号、旧格式 `# 附件1` 的降级，全都与导出和预览保持一致。
pub(crate) fn collect_entries(markdown: &str, numbering: &NumberingConfig) -> Vec<NavEntry> {
    let mut counters = export::HeadingCounters::with_numbering(*numbering);
    let mut entries = Vec::new();
    for (offset, line) in export::source_lines(markdown) {
        let number = counters.next(line);
        let line_range = offset..offset + line.len();
        // 层级取折算后的值：旧格式附件里 `#` 的个数比实际层级多一层，
        // 直接数井号会把附件里的「一、」画得比正文的「一、」深一层。
        let level = match (&number, counters.numbered_level()) {
            (Some(_), Some(level)) => level,
            // 正式标题（文档标题、附件标题）不排编号，但要作为根节点列出来。
            _ if counters.centered_title() => 1,
            _ => continue,
        };
        let raw = line.trim_start().trim_start_matches('#').trim();
        // 只有带编号的标题才清洗人工编号——解析器对正式标题（`#`）也不清洗，
        // 导航要和版式预览逐字一致。清洗规则会吃掉「一、」这样的开头，
        // 对标题一视同仁地跑一遍，反而可能把标题本身的字去掉。
        let text = match level {
            0 | 1 => raw.to_string(),
            _ => export::clean_heading_number(raw),
        };
        entries.push(NavEntry {
            level,
            number,
            text,
            line: line_range,
        });
    }
    entries
}

/// 回查某个标题这一帧被画在屏幕的什么高度。
///
/// 不改预览内部：`preview::layout` 给每个可点块注册的 widget id 本来就由源码范围
/// 决定，`read_response` 先查本帧、再回落上一帧，所以导航画在预览之后就能拿到
/// 当帧的真实矩形。红头呈批件走独立的打印分页器，id 里带页码和片段序号，
/// 没法按源码范围回查——那一种由调用方退化成按字节比例排布。
pub(crate) fn heading_top(ctx: &egui::Context, line: &Range<usize>) -> Option<f32> {
    // 普通块：preview::layout::clickable。
    let block = egui::Id::new(("gw-preview-block", line.start, line.end));
    // 节内紧缩：标题与随后的正文合成一段，标题占第 0 行的可点区。
    let compact = egui::Id::new(("gw-preview-source-line", line.start, line.end, 0usize));
    ctx.read_response(block)
        .or_else(|| ctx.read_response(compact))
        .map(|response| response.rect.top())
}

/// 一条标题在全文中的纵向位置，0 是文首、1 是文末。
struct Placed<'a> {
    entry: &'a NavEntry,
    fraction: f32,
    /// 在滚动内容坐标系里的高度。`None` 表示这一帧没量到（红头呈批件）。
    content_y: Option<f32>,
}

/// 把标题排到 0~1 的相对位置上。
///
/// 能量到真实版面位置就按真实位置排——这样刻度的疏密就是各节的真实长短。
/// 量不到（红头呈批件的打印分页器）退化成按源码字节比例排：公文以文字段落为主，
/// 字节数与版面高度大体成正比，用作兜底够用，但表格和图片多的稿件会有偏差。
fn place<'a>(
    ctx: &egui::Context,
    entries: &'a [NavEntry],
    scroll: PreviewScroll,
    markdown_len: usize,
) -> Vec<Placed<'a>> {
    let tops = entries
        .iter()
        .map(|entry| heading_top(ctx, &entry.line))
        .collect::<Vec<_>>();
    let measured = tops.iter().filter(|top| top.is_some()).count();
    let usable = scroll.content_height > 1.0 && measured * 2 >= entries.len().max(1);
    entries
        .iter()
        .zip(tops)
        .map(|(entry, top)| {
            let content_y = top.map(|top| top - scroll.viewport_top + scroll.offset_y);
            let fraction = match (usable, content_y) {
                (true, Some(y)) => (y / scroll.content_height).clamp(0.0, 1.0),
                _ => match markdown_len {
                    0 => 0.0,
                    len => (entry.line.start as f32 / len as f32).clamp(0.0, 1.0),
                },
            };
            Placed {
                entry,
                fraction,
                content_y: usable.then_some(content_y).flatten(),
            }
        })
        .collect()
}

/// 当前读到第几条。
///
/// 量到了真实版面位置就只按滚动位置判：此时"没有当前节"是个有意义的答案——
/// 停在第一个标题之前（红头、标题、主送那一段）本来就不属于任何一节，
/// 这时候回落到光标锚点会把一个远在下方的标题标成"当前"。
/// 只有整篇都量不到位置（红头呈批件走独立分页器）才回落到光标/点选的锚点。
fn current_index(
    placed: &[Placed<'_>],
    scroll: PreviewScroll,
    anchor_line: Option<usize>,
) -> Option<usize> {
    if placed.iter().any(|item| item.content_y.is_some()) {
        // 视口顶端往下四分之一处算"正在读"的位置：整节刚滚过顶边就换到下一节，
        // 会让标题还在屏幕上时刻度已经跳走。
        let reading = scroll.offset_y + scroll.viewport_height * 0.25;
        return placed
            .iter()
            .enumerate()
            .filter(|(_, item)| item.content_y.is_some_and(|y| y <= reading))
            .map(|(index, _)| index)
            .next_back();
    }
    let line = anchor_line?;
    placed
        .iter()
        .enumerate()
        .filter(|(_, item)| item.entry.line.start <= line)
        .map(|(index, _)| index)
        .next_back()
}

/// 刻度长度按层级递减：一级最长，往下每层短一截，一眼能看出层级而不必读字。
fn tick_length(level: u8) -> f32 {
    match level {
        0 | 1 => 12.0,
        2 => 11.0,
        3 => 8.0,
        4 => 5.5,
        _ => 4.0,
    }
}

impl DraftPage<'_> {
    /// 在 `region` 的右缘画导航刻度。必须在版式预览画完之后调用——
    /// 标题的屏幕位置是回查预览本帧注册的 widget 得来的。
    pub(crate) fn navigator_overlay(&mut self, ui: &mut egui::Ui, region: egui::Rect) {
        if !self.config.show_preview_navigator {
            return;
        }
        let entries = collect_entries(&self.doc.generated_markdown, &self.config.numbering);
        // 只有一个文档标题不值得画刻度：全文就一处，还在最顶上。
        if !entries.iter().any(|entry| entry.level >= 2) {
            return;
        }
        let ctx = ui.ctx().clone();
        let scroll = self.doc.preview_scroll;
        let placed = place(&ctx, &entries, scroll, self.doc.generated_markdown.len());
        let anchor_line = self
            .doc
            .preview_anchor
            .as_ref()
            .map(|anchor| anchor.range.start)
            .or(self.doc.preview_cursor_line);
        let current = current_index(&placed, scroll, anchor_line);

        let rail = rail_rect(region, ui.spacing().scroll.bar_width);
        if rail.height() < 40.0 {
            return;
        }

        // 刻度带留在预览这一层里，不另开前景层。
        //
        // 独立层会把滚轮一起吞掉：egui 的滚动区只在"自己是指针下最上面那一层"时
        // 才收滚轮，指针一停在刻度带上，预览就滚不动了——而扫完标题顺手滚页
        // 恰恰是最自然的动作。同层则不然：导航画在预览之后，注册得更晚，
        // 点击照样归它，滚轮仍旧落到滚动区。
        let strip = ui.interact(
            rail,
            egui::Id::new("gw_nav_rail_strip"),
            egui::Sense::click(),
        );

        // 靠近右缘才淡入。热区要把标题列一起圈进去，否则指针一往左挪到标题上
        // 就判定为"离开"，整列当场缩回去。
        let anim_id = ui.id().with("nav_reveal");
        let hot = egui::Rect::from_min_max(
            egui::pos2(rail.right() - COLUMN_WIDTH, region.top()),
            egui::pos2(region.right(), region.bottom()),
        );
        let pointer = ctx.pointer_latest_pos().filter(|pos| region.contains(*pos));
        let near = pointer.is_some_and(|pos| hot.contains(pos));
        let reveal = ctx.animate_bool_with_time(anim_id, near, REVEAL_TIME);

        // 指针最近的那条就是焦点。刻度只有一两个点粗，要求精确压线等于要求绣花，
        // 所以按纵坐标就近认。
        let focus = pointer.and_then(|pos| nearest_tick(&placed, rail, pos.y));
        let rows = (reveal > 0.01)
            .then(|| focus.map(|focus| label_rows(&ctx, ui, rail, &placed, focus, reveal)))
            .flatten()
            .unwrap_or_default();

        if !rows.is_empty() {
            paint_label_column(ui, region, rail, &rows, reveal);
        }
        paint_rail(ui, rail, &placed, current, focus);
        if focus.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // 点标题就跳到那一条；点刻度带跳到焦点那一条。
        // 只有标题文字本身可点，行与行之间的空档仍然穿透到正文——
        // 整列都吃掉点击的话，纸面右侧那一条就再也选不中字了。
        let mut jump = None;
        for row in &rows {
            let hit = ui.interact(
                row.rect,
                egui::Id::new(("gw_nav_label", row.index)),
                egui::Sense::click(),
            );
            if hit.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if hit.clicked() {
                jump = Some(placed[row.index].entry.line.clone());
            }
        }
        if jump.is_none()
            && strip.clicked()
            && let Some(index) = focus
        {
            jump = Some(placed[index].entry.line.clone());
        }

        if let Some(line) = jump {
            self.jump_to_heading(line);
            ctx.request_repaint();
        }
    }

    /// 点中一条标题：源码把光标挪过去，版式预览滚到那一块并标亮。
    /// 两边都设，五种显示方式里只要开着的那一种就能就位。
    fn jump_to_heading(&mut self, line: Range<usize>) {
        self.doc.pending_source_selection = None;
        self.doc.pending_source_jump = Some(line.start);
        self.doc.preview_cursor_line = Some(line.start);
        self.doc.preview_anchor =
            self.doc
                .generated_markdown
                .get(line.clone())
                .map(|text| PreviewAnchor {
                    range: line,
                    text: text.to_owned(),
                });
        self.doc.pending_render_jump = true;
    }
}

/// 刻度带的位置：贴着预览右缘，但让开滚动条。
///
/// 鼠标往右去多半是要拖滚动条，把刻度压在滚动条上会变成高频误触，
/// 所以整条带子挪到滚动条内侧，落在纸张右侧的留白上。
pub(crate) fn rail_rect(region: egui::Rect, bar_width: f32) -> egui::Rect {
    let right = region.right() - bar_width - 4.0;
    egui::Rect::from_min_max(
        egui::pos2(right - RAIL_WIDTH, region.top() + 12.0),
        egui::pos2(right, region.bottom() - 12.0),
    )
}

/// 离指针最近的那条刻度。文档标题不参与——它不在刻度里。
fn nearest_tick(placed: &[Placed<'_>], rail: egui::Rect, y: f32) -> Option<usize> {
    placed
        .iter()
        .enumerate()
        .filter(|(_, item)| item.entry.level >= 2)
        .map(|(index, item)| (index, (tick_y(rail, item) - y).abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

fn tick_y(rail: egui::Rect, item: &Placed<'_>) -> f32 {
    rail.top() + rail.height() * item.fraction
}

/// 标题列里的一行。
struct LabelRow {
    /// 在 `placed` 里的下标。
    index: usize,
    /// 文字的包围盒。只有这块可点，行间空档留给正文。
    rect: egui::Rect,
    galley: std::sync::Arc<egui::Galley>,
    color: egui::Color32,
}

/// 按"名次"铺开焦点附近的标题。
///
/// 为什么按名次而不按像素距离：刻度是按版面真实位置排的，长稿里彼此只隔十几个点，
/// 若让标题各自贴着自己的刻度画，行与行立刻叠在一起，越放大叠得越死。改成以焦点
/// 为中心、按各行自己的高度依次向上下堆叠——这正是 Dock 放大时把邻居顶开的做法，
/// 既不会重叠，也自然形成"隆起"的包络。焦点那一行仍然钉在它自己的刻度上，
/// 所以"标题跟刻度在一起"这件事在看的人真正关心的那一条上是成立的。
fn label_rows(
    ctx: &egui::Context,
    ui: &egui::Ui,
    rail: egui::Rect,
    placed: &[Placed<'_>],
    focus: usize,
    reveal: f32,
) -> Vec<LabelRow> {
    // 只有进了刻度的那些条目参与排名，文档标题不算。
    let ticks = placed
        .iter()
        .enumerate()
        .filter(|(_, item)| item.entry.level >= 2)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let Some(focus_rank) = ticks.iter().position(|index| *index == focus) else {
        return Vec::new();
    };

    // 焦点在刻度间跳动时，整列跟着硬切会很跳；把锚点插值一下，列就是滑过去的。
    let target = tick_y(rail, &placed[focus]);
    let anchor = ctx.animate_value_with_time(egui::Id::new("gw_nav_anchor"), target, ANCHOR_GLIDE);

    let right = rail.left() - LABEL_GAP;
    let max_width = COLUMN_WIDTH - RAIL_WIDTH - LABEL_GAP - LABEL_PAD_X;
    let mut rows = Vec::new();
    // 先焦点，再依次向上、向下堆叠，各自用自己的行高推进。
    let mut up_edge = anchor;
    let mut down_edge = anchor;
    for offset in 0..=FOCUS_RANKS as isize {
        for direction in [-1isize, 1] {
            if offset == 0 && direction == 1 {
                continue;
            }
            let rank = focus_rank as isize + offset * direction;
            if rank < 0 || rank as usize >= ticks.len() {
                continue;
            }
            let index = ticks[rank as usize];
            let step = offset as usize;
            let size = RANK_FONT[step];
            let alpha = RANK_ALPHA[step] * reveal;
            let Some(galley) = label_galley(ui, placed[index].entry, size, max_width) else {
                continue;
            };
            let height = galley.size().y + LABEL_ROW_PAD;
            let center = if offset == 0 {
                up_edge = anchor - height * 0.5;
                down_edge = anchor + height * 0.5;
                anchor
            } else if direction < 0 {
                up_edge -= height * 0.5;
                let center = up_edge;
                up_edge -= height * 0.5;
                center
            } else {
                down_edge += height * 0.5;
                let center = down_edge;
                down_edge += height * 0.5;
                center
            };
            let rect = egui::Rect::from_min_max(
                egui::pos2(right - galley.size().x, center - galley.size().y * 0.5),
                egui::pos2(right, center + galley.size().y * 0.5),
            );
            // 焦点用正文色，外圈越远越淡，融进纸里。
            let base = if offset == 0 {
                theme::text()
            } else {
                theme::text_soft()
            };
            rows.push(LabelRow {
                index,
                rect,
                galley,
                color: base.gamma_multiply(alpha),
            });
        }
    }
    rows
}

fn label_galley(
    ui: &egui::Ui,
    entry: &NavEntry,
    size: f32,
    max_width: f32,
) -> Option<std::sync::Arc<egui::Galley>> {
    let text = match &entry.number {
        Some(number) => format!("{number}{}", entry.text),
        None => entry.text.clone(),
    };
    if text.trim().is_empty() {
        return None;
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        text,
        egui::FontId::proportional(size),
        theme::text(),
    );
    // 超过一行就截断。公文标题动辄二十几字，整条铺出去会横穿版面。
    job.wrap.max_width = max_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.halign = egui::Align::LEFT;
    Some(ui.painter().layout_job(job))
}

/// 标题列：先铺一层从右往左化开的底衬，再写字。
fn paint_label_column(
    ui: &egui::Ui,
    region: egui::Rect,
    rail: egui::Rect,
    rows: &[LabelRow],
    reveal: f32,
) {
    let painter = ui.painter().with_clip_rect(region);
    let mut bounds = rows[0].rect;
    for row in &rows[1..] {
        bounds = bounds.union(row.rect);
    }
    let scrim = egui::Rect::from_min_max(
        egui::pos2(bounds.left() - SCRIM_BLEED, bounds.top() - SCRIM_BLEED),
        egui::pos2(rail.left() + RAIL_WIDTH, bounds.bottom() + SCRIM_BLEED),
    )
    .intersect(region);
    paint_scrim(&painter, scrim, reveal);
    for row in rows {
        painter.galley(row.rect.left_top(), row.galley.clone(), row.color);
    }
}

/// 从右往左化开的底衬。没有边框，也没有边界——右侧最实，越往左越透，
/// 上下两端同样收掉，所以标题看着像浮在纸上，而不是压在一块板子上。
///
/// 这不是真正的毛玻璃：egui 的渲染管线取不到已经画好的画面去做高斯模糊，
/// 硬做要自己加一道离屏渲染。这里用的是渐变的半透明薄色——底下的字透得出来
/// 但不会糊，在浅色纸面上观感接近，要真模糊得换渲染方案。
fn paint_scrim(painter: &egui::Painter, rect: egui::Rect, reveal: f32) {
    if !rect.is_positive() {
        return;
    }
    const COLS: usize = 10;
    const ROWS: usize = 12;
    let fill = theme::surface();
    let mut mesh = egui::epaint::Mesh::default();
    for row in 0..=ROWS {
        let v = row as f32 / ROWS as f32;
        let y = rect.top() + rect.height() * v;
        // 上下两端收掉：0 和 1 处为 0，中段为 1，两头各占约两成做过渡。
        let vertical = ((v / 0.2).min(1.0)).min(((1.0 - v) / 0.2).min(1.0));
        for col in 0..=COLS {
            let u = col as f32 / COLS as f32;
            let x = rect.left() + rect.width() * u;
            // 右端最实、左端全透，中间按三次方渐隐，收得比线性更柔和。
            let horizontal = u * u * u;
            let alpha = SCRIM_ALPHA * horizontal * vertical * reveal;
            mesh.colored_vertex(egui::pos2(x, y), fill.gamma_multiply(alpha));
        }
    }
    let stride = (COLS + 1) as u32;
    for row in 0..ROWS as u32 {
        for col in 0..COLS as u32 {
            let top_left = row * stride + col;
            mesh.add_triangle(top_left, top_left + 1, top_left + stride);
            mesh.add_triangle(top_left + 1, top_left + stride + 1, top_left + stride);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// 常驻刻度。当前所在那条用主色描粗，指针就近吸附到的那条再加一档。
fn paint_rail(
    ui: &egui::Ui,
    rail: egui::Rect,
    placed: &[Placed<'_>],
    current: Option<usize>,
    focus: Option<usize>,
) {
    let painter = ui.painter().with_clip_rect(rail.expand(4.0));
    let base = theme::text_muted().gamma_multiply(0.55);
    let accent = theme::accent();
    for (index, item) in placed.iter().enumerate() {
        // 文档标题不进刻度：全文只有一处，而且就在最顶上，占一条刻度纯属浪费。
        if item.entry.level < 2 {
            continue;
        }
        let y = tick_y(rail, item);
        let is_focus = focus == Some(index);
        let is_current = current == Some(index);
        // 焦点那条整条拉满刻度带宽度，"我正指着它"一眼可见，不必去比粗细。
        let length = if is_focus {
            RAIL_WIDTH
        } else {
            tick_length(item.entry.level)
        };
        let (width, color) = match (is_focus, is_current) {
            (true, _) => (2.5, accent),
            (false, true) => (2.0, accent),
            (false, false) => (1.0, base),
        };
        painter.line_segment(
            [
                egui::pos2(rail.right() - length, y),
                egui::pos2(rail.right(), y),
            ],
            egui::Stroke::new(width, color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(markdown: &str) -> Vec<NavEntry> {
        collect_entries(markdown, &NumberingConfig::default())
    }

    fn shape(markdown: &str) -> Vec<(u8, Option<String>, String)> {
        entries(markdown)
            .into_iter()
            .map(|entry| (entry.level, entry.number, entry.text))
            .collect()
    }

    #[test]
    fn numbering_matches_the_exporter_for_every_level() {
        // 导航里的编号必须与导出、版式预览逐字一致：写「三、」而排出「四、」
        // 是最难查的一类 bug，这条测试把三方钉在同一套计数器上。
        let markdown = "# 关于加强某项工作的通知\n\n## 总体要求\n\n### 指导思想\n\n#### 基本原则\n\n##### 具体条款\n";
        assert_eq!(
            shape(markdown),
            vec![
                (1, None, "关于加强某项工作的通知".to_string()),
                (2, Some("一、".into()), "总体要求".to_string()),
                (3, Some("（一）".into()), "指导思想".to_string()),
                (4, Some("1.".into()), "基本原则".to_string()),
                (5, Some("(1)".into()), "具体条款".to_string()),
            ]
        );
    }

    #[test]
    fn sibling_headings_count_up_and_deeper_levels_reset() {
        let markdown = "## 甲\n\n### 甲一\n\n### 甲二\n\n## 乙\n\n### 乙一\n";
        let numbers = entries(markdown)
            .into_iter()
            .filter_map(|entry| entry.number)
            .collect::<Vec<_>>();
        // 换到「二、」之后，下一级要从「（一）」重新数，不能接着「（三）」。
        assert_eq!(numbers, vec!["一、", "（一）", "（二）", "二、", "（一）"]);
    }

    #[test]
    fn attachment_marker_restarts_the_counters() {
        let markdown =
            "# 正文标题\n\n## 正文一节\n\n<!--附件-->\n\n# 附件正式标题\n\n## 附件一节\n";
        assert_eq!(
            shape(markdown),
            vec![
                (1, None, "正文标题".to_string()),
                (2, Some("一、".into()), "正文一节".to_string()),
                (1, None, "附件正式标题".to_string()),
                // 附件区自成一套编号，不接着正文往下数。
                (2, Some("一、".into()), "附件一节".to_string()),
            ]
        );
    }

    #[test]
    fn legacy_attachment_headings_fold_back_one_level() {
        // 旧格式里附件区的 `#` 比实际层级多一层；导航缩进要按折算后的层级算，
        // 否则附件里的「一、」会画得比正文的「一、」深一层。
        let markdown = "<!--附件-->\n\n# 附件1\n\n## 附件正式标题\n\n### 附件一节\n";
        assert_eq!(
            shape(markdown),
            vec![
                (1, None, "附件正式标题".to_string()),
                (2, Some("一、".into()), "附件一节".to_string()),
            ]
        );
    }

    #[test]
    fn hand_written_numbers_are_stripped_from_the_label() {
        // 人手或模型写进标题的旧编号由导出器统一清掉，导航显示的也必须是净标题，
        // 否则会出现「一、一、总体要求」。
        let markdown = "## 一、总体要求\n\n### （一）指导思想\n";
        assert_eq!(
            shape(markdown),
            vec![
                (2, Some("一、".into()), "总体要求".to_string()),
                (3, Some("（一）".into()), "指导思想".to_string()),
            ]
        );
    }

    #[test]
    fn formal_titles_keep_their_own_wording() {
        // 解析器对正式标题不清洗编号，导航也不能洗：「第一部分」正好落在
        // 清洗规则的射程里，一视同仁地跑一遍会把标题开头吃掉。
        let markdown = "<!--附件-->\n\n# 第一部分 工作要求\n\n## 责任分工\n";
        assert_eq!(
            shape(markdown),
            vec![
                (1, None, "第一部分 工作要求".to_string()),
                (2, Some("一、".into()), "责任分工".to_string()),
            ]
        );
    }

    #[test]
    fn heading_line_range_points_at_the_heading_itself() {
        // 跳转与版面位置回查都按这个范围走：它必须正好是标题那一行，
        // 且与 preview 给块注册 widget id 时用的范围逐字节相同。
        let markdown = "正文段落。\n\n## 总体要求\n\n后续正文。\n";
        let entry = entries(markdown).into_iter().next().expect("有一条标题");
        assert_eq!(&markdown[entry.line.clone()], "## 总体要求");
    }

    #[test]
    fn a_document_without_headings_yields_nothing_to_navigate() {
        assert!(entries("只有正文，没有任何标题。\n").is_empty());
    }
}
