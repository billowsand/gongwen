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

fn filled(value: &str) -> crate::task_assistant::extract::FilledValue {
    crate::task_assistant::extract::FilledValue {
        value: value.into(),
        evidence: "办公室要求形成方案报审".into(),
    }
}

#[test]
fn task_assistant_fill_empty_fields_suggestions_replace_and_undo() {
    let store = Store::open(":memory:").unwrap();
    let mut draft = Task::blank();
    draft.goal = "人工保留的完成条件".into();
    let mut state = State {
        fill_text: "办公室要求形成方案报审".into(),
        ..Default::default()
    };
    apply_fill(
        &mut state,
        &mut draft,
        crate::task_assistant::extract::Fill {
            name: Some(filled("形成方案")),
            goal: Some(filled("方案报审")),
            others: 2,
            ..Default::default()
        },
    );
    assert_eq!(draft.goal, "人工保留的完成条件");
    assert_eq!(draft.name, "形成方案");
    assert!(state.fill_marks.contains_key(&FillField::Name));
    assert!(state.fill_suggestions.contains_key(&FillField::Goal));
    state.draft = Some(draft);
    let mut h = bench(state, store);
    h.get_by_label("替换").scroll_to_me();
    h.run();
    h.get_by_label("替换").click();
    h.run();
    assert_eq!(h.state().state.draft.as_ref().unwrap().goal, "方案报审");
    assert!(h.state().state.fill_marks.contains_key(&FillField::Goal));
    h.state_mut().state.draft.as_mut().unwrap().name = "人工重新命名".into();
    h.run();
    assert!(!h.state().state.fill_marks.contains_key(&FillField::Name));
    h.get_by_label("全部撤销").scroll_to_me();
    h.run();
    h.get_by_label("全部撤销").click();
    h.run();
    let state = &h.state().state;
    let task = state.draft.as_ref().unwrap();
    assert_eq!(task.name, "人工重新命名");
    assert_eq!(task.goal, "人工保留的完成条件");
    assert!(state.fill_marks.is_empty() && state.fill_suggestions.is_empty());
}

#[test]
fn task_assistant_fill_save_attaches_source_only_with_remaining_marks() {
    for keep_marks in [true, false] {
        let store = Store::open(":memory:").unwrap();
        let mut draft = Task::blank();
        draft.goal = "完成条件".into();
        let mut state = State {
            fill_text: "办公室要求形成方案报审".into(),
            ..Default::default()
        };
        apply_fill(
            &mut state,
            &mut draft,
            crate::task_assistant::extract::Fill {
                name: Some(filled("形成方案")),
                ..Default::default()
            },
        );
        if !keep_marks {
            draft.name = "改成手工任务".into();
        }
        let id = draft.id.clone();
        state.draft = Some(draft);
        let mut h = bench(state, store);
        h.get_by_label("保存").click();
        h.run();
        assert!(
            h.state().state.draft.is_none(),
            "{:?}",
            h.state().state.form_error
        );
        let history = h.state().store.history_entries(&id).unwrap();
        assert_eq!(history[0].source_id.is_some(), keep_marks);
        assert!(h.state().state.fill_source.is_none() && h.state().state.fill_marks.is_empty());
    }
}

#[test]
fn task_assistant_fill_late_cancelled_and_reopened_results_are_discarded() {
    let draft = Task::blank();
    let id = draft.id.clone();
    let mut state = State {
        draft: Some(draft),
        fill_busy: true,
        ..Default::default()
    };
    let result = || crate::task_assistant::extract::Fill {
        name: Some(filled("不该填入")),
        ..Default::default()
    };
    receive_fill(
        &mut state,
        "另一个ID",
        "办公室要求形成方案报审",
        Ok(result()),
    );
    assert!(state.draft.as_ref().unwrap().name.is_empty());
    state.fill_discard = true;
    receive_fill(&mut state, &id, "办公室要求形成方案报审", Ok(result()));
    assert!(!state.fill_busy && state.draft.as_ref().unwrap().name.is_empty());
    state.fill_busy = true;
    state.close_draft();
    let mut reopened = Task::blank();
    reopened.id = id.clone();
    state.apply_replace(Replace::Edit(Box::new(reopened)));
    receive_fill(&mut state, &id, "办公室要求形成方案报审", Ok(result()));
    assert!(state.draft.as_ref().unwrap().name.is_empty());
}

#[test]
fn task_assistant_fill_keeps_request_source_and_transfers_other_matters_to_inbox() {
    let mut state = State {
        draft: Some(Task::blank()),
        fill_busy: true,
        fill_text: "用户识别期间另写的文字".into(),
        ..Default::default()
    };
    let id = state.draft.as_ref().unwrap().id.clone();
    receive_fill(
        &mut state,
        &id,
        "办公室要求形成方案报审",
        Ok(crate::task_assistant::extract::Fill {
            name: Some(filled("方案")),
            others: 2,
            ..Default::default()
        }),
    );
    assert_eq!(state.fill_source.as_deref(), Some("办公室要求形成方案报审"));
    assert_eq!(state.fill_text, "用户识别期间另写的文字");
    let mut h = bench(state, Store::open(":memory:").unwrap());
    h.get_by_label("转到批量整理").click();
    h.run();
    assert_eq!(h.state().state.view, View::Inbox);
    assert!(h.state().state.source_title.is_empty());
    assert!(h.state().state.draft.is_some());
}

#[test]
fn task_assistant_material_title_can_be_omitted_and_is_filled_before_request() {
    let state = State {
        view: View::Inbox,
        source_text: "  《办公室关于开展专项协调工作的通知》。\n请办公室跟进。".into(),
        ..Default::default()
    };
    let mut h = bench(state, Store::open(":memory:").unwrap());
    h.get_by_label("识别").click();
    h.run();
    let state = &h.state().state;
    assert_eq!(state.source_title, "办公室关于开展专项协调工作的通知");
    assert_eq!(
        state.request_extract.as_ref().unwrap().0,
        state.source_title
    );
}

#[test]
fn task_assistant_fill_repeated_sources_keep_evidence_and_limit_total_size() {
    use crate::task_assistant::extract::{Fill, MAX_SOURCE_BYTES};
    let mut state = State {
        fill_text: "办公室要求形成方案报审".into(),
        ..Default::default()
    };
    let mut draft = Task::blank();
    apply_fill(
        &mut state,
        &mut draft,
        Fill {
            name: Some(filled("方案报审")),
            ..Default::default()
        },
    );
    state.fill_text = "各科室反馈后汇总意见".into();
    apply_fill(
        &mut state,
        &mut draft,
        Fill {
            next_step: Some(crate::task_assistant::extract::FilledValue {
                value: "汇总意见".into(),
                evidence: "各科室反馈后汇总意见".into(),
            }),
            ..Default::default()
        },
    );
    for value in state.fill_marks.values() {
        assert!(
            state
                .fill_source
                .as_ref()
                .unwrap()
                .contains(&value.evidence)
        );
    }
    let source = state.fill_source.clone();
    state.fill_text = "x".repeat(MAX_SOURCE_BYTES);
    apply_fill(
        &mut state,
        &mut draft,
        Fill {
            goal: Some(filled("不得填入")),
            ..Default::default()
        },
    );
    assert!(draft.goal.is_empty());
    assert_eq!(state.fill_source, source);
    assert!(state.fill_error.is_some());
    fill::undo_fill(&mut state, &mut draft);
    assert!(draft.name.is_empty() && draft.next_step.is_empty());
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

#[test]
fn task_assistant_fill_never_makes_a_task_its_own_parent() {
    let mut draft = Task::blank();
    let mut state = State {
        fill_text: "办公室要求形成方案报审".into(),
        ..Default::default()
    };
    let own = crate::task_assistant::extract::FilledValue {
        value: draft.id.clone(),
        evidence: "办公室要求形成方案报审".into(),
    };
    apply_fill(
        &mut state,
        &mut draft,
        crate::task_assistant::extract::Fill {
            parent_id: Some(own),
            ..Default::default()
        },
    );
    assert!(draft.parent_id.is_none());
    assert_eq!(state.fill_summary, Some((1, 0)));
}
