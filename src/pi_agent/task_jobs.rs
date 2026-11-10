//! 任务助手的 Pi 作业：材料整理与单任务填写。
//!
//! 与风格学习同一套链：界面线程只收集材料与表单，后台线程组装工作区、启动 host，
//! host 用技能文件里的自然语言步骤工作，最后用 `submit_task` 交一份 JSON；Rust 在
//! 这里按与单次调用完全相同的规则复核（证据必须在原文里、期限要有原文支撑、候选
//! 类型与目标 ID 受限），通过后也只交给任务页由用户逐条采纳。
//!
//! 两件事与写作作业不同：
//! - 不占写作门禁：Pi 写作与任务作业可以同时进行（本地模型排队由服务端负责）。
//! - 工具面收窄：只开放 `task_search` / `task_read` / `vocab_search` / `current_date`，
//!   任务材料已经进工作区，不需要读知识库、稿件库或内网接口。
//!
//! 材料按 Pi 的单文件上限（512 KiB）拆分，最多 20 份、合计 4 MiB；整份原文仍留在
//! Rust 侧做指纹与证据核对，程序不截断原文。

use super::{
    MaterialRole, PiBridge, PiEvent, PiJob, PiMaterialInput, PiModel, PiOutcome, PiRunLog,
    PiRunRecord, PiRuntime, PiTask, PiToolContext, PiWorkspace, pi_skills,
};
use crate::models::{AppConfig, DraftInput};
use crate::task_assistant::Store;
use crate::task_assistant::extract::{self, Fill, Proposal, TaskDelivery};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// 单份材料文件的上限：Pi 的材料快照限制（`materials::MAX_MATERIAL_BYTES`）。
const MATERIAL_CHUNK_BYTES: usize = 512 * 1024;
/// 一轮最多几份材料文件：Pi 的 `materials::MAX_MATERIALS`。
const MAX_MATERIAL_FILES: usize = 20;
/// 任务作业的运行目录分组：0 留给风格学习，1/2 分别给整理与填写，各自独立清理。
const EXTRACT_DOC_KEY: u64 = 1;
const FILL_DOC_KEY: u64 = 2;
/// 与写作一致的单轮执行上限。
const TIMEOUT: Duration = Duration::from_secs(1800);

/// 材料整理的结果：候选仍要用户逐条采纳，这里只交回已通过确定性校验的条目。
pub(crate) struct TaskExtraction {
    pub(crate) proposals: Vec<Proposal>,
}

/// 单任务填写的结果：值、摘录与丢弃计数，由界面合并进草稿。
pub(crate) struct TaskFill {
    pub(crate) fill: Fill,
}

/// 把整份材料按 Pi 的上限拆成若干份文件；切点落在 UTF-8 字符边界与段落/换行附近。
fn material_files(title: &str, source: &str) -> Result<Vec<PiMaterialInput>, String> {
    if source.trim().is_empty() {
        return Err("材料是空的".into());
    }
    if source.len() > extract::MAX_SOURCE_BYTES {
        return Err(format!(
            "材料超过 {} MiB，请拆分后再整理；程序不会截断原文",
            extract::MAX_SOURCE_BYTES / (1024 * 1024)
        ));
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < source.len() {
        if ranges.len() >= MAX_MATERIAL_FILES {
            return Err(format!(
                "材料超过 {} 份（每份 {:.0} KiB），请拆分后再整理",
                MAX_MATERIAL_FILES,
                MATERIAL_CHUNK_BYTES as f64 / 1024.0
            ));
        }
        let mut end = (start + MATERIAL_CHUNK_BYTES).min(source.len());
        if end < source.len() {
            // 后退到字符边界，再优先在段落、其次在换行处切开，少切碎一句话。
            while end > start && !source.is_char_boundary(end) {
                end -= 1;
            }
            let mut floor = start + MATERIAL_CHUNK_BYTES / 2;
            while floor < end && !source.is_char_boundary(floor) {
                floor += 1;
            }
            let window = &source[..end];
            let paragraph = window[floor..].rfind("\n\n").map(|index| floor + index + 2);
            let line = window[floor..].rfind('\n').map(|index| floor + index + 1);
            if let Some(cut) = paragraph.or(line) {
                end = cut;
            }
        }
        if end <= start {
            return Err("材料里有一段超过单份上限且切不开，请拆分后再整理".into());
        }
        ranges.push((start, end));
        start = end;
    }
    let total = ranges.len();
    Ok(ranges
        .into_iter()
        .enumerate()
        .map(|(index, (start, end))| PiMaterialInput {
            title: if total == 1 {
                title.to_string()
            } else {
                format!("{title}（第 {}/{} 部分）", index + 1, total)
            },
            source: "任务材料".into(),
            role: MaterialRole::Fact,
            text: source[start..end].to_string(),
        })
        .collect())
}

/// 交付校验与提示词共用的日期基准：必须是 `YYYY-MM-DD`。
///
/// `TimeContext::today` 是中文本地化格式（`2026年11月10日`），`date_supported`
/// 只认 ISO；这里统一用 ISO，避免期限被静默丢弃（真实模型验收发现过这个回归）。
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 作业的固定标识：技能名、运行目录分组与运行记录里的任务种类。
struct JobSpec<'a> {
    skill: &'a str,
    doc_key: u64,
    kind: &'a str,
}

/// 组装一次任务作业：工作区、工具面、运行记录与模型参数。
struct Prepared {
    runtime: PiRuntime,
    workspace: PiWorkspace,
    model: PiModel,
    tools: Arc<PiToolContext>,
    record: Arc<PiRunLog>,
    request_id: String,
    run_id: String,
    task: PiTask,
}

fn prepare(
    config: &Arc<AppConfig>,
    store_path: &std::path::Path,
    spec: JobSpec<'_>,
    request: String,
    materials: Vec<PiMaterialInput>,
    catalog: &pi_skills::Catalog,
) -> Result<Prepared, String> {
    let JobSpec {
        skill,
        doc_key,
        kind,
    } = spec;
    let runtime = PiRuntime::locate()?;
    let chat = config.pi_chat().map_err(|error| error.to_string())?;
    let model = crate::ai_panel::pi_job::pi_model(&chat)?;
    let today = today();
    let (workspace, metas) = PiWorkspace::prepare(
        &super::base_dir()?,
        doc_key,
        "",
        &request,
        &materials,
        catalog,
        Some(skill),
    )?;
    let tools = Arc::new(
        PiToolContext::new(
            Arc::clone(config),
            DraftInput::default(),
            String::new(),
            request.clone(),
            Vec::new(),
        )
        .task_only(store_path.to_path_buf()),
    );
    let run_id = uuid::Uuid::new_v4().to_string();
    let request_id = format!("{kind}-{}", uuid::Uuid::new_v4());
    let record = PiRunRecord::start(
        run_id.clone(),
        request_id.clone(),
        &workspace,
        "",
        "",
        "",
        &workspace.read_request()?,
        None,
        &request,
        kind,
        &today,
        Some(skill),
        None,
        metas.clone(),
        &config.pi_model.provider_id,
        &model.model_id,
    );
    let record = Arc::new(PiRunLog::open(&workspace, record)?);
    Ok(Prepared {
        runtime,
        workspace,
        model,
        tools,
        record,
        request_id,
        run_id,
        task: PiTask {
            request,
            kind: kind.to_string(),
            system_date: today,
            skill: Some(skill.to_string()),
            strategy: Default::default(),
            capability: Default::default(),
            selection: None,
            materials: metas,
        },
    })
}

fn run_job(
    prepared: Prepared,
    config: &Arc<AppConfig>,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(String),
) -> Result<PiOutcome, String> {
    let label = prepared.workspace.label.clone();
    super::run(
        PiJob {
            runtime: prepared.runtime,
            workspace: prepared.workspace,
            model: prepared.model,
            task: prepared.task,
            before: String::new(),
            base: String::new(),
            tools: prepared.tools,
            bridge: Arc::new(PiBridge::default()),
            record: prepared.record,
            resume: false,
            request_id: prepared.request_id,
            run_id: prepared.run_id,
            timeout: TIMEOUT,
            proxy_env: crate::net::proxy_env(&config.proxy),
        },
        Arc::clone(cancel),
        |event| match event {
            PiEvent::Progress(message) | PiEvent::Tool(message) => on_progress(message),
            PiEvent::Activity(activity) if activity.outcome.is_none() => {
                on_progress(activity.action)
            }
            PiEvent::Strategy(info) => on_progress(info.line()),
            _ => {}
        },
    )
    .map_err(|error| format!("{error}；工作文件保留在 {label}"))
}

/// 整理一批材料：候选交给任务页，由用户逐条采纳；这里不写数据库。
pub(crate) fn extract(
    config: Arc<AppConfig>,
    store_path: PathBuf,
    title: String,
    source: String,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(String),
) -> Result<TaskExtraction, String> {
    if title.trim().is_empty() || title.chars().count() > 200 {
        return Err("材料标题不能为空且最多 200 字".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止整理材料。".into());
    }
    let catalog = pi_skills::load_catalog();
    catalog.resolve(Some(pi_skills::TASK_EXTRACT_SKILL))?;
    let known = Store::open(&store_path)
        .map_err(|error| format!("打开任务库失败：{error:#}"))?
        .list(true)
        .map_err(|error| format!("读取现有任务失败：{error:#}"))?;
    let materials = material_files(&title, &source)?;
    let request = format!(
        "按技能 {} 整理这批材料里的工作事项，最后用 submit_task 交 work/task.json。\
         材料共 {} 份，事实只来自这些材料；用 task_search / task_read 核对已有任务，\
         不要凭记忆写 task_id，也不要编造材料里没有的人员、日期、数字与结论。\
         今天日期：{today}。材料标题：{title}。",
        pi_skills::TASK_EXTRACT_SKILL,
        materials.len(),
        today = today(),
        title = title,
    );
    let prepared = prepare(
        &config,
        &store_path,
        JobSpec {
            skill: pi_skills::TASK_EXTRACT_SKILL,
            doc_key: EXTRACT_DOC_KEY,
            kind: "任务整理",
        },
        request,
        materials,
        &catalog,
    )?;
    let outcome = run_job(prepared, &config, &cancel, on_progress)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止整理材料，未采纳本轮结果。".into());
    }
    let PiOutcome::Task(delivery) = outcome else {
        return Err("Pi 没有交回任务整理结果，未采纳。".into());
    };
    let today = today();
    match extract::parse_task_delivery(&delivery.content, &source, &today, &known)
        .map_err(|error| format!("{error:#}"))?
    {
        TaskDelivery::Extract(items) => Ok(TaskExtraction {
            proposals: extract::check_proposal_dates(items, &source, &today),
        }),
        TaskDelivery::Fill(_) => Err("Pi 交回的是表单填写结果，不是材料整理结果".into()),
    }
}

/// 从文字填写一张任务表单：只填六栏，值带原文摘录，由界面合并进草稿。
pub(crate) fn fill(
    config: Arc<AppConfig>,
    store_path: PathBuf,
    source: String,
    current: crate::task_assistant::Task,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(String),
) -> Result<TaskFill, String> {
    if source.trim().is_empty() {
        return Err("请粘贴非空文字再填写".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止填写。".into());
    }
    let catalog = pi_skills::load_catalog();
    catalog.resolve(Some(pi_skills::TASK_FILL_SKILL))?;
    let values = serde_json::json!({
        "name": current.name,
        "owner": current.owner_name,
        "progress": current.progress,
        "next_step": current.next_step,
        "risks": current.risks,
        "deadline": current.deadline,
    });
    let request = format!(
        "按技能 {} 从材料里填写这张任务表单，最后用 submit_task 交 work/task.json。\
         只填六栏，原文没有的留 null；已有值只在与原文冲突时给建议。\
         今天日期：{today}。当前表单值：{values}",
        pi_skills::TASK_FILL_SKILL,
        today = today(),
    );
    let prepared = prepare(
        &config,
        &store_path,
        JobSpec {
            skill: pi_skills::TASK_FILL_SKILL,
            doc_key: FILL_DOC_KEY,
            kind: "任务填写",
        },
        request,
        material_files("填写材料", &source)?,
        &catalog,
    )?;
    let outcome = run_job(prepared, &config, &cancel, on_progress)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止填写，未采纳本轮结果。".into());
    }
    let PiOutcome::Task(delivery) = outcome else {
        return Err("Pi 没有交回表单填写结果，未采纳。".into());
    };
    let today = today();
    match extract::parse_task_delivery(&delivery.content, &source, &today, &[])
        .map_err(|error| format!("{error:#}"))?
    {
        TaskDelivery::Fill(fill) => Ok(TaskFill { fill: *fill }),
        TaskDelivery::Extract(_) => Err("Pi 交回的是材料整理结果，不是表单填写结果".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_chunks_respect_pi_limits_and_char_boundaries() {
        let text = "字".repeat(MATERIAL_CHUNK_BYTES); // 每字 3 字节，超过单份上限
        let files = material_files("材料", &text).unwrap();
        assert!(files.len() >= 2);
        assert!(
            files
                .iter()
                .all(|file| file.text.len() <= MATERIAL_CHUNK_BYTES)
        );
        assert_eq!(
            files.iter().map(|file| file.text.len()).sum::<usize>(),
            text.len()
        );
        assert!(files[0].title.contains("第 1/"));
    }

    #[test]
    fn oversized_material_is_rejected_not_truncated() {
        let text = "x".repeat(extract::MAX_SOURCE_BYTES + 1);
        assert!(material_files("材料", &text).is_err());
    }

    #[test]
    fn today_is_iso_for_deadline_validation() {
        let value = today();
        assert_eq!(value.len(), 10, "日期基准必须是 YYYY-MM-DD：{value}");
        assert!(chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d").is_ok());
    }

    #[test]
    fn too_many_chunks_are_rejected() {
        // 每块约 256 KiB（汉字 3 字节 → 约 87k 字），21 块超过 Pi 的 20 份上限。
        let text = "字".repeat((MATERIAL_CHUNK_BYTES / 3 / 2) * 21);
        assert!(material_files("材料", &text).is_err());
    }
}
