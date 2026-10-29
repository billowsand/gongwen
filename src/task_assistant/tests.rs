use super::extract::{MAX_OUTPUT_BYTES, auto_title, parse_fill, prompt_fill};
use super::*;
use tempfile::tempdir;

fn task(name: &str) -> Task {
    let mut t = Task::blank();
    t.name = name.into();
    t.progress = format!("{name}目前进展");
    t
}

#[test]
fn old_task_table_migrates_repeatedly_and_remains_writable() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("old.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE tasks(id TEXT PRIMARY KEY,name TEXT NOT NULL,category TEXT NOT NULL,role TEXT NOT NULL,collaborators TEXT NOT NULL,goal TEXT NOT NULL,next_step TEXT NOT NULL,next_deadline TEXT,deadline TEXT,status TEXT NOT NULL,parent_id TEXT,needs_review INTEGER NOT NULL DEFAULT 0,archived INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,revision INTEGER NOT NULL DEFAULT 1,updated_at TEXT NOT NULL); INSERT INTO tasks(id,name,category,role,collaborators,goal,next_step,next_deadline,deadline,status,parent_id,needs_review,archived,deleted,revision,updated_at) VALUES('old','旧任务','未分类','负责','','旧目标','旧下一步',NULL,NULL,'未开始',NULL,0,0,0,1,'2026-10-01T00:00:00+08:00');") .unwrap();
    drop(conn);
    {
        let mut store = Store::open(&path).unwrap();
        let old = store.list(true).unwrap().remove(0);
        assert_eq!(old.name, "旧任务");
        assert_eq!(old.next_step, "旧下一步");
        assert_eq!(old.progress, "");
        store.save(&old, "编辑", "迁移后保存").unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.list(true).unwrap()[0].name, "旧任务");
    let columns = store
        .conn
        .prepare("PRAGMA table_info(tasks)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for column in ["owner_id", "owner_name", "progress", "risks"] {
        assert!(columns.iter().any(|x| x == column));
    }
}

#[test]
fn only_name_is_required_and_progress_replaces_with_history() {
    let mut store = Store::open(":memory:").unwrap();
    let mut t = Task::blank();
    t.name = "只填名称".into();
    store.save(&t, "创建", "").unwrap();
    let mut empty = t.clone();
    empty.name.clear();
    assert!(store.save(&empty, "编辑", "").is_err());
    store.record_progress(&t.id, "第一次").unwrap();
    store.record_progress(&t.id, "第二次").unwrap();
    assert_eq!(latest(&store, &t.id).progress, "第二次");
    let history = store.history_entries(&t.id).unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|e| e.action == "进展更新")
            .rev()
            .take(2)
            .map(|e| e.detail.as_str())
            .collect::<Vec<_>>(),
        ["第一次", "第二次"]
    );
    store
        .conn
        .execute("UPDATE tasks SET archived=1 WHERE id=?1", [&t.id])
        .unwrap();
    assert!(store.record_progress(&t.id, "只读").is_err());
}

#[test]
fn evidence_accepts_whitespace_and_fullwidth_punctuation_differences() {
    use super::extract::evidence_in;
    assert!(evidence_in("办公室，牵头 形成方案", "办公室,牵头形成方案"));
    assert!(evidence_in("任务　正在推进", "任务正在推进"));
    assert!(!evidence_in("任务正在推进", "任务已经完成"));
    assert!(!evidence_in("任务正在推进", ""));
    assert!(!evidence_in("任务正在推进", " \n"));
    let response = r#"{"items":[{"kind":"progress","task_id":null,"name":null,"goal":null,"next_step":"牵头","deadline":null,"collaborators":null,"evidence":"办公室,牵头"}]}"#;
    assert!(super::extract::parse_response(response, "办公室，牵头", &[]).is_ok());
    let fill = r#"{"fields":{"progress":{"value":"办公室","evidence":"不匹配"}},"others":0}"#;
    let parsed = parse_fill(fill, "办公室牵头", "2026-10-29").unwrap();
    assert_eq!(parsed.progress.unwrap().evidence, "办公室");
    let fill = r#"{"fields":{"progress":{"value":"办公室,牵头","evidence":"办公室,牵头"}}}"#;
    assert!(
        parse_fill(fill, "办公室，牵头", "2026-10-29")
            .unwrap()
            .progress
            .is_some()
    );

    let mut store = Store::open(":memory:").unwrap();
    let mut target = task("负责人任务");
    target.owner_id = Some(7);
    target.owner_name = "旧负责人".into();
    store.save(&target, "创建", "").unwrap();
    let target = latest(&store, &target.id);
    let mut candidate = proposal("owner", Some(target.id.clone()), "办公室,牵头");
    candidate.owner = Some("王敏".into());
    store
        .commit_extraction(
            "材料",
            "办公室，牵头",
            "punct-fp",
            "test",
            &[candidate],
            std::slice::from_ref(&target),
        )
        .unwrap();
    let id = store.candidates().unwrap()[0].0.clone();
    store.apply_candidate(&id, &target.id).unwrap();
    assert_eq!(latest(&store, &target.id).owner_name, "旧负责人");
}

#[test]
fn responsibility_legacy_kind_uses_owner_only_when_person_is_resolved() {
    for resolved in [false, true] {
        let mut store = Store::open(":memory:").unwrap();
        let mut target = task("任务");
        target.owner_id = Some(1);
        target.owner_name = "原负责人".into();
        store.save(&target, "创建", "").unwrap();
        let target = latest(&store, &target.id);
        let mut candidate = proposal("owner", Some(target.id.clone()), "王敏牵头");
        candidate.owner = Some("王敏".into());
        store
            .commit_extraction(
                "材料",
                "王敏牵头",
                if resolved { "resolved" } else { "unresolved" },
                "test",
                &[candidate],
                std::slice::from_ref(&target),
            )
            .unwrap();
        let (id, _, payload, _, _) = store.candidates().unwrap().remove(0);
        let mut p: super::extract::Proposal = serde_json::from_str(&payload).unwrap();
        p.kind = "responsibility".into();
        if resolved {
            p.owner_id = Some(8);
        }
        store
            .conn
            .execute(
                "UPDATE task_candidates SET kind='responsibility' WHERE id=?1",
                [&id],
            )
            .unwrap();
        store.edit_candidate(&id, &p).unwrap();
        store.apply_candidate(&id, &target.id).unwrap();
        let after = latest(&store, &target.id);
        if resolved {
            assert_eq!(
                (after.owner_id, after.owner_name.as_str()),
                (Some(8), "王敏")
            );
        } else {
            assert_eq!(
                (after.owner_id, after.owner_name.as_str()),
                (Some(1), "原负责人")
            );
        }
    }
}

#[test]
fn old_scope_candidates_apply_only_name_and_keep_legacy_columns() {
    let mut store = Store::open(":memory:").unwrap();
    let mut t = task("旧名称");
    t.next_step = "人工安排".into();
    store.save(&t, "创建", "").unwrap();
    let t = latest(&store, &t.id);
    let mut p = proposal("scope", Some(t.id.clone()), "调整名称");
    p.name = Some("新名称".into());
    p.goal = Some("旧目标".into());
    p.next_step = Some("不得采纳的旧字段".into());
    store
        .commit_extraction(
            "旧材料",
            "调整名称",
            "old-scope",
            "test",
            &[p],
            std::slice::from_ref(&t),
        )
        .unwrap();
    let (id, _, payload, _, _) = store.candidates().unwrap().remove(0);
    let p = serde_json::from_str(&payload).unwrap();
    store.edit_candidate(&id, &p).unwrap();
    store.apply_candidate(&id, &t.id).unwrap();
    let after = latest(&store, &t.id);
    assert_eq!(after.name, "新名称");
    assert_eq!(after.next_step, "人工安排");
    assert_eq!(after.progress, t.progress);
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
fn invalid_real_calendar_date_and_name_only_is_valid() {
    let mut t = task("A");
    t.deadline = Some("2026-02-30".into());
    assert!(super::model::valid_task(&t).is_err());
    t.deadline = Some("2024-02-29".into());
    t.name.clear();
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
    let fake = raw
        .replace("2026年12月完成试点验收", "假摘录")
        .replace("\"验收\"", "\"无关内容\"");
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
        owner: None,
        owner_id: None,
        progress: None,
        risks: None,
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
        owner: None,
        owner_id: None,
        progress: None,
        risks: None,
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
        goal: None,
        next_step: Some("下一步".into()),
        deadline: None,
        collaborators: None,
        owner: None,
        owner_id: None,
        progress: None,
        risks: None,
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

#[test]
fn task_assistant_fill_validates_fields_individually() {
    let source = "办公室牵头编制方案，完成测算后报审，2026-02-30，交领导复核。";
    let raw = serde_json::json!({"fields":{
        "name":{"value":" 编制方案 ","evidence":"办公室牵头编制方案"},
        "owner":{"value":"办公室","evidence":"办公室牵头"},
        "progress":{"value":"方案已编制","evidence":"编制方案"},
        "next_step":{"value":"完成测算","evidence":"完成测算后报审"},
        "deadline":{"value":"2026-02-30","evidence":"2026-02-30"},
        "risks":{"value":"暂无","evidence":"交领导复核"}
    },"others":2})
    .to_string();
    let fill = parse_fill(&raw, source, "2026-10-29").unwrap();
    assert_eq!(fill.name.unwrap().value, "编制方案");
    assert_eq!(fill.next_step.unwrap().value, "完成测算");
    assert!(fill.owner.is_some() && fill.progress.is_some() && fill.risks.is_some());
    assert!(fill.deadline.is_none());
    assert_eq!((fill.dropped, fill.others), (1, 2));
    let legacy = r#"{"fields":{"goal":{"value":"旧栏","evidence":"编制方案"}},"others":0}"#;
    assert!(parse_fill(legacy, source, "2026-10-29").is_err());
}

#[test]
fn task_assistant_fill_rejects_protocol_and_keeps_only_eligible_context() {
    for raw in [
        "不是JSON",
        r#"{"fields":{},"others":0,"extra":1}"#,
        r#"{"fields":{"status":null},"others":0}"#,
        r#"{"fields":{},"others":31}"#,
    ] {
        assert!(parse_fill(raw, "原文", "2026-10-29").is_err());
    }
    assert!(parse_fill(&"x".repeat(MAX_OUTPUT_BYTES + 1), "原文", "2026-10-29").is_err());
    let malformed = r#"{"fields":{"name":3},"others":0}"#;
    assert_eq!(
        parse_fill(malformed, "原文", "2026-10-29").unwrap().dropped,
        1
    );
}

#[test]
fn task_assistant_fill_prompt_has_today_and_only_six_fields() {
    let mut tasks = (0..60)
        .map(|i| task(&format!("上下文{i}")))
        .collect::<Vec<_>>();
    tasks[0].deleted = true;
    tasks[1].archived = true;
    let (system, user) = prompt_fill("交办原文", "2026-10-28", &task("当前任务"));
    assert!(user.contains("2026-10-28") && user.contains("当前任务") && user.contains("交办原文"));
    assert!(!user.contains("专项协调") && tasks.iter().all(|t| !user.contains(&t.id)));
    for field in [
        "name",
        "owner",
        "progress",
        "next_step",
        "risks",
        "deadline",
    ] {
        assert!(system.contains(field));
    }
    assert!(
        !system.contains("goal") && !system.contains("collaborators") && system.contains("下周")
    );
}

#[test]
fn task_assistant_auto_title_is_trimmed_bounded_and_stable() {
    use chrono::TimeZone;
    let now = chrono::Local
        .with_ymd_and_hms(2026, 10, 28, 9, 30, 0)
        .single()
        .unwrap();
    let source = "\n  《办公室关于开展专项协调工作的通知》。\n正文";
    assert_eq!(auto_title(source, now), "办公室关于开展专项协调工作的通知");
    assert_eq!(auto_title(&"长".repeat(50), now).chars().count(), 30);
    assert_eq!(auto_title("\n （好）。", now), "粘贴材料 10-28 09:30");
    assert_eq!(
        auto_title(source, now),
        auto_title(source, now + chrono::Duration::hours(1))
    );
}

#[test]
fn task_assistant_fill_save_source_is_atomic_reused_and_separate_from_batch() {
    use sha2::{Digest, Sha256};
    let mut store = Store::open(":memory:").unwrap();
    let t = task("任务");
    let title = "办公室交办事项";
    let source = "办公室要求形成方案报审。";
    store
        .save_with_source(&t, "创建", "填表", title, source)
        .unwrap();
    let first = store.history_entries(&t.id).unwrap().remove(0);
    assert!(first.source_id.is_some());
    assert_eq!(
        store.source(first.source_id.as_ref().unwrap()).unwrap().0,
        title
    );
    let batch = Sha256::digest(format!("{title}\0{source}").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert!(!store.has_source(&batch).unwrap());
    let mut current = store.list(true).unwrap().remove(0);
    current.next_step = "送审".into();
    store
        .save_with_source(&current, "编辑", "填表", title, source)
        .unwrap();
    let after = store.history_entries(&t.id).unwrap();
    assert_eq!(after[0].source_id, first.source_id);
    let count = || {
        store
            .conn
            .query_row("SELECT count(*) FROM task_sources", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    assert_eq!(count(), 1);
    let mut invalid = task("无效");
    invalid.category = "不存在".into();
    assert!(
        store
            .save_with_source(&invalid, "创建", "", "另份材料", "另一份原文")
            .is_err()
    );
    assert_eq!(
        store
            .conn
            .query_row("SELECT count(*) FROM task_sources", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    invalid.name.clear();
    assert!(
        store
            .save_with_source(&invalid, "创建", "", "其它", "原文")
            .is_err()
    );
    assert_eq!(store.list(true).unwrap().len(), 1);
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
        owner: None,
        owner_id: None,
        progress: None,
        risks: None,
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

#[test]
fn task_assistant_fill_tolerates_missing_others_count() {
    let raw = r#"{"fields":{"name":{"value":"形成方案","evidence":"形成方案"}}}"#;
    let fill = super::extract::parse_fill(raw, "要求形成方案", "2026-10-29").unwrap();
    assert_eq!(fill.others, 0);
    assert_eq!(fill.name.unwrap().value, "形成方案");
}

#[test]
fn task_assistant_batch_evidence_fallback_needs_a_long_body_field() {
    let source = "张三负责，信息中心于十一月前完成数据汇总并报送办公室。";
    // 摘录对不上、只有人名能在原文里找到：不能拿人名当出处。
    let short = r#"{"items":[{"kind":"owner","task_id":null,"name":null,"next_step":null,"deadline":null,"owner":"张三","evidence":"杜撰的摘录"}]}"#;
    assert!(super::extract::parse_response(short, source, &[]).is_err());
    // 足够长、原样出现在原文里的下一步可以顶替摘录。
    let long = r#"{"items":[{"kind":"progress","task_id":null,"name":null,"next_step":"完成数据汇总并报送办公室","deadline":null,"evidence":"杜撰的摘录"}]}"#;
    let items = super::extract::parse_response(long, source, &[]).unwrap();
    assert_eq!(items[0].evidence, "完成数据汇总并报送办公室");
}

#[test]
fn task_assistant_dates_must_match_the_source_and_the_nearest_year() {
    use super::extract::date_supported;
    let today = "2026-10-29";
    // 只写月日：必须是今天起最近的那一个，模型编的年份不认。
    assert!(date_supported("12月15日前完成", "2026-12-15", today));
    assert!(!date_supported("12月15日前完成", "2023-12-15", today));
    // 已经过去的月日顺延到明年。
    assert!(date_supported("3月1日前报送", "2027-03-01", today));
    assert!(!date_supported("3月1日前报送", "2026-03-01", today));
    // 原文写了年份或完整日期就照原文。
    assert!(date_supported("2027年1月5日启动", "2027-01-05", today));
    assert!(date_supported("截止 2026-11-15", "2026-11-15", today));
    // 原文没有这个日期：不认（相对说法如「下周五」不能被换算成某一天）。
    assert!(!date_supported("下周五前发出", "2026-11-06", today));
}

#[test]
fn task_assistant_model_output_in_code_fences_and_bare_values_still_parse() {
    let fenced = "```json\n{\"items\":[]}\n```";
    assert!(
        super::extract::parse_response(fenced, "原文", &[])
            .unwrap()
            .is_empty()
    );
    // 小模型只给值、不给摘录：值本身在原文里就接受，编出来的（如「待开始」）丢掉。
    let source = "请李明牵头，11月15日前把全市政务数据目录梳理完。";
    let bare = "```json\n{\"fields\":{\"name\":\"全市政务数据目录梳理\",\"owner\":\"李明\",\"progress\":\"待开始\",\"next_step\":null,\"risks\":null,\"deadline\":\"2026-11-15\"},\"others\":0}\n```";
    let fill = parse_fill(bare, source, "2026-10-29").unwrap();
    assert_eq!(fill.name.unwrap().value, "全市政务数据目录梳理");
    assert_eq!(fill.owner.unwrap().value, "李明");
    assert_eq!(fill.deadline.unwrap().value, "2026-11-15");
    assert!(fill.progress.is_none());
    assert_eq!(fill.dropped, 1);
}

#[test]
fn task_assistant_batch_drops_deadlines_with_invented_years() {
    let source = "会议确定由赵磊负责开展固定资产盘点，12月15日前完成。";
    let mut wrong = super::extract::Proposal {
        kind: "new_task".into(),
        task_id: None,
        name: Some("固定资产盘点".into()),
        goal: None,
        next_step: None,
        deadline: Some("2023-12-15".into()),
        collaborators: None,
        owner: None,
        owner_id: None,
        progress: None,
        risks: None,
        speaker: None,
        about: None,
        note: None,
        evidence: "12月15日前完成".into(),
    };
    let mut only_deadline = wrong.clone();
    only_deadline.kind = "deadline".into();
    let kept = super::extract::check_proposal_dates(
        vec![wrong.clone(), only_deadline],
        source,
        "2026-10-29",
    );
    // 新任务保留、错年份的期限清空；纯「期限调整」候选没了期限就整条拿掉。
    assert_eq!(kept.len(), 1);
    assert!(kept[0].deadline.is_none());
    wrong.deadline = Some("2026-12-15".into());
    let kept = super::extract::check_proposal_dates(vec![wrong], source, "2026-10-29");
    assert_eq!(kept[0].deadline.as_deref(), Some("2026-12-15"));
}
