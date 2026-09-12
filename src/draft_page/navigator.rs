//! 公文预览右缘的导航刻度：常驻一列细刻度反映全文结构，悬停某条就地浮出该节标题。
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
//! 为什么标题是就地浮出而不是另开一个大纲框。框一展开就盖掉一块版面，视线也得
//! 离开正文横移过去。标签贴着刻度浮在左侧、与刻度同高，指针沿带子上下扫就能连着
//! 看过各节标题，眼睛始终没离开纸面。代价是看不到全文目录的全貌，只能一条条扫——
//! 定位用够了，通览不够。
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
/// 指针离某条刻度多近才算"指着它"。刻度只有一两个点粗，要求精确压线
/// 等于要求用户绣花，所以就近吸附；但也不能无限远，否则空白处会浮出远处的标题。
const TICK_SNAP: f32 = 20.0;
/// 悬停标签的字号、内边距，以及它与刻度带之间的空隙。
const LABEL_FONT_SIZE: f32 = 12.0;
const LABEL_PAD_X: f32 = 9.0;
const LABEL_PAD_Y: f32 = 5.0;
const LABEL_GAP: f32 = 8.0;
/// 标签最宽到这里，再长就折行/截断——公文标题动辄二十几字，
/// 整条铺出去会横穿版面。
const LABEL_MAX_WIDTH: f32 = 300.0;

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
        // 才收滚轮，指针一停在刻度带上，预览就滚不动了——而扫完刻度顺手滚页
        // 恰恰是最自然的动作。同层则不然：导航画在预览之后，注册得更晚，
        // 点击照样归它，滚轮仍旧落到滚动区。
        let response = ui.interact(
            rail,
            egui::Id::new("gw_nav_rail_strip"),
            egui::Sense::click(),
        );
        // 刻度只有一两个点粗，要求指针精确压在线上等于要求用户绣花。
        // 改成吸附：指针在刻度带里上下移动，就近认最近的那条。
        let hovered = response
            .hover_pos()
            .and_then(|pos| nearest_tick(&placed, rail, pos.y));
        if hovered.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        paint_rail(ui, rail, &placed, current, hovered);
        let mut jump = None;
        if let Some(index) = hovered {
            paint_tick_label(ui, region, rail, &placed[index]);
            if response.clicked() {
                jump = Some(placed[index].entry.line.clone());
            }
        }

        if let Some(line) = jump {
            self.jump_to_heading(line);
            ctx.request_repaint();
        }
    }

    /// 点中一条刻度：源码把光标挪过去，版式预览滚到那一块并标亮。
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

/// 离指针最近的那条刻度，超出吸附距离就不认。
///
/// 不设上限的话，一篇只有两三节的稿子里，指针停在刻度带中段的大片空白上
/// 也会浮出某个远处的标题，看着像乱跳。
fn nearest_tick(placed: &[Placed<'_>], rail: egui::Rect, y: f32) -> Option<usize> {
    placed
        .iter()
        .enumerate()
        .filter(|(_, item)| item.entry.level >= 2)
        .map(|(index, item)| (index, (tick_y(rail, item) - y).abs()))
        .filter(|(_, distance)| *distance <= TICK_SNAP)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

fn tick_y(rail: egui::Rect, item: &Placed<'_>) -> f32 {
    rail.top() + rail.height() * item.fraction
}

/// 常驻刻度。当前所在那条用主色描粗，指针就近吸附到的那条再加一档。
fn paint_rail(
    ui: &egui::Ui,
    rail: egui::Rect,
    placed: &[Placed<'_>],
    current: Option<usize>,
    hovered: Option<usize>,
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
        let is_hovered = hovered == Some(index);
        let is_current = current == Some(index);
        // 悬停那条整条拉满宽度，让"我正指着它"一眼可见，不用去比粗细。
        let length = if is_hovered {
            RAIL_WIDTH
        } else {
            tick_length(item.entry.level)
        };
        let (width, color) = match (is_hovered, is_current) {
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

/// 悬停那条刻度的标题：半透明浮在刻度左侧、与刻度同高。
///
/// 不再单开一个大纲框。标签就地浮出，看的人视线不必离开正文，
/// 沿刻度带上下扫就能连着看过各节标题。
fn paint_tick_label(ui: &egui::Ui, region: egui::Rect, rail: egui::Rect, item: &Placed<'_>) {
    let text = match &item.entry.number {
        Some(number) => format!("{number}{}", item.entry.text),
        None => item.entry.text.clone(),
    };
    if text.trim().is_empty() {
        return;
    }
    let font = egui::FontId::proportional(LABEL_FONT_SIZE);
    // 先按上限截断再排版：公文标题动辄二十几字，整条铺出去会横穿版面。
    let galley = ui.painter().layout(
        text,
        font,
        theme::text(),
        LABEL_MAX_WIDTH - LABEL_PAD_X * 2.0,
    );

    let height = galley.size().y + LABEL_PAD_Y * 2.0;
    let width = galley.size().x + LABEL_PAD_X * 2.0;
    let right = rail.left() - LABEL_GAP;
    // 与刻度同高居中；贴到预览上下边时收回来，别让标签被切掉半截。
    let center_y = tick_y(rail, item).clamp(
        region.top() + height * 0.5 + 4.0,
        region.bottom() - height * 0.5 - 4.0,
    );
    let rect = egui::Rect::from_min_max(
        egui::pos2(right - width, center_y - height * 0.5),
        egui::pos2(right, center_y + height * 0.5),
    );

    // 标签画在刻度带那一层，但在带子之外——层只在带子上拦截指针，
    // 所以标签盖住的正文照样点得到。
    let painter = ui.painter().with_clip_rect(region);
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: egui::Color32::from_black_alpha(theme::paper::shadow_alpha()),
        }
        .as_shape(rect, egui::CornerRadius::same(6)),
    );
    // 半透明：底下的正文仍透得出来，标签不会把版面切掉一块。
    painter.rect(
        rect,
        egui::CornerRadius::same(6),
        theme::surface().gamma_multiply(0.92),
        egui::Stroke::new(1.0, theme::border()),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(rect.left() + LABEL_PAD_X, rect.top() + LABEL_PAD_Y),
        galley,
        theme::text(),
    );
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
