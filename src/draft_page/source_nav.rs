//! Markdown 源码模式的标题目录与全文缩略图。
//!
//! 目录与版式预览共用标题收集、编号和字面。
//! 缩略图取编辑器本帧实际排出的视觉行；字号或编辑区宽度改变后，刻度与可见框
//! 仍能对上源码的滚动位置。

use crate::draft_page::{DraftPage, editor_id, line_ranges, navigator};
use crate::models::{NumberingConfig, TemplateKind};
use crate::storage;
use crate::theme;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

const MINIMAP_WIDTH: f32 = 94.0;
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
    drag_anchor_y: f32,
}

impl SourceMinimap {
    fn target_offset_for(content_height: f32, viewport_height: f32, fraction: f32) -> f32 {
        let total = content_height.max(1.0);
        let max_offset = (total - viewport_height).max(0.0);
        (fraction.clamp(0.0, 1.0) * total - viewport_height * 0.5).clamp(0.0, max_offset)
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
        rows.push(SourceMiniRow {
            top: output.galley_pos.y + placed.pos.y,
            width: (placed.size.x / editor_width).clamp(0.0, 1.0),
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
        if show_minimap {
            egui::Panel::right("source_minimap_v1")
                .default_size(MINIMAP_WIDTH)
                .resizable(false)
                .frame(theme::panel(theme::surface_sunk(), 6))
                .show(ui, |ui| self.source_minimap_ui(ui));
        }
        if show_outline {
            egui::Panel::left("source_outline_v1")
                .default_size(OUTLINE_WIDTH)
                .size_range(160.0..=300.0)
                .frame(theme::panel(theme::surface_sunk(), 8))
                .show(ui, |ui| self.source_outline_ui(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.markdown_editor(ui));
    }

    fn source_outline_ui(&mut self, ui: &mut egui::Ui) {
        let mut close = false;
        ui.horizontal(|ui| {
            ui.strong("目录");
            ui.weak(format!("{} 项", self.doc.source_outline.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = theme::icon_button(ui, theme::Icon::X, "收起 Markdown 目录").clicked();
                if !self.doc.source_outline.collapsed.is_empty()
                    && ui.small_button("全部展开").clicked()
                {
                    self.doc.source_outline.collapsed.clear();
                }
            });
        });
        ui.separator();
        if close {
            self.config.show_source_outline = false;
            let _ = storage::save(self.config);
        }
        if self.doc.source_outline.entries.is_empty() {
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
        let mut close = false;
        ui.horizontal(|ui| {
            ui.weak("缩略图");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = theme::icon_button(ui, theme::Icon::X, "收起 Markdown 缩略图").clicked();
            });
        });
        ui.separator();
        if close {
            self.config.show_source_minimap = false;
            let _ = storage::save(self.config);
        }
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ui.available_height()),
            egui::Sense::hover(),
        );
        let response = ui.interact(
            rect,
            egui::Id::new("gw_source_minimap"),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, theme::surface_sunk());
        let map = &self.doc.source_minimap;
        let total = map.content_height.max(1.0);
        let buckets = (rect.height() * ui.ctx().pixels_per_point())
            .ceil()
            .max(1.0) as usize;
        let mut marks = vec![(0.0f32, false); buckets];
        for row in &map.rows {
            if row.blank {
                continue;
            }
            let fraction = (row.top / total).clamp(0.0, 1.0);
            let index = ((fraction * buckets as f32) as usize).min(buckets - 1);
            let mark = &mut marks[index];
            mark.0 = mark.0.max(row.width.clamp(0.05, 1.0));
            mark.1 |= row.heading;
        }
        let margin = 5.0;
        let usable = (rect.width() - margin * 2.0).max(1.0);
        let bucket_height = rect.height() / buckets as f32;
        for (index, (width, heading)) in marks.into_iter().enumerate() {
            if width == 0.0 {
                continue;
            }
            let y = rect.top() + index as f32 * bucket_height;
            let width = (usable * width).max(if heading { 8.0 } else { 3.0 });
            let color = if heading {
                theme::accent()
            } else {
                theme::text_muted().gamma_multiply(0.55)
            };
            painter.line_segment(
                [
                    egui::pos2(rect.left() + margin, y),
                    egui::pos2(rect.left() + margin + width, y),
                ],
                egui::Stroke::new(if heading { 2.0 } else { 1.0 }, color),
            );
        }
        let top = rect.top() + rect.height() * (map.offset / total).clamp(0.0, 1.0);
        let height =
            (rect.height() * map.viewport_height / total).clamp(8.0, rect.height().max(8.0));
        let viewport = egui::Rect::from_min_size(
            egui::pos2(rect.left(), top.min(rect.bottom() - height)),
            egui::vec2(rect.width(), height),
        );
        painter.rect_filled(viewport, 2.0, theme::accent_soft().gamma_multiply(0.65));
        painter.rect_stroke(
            viewport,
            2.0,
            egui::Stroke::new(1.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
        let content_height = map.content_height;
        let viewport_height = map.viewport_height;
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            if response.drag_started() {
                self.doc.source_minimap.drag_anchor_y = if viewport.contains(pointer) {
                    pointer.y - viewport.center().y
                } else {
                    0.0
                };
            }
            if response.dragged() || (response.clicked() && !viewport.contains(pointer)) {
                let anchor = if response.dragged() {
                    self.doc.source_minimap.drag_anchor_y
                } else {
                    0.0
                };
                let fraction = ((pointer.y - anchor - rect.top()) / rect.height()).clamp(0.0, 1.0);
                self.doc.source_minimap.requested_offset = Some(SourceMinimap::target_offset_for(
                    content_height,
                    viewport_height,
                    fraction,
                ));
                ui.ctx().request_repaint();
            }
        }
        response.on_hover_text("点击跳转；按住拖动可连续定位");
    }
}

#[cfg(test)]
mod tests {
    use super::{OutlineNode, SourceMiniRow, SourceMinimap, SourceOutline, outline_row_ui};
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
    fn minimap_click_centers_a_long_document_and_clamps_its_ends() {
        assert_eq!(SourceMinimap::target_offset_for(1000.0, 200.0, 0.0), 0.0);
        assert_eq!(SourceMinimap::target_offset_for(1000.0, 200.0, 0.5), 400.0);
        assert_eq!(SourceMinimap::target_offset_for(1000.0, 200.0, 1.0), 800.0);
        assert_eq!(SourceMinimap::target_offset_for(120.0, 200.0, 0.8), 0.0);
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
