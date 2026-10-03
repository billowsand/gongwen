//! 真实 egui 指针事件与内存稿件库一起验证排序和导出计划。

use super::*;
use crate::manuscript::NewManuscript;
use crate::models::{DraftInput, TemplateProfile};

fn package() -> (ManuscriptStore, SendPackagePanel) {
    let mut store = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let mut ids = Vec::new();
    for (kind, title) in [
        (TemplateKind::WhitePaper, "Owner"),
        (TemplateKind::OfficialLetter, "Letter"),
        (TemplateKind::ResearchReport, "Report"),
        (TemplateKind::PlainDocument, "Notice"),
    ] {
        let snapshot = DraftInput {
            kind,
            title_hint: title.into(),
            profile: TemplateProfile::for_kind(kind),
            ..Default::default()
        };
        let markdown = format!("# {title}\n\n正文。");
        let id = store
            .create(
                &NewManuscript {
                    snapshot: snapshot.clone(),
                    content_markdown: markdown.clone(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        store
            .commit_manuscript_version(id, "初稿", "", &snapshot, &markdown, "")
            .unwrap();
        ids.push(id);
    }
    for id in &ids[1..] {
        store.add_send_package_item(ids[0], *id).unwrap();
    }
    let mut panel = SendPackagePanel::new(ids[0]);
    panel.reload(&store);
    panel.confirm = Some(ExportConfirm {
        plan: store.send_package_plan(ids[0]).unwrap(),
        with_toc: true,
    });
    (store, panel)
}

struct Harness {
    ctx: egui::Context,
    store: ManuscriptStore,
    panel: SendPackagePanel,
    clock: f64,
}

impl Harness {
    fn new() -> Self {
        let (store, panel) = package();
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        Self {
            ctx,
            store,
            panel,
            clock: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> (egui::FullOutput, Option<PanelAction>) {
        self.clock += 0.05;
        let mut action = None;
        let output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 900.0),
                )),
                time: Some(self.clock),
                events,
                ..Default::default()
            },
            |ui| {
                row_card(
                    ui,
                    self.panel.owner.as_ref().unwrap(),
                    RowRole::Owner,
                    None,
                    &|_| false,
                    &mut None,
                    &mut action,
                );
                self.panel.dragging = items_ui(
                    ui,
                    self.panel.owner_id,
                    &self.panel.items,
                    !self.panel.archived(),
                    &|_| false,
                    &mut None,
                    &mut action,
                );
            },
        );
        (output, action)
    }

    fn button(&mut self, pos: egui::Pos2, pressed: bool) -> Option<PanelAction> {
        self.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }])
        .1
    }

    fn start_drag(&mut self, from: usize, to: usize) -> egui::Pos2 {
        self.frame(Vec::new());
        let (output, _) = self.frame(Vec::new());
        let source = number_pos(&output, &format!("{}.", from + 1));
        let destination = number_pos(&output, &format!("{}.", to + 1));
        // 手柄位于序号左边，使用真实绘制坐标，不依赖卡片高度。
        let start = source - egui::vec2(16.0, 0.0);
        self.frame(vec![egui::Event::PointerMoved(start)]);
        assert!(self.button(start, true).is_none());
        self.frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(0.0, 3.0),
        )]);
        self.frame(Vec::new());
        assert!(self.panel.dragging, "按住手柄并移动应进入拖动");
        let target = egui::pos2(start.x, destination.y + if to > from { 20.0 } else { -2.0 });
        assert!(
            self.frame(vec![egui::Event::PointerMoved(target)])
                .1
                .is_none(),
            "松手前不写入排序"
        );
        self.frame(Vec::new());
        target
    }
}

fn number_pos(output: &egui::FullOutput, number: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == number => {
                Some(text.pos + egui::vec2(0.0, 8.0))
            }
            _ => None,
        })
        .expect("清单应显示序号")
}

#[test]
fn drag_saves_order_and_updates_confirm_without_changing_revisions() {
    let mut harness = Harness::new();
    let before: Vec<_> = harness
        .panel
        .confirm
        .as_ref()
        .unwrap()
        .plan
        .entries
        .iter()
        .map(|entry| (entry.title.clone(), entry.revision.clone()))
        .collect();
    let target = harness.start_drag(0, 2);
    let Some(PanelAction::Move { from, to }) = harness.button(target, false) else {
        panic!("松手后应提交排序");
    };
    assert_eq!((from, to), (0, 2));
    harness
        .panel
        .move_item(&mut harness.store, from, to)
        .unwrap();
    let confirm = harness.panel.confirm.as_ref().unwrap();
    assert!(confirm.with_toc);
    let entries = &confirm.plan.entries;
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>(),
        ["Owner", "Report", "Notice", "Letter"]
    );
    for entry in entries {
        assert_eq!(
            &entry.revision,
            &before
                .iter()
                .find(|(title, _)| title == &entry.title)
                .unwrap()
                .1
        );
    }
    harness.panel.reload(&harness.store);
    assert_eq!(
        harness
            .panel
            .items
            .iter()
            .map(|row| row.brief.as_ref().unwrap().title.as_str())
            .collect::<Vec<_>>(),
        ["Report", "Notice", "Letter"]
    );
    harness.panel.undo_sort(&mut harness.store).unwrap();
    assert!(harness.panel.sort_undo.is_none());
    assert_eq!(
        harness
            .panel
            .confirm
            .as_ref()
            .unwrap()
            .plan
            .entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>(),
        ["Owner", "Letter", "Report", "Notice"]
    );
    assert!(harness.panel.confirm.as_ref().unwrap().with_toc);
}

#[test]
fn undo_does_not_overwrite_a_changed_list() {
    let (mut store, mut panel) = package();
    panel.move_item(&mut store, 0, 2).unwrap();
    let order: Vec<_> = store
        .send_package_items(panel.owner_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_uuid)
        .rev()
        .collect();
    store.reorder_send_package(panel.owner_id, &order).unwrap();
    assert!(panel.undo_sort(&mut store).is_err());
    assert!(panel.sort_undo.is_none());
    assert_eq!(
        store
            .send_package_items(panel.owner_id)
            .unwrap()
            .into_iter()
            .map(|item| item.item_uuid)
            .collect::<Vec<_>>(),
        order
    );
}

#[test]
fn escape_and_drop_outside_do_not_submit_order() {
    for escape in [true, false] {
        let mut harness = Harness::new();
        let target = harness.start_drag(0, 2);
        if escape {
            harness.frame(vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
            harness.frame(vec![egui::Event::PointerMoved(
                target + egui::vec2(0.0, 5.0),
            )]);
            assert!(harness.button(target, false).is_none());
        } else {
            let outside = egui::pos2(850.0, target.y);
            harness.frame(vec![egui::Event::PointerMoved(outside)]);
            assert!(harness.button(outside, false).is_none());
        }
        assert_eq!(
            harness
                .store
                .send_package_plan(harness.panel.owner_id)
                .unwrap()
                .entries[1]
                .title,
            "Letter"
        );
    }
}

#[test]
fn drag_from_last_to_first_keeps_owner_first() {
    let mut harness = Harness::new();
    let target = harness.start_drag(2, 0);
    let Some(PanelAction::Move { from, to }) = harness.button(target, false) else {
        panic!("向上拖动应提交排序");
    };
    assert_eq!((from, to), (2, 0));
    harness
        .panel
        .move_item(&mut harness.store, from, to)
        .unwrap();
    let plan = harness
        .store
        .send_package_plan(harness.panel.owner_id)
        .unwrap();
    assert_eq!(
        plan.entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>(),
        ["Owner", "Notice", "Letter", "Report"]
    );
}

#[test]
fn archived_list_has_no_drag_handle_and_store_rejects_reorder() {
    let mut harness = Harness::new();
    harness
        .store
        .set_status(harness.panel.owner_id, ManuscriptStatus::Archived)
        .unwrap();
    harness.panel.reload(&harness.store);
    let (output, _) = harness.frame(Vec::new());
    let start = number_pos(&output, "1.");
    harness.frame(vec![egui::Event::PointerMoved(start)]);
    harness.button(start, true);
    harness.frame(vec![egui::Event::PointerMoved(
        start + egui::vec2(0.0, 120.0),
    )]);
    assert!(!harness.panel.dragging);
    assert!(harness.button(start, false).is_none());
    assert!(harness.panel.move_item(&mut harness.store, 0, 2).is_err());
}

#[test]
fn never_committed_items_block_export_and_are_pending() {
    let (mut store, mut panel) = package();
    assert_eq!(panel.blocked_count(), 0);
    assert!(panel.pending_commits(&|_| false).is_empty());

    let kind = TemplateKind::OfficialLetter;
    let id = store
        .create(
            &NewManuscript {
                snapshot: DraftInput {
                    kind,
                    title_hint: "Draft".into(),
                    profile: TemplateProfile::for_kind(kind),
                    ..Default::default()
                },
                content_markdown: "# Draft\n\n正文。".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    store.add_send_package_item(panel.owner_id, id).unwrap();
    panel.reload(&store);

    assert_eq!(panel.blocked_count(), 1);
    assert_eq!(
        panel.pending_commits(&|_| false),
        [(id, "初稿".to_string())]
    );

    // 已提交但编辑器里有未保存修改：不拦导出，但算待提交，默认名改叫「修订稿」。
    let committed = panel.items[0].brief.as_ref().unwrap().id;
    assert_eq!(
        panel.pending_commits(&|candidate| candidate == committed),
        [(committed, "修订稿".to_string()), (id, "初稿".to_string())]
    );

    store
        .set_status(panel.owner_id, ManuscriptStatus::Archived)
        .unwrap();
    panel.reload(&store);
    assert!(
        panel
            .pending_commits(&|_| true)
            .iter()
            .all(|(candidate, _)| *candidate != panel.owner_id),
        "已归档的件不提供提交"
    );
}
