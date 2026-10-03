//! 标准词库同级拖放：落点校验、边缘滚动与只撤销顺序的快照。

use super::{TreeRow, VocabAction};
use crate::models::VocabularyEntry;
use crate::{theme, units};
use eframe::egui;
use units::SiblingPosition;

#[derive(Clone, Copy)]
pub(super) struct Payload(pub u64);

/// 暂时隐藏被拖单位的后代，不改持久化折叠状态；松手或取消后自然恢复。
pub(super) fn visible_rows(rows: &[TreeRow], dragged: Option<u64>) -> Vec<&TreeRow> {
    let mut hidden_below = None;
    rows.iter()
        .filter(|row| {
            if let Some(depth) = hidden_below {
                if row.depth > depth {
                    return false;
                }
                hidden_below = None;
            }
            if Some(row.id) == dragged && row.is_unit {
                hidden_below = Some(row.depth);
            }
            true
        })
        .collect()
}

pub(crate) struct SortUndo {
    id: u64,
    before: Vec<u64>,
    after: Vec<u64>,
}

impl SortUndo {
    pub(super) fn place(
        vocab: &mut Vec<VocabularyEntry>,
        id: u64,
        position: SiblingPosition,
    ) -> Result<Option<Self>, String> {
        let before = units::sibling_ids(vocab, id);
        if !before.contains(&id) {
            return Err("找不到要移动的词条".into());
        }
        if let SiblingPosition::Before(anchor) | SiblingPosition::After(anchor) = position {
            if anchor == id {
                return Ok(None);
            }
            if !before.contains(&anchor) {
                return Err("只能调整同级顺序".into());
            }
        }
        units::place_sibling(vocab, id, position)?;
        let after = units::sibling_ids(vocab, id);
        Ok((before != after).then_some(Self { id, before, after }))
    }

    pub(super) fn valid(&self, vocab: &[VocabularyEntry]) -> bool {
        units::sibling_ids(vocab, self.id) == self.after
    }

    pub(super) fn restore(&self, vocab: &mut Vec<VocabularyEntry>) -> Result<(), String> {
        if !self.valid(vocab) {
            return Err("同级清单已发生变化，无法撤销排序".into());
        }
        units::apply_sibling_order(vocab, &self.before);
        units::normalize(vocab);
        Ok(())
    }
}

pub(super) fn handle(ui: &mut egui::Ui, id: u64) -> egui::Response {
    let response = ui.add(
        theme::Icon::Grip
            .image()
            .tint(theme::text_muted())
            .sense(egui::Sense::drag()),
    );
    response.dnd_set_drag_payload(Payload(id));
    response
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("拖动调整同级顺序，Esc 取消")
}

/// 只接受同类别、同一上级的落点。无效落点只改变指针，不生成写入动作。
pub(super) fn drop_target(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    target: u64,
    vocab: &[VocabularyEntry],
) -> Option<VocabAction> {
    let response = ui.interact(
        rect,
        egui::Id::new(("vocab_drop", target)),
        egui::Sense::hover(),
    );
    let dragged = response.dnd_hover_payload::<Payload>()?;
    if dragged.0 == target {
        return None;
    }
    if !units::sibling_ids(vocab, dragged.0).contains(&target) {
        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::NotAllowed);
        return None;
    }
    let before = ui
        .ctx()
        .pointer_interact_pos()
        .is_none_or(|pointer| pointer.y <= rect.center().y);
    let y = if before { rect.top() } else { rect.bottom() };
    ui.painter().line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(2.0, theme::accent()),
    );
    response
        .dnd_release_payload::<Payload>()
        .map(|dragged| VocabAction::Place {
            id: dragged.0,
            position: if before {
                SiblingPosition::Before(target)
            } else {
                SiblingPosition::After(target)
            },
        })
}

/// 只在树的可见区域边缘滚动，避免窄窗口中拖到右侧编辑区也滚动。
pub(super) fn edge_scroll(ui: &mut egui::Ui, tree_rect: egui::Rect) {
    if !egui::DragAndDrop::has_payload_of_type::<Payload>(ui.ctx()) {
        return;
    }
    let viewport = ui.clip_rect().intersect(tree_rect);
    let Some(pointer) = ui
        .ctx()
        .pointer_interact_pos()
        .filter(|point| viewport.contains(*point))
    else {
        return;
    };
    let edge = 32.0;
    let speed = if pointer.y < viewport.top() + edge {
        (viewport.top() + edge - pointer.y) / edge
    } else if pointer.y > viewport.bottom() - edge {
        -(pointer.y - (viewport.bottom() - edge)) / edge
    } else {
        return;
    };
    let dt = ui.input(|input| input.stable_dt).min(0.05);
    ui.scroll_with_delta(egui::vec2(0.0, speed * 480.0 * dt));
    ui.ctx().request_repaint();
}

#[cfg(test)]
#[path = "vocabulary_drag_tests.rs"]
mod tests;
