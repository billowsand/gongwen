use super::*;
use tempfile::tempdir;

fn task(name: &str) -> Task {
    let mut t = Task::blank();
    t.name = name.into();
    t.goal = format!("{name}完成条件");
    t
}
#[test]
fn sqlite_reopen_and_category_migration_preserve_tasks() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tasks.db");
    {
        let mut s = Store::open(&path).unwrap();
        s.add_category("试点").unwrap();
        let mut t = task("A");
        t.category = "试点".into();
        s.save(&t, "创建", "手工").unwrap();
        s.move_category("试点", "未分类").unwrap();
    }
    {
        let s = Store::open(&path).unwrap();
        let rows = s.list(false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].category, "未分类");
        assert_eq!(rows[0].revision, 2);
    }
}
#[test]
fn invalid_real_calendar_date_and_empty_goal_are_rejected() {
    let mut t = task("A");
    t.deadline = Some("2026-02-30".into());
    assert!(super::model::valid_task(&t).is_err());
    t.deadline = Some("2024-02-29".into());
    t.goal.clear();
    assert!(super::model::valid_task(&t).is_err());
}
#[test]
fn combined_parent_and_prerequisite_deadlock_is_rejected() {
    let a = task("A");
    let mut b = task("B");
    b.parent_id = Some(a.id.clone());
    let mut child_depends_parent = b.clone();
    child_depends_parent.prerequisites = vec![a.id.clone()];
    assert!(
        super::relations::validate_graph(&[a.clone(), b.clone()], &child_depends_parent).is_err()
    );
    assert!(super::relations::validate_graph(&[a], &b).is_ok());
}
#[test]
fn canceled_prerequisite_and_child_do_not_satisfy_completion() {
    let mut p = task("前置");
    p.status = Status::Cancelled;
    let mut t = task("目标");
    t.prerequisites.push(p.id.clone());
    assert!(super::relations::can_enter(&[p], &t, Status::InProgress).is_err());
    let parent = task("上级");
    let mut child = task("子任务");
    child.parent_id = Some(parent.id.clone());
    child.status = Status::Cancelled;
    assert!(super::relations::can_enter(&[child], &parent, Status::Done).is_err());
}
#[test]
fn model_output_requires_existing_ids_and_real_evidence() {
    let mut t = task("已有");
    t.revision = 4;
    let text = "请于2026年12月完成试点验收。";
    let raw = format!(
        r#"{{"items":[{{"kind":"progress","task_id":"{}","name":null,"goal":null,"next_step":"验收","deadline":null,"collaborators":null,"evidence":"2026年12月完成试点验收"}}]}}"#,
        t.id
    );
    let got = super::extract::parse_response(&raw, text, &[t.clone()]).unwrap();
    assert_eq!(got.len(), 1);
    let fake = raw.replace("2026年12月完成试点验收", "假摘录");
    assert!(super::extract::parse_response(&fake, text, &[t.clone()]).is_err());
    let alien = raw.replace(&t.id, "unknown");
    assert!(super::extract::parse_response(&alien, text, &[t]).is_err());
}
#[test]
fn extraction_snapshot_and_candidates_commit_together_and_dedupe() {
    let dir = tempdir().unwrap();
    let mut s = Store::open(dir.path().join("tasks.db")).unwrap();
    let t = task("已有");
    s.save(&t, "创建", "").unwrap();
    let p = super::extract::Proposal {
        kind: "progress".into(),
        task_id: Some(t.id.clone()),
        name: None,
        goal: None,
        next_step: Some("落实".into()),
        deadline: None,
        collaborators: None,
        speaker: None,
        about: None,
        note: None,
        evidence: "原文证据".into(),
    };
    let n = s
        .commit_extraction(
            "材料",
            "这是原文证据",
            "fingerprint",
            "test",
            &[p],
            std::slice::from_ref(&t),
        )
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        s.commit_extraction("材料", "这是原文证据", "fingerprint", "test", &[], &[t])
            .unwrap(),
        0
    );
    assert_eq!(s.candidates().unwrap()[0].1, "progress");
}
#[test]
fn stale_candidate_requires_explicit_recheck_and_never_overwrites_edits() {
    let dir = tempdir().unwrap();
    let mut s = Store::open(dir.path().join("db.sqlite")).unwrap();
    let mut t = task("目标");
    s.save(&t, "创建", "").unwrap();
    let p = super::extract::Proposal {
        kind: "progress".into(),
        task_id: Some(t.id.clone()),
        name: None,
        goal: None,
        next_step: Some("模型建议".into()),
        deadline: None,
        collaborators: None,
        speaker: None,
        about: None,
        note: None,
        evidence: "原文摘录".into(),
    };
    s.commit_extraction(
        "材料",
        "这是原文摘录",
        "fp",
        "test",
        &[p],
        std::slice::from_ref(&t),
    )
    .unwrap();
    let id = s.candidates().unwrap()[0].0.clone();
    t = s
        .list(true)
        .unwrap()
        .into_iter()
        .find(|x| x.id == t.id)
        .unwrap();
    t.next_step = "人工新值".into();
    s.save(&t, "编辑", "人工").unwrap();
    assert!(s.apply_candidate(&id, &t.id).is_err());
    assert_eq!(s.list(false).unwrap()[0].next_step, "人工新值");
    s.recheck_candidate(&id, &t.id).unwrap();
    s.apply_candidate(&id, &t.id).unwrap();
    assert_eq!(s.list(false).unwrap()[0].next_step, "模型建议");
}
#[test]
fn new_task_candidate_and_source_history_are_applied_together() {
    let dir = tempdir().unwrap();
    let mut s = Store::open(dir.path().join("db.sqlite")).unwrap();
    let p = super::extract::Proposal {
        kind: "new_task".into(),
        task_id: None,
        name: Some("建议任务".into()),
        goal: Some("完成条件".into()),
        next_step: Some("下一步".into()),
        deadline: None,
        collaborators: None,
        speaker: None,
        about: None,
        note: None,
        evidence: "原文摘录".into(),
    };
    s.commit_extraction("材料", "这是原文摘录", "fp", "test", &[p], &[])
        .unwrap();
    let id = s.candidates().unwrap()[0].0.clone();
    let mut t = super::extract::proposal_task(
        &serde_json::from_str(&s.candidates().unwrap()[0].2).unwrap(),
    );
    t.needs_review = false;
    s.create_from_candidate(&id, &t).unwrap();
    assert_eq!(s.candidates().unwrap()[0].4, "accepted");
    assert_eq!(
        s.history_entries(&t.id).unwrap()[0].action,
        "采纳新任务候选"
    );
    // 搜索一次取全：材料原文里的词能找到据此建立的任务，历史条目带来源可点开原文。
    let hits = s.source_hits("原文摘录").unwrap();
    assert!(hits.contains(&t.id));
    assert!(s.source_hits("不存在的词").unwrap().is_empty());
    let source = s.history_entries(&t.id).unwrap()[0]
        .source_id
        .clone()
        .unwrap();
    assert_eq!(s.source(&source).unwrap().1, "这是原文摘录");
}
#[test]
fn deleting_task_unlinks_and_marks_related_tasks_for_review() {
    let dir = tempdir().unwrap();
    let mut s = Store::open(dir.path().join("db.sqlite")).unwrap();
    let parent = task("目标");
    s.save(&parent, "创建", "").unwrap();
    let mut child = task("子任务");
    child.parent_id = Some(parent.id.clone());
    s.save(&child, "创建", "").unwrap();
    let mut next = task("后续");
    next.prerequisites.push(parent.id.clone());
    s.save(&next, "创建", "").unwrap();
    s.soft_delete(&parent.id).unwrap();
    let rows = s.list(true).unwrap();
    let child = rows.iter().find(|t| t.id == child.id).unwrap();
    let next = rows.iter().find(|t| t.id == next.id).unwrap();
    assert!(child.parent_id.is_none() && child.needs_review);
    assert!(next.prerequisites.is_empty() && next.needs_review);
}
#[test]
fn auto_organize_preference_persists_and_defaults_off() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    {
        let s = Store::open(&path).unwrap();
        assert!(!s.auto().unwrap());
        s.set_auto(true).unwrap();
    }
    {
        let s = Store::open(&path).unwrap();
        assert!(s.auto().unwrap());
    }
}

fn latest(s: &Store, id: &str) -> Task {
    s.list(true)
        .unwrap()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
}
fn proposal(kind: &str, id: Option<String>, evidence: &str) -> super::extract::Proposal {
    super::extract::Proposal {
        kind: kind.into(),
        task_id: id,
        name: None,
        goal: None,
        next_step: None,
        deadline: None,
        collaborators: None,
        speaker: None,
        about: None,
        note: None,
        evidence: evidence.into(),
    }
}

#[test]
fn manual_edit_revision_is_checked_and_archive_requires_terminal_state() {
    let mut s = Store::open(":memory:").unwrap();
    let t = task("任务");
    s.save(&t, "创建", "").unwrap();
    let mut stale = t.clone();
    stale.next_step = "旧草稿".into();
    assert!(s.save(&stale, "编辑", "").is_err());
    let mut current = latest(&s, &t.id);
    current.archived = true;
    assert!(s.save(&current, "归档", "").is_err());
    current.archived = false;
    current.status = Status::Done;
    s.save(&current, "完成", "").unwrap();
    current = latest(&s, &t.id);
    current.archived = true;
    s.save(&current, "归档", "").unwrap();
    s.restore_task(&t.id).unwrap();
    assert_eq!(latest(&s, &t.id).status, Status::Done);
    assert!(!latest(&s, &t.id).archived);
}

#[test]
fn reopening_child_marks_all_completed_ancestors_and_indirect_followers() {
    let mut s = Store::open(":memory:").unwrap();
    let root = task("总任务");
    s.save(&root, "创建", "").unwrap();
    let mut parent = task("中间任务");
    parent.parent_id = Some(root.id.clone());
    s.save(&parent, "创建", "").unwrap();
    let mut child = task("子任务");
    child.parent_id = Some(parent.id.clone());
    s.save(&child, "创建", "").unwrap();
    for id in [&child.id, &parent.id, &root.id] {
        let mut t = latest(&s, id);
        t.status = Status::Done;
        s.save(&t, "完成", "").unwrap();
    }
    let mut follower = task("后续");
    follower.prerequisites.push(root.id.clone());
    follower.status = Status::Done;
    s.save(&follower, "创建", "").unwrap();
    let mut t = latest(&s, &child.id);
    t.status = Status::Todo;
    s.save(&t, "重开", "").unwrap();
    for id in [&parent.id, &root.id, &follower.id] {
        assert!(latest(&s, id).needs_review);
    }
}

#[test]
fn existing_child_of_completed_parent_is_editable_but_new_child_is_rejected() {
    let mut s = Store::open(":memory:").unwrap();
    let p = task("父");
    s.save(&p, "创建", "").unwrap();
    let mut c = task("子");
    c.parent_id = Some(p.id.clone());
    c.status = Status::Done;
    s.save(&c, "创建", "").unwrap();
    let mut parent = latest(&s, &p.id);
    parent.status = Status::Done;
    s.save(&parent, "完成", "").unwrap();
    let mut child = latest(&s, &c.id);
    child.next_step = "补记已完成事项".into();
    s.save(&child, "编辑", "").unwrap();
    let mut new = task("新子任务");
    new.parent_id = Some(p.id);
    assert!(s.save(&new, "创建", "").is_err());
}

#[test]
fn completing_parent_requires_reviewed_children_and_cancelled_is_not_complete() {
    let p = task("父");
    let mut child = task("子");
    child.parent_id = Some(p.id.clone());
    child.status = Status::Done;
    child.needs_review = true;
    assert!(super::relations::can_enter(&[child], &p, Status::Done).is_err());
}

#[test]
fn completion_candidate_closes_task_only_on_explicit_acceptance() {
    let mut s = Store::open(":memory:").unwrap();
    let t = task("提交材料");
    s.save(&t, "创建", "").unwrap();
    let tasks = s.list(true).unwrap();
    let p = proposal("completion_evidence", Some(t.id.clone()), "材料已收悉");
    s.commit_extraction("回执", "材料已收悉", "receipt", "test", &[p], &tasks)
        .unwrap();
    assert_eq!(latest(&s, &t.id).status, Status::Todo);
    let id = s.candidates().unwrap()[0].0.clone();
    s.apply_candidate(&id, &t.id).unwrap();
    assert_eq!(latest(&s, &t.id).status, Status::Done);
    assert!(s.apply_candidate(&id, &t.id).is_err());
}

#[test]
fn ordinary_progress_keeps_relationship_review_and_opinion_can_record_while_blocked() {
    let mut s = Store::open(":memory:").unwrap();
    let mut t = task("任务");
    t.needs_review = true;
    s.save(&t, "创建", "").unwrap();
    let tasks = s.list(true).unwrap();
    let mut p = proposal("progress", Some(t.id.clone()), "已有反馈");
    p.next_step = Some("继续协调".into());
    s.commit_extraction("反馈", "已有反馈", "a", "test", &[p], &tasks)
        .unwrap();
    let id = s.candidates().unwrap()[0].0.clone();
    s.apply_candidate(&id, &t.id).unwrap();
    assert!(latest(&s, &t.id).needs_review);
    let tasks = s.list(true).unwrap();
    let p = proposal("opinion", Some(t.id.clone()), "建议先核实");
    s.commit_extraction("意见", "建议先核实", "b", "test", &[p], &tasks)
        .unwrap();
    let id = s
        .candidates()
        .unwrap()
        .into_iter()
        .find(|x| x.4 == "pending")
        .unwrap()
        .0;
    s.apply_candidate(&id, &t.id).unwrap();
    assert!(latest(&s, &t.id).needs_review);
}

#[test]
fn unresolved_target_is_reviewable_and_tentative_deadline_does_not_change_fields() {
    let t = task("任务");
    let p = proposal("progress", None, "已有反馈");
    let raw = serde_json::json!({"items":[p]}).to_string();
    assert_eq!(
        super::extract::parse_response(&raw, "已有反馈", &[]).unwrap()[0].task_id,
        None
    );
    let mut p = proposal("deadline", Some(t.id.clone()), "建议延期至2026-12-01");
    p.deadline = Some("2026-12-01".into());
    let raw = serde_json::json!({"items":[p]}).to_string();
    let parsed =
        super::extract::parse_response(&raw, "建议延期至2026-12-01", std::slice::from_ref(&t))
            .unwrap();
    assert_eq!(parsed[0].kind, "decision");
    let mut s = Store::open(":memory:").unwrap();
    s.save(&t, "创建", "").unwrap();
    let tasks = s.list(true).unwrap();
    s.commit_extraction(
        "待决",
        "建议延期至2026-12-01",
        "suggestion",
        "test",
        &parsed,
        &tasks,
    )
    .unwrap();
    let id = s.candidates().unwrap()[0].0.clone();
    s.apply_candidate(&id, &t.id).unwrap();
    assert_eq!(latest(&s, &t.id).deadline, None);
}

#[test]
fn invalid_candidate_rolls_back_entire_material_and_acceptance() {
    let mut s = Store::open(":memory:").unwrap();
    let t = task("任务");
    s.save(&t, "创建", "").unwrap();
    let tasks = s.list(true).unwrap();
    let p = proposal("progress", Some(t.id.clone()), "不存在的摘录");
    assert!(
        s.commit_extraction("材料", "原文", "invalid", "test", &[p], &tasks)
            .is_err()
    );
    assert!(!s.has_source("invalid").unwrap());
    assert!(s.candidates().unwrap().is_empty());
    let p = proposal("completion_evidence", Some(t.id.clone()), "报称完成");
    s.commit_extraction("报告", "报称完成", "valid", "test", &[p], &tasks)
        .unwrap();
    let mut changed = latest(&s, &t.id);
    changed.needs_review = true;
    s.save(&changed, "编辑", "").unwrap();
    let id = s.candidates().unwrap()[0].0.clone();
    s.recheck_candidate(&id, &t.id).unwrap();
    assert!(s.apply_candidate(&id, &t.id).is_err());
    assert_eq!(s.candidates().unwrap()[0].4, "pending");
    assert_eq!(latest(&s, &t.id).status, Status::Todo);
    let mut oversized: super::extract::Proposal =
        serde_json::from_str(&s.candidates().unwrap()[0].2).unwrap();
    oversized.note = Some("x".repeat(super::model::MAX_TEXT + 1));
    assert!(s.edit_candidate(&id, &oversized).is_err());
}
