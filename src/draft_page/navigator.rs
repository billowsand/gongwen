//! 公文预览右缘的导航刻度：常驻一列细刻度反映全文结构，鼠标靠近展开成标题列表。
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

/// 刻度条宽度。只画刻度，不吃点击——它浮在纸张右侧留白上。
const RAIL_WIDTH: f32 = 14.0;
/// 展开后的标题面板宽度。
const PANEL_WIDTH: f32 = 268.0;
/// 鼠标进入右缘多宽的范围就展开。收起态给得比刻度条宽一些，免得要贴着像素挪。
const HOT_MARGIN: f32 = 26.0;
const ANIM_TIME: f32 = 0.12;

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

/// 展开态面板这一帧的显示状态。
struct PanelState {
    /// 当前读到的那一条在 `placed` 里的下标。
    current: Option<usize>,
    /// 展开动画进度：0 完全收起，1 完全展开。
    t: f32,
    /// 是否要把当前节滚到眼前。只在刚展开的那一帧为真。
    reveal: bool,
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

        // 刻度条浮在纸张右侧留白上，让开滚动条——鼠标往右去多半是要拖滚动条，
        // 把可点区压在滚动条上会变成高频误触。
        let bar = ui.spacing().scroll.bar_width + 4.0;
        let rail_right = region.right() - bar;
        let rail = egui::Rect::from_min_max(
            egui::pos2(rail_right - RAIL_WIDTH, region.top() + 12.0),
            egui::pos2(rail_right, region.bottom() - 12.0),
        );
        if rail.height() < 40.0 {
            return;
        }

        let anim_id = ui.id().with("nav_rail_expand");
        let pointer = ctx.pointer_latest_pos();
        // 热区随面板一起变宽：展开后鼠标移到面板上仍算"在右缘"，不会一进面板就收回。
        // 取上一帧的展开状态而不是当帧的动画值——`animate_bool_with_time` 每调一次
        // 就把动画朝目标推一步，同一帧按两个目标各取一次会让它自己跟自己打架。
        let was_expanded = ctx.data(|data| data.get_temp::<bool>(anim_id).unwrap_or(false));
        let hot_width = if was_expanded {
            PANEL_WIDTH + bar
        } else {
            RAIL_WIDTH + HOT_MARGIN
        };
        let hot = egui::Rect::from_min_max(
            egui::pos2(region.right() - hot_width, region.top()),
            region.right_bottom(),
        );
        let hovered = pointer.is_some_and(|pos| hot.contains(pos) && region.contains(pos));
        ctx.data_mut(|data| data.insert_temp(anim_id, hovered));
        let t = ctx.animate_bool_with_time(anim_id, hovered, ANIM_TIME);

        self.paint_rail(&ctx, rail, &placed, current, 1.0 - t * 0.65);
        if t > 0.01 {
            // 只在刚展开的那一帧把当前节滚到眼前。长文里列表比面板长得多，
            // 展开后看到的若是列表顶部，等于还得自己找一遍。之后不再自动滚，
            // 否则用户手动翻列表会被一直拽回去。
            let state = PanelState {
                current,
                t,
                reveal: hovered && !was_expanded,
            };
            self.navigator_panel(&ctx, region, rail, &placed, &state);
        }
    }

    /// 常驻刻度。纯绘制，不占任何可点区域，因此不干扰滚动、选字和点块跳转。
    fn paint_rail(
        &self,
        ctx: &egui::Context,
        rail: egui::Rect,
        placed: &[Placed<'_>],
        current: Option<usize>,
        alpha: f32,
    ) {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("gw_nav_rail"),
        ));
        let painter = painter.with_clip_rect(rail.expand(4.0));
        let base = theme::text_muted().gamma_multiply(0.55 * alpha);
        let hot = theme::accent().gamma_multiply(alpha.max(0.35));
        for (index, item) in placed.iter().enumerate() {
            // 文档标题不进刻度：全文只有一处，而且就在最顶上，占一条刻度纯属浪费。
            if item.entry.level < 2 {
                continue;
            }
            let y = rail.top() + rail.height() * item.fraction;
            let length = tick_length(item.entry.level);
            let is_current = current == Some(index);
            painter.line_segment(
                [
                    egui::pos2(rail.right() - length, y),
                    egui::pos2(rail.right(), y),
                ],
                egui::Stroke::new(
                    if is_current { 2.0 } else { 1.0 },
                    if is_current { hot } else { base },
                ),
            );
        }
    }

    /// 展开态的标题列表。半透明浮在纸面上，字号压到 11.5，长标题截断。
    fn navigator_panel(
        &mut self,
        ctx: &egui::Context,
        region: egui::Rect,
        rail: egui::Rect,
        placed: &[Placed<'_>],
        state: &PanelState,
    ) {
        let PanelState { current, t, reveal } = *state;
        let height = (region.height() - 24.0).clamp(120.0, 520.0);
        // 从右缘滑出一小段，配合淡入；收起时反向滑回，不会"啪"地消失。
        let left = rail.right() - PANEL_WIDTH + (1.0 - t) * 14.0;
        let pos = egui::pos2(left, region.top() + 12.0);
        let mut jump = None;

        egui::Area::new(egui::Id::new("gw_nav_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain_to(region)
            .show(ctx, |ui| {
                ui.set_opacity(t);
                ui.set_width(PANEL_WIDTH - RAIL_WIDTH - 6.0);
                egui::Frame::new()
                    // 半透明：底下的正文仍透得出来，浮层不会把版面切掉一块。
                    .fill(theme::surface().gamma_multiply(0.97))
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 2],
                        blur: 12,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(theme::paper::shadow_alpha()),
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical()
                            .id_salt("gw_nav_list")
                            .max_height(height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                for (index, item) in placed.iter().enumerate() {
                                    let is_current = current == Some(index);
                                    if nav_row(ui, item.entry, is_current, reveal && is_current) {
                                        jump = Some(item.entry.line.clone());
                                    }
                                }
                            });
                    });
            });

        if let Some(line) = jump {
            self.jump_to_heading(line);
            ctx.request_repaint();
        }
    }

    /// 点中导航里的一条：源码把光标挪过去，版式预览滚到那一块并标亮。
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

/// 一行标题。返回是否被点中。
fn nav_row(ui: &mut egui::Ui, entry: &NavEntry, current: bool, reveal: bool) -> bool {
    // 底色要压在文字下面：先占一个空图形位，量出整行范围后再回填，
    // 与版式预览里标亮块的做法一致。
    let backdrop = ui.painter().add(egui::Shape::Noop);
    let indent = match entry.level {
        0 | 1 => 0.0,
        level => (level - 2) as f32 * 11.0,
    };
    let width = ui.available_width();
    let number_color = if current {
        theme::accent()
    } else {
        theme::text_muted()
    };
    let text_color = if current {
        theme::accent()
    } else {
        theme::text_soft()
    };
    let inner = ui
        .scope(|ui| {
            ui.horizontal(|ui| {
                ui.add_space(indent);
                if let Some(number) = &entry.number {
                    ui.label(egui::RichText::new(number).size(11.5).color(number_color));
                }
                let mut title = egui::RichText::new(&entry.text)
                    .size(11.5)
                    .color(text_color);
                // 正式标题（文档标题、附件标题）加粗当根节点。
                if entry.level < 2 {
                    title = title.strong();
                }
                ui.add(
                    // 长标题截断。公文标题动辄二十几字，撑宽面板等于又去抢宽度。
                    egui::Label::new(title).truncate().selectable(false),
                );
            });
        })
        .response
        .rect;

    let row = egui::Rect::from_min_size(inner.left_top(), egui::vec2(width, inner.height()))
        .expand2(egui::vec2(3.0, 1.0));
    let hit = ui.interact(
        row,
        ui.id().with(("nav_row", entry.line.start)),
        egui::Sense::click(),
    );
    if hit.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if reveal {
        ui.scroll_to_rect(row, Some(egui::Align::Center));
    }
    let fill = if current {
        theme::accent_soft()
    } else if hit.hovered() {
        theme::surface_hover()
    } else {
        return hit.clicked();
    };
    ui.painter().set(
        backdrop,
        egui::epaint::RectShape::filled(row, egui::CornerRadius::same(4), fill),
    );
    hit.clicked()
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
