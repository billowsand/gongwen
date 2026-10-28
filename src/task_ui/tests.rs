use super::*;
use egui_kittest::kittest::Queryable;

struct Bench {
    store: Store,
    state: State,
}
fn harness() -> egui_kittest::Harness<'static, Bench> {
    let mut task = Task::blank();
    task.name = "测试任务".into();
    task.goal = "交付说明".into();
    let state = State {
        draft: Some(task),
        ..Default::default()
    };
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, b: &mut Bench| {
            show(ui, Some(&mut b.store), None, &mut b.state);
        },
        Bench {
            store: Store::open(":memory:").unwrap(),
            state,
        },
    );
    h.set_size(egui::vec2(1280.0, 900.0));
    h.run();
    h
}

#[test]
fn task_assistant_editor_save_closes_draft_without_reinserting_it() {
    let mut h = harness();
    let id = h.state().state.draft.as_ref().unwrap().id.clone();
    h.get_by_label("保存").scroll_to_me();
    h.run();
    h.get_by_label("保存").click();
    h.run();
    assert!(
        h.state().state.draft.is_none(),
        "{:?}",
        h.state().state.error
    );
    assert_eq!(h.state().store.list(true).unwrap().len(), 1);
    assert_eq!(h.state().state.selected, Some(id));
}

#[test]
fn task_assistant_editor_cancel_closes_without_writing() {
    let mut h = harness();
    h.get_by_label("放弃本次编辑").scroll_to_me();
    h.run();
    h.get_by_label("放弃本次编辑").click();
    h.run();
    assert!(h.state().state.draft.is_none());
    assert!(h.state().store.list(true).unwrap().is_empty());
}
