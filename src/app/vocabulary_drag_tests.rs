//! 用真实指针事件验证落点限制，并检查排序与撤销不会覆盖词条资料。

use super::*;
use crate::models::VocabularyCategory;

fn vocabulary() -> Vec<VocabularyEntry> {
    let mut vocab = Vec::new();
    for (id, code, parent) in [
        (1, "00", ""),
        (2, "0001", "00"),
        (3, "0002", "00"),
        (4, "000101", "0001"),
        (8, "01", ""),
    ] {
        vocab.push(VocabularyEntry {
            id,
            code: code.into(),
            parent: parent.into(),
            canonical: format!("单位{id}"),
            category: VocabularyCategory::Unit,
            ..Default::default()
        });
    }
    for (id, unit) in [(5, "0001"), (6, "0001"), (7, "0002")] {
        vocab.push(VocabularyEntry {
            id,
            unit: unit.into(),
            canonical: format!("人员{id}"),
            phone: "123456".into(),
            category: VocabularyCategory::Person,
            ..Default::default()
        });
    }
    units::normalize(&mut vocab);
    vocab
}

#[test]
fn sibling_sort_moves_entire_subtree_and_preserves_unit_codes() {
    let mut vocab = vocabulary();
    let before = vocab.clone();
    let undo = SortUndo::place(&mut vocab, 2, SiblingPosition::After(3))
        .unwrap()
        .unwrap();
    assert_eq!(
        vocab.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        [1, 3, 7, 2, 5, 6, 4, 8]
    );
    for entry in &vocab {
        let mut previous = before
            .iter()
            .find(|previous| previous.id == entry.id)
            .unwrap()
            .clone();
        previous.sort_order = entry.sort_order;
        assert_eq!(entry, &previous);
    }
    undo.restore(&mut vocab).unwrap();
    assert_eq!(
        vocab.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        before.iter().map(|entry| entry.id).collect::<Vec<_>>()
    );
}

#[test]
fn person_sort_and_undo_preserve_later_edits() {
    let mut vocab = vocabulary();
    let undo = SortUndo::place(&mut vocab, 5, SiblingPosition::After(6))
        .unwrap()
        .unwrap();
    assert_eq!(units::sibling_ids(&vocab, 5), [6, 5]);
    let person = vocab.iter_mut().find(|entry| entry.id == 5).unwrap();
    person.canonical = "新姓名".into();
    person.phone = "654321".into();
    undo.restore(&mut vocab).unwrap();
    assert_eq!(units::sibling_ids(&vocab, 5), [5, 6]);
    let person = vocab.iter().find(|entry| entry.id == 5).unwrap();
    assert_eq!(
        (
            person.canonical.as_str(),
            person.phone.as_str(),
            person.unit.as_str()
        ),
        ("新姓名", "654321", "0001")
    );
}

#[test]
fn invalid_drop_and_stale_undo_leave_vocabulary_untouched() {
    let mut vocab = vocabulary();
    let before = vocab.clone();
    for (source, target) in [(1, 2), (5, 7), (5, 4)] {
        assert!(SortUndo::place(&mut vocab, source, SiblingPosition::Before(target)).is_err());
        assert_eq!(vocab, before);
    }
    let undo = SortUndo::place(&mut vocab, 2, SiblingPosition::After(3))
        .unwrap()
        .unwrap();
    units::place_sibling(&mut vocab, 2, SiblingPosition::Before(3)).unwrap();
    let changed = vocab.clone();
    assert!(undo.restore(&mut vocab).is_err());
    assert_eq!(vocab, changed);
}

#[test]
fn temporary_collapse_only_hides_dragged_units_descendants() {
    let rows: Vec<_> = [
        (1, 0, true),
        (5, 1, false),
        (2, 1, true),
        (6, 2, false),
        (4, 2, true),
        (3, 1, true),
        (8, 0, true),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (id, depth, is_unit))| TreeRow {
        index,
        id,
        depth,
        is_unit,
        has_children: is_unit,
    })
    .collect();
    assert_eq!(
        visible_rows(&rows, Some(2))
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [1, 5, 2, 3, 8]
    );
    assert_eq!(visible_rows(&rows, None).len(), rows.len());
    assert_eq!(visible_rows(&rows, Some(5)).len(), rows.len());
}

struct Harness {
    ctx: egui::Context,
    vocab: Vec<VocabularyEntry>,
    rects: Vec<(u64, egui::Rect, egui::Rect)>,
    clock: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        Self {
            ctx,
            vocab: vocabulary(),
            rects: Vec::new(),
            clock: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Option<VocabAction> {
        self.clock += 0.05;
        self.rects.clear();
        let mut action = None;
        let _ = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 700.0),
                )),
                time: Some(self.clock),
                events,
                ..Default::default()
            },
            |ui| {
                for entry in &self.vocab {
                    let mut grip = egui::Rect::NOTHING;
                    let rect = ui
                        .horizontal(|ui| {
                            grip = handle(ui, entry.id).rect;
                            ui.label(format!("{}", entry.id));
                            ui.allocate_space(egui::vec2(250.0, 26.0));
                        })
                        .response
                        .rect;
                    self.rects.push((entry.id, grip, rect));
                    if let Some(drop) = drop_target(ui, rect, entry.id, &self.vocab) {
                        action = Some(drop);
                    }
                }
            },
        );
        action
    }

    fn button(&mut self, pos: egui::Pos2, pressed: bool) -> Option<VocabAction> {
        self.frame(vec![egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        }])
    }

    fn drag(&mut self, source: u64, target: u64, before: bool) -> egui::Pos2 {
        self.frame(Vec::new());
        self.frame(Vec::new());
        let start = self
            .rects
            .iter()
            .find(|row| row.0 == source)
            .unwrap()
            .1
            .center();
        let rect = self.rects.iter().find(|row| row.0 == target).unwrap().2;
        let end = egui::pos2(
            rect.center().x,
            if before {
                rect.top() + 3.0
            } else {
                rect.bottom() - 3.0
            },
        );
        self.frame(vec![egui::Event::PointerMoved(start)]);
        self.button(start, true);
        self.frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(0.0, 10.0),
        )]);
        self.frame(Vec::new());
        assert!(egui::DragAndDrop::has_payload_of_type::<Payload>(&self.ctx));
        assert!(self.frame(vec![egui::Event::PointerMoved(end)]).is_none());
        self.frame(Vec::new());
        end
    }
}

#[test]
fn pointer_drop_selects_before_or_after_a_sibling() {
    for before in [true, false] {
        let mut harness = Harness::new();
        let end = harness.drag(2, 3, before);
        let Some(VocabAction::Place { id, position }) = harness.button(end, false) else {
            panic!("同级松手应生成排序动作");
        };
        assert_eq!(id, 2);
        assert!(matches!(
            (before, position),
            (true, SiblingPosition::Before(3)) | (false, SiblingPosition::After(3))
        ));
    }
}

#[test]
fn pointer_drop_rejects_cross_level_and_escape() {
    for (source, target) in [(1, 2), (5, 7), (5, 4)] {
        let mut harness = Harness::new();
        let end = harness.drag(source, target, true);
        assert!(harness.button(end, false).is_none());
    }
    let mut harness = Harness::new();
    let end = harness.drag(2, 3, false);
    harness.frame(vec![egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert!(!egui::DragAndDrop::has_payload_of_type::<Payload>(
        &harness.ctx
    ));
    assert!(harness.button(end, false).is_none());
}

#[test]
fn edge_scroll_only_moves_when_pointer_is_inside_tree() {
    for (x, should_scroll) in [(20.0, true), (500.0, false)] {
        let ctx = egui::Context::default();
        let mut offset = 0.0;
        for frame in 0..4 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 220.0),
                    )),
                    time: Some(frame as f64 * 0.05),
                    events: vec![egui::Event::PointerMoved(egui::pos2(x, 158.0))],
                    ..Default::default()
                },
                |ui| {
                    egui::DragAndDrop::set_payload(ui.ctx(), Payload(1));
                    let output = egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let (_, rect) = ui.allocate_space(egui::vec2(350.0, 1000.0));
                            edge_scroll(ui, rect);
                        });
                    offset = output.state.offset.y;
                },
            );
        }
        assert_eq!(
            offset > 0.0,
            should_scroll,
            "仅列表内部的边缘拖动应滚动：{offset}"
        );
    }
}
