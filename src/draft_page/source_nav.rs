//! Markdown 源码模式的标题目录与全文缩略图。
//!
//! 目录与版式预览共用标题收集、编号和字面。
//! 缩略图取编辑器本帧实际排出的视觉行；字号或编辑区宽度改变后，刻度与可见框
//! 仍能对上源码的滚动位置。

use crate::draft_page::{DraftPage, editor_id, line_ranges, navigator};
use crate::models::{NumberingConfig, TemplateKind};
use crate::theme;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

const MINIMAP_WIDTH: f32 = 94.0;
const MINIMAP_SCALE: f32 = 0.16;
const MINIMAP_BAR_WIDTH: f32 = 8.0;
const MINIMAP_VIEWPORT_HEIGHT: f32 = 96.0;
/// 拖到缩略图边缘后的自动滚动：速度与越界距离成正比，并有上限，避免长稿飞页。
const MINIMAP_EDGE_GAIN: f32 = 4.0;
const MINIMAP_EDGE_MAX_SPEED: f32 = 240.0;
const OUTLINE_WIDTH: f32 = 268.0;
const OUTLINE_ROW_HEIGHT: f32 = 28.0;
const OUTLINE_INDENT: f32 = 18.0;
const OUTLINE_ARROW_WIDTH: f32 = 16.0;

#[derive(Debug)]
struct OutlineNode {
    parent: Option<usize>,
    depth: usize,
    has_children: bool,
    key: String,
}

#[derive(Debug, Default)]
pub(crate) struct SourceOutline {
    fingerprint: u64,
    source_len: usize,
    numbering: Option<NumberingConfig>,
    kind: Option<TemplateKind>,
    entries: Vec<navigator::NavEntry>,
    nodes: Vec<OutlineNode>,
    collapsed: HashSet<String>,
}

impl SourceOutline {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn refresh(
        &mut self,
        markdown: &str,
        numbering: &NumberingConfig,
        kind: TemplateKind,
    ) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        markdown.hash(&mut hasher);
        let fingerprint = hasher.finish();
        if self.fingerprint == fingerprint
            && self.source_len == markdown.len()
            && self.numbering == Some(*numbering)
            && self.kind == Some(kind)
        {
            return;
        }
        self.fingerprint = fingerprint;
        self.source_len = markdown.len();
        self.numbering = Some(*numbering);
        self.kind = Some(kind);
        self.entries = navigator::collect_entries(markdown, numbering, kind);
        self.nodes = outline_nodes(&self.entries);
        let keys = self
            .nodes
            .iter()
            .map(|node| node.key.as_str())
            .collect::<HashSet<_>>();
        self.collapsed.retain(|key| keys.contains(key.as_str()));
    }

    fn current(&self, byte: usize) -> Option<usize> {
        self.entries
            .iter()
            .rposition(|entry| entry.line.start <= byte)
    }

    fn contains_heading_at(&self, byte: usize) -> bool {
        self.entries.iter().any(|entry| entry.line.start == byte)
    }

    fn visible_indices(&self) -> Vec<usize> {
        let mut visible = vec![false; self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate() {
            visible[index] = node.parent.is_none_or(|parent| {
                visible[parent] && !self.collapsed.contains(&self.nodes[parent].key)
            });
        }
        visible
            .into_iter()
            .enumerate()
            .filter_map(|(index, show)| show.then_some(index))
            .collect()
    }

    fn visible_current(&self, byte: usize) -> Option<usize> {
        let mut index = self.current(byte)?;
        let mut ancestor = self.nodes[index].parent;
        while let Some(parent) = ancestor {
            if self.collapsed.contains(&self.nodes[parent].key) {
                index = parent;
            }
            ancestor = self.nodes[parent].parent;
        }
        Some(index)
    }

    fn toggle(&mut self, index: usize) {
        let node = &self.nodes[index];
        if !node.has_children {
            return;
        }
        if !self.collapsed.insert(node.key.clone()) {
            self.collapsed.remove(&node.key);
        }
    }
}

/// 按最近的上级标题建树；级别跳跃时只缩进一个实际父节点，缺少文档标题时
/// 第一层也从左边起排。稳定键让正文编辑导致的行号移动不丢掉折叠状态。
fn outline_nodes(entries: &[navigator::NavEntry]) -> Vec<OutlineNode> {
    let mut nodes: Vec<OutlineNode> = Vec::with_capacity(entries.len());
    let mut stack: Vec<usize> = Vec::new();
    let mut sibling_counts: HashMap<String, usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&ancestor| entries[ancestor].level >= entry.level)
        {
            stack.pop();
        }
        let parent = stack.last().copied();
        let depth = parent.map_or(0, |parent| nodes[parent].depth + 1);
        let base = format!(
            "{}\0{}\0{}",
            parent.map_or("", |parent| nodes[parent].key.as_str()),
            entry.level,
            entry.text
        );
        let occurrence = sibling_counts.entry(base.clone()).or_default();
        let key = format!("{base}\0{occurrence}");
        *occurrence += 1;
        if let Some(parent) = parent {
            nodes[parent].has_children = true;
        }
        nodes.push(OutlineNode {
            parent,
            depth,
            has_children: false,
            key,
        });
        stack.push(index);
    }
    nodes
}

/// 一整行都是命中区；标题区跳转，箭头区收放子标题。悬停与当前章节均给出
/// 可见底色，标题字形仍取预览目录那套。
fn outline_row_ui(
    ui: &mut egui::Ui,
    entry: &navigator::NavEntry,
    node: &OutlineNode,
    collapsed: bool,
    current: bool,
) -> (egui::Response, bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), OUTLINE_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let response = ui.interact(
        rect,
        ui.id().with(("source_outline_node", &node.key)),
        egui::Sense::click(),
    );
    let arrow_x = rect.left() + 5.0 + node.depth.min(5) as f32 * OUTLINE_INDENT;
    let arrow_rect = egui::Rect::from_min_max(
        egui::pos2(arrow_x, rect.top()),
        egui::pos2(arrow_x + OUTLINE_ARROW_WIDTH, rect.bottom()),
    );
    let hovered = response.hovered();
    let painter = ui.painter().with_clip_rect(rect);
    if hovered || current {
        let fill = theme::accent_soft().gamma_multiply(if hovered { 0.8 } else { 0.45 });
        painter.rect_filled(rect.shrink2(egui::vec2(1.0, 0.0)), 4.0, fill);
    }
    let color = if hovered || current {
        theme::accent()
    } else if entry.level <= 2 {
        theme::text()
    } else {
        theme::text_soft()
    };
    if node.has_children {
        let center = arrow_rect.center();
        let points = if collapsed {
            vec![
                egui::pos2(center.x - 2.0, center.y - 4.0),
                egui::pos2(center.x + 2.0, center.y),
                egui::pos2(center.x - 2.0, center.y + 4.0),
            ]
        } else {
            vec![
                egui::pos2(center.x - 4.0, center.y - 2.0),
                egui::pos2(center.x + 4.0, center.y - 2.0),
                egui::pos2(center.x, center.y + 2.0),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    }
    let label = navigator::label_text(entry);
    let text_x = arrow_rect.right() + 2.0;
    let max_width = (rect.right() - text_x - 4.0).max(1.0);
    let font = egui::FontId::new(
        navigator::label_size(entry.level),
        navigator::label_family(entry),
    );
    let truncated = hovered
        && painter
            .layout_no_wrap(label.clone(), font.clone(), color)
            .size()
            .x
            > max_width;
    let mut job = egui::text::LayoutJob::simple_singleline(label.clone(), font, color);
    job.wrap.max_width = max_width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(text_x, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    let toggle = node.has_children
        && response.clicked()
        && ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| arrow_rect.contains(p));
    let arrow_hovered = node.has_children
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|p| arrow_rect.contains(p));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let response = if arrow_hovered {
        response.on_hover_text(if collapsed {
            "展开下级目录"
        } else {
            "收起下级目录"
        })
    } else if truncated {
        response.on_hover_text(label)
    } else {
        response
    };
    (response, toggle)
}

/// 编辑器排出的一个视觉行。自动换行后的续行也各有一条刻度。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SourceMiniRow {
    pub(crate) top: f32,
    pub(crate) height: f32,
    /// 行首缩进与行宽，都是占编辑器宽度的比例。
    pub(crate) left: f32,
    pub(crate) width: f32,
    pub(crate) source_offset: usize,
    pub(crate) heading: bool,
    pub(crate) blank: bool,
}

#[derive(Debug, Default)]
pub(crate) struct SourceMinimap {
    pub(crate) rows: Vec<SourceMiniRow>,
    pub(crate) visible: bool,
    pub(crate) content_height: f32,
    pub(crate) viewport_height: f32,
    pub(crate) offset: f32,
    pub(crate) requested_offset: Option<f32>,
    pub(crate) mini_scroll: f32,
    last_editor_offset: Option<f32>,
    drag_anchor_y: f32,
    dragging: bool,
}

#[derive(Debug, Clone, Copy)]
struct MiniLayout {
    height: f32,
    map_height: f32,
    box_height: f32,
    editor_max: f32,
}

impl MiniLayout {
    fn new(map: &SourceMinimap, height: f32) -> Self {
        let height = height.max(1.0);
        let box_height = MINIMAP_VIEWPORT_HEIGHT.min(height);
        Self {
            height,
            map_height: (map.content_height * MINIMAP_SCALE).max(box_height),
            box_height,
            editor_max: (map.content_height - map.viewport_height).max(0.0),
        }
    }

    fn max_scroll(self) -> f32 {
        (self.map_height - self.height).max(0.0)
    }

    fn box_top(self, editor_offset: f32) -> f32 {
        if self.editor_max <= 0.0 {
            0.0
        } else {
            (editor_offset / self.editor_max).clamp(0.0, 1.0) * (self.map_height - self.box_height)
        }
    }

    fn editor_offset_for(self, box_top: f32) -> f32 {
        let travel = self.map_height - self.box_height;
        if travel <= 0.0 {
            0.0
        } else {
            (box_top / travel).clamp(0.0, 1.0) * self.editor_max
        }
    }
}

impl SourceMinimap {
    /// 正文位置变动且可见框将离开缩略图时才跟随；单独滚动缩略图时保留用户所看的段落。
    fn follow_editor(&mut self, layout: MiniLayout) {
        self.mini_scroll = self.mini_scroll.clamp(0.0, layout.max_scroll());
        if self.dragging {
            // 拖动已经在控制缩略图的滚动位置，正文回传的 offset 只用于下次比较。
            self.last_editor_offset = Some(self.offset);
            return;
        }
        if self
            .last_editor_offset
            .is_some_and(|previous| (previous - self.offset).abs() < 0.5)
        {
            return;
        }
        self.last_editor_offset = Some(self.offset);
        let top = layout.box_top(self.offset);
        let margin = ((layout.height - layout.box_height) * 0.25).clamp(0.0, 18.0);
        if top < self.mini_scroll + margin
            || top + layout.box_height > self.mini_scroll + layout.height - margin
        {
            self.mini_scroll = top + layout.box_height * 0.5 - layout.height * 0.5;
        }
        self.mini_scroll = self.mini_scroll.clamp(0.0, layout.max_scroll());
    }

    /// 手指先带着可见框走；碰到边缘后，可见框停在边缘，内容按越界距离匀速
    /// 滑过。位移由秒数而非帧数决定，低帧率也不会突然加速。
    fn drag_to(&mut self, layout: MiniLayout, pointer_y: f32, dt: f32) -> f32 {
        let desired_top = pointer_y - self.drag_anchor_y;
        let overflow = if desired_top < 0.0 {
            desired_top
        } else {
            (desired_top + layout.box_height - layout.height).max(0.0)
        };
        let speed =
            (overflow * MINIMAP_EDGE_GAIN).clamp(-MINIMAP_EDGE_MAX_SPEED, MINIMAP_EDGE_MAX_SPEED);
        self.mini_scroll =
            (self.mini_scroll + speed * dt.clamp(0.0, 1.0 / 30.0)).clamp(0.0, layout.max_scroll());
        let visible_top = desired_top.clamp(0.0, (layout.height - layout.box_height).max(0.0));
        layout.editor_offset_for(self.mini_scroll + visible_top)
    }

    pub(crate) fn visible_source_offset(&self) -> usize {
        let reading_y = self.offset + self.viewport_height * 0.25;
        let index = self.rows.partition_point(|row| row.top < reading_y);
        match (
            index.checked_sub(1).and_then(|i| self.rows.get(i)),
            self.rows.get(index),
        ) {
            (Some(before), Some(after)) if reading_y - before.top <= after.top - reading_y => {
                before.source_offset
            }
            (_, Some(after)) => after.source_offset,
            (Some(before), None) => before.source_offset,
            (None, None) => 0,
        }
    }

    pub(crate) fn update(
        &mut self,
        mut rows: Vec<SourceMiniRow>,
        offset: f32,
        content_height: f32,
        viewport_top: f32,
        viewport_height: f32,
    ) {
        for row in &mut rows {
            row.top = row.top - viewport_top + offset;
        }
        self.rows = rows;
        self.offset = offset;
        self.content_height = content_height.max(viewport_height);
        self.viewport_height = viewport_height;
    }
}

pub(crate) fn capture_source_rows(
    text: &str,
    output: &egui::text_edit::TextEditOutput,
    outline: &SourceOutline,
) -> Vec<SourceMiniRow> {
    let ranges = line_ranges(text);
    let editor_width = output.response.rect.width().max(1.0);
    let mut source_line = 0usize;
    let mut rows = Vec::with_capacity(output.galley.rows.len());
    for placed in &output.galley.rows {
        let line_start = ranges[source_line.min(ranges.len() - 1)].start;
        let left = placed.glyphs.first().map_or(0.0, |glyph| glyph.pos.x);
        rows.push(SourceMiniRow {
            top: output.galley_pos.y + placed.pos.y,
            height: placed.size.y,
            left: (left / editor_width).clamp(0.0, 1.0),
            width: ((placed.size.x - left).max(0.0) / editor_width).clamp(0.0, 1.0),
            source_offset: line_start,
            heading: outline.contains_heading_at(line_start),
            blank: placed.glyphs.is_empty(),
        });
        if placed.ends_with_newline {
            source_line += 1;
        }
    }
    rows
}

impl DraftPage<'_> {
    /// 源码模式自己的两列导航；不会改变实时排版、预览或对照模式的布局。
    pub(crate) fn source_editor_ui(&mut self, ui: &mut egui::Ui) {
        self.doc.source_outline.refresh(
            &self.doc.generated_markdown,
            &self.config.numbering,
            self.doc.draft.kind,
        );
        // 外层还可能开着文档要素和审校抽屉。宽度不够时临时收起导航，保住正文
        // 的可编辑宽度；配置不变，窗口放大后自动回来。
        let available = ui.available_width();
        let show_minimap = self.config.show_source_minimap && available >= 360.0;
        let show_outline = self.config.show_source_outline
            && available >= if show_minimap { 600.0 } else { 450.0 };
        self.doc.source_minimap.visible = show_minimap;
        // 大纲、正文、小地图共用同一张卡片，之间不留缝，像 Sublime 一样贴着正文。
        let pad = theme::PANE_PADDING;
        let card = theme::pane().inner_margin(egui::Margin {
            left: if show_outline { 0 } else { pad },
            right: if show_minimap { 0 } else { pad },
            top: pad,
            bottom: pad,
        });
        egui::CentralPanel::default().frame(card).show(ui, |ui| {
            if show_outline {
                egui::Panel::left("source_outline_v1")
                    .default_size(OUTLINE_WIDTH)
                    .size_range(160.0..=300.0)
                    .frame(egui::Frame::new().inner_margin(egui::Margin {
                        left: pad,
                        right: 4,
                        top: 0,
                        bottom: 0,
                    }))
                    .show(ui, |ui| self.source_outline_ui(ui));
            }
            if show_minimap {
                egui::Panel::right("source_minimap_v1")
                    .default_size(MINIMAP_WIDTH)
                    .resizable(false)
                    .frame(egui::Frame::new())
                    .show_separator_line(false)
                    .show(ui, |ui| self.source_minimap_ui(ui));
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::new().inner_margin(egui::Margin {
                    left: if show_outline { pad } else { 0 },
                    ..egui::Margin::ZERO
                }))
                .show(ui, |ui| self.markdown_editor(ui));
        });
    }

    fn source_outline_ui(&mut self, ui: &mut egui::Ui) {
        if self.doc.source_outline.len() == 0 {
            ui.add_space(8.0);
            ui.weak("还没有 Markdown 标题");
            ui.weak("用 #、##、### 等标记标题");
            return;
        }
        let current_byte = self
            .doc
            .pending_source_jump
            .or_else(|| {
                ui.ctx()
                    .memory(|memory| memory.has_focus(editor_id()))
                    .then(|| {
                        crate::draft_page::markdown::editor_cursor(
                            ui.ctx(),
                            &self.doc.generated_markdown,
                        )
                    })
                    .flatten()
            })
            .unwrap_or_else(|| self.doc.source_minimap.visible_source_offset());
        let current = self.doc.source_outline.visible_current(current_byte);
        let visible = self.doc.source_outline.visible_indices();
        let mut jump = None;
        let mut toggle = None;
        egui::ScrollArea::vertical()
            .id_salt("source_outline_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for index in visible {
                    let entry = &self.doc.source_outline.entries[index];
                    let node = &self.doc.source_outline.nodes[index];
                    let (response, arrow_clicked) = outline_row_ui(
                        ui,
                        entry,
                        node,
                        self.doc.source_outline.collapsed.contains(&node.key),
                        current == Some(index),
                    );
                    if arrow_clicked {
                        toggle = Some(index);
                    } else if response.clicked() {
                        jump = Some(entry.line.start);
                    }
                }
            });
        if let Some(index) = toggle {
            self.doc.source_outline.toggle(index);
            ui.ctx().request_repaint();
        }
        if let Some(byte) = jump {
            self.doc.pending_source_selection = None;
            self.doc.pending_source_jump = Some(byte);
            ui.ctx().request_repaint();
        }
    }

    fn source_minimap_ui(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ui.available_height()),
            egui::Sense::hover(),
        );
        let response = ui.interact(
            rect,
            egui::Id::new("gw_source_minimap"),
            egui::Sense::click_and_drag(),
        );
        let map = &mut self.doc.source_minimap;
        let layout = MiniLayout::new(map, rect.height());
        map.follow_editor(layout);
        if response.hovered() {
            let wheel = ui.input(|input| input.smooth_scroll_delta.y);
            if wheel != 0.0 {
                map.mini_scroll = (map.mini_scroll - wheel).clamp(0.0, layout.max_scroll());
                ui.ctx()
                    .input_mut(|input| input.smooth_scroll_delta.y = 0.0);
            }
        }
        let hit_top = rect.top() + layout.box_top(map.offset) - map.mini_scroll;
        let hit_viewport = egui::Rect::from_min_size(
            egui::pos2(rect.left(), hit_top),
            egui::vec2(rect.width(), layout.box_height),
        );
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            if response.drag_started() {
                map.dragging = true;
                let origin = ui
                    .input(|input| input.pointer.press_origin())
                    .unwrap_or(pointer);
                map.drag_anchor_y = if hit_viewport.contains(origin) {
                    origin.y - hit_viewport.top()
                } else {
                    layout.box_height * 0.5
                };
            }
            if response.dragged() {
                map.dragging = true;
                let dt = ui.input(|input| input.stable_dt.max(input.predicted_dt));
                map.requested_offset = Some(map.drag_to(layout, pointer.y - rect.top(), dt));
                ui.ctx().request_repaint();
            } else if response.clicked() && !hit_viewport.contains(pointer) {
                let top = pointer.y - rect.top() + map.mini_scroll - layout.box_height * 0.5;
                map.requested_offset = Some(layout.editor_offset_for(top));
                ui.ctx().request_repaint();
            }
        }
        if response.drag_stopped() {
            map.dragging = false;
            map.last_editor_offset = Some(map.offset);
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::surface());
        // 每个视觉行画成与正文行等比缩小的实心条：行与行首尾相接，像 Sublime
        // 那样和左侧正文一一对应，不再是稀疏的细线。
        let margin = 4.0;
        let usable = (rect.width() - margin - MINIMAP_BAR_WIDTH - 3.0).max(1.0);
        let first = map
            .rows
            .partition_point(|row| (row.top + row.height) * MINIMAP_SCALE < map.mini_scroll);
        for row in map.rows[first..].iter() {
            let y = row.top * MINIMAP_SCALE - map.mini_scroll;
            if y > rect.height() {
                break;
            }
            if row.blank {
                continue;
            }
            let pitch = (row.height * MINIMAP_SCALE).max(1.0);
            let bar_height = if row.heading {
                (pitch * 0.9).max(2.0)
            } else {
                (pitch * 0.68).max(1.5)
            };
            let x0 = rect.left() + margin + usable * row.left.clamp(0.0, 0.95);
            let x1 = (x0 + (usable * row.width).max(if row.heading { 8.0 } else { 3.0 }))
                .min(rect.left() + margin + usable);
            let color = if row.heading {
                theme::accent()
            } else {
                theme::text_muted().gamma_multiply(0.5)
            };
            let bar = egui::Rect::from_min_max(
                egui::pos2(x0, rect.top() + y + (pitch - bar_height) * 0.5),
                egui::pos2(x1, rect.top() + y + (pitch + bar_height) * 0.5),
            );
            painter.rect_filled(bar, 0.0, color);
        }
        let top = rect.top() + layout.box_top(map.requested_offset.unwrap_or(map.offset))
            - map.mini_scroll;
        let viewport = egui::Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(rect.width() - MINIMAP_BAR_WIDTH - 2.0, layout.box_height),
        );
        painter.rect_filled(viewport, 0.0, theme::accent_soft().gamma_multiply(0.55));

        // 右侧细滚动条：对应整篇正文，独立于缩略图自己的滚动。
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.right() - MINIMAP_BAR_WIDTH, rect.top()),
            rect.right_bottom(),
        );
        let bar = ui.interact(
            track,
            egui::Id::new("gw_source_minimap_bar"),
            egui::Sense::click_and_drag(),
        );
        let ratio = (map.viewport_height / map.content_height.max(1.0)).clamp(0.0, 1.0);
        let thumb_height =
            (track.height() * ratio).clamp(20.0_f32.min(track.height()), track.height());
        let travel = (track.height() - thumb_height).max(0.0);
        if (bar.dragged() || bar.clicked())
            && layout.editor_max > 0.0
            && travel > 0.0
            && let Some(pointer) = bar.interact_pointer_pos()
        {
            let thumb_top = (pointer.y - track.top() - thumb_height * 0.5).clamp(0.0, travel);
            map.requested_offset = Some(thumb_top / travel * layout.editor_max);
            ui.ctx().request_repaint();
        }
        let shown = map.requested_offset.unwrap_or(map.offset);
        let thumb_top = if layout.editor_max > 0.0 {
            (shown / layout.editor_max).clamp(0.0, 1.0) * travel
        } else {
            0.0
        };
        let active = bar.hovered() || bar.dragged();
        painter.rect_filled(track, 0.0, theme::text_muted().gamma_multiply(0.08));
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(track.left() + 1.0, track.top() + thumb_top),
                egui::vec2(track.width() - 2.0, thumb_height),
            ),
            2.0,
            theme::text_muted().gamma_multiply(if active { 0.75 } else { 0.45 }),
        );
        response.on_hover_text("滚轮浏览全文；点击或拖动定位");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MiniLayout, OutlineNode, SourceMiniRow, SourceMinimap, SourceOutline, outline_row_ui,
    };
    use crate::draft_page::navigator;
    use crate::models::{HeadingNumbering, NumberingConfig, TemplateKind};
    use eframe::egui;

    fn labels(outline: &SourceOutline) -> Vec<String> {
        outline.entries.iter().map(navigator::label_text).collect()
    }

    #[test]
    fn outline_tracks_heading_ranges_and_ignores_plain_hashes() {
        let text = "# 标题\n\n正文 # 不是标题\n\n## 总体要求\n内容\n### 子节\n";
        let mut outline = SourceOutline::default();
        outline.refresh(
            text,
            &NumberingConfig::default(),
            TemplateKind::OfficialLetter,
        );
        assert_eq!(outline.entries.len(), 3);
        assert_eq!(labels(&outline), vec!["标题", "一、总体要求", "（一）子节"]);
        assert_eq!(outline.entries[1].level, 2);
        assert_eq!(outline.current(text.find("内容").unwrap()), Some(1));
        assert_eq!(outline.entries[2].line.start, text.find("###").unwrap());
    }

    #[test]
    fn outline_uses_preview_labels_for_numbering_and_attachments() {
        let text = "# 标题\n\n## 一、正文标题\n\n<!--附件-->\n\n# 附件标题\n\n## 附件小节\n";
        let mut outline = SourceOutline::default();
        let mut numbering = NumberingConfig::default();
        outline.refresh(text, &numbering, TemplateKind::OfficialLetter);
        assert_eq!(
            labels(&outline),
            vec!["标题", "一、正文标题", "【附件】附件标题", "一、附件小节"]
        );
        numbering.heading1 = HeadingNumbering::ChapterDigit;
        outline.refresh(text, &numbering, TemplateKind::OfficialLetter);
        assert_eq!(
            labels(&outline),
            vec![
                "标题",
                "第1章　正文标题",
                "【附件】附件标题",
                "第1章　附件小节"
            ]
        );
    }

    #[test]
    fn outline_switches_to_research_preview_numbering_when_kind_changes() {
        let text = "<!-- [正文] -->\n\n# 研究报告\n\n## 背景\n\n### 方法\n";
        let mut outline = SourceOutline::default();
        outline.refresh(
            text,
            &NumberingConfig::default(),
            TemplateKind::OfficialLetter,
        );
        assert_eq!(labels(&outline), vec!["研究报告", "一、背景", "（一）方法"]);
        outline.refresh(
            text,
            &NumberingConfig::default(),
            TemplateKind::ResearchReport,
        );
        assert_eq!(
            labels(&outline),
            vec!["研究报告", "第1章　背景", "1.1 方法"]
        );
    }

    #[test]
    fn outline_displays_visible_heading_text_without_markdown_marks() {
        let text = "# 关于**改革**的通知\n\n## 完善\\_机制与**配套**措施\n";
        let mut outline = SourceOutline::default();
        outline.refresh(
            text,
            &NumberingConfig::default(),
            TemplateKind::OfficialLetter,
        );
        assert_eq!(
            labels(&outline),
            vec!["关于改革的通知", "一、完善_机制与配套措施"]
        );
    }

    #[test]
    fn outline_tree_indents_from_real_parents_and_keeps_folds_after_edits() {
        let text = "# 标题\n## 第一章\n### 第一节\n#### 小节\n## 第二章\n";
        let mut outline = SourceOutline::default();
        let numbering = NumberingConfig::default();
        outline.refresh(text, &numbering, TemplateKind::OfficialLetter);
        assert_eq!(
            outline
                .nodes
                .iter()
                .map(|node| node.depth)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 1]
        );
        assert!(outline.nodes[1].has_children);
        outline.toggle(1);
        assert_eq!(outline.visible_indices(), vec![0, 1, 4]);
        assert_eq!(outline.visible_current(text.find("小节").unwrap()), Some(1));
        outline.refresh(
            &format!("前言。\n{text}"),
            &numbering,
            TemplateKind::OfficialLetter,
        );
        assert_eq!(outline.visible_indices(), vec![0, 1, 4]);
        outline.toggle(1);
        assert_eq!(outline.visible_indices(), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn research_chapter_without_report_title_starts_at_tree_root() {
        let text = "<!-- [正文] -->\n## 项目概述\n### 建设目标\n#### 任务分解\n## 实施路径\n";
        let mut outline = SourceOutline::default();
        outline.refresh(
            text,
            &NumberingConfig::default(),
            TemplateKind::ResearchReport,
        );
        assert_eq!(
            outline
                .nodes
                .iter()
                .map(|node| node.depth)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 0]
        );
        outline.toggle(0);
        assert_eq!(outline.visible_indices(), vec![0, 3]);
    }

    #[test]
    fn hovering_an_outline_row_paints_a_highlight() {
        let ctx = egui::Context::default();
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let entry = navigator::NavEntry {
            level: 2,
            number: Some("第1章　".to_string()),
            text: "项目概述".to_string(),
            is_attachment_title: false,
            line: 0..10,
            family: crate::theme::FONT_HEITI,
        };
        let node = OutlineNode {
            parent: None,
            depth: 0,
            has_children: true,
            key: "chapter".to_string(),
        };
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0));
        let paint = |events: Vec<egui::Event>| {
            let mut hovered = false;
            let mut clicked = false;
            let mut toggle = false;
            let mut rect = egui::Rect::NOTHING;
            let output = ctx.clone().run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let (response, arrow_clicked) =
                            outline_row_ui(ui, &entry, &node, false, false);
                        rect = response.rect;
                        hovered = response.hovered();
                        clicked = response.clicked();
                        toggle = arrow_clicked;
                    });
                },
            );
            (hovered, clicked, toggle, rect, output.shapes)
        };
        let (hovered, _, _, rect, _) = paint(Vec::new());
        assert!(!hovered);
        let pointer = rect.center();
        let (hovered, _, _, _, shapes) = paint(vec![egui::Event::PointerMoved(pointer)]);
        assert!(hovered);
        let highlight = crate::theme::accent_soft().gamma_multiply(0.8);
        assert!(shapes.iter().any(|shape| matches!(
            &shape.shape,
            egui::epaint::Shape::Rect(rect) if rect.fill == highlight
        )));
        let click = |position| {
            let _ = paint(vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            paint(vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }])
        };
        let (_, clicked, toggle, _, _) = click(egui::pos2(rect.left() + 13.0, rect.center().y));
        assert!(clicked && toggle, "点箭头应收放下级目录");
        let (_, clicked, toggle, _, _) = click(egui::pos2(rect.left() + 75.0, rect.center().y));
        assert!(clicked && !toggle, "点标题应跳转，而不是折叠目录");
    }

    #[test]
    fn minimap_keeps_a_fixed_viewport_and_scrolls_past_the_panel_height() {
        let mut map = SourceMinimap {
            content_height: 12_000.0,
            viewport_height: 600.0,
            offset: 9_000.0,
            ..Default::default()
        };
        let layout = MiniLayout::new(&map, 500.0);
        assert_eq!(layout.box_height, 96.0);
        assert!(layout.map_height > layout.height);
        assert!((layout.editor_offset_for(layout.box_top(5_700.0)) - 5_700.0).abs() < 0.1);
        map.follow_editor(layout);
        assert!(map.mini_scroll > 0.0);
        let box_top = layout.box_top(map.offset) - map.mini_scroll;
        assert!(box_top >= 0.0 && box_top + layout.box_height <= layout.height);
        map.mini_scroll = 0.0;
        map.follow_editor(layout);
        assert_eq!(map.mini_scroll, 0.0, "用户滚动缩略图后不应被自动拉回");
    }

    #[test]
    fn minimap_drag_tracks_pointer_and_edge_speed_uses_distance_and_time() {
        let mut map = SourceMinimap {
            content_height: 12_000.0,
            viewport_height: 600.0,
            mini_scroll: 500.0,
            drag_anchor_y: 48.0,
            ..Default::default()
        };
        let layout = MiniLayout::new(&map, 500.0);
        let inside = map.drag_to(layout, 200.0, 1.0 / 60.0);
        assert_eq!(map.mini_scroll, 500.0);
        let lower = map.drag_to(layout, 220.0, 1.0 / 60.0);
        assert!(lower > inside, "框在可见范围内应直接跟随指针");
        assert_eq!(map.mini_scroll, 500.0);

        let edge = layout.height - layout.box_height + map.drag_anchor_y;
        let _ = map.drag_to(layout, edge, 1.0 / 60.0);
        assert_eq!(map.mini_scroll, 500.0, "刚碰到边缘时速度应从零开始");
        let _ = map.drag_to(layout, edge + 10.0, 1.0 / 60.0);
        let one_frame = map.mini_scroll - 500.0;
        assert!((one_frame - 40.0 / 60.0).abs() < 0.01);
        let _ = map.drag_to(layout, edge + 10.0, 1.0 / 30.0);
        assert!((map.mini_scroll - 500.0 - one_frame * 3.0).abs() < 0.01);

        map.mini_scroll = 500.0;
        let _ = map.drag_to(layout, edge + 500.0, 1.0);
        assert!((map.mini_scroll - 508.0).abs() < 0.01, "停顿一帧不应飞页");
        map.dragging = true;
        map.offset = 9_000.0;
        map.follow_editor(layout);
        assert_eq!(map.mini_scroll, 508.0, "拖动时正文回传不应把地图拉走");

        map.mini_scroll = 500.0;
        let _ = map.drag_to(layout, 38.0, 1.0 / 60.0);
        assert!(map.mini_scroll < 500.0, "上边缘应同样平滑地反向滚动");
    }

    #[test]
    fn visible_position_follows_editor_layout_rows() {
        let map = SourceMinimap {
            rows: vec![
                SourceMiniRow {
                    top: 10.0,
                    source_offset: 0,
                    ..Default::default()
                },
                SourceMiniRow {
                    top: 140.0,
                    source_offset: 25,
                    ..Default::default()
                },
                SourceMiniRow {
                    top: 350.0,
                    source_offset: 80,
                    ..Default::default()
                },
            ],
            offset: 300.0,
            viewport_height: 200.0,
            ..Default::default()
        };
        assert_eq!(map.visible_source_offset(), 80);
    }
}
