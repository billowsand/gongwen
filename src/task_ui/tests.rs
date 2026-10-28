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
    h.get_by_label("放弃").scroll_to_me();
    h.run();
    h.get_by_label("放弃").click();
    h.run();
    assert!(h.state().state.draft.is_none());
    assert!(h.state().store.list(true).unwrap().is_empty());
}

fn bench(state: State, store: Store) -> egui_kittest::Harness<'static, Bench> {
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, b: &mut Bench| {
            show(ui, Some(&mut b.store), None, &mut b.state);
        },
        Bench { store, state },
    );
    h.set_size(egui::vec2(1280.0, 900.0));
    h.run();
    h
}

fn saved(store: &mut Store, name: &str, edit: impl FnOnce(&mut Task)) -> Task {
    let mut t = Task::blank();
    t.name = name.into();
    t.goal = "交付说明".into();
    edit(&mut t);
    store.save(&t, "创建", "测试").unwrap();
    store
        .list(true)
        .unwrap()
        .into_iter()
        .find(|x| x.id == t.id)
        .unwrap()
}

#[test]
fn task_assistant_detail_review_banner_clears_needs_review() {
    let mut store = Store::open(":memory:").unwrap();
    let t = saved(&mut store, "待核对任务", |t| t.needs_review = true);
    let state = State {
        selected: Some(t.id.clone()),
        ..Default::default()
    };
    let mut h = bench(state, store);
    h.get_by_label("已核对").click();
    h.run();
    let after = h.state().store.list(true).unwrap();
    assert!(!after[0].needs_review, "{:?}", h.state().state.error);
}

#[test]
fn task_assistant_new_task_with_open_draft_asks_before_replacing() {
    let mut draft = Task::blank();
    draft.name = "写了一半".into();
    let state = State {
        draft: Some(draft),
        ..Default::default()
    };
    let mut h = bench(state, Store::open(":memory:").unwrap());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "新建任务")
        .click();
    h.run();
    assert_eq!(h.state().state.draft.as_ref().unwrap().name, "写了一半");
    h.get_by_label("放弃并继续").click();
    h.run();
    assert!(h.state().state.draft.as_ref().unwrap().name.is_empty());
}

#[test]
fn task_assistant_views_split_review_from_blocked() {
    let mut store = Store::open(":memory:").unwrap();
    let review = saved(&mut store, "待核对", |t| t.needs_review = true);
    let before = saved(&mut store, "前置", |_| {});
    let after = saved(&mut store, "后续", |t| {
        t.prerequisites = vec![before.id.clone()]
    });
    let all = store.list(true).unwrap();
    let find = |id: &str| all.iter().find(|t| t.id == id).unwrap();
    let today = "2026-10-28";
    assert!(View::Review.matches(&all, find(&review.id), today));
    // 待核对只进「待核对」，不再同时标成阻塞。
    assert!(!View::Waiting.matches(&all, find(&review.id), today));
    assert!(View::Waiting.matches(&all, find(&after.id), today));
    assert!(View::Active.matches(&all, find(&before.id), today));
    assert!(!View::Archived.matches(&all, find(&before.id), today));
}

#[test]
fn task_assistant_today_view_covers_both_deadlines() {
    let mut store = Store::open(":memory:").unwrap();
    let late = saved(&mut store, "逾期", |t| {
        t.deadline = Some("2026-10-20".into())
    });
    let next = saved(&mut store, "下一步到期", |t| {
        t.next_deadline = Some("2026-10-28".into())
    });
    let later = saved(&mut store, "以后", |t| {
        t.deadline = Some("2026-12-01".into())
    });
    let all = store.list(true).unwrap();
    let today = "2026-10-28";
    let hit =
        |t: &Task| View::Today.matches(&all, all.iter().find(|x| x.id == t.id).unwrap(), today);
    assert!(hit(&late));
    assert!(hit(&next));
    assert!(!hit(&later));
}

#[test]
fn task_assistant_cache_refreshes_only_after_writes() {
    let mut store = Store::open(":memory:").unwrap();
    let mut cache = cache::Cache::default();
    cache.refresh(&store).unwrap();
    let key = store.data_key();
    cache.refresh(&store).unwrap();
    assert_eq!(store.data_key(), key, "只读不应改变数据代号");
    saved(&mut store, "新任务", |_| {});
    assert_ne!(store.data_key(), key);
    cache.refresh(&store).unwrap();
    assert_eq!(cache.tasks.len(), 1);
}

#[test]
fn task_assistant_short_dates_drop_current_year() {
    assert_eq!(short_date("2026-11-05", "2026-10-28"), "11-05");
    assert_eq!(short_date("2027-01-05", "2026-10-28"), "2027-01-05");
    assert_eq!(days_from("2026-10-28", "2026-10-26"), Some(-2));
}
