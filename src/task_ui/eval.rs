//! 任务助手的模型评测：拿真实模型跑一遍「单任务填写」与「批量材料整理」整条链。
//!
//! 走的是产品代码本身：`prompt_fill` / `prompt` → `lmstudio::generate` →
//! `parse_fill` / `parse_response` → `apply_fill`（含标准词库负责人匹配）。用例在
//! `tests/fixtures/task-eval/cases.json`，报告写到 `tmp/task-eval/`。默认连本机 LM Studio：
//!
//! ```text
//! GONGWEN_TASK_EVAL_URL=http://127.0.0.1:12345/v1 GONGWEN_TASK_EVAL_MODEL=qwen/qwen3.5-9b \
//!   cargo test --locked task_assistant_model_eval -- --ignored --nocapture
//! ```
//!
//! `GONGWEN_TASK_EVAL_ONLY` 可填用例 id 前缀（如 `F05`、`B`）只跑一部分。
use super::{State, apply_fill};
use crate::models::{LmStudioConfig, VocabularyCategory, VocabularyEntry};
use crate::task_assistant::Task;
use crate::task_assistant::extract::{parse_fill, parse_response, prompt, prompt_fill};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write as _;
use std::time::Instant;

#[derive(Deserialize)]
struct Cases {
    today: String,
    vocabulary: Vocab,
    fill: Vec<FillCase>,
    batch: Vec<BatchCase>,
}
#[derive(Deserialize)]
struct Vocab {
    units: Vec<UnitRow>,
    people: Vec<PersonRow>,
}
#[derive(Deserialize)]
struct UnitRow {
    code: String,
    name: String,
    abbr: String,
}
#[derive(Deserialize)]
struct PersonRow {
    id: u64,
    name: String,
    position: String,
    unit: String,
    #[serde(default)]
    aliases: Vec<String>,
}
#[derive(Deserialize)]
struct FillCase {
    id: String,
    note: String,
    text: String,
    #[serde(default)]
    draft: Option<DraftPreset>,
    expect: serde_json::Map<String, Value>,
}
#[derive(Deserialize)]
struct DraftPreset {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    owner_id: Option<u64>,
}
#[derive(Deserialize)]
struct BatchCase {
    id: String,
    note: String,
    tasks: Vec<TaskRow>,
    title: String,
    text: String,
    expect: serde_json::Map<String, Value>,
}
#[derive(Deserialize)]
struct TaskRow {
    id: String,
    name: String,
    next_step: String,
}

/// 一条用例的结果：每项检查一行，`raw` 留模型原始输出便于复盘。
struct Outcome {
    id: String,
    note: String,
    seconds: f64,
    error: Option<String>,
    checks: Vec<(String, bool, String)>,
    raw: String,
}
impl Outcome {
    fn passed(&self) -> bool {
        self.error.is_none() && self.checks.iter().all(|c| c.1)
    }
}

fn vocabulary(v: &Vocab) -> Vec<VocabularyEntry> {
    let mut out = Vec::new();
    for (i, u) in v.units.iter().enumerate() {
        out.push(VocabularyEntry {
            id: 1 + i as u64,
            category: VocabularyCategory::Unit,
            code: u.code.clone(),
            canonical: u.name.clone(),
            abbr: u.abbr.clone(),
            ..Default::default()
        });
    }
    for p in &v.people {
        out.push(VocabularyEntry {
            id: p.id,
            category: VocabularyCategory::Person,
            canonical: p.name.clone(),
            position: p.position.clone(),
            unit: p.unit.clone(),
            aliases: p.aliases.clone(),
            ..Default::default()
        });
    }
    out
}

fn config(max_tokens: u32) -> LmStudioConfig {
    LmStudioConfig {
        base_url: std::env::var("GONGWEN_TASK_EVAL_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:12345/v1".into()),
        model: std::env::var("GONGWEN_TASK_EVAL_MODEL")
            .unwrap_or_else(|_| "qwen/qwen3.5-9b".into()),
        // 与产品里 `start_task_fill` / `start_task_material` 的取值一致。
        temperature: 0.0,
        max_tokens,
        timeout_seconds: 300,
        ..Default::default()
    }
}

/// `present` / `absent` / `contains:片段` 三种写法。
fn text_check(expect: &Value, actual: &str) -> bool {
    match expect.as_str().unwrap_or_default() {
        "present" => !actual.trim().is_empty(),
        "absent" => actual.trim().is_empty(),
        s => s
            .strip_prefix("contains:")
            .is_some_and(|needle| actual.contains(needle)),
    }
}

fn run_fill(case: &FillCase, today: &str, vocab: &[VocabularyEntry]) -> Outcome {
    let mut draft = Task::blank();
    if let Some(preset) = &case.draft {
        if let Some(name) = &preset.name {
            draft.name = name.clone();
        }
        if let Some(id) = preset.owner_id {
            draft.owner_id = Some(id);
            draft.owner_name = vocab
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.canonical.clone())
                .unwrap_or_default();
        }
    }
    let before = draft.clone();
    let mut outcome = Outcome {
        id: case.id.clone(),
        note: case.note.clone(),
        seconds: 0.0,
        error: None,
        checks: Vec::new(),
        raw: String::new(),
    };
    let (system, user) = prompt_fill(&case.text, today, &draft);
    let start = Instant::now();
    let raw = crate::lmstudio::generate(&config(4096), &system, &user);
    outcome.seconds = start.elapsed().as_secs_f64();
    let raw = match raw {
        Ok(raw) => raw,
        Err(e) => {
            outcome.error = Some(format!("模型调用失败：{e:#}"));
            return outcome;
        }
    };
    outcome.raw = raw.clone();
    let fill = match parse_fill(&raw, &case.text, today) {
        Ok(fill) => fill,
        Err(e) => {
            outcome.error = Some(format!("协议解析失败：{e:#}"));
            return outcome;
        }
    };
    let (others, dropped) = (fill.others, fill.dropped);
    let mut state = State {
        fill_text: case.text.clone(),
        ..Default::default()
    };
    apply_fill(&mut state, &mut draft, fill, vocab);
    let note = state.fill_owner_note.clone().unwrap_or_default();
    for (key, want) in &case.expect {
        let (ok, got) = match key.as_str() {
            "protocol_ok" => (true, "已按约定 JSON 解析".into()),
            "name" => (text_check(want, &draft.name), draft.name.clone()),
            "progress" => (text_check(want, &draft.progress), draft.progress.clone()),
            "next_step" => (text_check(want, &draft.next_step), draft.next_step.clone()),
            "risks" => (text_check(want, &draft.risks), draft.risks.clone()),
            "name_untouched" => (draft.name == before.name, draft.name.clone()),
            "owner_untouched" => (
                draft.owner_id == before.owner_id,
                format!("{:?}", draft.owner_id),
            ),
            "owner_id" => (
                draft.owner_id == want.as_u64().or(before.owner_id),
                format!("{:?} {}", draft.owner_id, draft.owner_name),
            ),
            "owner_note" => (
                match want.as_str() {
                    Some("same_name") => note.contains("同名"),
                    Some("missing") => note.contains("没有此人"),
                    _ => false,
                },
                note.clone(),
            ),
            "deadline" => (
                draft.deadline.as_deref() == want.as_str(),
                format!("{:?}", draft.deadline),
            ),
            "others_min" => (
                others as u64 >= want.as_u64().unwrap_or(0),
                others.to_string(),
            ),
            "dropped_max" => (
                dropped as u64 <= want.as_u64().unwrap_or(0),
                dropped.to_string(),
            ),
            other => (false, format!("未知检查项 {other}")),
        };
        outcome.checks.push((format!("{key} = {want}"), ok, got));
    }
    outcome
}

fn str_list(v: &Value) -> Vec<&str> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default()
}

fn run_batch(case: &BatchCase, today: &str) -> Outcome {
    let tasks = case
        .tasks
        .iter()
        .map(|row| {
            let mut t = Task::blank();
            t.id = row.id.clone();
            t.name = row.name.clone();
            t.next_step = row.next_step.clone();
            t
        })
        .collect::<Vec<_>>();
    let mut outcome = Outcome {
        id: case.id.clone(),
        note: case.note.clone(),
        seconds: 0.0,
        error: None,
        checks: Vec::new(),
        raw: String::new(),
    };
    let (system, user) = prompt(&case.title, &case.text, &tasks, today);
    let start = Instant::now();
    let raw = crate::lmstudio::generate(&config(8192), &system, &user);
    outcome.seconds = start.elapsed().as_secs_f64();
    let raw = match raw {
        Ok(raw) => raw,
        Err(e) => {
            outcome.error = Some(format!("模型调用失败：{e:#}"));
            return outcome;
        }
    };
    outcome.raw = raw.clone();
    // 与产品里 `start_task_material` 一样：解析后再按原文核对期限。
    let items = match parse_response(&raw, &case.text, &tasks) {
        Ok(items) => crate::task_assistant::extract::check_proposal_dates(items, &case.text, today),
        Err(e) => {
            outcome.error = Some(format!("协议解析失败：{e:#}"));
            return outcome;
        }
    };
    let kinds = items.iter().map(|p| p.kind.as_str()).collect::<Vec<_>>();
    let ids = items
        .iter()
        .filter_map(|p| p.task_id.as_deref())
        .collect::<Vec<_>>();
    let summary = format!("kinds={kinds:?} task_ids={ids:?}");
    for (key, want) in &case.expect {
        let ok = match key.as_str() {
            "kinds_any" => str_list(want).iter().any(|k| kinds.contains(k)),
            "kinds_only" => kinds.iter().all(|k| str_list(want).contains(k)),
            "task_ids_any" => str_list(want).iter().any(|id| ids.contains(id)),
            "new_task_without_id" => items
                .iter()
                .any(|p| p.kind == "new_task" && p.task_id.is_none()),
            "empty" => items.is_empty() == want.as_bool().unwrap_or(false),
            "new_task_deadline" => items
                .iter()
                .any(|p| p.kind == "new_task" && p.deadline.as_deref() == want.as_str()),
            _ => false,
        };
        outcome
            .checks
            .push((format!("{key} = {want}"), ok, summary.clone()));
    }
    outcome
}

fn report(model: &str, fill: &[Outcome], batch: &[Outcome]) -> String {
    let mut md = String::new();
    let passed = |v: &[Outcome]| v.iter().filter(|o| o.passed()).count();
    let protocol = |v: &[Outcome]| v.iter().filter(|o| o.error.is_none()).count();
    let checks = |v: &[Outcome]| {
        let all = v.iter().flat_map(|o| &o.checks).count();
        let ok = v.iter().flat_map(|o| &o.checks).filter(|c| c.1).count();
        (ok, all)
    };
    let seconds = |v: &[Outcome]| v.iter().map(|o| o.seconds).sum::<f64>() / v.len().max(1) as f64;
    let _ = writeln!(md, "# 任务助手模型评测\n\n模型：`{model}`\n");
    for (title, set) in [("单任务填写", fill), ("批量材料整理", batch)] {
        if set.is_empty() {
            continue;
        }
        let (ok, all) = checks(set);
        let _ = writeln!(
            md,
            "## {title}\n\n用例全过 {}/{}；协议可解析 {}/{}；检查项 {ok}/{all}；平均耗时 {:.1} 秒\n",
            passed(set),
            set.len(),
            protocol(set),
            set.len(),
            seconds(set)
        );
        let _ = writeln!(
            md,
            "| 用例 | 结果 | 耗时 | 未通过的检查 |\n|---|---|---|---|"
        );
        for o in set {
            let failed = match &o.error {
                Some(e) => e.replace('|', "\\|").replace('\n', " "),
                None => o
                    .checks
                    .iter()
                    .filter(|c| !c.1)
                    .map(|c| format!("{}（实际：{}）", c.0, c.2))
                    .collect::<Vec<_>>()
                    .join("；")
                    .replace('|', "\\|")
                    .replace('\n', " "),
            };
            let _ = writeln!(
                md,
                "| {} | {} | {:.1}s | {} |",
                o.id,
                if o.passed() { "通过" } else { "未通过" },
                o.seconds,
                failed
            );
        }
        let _ = writeln!(md);
    }
    let _ = writeln!(md, "## 原始输出\n");
    for o in fill.iter().chain(batch) {
        let _ = writeln!(
            md,
            "### {} — {}\n\n```json\n{}\n```\n",
            o.id,
            o.note,
            o.raw.trim()
        );
    }
    md
}

#[test]
#[ignore = "连真实模型评测任务助手，需本机 LM Studio 或 GONGWEN_TASK_EVAL_URL"]
fn task_assistant_model_eval() {
    let cases: Cases =
        serde_json::from_str(include_str!("../../tests/fixtures/task-eval/cases.json"))
            .expect("评测用例格式不对");
    let only = std::env::var("GONGWEN_TASK_EVAL_ONLY").unwrap_or_default();
    let vocab = vocabulary(&cases.vocabulary);
    let mut fill = Vec::new();
    for case in cases.fill.iter().filter(|c| c.id.starts_with(&only)) {
        let o = run_fill(case, &cases.today, &vocab);
        println!(
            "{} {} {:.1}s",
            o.id,
            if o.passed() { "通过" } else { "未通过" },
            o.seconds
        );
        fill.push(o);
    }
    let mut batch = Vec::new();
    for case in cases.batch.iter().filter(|c| c.id.starts_with(&only)) {
        let o = run_batch(case, &cases.today);
        println!(
            "{} {} {:.1}s",
            o.id,
            if o.passed() { "通过" } else { "未通过" },
            o.seconds
        );
        batch.push(o);
    }
    let model = config(0).model;
    let md = report(&model, &fill, &batch);
    let dir = std::path::Path::new("tmp/task-eval");
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{}.md", model.replace(['/', ':'], "_")));
    std::fs::write(&path, &md).unwrap();
    println!("报告：{}", path.display());
}
