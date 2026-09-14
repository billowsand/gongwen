//! 公文预览右缘的导航刻度：常驻一列细刻度反映全文结构，靠近右缘则从右缘推出
//! 一块亚克力板，把全篇标题排成一份大纲。
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
//! 为什么是悬浮的板而不是常驻的大纲栏。栏一开就常驻地吃掉一块宽度，而这里的板
//! 只在指针靠过来的那一瞬间从右缘滑出，手一挪就收回去，版面不留欠账。
//!
//! 板上是一份**固定**的大纲：字号只跟层级走，不跟指针走。曾经做过 Dock 那样按
//! 指针远近放大的版本——指针沿刻度带划过去，整列的字忽大忽小，看着晃眼，而且
//! 被放大的那条未必是要找的那条，眼睛还得在变动的字号里重新定位。大纲是用来
//! 照着往下读的，读的东西不该在读的时候动。指针指到哪一条，由那一条的底衬高亮
//! 说明，不必动字号。
//!
//! 底衬是**一整块**亚克力板，不是每条标题各带一条底衬。分条的底衬每行各有各的
//! 左缘，长短不一的标题排下来，左边就是一排参差的舌头，越往外越碎；一块定宽的
//! 板则只有一条干净的左缘，标题落在板上，看着才是一件东西。板宽定死不随最长的
//! 标题走——宽度跟着内容变，焦点在长短标题之间跳一下，左缘就跟着晃。
//!
//! 说明白一点：板不是真正的毛玻璃。egui 的渲染管线取不到已经画好的画面去做
//! 高斯模糊，所以这里是「不透明底色 + 颗粒 + 投影」，靠挡而不是靠糊——详见
//! [`paint_panel`]。
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
use crate::preview;
use crate::theme;
use eframe::egui;
use std::ops::Range;

/// 刻度带宽度。它浮在纸张右侧的留白上，让开滚动条。
const RAIL_WIDTH: f32 = 14.0;
/// "靠近右缘"的判定宽度。热区必须把整块板圈进去并且还宽出一截，否则指针
/// 往左挪到板上就判成离开，板当场缩回去。
const COLUMN_WIDTH: f32 = 320.0;
/// 亚克力板的宽度，连刻度带一起算在内。定宽，不随标题长短走。
const PANEL_WIDTH: f32 = 288.0;
/// 板内文字四周的留白，以及板的圆角。
const PANEL_PAD_X: f32 = 14.0;
const PANEL_PAD_Y: f32 = 9.0;
const PANEL_RADIUS: u8 = 10;
/// 板面的不透明度。留一丝透，底下纸面的色调还能渗上来一点——全不透就是一块
/// 挡板，不是亚克力。
const PANEL_ALPHA: f32 = 0.94;
/// 标题与刻度带之间的空隙，以及每行文字上下留白。
const LABEL_GAP: f32 = 10.0;
const LABEL_ROW_PAD: f32 = 7.0;
/// 层级缩进：最浅那层贴着板的左缘，往下每层缩一档，板面读起来才是一份大纲。
/// 缩到三档为止，再深就把本来就不长的可用宽度吃光了。
const LEVEL_INDENT: f32 = 13.0;
const LEVEL_INDENT_MAX: u8 = 3;
/// 各层级固定的字号：文档标题、二级、三级、更深。只跟层级走，不跟指针走。
/// 层级之间只差一档——层级本来已经由字体（黑体／楷体／仿宋）和缩进标出来了，
/// 字号再拉开差距，一份十几行的大纲就会显得七零八落。
const LEVEL_FONT: [f32; 4] = [14.5, 14.0, 13.0, 12.0];
/// 整块板淡入淡出的时长，淡入时从右缘滑出来的距离，以及大纲太长要滚动时
/// 滑过去的时长。滑一小段，板才是"推出来"的，不是凭空浮现的。
const REVEAL_TIME: f32 = 0.14;
const REVEAL_SLIDE: f32 = 16.0;
const SCROLL_GLIDE: f32 = 0.12;
/// 颗粒的浓度、贴图边长，以及四边收掉的宽度（板是圆角的，方网格不收边会在
/// 四个角上漏出板外的噪点）。
const GRAIN_ALPHA: f32 = 0.05;
const GRAIN_TILE: usize = 64;
const GRAIN_EDGE: f32 = 6.0;

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

        // 靠近右缘才淡入。热区要把整块板圈进去并且宽出一截，否则指针一往左挪到
        // 板上就判定为"离开"，板当场缩回去。
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
        // 大纲的内容与指针无关，所以只看淡入的进度：指针移出预览区时 `focus`
        // 会立刻变回 `None`，若拿它当条件，板就不是淡出而是当场消失。
        let rows = if reveal > 0.01 {
            outline_rows(&ctx, ui, rail, &placed, current, focus, reveal)
        } else {
            Vec::new()
        };

        // 点标题就跳到那一条；点刻度带跳到指针就近认到的那一条。
        //
        // 可点的是整条横带，不只是文字那几个字：板是不透明的，板上任何一处底下的
        // 正文本来就已经看不见也选不中，这时候还只让文字可点，只会让人点在两行
        // 中间却什么都没发生。分条底衬时行与行之间要留给正文穿透，板铺开之后
        // 这个理由不再成立。
        //
        // 横带到刻度带左缘为止，右边那一条仍旧归刻度。两者答的是不同的问题：
        // 大纲问"第几条"，刻度问"文中什么位置"——长稿里同一个纵坐标上，
        // 大纲的第 N 行与刻度的第 N 条并不是同一条。让横带盖住刻度，就会出现
        // 指着这条刻度、跳到另一条标题。
        let mut jump = None;
        if !rows.is_empty() {
            let spans = rows.iter().map(|row| row.rect).collect::<Vec<_>>();
            let panel = panel_rect(rail, reveal, &spans);
            let bands = row_bands(&spans, panel.with_max_x(rail.left()));
            let mut hovered = None;
            for (position, (row, band)) in rows.iter().zip(&bands).enumerate() {
                let hit = ui.interact(
                    *band,
                    egui::Id::new(("gw_nav_label", row.index)),
                    egui::Sense::click(),
                );
                if hit.hovered() {
                    hovered = Some(position);
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if hit.clicked() {
                    jump = Some(placed[row.index].entry.line.clone());
                }
            }
            paint_panel_labels(ui, region, panel, &rows, &bands, hovered, reveal);
        }
        // 刻度画在板之后：刻度带落在板的右缘上，两者是一件东西，刻度得压在板面上。
        paint_rail(ui, rail, &placed, current, focus);
        if focus.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
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

/// 板上的一行。
struct LabelRow {
    /// 在 `placed` 里的下标。
    index: usize,
    /// 文字的包围盒。可点的范围由 [`row_bands`] 从它推出来，比它宽。
    rect: egui::Rect,
    galley: std::sync::Arc<egui::Galley>,
    color: egui::Color32,
}

/// 把全篇标题按源码顺序排成一份大纲。
///
/// 行距、字号、左缘都只跟层级走，与指针位置无关：这一列是拿来照着读的，
/// 读的时候不该动。指针指到哪一条由底衬的高亮说明，不动字号。
///
/// 标题不各自贴着自己的刻度画——刻度按版面真实位置排，长稿里彼此只隔十几个点，
/// 贴着画行与行立刻叠成一团。大纲照源码顺序等距往下排，位置的事交给刻度说。
fn outline_rows(
    ctx: &egui::Context,
    ui: &egui::Ui,
    rail: egui::Rect,
    placed: &[Placed<'_>],
    current: Option<usize>,
    focus: Option<usize>,
    reveal: f32,
) -> Vec<LabelRow> {
    let (text_left, text_right) = text_span(rail, reveal);
    let mut measured = Vec::with_capacity(placed.len());
    for (index, item) in placed.iter().enumerate() {
        let left = text_left + level_indent(item.entry.level);
        let size = label_size(item.entry.level);
        let Some(galley) = label_galley(ui, item.entry, size, text_right - left) else {
            continue;
        };
        let height = galley.size().y + LABEL_ROW_PAD;
        measured.push((index, left, galley, height));
    }
    if measured.is_empty() {
        return Vec::new();
    }

    let heights = measured
        .iter()
        .map(|(index, _, _, height)| (*index, *height))
        .collect::<Vec<_>>();
    let top = outline_top(rail, &heights, focus.or(current));
    // 滚动要插值：大纲一整列瞬移比字号忽大忽小更晃眼。装得下的稿子这里恒等。
    let top = ctx.animate_value_with_time(egui::Id::new("gw_nav_outline_top"), top, SCROLL_GLIDE);

    let mut rows = Vec::with_capacity(measured.len());
    let mut y = top;
    for (index, left, galley, height) in measured {
        let text_top = y + (height - galley.size().y) * 0.5;
        y += height;
        // 排不进刻度带那一段的行整条不画：板只有这么高，让半截字切在板沿上，
        // 比少列一条难看得多。
        if text_top < rail.top() || text_top + galley.size().y > rail.bottom() {
            continue;
        }
        // 左对齐。分条底衬时标题是贴着刻度右对齐的——那时候右边是唯一一条
        // 直的边。板铺开之后左缘成了那条直边，字再右对齐就是在一块方正的板上
        // 排出一排参差的左缘，看着像没排齐。
        let rect = egui::Rect::from_min_max(
            egui::pos2(left, text_top),
            egui::pos2(left + galley.size().x, text_top + galley.size().y),
        );
        // 正在读的那一条用主色标出来——这是整个导航最该一直在线的一条信息。
        // 其余的按层级分浓淡：深层多、浅层少，深的淡一点，整列才有层次。
        let base = match (current == Some(index), placed[index].entry.level) {
            (true, _) => theme::accent(),
            (false, 0..=2) => theme::text(),
            (false, _) => theme::text_soft(),
        };
        rows.push(LabelRow {
            index,
            rect,
            galley,
            color: base.gamma_multiply(reveal),
        });
    }
    rows
}

/// 大纲第一行的纵向起点。
///
/// 装得下就整列居中，指针怎么动都一帧不动——这是绝大多数公文的情形。
/// 装不下才滚：把锚点那一条带到刻度带正中，再把整列夹回两端，列到头就停住，
/// 不会在上下留出一截空板。
fn outline_top(rail: egui::Rect, heights: &[(usize, f32)], anchor: Option<usize>) -> f32 {
    let total = heights.iter().map(|(_, height)| *height).sum::<f32>();
    let available = rail.height();
    if total <= available {
        return rail.top() + (available - total) * 0.5;
    }
    let Some(anchor) = anchor else {
        return rail.top();
    };
    let mut center = 0.0;
    for (index, height) in heights {
        if *index == anchor {
            center += height * 0.5;
            break;
        }
        center += height;
    }
    (rail.center().y - center).clamp(rail.bottom() - total, rail.top())
}

/// 每一级固定的字号。
fn label_size(level: u8) -> f32 {
    LEVEL_FONT[usize::from(level.saturating_sub(1).min(3))]
}

/// 层级缩进。公文正文层级最浅是二级（「一、」），所以从二级起算。
fn level_indent(level: u8) -> f32 {
    f32::from(level.saturating_sub(2).min(LEVEL_INDENT_MAX)) * LEVEL_INDENT
}

/// 板上某一层级的标题该用哪支字体。直接取版式预览那一套，不另立一份映射：
/// 两份映射迟早会走岔，而走岔的表现是导航里的「一、」是黑体、纸上却成了楷体。
fn label_family(level: u8) -> egui::FontFamily {
    match level {
        // 文档标题与附件标题在纸上是方正小标宋。预览把它排在独立的标题块里，
        // 不走 `heading_family`，所以这一级要在这里单独对上。
        0 | 1 => theme::official_family(theme::FONT_BIAOSONG),
        _ => theme::official_family(preview::heading_family(level)),
    }
}

/// 一条标题在板上的字面。
///
/// 字体跟着公文走，不用界面默认的那支无衬线：二级黑体、三级楷体、更深的仿宋，
/// 与版式预览、DOCX/LaTeX 导出取自同一个 [`preview::heading_family`]。这列的
/// 是纸上那些标题，字形不一样就等于换了一份东西——而且公文的层级本来就是靠
/// 字体区分的（黑体一级、楷体二级），字形对上，层级不读字也认得出来。
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
        egui::FontId::new(size, label_family(entry.level)),
        theme::text(),
    );
    // 超过一行就截断。公文标题动辄二十几字，整条铺出去会横穿版面。
    job.wrap.max_width = max_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.halign = egui::Align::LEFT;
    Some(ui.painter().layout_job(job))
}

/// 板的横向范围。右缘压住刻度带的外沿——刻度和标题是一件东西，中间断开会显出
/// 两截；左缘由 [`PANEL_WIDTH`] 定死。
///
/// `reveal` 未满时整块往右挪一截：板是从纸的右缘推出来的，不是凭空浮现的。
fn panel_span(rail: egui::Rect, reveal: f32) -> (f32, f32) {
    let right = rail.right() + (1.0 - reveal) * REVEAL_SLIDE;
    (right - PANEL_WIDTH, right)
}

/// 板内文字能占的横向范围：左边留出内边距，右边给刻度带让开。
fn text_span(rail: egui::Rect, reveal: f32) -> (f32, f32) {
    let (left, right) = panel_span(rail, reveal);
    (left + PANEL_PAD_X, right - RAIL_WIDTH - LABEL_GAP)
}

/// 板的位置：横向定宽，纵向把所有行包进去，上下各留一档内边距。
///
/// `rows` 是各行文字的包围盒，非空且已按纵向排好。
fn panel_rect(rail: egui::Rect, reveal: f32, rows: &[egui::Rect]) -> egui::Rect {
    let (left, right) = panel_span(rail, reveal);
    let top = rows[0].top() - PANEL_PAD_Y;
    let bottom = rows[rows.len() - 1].bottom() + PANEL_PAD_Y;
    egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
}

/// 每一行在板上占的那条横带：以相邻两行的中点为界，首尾相接铺满 `area`。
///
/// 它既是可点的范围，也是悬停时高亮的范围。按中点切而不是按文字包围盒扩一圈，
/// 是为了让板面不留死角：指针落在板上任何一处，都明确属于某一条标题。
///
/// `area` 是板去掉刻度带那一条之后的部分——刻度自己收点击。
fn row_bands(rows: &[egui::Rect], area: egui::Rect) -> Vec<egui::Rect> {
    (0..rows.len())
        .map(|position| {
            let top = match position.checked_sub(1) {
                Some(above) => (rows[above].bottom() + rows[position].top()) * 0.5,
                None => area.top(),
            };
            let bottom = match rows.get(position + 1) {
                Some(below) => (rows[position].bottom() + below.top()) * 0.5,
                None => area.bottom(),
            };
            egui::Rect::from_min_max(
                egui::pos2(area.left(), top),
                egui::pos2(area.right(), bottom),
            )
        })
        .collect()
}

/// 板、悬停高亮、标题，从下往上依次画。
fn paint_panel_labels(
    ui: &egui::Ui,
    region: egui::Rect,
    panel: egui::Rect,
    rows: &[LabelRow],
    bands: &[egui::Rect],
    hovered: Option<usize>,
    reveal: f32,
) {
    let painter = ui.painter().with_clip_rect(region);
    paint_panel(&painter, panel, reveal);
    if let Some(position) = hovered.filter(|_| reveal > 0.5)
        && let Some(band) = bands.get(position)
    {
        // 高亮往里收一圈，才不会顶到板的圆角上；上下也收一点，免得与相邻两条
        // 贴成一整片。
        painter.rect_filled(
            band.shrink2(egui::vec2(6.0, 1.5)),
            6,
            theme::surface_hover().gamma_multiply(0.85 * reveal),
        );
    }
    for row in rows {
        painter.galley(row.rect.left_top(), row.galley.clone(), row.color);
    }
}

/// 亚克力板本身：投影 + 底色 + 颗粒 + 一道极淡的描边。
///
/// 这不是真的毛玻璃——egui 取不到已画好的画面做高斯模糊，硬做要自己加一道离屏
/// 渲染。但亚克力真正让人看得清的是那层不透明的底色，模糊只是锦上添花：
///
/// - 底色压到 [`PANEL_ALPHA`]，底下的正文基本退掉，再长的标题也不会被字缝里
///   透上来的笔画搅乱。留那一丝透，是为了让纸面的色调还能渗上来一点。
/// - 颗粒补材质。纯色半透明看着像一张塑料贴纸，撒一层极细的噪点才像一块板。
/// - 投影和描边负责"浮起来"。分条底衬时靠四边化开来回避边界，一块板则相反：
///   它就该有一条清清楚楚的边，边之外靠投影和纸分开。
fn paint_panel(painter: &egui::Painter, rect: egui::Rect, reveal: f32) {
    if !rect.is_positive() {
        return;
    }
    let shadow_alpha = f32::from(theme::paper::shadow_alpha()) * 1.6 * reveal;
    painter.add(
        egui::epaint::Shadow {
            // 往左下偏一点：板是从右边推出来的，光从左上来。
            offset: [-3, 4],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha(shadow_alpha.min(255.0) as u8),
        }
        .as_shape(rect, PANEL_RADIUS),
    );
    painter.rect_filled(
        rect,
        PANEL_RADIUS,
        theme::surface().gamma_multiply(PANEL_ALPHA * reveal),
    );
    paint_grain(painter, rect, reveal);
    painter.rect_stroke(
        rect,
        PANEL_RADIUS,
        egui::Stroke::new(1.0, theme::border().gamma_multiply(0.7 * reveal)),
        egui::StrokeKind::Inside,
    );
}

/// 板面的颗粒：噪声贴图平铺盖满整块板，四边各收掉一小截。
///
/// 收边是因为板是圆角的而这张网格是方的：不收，四个角上会漏出板外的噪点。
fn paint_grain(painter: &egui::Painter, rect: egui::Rect, reveal: f32) {
    let grain = grain_texture(painter.ctx());
    let cols = edge_taper(rect.left(), rect.right());
    let rows = edge_taper(rect.top(), rect.bottom());
    painter.add(egui::Shape::mesh(grid_mesh(
        &cols,
        &rows,
        egui::Color32::WHITE.gamma_multiply(GRAIN_ALPHA * reveal),
        Some((grain.id(), GRAIN_TILE as f32)),
    )));
}

/// 一个方向上的采样柱：两端各在 [`GRAIN_EDGE`] 的宽度里收成透明，中间满强度。
fn edge_taper(start: f32, end: f32) -> Vec<(f32, f32)> {
    // 板窄到装不下两道收边时按比例缩，采样柱才不会前后颠倒、把网格翻成一块乱片。
    let inset = GRAIN_EDGE.min((end - start) * 0.25).max(0.0);
    vec![
        (start, 0.0),
        (start + inset, 1.0),
        (end - inset, 1.0),
        (end, 0.0),
    ]
}

/// 亚克力的颗粒：一张 64×64 的灰噪声，平铺盖在板上。
///
/// 为什么要它：纯色半透明看着像一层塑料贴纸，加一点极细的颗粒才有「材质」感。
/// 这也是各家毛玻璃材质里唯一一层不依赖背景模糊、能直接照搬过来的东西。
/// 噪声围着中灰上下摆，浅色纸上压、深色纸上提，两套主题都成立。
/// 种子写死，颗粒每帧完全一样——否则整片底衬会像电视雪花一样闪。
fn grain_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("gw_nav_grain");
    if let Some(handle) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return handle;
    }
    let mut rgb = Vec::with_capacity(GRAIN_TILE * GRAIN_TILE * 3);
    let mut state: u32 = 0x9e37_79b9;
    for _ in 0..GRAIN_TILE * GRAIN_TILE {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let value = 64 + (state >> 25) as u8;
        rgb.extend_from_slice(&[value, value, value]);
    }
    let handle = ctx.load_texture(
        "gw_nav_grain",
        egui::ColorImage::from_rgb([GRAIN_TILE, GRAIN_TILE], &rgb),
        egui::TextureOptions {
            // 颗粒要的是硬边的细点，插值一平滑就成了糊开的云。
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            ..Default::default()
        },
    );
    ctx.data_mut(|data| data.insert_temp(id, handle.clone()));
    handle
}

/// 把「横向采样柱 × 纵向采样行」织成一片网格，每个顶点的不透明度是两个方向系数
/// 的乘积。底衬的每一层都是这么画出来的：顶点着色天然就是渐变，不需要边框，
/// 也就没有边界可言。
///
/// `tile` 给定时按屏幕坐标算 uv，贴图平铺；uv 是位置的线性函数，稀疏采样也不失真。
fn grid_mesh(
    cols: &[(f32, f32)],
    rows: &[(f32, f32)],
    color: egui::Color32,
    tile: Option<(egui::TextureId, f32)>,
) -> egui::epaint::Mesh {
    let mut mesh = egui::epaint::Mesh::default();
    if cols.len() < 2 || rows.len() < 2 {
        return mesh;
    }
    if let Some((texture, _)) = tile {
        mesh.texture_id = texture;
    }
    for &(y, vertical) in rows {
        for &(x, horizontal) in cols {
            let tint = color.gamma_multiply(horizontal * vertical);
            match tile {
                Some((_, size)) => mesh.vertices.push(egui::epaint::Vertex {
                    pos: egui::pos2(x, y),
                    uv: egui::pos2(x / size, y / size),
                    color: tint,
                }),
                None => mesh.colored_vertex(egui::pos2(x, y), tint),
            }
        }
    }
    let stride = cols.len() as u32;
    for row in 1..rows.len() as u32 {
        for col in 1..stride {
            let bottom_right = row * stride + col;
            let top_right = bottom_right - stride;
            mesh.add_triangle(top_right - 1, top_right, bottom_right - 1);
            mesh.add_triangle(top_right, bottom_right, bottom_right - 1);
        }
    }
    mesh
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

    fn rail() -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(586.0, 40.0), egui::pos2(600.0, 640.0))
    }

    /// 板上第 `position` 行文字的包围盒，按该层级的缩进与字号。
    fn label(position: usize, level: u8, width: f32) -> egui::Rect {
        let left = text_span(rail(), 1.0).0 + level_indent(level);
        let top = 200.0 + position as f32 * 26.0;
        egui::Rect::from_min_max(
            egui::pos2(left, top),
            egui::pos2(left + width, top + label_size(level)),
        )
    }

    #[test]
    fn no_label_ever_hangs_off_the_panel() {
        // 分条底衬的时代，底衬是贴着每行文字长出来的，长标题只会把自己那条撑宽。
        // 换成一块定宽的板之后，"字不会跑到板外面去"改由排版这一头保证：
        // 文字的可用范围必须整个落在板里，四边都还留着内边距。
        for reveal in [0.0f32, 0.35, 0.8, 1.0] {
            let panel = panel_rect(rail(), reveal, &[label(0, 2, 10.0)]);
            let (left, right) = text_span(rail(), reveal);
            assert!(
                left > panel.left() && right < panel.right(),
                "文字越出板外：{left}..{right} 不在 {panel:?} 里"
            );
            // 最深一层缩进之后仍要装得下十来个字，否则深层标题只剩省略号。
            let usable = right - left - level_indent(2 + LEVEL_INDENT_MAX);
            assert!(
                usable > label_size(2) * 10.0,
                "最深一层可用宽度只剩 {usable}"
            );
        }
        // 热区要比板宽出一截，否则指针挪到板的左缘就判成离开。
        const { assert!(COLUMN_WIDTH > PANEL_WIDTH + 16.0) };
    }

    #[test]
    fn the_bands_tile_the_whole_panel_without_gaps() {
        // 板是不透明的：板上任何一处都必须明确属于某一条标题，否则会出现
        // "点在板上却什么都没发生"，而底下的正文又早被板挡住了。
        let rail = rail();
        let rows = (0..5).map(|i| label(i, 2, 120.0)).collect::<Vec<_>>();
        let panel = panel_rect(rail, 1.0, &rows);
        let area = panel.with_max_x(rail.left());
        let bands = row_bands(&rows, area);
        assert_eq!(bands.len(), rows.len());
        assert_eq!(bands[0].top(), panel.top());
        assert_eq!(bands[bands.len() - 1].bottom(), panel.bottom());
        for (band, row) in bands.iter().zip(&rows) {
            assert!(
                band.contains_rect(*row),
                "{band:?} 没盖住它自己那行 {row:?}"
            );
            assert_eq!(band.left(), panel.left());
            // 刻度带那一条不归横带：那里归刻度，答的是"文中什么位置"。
            assert_eq!(band.right(), rail.left());
        }
        // 首尾相接，既不重叠也不留缝。
        for pair in bands.windows(2) {
            assert_eq!(pair[0].bottom(), pair[1].top());
        }
    }

    #[test]
    fn the_labels_use_the_same_faces_as_the_paper() {
        // 板上的字体必须与纸上逐级对应：公文的层级本来就是靠字体区分的，
        // 导航里「一、」是黑体而纸上排成楷体，等于把层级读错。
        assert_eq!(
            label_family(1),
            theme::official_family(theme::FONT_BIAOSONG)
        );
        assert_eq!(label_family(2), theme::official_family(theme::FONT_HEITI));
        assert_eq!(label_family(3), theme::official_family(theme::FONT_KAITI));
        for deeper in [4, 5] {
            assert_eq!(
                label_family(deeper),
                theme::official_family(theme::FONT_FANGSONG)
            );
        }
        // 任何一级都不许退回界面默认的无衬线。
        for level in 0..=6u8 {
            assert_ne!(label_family(level), egui::FontFamily::Proportional);
        }
    }

    #[test]
    fn the_type_size_depends_on_the_level_alone() {
        // 这条钉住的是"不忽大忽小"：字号只是层级的函数，指针、焦点、当前节
        // 都不得参与。层级之间只差一档，整列才不显得七零八落。
        for level in 0..=8u8 {
            assert_eq!(label_size(level), label_size(level));
        }
        assert_eq!(label_size(0), label_size(1), "文档标题与附件标题同级");
        assert!(label_size(1) >= label_size(2) && label_size(2) > label_size(5));
        assert!(
            LEVEL_FONT.windows(2).all(|pair| pair[0] - pair[1] <= 1.5),
            "相邻层级的字号差不该拉开到一眼就看出两种大小"
        );
        assert!(
            LEVEL_FONT.iter().all(|size| *size >= 12.0),
            "楷体、仿宋再小就看不出字形"
        );
    }

    #[test]
    fn a_short_outline_never_moves_and_a_long_one_scrolls_within_its_ends() {
        let rail = rail();
        // 装得下：整列居中，锚点指哪都是同一个起点——一份十来节的公文属于这一档。
        let short = (0..10).map(|index| (index, 24.0)).collect::<Vec<_>>();
        let centered = outline_top(rail, &short, None);
        for anchor in [None, Some(0), Some(4), Some(9)] {
            assert_eq!(outline_top(rail, &short, anchor), centered);
        }
        assert!(centered > rail.top(), "居中之后上面该留出空当");

        // 装不下：按锚点滚，但整列夹在两端之间，不会在板的上下露出空白。
        let long = (0..60).map(|index| (index, 24.0)).collect::<Vec<_>>();
        let total = 60.0 * 24.0;
        let tops = [0usize, 12, 30, 45, 59]
            .map(|anchor| outline_top(rail, &long, Some(anchor)))
            .to_vec();
        for top in &tops {
            assert!(
                *top <= rail.top() + 0.01 && *top >= rail.bottom() - total - 0.01,
                "大纲滚出了两端：{top}"
            );
        }
        // 锚点越往下，列就越往上滚，中间不允许反向。
        assert!(tops.windows(2).all(|pair| pair[0] >= pair[1]));
        assert!(tops[0] > tops[tops.len() - 1], "首尾两端该滚到不同位置");
    }

    #[test]
    fn the_grain_stops_short_of_the_rounded_corners() {
        // 颗粒是方网格，板是圆角：不收边，四个角上会漏出板外的噪点。
        for span in [(0.0f32, 288.0f32), (10.0, 22.0), (5.0, 5.0)] {
            let taper = edge_taper(span.0, span.1);
            assert!(
                taper.windows(2).all(|pair| pair[0].0 <= pair[1].0),
                "采样柱不单调，网格会翻面：{taper:?}"
            );
            assert!(taper.first().expect("有采样柱").1.abs() < 1e-6);
            assert!(taper.last().expect("有采样柱").1.abs() < 1e-6);
            assert!(taper.iter().all(|(_, factor)| (0.0..=1.0).contains(factor)));
        }
    }

    #[test]
    fn a_document_without_headings_yields_nothing_to_navigate() {
        assert!(entries("只有正文，没有任何标题。\n").is_empty());
    }
}
