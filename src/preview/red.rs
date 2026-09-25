//! 红头呈批件：打印分页器、红头版式布局与覆盖绘制。
//!
//! 由 src/preview.rs 拆分而来：本文件是模块 `preview::red`，与其它子模块共享
//! `preview` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::export::RedlineKind;
use crate::export::{LocatedBlock, MarkdownBlock};
use crate::models::{DraftInput, NumberingConfig};
use crate::preview::gutter;
use crate::preview::layout::{MeasuredTable, measure_table};
use crate::preview::marks;
use crate::preview::{
    BODY_PT, CLOSING_GAP_LINES, HEADER_PT, INDENT_CHARS, LINE_PT, MM, Metrics, PAREN_PT,
    clickable_content_block, document_number, first_ink, header_unit, heading_family, indent,
    is_renderable_paragraph, job, justified_rows, layout, line_block, row_tint_offset,
    scroll_preview_to_rect, sheet, signature_date, single_line, text_format,
};
use crate::theme;
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use eframe::egui;
use eframe::egui::text::LayoutJob;
use eframe::egui::{Align, Color32, Stroke};
use std::ops::Range;
use std::sync::Arc;

/// 一行里字形底沿在行盒中的位置比例：基线在行高的 75%（见 `gutter::row_baseline`），
/// 再加约 7% 的字身下沿。判断首页末行放不放得下时按它量，而不是按整个行盒。
pub(crate) const RED_INK_RATIO: f32 = 0.82;

/// 片段内第 `index` 行的行盒上沿（相对片段顶）。
///
/// 呈批件是真分页的，行位置必须按固定行距 `metrics.line` 累加，不能取 galley
/// 自己的行盒：epaint 会把每一行的行高取整到物理像素（1.2 倍缩放时 44.8px 的
/// 行排成 45px），整页十几行攒下来差出将近一行，首页末行就在某些缩放 / DPI 下
/// 被判放不下、挪到下一页，红线上方白白空出一行。排版与绘制都按这里的位置走。
pub(crate) fn red_row_top(metrics: &Metrics, index: usize) -> f32 {
    index as f32 * metrics.line
}

/// 普通文种正文区渲染参数。红头呈批件走下方独立的打印分页模型。
pub(crate) struct BodyRun {
    pub(crate) compact_headings: Vec<bool>,
    pub(crate) numbered: bool,
}

/// 红头呈批件打印预览中的一段可见文字。正文段落可以被切成多个 fragment：
/// 首页片段按 96mm 排，续页片段重新按 156mm 排，而不是沿用首页的换行结果。
pub(crate) struct RedPrintFragment {
    pub(crate) range: Option<Range<usize>>,
    /// 正文自然段按 Markdown 源码行拆开的可见字符范围；空表示整块使用 `range`。
    source_segments: Vec<crate::preview::ClickableSourceSegment>,
    pub(crate) galley: Arc<egui::Galley>,
    /// 两端对齐后的逐行 galley，与 `galley.rows` 一一对应；非空时按行画，
    /// 空表示这一段不参与对齐（标题、落款等自有对齐方式的固定片段）。
    justified: Vec<Arc<egui::Galley>>,
    x: f32,
    y: f32,
    pub(crate) width: f32,
    visible_height: f32,
    align: Align,
}

impl RedPrintFragment {
    /// 这一片可见文字的行盒下沿（纸面坐标）。首页排满与否按它量。
    #[cfg(test)]
    pub(crate) fn bottom(&self) -> f32 {
        self.y + self.visible_height
    }
}

#[derive(Default)]
pub(crate) struct RedPrintPage {
    pub(crate) fragments: Vec<RedPrintFragment>,
    pub(crate) tables: Vec<RedTableSlice>,
}

/// 落在某一页上的一段表格：同一张表可以跨页，续页上表头重复一遍，与 TeX 的
/// longtblr（`rowhead = 1`）一致。
pub(crate) struct RedTableSlice {
    table: Arc<MeasuredTable>,
    /// 这一段画哪几行（续页以表头 0 打头），自上而下紧挨着排。
    pub(crate) rows: Vec<usize>,
    /// 每一行对应的那一行 Markdown 源码，点击回跳与页边行号都认它。
    sources: Arc<Vec<Range<usize>>>,
    x: f32,
    pub(crate) y: f32,
}

impl RedTableSlice {
    #[cfg(test)]
    pub(crate) fn bottom(&self) -> f32 {
        self.y
            + self
                .rows
                .iter()
                .map(|row| self.table.row_heights[*row])
                .sum::<f32>()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RedTextStyle {
    Body,
    Heading(u8),
    List,
}

#[derive(Clone)]
struct RedFlowSegment {
    text: String,
    bold: bool,
    parenthesized: bool,
    style: RedTextStyle,
    /// 花脸稿标记：跨页切开时跟着字走，续页上的删除线与框不会丢。
    mark: RedlineKind,
}

/// 行内 Markdown → 排版片段：与 DOCX 的 `body_runs` 同一个切法，先按花脸稿
/// 哨兵切块，再在块内解析加粗与括号。没有哨兵时只有一块 `Same`。
fn red_flow_segments(text: &str, style: RedTextStyle) -> Vec<RedFlowSegment> {
    export::redline_chunks(text)
        .into_iter()
        .flat_map(|chunk| {
            export::inline_segments(&chunk.text)
                .into_iter()
                .map(move |segment| RedFlowSegment {
                    text: segment.text,
                    bold: segment.bold,
                    parenthesized: segment.parenthesized,
                    style,
                    mark: chunk.kind,
                })
        })
        .collect()
}

/// 已是纯文本、只可能带着哨兵的整行文字（标题）→ 排版片段。
fn red_plain_segments(text: &str, style: RedTextStyle) -> Vec<RedFlowSegment> {
    export::redline_chunks(text)
        .into_iter()
        .map(|chunk| RedFlowSegment {
            text: chunk.text,
            bold: false,
            parenthesized: false,
            style,
            mark: chunk.kind,
        })
        .collect()
}

pub(crate) struct RedPrintLayout {
    pub(crate) pages: Vec<RedPrintPage>,
    page_index: usize,
    cursor_y: f32,
    first_body_bottom: f32,
}

impl RedPrintLayout {
    pub(crate) fn new(first_cursor_y: f32, first_body_bottom: f32) -> Self {
        Self {
            pages: vec![RedPrintPage::default()],
            page_index: 0,
            cursor_y: first_cursor_y,
            first_body_bottom,
        }
    }

    pub(crate) fn body_left(&self, metrics: &Metrics) -> f32 {
        metrics.mm(28.0)
    }

    pub(crate) fn body_width(&self, metrics: &Metrics) -> f32 {
        metrics.mm(if self.page_index == 0 {
            export::RED_APPROVAL_NARROW_MM as f32
        } else {
            156.0
        })
    }

    pub(crate) fn body_bottom(&self, metrics: &Metrics) -> f32 {
        if self.page_index == 0 {
            self.first_body_bottom
        } else {
            metrics.mm(37.0 + 225.0)
        }
    }

    pub(crate) fn next_page(&mut self, metrics: &Metrics) {
        self.pages.push(RedPrintPage::default());
        self.page_index += 1;
        self.cursor_y = metrics.mm(37.0);
    }

    pub(crate) fn push(&mut self, fragment: RedPrintFragment) {
        self.pages[self.page_index].fragments.push(fragment);
    }
}

/// 返回当前页能容纳的最大落款间距：优先 3 行，临界时缩为 2 行或 1 行。
/// 连 1 行都放不下时返回 `None`，由调用方另起“此页无正文”页。
pub(crate) fn fitting_closing_gap_lines(
    cursor_y: f32,
    signature_height: f32,
    page_bottom: f32,
    line_height: f32,
) -> Option<usize> {
    (1..=CLOSING_GAP_LINES)
        .rev()
        .find(|lines| cursor_y + *lines as f32 * line_height + signature_height <= page_bottom)
}

/// 开启专门的落款页，并保持“此页无正文”与落款之间空 3 行。
pub(crate) fn red_start_no_body_closing_page(
    ui: &egui::Ui,
    metrics: &Metrics,
    state: &mut RedPrintLayout,
    left: f32,
) {
    state.next_page(metrics);
    state.cursor_y += metrics.pt(58.0);
    let notice = red_fixed_fragment(
        ui,
        metrics,
        &format!("{}（此页无正文）", indent(INDENT_CHARS)),
        theme::FONT_FANGSONG,
        BODY_PT,
        metrics.mm(156.0),
        Align::LEFT,
        None,
        left,
        state.cursor_y,
    );
    state.cursor_y += notice.visible_height;
    state.push(notice);
    state.cursor_y += metrics.line * CLOSING_GAP_LINES as f32;
}

fn red_inline_job(
    metrics: &Metrics,
    width: f32,
    segments: &[RedFlowSegment],
    first_line_indent: bool,
) -> LayoutJob {
    let mut job = job(width);
    let normal = metrics.font(theme::FONT_FANGSONG, BODY_PT);
    if first_line_indent {
        job.append(
            &indent(INDENT_CHARS),
            0.0,
            text_format(normal.clone(), metrics.line),
        );
    }
    let mut previous = RedlineKind::Same;
    for segment in segments {
        let font = match segment.style {
            RedTextStyle::Heading(level) => metrics.font(heading_family(level), BODY_PT),
            RedTextStyle::List => normal.clone(),
            RedTextStyle::Body if segment.parenthesized => {
                metrics.font(theme::FONT_KAITI, PAREN_PT)
            }
            RedTextStyle::Body if segment.bold => metrics.font(theme::FONT_BOLD, BODY_PT),
            RedTextStyle::Body => normal.clone(),
        };
        job.append(
            &segment.text,
            marks::chunk_gap(metrics, previous, segment.mark),
            marks::mark_format(text_format(font, metrics.line), segment.mark),
        );
        previous = segment.mark;
    }
    job
}

fn red_drop_chars(segments: &mut Vec<RedFlowSegment>, mut count: usize) {
    while count > 0 && !segments.is_empty() {
        let chars = segments[0].text.chars().count();
        if count >= chars {
            count -= chars;
            segments.remove(0);
        } else {
            segments[0].text = segments[0].text.chars().skip(count).collect();
            count = 0;
        }
    }
}

/// 把一个逻辑段落连续排入页面。首页放得下多少行就放多少行；剩余文本进入
/// 下一页后重新生成 galley，因此第二页第一行立即采用标准 156mm 版心。
fn red_place_flow_text(
    ui: &egui::Ui,
    metrics: &Metrics,
    layout_state: &mut RedPrintLayout,
    range: Range<usize>,
    segments: Vec<RedFlowSegment>,
    first_line_indent: bool,
) {
    let visible_chars = segments
        .iter()
        .map(|segment| segment.text.chars().count())
        .sum();
    red_place_styled_flow_text(
        ui,
        metrics,
        layout_state,
        segments,
        vec![crate::preview::ClickableSourceSegment {
            source: range,
            chars: 0..visible_chars,
        }],
        first_line_indent,
    );
}

fn red_place_styled_flow_text(
    ui: &egui::Ui,
    metrics: &Metrics,
    layout_state: &mut RedPrintLayout,
    mut segments: Vec<RedFlowSegment>,
    source_segments: Vec<crate::preview::ClickableSourceSegment>,
    first_line_indent: bool,
) {
    let combined_range = source_segments
        .first()
        .zip(source_segments.last())
        .map(|(first, last)| first.source.start..last.source.end);
    let mut first_fragment = true;
    let mut consumed_total = 0usize;
    while !segments.is_empty() {
        let width = layout_state.body_width(metrics);
        let available = layout_state.body_bottom(metrics) - layout_state.cursor_y;
        if available < metrics.line * 0.75 {
            layout_state.next_page(metrics);
            continue;
        }
        let indent_this_fragment = first_fragment && first_line_indent;
        let flow_job = red_inline_job(metrics, width, &segments, indent_this_fragment);
        let galley = layout(ui, flow_job.clone());
        // 正文两端对齐，与 Word 导出和 TeX 一致；末行保持自然宽度。
        let justified = justified_rows(ui, &flow_job, &galley);
        // 行盒是 28pt，字形只占其中约 82%（基线在 75% 处，再加一点字身下沿）。
        // 按行盒底判定会白白空掉最后一行——TeX 那边只要红线上方还容得下一个
        // 三号字就照排（cls 里 \RedFirstPageRemaining>16pt 那一支）。这里同样
        // 按字形底沿判定：末行的行盒可以越过正文区下沿，字形仍在承办区红线
        // 上方 2mm 的安全距离之内。行位置按固定行距算（见 `red_row_top`），
        // 不取 galley 里取整过的行盒。
        let ink_bottom = |index: usize| red_row_top(metrics, index) + metrics.line * RED_INK_RATIO;
        let fitting = (0..galley.rows.len())
            .take_while(|index| ink_bottom(*index) <= available + 0.5)
            .count();
        if fitting == 0 {
            layout_state.next_page(metrics);
            continue;
        }
        let visible_height = red_row_top(metrics, fitting);
        let mut consumed = galley.rows[..fitting]
            .iter()
            .map(|row| row.glyphs.len())
            .sum::<usize>();
        if indent_this_fragment {
            consumed = consumed.saturating_sub(INDENT_CHARS as usize);
        }
        let all_fit = fitting == galley.rows.len();
        let indent_chars = usize::from(indent_this_fragment) * INDENT_CHARS as usize;
        let fragment_segments = source_segments
            .iter()
            .filter_map(|segment| {
                let start = segment.chars.start.max(consumed_total);
                let end = segment
                    .chars
                    .end
                    .min(consumed_total.saturating_add(consumed));
                (start < end).then(|| crate::preview::ClickableSourceSegment {
                    source: segment.source.clone(),
                    chars: if segment.chars.start == 0 && consumed_total == 0 {
                        0..end - consumed_total + indent_chars
                    } else {
                        start - consumed_total + indent_chars..end - consumed_total + indent_chars
                    },
                })
            })
            .collect();
        layout_state.push(RedPrintFragment {
            range: combined_range.clone(),
            source_segments: fragment_segments,
            galley,
            justified,
            x: layout_state.body_left(metrics),
            y: layout_state.cursor_y,
            width,
            visible_height,
            align: Align::LEFT,
        });
        layout_state.cursor_y += visible_height;
        if all_fit {
            break;
        }
        if consumed == 0 {
            layout_state.next_page(metrics);
            continue;
        }
        red_drop_chars(&mut segments, consumed);
        consumed_total += consumed;
        first_fragment = false;
        layout_state.next_page(metrics);
    }
}

/// 居中 / 居右区的一行：与正文同字体，不缩进、不两端对齐，整行相对当前页的
/// 版心（首页是批示栏左边的窄栏）居中或靠右。一行里放不下就整体推到下一页，
/// 不在行内拆页——这类行本来就短。
fn red_place_aligned_text(
    ui: &egui::Ui,
    metrics: &Metrics,
    layout_state: &mut RedPrintLayout,
    range: Range<usize>,
    text: &str,
    align: export::LineAlign,
) {
    let halign = match align {
        export::LineAlign::Center => Align::Center,
        export::LineAlign::Right => Align::Max,
    };
    let segments = red_flow_segments(text, RedTextStyle::Body);
    loop {
        let width = layout_state.body_width(metrics);
        let available = layout_state.body_bottom(metrics) - layout_state.cursor_y;
        let mut job = red_inline_job(metrics, width, &segments, false);
        job.halign = halign;
        let galley = layout(ui, job);
        let row_count = galley.rows.len().max(1);
        let ink_bottom = red_row_top(metrics, row_count - 1) + metrics.line * RED_INK_RATIO;
        if ink_bottom > available + 0.5 && layout_state.cursor_y > metrics.mm(37.0) + 0.5 {
            layout_state.next_page(metrics);
            continue;
        }
        let left = layout_state.body_left(metrics);
        let x = match align {
            export::LineAlign::Center => left + width / 2.0,
            export::LineAlign::Right => left + width,
        };
        let visible_height = red_row_top(metrics, row_count);
        layout_state.push(RedPrintFragment {
            range: Some(range),
            source_segments: Vec::new(),
            galley,
            justified: Vec::new(),
            x,
            y: layout_state.cursor_y,
            width,
            visible_height,
            align: halign,
        });
        layout_state.cursor_y += visible_height;
        return;
    }
}

/// 表格按真表格排进呈批件的续页：与 TeX 一致不进首页批示窄栏，占 156mm 版心。
/// 一页放不下就在行与行之间断开，续页先重复表头；纵向合并的几行不拆开，
/// 表头也不单独留在页底。
#[allow(clippy::too_many_arguments)]
fn red_place_table(
    ui: &egui::Ui,
    metrics: &Metrics,
    state: &mut RedPrintLayout,
    markdown: &str,
    range: Range<usize>,
    rows: &[Vec<String>],
    aligns: &[export::ColumnAlign],
    spans: &[export::TableSpan],
    numbered: bool,
) {
    if state.page_index == 0 {
        state.next_page(metrics);
    }
    let Some(table) = measure_table(
        ui,
        metrics,
        rows,
        aligns,
        spans,
        numbered,
        state.body_width(metrics),
    ) else {
        return;
    };
    let table = Arc::new(table);
    let sources = Arc::new(table_row_sources(markdown, &range, rows.len()));
    let heights = &table.row_heights;
    let height_of = |from: usize, to: usize| heights[from..to].iter().sum::<f32>();
    let page_top = metrics.mm(37.0);

    let mut next = 1usize;
    loop {
        // 表头连同紧跟的第一段放不下、这一页又不是从头排的，整张挪到下一页。
        let first_end = if next < heights.len() {
            table.unbreakable_end(next)
        } else {
            next
        };
        if state.cursor_y + heights[0] + height_of(next, first_end) > state.body_bottom(metrics)
            && state.cursor_y > page_top + 0.5
        {
            state.next_page(metrics);
        }
        let mut slice_rows = vec![0];
        let mut height = heights[0];
        while next < heights.len() {
            let end = table.unbreakable_end(next);
            let group = height_of(next, end);
            // 每页至少放一段，哪怕它比整页还高，免得死循环。
            if slice_rows.len() > 1 && state.cursor_y + height + group > state.body_bottom(metrics)
            {
                break;
            }
            slice_rows.extend(next..end);
            height += group;
            next = end;
        }
        let slice = RedTableSlice {
            table: table.clone(),
            rows: slice_rows,
            sources: sources.clone(),
            x: state.body_left(metrics),
            y: state.cursor_y,
        };
        let page = state.page_index;
        state.pages[page].tables.push(slice);
        state.cursor_y += height;
        if next >= heights.len() {
            break;
        }
        state.next_page(metrics);
    }
}

/// 表格第 `row` 行对应的那一行源码：表头是第 0 行，分隔行不算，其后一行一行对应。
fn table_row_sources(markdown: &str, range: &Range<usize>, row_count: usize) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = range.start;
    for line in markdown[range.clone()].split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        lines.push(start..start + content.len());
        start += line.len();
    }
    (0..row_count)
        .map(|row| {
            let line = if row == 0 { 0 } else { row + 1 };
            lines.get(line).cloned().unwrap_or_else(|| range.clone())
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn red_fixed_fragment(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    width: f32,
    align: Align,
    range: Option<Range<usize>>,
    x: f32,
    y: f32,
) -> RedPrintFragment {
    let mut job = job(width);
    job.halign = align;
    marks::append_marked_text(
        &mut job,
        metrics,
        text,
        text_format(metrics.font(family, size), metrics.line),
    );
    let galley = layout(ui, job);
    RedPrintFragment {
        range,
        source_segments: Vec::new(),
        visible_height: red_row_top(metrics, galley.rows.len().max(1)),
        galley,
        justified: Vec::new(),
        x,
        y,
        width,
        align,
    }
}

pub(crate) fn red_responsible_rows(input: &DraftInput, display: &UnitDisplay) -> Vec<[String; 3]> {
    let entries = crate::models::joint_responsible_entries(&input.profile);
    if entries.is_empty() {
        return vec![[String::new(), String::new(), String::new()]];
    }
    entries
        .iter()
        .map(|entry| {
            [
                display.abbr(&entry.unit),
                entry.name.clone(),
                entry.phone.clone(),
            ]
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn red_build_print_layout(
    ui: &egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
    body: &[&LocatedBlock],
    title: &(String, Range<usize>),
    attachment_names: &[String],
    numbering: &NumberingConfig,
    markdown: &str,
    elements: &ElementMarks,
) -> (RedPrintLayout, Vec<[String; 3]>) {
    let rows = red_responsible_rows(input, display);
    // TeX 承办区 = 0.4mm 红线 + 1mm 间距 + 每条固定 28pt 基线。
    let record_height = metrics.mm(1.4) + metrics.line * rows.len().max(1) as f32;
    let first_body_bottom = metrics.mm(37.0 + 225.0) - record_height - metrics.mm(2.0);
    let left = metrics.mm(28.0);
    // 首页窄栏比红色竖线再窄 4mm，标题与正文同宽（见 export::RED_APPROVAL_NARROW_MM）。
    let narrow = metrics.mm(export::RED_APPROVAL_NARROW_MM as f32);
    let mut state = RedPrintLayout::new(metrics.mm(37.0 + 55.0), first_body_bottom);

    if !title.0.is_empty() {
        let fragment = red_fixed_fragment(
            ui,
            metrics,
            &title.0,
            theme::FONT_BIAOSONG,
            18.0,
            narrow,
            Align::Center,
            Some(title.1.clone()),
            left + narrow / 2.0,
            state.cursor_y,
        );
        state.cursor_y += fragment.visible_height;
        state.push(fragment);
    }
    state.cursor_y += metrics.line;
    let leaders = crate::export::element_display::addressee_display(input, display);
    if !leaders.is_empty() || elements.recipient().changed() {
        // 呈报领导（主送位）变了整字段替换，冒号不进标注。
        let leaders_text = if elements.recipient().changed() {
            format!("{}：", elements.recipient().marked())
        } else {
            format!("{leaders}：")
        };
        let fragment = red_fixed_fragment(
            ui,
            metrics,
            &leaders_text,
            theme::FONT_KAITI,
            BODY_PT,
            narrow,
            Align::LEFT,
            None,
            left,
            state.cursor_y,
        );
        state.cursor_y += fragment.visible_height;
        state.push(fragment);
    }

    let body_plain = body
        .iter()
        .map(|located| located.block.clone())
        .collect::<Vec<_>>();
    let compact_headings = export::compact_heading_flags(&body_plain, input.profile.style_mode);
    let mut counters = [0usize; 4];
    let mut index = 0usize;
    while index < body.len() {
        let located = body[index];
        match &located.block {
            MarkdownBlock::Title(_) | MarkdownBlock::Marker(_) | MarkdownBlock::Html(_) => {}
            MarkdownBlock::Heading(level, text) => {
                if let Some(number) =
                    export::official_heading_prefix(*level, &mut counters, numbering)
                {
                    // 新增标题连编号一起加框（方案规则 8），其余只标文字。
                    let marked = marks::plain_keep_marks(text);
                    let text = if export::whole_chunk_kind(&marked) == Some(RedlineKind::Added) {
                        export::mark_added(&format!("{number}{}", export::strip_redline(&marked)))
                    } else {
                        format!("{number}{marked}")
                    };
                    let next_paragraph = body.get(index + 1).and_then(|next| {
                        let MarkdownBlock::Paragraph(body) = &next.block else {
                            return None;
                        };
                        is_renderable_paragraph(body).then_some((*next, body.as_str()))
                    });
                    if compact_headings[index]
                        && let Some((next, body_text)) = next_paragraph
                    {
                        let heading_text = format!("{text}。");
                        let heading_chars = export::strip_redline(&heading_text).chars().count();
                        let mut flow =
                            red_plain_segments(&heading_text, RedTextStyle::Heading(*level));
                        flow.extend(red_flow_segments(body_text, RedTextStyle::Body));
                        let mut source_segments = vec![crate::preview::ClickableSourceSegment {
                            source: located.range.clone(),
                            chars: 0..heading_chars,
                        }];
                        source_segments.extend(
                            crate::preview::paragraph_source_segments(markdown, next, body_text)
                                .into_iter()
                                .map(|segment| crate::preview::ClickableSourceSegment {
                                    source: segment.source,
                                    chars: heading_chars + segment.chars.start
                                        ..heading_chars + segment.chars.end,
                                }),
                        );
                        red_place_styled_flow_text(
                            ui,
                            metrics,
                            &mut state,
                            flow,
                            source_segments,
                            true,
                        );
                        index += 1;
                    } else {
                        let mut segments = red_plain_segments(&text, RedTextStyle::Heading(*level));
                        segments.insert(
                            0,
                            RedFlowSegment {
                                text: indent(INDENT_CHARS),
                                bold: false,
                                parenthesized: false,
                                style: RedTextStyle::Heading(*level),
                                mark: RedlineKind::Same,
                            },
                        );
                        red_place_flow_text(
                            ui,
                            metrics,
                            &mut state,
                            located.range.clone(),
                            segments,
                            false,
                        );
                    }
                }
            }
            MarkdownBlock::Paragraph(text) if is_renderable_paragraph(text) => {
                red_place_styled_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    red_flow_segments(text, RedTextStyle::Body),
                    crate::preview::paragraph_source_segments(markdown, located, text),
                    true,
                );
            }
            MarkdownBlock::Aligned { align, text } => {
                red_place_aligned_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    text,
                    *align,
                );
            }
            MarkdownBlock::OrderedListItem { number, text } => {
                let prefix = export::render_list_number(numbering.list2, *number);
                let mut segments =
                    red_flow_segments(&format!("{prefix}{text}"), RedTextStyle::List);
                if let Some(first) = segments.first_mut() {
                    first.text = format!("{}{}", indent(INDENT_CHARS), first.text);
                }
                red_place_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    segments,
                    false,
                );
            }
            MarkdownBlock::Table {
                rows,
                aligns,
                spans,
                numbered,
            } => {
                red_place_table(
                    ui,
                    metrics,
                    &mut state,
                    markdown,
                    located.range.clone(),
                    rows,
                    aligns,
                    spans,
                    *numbered,
                );
            }
            MarkdownBlock::Image { alt, .. } => {
                if state.page_index == 0 {
                    state.next_page(metrics);
                }
                red_place_flow_text(
                    ui,
                    metrics,
                    &mut state,
                    located.range.clone(),
                    vec![RedFlowSegment {
                        text: format!("〔图片：{}〕", export::plain_text(alt)),
                        bold: false,
                        parenthesized: false,
                        style: RedTextStyle::Body,
                        mark: RedlineKind::Same,
                    }],
                    false,
                );
            }
            MarkdownBlock::Paragraph(_) => {}
        }
        index += 1;
    }

    if !attachment_names.is_empty() {
        if state.page_index == 0 {
            state.next_page(metrics);
        }
        state.cursor_y += metrics.line * 2.0;
        for (index, name) in attachment_names.iter().enumerate() {
            let label = if attachment_names.len() == 1 {
                format!("附件：{name}")
            } else if index == 0 {
                format!("附件{}：{name}", index + 1)
            } else {
                format!("　　{}：{name}", index + 1)
            };
            red_place_flow_text(
                ui,
                metrics,
                &mut state,
                0..0,
                vec![RedFlowSegment {
                    text: label,
                    bold: false,
                    parenthesized: false,
                    style: RedTextStyle::Body,
                    mark: RedlineKind::Same,
                }],
                // 与 Word / TeX 一致：附件概要首行缩进两个汉字。
                true,
            );
        }
    }

    let signature_units = crate::export::element_display::signing_unit_display(input, display)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect::<Vec<_>>();
    // 落款单位按行标注：变了的行整行删旧插新，旧版多出来的行整行画删除线。
    let line_marks = elements.signing_units();
    let mut signature_lines: Vec<String> = signature_units
        .iter()
        .enumerate()
        .map(|(index, unit)| match line_marks.get(index) {
            Some(mark) if mark.changed() => mark.marked(),
            _ => unit.clone(),
        })
        .collect();
    for mark in line_marks.iter().skip(signature_units.len()) {
        if mark.changed() {
            signature_lines.push(mark.marked());
        }
    }
    let signature_height = metrics.line * (signature_lines.len().max(1) * 2 + 1) as f32;

    // 呈批件落款不得在首页。正文已经续页时优先空 3 行；若仅因这 3 行放不下，
    // 依次缩到 2 行、1 行，避免无谓制造“此页无正文”页。
    let body_ended_on_first = state.page_index == 0;
    if body_ended_on_first {
        red_start_no_body_closing_page(ui, metrics, &mut state, left);
    } else if let Some(gap_lines) = fitting_closing_gap_lines(
        state.cursor_y,
        signature_height,
        metrics.mm(37.0 + 225.0),
        metrics.line,
    ) {
        state.cursor_y += metrics.line * gap_lines as f32;
    } else {
        red_start_no_body_closing_page(ui, metrics, &mut state, left);
    }
    let signature_right = left + metrics.mm(156.0 - crate::export::SIGNATURE_ROOM_MM);
    for (index, unit) in signature_lines.iter().enumerate() {
        if index > 0 {
            state.cursor_y += metrics.line;
        }
        let fragment = red_fixed_fragment(
            ui,
            metrics,
            unit,
            theme::FONT_FANGSONG,
            BODY_PT,
            metrics.mm(116.0),
            Align::Max,
            None,
            signature_right,
            state.cursor_y,
        );
        state.cursor_y += fragment.visible_height;
        state.push(fragment);
    }
    state.cursor_y += metrics.line;
    let date_text = if elements.date().changed() {
        elements.date().marked_line()
    } else {
        signature_date(input)
    };
    let date = red_fixed_fragment(
        ui,
        metrics,
        &date_text,
        theme::FONT_FANGSONG,
        BODY_PT,
        metrics.mm(116.0),
        Align::Center,
        None,
        left + metrics.mm(116.0) / 2.0,
        state.cursor_y,
    );
    state.push(date);

    (state, rows)
}

pub(crate) fn red_overlay_text(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    color: Color32,
) -> Arc<egui::Galley> {
    let mut format = text_format(metrics.font(family, size), metrics.line);
    format.color = color;
    layout(ui, single_line(text, format))
}

/// 同 [`red_overlay_text`]，但认花脸稿哨兵：带标注的要素（密级、文号）就地画
/// 删除线与新增框；绘制后要调 [`marks::paint_galley_marks`]。
pub(crate) fn red_overlay_marked_text(
    ui: &egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    color: Color32,
) -> Arc<egui::Galley> {
    let mut format = text_format(metrics.font(family, size), metrics.line);
    format.color = color;
    let mut job = single_line("", format.clone());
    marks::append_marked_text(&mut job, metrics, text, format);
    layout(ui, job)
}

/// 覆盖层元素的**基线**在自己 galley 里的高度。
///
/// 密级、发文机关、文号、批示在类文件里都是 `\raisebox{-Xmm}` 摆的，量的是基线；
/// egui 的 `painter.galley` 吃的却是左上角。直接把这几个毫米数当左上角用，整块
/// 红头就整体下沉一个字的上伸高度——文号会压到红线上，与密级之间的空档也被撑大。
pub(crate) fn overlay_baseline(galley: &egui::Galley) -> f32 {
    galley
        .rows
        .first()
        .map_or(galley.size().y * 0.75, |row| gutter::row_baseline(row, 0.0))
}

pub(crate) fn paint_red_approval_overlay(
    ui: &egui::Ui,
    metrics: &Metrics,
    page: egui::Rect,
    input: &DraftInput,
    display: &UnitDisplay,
    rows: &[[String; 3]],
    elements: &ElementMarks,
) {
    let painter = ui.painter();
    let at = |x: f32, y: f32| page.min + egui::vec2(metrics.mm(x), metrics.mm(y));
    let text_left = 28.0;
    let text_top = 37.0;
    let text_bottom = text_top + 225.0;
    let record_height = 1.4 + rows.len().max(1) as f32 * (LINE_PT / MM);
    let record_top = text_bottom - record_height;

    let security = crate::export::element_display::security_display(input);
    if security.is_some() || elements.security().changed() {
        let security = security.unwrap_or_default();
        let security_changed = elements.security().changed();
        let security_text = if security_changed {
            elements.security().marked()
        } else {
            security
        };
        let galley = if security_changed {
            red_overlay_marked_text(
                ui,
                metrics,
                &security_text,
                theme::FONT_HEITI,
                BODY_PT,
                theme::paper::ink(),
            )
        } else {
            red_overlay_text(
                ui,
                metrics,
                &security_text,
                theme::FONT_HEITI,
                BODY_PT,
                theme::paper::ink(),
            )
        };
        let baseline = overlay_baseline(&galley);
        let pos = at(text_left, text_top + 10.0) - egui::vec2(0.0, baseline);
        painter.galley(pos, galley.clone(), theme::paper::ink());
        if security_changed {
            marks::paint_galley_marks(painter, metrics, pos, &galley);
        }
    }

    let unit = header_unit(input, display);
    if !unit.trim().is_empty() {
        let count = unit.chars().count() as f32;
        let mut format = text_format(
            metrics.font(theme::FONT_BIAOSONG, HEADER_PT),
            metrics.pt(HEADER_PT * 1.2),
        );
        format.color = theme::paper::red();
        let natural = layout(ui, single_line(&unit, format.clone())).size().x;
        if count > 1.0 && natural < metrics.mm(156.0) {
            format.extra_letter_spacing =
                ((metrics.mm(156.0) - natural) / (count - 1.0)).min(metrics.pt(HEADER_PT));
        }
        let galley = layout(ui, single_line(&unit, format));
        let center = at(text_left + 78.0, text_top + 30.0);
        let offset = egui::vec2(galley.size().x / 2.0, overlay_baseline(&galley));
        painter.galley(center - offset, galley, theme::paper::red());
    }

    let number_changed = elements.number().iter().any(|part| part.changed());
    let number = if number_changed {
        elements.number_marked_line(" ")
    } else {
        document_number(input)
    };
    let number_galley = if number_changed {
        red_overlay_marked_text(
            ui,
            metrics,
            &number,
            theme::FONT_FANGSONG,
            BODY_PT,
            theme::paper::ink(),
        )
    } else {
        red_overlay_text(
            ui,
            metrics,
            &number,
            theme::FONT_FANGSONG,
            BODY_PT,
            theme::paper::ink(),
        )
    };
    let number_x = at(text_left + 78.0, text_top + 43.0)
        - egui::vec2(
            number_galley.size().x / 2.0,
            overlay_baseline(&number_galley),
        );
    painter.galley(number_x, number_galley.clone(), theme::paper::ink());
    if number_changed {
        marks::paint_galley_marks(painter, metrics, number_x, &number_galley);
    }

    let red = Stroke::new(metrics.mm(0.4).max(1.0), theme::paper::red());
    painter.line_segment(
        [
            at(text_left, text_top + 48.0),
            at(text_left + 156.0, text_top + 48.0),
        ],
        red,
    );
    let rule_x = text_left + export::RED_APPROVAL_RULE_MM as f32;
    painter.line_segment([at(rule_x, text_top + 48.0), at(rule_x, record_top)], red);
    let instruction = red_overlay_text(
        ui,
        metrics,
        "批　示",
        theme::FONT_FANGSONG,
        BODY_PT,
        theme::paper::red(),
    );
    let instruction_center = at((rule_x + text_left + 156.0) / 2.0, text_top + 61.0);
    let instruction_offset = egui::vec2(instruction.size().x / 2.0, overlay_baseline(&instruction));
    painter.galley(
        instruction_center - instruction_offset,
        instruction,
        theme::paper::red(),
    );

    painter.line_segment(
        [at(text_left, record_top), at(text_left + 156.0, record_top)],
        red,
    );
    // 栏宽与 Word/LaTeX 同源：按各行实际内容一次算定，联系人栏固定 8 em 不压缩。
    let record_columns = export::red_record_columns(rows);
    let columns = [
        export::RedRecordColumns::mm(record_columns.unit),
        export::RedRecordColumns::mm(record_columns.contact),
        export::RedRecordColumns::mm(record_columns.phone),
    ];
    let labels = ["承办单位：", "联系人：", "电话："];
    // 标签只在首行绘制；续行取值按标签宽度右移，与首行取值上下对齐。
    let label_galleys = labels.map(|label| {
        red_overlay_text(
            ui,
            metrics,
            label,
            theme::FONT_FANGSONG,
            BODY_PT,
            theme::paper::red(),
        )
    });
    for (row_index, row) in rows.iter().enumerate() {
        let y = record_top + 1.4 + row_index as f32 * (LINE_PT / MM);
        let mut x = text_left;
        for column in 0..3 {
            let label = &label_galleys[column];
            let label_width = label.size().x;
            if row_index == 0 {
                painter.galley(at(x, y), label.clone(), theme::paper::red());
            }
            // 姓名恒为 3em：2 字加全角空格两端对齐，4 字缩字号近似压缩（同 docx_name）。
            let chars = row[column].chars().count();
            let (text, pt) = if column == 1 && chars == 2 {
                let mut it = row[column].chars();
                (
                    format!("{}\u{2003}{}", it.next().unwrap(), it.next().unwrap()),
                    BODY_PT,
                )
            } else if column == 1 && chars == 4 {
                (row[column].clone(), BODY_PT * 0.75)
            } else {
                (row[column].clone(), BODY_PT)
            };
            let value = red_overlay_text(
                ui,
                metrics,
                &text,
                theme::FONT_FANGSONG,
                pt,
                theme::paper::ink(),
            );
            let value_pos = if column == 2 {
                at(x + columns[column], y) - egui::vec2(value.size().x, 0.0)
            } else {
                at(x, y) + egui::vec2(label_width, 0.0)
            };
            painter.galley(value_pos, value, theme::paper::ink());
            x += columns[column];
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_red_print_pages(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    layout_state: &RedPrintLayout,
    input: &DraftInput,
    display: &UnitDisplay,
    rows: &[[String; 3]],
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    elements: &ElementMarks,
) {
    for (page_index, page_layout) in layout_state.pages.iter().enumerate() {
        if page_index > 0 {
            ui.add_space(14.0);
        }
        // 呈批件是真分页的：每翻一页，页边的号栏另起一条。
        metrics.next_page();
        ui.horizontal(|ui| {
            let side = ((metrics.viewport - metrics.page) / 2.0).max(0.0);
            ui.add_space(side);
            let (page, _) = ui.allocate_exact_size(
                egui::vec2(metrics.page, metrics.page_height),
                egui::Sense::hover(),
            );
            ui.painter().rect(
                page,
                egui::CornerRadius::same(3),
                theme::paper::bg(),
                Stroke::new(1.0, theme::border()),
                egui::StrokeKind::Inside,
            );
            if page_index == 0 {
                paint_red_approval_overlay(ui, metrics, page, input, display, rows, elements);
            } else {
                let page_number = red_overlay_text(
                    ui,
                    metrics,
                    &format!("— {} —", page_index + 1),
                    theme::FONT_FANGSONG,
                    14.0,
                    theme::paper::ink(),
                );
                let center = page.min + egui::vec2(metrics.page / 2.0, metrics.mm(282.0));
                ui.painter().galley(
                    center - egui::vec2(page_number.size().x / 2.0, 0.0),
                    page_number,
                    theme::paper::ink(),
                );
            }

            for (fragment_index, fragment) in page_layout.fragments.iter().enumerate() {
                let rect_left = match fragment.align {
                    Align::Center => fragment.x - fragment.width / 2.0,
                    Align::Max => fragment.x - fragment.width,
                    Align::Min => fragment.x,
                };
                let top_left = page.min + egui::vec2(rect_left, fragment.y);
                let rect = egui::Rect::from_min_size(
                    top_left,
                    egui::vec2(fragment.width, fragment.visible_height),
                );
                if !fragment.source_segments.is_empty() {
                    let mut row_start = 0usize;
                    for (row_index, (placed, row_galley)) in fragment
                        .galley
                        .rows
                        .iter()
                        .zip(&fragment.justified)
                        .enumerate()
                    {
                        let row_y = red_row_top(metrics, row_index);
                        if row_y >= fragment.visible_height - 0.5 {
                            break;
                        }
                        let row_end = row_start + placed.glyphs.len();
                        // 行号认纸面上的行。呈批件是真分页的，正文块在每一页都从
                        // 左边 28mm 起排，页边那一列因此每页都在同一个位置上；
                        // 号码本身则一路数下去，不随翻页归零——看稿的人报的是
                        // 「第几行」，不是「第几页第几行」。
                        metrics.mark_row(
                            egui::Rect::from_min_size(
                                egui::pos2(top_left.x, top_left.y + row_y),
                                egui::vec2(fragment.width, placed.size.y),
                            ),
                            gutter::row_baseline(placed, top_left.y + row_y),
                            fragment
                                .source_segments
                                .iter()
                                .find(|segment| {
                                    segment.chars.start < row_end && row_start < segment.chars.end
                                })
                                .map(|segment| segment.source.clone()),
                        );
                        for segment in &fragment.source_segments {
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
                                let origin = top_left + egui::vec2(placed.pos.x, row_y);
                                egui::Rect::from_min_max(
                                    origin + egui::vec2(left, tint_top),
                                    origin + egui::vec2(right, tint_top + placed.size.y),
                                )
                                .expand2(egui::vec2(3.0, 1.0))
                            };
                            let ink_start = first_ink(&placed.glyphs, local_start..local_end);
                            let line_rect = row_rect(local_start, local_end);
                            let response = ui.interact(
                                line_rect,
                                egui::Id::new((
                                    "red-print-source-line",
                                    page_index,
                                    fragment_index,
                                    row_index,
                                    segment.source.start,
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
                                scroll_preview_to_rect(ui, line_rect);
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
                } else if let Some(range) = &fragment.range
                    && !range.is_empty()
                {
                    // 整块对应一行源码的片段（标题等）：逐行编号，号都指向那一行。
                    for (row_index, placed) in fragment.galley.rows.iter().enumerate() {
                        let row_y = red_row_top(metrics, row_index);
                        if row_y >= fragment.visible_height - 0.5 {
                            break;
                        }
                        metrics.mark_row(
                            egui::Rect::from_min_size(
                                egui::pos2(top_left.x, top_left.y + row_y),
                                egui::vec2(fragment.width, placed.size.y),
                            ),
                            gutter::row_baseline(placed, top_left.y + row_y),
                            Some(range.clone()),
                        );
                    }
                    let response = ui.interact(
                        rect,
                        egui::Id::new(("red-print-fragment", page_index, fragment_index)),
                        egui::Sense::click(),
                    );
                    if response.clicked() {
                        *clicked = Some(range.clone());
                    }
                    let anchored = anchor.is_some_and(|anchor| {
                        !anchor.is_empty() && anchor.start < range.end && range.start < anchor.end
                    });
                    if anchored && *scroll_to_anchor {
                        scroll_preview_to_rect(ui, rect);
                        *scroll_to_anchor = false;
                    }
                    if anchored || response.hovered() {
                        // 底色照首行的字框取中：整块也是字贴上沿、余量留在下面。
                        let tint_top = fragment
                            .galley
                            .rows
                            .first()
                            .map_or(0.0, |row| row_tint_offset(row));
                        ui.painter().rect_filled(
                            rect.translate(egui::vec2(0.0, tint_top))
                                .expand2(egui::vec2(3.0, 1.0)),
                            3.0,
                            if anchored {
                                theme::accent_soft()
                            } else {
                                theme::paper::hover_tint()
                            },
                        );
                    }
                }
                let anchor_pos = match fragment.align {
                    Align::Center | Align::Max | Align::Min => {
                        page.min + egui::vec2(fragment.x, fragment.y)
                    }
                };
                let painter = ui.painter().with_clip_rect(rect);
                if fragment.justified.is_empty() {
                    marks::paint_galley_marks(&painter, metrics, anchor_pos, &fragment.galley);
                    painter.galley(anchor_pos, fragment.galley.clone(), theme::paper::ink());
                } else {
                    // 两端对齐的正文按行画：每行是一个单独 galley，横向取原 galley
                    // 的行位置，纵向按固定行距（`red_row_top`），与分页、命中一致。
                    let boxes = marks::LineMarks::of(&fragment.galley.job);
                    let mut first_char = 0usize;
                    for (row_index, (placed, row)) in fragment
                        .galley
                        .rows
                        .iter()
                        .zip(&fragment.justified)
                        .enumerate()
                    {
                        let at =
                            anchor_pos + egui::vec2(placed.pos.x, red_row_top(metrics, row_index));
                        if let Some(boxes) = &boxes
                            && let Some(line) = row.rows.first()
                        {
                            boxes.paint_row(
                                &painter,
                                metrics,
                                first_char,
                                at + line.pos.to_vec2(),
                                &line.row,
                            );
                        }
                        painter.galley(at, row.clone(), theme::paper::ink());
                        first_char += placed.glyphs.len();
                    }
                }
            }

            for (slice_index, slice) in page_layout.tables.iter().enumerate() {
                // 底色压在表格线和字下面：先占位，量出各行的框再回填。
                let backdrops = slice
                    .rows
                    .iter()
                    .map(|_| ui.painter().add(egui::Shape::Noop))
                    .collect::<Vec<_>>();
                let origin = page.min + egui::vec2(slice.x, slice.y);
                let placed = slice
                    .table
                    .paint_rows(ui.painter(), metrics, origin, &slice.rows);
                for ((row, row_rect, baseline), backdrop) in placed.into_iter().zip(backdrops) {
                    let source = slice.sources[row].clone();
                    metrics.mark_row(row_rect, baseline, Some(source.clone()));
                    let response = ui.interact(
                        row_rect,
                        egui::Id::new(("red-print-table-row", page_index, slice_index, row)),
                        egui::Sense::click(),
                    );
                    if response.clicked() {
                        *clicked = Some(source.clone());
                    }
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let anchored = anchor.is_some_and(|anchor| {
                        !anchor.is_empty() && anchor.start < source.end && source.start < anchor.end
                    });
                    if anchored && *scroll_to_anchor {
                        scroll_preview_to_rect(ui, row_rect);
                        *scroll_to_anchor = false;
                    }
                    if anchored || response.hovered() {
                        ui.painter().set(
                            backdrop,
                            egui::epaint::RectShape::filled(
                                row_rect,
                                egui::CornerRadius::ZERO,
                                if anchored {
                                    theme::accent_soft()
                                } else {
                                    theme::paper::hover_tint()
                                },
                            ),
                        );
                    }
                }
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn red_approval_print_preview(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    display: &UnitDisplay,
    body: &[&LocatedBlock],
    attachments: &[Vec<&LocatedBlock>],
    title: &(String, Range<usize>),
    attachment_names: &[String],
    anchor: Option<&Range<usize>>,
    scroll_to_anchor: &mut bool,
    clicked: &mut Option<Range<usize>>,
    numbering: &NumberingConfig,
    markdown: &str,
    elements: &ElementMarks,
) {
    let (layout_state, rows) = red_build_print_layout(
        ui,
        metrics,
        input,
        display,
        body,
        title,
        attachment_names,
        numbering,
        markdown,
        elements,
    );
    paint_red_print_pages(
        ui,
        metrics,
        &layout_state,
        input,
        display,
        &rows,
        anchor,
        scroll_to_anchor,
        clicked,
        elements,
    );

    // 附件仍沿用现有的逐份分页渲染；正文和附件概要已经由上面的打印分页器处理。
    for (index, attachment) in attachments.iter().enumerate() {
        ui.add_space(14.0);
        sheet(ui, metrics, |ui| {
            let mut counters = [0usize; 4];
            line_block(
                ui,
                metrics,
                if attachments.len() == 1 {
                    "附件".to_string()
                } else {
                    format!("附件{}", index + 1)
                }
                .as_str(),
                theme::FONT_HEITI,
                BODY_PT,
                Align::LEFT,
            );
            for located in attachment {
                clickable_content_block(
                    ui,
                    metrics,
                    located,
                    markdown,
                    &mut counters,
                    true,
                    numbering,
                    anchor,
                    scroll_to_anchor,
                    clicked,
                );
            }
        });
    }
}
