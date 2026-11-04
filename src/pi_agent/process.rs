//! 后台进程：启动 Node host、双向 JSONL、工具桥、提问、取消与回收。
//!
//! UI 线程只收事件；这里从不阻塞界面。stdin 只写协议，密钥不出现在 argv，也不
//! 出现在错误文本里。stdout 的 EOF、进程退出、协议错、取消和超时都按失败返回，
//! 不会把最后一段聊天或半截文件当成交付。
//!
//! 领域工具、提问与审核清单都走同一条 `callId` 关联的请求 / 结果通道；Rust 在
//! 后台线程执行工具，协议循环继续收事件、检查取消与超时，检索慢不会把取消卡住。
//! 停下来的任务再收到的工具结果一律丢弃；工具上下文同时拿到本轮的停止信号，慢工具在
//! 阶段边界尽快收手（HTTP 请求本身无法中途 abort，靠工具层在途槽位限制资源叠加）。

use super::PiRuntime;
use super::PiWorkspace;
use super::delivery::{PiSelection, safe_work_path, sha256_hex, verify_submission};
use super::materials::PiMaterialMeta;
use super::protocol::{
    HOST_VERSION, HostEvent, InitCapability, InitLimits, InitMessage, InitModel, InitRecovery,
    InitSelection, InitStrategy, InitTask, InitWorkspace, MAX_CANDIDATE_BYTES, MAX_DELIVERY_BYTES,
    MAX_EVENTS, MAX_HOST_LINE_BYTES, MAX_MESSAGE_CHARS, PROTOCOL_VERSION, PiReportItem,
    RawQuestion,
};
use super::recovery::{PiReceipt, PiRunLog, PiRunStatus, read_receipt, read_snapshot};
use super::runtime::hidden_command;
use super::tools::PiToolContext;
use crate::agent::clarify::{Question, Reply};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 一条工具请求参数的上限。
const MAX_TOOL_ARGS_BYTES: usize = 1024 * 1024;
/// 分阶段写作时 host 会按阶段追加额度，总上限 84（常规 60 + 24）；Rust 侧按同一个
/// 硬上限判定，避免宿主放开额度后仍被这里杀掉。
const MAX_TOTAL_TOOL_CALLS: usize = 84;
/// 一批最多几题（与 `clarify::agent_questions` 一致）。
const MAX_QUESTIONS_PER_BATCH: u32 = 3;
/// 一轮最多几批提问。
const MAX_QUESTION_BATCHES: u32 = 2;
/// 心跳：宿主 30 秒没有任何消息就 ping，再 30 秒没有 pong 判失联。
const HEARTBEAT_QUIET: Duration = Duration::from_secs(30);
/// 宿主心跳响应上限。
const HEARTBEAT_DEAD: Duration = Duration::from_secs(30);

/// 全局只允许一轮 Pi 任务（正文写作 / 风格学习 / 能力测试）同时进行：每篇稿件各自
/// 起 Node 进程会抢同一个本地模型，最终一起超时。
static WRITING_RUNS: AtomicUsize = AtomicUsize::new(0);

/// 一轮 Pi 任务的占位令牌，线程结束自动释放。
pub(crate) struct PiWritingGuard {
    /// 测试里不做全局互斥（并行 UI 用例会互相抢），因此也就不减计数。
    counted: bool,
}

impl Drop for PiWritingGuard {
    fn drop(&mut self) {
        if self.counted {
            WRITING_RUNS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// 开始一轮 Pi 任务（正文写作 / 风格学习 / 能力测试）。已有任务在跑时返回可读错误。
///
/// 单元测试并行跑，多个用例会各自“启动 Pi”，全局门禁在测试里放行、只验证调用方
/// 持有令牌的代码路径；生产构建里只有第一个能拿到。
pub(crate) fn begin_writing() -> Result<PiWritingGuard, String> {
    #[cfg(test)]
    {
        Ok(PiWritingGuard { counted: false })
    }
    #[cfg(not(test))]
    {
        if WRITING_RUNS.fetch_add(1, Ordering::AcqRel) > 0 {
            WRITING_RUNS.fetch_sub(1, Ordering::AcqRel);
            return Err(
                "已有 Pi 任务（写作 / 风格学习 / 能力测试）正在运行，请等它结束后再开始。".into(),
            );
        }
        Ok(PiWritingGuard { counted: true })
    }
}

/// 当前是否有 Pi 任务在跑（能力校准、正文写作与风格学习共用同一门禁）。
pub(crate) fn writing_active() -> bool {
    #[cfg(test)]
    {
        false
    }
    #[cfg(not(test))]
    {
        WRITING_RUNS.load(Ordering::Acquire) > 0
    }
}

/// 交给 host 的模型参数。`api_key` 只进 stdin，不进 argv / 日志。
#[derive(Clone)]
pub(crate) struct PiModel {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model_id: String,
    pub(crate) context_window: u32,
    pub(crate) window_source: super::capability::CapabilityWindowSource,
    pub(crate) max_tokens: u32,
    pub(crate) reasoning: bool,
    pub(crate) thinking_level: Option<String>,
}

pub(crate) struct PiTask {
    pub(crate) request: String,
    pub(crate) kind: String,
    pub(crate) system_date: String,
    /// 用户选择的 Pi 技能（内置八份或用户副本之一）；None 表示自动。
    pub(crate) skill: Option<String>,
    /// 统一写作入口的策略倾向；实际策略由 host 按窗口与任务规模决定。
    pub(crate) strategy: super::strategy::PiStrategyPreference,
    pub(crate) capability: super::strategy::PiCapabilitySnapshot,
    pub(crate) selection: Option<PiSelection>,
    pub(crate) materials: Vec<PiMaterialMeta>,
}

pub(crate) struct PiJob {
    pub(crate) runtime: PiRuntime,
    pub(crate) workspace: PiWorkspace,
    pub(crate) model: PiModel,
    pub(crate) task: PiTask,
    /// 发起本轮时的正式正文快照；接受提案时还要用它保护过期提案。
    pub(crate) before: String,
    /// 本轮写作基准：可能是未采纳的候选稿（排除块之后）。
    pub(crate) base: String,
    pub(crate) tools: Arc<PiToolContext>,
    pub(crate) bridge: Arc<PiBridge>,
    /// 运行记录：开跑前已写一份，host 的报告与交付状态都回写在这里。
    pub(crate) record: Arc<PiRunLog>,
    /// 恢复模式：沿原工作区与 Durable 库，用同一份 requestId 找回原来的提交。
    pub(crate) resume: bool,
    pub(crate) request_id: String,
    pub(crate) run_id: String,
    pub(crate) timeout: Duration,
    /// 从应用代理设置派生的环境变量（`HTTP_PROXY` 等）；host 在最小环境里运行。
    pub(crate) proxy_env: Vec<(String, String)>,
}

#[derive(Debug)]
pub(crate) enum PiEvent {
    Progress(String),
    Tool(String),
    /// host 算出的策略决定：模式、原因、实际窗口与工作预算。
    Strategy(PiStrategyInfo),
    Candidate {
        path: String,
    },
    /// 模型在等用户回答。答案由界面通过 [`PiBridge::answer`] 送回。
    Questions(PiQuestionBatch),
    /// 工具返回的证据已登记（侧栏「过程」里一行）。
    Evidence(String),
}

/// host 算出的策略决定。原因只给人看；界面不解析字符串判断类别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiStrategyInfo {
    pub(crate) mode: String,
    pub(crate) reasons: Vec<String>,
    pub(crate) window: u64,
    pub(crate) work_budget: u64,
    pub(crate) materials_budget: u64,
    pub(crate) verified: bool,
}

impl PiStrategyInfo {
    /// 「分阶段 · 262k 窗口 / 96k 工作预算 · 材料超过本次可用预算…」
    pub(crate) fn line(&self) -> String {
        let mode = if self.mode == "phased" {
            "分阶段"
        } else {
            "常规"
        };
        let reasons = if self.reasons.is_empty() {
            String::new()
        } else {
            format!(" · {}", self.reasons.join("；"))
        };
        let verified = if self.verified {
            "已验证"
        } else {
            "未验证"
        };
        format!(
            "策略：{mode} · {} 窗口 / {} 工作预算（{}）· 材料预算 {}{reasons}",
            crate::lmstudio::context::tokens_label(self.window as usize),
            crate::lmstudio::context::tokens_label(self.work_budget as usize),
            verified,
            crate::lmstudio::context::tokens_label(self.materials_budget as usize),
        )
    }
}

/// 一道已经过 `clarify::agent_questions` 校验的题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiQuestion {
    pub(crate) id: usize,
    pub(crate) text: String,
    pub(crate) options: Vec<String>,
    pub(crate) custom: bool,
    pub(crate) skippable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiQuestionBatch {
    pub(crate) call_id: String,
    pub(crate) batch: u32,
    pub(crate) questions: Vec<PiQuestion>,
}

/// 用户对一道题的回答。空 `value` 表示跳过；`question` 只用于会话里的已确认记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PiAnswer {
    pub(crate) id: usize,
    pub(crate) question: String,
    pub(crate) value: String,
}

/// 界面 ↔ 后台进程的受控通道：只允许送回答与读已确认答案，不暴露 stdin 原文。
#[derive(Default)]
pub(crate) struct PiBridge {
    closed: AtomicBool,
    tx: Mutex<Option<mpsc::SyncSender<String>>>,
    answered: AtomicU64,
    confirmed: Mutex<Vec<String>>,
    confirmed_values: Mutex<Vec<String>>,
    /// 回答先落到运行记录与事实来源，再交给 host；没有这个挂接时只留内存。
    persist: Mutex<Option<(Arc<PiToolContext>, Arc<PiRunLog>)>>,
}

impl std::fmt::Debug for PiBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PiBridge")
            .field("answered", &self.answered())
            .finish_non_exhaustive()
    }
}

impl PiBridge {
    fn install(&self, tx: mpsc::SyncSender<String>) {
        *self.tx.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(tx);
    }

    /// 后台进程挂上工具台账与运行记录：回答落盘早于 host 看到回答。
    fn attach(&self, tools: Arc<PiToolContext>, record: Arc<PiRunLog>) {
        *self
            .persist
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some((tools, record));
    }

    /// 把用户回答登记为事实并立即落盘；失败就不把回答交给模型。
    fn persist_answers(&self, answers: &[PiAnswer]) -> Result<(), String> {
        let values: Vec<String> = answers
            .iter()
            .filter(|answer| !answer.value.trim().is_empty())
            .map(|answer| answer.value.chars().take(2_000).collect())
            .collect();
        let display: Vec<String> = answers
            .iter()
            .filter(|answer| !answer.value.trim().is_empty())
            .map(|answer| format!("{}：{}", answer.question.trim(), answer.value.trim()))
            .collect();
        let guard = self
            .persist
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self.closed.load(Ordering::Acquire) {
            return Err("Pi 进程已经结束，回答没有送到。".into());
        }
        if let Some((tools, record)) = guard.as_ref() {
            let mut sources = tools.fact_sources();
            sources.extend(values.iter().cloned());
            record
                .confirmed_sources(&display, &values, &sources)
                .map_err(|error| format!("记录已确认回答失败，未把回答交给模型：{error}"))?;
            tools.confirm_answers(&values);
        }
        Ok(())
    }

    fn answered(&self) -> u64 {
        self.answered.load(Ordering::Relaxed)
    }

    fn confirmed(&self) -> Vec<String> {
        self.confirmed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// 回答一批题。题目 ID 绑定本轮问题，旧轮 / 重复的回答会被 host 忽略；
    /// 这里也拒绝空 ID 列表，避免看似“答了”其实没答。
    pub(crate) fn answer(&self, call_id: &str, answers: &[PiAnswer]) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("Pi 进程已经结束，回答没有送到。".into());
        }
        if call_id.trim().is_empty() || answers.is_empty() {
            return Err("没有可提交的回答".into());
        }
        self.persist_answers(answers)?;
        let mut payload = serde_json::Map::new();
        payload.insert("type".into(), "tool_result".into());
        payload.insert("callId".into(), call_id.into());
        payload.insert("ok".into(), true.into());
        payload.insert(
            "result".into(),
            serde_json::json!({
                "answers": answers
                    .iter()
                    .map(|answer| serde_json::json!({
                        "id": answer.id,
                        "value": answer.value.chars().take(2_000).collect::<String>(),
                    }))
                    .collect::<Vec<_>>()
            }),
        );
        let mut line =
            serde_json::to_string(&payload).map_err(|error| format!("组装回答失败：{error}"))?;
        line.push('\n');
        self.send(line)?;
        for answer in answers {
            if !answer.value.trim().is_empty() {
                self.confirmed_values
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(answer.value.chars().take(2_000).collect());
                self.confirmed
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(format!(
                        "{}：{}",
                        answer.question.trim(),
                        answer.value.trim()
                    ));
            }
        }
        self.answered.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn send(&self, line: String) -> Result<(), String> {
        let guard = self.tx.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(tx) = guard.as_ref() else {
            return Err("Pi 进程已经结束，回答没有送到。".into());
        };
        // 回答来自界面线程：队列满时不能在界面线程上阻塞等 host 读 stdin。
        // 交给后台线程继续投递，保证“答了就是答了”，界面先返回。
        match tx.try_send(line) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(line)) => {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(line);
                });
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => {
                Err("Pi 进程已经结束，回答没有送到。".to_string())
            }
        }
    }
}

/// 结束后关闭回答能力，并等正在落盘的回答完成；随后才释放运行期所有权锁。
struct BridgeLifetime<'a>(&'a PiBridge);
impl Drop for BridgeLifetime<'_> {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0
            .persist
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        self.0
            .tx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
    }
}

/// 成功交付的固定版本。
#[derive(Debug)]
pub(crate) struct PiDelivery {
    pub(crate) before: String,
    pub(crate) base: String,
    pub(crate) request: String,
    pub(crate) content: String,
    pub(crate) path: String,
    pub(crate) summary: String,
    pub(crate) sha256: String,
    pub(crate) bytes: usize,
    pub(crate) selection: Option<PiSelection>,
    /// 用户材料与工具实际返回的资料全文；交付闸门只认这些，不认模型自报。
    pub(crate) fact_sources: Vec<String>,
    /// 已确认的回答（追问时随上下文保存）。
    pub(crate) confirmed: Vec<String>,
    pub(crate) workspace_label: String,
    pub(crate) elapsed: Duration,
    /// 这次结果是从上次运行的回执恢复出来的，没有重新生成正文。
    pub(crate) recovered: bool,
    /// 本轮事实来源文件的哈希（`pi/sources.md`）；重载会话后继续改提案时用。
    pub(crate) sources_sha256: Option<String>,
}

/// 审核 / 找资料交的清单：只进侧栏，不产生正文提案。
#[derive(Debug)]
pub(crate) struct PiReport {
    pub(crate) summary: String,
    pub(crate) items: Vec<PiReportItem>,
    pub(crate) confirmed: Vec<String>,
    pub(crate) workspace_label: String,
    pub(crate) elapsed: Duration,
    pub(crate) recovered: bool,
    pub(crate) sources_sha256: Option<String>,
}

#[derive(Debug)]
pub(crate) enum PiOutcome {
    Document(PiDelivery),
    /// 独立风格文件，只能由风格设置页接收；不能写正文。
    Style(PiDelivery),
    Report(PiReport),
}

struct Submission {
    /// host 落盘的交付回执；没有回执就不算交付。
    receipt: PiReceipt,
    /// 回执是重放上次已落盘成品得到的。
    recovered: bool,
}

fn builtin_delivery_kind(skill: Option<&str>) -> Option<&'static str> {
    match skill? {
        "gongwen-draft" | "gongwen-revise" | "gongwen-imitate" | "gongwen-research"
        | "gongwen-reply" => Some("document"),
        "gongwen-extract" => Some("report"),
        super::pi_skills::STYLE_SKILL => Some("style"),
        _ => None,
    }
}

/// 子进程守卫：任何返回路径都 kill + wait，避免孤儿进程与卡住的管道。
struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn take(&mut self) -> Option<Child> {
        self.0.take()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

enum StdoutMsg {
    Line(Vec<u8>),
    Protocol(String),
    Eof,
}

struct ToolDone {
    call_id: String,
    evidence_before: usize,
    result: Result<serde_json::Value, String>,
}

/// 跑一轮 host，直到提交成功、交回报告或失败。
fn lock_run(run: &std::path::Path) -> Result<std::fs::File, String> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(run.join("rust-owner.lock"))
        .map_err(|error| format!("打开 Pi 运行所有权锁失败：{error}"))?;
    lock.try_lock()
        .map_err(|error| format!("这一轮 Pi 正由另一个窗口处理，请稍后再试：{error}"))?;
    Ok(lock)
}

pub(crate) fn run(
    job: PiJob,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(PiEvent),
) -> Result<PiOutcome, String> {
    if job.task.skill.as_deref() == Some(super::pi_skills::STYLE_SKILL)
        && job.task.kind != "风格档案"
    {
        return Err("风格学习只能提交独立风格档案，不能作为正文任务运行。".into());
    }
    // 覆盖 Rust 侧的准备完成后更新、host 退出后落盘窗口；Node 自己另持 SQLite 锁，
    // 即使 Rust 崩溃而 host 尚存，也不能同时打开 Durable 库。
    let _run_owner = lock_run(&job.workspace.run)?;
    // 只把启动前已有、身份匹配的回执作为旧类型回放凭据。仍由 host 在 SQLite 锁内
    // 实际重放；这里的只读快照仅用于拒绝 fresh/resume 过程中伪造 recovered 标记。
    let preexisting_receipt = if job.resume {
        read_receipt(&job.workspace.run, &job.run_id)?
            .filter(|receipt| receipt.request_id == job.request_id)
    } else {
        None
    };
    let _bridge_lifetime = BridgeLifetime(&job.bridge);
    let started = Instant::now();
    // 工具在独立线程执行，慢工具（知识库检索 / LLM 重排、内网 HTTP、稿件库读取、文档检查）
    // 无法从外部强杀；把界面停止信号交给工具上下文，让它们在阶段边界收手、迟到结果不写
    // 证据也不当成功返回。反复停止重开的资源叠加由工具层的在途槽位兜底。
    job.tools.attach_cancel(Arc::clone(&cancel));
    if cancel.load(Ordering::Relaxed) {
        if !job.resume {
            let _ = job
                .record
                .finished(PiRunStatus::Interrupted, "用户在开跑前就停止了这一轮。");
        }
        return Err("已停止 Pi 智能写作，未交付任何提案。".into());
    }
    if job.model.model_id.trim().is_empty() {
        return Err("还没有选择智能写作模型：请到「AI 管理 → 模型服务」选择。".into());
    }
    if job.task.request.len() > 200_000 {
        return Err("本轮要求太长（超过 200000 字节），请拆成几轮再写。".into());
    }
    if let Some(selection) = job.task.selection
        && let Err(error) = selection.validate(&job.before)
    {
        return Err(error);
    }
    // 恢复不在这里直读回执：必须让 host 在 SQLite 所有权锁内重放，否则另一个仍活着的
    // host 可能同时改同一轮的 sources / 运行记录。
    let mut command = hidden_command(&job.runtime.node);
    command
        .arg("--disable-warning=ExperimentalWarning")
        .arg(&job.runtime.host)
        .current_dir(&job.workspace.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // 生产同样用最小环境：不继承用户的 NODE_OPTIONS / NODE_PATH / LD_LIBRARY_PATH
    // 或模型凭据；只放运行必需项和从应用代理设置派生的代理变量。
    command.env_clear();
    for (key, value) in host_environment(&job.proxy_env) {
        command.env(key, value);
    }
    let child = command.spawn().map_err(|error| {
        format!(
            "启动 Pi host 失败（{}）：{error}",
            job.runtime.node.display()
        )
    })?;
    let mut guard = ChildGuard::new(child);
    let (stdin, stdout, stderr) = {
        let Some(child) = guard.0.as_mut() else {
            return Err("Pi host 启动后立即退出".into());
        };
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    };

    let (Some(mut stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
        return Err("Pi host 的管道没有建立起来".into());
    };

    let stderr_lines = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_bytes = Arc::new(Mutex::new(0usize));
    {
        let lines = stderr_lines.clone();
        let bytes = stderr_bytes.clone();
        let secret = job.model.api_key.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let buffer = match bounded_line(&mut reader, 32 * 1024) {
                    Ok(buffer) => buffer,
                    // 单行超长只丢这一行并继续排空：排空线程退出会让 host 写满管道后卡死。
                    Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                        skip_line(&mut reader);
                        let mut kept = lines.lock().unwrap_or_else(|poison| poison.into_inner());
                        kept.push_back("（已跳过一行超长 host 诊断输出）".to_string());
                        while kept.len() > 120 {
                            kept.pop_front();
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                if buffer.is_empty() {
                    break;
                }
                let line = scrub(&String::from_utf8_lossy(&buffer), &secret);
                let mut kept = lines.lock().unwrap_or_else(|poison| poison.into_inner());
                let mut total = bytes.lock().unwrap_or_else(|poison| poison.into_inner());
                *total += line.len();
                kept.push_back(line);
                while kept.len() > 120 || *total > 32 * 1024 {
                    if let Some(removed) = kept.pop_front() {
                        *total = total.saturating_sub(removed.len());
                    } else {
                        break;
                    }
                }
            }
        });
    }

    // 有界队列：host 即使持续快速输出，也不能无限堆积整篇候选稿。
    let (tx, rx) = mpsc::sync_channel(2);
    let stdin_errors = tx.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match bounded_line(&mut reader, MAX_HOST_LINE_BYTES) {
                Ok(buffer) if buffer.is_empty() => {
                    let _ = tx.send(StdoutMsg::Eof);
                    return;
                }
                Ok(mut buffer) => {
                    if !buffer.ends_with(b"\n") {
                        let _ =
                            tx.send(StdoutMsg::Protocol("host 输出的协议行未以换行结束".into()));
                        return;
                    }
                    while buffer
                        .last()
                        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
                    {
                        buffer.pop();
                    }
                    if buffer.is_empty() {
                        continue;
                    }
                    if std::str::from_utf8(&buffer).is_err() {
                        let _ = tx.send(StdoutMsg::Protocol("host 输出不是合法 UTF-8".into()));
                        return;
                    }
                    if tx.send(StdoutMsg::Line(buffer)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = tx.send(StdoutMsg::Protocol(format!("读 host 输出失败：{error}")));
                    return;
                }
            }
        }
    });

    let init = InitMessage {
        kind: "init",
        protocol: PROTOCOL_VERSION,
        request_id: job.request_id.clone(),
        run_id: job.run_id.clone(),
        workspace: InitWorkspace {
            root: path_text(&job.workspace.root),
            state: path_text(&job.workspace.state),
            current: "input/current.md".into(),
            request: "input/request.md".into(),
            skills: "skills/gongwen-markdown".into(),
            pi_skills: "skills/pi-skills".into(),
            materials: "input/materials".into(),
            delivery: path_text(&job.workspace.run),
        },
        model: InitModel {
            base_url: job.model.base_url.clone(),
            api_key: job.model.api_key.clone(),
            model_id: job.model.model_id.clone(),
            context_window: job.model.context_window,
            max_tokens: job.model.max_tokens,
            reasoning: job.model.reasoning,
            thinking_level: job.model.thinking_level.clone(),
        },
        strategy: InitStrategy {
            preference: job.task.strategy.as_str(),
            // 沿用准备阶段匹配的能力快照；未验证的预算由 host 保守规划，
            // 不按模型名称或参数量猜。任务规模与窗口由 host 现算。
            capability: InitCapability {
                verified_work_budget: job.task.capability.verified_work_budget,
                needs_short_steps: job.task.capability.needs_short_steps,
                reason: job.task.capability.reason.clone(),
            },
        },
        task: InitTask {
            request: job.task.request.clone(),
            kind: job.task.kind.clone(),
            system_date: job.task.system_date.clone(),
            writing_context: job.tools.writing_context(),
            skill: job.task.skill.clone(),
            selection: job.task.selection.map(|selection| InitSelection {
                start: selection.start,
                end: selection.end,
                text: selection.text(&job.before).to_string(),
            }),
            materials: job.task.materials.clone(),
        },
        limits: InitLimits {
            max_file_bytes: MAX_DELIVERY_BYTES as u64,
            max_candidate_bytes: MAX_CANDIDATE_BYTES as u64,
            timeout_seconds: job.timeout.as_secs().max(1),
            max_question_batches: MAX_QUESTION_BATCHES,
            max_questions: MAX_QUESTIONS_PER_BATCH,
        },
        recovery: job.resume.then(|| InitRecovery {
            mode: "resume",
            request_id: job.request_id.clone(),
            continuation_id: uuid::Uuid::new_v4().to_string(),
        }),
    };
    let mut payload =
        serde_json::to_vec(&init).map_err(|error| format!("组装 Pi init 消息失败：{error}"))?;
    payload.push(b'\n');
    // stdin 也可能被不读输入的 host 堵住；写线程只持管道，主循环仍能超时并回收进程。
    let (control_tx, control_rx) = mpsc::sync_channel(4);
    job.bridge.install(control_tx.clone());
    job.bridge.attach(job.tools.clone(), job.record.clone());
    std::thread::spawn(move || {
        if stdin.write_all(&payload).is_err() || stdin.flush().is_err() {
            let _ = stdin_errors.send(StdoutMsg::Protocol("Pi host 没有接收初始配置".into()));
            return;
        }
        while let Ok(line) = control_rx.recv() {
            if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                break;
            }
        }
    });

    let (tool_tx, tool_rx) = mpsc::channel::<ToolDone>();
    let mut pending_tool: Option<(String, Instant)> = None;
    let mut tool_calls = 0usize;
    let mut question_batches = 0u32;
    let mut waiting: Option<(String, u64, Instant)> = None;
    let mut paused = Duration::ZERO;
    let mut submitted: Option<Submission> = None;
    let mut report: Option<(String, Vec<PiReportItem>)> = None;
    let mut report_recovered = false;
    let mut delivered_receipt: Option<PiReceipt> = None;
    let mut ready = false;
    let mut events = 0usize;
    let mut cancel_sent: Option<Instant> = None;
    // 心跳：模型长输出期间宿主可能很久不发消息，用 ping/pong 区分“还在跑”与“卡死”。
    let mut last_message = Instant::now();
    let mut ping_sent: Option<Instant> = None;
    loop {
        if cancel.load(Ordering::Relaxed) && cancel_sent.is_none() {
            let _ = control_tx.try_send("{\"type\":\"cancel\"}\n".into());
            cancel_sent = Some(Instant::now());
        }
        if let Some(at) = cancel_sent
            && at.elapsed() > Duration::from_secs(2)
        {
            return Err("已停止 Pi 智能写作，未交付任何提案。".into());
        }
        // 心跳放在等事件之前：没有输出时也定期问一次，失联按“可接着写”失败处理。
        if ping_sent.is_none() && last_message.elapsed() > HEARTBEAT_QUIET {
            if control_tx
                .try_send("{\"type\":\"ping\"}\n".to_string())
                .is_ok()
            {
                ping_sent = Some(Instant::now());
            }
        } else if let Some(sent) = ping_sent
            && sent.elapsed() > HEARTBEAT_DEAD
        {
            let _ = control_tx.try_send("{\"type\":\"cancel\"}\n".into());
            if ready || !job.resume {
                let _ = job
                    .record
                    .finished(PiRunStatus::Failed, "Pi host 心跳超时，已停止本轮。");
            }
            return Err(
                "Pi host 超过 60 秒没有响应（可能卡死），已停止本轮；工作区保留，可「接着写」。"
                    .into(),
            );
        }
        // 人在看题时不计执行超时；回答之后接着按剩余时间算。
        if let Some((_, answered, since)) = &waiting {
            if job.bridge.answered() != *answered {
                // 回答已在 `PiBridge::answer` 里登记为事实并落盘（早于交给 host）。
                paused += since.elapsed();
                waiting = None;
            } else if since.elapsed() > Duration::from_secs(30 * 60) {
                return Err("等用户回答超过 30 分钟，已停止本轮。".into());
            }
        }
        if waiting.is_none() && started.elapsed().saturating_sub(paused) > job.timeout {
            let _ = control_tx.try_send("{\"type\":\"cancel\"}\n".into());
            let mut message = format!(
                "Pi 智能写作超时（{} 秒），未交付任何提案。",
                job.timeout.as_secs()
            );
            if (ready || !job.resume)
                && let Err(error) = job
                    .record
                    .finished(PiRunStatus::Failed, "Pi 智能写作执行超时。")
            {
                message.push_str(&format!("（运行记录未写成：{error}）"));
            }
            return Err(message);
        }
        if !ready
            && cancel_sent.is_none()
            && started.elapsed().saturating_sub(paused) > Duration::from_secs(30)
        {
            return Err("Pi host 启动后 30 秒内没有就绪，已停止。".into());
        }
        // 工具在独立线程执行：检索慢也不能让取消、超时和 host 输出停摆。
        match tool_rx.try_recv() {
            Ok(done) => {
                pending_tool = None;
                if cancel_sent.is_some() || cancel.load(Ordering::Relaxed) {
                    return Err("已停止 Pi 智能写作，未交付任何提案。".into());
                }
                let (ok, result, error) = match done.result {
                    Ok(value) => (true, value, String::new()),
                    Err(error) => (false, serde_json::Value::Null, error),
                };
                if ok {
                    if let Some(warnings) =
                        result.get("warnings").and_then(serde_json::Value::as_array)
                    {
                        for warning in warnings
                            .iter()
                            .take(3)
                            .filter_map(serde_json::Value::as_str)
                        {
                            let short: String = warning.chars().take(300).collect();
                            on_event(PiEvent::Tool(format!("Pi · kb_search 降级说明：{short}")));
                        }
                    }
                    let ledger = job.tools.evidence();
                    if let Some(item) = ledger.get(done.evidence_before) {
                        job.record.evidence(&item.line());
                        on_event(PiEvent::Evidence(item.line()));
                    }
                    // 工具实际返回的事实先落盘，再把结果交给 host：崩溃后恢复的闸门
                    // 仍认这些依据，不会因为“还没到最后交付”就丢掉。来源只是恢复辅助，
                    // 落盘失败记一行提示、继续本轮，不能因此中止已经花掉模型时间的任务。
                    if let Err(error) = job.record.persist_sources(&job.tools.fact_sources()) {
                        job.record
                            .evidence(&format!("本轮事实来源暂未落盘：{error}"));
                        on_event(PiEvent::Tool(format!(
                            "Pi · 事实来源暂未落盘（不影响本轮）：{error}"
                        )));
                    }
                }
                let line = serde_json::json!({
                    "type": "tool_result",
                    "callId": done.call_id,
                    "ok": ok,
                    "result": result,
                    "error": error,
                })
                .to_string()
                    + "\n";
                if control_tx.try_send(line).is_err() {
                    return Err("Pi host 不再接收工具结果，已停止。".into());
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {}
        }
        let message = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => StdoutMsg::Eof,
        };
        let line = match message {
            StdoutMsg::Line(line) => line,
            StdoutMsg::Protocol(error) => {
                return Err(host_failure(error, &stderr_lines, &job.model.api_key));
            }
            StdoutMsg::Eof => {
                if cancel_sent.is_some() {
                    return Err("已停止 Pi 智能写作，未交付任何提案。".into());
                }
                let status = wait_status(&mut guard);
                return Err(host_failure(
                    format!("Pi host 提前退出（{status}），没有完成本轮"),
                    &stderr_lines,
                    &job.model.api_key,
                ));
            }
        };
        events += 1;
        if events > MAX_EVENTS {
            return Err("Pi host 发来的消息过多，已停止。".into());
        }
        let event: HostEvent =
            serde_json::from_slice(&line).map_err(|error| format!("Pi host 协议错误：{error}"))?;
        if !event.matches(&job.request_id, &job.run_id) {
            return Err("Pi host 返回了不属于本轮的消息，已停止。".into());
        }
        if cancel_sent.is_some() || cancel.load(Ordering::Relaxed) {
            return Err("已停止 Pi 智能写作，未交付任何提案。".into());
        }
        last_message = Instant::now();
        ping_sent = None;
        match event {
            HostEvent::Ready {
                protocol,
                host_version,
                node_version,
                model,
                ..
            } => {
                if protocol != Some(PROTOCOL_VERSION) {
                    return Err(format!(
                        "Pi host 协议版本不匹配：应用是 {PROTOCOL_VERSION}，host 是 {:?}",
                        protocol
                    ));
                }
                let reported = host_version.as_deref().unwrap_or("");
                if reported != HOST_VERSION {
                    return Err(format!(
                        "Pi host 版本不匹配：应用需要 {HOST_VERSION}，host 报告 {reported:?}。请重装或更新随包运行时。"
                    ));
                }
                job.record.ready(reported)?;
                on_event(PiEvent::Progress(format!(
                    "Pi host 已就绪（host {} / Node {} / 模型 {}）",
                    reported,
                    node_version.as_deref().unwrap_or("?"),
                    model.as_deref().unwrap_or("?")
                )));
                ready = true;
            }
            HostEvent::Progress { message, .. } => {
                on_event(PiEvent::Progress(scrub(
                    &short(&message),
                    &job.model.api_key,
                )));
            }
            HostEvent::Strategy {
                mode,
                reasons,
                window,
                work_budget,
                materials_budget,
                verified,
                ..
            } => {
                let info = PiStrategyInfo {
                    mode: short(&mode),
                    reasons: reasons
                        .into_iter()
                        .take(8)
                        .map(|reason| short(&reason))
                        .collect(),
                    window,
                    work_budget,
                    materials_budget,
                    verified,
                };
                on_event(PiEvent::Strategy(info));
            }
            HostEvent::Tool { name, message, .. } => {
                let line = match message.filter(|text| !text.trim().is_empty()) {
                    Some(message) => format!(
                        "Pi · {name}：{}",
                        scrub(&short(&message), &job.model.api_key)
                    ),
                    None => format!("Pi · {name}"),
                };
                on_event(PiEvent::Tool(line));
            }
            HostEvent::Candidate {
                path,
                content,
                sha256,
                bytes,
                ..
            } => {
                if !safe_work_path(&path) {
                    return Err("Pi host 返回的候选路径不在 work/ 下".into());
                }
                if content.len() > MAX_CANDIDATE_BYTES {
                    continue;
                }
                if bytes != content.len() || sha256_hex(&content) != sha256.to_ascii_lowercase() {
                    return Err("Pi host 返回的候选稿与哈希不一致".into());
                }
                let _ = content;
                on_event(PiEvent::Candidate { path });
            }
            HostEvent::ToolRequest {
                call_id,
                name,
                args,
                ..
            } => {
                if !ready {
                    return Err("Pi host 还没有报告就绪就调用了工具，已拒绝。".into());
                }
                if pending_tool.is_some() {
                    return Err("Pi host 同时发起了多个工具调用，已停止。".into());
                }
                if call_id.trim().is_empty() || call_id.len() > 64 {
                    return Err("Pi host 的工具调用 ID 不合法".into());
                }
                tool_calls += 1;
                if tool_calls > MAX_TOTAL_TOOL_CALLS {
                    return Err(format!(
                        "本轮工具调用次数过多（最多 {MAX_TOTAL_TOOL_CALLS} 次），已停止；工作区保留，可「接着写」。"
                    ));
                }
                let args_text = serde_json::to_vec(&args)
                    .map_err(|error| format!("工具参数无法序列化：{error}"))?;
                if args_text.len() > MAX_TOOL_ARGS_BYTES {
                    return Err("工具参数超过上限，已拒绝。".into());
                }
                if !PiToolContext::known(&name) {
                    let line = serde_json::json!({
                        "type": "tool_result",
                        "callId": call_id,
                        "ok": false,
                        "error": format!("未知工具：{name}"),
                    })
                    .to_string()
                        + "\n";
                    let _ = control_tx.try_send(line);
                    on_event(PiEvent::Tool(format!("Pi · 拒绝未知工具 {name}")));
                    continue;
                }
                let tools = job.tools.clone();
                let tx = tool_tx.clone();
                on_event(PiEvent::Tool(format!("Pi · {name}：执行中")));
                let evidence_before = job.tools.evidence().len();
                pending_tool = Some((call_id.clone(), Instant::now()));
                std::thread::spawn(move || {
                    let result = tools.call(&name, &args);
                    let _ = tx.send(ToolDone {
                        call_id,
                        evidence_before,
                        result,
                    });
                });
            }
            HostEvent::Questions {
                call_id,
                questions,
                batch,
                ..
            } => {
                if !ready {
                    return Err("Pi host 还没有报告就绪就出题，已拒绝。".into());
                }
                question_batches += 1;
                let raw: Vec<(String, Vec<String>)> = questions
                    .into_iter()
                    .map(|RawQuestion { question, options }| (question, options))
                    .collect();
                let accepted =
                    if batch > MAX_QUESTION_BATCHES || question_batches > MAX_QUESTION_BATCHES {
                        Err("本轮提问批次已用完（最多 2 批）".to_string())
                    } else if raw.len() as u32 > MAX_QUESTIONS_PER_BATCH {
                        Err("一次最多问 3 道".to_string())
                    } else {
                        crate::agent::clarify::agent_questions(&raw)
                    };
                match accepted {
                    Ok(questions) => {
                        let questions = questions
                            .into_iter()
                            .map(|question: Question| PiQuestion {
                                id: question.id,
                                text: question.text,
                                options: question
                                    .choices
                                    .iter()
                                    .map(|choice| choice.label.clone())
                                    .collect(),
                                custom: question.custom_hint.is_some(),
                                skippable: question.skippable,
                            })
                            .collect();
                        waiting = Some((call_id.clone(), job.bridge.answered(), Instant::now()));
                        on_event(PiEvent::Progress(
                            "模型在等你的回答（等待时间不占用执行超时）".into(),
                        ));
                        on_event(PiEvent::Questions(PiQuestionBatch {
                            call_id,
                            batch: batch.max(question_batches),
                            questions,
                        }));
                    }
                    Err(error) => {
                        let line = serde_json::json!({
                            "type": "tool_result",
                            "callId": call_id,
                            "ok": false,
                            "error": error,
                        })
                        .to_string()
                            + "\n";
                        let _ = control_tx.try_send(line);
                        on_event(PiEvent::Tool(format!("Pi · 提问被拒：{error}")));
                    }
                }
            }
            HostEvent::Report {
                summary,
                items,
                recovered,
                ..
            } => {
                if !ready {
                    return Err("Pi host 还没有报告就绪就交了清单，已拒绝。".into());
                }
                if submitted.is_some() {
                    return Err("Pi host 既提交了正文又交了审核清单，已拒绝。".into());
                }
                let _ = (summary, items);
                let Some(receipt) = read_receipt(&job.workspace.run, &job.run_id)? else {
                    return Err("Pi host 交了清单却没有落交付回执，已拒绝交付。".into());
                };
                let known_kind = builtin_delivery_kind(job.task.skill.as_deref());
                let historical_replay = job.resume
                    && recovered
                    && preexisting_receipt.as_ref().is_some_and(|prior| {
                        prior.kind == receipt.kind
                            && prior.request_id == receipt.request_id
                            && prior.run_id == receipt.run_id
                            && prior.summary == receipt.summary
                            && prior.items == receipt.items
                    });
                if receipt.kind != "report"
                    || receipt.items.len() > 100
                    || job.task.skill.as_deref() == Some(super::pi_skills::STYLE_SKILL)
                    || (known_kind.is_some_and(|kind| kind != "report") && !historical_replay)
                    || receipt.request_id != job.request_id
                {
                    return Err("Pi 清单回执的内容不对，已拒绝。".into());
                }
                let items = receipt
                    .items
                    .iter()
                    .take(100)
                    .map(|item| PiReportItem {
                        title: short(&item.title),
                        detail: short(&item.detail),
                        severity: short(&item.severity),
                    })
                    .collect();
                report = Some((short(&receipt.summary), items));
                report_recovered = recovered;
                delivered_receipt = Some(receipt);
            }
            HostEvent::Submitted {
                path,
                summary,
                content,
                sha256,
                bytes,
                recovered,
                ..
            } => {
                if !ready {
                    return Err("Pi host 还没有报告就绪就送来了成品，已拒绝。".into());
                }
                if report.is_some() {
                    return Err("Pi host 既交了清单又提交正文，已拒绝。".into());
                }
                verify_submission(&path, bytes, &sha256, &content)?;
                if content.len() > MAX_DELIVERY_BYTES {
                    return Err("Pi host 提交的 Markdown 超过上限".into());
                }
                if summary.chars().count() > 2_000 {
                    return Err("Pi host 提交说明太长，已拒绝。".into());
                }
                let Some(receipt) = read_receipt(&job.workspace.run, &job.run_id)? else {
                    return Err("Pi host 提交了正文却没有落交付回执，已拒绝交付。".into());
                };
                let style_task = job.task.skill.as_deref() == Some(super::pi_skills::STYLE_SKILL)
                    && job.task.kind == "风格档案";
                let expected_kind = if style_task { "style" } else { "document" };
                let historical_replay = job.resume
                    && recovered
                    && preexisting_receipt.as_ref().is_some_and(|prior| {
                        prior.kind == receipt.kind
                            && prior.request_id == receipt.request_id
                            && prior.run_id == receipt.run_id
                            && prior.path == receipt.path
                            && prior.sha256 == receipt.sha256
                            && prior.bytes == receipt.bytes
                            && prior.summary == receipt.summary
                    });
                if receipt.kind != expected_kind
                    || receipt.path != path
                    || receipt.bytes != bytes
                    || receipt.sha256 != sha256.to_ascii_lowercase()
                    || (builtin_delivery_kind(job.task.skill.as_deref())
                        .is_some_and(|kind| kind != expected_kind)
                        && !historical_replay)
                {
                    return Err("Pi 交付回执与提交内容不一致，已拒绝。".into());
                }
                on_event(PiEvent::Progress(format!("已提交 {path}，准备交付检查…")));
                submitted = Some(Submission { receipt, recovered });
            }
            HostEvent::Completed {
                submitted: claimed, ..
            } => {
                if !ready {
                    return Err("Pi host 还没有报告就绪就结束了本轮，已拒绝。".into());
                }
                if claimed {
                    let Some(submission) = submitted else {
                        return Err("Pi host 报告已提交，但没有送来成品内容".into());
                    };
                    if report.is_some() {
                        return Err("Pi host 前后矛盾：交清单的同时报告已提交正文".into());
                    }
                    // 以 host 写的不可变快照与回执为准，不重读模型可写的 work/。
                    let content = read_snapshot(&job.workspace.run, &submission.receipt)?;
                    verify_submission(
                        &submission.receipt.path,
                        submission.receipt.bytes,
                        &submission.receipt.sha256,
                        &content,
                    )?;
                    let is_style = submission.receipt.kind == "style";
                    if is_style {
                        super::style_learning::validate_draft(&content)?;
                    }
                    // 完成事件之后仍须确认进程正常退出，不能接受“先报成功再崩溃”。
                    successful_exit(&mut guard, &cancel)?;
                    // 成品要直接写进正文：修改前基线必须确实在盘上，撤销才有准确的原稿。
                    super::recovery::check_baseline_snapshot(&job.workspace, &job.before)?;
                    let sources = job.tools.fact_sources();
                    let sources_sha256 =
                        job.record.delivered(&submission.receipt, Some(&sources))?;
                    let delivery = PiDelivery {
                        before: job.before,
                        base: job.base,
                        request: job.task.request,
                        content,
                        path: submission.receipt.path,
                        summary: submission.receipt.summary,
                        sha256: submission.receipt.sha256,
                        bytes: submission.receipt.bytes,
                        selection: job.task.selection,
                        fact_sources: sources,
                        confirmed: job.bridge.confirmed(),
                        workspace_label: job.workspace.label,
                        elapsed: started.elapsed().saturating_sub(paused),
                        recovered: submission.recovered,
                        sources_sha256,
                    };
                    return Ok(if is_style {
                        PiOutcome::Style(delivery)
                    } else {
                        PiOutcome::Document(delivery)
                    });
                }
                if submitted.is_some() {
                    return Err("Pi host 前后矛盾：送来了成品却报告未提交".into());
                }
                let Some((summary, items)) = report else {
                    return Err(
                        "模型结束了本轮但没有提交成品（未调用 submit_document 或 finish_report），未交付任何结果。"
                            .into(),
                    );
                };
                successful_exit(&mut guard, &cancel)?;
                let Some(receipt) = delivered_receipt else {
                    return Err("Pi 清单没有交付回执，未交付任何结果。".into());
                };
                let sources_sha256 = job.record.delivered(&receipt, None)?;
                return Ok(PiOutcome::Report(PiReport {
                    summary,
                    items,
                    confirmed: job.bridge.confirmed(),
                    workspace_label: job.workspace.label,
                    elapsed: started.elapsed().saturating_sub(paused),
                    recovered: report_recovered,
                    sources_sha256,
                }));
            }
            HostEvent::Cancelled { message, .. } => {
                let message = if message.trim().is_empty() {
                    "已停止 Pi 智能写作，未交付任何提案。".to_string()
                } else {
                    short(&scrub(&message, &job.model.api_key))
                };
                if ready || !job.resume {
                    let _ = job.record.finished(PiRunStatus::Interrupted, &message);
                }
                return Err(message);
            }
            HostEvent::Error { code, message, .. } => {
                let code = code.unwrap_or_else(|| "host_error".into());
                let message = scrub(&short(&message), &job.model.api_key);
                let parked = code == "task_parked";
                if ready || !job.resume {
                    let _ = job.record.finished(
                        if parked {
                            PiRunStatus::Parked
                        } else {
                            PiRunStatus::Failed
                        },
                        &message,
                    );
                }
                return Err(host_failure(
                    format!("Pi host 失败（{code}）：{message}"),
                    &stderr_lines,
                    &job.model.api_key,
                ));
            }
            HostEvent::Pong { .. } => {}
        }
    }
}

/// 读取时即限额，不能先 read_until 分配任意大的缓冲再检查长度。
fn bounded_line(reader: &mut impl BufRead, max: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    reader
        .take(max.saturating_add(1) as u64)
        .read_until(b'\n', &mut buffer)?;
    if buffer.len() > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "host 输出单行超过上限",
        ));
    }
    Ok(buffer)
}

/// 丢掉当前这一行的剩余字节（已经读了前 max 字节），返回是否正常遇到换行。
fn skip_line(reader: &mut impl BufRead) -> bool {
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(_) => return false,
        };
        if chunk.is_empty() {
            return false;
        }
        if let Some(position) = chunk.iter().position(|byte| *byte == b'\n') {
            reader.consume(position + 1);
            return true;
        }
        let length = chunk.len();
        reader.consume(length);
    }
}

/// 交给 host 的最小环境：只保留运行必需项，再叠加应用代理设置派生的变量。
/// 不继承 `NODE_OPTIONS` / `NODE_PATH` / `LD_LIBRARY_PATH` 或任何模型凭据。
fn host_environment(proxy: &[(String, String)]) -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "PATH",
        "Path",
        "SystemRoot",
        "SYSTEMROOT",
        "windir",
        "TEMP",
        "TMP",
        "TMPDIR",
        "HOME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
    ];
    let mut environment = Vec::new();
    for key in KEEP {
        if let Some(value) = std::env::var_os(key) {
            environment.push(((*key).to_string(), value.to_string_lossy().into_owned()));
        }
    }
    environment.extend(proxy.iter().cloned());
    environment
}

fn successful_exit(guard: &mut ChildGuard, cancel: &AtomicBool) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已停止 Pi 智能写作，未交付任何提案。".into());
        }
        let child = guard.0.as_mut().ok_or("Pi host 进程不存在")?;
        match child
            .try_wait()
            .map_err(|error| format!("读取 Pi host 退出状态失败：{error}"))?
        {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(format!(
                    "Pi host 报告完成后异常退出（{status}），未交付提案。"
                ));
            }
            // host 报完成后还要等它自己退出；慢盘/杀软下 sqlite 收尾可能超过 2 秒，
            // 给到 10 秒，避免成品已落盘却报“未正常退出”。
            None if started.elapsed() >= Duration::from_secs(10) => {
                return Err("Pi host 报告完成后未正常退出，未交付提案。".into());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn wait_status(guard: &mut ChildGuard) -> String {
    let Some(mut child) = guard.take() else {
        return "已退出".into();
    };
    match child.try_wait() {
        Ok(Some(status)) => status.to_string(),
        Ok(None) => {
            let _ = child.kill();
            match child.wait() {
                Ok(status) => format!("{status}（已回收）"),
                Err(error) => format!("无法读取退出码：{error}"),
            }
        }
        Err(error) => format!("无法读取退出码：{error}"),
    }
}

fn host_failure(
    message: impl Into<String>,
    stderr: &Arc<Mutex<VecDeque<String>>>,
    secret: &str,
) -> String {
    let message = message.into();
    let tail = tail(stderr, secret);
    if tail.is_empty() {
        message
    } else {
        format!("{message}\nhost 诊断：{tail}")
    }
}

fn tail(stderr: &Arc<Mutex<VecDeque<String>>>, secret: &str) -> String {
    let lines = stderr.lock().unwrap_or_else(|poison| poison.into_inner());
    let text = lines
        .iter()
        .rev()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    scrub(&text, secret).chars().take(2_000).collect()
}

fn scrub(text: &str, secret: &str) -> String {
    if secret.len() >= 4 {
        text.replace(secret, "[已隐藏]")
    } else {
        text.to_string()
    }
}

fn short(text: &str) -> String {
    let text = text.replace(['\r', '\0'], " ");
    if text.chars().count() > MAX_MESSAGE_CHARS {
        let mut out: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
        out.push('…');
        out
    } else {
        text
    }
}

fn path_text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// 把已确认的回答交给追问上下文；`Reply` 只为与旧 `clarify` 的题面结构对齐，
/// 这里不调用 `engine::apply_answers`。
#[allow(dead_code)]
fn answer_text(question: &Question, reply: &Reply) -> String {
    match reply {
        Reply::Choice(index) => question
            .choices
            .get(*index)
            .map(|choice| choice.label.clone())
            .unwrap_or_default(),
        Reply::Custom(text) => text.clone(),
        Reply::Many(indexes) => indexes
            .iter()
            .filter_map(|index| question.choices.get(*index))
            .map(|choice| choice.label.clone())
            .collect::<Vec<_>>()
            .join("、"),
        Reply::Skip => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_run_ownership_lasts_until_the_record_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        let guard = lock_run(dir.path()).unwrap();
        assert!(lock_run(dir.path()).is_err());
        drop(guard);
        assert!(lock_run(dir.path()).is_ok());
    }
    use std::path::{Path, PathBuf};

    /// 真实 Node 小夹具：只验证协议、进程回收与交付判定；不访问任何模型端点。
    const FIXTURE: &str = r##"
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
const rl = readline.createInterface({ input: process.stdin });
let initRaw;
let handled = false;
const waiters = new Map();
const emit = (o) => process.stdout.write(JSON.stringify({ ...base(o), ...o }) + "\n");
let init;
function base(o) { return { protocol: 3, requestId: init?.requestId ?? "r", runId: init?.runId ?? "u" }; }
const content = "# 成品\n\n正文。\n";
const sha256 = createHash("sha256").update(content, "utf8").digest("hex");
const delivery = () => init.workspace.delivery;
const writeReceipt = (receipt) => {
  const dir = delivery();
  fs.writeFileSync(path.join(dir, "final.md"), content);
  fs.writeFileSync(path.join(dir, "receipt.json"), JSON.stringify({
    version: 1, runId: init.runId, requestId: init.requestId,
    at: "2026-11-01T00:00:00+08:00", ...receipt,
  }));
};
const submit = () => {
  const receipt = { kind: "document", path: "work/final.md", sha256, bytes: Buffer.byteLength(content), summary: "夹具", callId: "c1" };
  writeReceipt(receipt);
  emit({ type: "candidate", path: "work/final.md", content, sha256, bytes: Buffer.byteLength(content) });
  emit({ type: "submitted", path: "work/final.md", summary: "夹具", content, sha256, bytes: Buffer.byteLength(content) });
  emit({ type: "completed", submitted: true });
  setTimeout(() => process.exit(0), 20);
};
rl.on("line", (line) => {
  const value = JSON.parse(line);
  if (value.type === "init" && !handled) {
    handled = true;
    init = value;
    const mode = init.task.request;
    if (mode === "forged-report") {
      writeReceipt({ kind: "report", summary: "审完", items: [{ title: "称谓", detail: "第二条", severity: "建议" }] });
    }
    emit({ type: "ready", hostVersion: "0.3.5", nodeVersion: process.versions.node, model: init.model.modelId });
    if (init.recovery?.mode === "resume") {
      const dir = init.workspace.delivery;
      const receipt = JSON.parse(fs.readFileSync(path.join(dir, "receipt.json"), "utf8"));
      if (receipt.kind === "report") {
        emit({ type: "report", summary: receipt.summary, items: receipt.items, recovered: true });
        emit({ type: "completed", submitted: false });
      } else {
        const text = fs.readFileSync(path.join(dir, "final.md"), "utf8");
        emit({ type: "submitted", path: receipt.path, summary: receipt.summary, content: text, sha256: receipt.sha256, bytes: receipt.bytes, recovered: true });
        emit({ type: "completed", submitted: true });
      }
      setTimeout(() => process.exit(0), 20);
      return;
    }
    if (mode === "error") {
      emit({ type: "error", code: "boom", message: "失败且包含密钥 " + init.model.apiKey });
      setTimeout(() => process.exit(1), 20);
      return;
    }
    if (mode === "nosubmit") {
      emit({ type: "completed", submitted: false });
      setTimeout(() => process.exit(0), 20);
      return;
    }
    if (mode === "wrongid") {
      process.stdout.write(JSON.stringify({ type: "progress", requestId: "other", runId: init.runId, message: "x" }) + "\n");
      return;
    }
    if (mode === "hang") return;
    if (mode === "style") {
      const text = JSON.stringify({name:"简洁部署",description:"开头直接说明目的，主体按任务安排。",example_numbers:[]});
      const hash = createHash("sha256").update(text).digest("hex");
      fs.writeFileSync(path.join(delivery(),"final.json"),text);
      fs.writeFileSync(path.join(delivery(),"receipt.json"),JSON.stringify({version:1,runId:init.runId,requestId:init.requestId,kind:"style",path:"work/style.json",sha256:hash,bytes:Buffer.byteLength(text),summary:"风格",callId:"style"}));
      emit({type:"submitted",path:"work/style.json",summary:"风格",content:text,sha256:hash,bytes:Buffer.byteLength(text)});
      emit({type:"completed",submitted:true});
      setTimeout(()=>process.exit(0),20);
      return;
    }
    if (mode === "capability") {
      if (!init.task.writingContext?.date?.now?.includes("星期") || !init.task.writingContext?.date?.timezone || !Array.isArray(init.task.writingContext?.organization?.results)) {
        emit({ type: "error", code: "context_missing", message: "Init did not carry date and organization context" });
        setTimeout(() => process.exit(1), 20);
        return;
      }
      if (init.strategy?.capability?.verifiedWorkBudget !== 4096 || init.strategy.capability.needsShortSteps !== true || init.strategy.capability.reason !== "matched evidence") {
        emit({ type: "error", code: "capability_missing", message: "InitCapability did not carry matched evidence" });
        setTimeout(() => process.exit(1), 20);
        return;
      }
      submit();
      return;
    }
    if (mode === "badargs") {
      emit({ type: "tool_request", callId: "c1", name: "kb_search", args: { query: "x" } });
      return;
    }
    if (mode === "tool") {
      emit({ type: "tool_request", callId: "c1", name: "kb_search", args: { query: "防火" } });
      return;
    }
    if (mode === "ask") {
      emit({ type: "questions", callId: "q1", batch: 1, questions: [{ question: "想强调哪方面", options: ["责任", "时限"] }] });
      return;
    }
    if (mode === "report" || mode === "forged-report") {
      if (mode === "report") writeReceipt({ kind: "report", summary: "审完", items: [{ title: "称谓", detail: "第二条", severity: "建议" }] });
      emit({ type: "report", summary: "审完", items: [{ title: "称谓", detail: "第二条", severity: "建议" }], recovered: mode === "forged-report" });
      emit({ type: "completed", submitted: false });
      setTimeout(() => process.exit(0), 20);
      return;
    }
    submit();
    return;
  }
  if (value.type === "cancel") { process.exit(3); }
  if (value.type === "tool_result") {
    const resolve = waiters.get(value.callId);
    if (resolve) { waiters.delete(value.callId); resolve(value); return; }
    if (value.ok && value.callId === "c1" && init.task.request === "tool") { submit(); }
    if (value.ok && value.callId === "q1") {
      writeReceipt({ kind: "report", summary: "答完了", items: [{ title: "已按回答处理", detail: "", severity: "提示" }] });
      emit({ type: "report", summary: "答完了", items: [{ title: "已按回答处理", detail: "", severity: "提示" }] });
      emit({ type: "completed", submitted: false });
      setTimeout(() => process.exit(0), 20);
    }
    if (!value.ok) {
      writeReceipt({ kind: "report", summary: "工具被拒：" + value.error, items: [] });
      emit({ type: "report", summary: "工具被拒：" + value.error, items: [] });
      emit({ type: "completed", submitted: false });
      setTimeout(() => process.exit(0), 20);
    }
  }
});
"##;

    fn node_available() -> bool {
        std::process::Command::new("node")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn production_init_carries_the_matched_capability_snapshot() {
        if !node_available() {
            eprintln!("skip: node runtime is unavailable");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut job = job(temp.path(), "capability");
        job.task.capability = super::super::strategy::PiCapabilitySnapshot {
            verified_work_budget: Some(4096),
            needs_short_steps: true,
            identity_key: "identity-hash".into(),
            reason: "matched evidence".into(),
        };
        let outcome = run(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(matches!(outcome, PiOutcome::Document(_)));
    }

    #[test]
    fn style_receipt_is_a_separate_outcome_and_wrong_task_cannot_accept_it() {
        if !node_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut style = job(temp.path(), "style");
        style.task.skill = Some(super::super::pi_skills::STYLE_SKILL.into());
        style.task.kind = "风格档案".into();
        let outcome = run(style, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(matches!(outcome, PiOutcome::Style(_)));
        let temp = tempfile::tempdir().unwrap();
        let wrong = job(temp.path(), "style");
        assert!(
            run(wrong, Arc::new(AtomicBool::new(false)), |_| {})
                .unwrap_err()
                .contains("回执与提交内容不一致")
        );
    }

    fn fixture(dir: &Path) -> PathBuf {
        let path = dir.join("host-fixture.mjs");
        std::fs::write(&path, FIXTURE).unwrap();
        path
    }

    struct NoKb;
    impl super::super::tools::PiKnowledge for NoKb {
        fn enabled(&self) -> bool {
            true
        }
        fn search(
            &self,
            _query: &str,
            _top: usize,
        ) -> Result<super::super::tools::PiSearchOutcome, String> {
            Ok(super::super::tools::PiSearchOutcome {
                chunks: vec![super::super::tools::PiChunk {
                    doc_id: 1,
                    title: "防火规定".into(),
                    section: "第一条".into(),
                    text: "各单位应于每年十月开展一次检查。".into(),
                }],
                warnings: Vec::new(),
            })
        }
        fn read(&self, _doc_id: i64) -> Result<Option<(String, String)>, String> {
            Ok(None)
        }
    }

    struct NoMs;
    impl super::super::tools::PiManuscripts for NoMs {
        fn search(
            &self,
            _query: &str,
            _limit: usize,
        ) -> Result<Vec<super::super::tools::PiManuscriptRow>, String> {
            Ok(Vec::new())
        }
        fn read(
            &self,
            _id: i64,
            _version: Option<i64>,
        ) -> Result<Option<super::super::tools::PiManuscriptRow>, String> {
            Ok(None)
        }
    }

    fn job(dir: &Path, mode: &str) -> PiJob {
        let root = dir.join("workspace");
        std::fs::create_dir_all(root.join("input")).unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("input/current.md"), "# 当前\n").unwrap();
        std::fs::write(root.join("input/request.md"), mode).unwrap();
        let config = Arc::new(crate::models::AppConfig::default());
        let tools = PiToolContext::with_sources(
            config,
            crate::models::DraftInput::default(),
            "# 当前\n".into(),
            mode.into(),
            Vec::new(),
            Arc::new(NoKb),
            Arc::new(NoMs),
        );
        PiJob {
            runtime: PiRuntime {
                node: PathBuf::from("node"),
                host: fixture(dir),
                bundled: false,
            },
            workspace: PiWorkspace {
                root,
                state: dir.join("session.sqlite"),
                run: dir.to_path_buf(),
                label: "test/workspace".into(),
            },
            model: PiModel {
                base_url: "http://127.0.0.1:9/v1".into(),
                api_key: "sk-test-secret-1234".into(),
                model_id: "fixture".into(),
                context_window: 8192,
                window_source: super::super::capability::CapabilityWindowSource::Default,
                max_tokens: 1024,
                reasoning: false,
                thinking_level: None,
            },
            task: PiTask {
                request: mode.into(),
                kind: "通知".into(),
                system_date: "2026-11-01".into(),
                skill: None,
                strategy: super::super::strategy::PiStrategyPreference::Auto,
                capability: super::super::strategy::PiCapabilitySnapshot::default(),
                selection: None,
                materials: Vec::new(),
            },
            before: "# 当前\n".into(),
            base: "# 当前\n".into(),
            tools: Arc::new(tools),
            bridge: Arc::new(PiBridge::default()),
            record: {
                let workspace = PiWorkspace {
                    root: dir.join("workspace"),
                    state: dir.join("session.sqlite"),
                    run: dir.to_path_buf(),
                    label: "test/workspace".into(),
                };
                super::super::recovery::test_log(&workspace, "req-1", "run-1")
            },
            resume: false,
            request_id: "req-1".into(),
            run_id: "run-1".into(),
            timeout: Duration::from_secs(10),
            proxy_env: Vec::new(),
        }
    }

    fn outcome(
        job: PiJob,
        cancel: Arc<AtomicBool>,
        mut on: impl FnMut(PiEvent),
    ) -> Result<PiOutcome, String> {
        run(job, cancel, &mut on)
    }

    #[test]
    fn submitted_snapshot_is_verified_and_returned() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let outcome = outcome(
            job(temp.path(), "ok"),
            Arc::new(AtomicBool::new(false)),
            |event| {
                sink.lock().unwrap().push(format!("{event:?}"));
            },
        )
        .unwrap();
        let PiOutcome::Document(delivery) = outcome else {
            panic!("应交付文档");
        };
        assert_eq!(delivery.content, "# 成品\n\n正文。\n");
        assert_eq!(delivery.path, "work/final.md");
        assert!(delivery.bytes > 0);
        let events = events.lock().unwrap();
        assert!(events.iter().any(|line| line.contains("Candidate")));
        assert!(events.iter().any(|line| line.contains("已提交")));
    }

    #[test]
    fn a_resume_with_a_receipt_replays_it_through_the_host_and_verifies_the_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let mut job = job(temp.path(), "ok");
        job.resume = true;
        let content = "# 恢复的成品\n";
        std::fs::write(temp.path().join("final.md"), content).unwrap();
        let receipt = serde_json::json!({
            "version": 1,
            "runId": "run-1",
            "requestId": "req-1",
            "kind": "document",
            "path": "work/final.md",
            "sha256": sha256_hex(content),
            "bytes": content.len(),
            "summary": "恢复",
            "callId": "c1",
            "at": "2026-11-01T00:00:00+08:00",
        });
        std::fs::write(
            temp.path().join("receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let outcome = outcome(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let PiOutcome::Document(delivery) = outcome else {
            panic!("应交付文档");
        };
        assert!(delivery.recovered, "host 重放回执时必须标记为恢复");
        assert_eq!(delivery.content, content);
        assert_eq!(delivery.sha256, sha256_hex(content));
    }

    #[test]
    fn answers_are_persisted_before_the_host_can_see_them() {
        let temp = tempfile::tempdir().unwrap();
        let job = job(temp.path(), "ask");
        let (tx, rx) = mpsc::sync_channel(4);
        job.bridge.install(tx);
        job.bridge.attach(job.tools.clone(), job.record.clone());
        let answers = vec![PiAnswer {
            id: 1,
            question: "想强调哪方面".into(),
            value: "责任".into(),
        }];
        job.bridge.answer("q1", &answers).unwrap();
        // 管道里已有回答，但落盘发生在发送之前。
        assert!(rx.try_recv().is_ok());
        let record = super::super::recovery::PiRunRecord::read_from(&job.workspace.run).unwrap();
        assert_eq!(record.answers, vec!["责任"]);
        assert!(
            record
                .confirmed
                .iter()
                .any(|line| line.contains("想强调哪方面：责任"))
        );
        let hash = record.sources.expect("回答必须进入事实来源");
        let sources = super::super::recovery::read_sources(&job.workspace.run, &hash)
            .unwrap()
            .unwrap();
        assert!(sources.contains("责任"));
        assert!(!sources.contains("想强调哪方面"), "题面不是事实来源");
    }

    #[test]
    fn line_limit_applies_before_reading_an_unbounded_line() {
        let mut reader = std::io::Cursor::new(vec![b'x'; 1024]);
        assert!(bounded_line(&mut reader, 8).is_err());
        assert_eq!(reader.position(), 9);
    }

    #[test]
    fn a_host_that_never_reads_init_still_times_out() {
        if !node_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut stalled = job(temp.path(), "stalled");
        std::fs::write(&stalled.runtime.host, "setInterval(() => {}, 1000);").unwrap();
        stalled.task.request = "x".repeat(150_000);
        stalled.timeout = Duration::from_millis(200);
        let started = Instant::now();
        let error = outcome(stalled, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("超时"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn completed_event_cannot_hide_a_failed_exit() {
        if !node_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let job = job(temp.path(), "badexit");
        let script = std::fs::read_to_string(&job.runtime.host)
            .unwrap()
            .replace("process.exit(0), 20", "process.exit(7), 20");
        std::fs::write(&job.runtime.host, script).unwrap();
        let error = outcome(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("异常退出"), "{error}");
    }

    #[test]
    fn cancellation_wins_over_a_queued_completion() {
        if !node_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let error = outcome(job(temp.path(), "ok"), cancel, |event| {
            if matches!(event, PiEvent::Progress(ref message) if message.starts_with("已提交 "))
            {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .unwrap_err();
        assert!(error.contains("已停止"), "{error}");
    }

    #[test]
    fn a_run_without_submit_or_report_is_a_failure() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let error = outcome(
            job(temp.path(), "nosubmit"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("没有提交成品"), "{error}");
    }

    #[test]
    fn host_errors_are_failures_and_do_not_echo_the_key() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let error = outcome(
            job(temp.path(), "error"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("Pi host 失败"), "{error}");
        assert!(!error.contains("sk-test-secret-1234"), "{error}");
    }

    #[test]
    fn a_message_for_another_request_is_rejected() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let error = outcome(
            job(temp.path(), "wrongid"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains("不属于本轮"), "{error}");
    }

    #[test]
    fn cancel_reclaims_the_process_and_is_not_a_completion() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let start = Instant::now();
        let handle = std::thread::spawn(move || outcome(job(temp.path(), "hang"), cancel, |_| {}));
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::Relaxed);
        let error = handle.join().unwrap().unwrap_err();
        assert!(error.contains("停止"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn eof_before_completion_is_a_failure() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut job = job(temp.path(), "ok");
        // 把 host 换成一个立即退出、什么都不写的脚本。
        let empty = temp.path().join("empty-host.mjs");
        std::fs::write(&empty, "process.exit(0);\n").unwrap();
        job.runtime.host = empty;
        let error = outcome(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("提前退出"), "{error}");
    }

    #[test]
    fn tool_request_round_trip_registers_evidence_and_can_submit() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let outcome = outcome(
            job(temp.path(), "tool"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let PiOutcome::Document(delivery) = outcome else {
            panic!("工具跑通后应交付文档");
        };
        assert!(
            delivery
                .fact_sources
                .iter()
                .any(|text| text.contains("每年十月")),
            "{:?}",
            delivery.fact_sources
        );
    }

    #[test]
    fn unknown_or_oversized_tool_requests_are_rejected_structurally() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let tool_job = job(temp.path(), "tool");
        let script = std::fs::read_to_string(&tool_job.runtime.host)
            .unwrap()
            .replace("\"kb_search\"", "\"bash\"");
        std::fs::write(&tool_job.runtime.host, script).unwrap();
        let outcome = outcome(tool_job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let PiOutcome::Report(report) = outcome else {
            panic!("未知工具应以结构化失败回到模型");
        };
        assert!(report.summary.contains("未知工具"), "{:?}", report.summary);

        // 参数超限：直接构造事件走同一套校验。
        let mut args_job = job(temp.path(), "badargs");
        args_job.task.request = "x".into();
        let error = run_with_tool_args(
            args_job,
            serde_json::json!({"query": "x".repeat(1_200_000)}),
        )
        .unwrap_err();
        assert!(error.contains("参数超过上限"), "{error}");
    }

    /// 把 host 换成一个发指定工具参数、然后结束的脚本。
    fn run_with_tool_args(mut job: PiJob, args: serde_json::Value) -> Result<(), String> {
        let script = format!(
            r##"
import readline from "node:readline";
const rl = readline.createInterface({{ input: process.stdin }});
let init;
const emit = (o) => process.stdout.write(JSON.stringify({{ protocol: 3, requestId: init.requestId, runId: init.runId, ...o }}) + "\n");
rl.on("line", (line) => {{
  const value = JSON.parse(line);
  if (value.type === "init") {{
    init = value;
  emit({{ type: "ready", hostVersion: "0.3.5", nodeVersion: process.versions.node, model: init.model.modelId }});
    emit({{ type: "tool_request", callId: "c1", name: "kb_search", args: {args} }});
  }}
  if (value.type === "tool_result") {{ setTimeout(() => process.exit(0), 10); }}
}});
"##
        );
        let path = job.workspace.root.parent().unwrap().join("args-host.mjs");
        std::fs::write(&path, script).unwrap();
        job.runtime.host = path;
        match run(job, Arc::new(AtomicBool::new(false)), |_| {}) {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[test]
    fn questions_are_paused_answered_and_reported_only() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let job = job(temp.path(), "ask");
        let bridge = job.bridge.clone();
        let (question_tx, question_rx) = mpsc::channel();
        let handle = std::thread::spawn({
            let job = job;
            move || {
                run(job, Arc::new(AtomicBool::new(false)), |event| {
                    if let PiEvent::Questions(batch) = event {
                        let _ = question_tx.send(batch.call_id);
                    }
                })
            }
        });
        let call_id = question_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("应收到提问事件");
        bridge
            .answer(
                &call_id,
                &[PiAnswer {
                    id: 1,
                    question: "想强调哪方面？".into(),
                    value: "责任".into(),
                }],
            )
            .unwrap();
        let outcome = handle.join().unwrap().unwrap();
        let PiOutcome::Report(report) = outcome else {
            panic!("只交清单，不应有正文提案");
        };
        assert_eq!(report.summary, "答完了");
        assert!(report.confirmed.iter().any(|line| line.contains("责任")));
    }

    #[test]
    fn cancelling_during_a_slow_tool_stops_promptly_and_discards_its_late_result() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        struct SlowKb {
            searches: Arc<AtomicUsize>,
        }
        impl super::super::tools::PiKnowledge for SlowKb {
            fn enabled(&self) -> bool {
                true
            }
            fn search(
                &self,
                _query: &str,
                _top: usize,
            ) -> Result<super::super::tools::PiSearchOutcome, String> {
                self.searches.fetch_add(1, Ordering::AcqRel);
                std::thread::sleep(Duration::from_millis(400));
                Ok(super::super::tools::PiSearchOutcome {
                    chunks: vec![super::super::tools::PiChunk {
                        doc_id: 1,
                        title: "防火规定".into(),
                        section: "第一条".into(),
                        text: "各单位应于每年十月开展一次检查。".into(),
                    }],
                    warnings: Vec::new(),
                })
            }
            fn read(&self, _doc_id: i64) -> Result<Option<(String, String)>, String> {
                Ok(None)
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let mut job = job(temp.path(), "tool");
        let searches = Arc::new(AtomicUsize::new(0));
        job.tools = Arc::new(PiToolContext::with_sources(
            Arc::new(crate::models::AppConfig::default()),
            crate::models::DraftInput::default(),
            "# 当前\n".into(),
            "tool".into(),
            Vec::new(),
            Arc::new(SlowKb {
                searches: searches.clone(),
            }),
            Arc::new(NoMs),
        ));
        let tools = job.tools.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let started = Instant::now();
        let error = outcome(job, cancel, |event| {
            if matches!(event, PiEvent::Tool(ref line) if line.contains("执行中")) {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .unwrap_err();
        assert!(error.contains("已停止"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "检索阻塞不能拖住取消"
        );
        // 停止信号在工具线程启动前就已置位：该轮不再发起慢检索，也不留下证据。
        assert_eq!(
            searches.load(Ordering::Acquire),
            0,
            "停止后不应再发起知识库检索"
        );
        assert!(tools.evidence().is_empty(), "停止后不应写证据");
    }

    #[test]
    fn a_report_never_becomes_a_document() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let outcome = outcome(
            job(temp.path(), "report"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let PiOutcome::Report(report) = outcome else {
            panic!("finish_report 只进侧栏");
        };
        assert_eq!(report.items[0].title, "称谓");
        assert_eq!(report.items[0].severity, "建议");
    }

    #[test]
    fn known_delivery_types_are_enforced_but_only_preexisting_receipts_replay() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let mut research_report = job(temp.path(), "report");
        research_report.task.skill = Some("gongwen-research".into());
        let error = outcome(research_report, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("回执的内容不对"), "{error}");

        let temp = tempfile::tempdir().unwrap();
        let mut extract_document = job(temp.path(), "ok");
        extract_document.task.skill = Some("gongwen-extract".into());
        let error =
            outcome(extract_document, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("回执与提交内容不一致"), "{error}");

        // Even a resume task cannot trust a recovered=true host event when the run had no receipt at start.
        let temp = tempfile::tempdir().unwrap();
        let mut forged = job(temp.path(), "forged-report");
        forged.task.skill = Some("gongwen-research".into());
        forged.resume = true;
        let error = outcome(forged, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("回执的内容不对"), "{error}");

        // A valid report receipt already present before resume remains viewable for history compatibility.
        let temp = tempfile::tempdir().unwrap();
        let receipt = serde_json::json!({
            "version": 1,
            "runId": "run-1",
            "requestId": "req-1",
            "kind": "report",
            "path": "",
            "sha256": "",
            "bytes": 0,
            "summary": "旧清单",
            "callId": "old-call",
            "items": [{"title":"旧条目","detail":"历史回执","severity":"提示"}],
            "at": "2026-11-03T00:00:00Z"
        });
        std::fs::write(temp.path().join("receipt.json"), receipt.to_string()).unwrap();
        let mut replay = job(temp.path(), "resume-report");
        replay.task.skill = Some("gongwen-research".into());
        replay.resume = true;
        let outcome = outcome(replay, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let PiOutcome::Report(report) = outcome else {
            panic!("舊清單回執仍可回放");
        };
        assert_eq!(report.summary, "旧清单");
        assert_eq!(report.items[0].title, "旧条目");
    }

    #[test]
    fn authorized_question_is_rejected_without_stopping_the_run() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let job = job(temp.path(), "ask");
        let script = std::fs::read_to_string(&job.runtime.host).unwrap().replace(
            "想强调哪方面\", options: [\"责任\", \"时限\"]",
            "文号用几号\", options: [\"3号\", \"4号\"]",
        );
        std::fs::write(&job.runtime.host, script).unwrap();
        let outcome = outcome(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let PiOutcome::Report(report) = outcome else {
            panic!("拒绝提问后模型应改用清单交付");
        };
        assert!(report.summary.contains("授权要素"), "{}", report.summary);
    }

    #[test]
    fn an_answer_for_a_finished_run_is_rejected() {
        let bridge = PiBridge::default();
        assert!(
            bridge
                .answer(
                    "q1",
                    &[PiAnswer {
                        id: 1,
                        question: "哪方面".into(),
                        value: "x".into()
                    }]
                )
                .is_err()
        );
        let (tx, rx) = mpsc::sync_channel(1);
        bridge.install(tx);
        bridge
            .answer(
                "q1",
                &[PiAnswer {
                    id: 1,
                    question: "哪方面".into(),
                    value: "责任".into(),
                }],
            )
            .unwrap();
        let line = rx.recv().unwrap();
        assert!(line.contains("\"callId\":\"q1\""));
        assert!(line.contains("责任"));
        assert!(
            bridge
                .answer(
                    "",
                    &[PiAnswer {
                        id: 1,
                        question: "q".into(),
                        value: "x".into()
                    }]
                )
                .is_err()
        );
        assert!(bridge.answer("q1", &[]).is_err());
    }

    /// 只为让测试断言「登记过的证据能授权日期」：工具台账在交付里原样带出。
    #[test]
    fn evidence_ledger_is_carried_into_the_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let job = job(temp.path(), "ok");
        let context = job.tools.clone();
        context
            .call("kb_search", &serde_json::json!({"query": "防火"}))
            .unwrap();
        assert!(!context.evidence().is_empty());
        assert!(
            context
                .fact_sources()
                .iter()
                .any(|text| text.contains("每年十月"))
        );
    }
}
