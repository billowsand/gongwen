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

fn source_row(
    id: &str,
    title: &str,
    pending: usize,
    accepted: usize,
) -> crate::task_assistant::SourceRow {
    crate::task_assistant::SourceRow {
        id: id.into(),
        title: title.into(),
        source_ref: "pasted".into(),
        created_at: chrono::Local::now().to_rfc3339(),
        hidden: false,
        pending,
        accepted,
        ignored: 0,
    }
}

/// 一份带一条新任务建议的材料；`evidence` 必须出现在 `content` 里。
fn store_with_material(store: &mut Store, title: &str, content: &str, evidence: &str) -> String {
    let p: crate::task_assistant::extract::Proposal = serde_json::from_value(serde_json::json!({
        "kind": "new_task", "task_id": null, "name": evidence, "next_step": null,
        "deadline": null, "evidence": evidence
    }))
    .unwrap();
    let fingerprint = format!("fp-{title}");
    store
        .commit_extraction(title, content, &fingerprint, "pasted", &[p], &[])
        .unwrap();
    store
        .source_by_fingerprint(&fingerprint)
        .unwrap()
        .unwrap()
        .id
}

#[test]
fn task_assistant_extraction_result_names_the_material_and_offers_a_filter() {
    // 新材料：说新增几条，不再挂「同版材料不重复生成」；框里还是交出去的原文就清空。
    let mut state = State {
        view: View::Inbox,
        source_title: "第一份纪要".into(),
        source_text: "第一份原文".into(),
        source_title_auto: Some("第一份纪要".into()),
        extraction_error: Some("上一次失败".into()),
        ..Default::default()
    };
    receive_extraction(
        &mut state,
        ExtractionOutcome::Saved {
            source_id: "s1".into(),
            title: "第一份纪要".into(),
            text: "第一份原文".into(),
            added: 3,
        },
    );
    let result = state.extraction_result.clone().unwrap();
    assert!(result.contains("「第一份纪要」") && result.contains("新增 3 条"));
    assert!(!result.contains("不重复"), "{result}");
    assert!(state.source_text.is_empty() && state.source_title.is_empty());
    assert!(state.extraction_error.is_none());
    assert_eq!(state.extraction_source.as_deref(), Some("s1"));
    // 在「待整理建议」里结果就写在按钮旁边，不再弹提示。
    assert!(state.notice.is_none());

    // 识别期间用户已经贴了另一份：不动输入框。
    state.source_text = "第二份原文".into();
    receive_extraction(
        &mut state,
        ExtractionOutcome::Saved {
            source_id: "s2".into(),
            title: "知识库资料".into(),
            text: "资料正文".into(),
            added: 0,
        },
    );
    assert_eq!(state.source_text, "第二份原文");
    assert!(
        state
            .extraction_result
            .as_ref()
            .unwrap()
            .contains("没有从中找出任务或进展")
    );
    assert!(state.extraction_source.is_none());

    // 重新识别：说清新增、替换与跳过各几条。
    receive_extraction(
        &mut state,
        ExtractionOutcome::Rerun {
            source_id: "s1".into(),
            title: "第一份纪要".into(),
            result: crate::task_assistant::Rerun {
                replaced: 2,
                added: 1,
                skipped: 1,
            },
        },
    );
    let result = state.extraction_result.clone().unwrap();
    assert!(
        result.contains("新增 1 条")
            && result.contains("替换了 2 条")
            && result.contains("1 条与已采纳或已忽略的相同"),
        "{result}"
    );

    // 识别过：不清框、不只回一句话，留卡片让人选查看或重新识别。
    receive_extraction(
        &mut state,
        ExtractionOutcome::Known {
            source: source_row("s2", "第二份", 2, 1),
            text: "第二份原文".into(),
        },
    );
    assert_eq!(state.source_text, "第二份原文");
    assert_eq!(state.known_source.as_ref().unwrap().id, "s2");
    assert!(state.extraction_result.is_none());
    assert!(state.notice.is_none());

    // 不在本视图时另给底部提示，指回「待整理建议」。
    state.view = View::Today;
    receive_extraction(
        &mut state,
        ExtractionOutcome::Saved {
            source_id: "s3".into(),
            title: "第三份".into(),
            text: "第三份原文".into(),
            added: 1,
        },
    );
    assert!(state.notice.as_ref().unwrap().contains("待整理建议"));
}

#[test]
fn task_assistant_known_material_card_offers_rerun_and_its_suggestions() {
    let mut store = Store::open(":memory:").unwrap();
    let id = store_with_material(&mut store, "周报", "本周周报原文已完成。", "周报原文已完成");
    let source = store.sources().unwrap().remove(0);
    let state = State {
        view: View::Inbox,
        source_text: "本周周报原文已完成。".into(),
        known_source: Some(source.clone()),
        ..Default::default()
    };
    let mut h = bench(state, store);
    assert!(h.query_by_label_contains("待核对 1 · 已采纳 0").is_some());
    h.get_by_label("查看这批建议").click();
    h.run();
    assert_eq!(h.state().state.inbox_source.as_deref(), Some(id.as_str()));
    assert!(h.state().state.known_source.is_none());
    assert!(h.query_by_label("只看「周报」的建议").is_some());

    h.state_mut().state.known_source = Some(source);
    h.run();
    h.get_by_label("重新识别").click();
    h.run();
    assert_eq!(h.state().state.request_rerun.as_deref(), Some(id.as_str()));
    assert!(h.state().state.known_source.is_none());
}

#[test]
fn task_assistant_inbox_can_show_one_material_and_back_to_all() {
    let mut store = Store::open(":memory:").unwrap();
    let first = store_with_material(&mut store, "周报", "本周周报原文已完成。", "周报原文已完成");
    store_with_material(&mut store, "纪要", "会议纪要原文已印发。", "纪要原文已印发");
    let state = State {
        view: View::Inbox,
        inbox_source: Some(first),
        ..Default::default()
    };
    let mut h = bench(state, store);
    assert!(h.query_by_label("待核对的建议 · 1 条").is_some());
    h.get_by_label("显示全部").click();
    h.run();
    assert!(h.state().state.inbox_source.is_none());
    assert!(h.query_by_label("待核对的建议 · 2 条").is_some());
}

#[test]
fn task_assistant_materials_view_shows_the_snapshot_and_deletes() {
    let mut store = Store::open(":memory:").unwrap();
    let id = store_with_material(
        &mut store,
        "周报",
        "第一段。本周周报原文已完成。",
        "周报原文已完成",
    );
    store_with_material(&mut store, "纪要", "会议纪要原文已印发。", "纪要原文已印发");
    let state = State {
        view: View::Materials,
        ..Default::default()
    };
    let mut h = bench(state, store);
    assert!(h.query_by_label_contains("识别过的材料 · 2 份").is_some());
    // 卡片与里面的标题文字各有一个同名节点，点哪个都落在卡片上。
    h.get_all_by_label("周报").next().unwrap().click();
    h.run();
    assert_eq!(
        h.state().state.material_selected.as_deref(),
        Some(id.as_str())
    );
    assert!(h.query_by_label("第一段。本周周报原文已完成。").is_some());

    h.get_by_label("重新识别").click();
    h.run();
    assert_eq!(h.state().state.request_rerun.as_deref(), Some(id.as_str()));

    h.get_by_label("删除").click();
    h.run();
    h.get_by_label("删除材料").click();
    h.run();
    assert_eq!(h.state().store.sources().unwrap().len(), 1);
    assert!(h.state().state.material_selected.is_none());
}

#[test]
fn task_assistant_new_paste_drops_the_auto_title_but_keeps_a_typed_one() {
    let state = State {
        view: View::Inbox,
        source_text: "关于机房搬迁的通知\n请信息中心准备方案。".into(),
        ..Default::default()
    };
    let mut h = bench(state, Store::open(":memory:").unwrap());
    h.get_by_label("识别").click();
    h.run();
    assert_eq!(h.state().state.source_title, "关于机房搬迁的通知");
    // 识别失败、原文还在框里，用户换贴一份：自动取的旧标题不能挂到新材料上。
    h.state_mut().state.request_extract = None;
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .focus();
    h.run();
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .type_text("另一份");
    h.run();
    assert!(h.state().state.source_title.is_empty());

    // 人自己写的标题不清。
    h.state_mut().state.source_title = "人工标题".into();
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .focus();
    h.run();
    h.get_by_role(egui::accesskit::Role::MultilineTextInput)
        .type_text("再改");
    h.run();
    assert_eq!(h.state().state.source_title, "人工标题");
}

#[test]
fn task_assistant_success_notice_floats_without_moving_the_columns() {
    let state = State {
        view: View::Inbox,
        ..Default::default()
    };
    let mut h = bench(state, Store::open(":memory:").unwrap());
    let before = h.get_by_label("识别").rect();
    h.state_mut().state.notice = Some("已保存任务".into());
    h.run();
    assert_eq!(h.get_by_label("识别").rect(), before);
    let toast = h.get_by_label("已保存任务").rect();
    assert!(toast.min.y > before.max.y, "提示应在页面下方：{toast:?}");
    h.get_by_label("已保存任务").click();
    h.run();
    assert!(h.state().state.notice.is_none());
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
    // 平时按正文显示，点「更新进展」才就地变成编辑框。
    assert!(h.query_by_label("原进展").is_some());
    h.get_by_label("更新进展").click();
    h.run();
    assert_eq!(
        h.state().state.progress_editing.as_deref(),
        Some(task.id.as_str())
    );
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
    // 更新完回到显示状态；再进编辑，点「取消」不写库。
    assert!(h.state().state.progress_editing.is_none());
    h.get_by_label("更新进展").click();
    h.run();
    h.state_mut().state.progress_text = "不要了".into();
    h.run();
    h.get_by_label("取消").click();
    h.run();
    assert!(h.state().state.progress_editing.is_none());
    assert_eq!(
        h.state().store.list(true).unwrap().remove(0).progress,
        "已完成现场核查"
    );
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

#[test]
fn task_assistant_untargeted_progress_can_be_built_into_a_new_task() {
    let mut store = Store::open(":memory:").unwrap();
    let mut other = Task::blank();
    other.name = "别的任务".into();
    store.save(&other, "创建", "").unwrap();
    let p: crate::task_assistant::extract::Proposal = serde_json::from_value(serde_json::json!({
        "kind": "progress", "task_id": null, "name": null, "next_step": null, "deadline": null,
        "progress": "完成10项数据标准修订", "evidence": "完成10项数据标准修订"
    }))
    .unwrap();
    store
        .commit_extraction(
            "总结",
            "专项完成10项数据标准修订。",
            "fp",
            "pasted",
            &[p],
            &[other],
        )
        .unwrap();
    let state = State {
        view: View::Inbox,
        ..Default::default()
    };
    let mut h = bench(state, store);
    h.get_by_label("建成新任务").click();
    h.run();
    let state = &h.state().state;
    assert_eq!(
        state.draft.as_ref().unwrap().progress,
        "完成10项数据标准修订"
    );
    assert!(state.candidate_create_id.is_some());
}

#[test]
fn permanent_owner_identity_wins_over_a_reused_local_id_and_same_name() {
    use crate::models::{VocabularyCategory as C, VocabularyEntry as E};
    let original = uuid::Uuid::new_v4().to_string();
    let other = E {
        id: 12,
        identity: uuid::Uuid::new_v4().to_string(),
        category: C::Person,
        canonical: "张宁".into(),
        ..Default::default()
    };
    let entries = vec![other];
    let (shown, valid) = owner::label_bound(&entries, Some(12), "张宁", &original);
    assert!(!valid);
    assert!(shown.contains("身份已失效"));
}
