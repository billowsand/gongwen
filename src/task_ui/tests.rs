use super::*;
use egui_kittest::kittest::Queryable;

struct Bench {
    store: Store,
    state: State,
    vocabulary: Vec<crate::models::VocabularyEntry>,
}
fn harness() -> egui_kittest::Harness<'static, Bench> {
    let mut task = Task::blank();
    task.name = "测试任务".into();
    task.progress = "已启动".into();
    let state = State {
        draft: Some(task),
        ..Default::default()
    };
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, b: &mut Bench| {
            show(ui, Some(&mut b.store), None, &b.vocabulary, &mut b.state);
        },
        Bench {
            store: Store::open(":memory:").unwrap(),
            state,
            vocabulary: Vec::new(),
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
            show(ui, Some(&mut b.store), None, &b.vocabulary, &mut b.state);
        },
        Bench {
            store,
            state,
            vocabulary: Vec::new(),
        },
    );
    h.set_size(egui::vec2(1280.0, 900.0));
    h.run();
    h
}

fn saved(store: &mut Store, name: &str, edit: impl FnOnce(&mut Task)) -> Task {
    let mut t = Task::blank();
    t.name = name.into();
    t.progress = "已启动".into();
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
    draft.progress = "人工保留的完成条件".into();
    let mut state = State {
        fill_text: "办公室要求形成方案报审".into(),
        ..Default::default()
    };
    apply_fill(
        &mut state,
        &mut draft,
        crate::task_assistant::extract::Fill {
            name: Some(filled("形成方案")),
            progress: Some(filled("方案报审")),
            others: 2,
            ..Default::default()
        },
        &[],
    );
    assert_eq!(draft.progress, "人工保留的完成条件");
    assert_eq!(draft.name, "形成方案");
    assert!(state.fill_marks.contains_key(&FillField::Name));
    assert!(state.fill_suggestions.contains_key(&FillField::Progress));
    state.draft = Some(draft);
    let mut h = bench(state, store);
    h.get_by_label("替换").scroll_to_me();
    h.run();
    h.get_by_label("替换").click();
    h.run();
    assert_eq!(h.state().state.draft.as_ref().unwrap().progress, "方案报审");
    assert!(
        h.state()
            .state
            .fill_marks
            .contains_key(&FillField::Progress)
    );
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
    assert_eq!(task.progress, "人工保留的完成条件");
    assert!(state.fill_marks.is_empty() && state.fill_suggestions.is_empty());
}

#[test]
fn task_assistant_fill_save_attaches_source_only_with_remaining_marks() {
    for keep_marks in [true, false] {
        let store = Store::open(":memory:").unwrap();
        let mut draft = Task::blank();
        draft.progress = "完成条件".into();
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
            &[],
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
        &[],
    );
    assert!(state.draft.as_ref().unwrap().name.is_empty());
    state.fill_discard = true;
    receive_fill(&mut state, &id, "办公室要求形成方案报审", Ok(result()), &[]);
    assert!(!state.fill_busy && state.draft.as_ref().unwrap().name.is_empty());
    state.fill_busy = true;
    state.close_draft();
    let mut reopened = Task::blank();
    reopened.id = id.clone();
    state.apply_replace(Replace::Edit(Box::new(reopened)));
    receive_fill(&mut state, &id, "办公室要求形成方案报审", Ok(result()), &[]);
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
        &[],
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
        &[],
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
        &[],
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
            progress: Some(filled("不得填入")),
            ..Default::default()
        },
        &[],
    );
    assert!(draft.progress.is_empty());
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
fn task_assistant_today_view_uses_only_task_deadline() {
    let mut store = Store::open(":memory:").unwrap();
    let late = saved(&mut store, "逾期", |t| {
        t.deadline = Some("2026-10-20".into())
    });
    let next = saved(&mut store, "无任务期限", |t| {
        t.progress = "今天继续推进".into()
    });
    let later = saved(&mut store, "以后", |t| {
        t.deadline = Some("2026-12-01".into())
    });
    let all = store.list(true).unwrap();
    let today = "2026-10-28";
    let hit =
        |t: &Task| View::Today.matches(&all, all.iter().find(|x| x.id == t.id).unwrap(), today);
    assert!(hit(&late));
    assert!(!hit(&next));
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
fn task_assistant_fill_owner_matching_is_unique_or_explicitly_ambiguous() {
    use crate::models::{VocabularyCategory as C, VocabularyEntry as E};
    let vocab = vec![
        E {
            id: 1,
            category: C::Unit,
            canonical: "甲单位".into(),
            code: "0001".into(),
            ..Default::default()
        },
        E {
            id: 2,
            category: C::Unit,
            canonical: "乙单位".into(),
            code: "0002".into(),
            ..Default::default()
        },
        E {
            id: 11,
            category: C::Person,
            canonical: "张宁".into(),
            aliases: vec!["小张".into()],
            unit: "0001".into(),
            ..Default::default()
        },
        E {
            id: 12,
            category: C::Person,
            canonical: "张宁".into(),
            unit: "0002".into(),
            ..Default::default()
        },
        E {
            id: 13,
            category: C::Person,
            canonical: "李明".into(),
            unit: "0001".into(),
            ..Default::default()
        },
    ];
    let draft = Task::blank();
    let id = draft.id.clone();
    let mut state = State {
        draft: Some(draft),
        fill_busy: true,
        ..Default::default()
    };
    receive_fill(
        &mut state,
        &id,
        "由小张牵头",
        Ok(crate::task_assistant::extract::Fill {
            owner: Some(crate::task_assistant::extract::FilledValue {
                value: "小张".into(),
                evidence: "小张".into(),
            }),
            ..Default::default()
        }),
        &vocab,
    );
    assert_eq!(state.draft.as_ref().unwrap().owner_id, Some(11));
    assert!(state.fill_marks.contains_key(&FillField::Owner));

    let draft = Task::blank();
    let id = draft.id.clone();
    let mut state = State {
        draft: Some(draft),
        fill_busy: true,
        ..Default::default()
    };
    receive_fill(
        &mut state,
        &id,
        "张宁牵头",
        Ok(crate::task_assistant::extract::Fill {
            owner: Some(crate::task_assistant::extract::FilledValue {
                value: "张宁".into(),
                evidence: "张宁".into(),
            }),
            ..Default::default()
        }),
        &vocab,
    );
    assert_eq!(state.draft.as_ref().unwrap().owner_id, None);
    assert_eq!(state.owner_query, "张宁");
    assert!(
        state
            .fill_owner_note
            .as_deref()
            .unwrap()
            .contains("2 位同名")
    );

    let draft = Task::blank();
    let id = draft.id.clone();
    let mut state = State {
        draft: Some(draft),
        fill_busy: true,
        ..Default::default()
    };
    receive_fill(
        &mut state,
        &id,
        "王五牵头",
        Ok(crate::task_assistant::extract::Fill {
            owner: Some(crate::task_assistant::extract::FilledValue {
                value: "王五".into(),
                evidence: "王五".into(),
            }),
            ..Default::default()
        }),
        &vocab,
    );
    assert_eq!(state.draft.as_ref().unwrap().owner_id, None);
    assert!(
        state
            .fill_owner_note
            .as_deref()
            .unwrap()
            .contains("标准词库里没有此人")
    );
}

#[test]
fn task_assistant_owner_picker_keeps_same_name_people_separate() {
    use crate::models::{VocabularyCategory as C, VocabularyEntry as E};
    let vocabulary = vec![
        E {
            id: 1,
            category: C::Unit,
            canonical: "甲单位".into(),
            code: "0001".into(),
            ..Default::default()
        },
        E {
            id: 2,
            category: C::Unit,
            canonical: "乙单位".into(),
            code: "0002".into(),
            ..Default::default()
        },
        E {
            id: 11,
            category: C::Person,
            canonical: "张宁".into(),
            position: "主任".into(),
            unit: "0001".into(),
            ..Default::default()
        },
        E {
            id: 12,
            category: C::Person,
            canonical: "张宁".into(),
            position: "主任".into(),
            unit: "0002".into(),
            ..Default::default()
        },
        E {
            id: 13,
            category: C::Person,
            canonical: "李明".into(),
            position: "副主任".into(),
            unit: "0001".into(),
            ..Default::default()
        },
    ];
    let state = State {
        draft: Some(Task::blank()),
        ..Default::default()
    };
    let mut h = egui_kittest::Harness::new_ui_state(
        |ui, b: &mut Bench| {
            if let Some(d) = b.state.draft.as_mut() {
                let _ = owner::picker(
                    ui,
                    "test-owner",
                    &b.vocabulary,
                    &mut d.owner_id,
                    &mut d.owner_name,
                    &mut b.state.owner_query,
                );
            }
        },
        Bench {
            store: Store::open(":memory:").unwrap(),
            state,
            vocabulary,
        },
    );
    h.set_size(egui::vec2(640.0, 480.0));
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput).click();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("张宁");
    h.run();
    h.get_by_label("张宁 · 主任 · 乙单位").click();
    h.run();
    assert_eq!(h.state().state.draft.as_ref().unwrap().owner_id, Some(12));
    let (label, found) = owner::label(&h.state().vocabulary, Some(12), "张宁");
    assert!(found && label.contains("乙单位"));
}

#[test]
fn task_assistant_detail_updates_current_progress_and_history() {
    let mut store = Store::open(":memory:").unwrap();
    let task = saved(&mut store, "更新进展测试", |t| {
        t.progress = "原进展".into()
    });
    let state = State {
        selected: Some(task.id.clone()),
        ..Default::default()
    };
    let mut h = bench(state, store);
    assert_eq!(h.state().state.progress_text, "原进展");
    h.state_mut().state.progress_text = "已完成现场核查".into();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "更新")
        .scroll_to_me();
    h.run();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "更新")
        .click();
    h.run();
    let updated = h.state().store.list(true).unwrap().remove(0);
    assert_eq!(updated.progress, "已完成现场核查");
    assert_eq!(h.state().state.progress_text, updated.progress);
    assert_eq!(
        h.state().store.history_entries(&task.id).unwrap()[0].action,
        "进展更新"
    );
    assert!(h.query_by_label("当前进展").is_some());
}

#[test]
fn task_assistant_progress_box_follows_changes_it_has_not_edited() {
    let mut state = State::default();
    let mut t = Task::blank();
    t.progress = "旧进展".into();
    detail::sync_progress_box(&mut state, &t);
    assert_eq!(state.progress_text, "旧进展");
    // 别处改了进展、框里没动过：跟着刷新，免得「更新」用旧字覆盖新进展。
    t.progress = "编辑表单里改的新进展".into();
    detail::sync_progress_box(&mut state, &t);
    assert_eq!(state.progress_text, "编辑表单里改的新进展");
    // 用户已经在框里改了字：保留用户的字。
    state.progress_text = "我正在写的".into();
    t.progress = "又一次外部修改".into();
    detail::sync_progress_box(&mut state, &t);
    assert_eq!(state.progress_text, "我正在写的");
}
