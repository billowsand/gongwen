//! 后台进程：启动 Node host、双向 JSONL、取消与回收。
//!
//! UI 线程只收事件；这里从不阻塞界面。stdin 只写协议，密钥不出现在 argv，也不
//! 出现在错误文本里。stdout 的 EOF、进程退出、协议错、取消和超时都按失败返回，
//! 不会把最后一段聊天或半截文件当成交付。

use super::PiRuntime;
use super::PiWorkspace;
use super::delivery::{safe_work_path, sha256_hex, verify_submission};
use super::protocol::{
    HostEvent, InitLimits, InitMessage, InitModel, InitTask, InitWorkspace, MAX_CANDIDATE_BYTES,
    MAX_DELIVERY_BYTES, MAX_EVENTS, MAX_HOST_LINE_BYTES, MAX_MESSAGE_CHARS, PROTOCOL_VERSION,
};
use crate::pi_agent::runtime::hidden_command;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 交给 host 的模型参数。`api_key` 只进 stdin，不进 argv / 日志。
pub(crate) struct PiModel {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model_id: String,
    pub(crate) context_window: u32,
    pub(crate) max_tokens: u32,
    pub(crate) reasoning: bool,
    pub(crate) thinking_level: Option<String>,
}

pub(crate) struct PiTask {
    pub(crate) request: String,
    pub(crate) kind: String,
    pub(crate) system_date: String,
}

pub(crate) struct PiJob {
    pub(crate) runtime: PiRuntime,
    pub(crate) workspace: PiWorkspace,
    pub(crate) model: PiModel,
    pub(crate) task: PiTask,
    /// 发起本轮时的正文快照；以后接受提案时还要用它保护过期提案。
    pub(crate) before: String,
    pub(crate) request_id: String,
    pub(crate) run_id: String,
    pub(crate) timeout: Duration,
}

#[derive(Debug)]
pub(crate) enum PiEvent {
    Progress(String),
    Tool(String),
    Candidate { path: String, content: String },
}

/// 成功交付的固定版本。
#[derive(Debug)]
pub(crate) struct PiDelivery {
    pub(crate) before: String,
    pub(crate) request: String,
    pub(crate) content: String,
    pub(crate) path: String,
    pub(crate) summary: String,
    pub(crate) sha256: String,
    pub(crate) bytes: usize,
    pub(crate) workspace_label: String,
    pub(crate) elapsed: Duration,
}

struct Submission {
    path: String,
    summary: String,
    content: String,
    sha256: String,
    bytes: usize,
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

/// 跑一轮 host，直到提交成功或失败。`on_event` 在调用线程里同步执行，界面侧只
/// 投消息，不在这里做重活。
pub(crate) fn run(
    job: PiJob,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(PiEvent),
) -> Result<PiDelivery, String> {
    let started = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止 Pi 智能写作，未交付任何提案。".into());
    }
    if job.model.model_id.trim().is_empty() {
        return Err("还没有选择智能写作模型：请到「AI 管理 → 模型服务」选择。".into());
    }
    if job.task.request.len() > 200_000 {
        return Err("本轮要求太长（超过 200000 字节），请拆成几轮再写。".into());
    }
    let mut command = hidden_command(&job.runtime.node);
    command
        .arg("--disable-warning=ExperimentalWarning")
        .arg(&job.runtime.host)
        .current_dir(&job.workspace.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
            while let Ok(buffer) = bounded_line(&mut reader, 32 * 1024) {
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
        task: InitTask {
            request: job.task.request.clone(),
            kind: job.task.kind.clone(),
            system_date: job.task.system_date.clone(),
        },
        limits: InitLimits {
            max_file_bytes: MAX_DELIVERY_BYTES as u64,
            max_candidate_bytes: MAX_CANDIDATE_BYTES as u64,
            timeout_seconds: job.timeout.as_secs().max(1),
        },
    };
    let mut payload =
        serde_json::to_vec(&init).map_err(|error| format!("组装 Pi init 消息失败：{error}"))?;
    payload.push(b'\n');
    // stdin 也可能被不读输入的 host 堵住；写线程只持管道，主循环仍能超时并回收进程。
    let (control_tx, control_rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        if stdin.write_all(&payload).is_err() || stdin.flush().is_err() {
            let _ = stdin_errors.send(StdoutMsg::Protocol("Pi host 没有接收初始配置".into()));
            return;
        }
        while let Ok(()) = control_rx.recv() {
            if stdin.write_all(b"{\"type\":\"cancel\"}\n").is_err() || stdin.flush().is_err() {
                break;
            }
        }
    });

    let mut submitted: Option<Submission> = None;
    let mut ready = false;
    let mut events = 0usize;
    let mut cancel_sent: Option<Instant> = None;
    loop {
        if cancel.load(Ordering::Relaxed) && cancel_sent.is_none() {
            let _ = control_tx.try_send(());
            cancel_sent = Some(Instant::now());
        }
        if let Some(at) = cancel_sent
            && at.elapsed() > Duration::from_secs(2)
        {
            return Err("已停止 Pi 智能写作，未交付任何提案。".into());
        }
        if started.elapsed() > job.timeout {
            let _ = control_tx.try_send(());
            return Err(format!(
                "Pi 智能写作超时（{} 秒），未交付任何提案。",
                job.timeout.as_secs()
            ));
        }
        if !ready && cancel_sent.is_none() && started.elapsed() > Duration::from_secs(30) {
            return Err("Pi host 启动后 30 秒内没有就绪，已停止。".into());
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
                on_event(PiEvent::Progress(format!(
                    "Pi host 已就绪（host {} / Node {} / 模型 {}）",
                    host_version.as_deref().unwrap_or("?"),
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
                on_event(PiEvent::Candidate { path, content });
            }
            HostEvent::Submitted {
                path,
                summary,
                content,
                sha256,
                bytes,
                ..
            } => {
                if !ready {
                    return Err("Pi host 还没有报告就绪就送来了成品，已拒绝。".into());
                }
                verify_submission(&path, bytes, &sha256, &content)?;
                if content.len() > MAX_DELIVERY_BYTES {
                    return Err("Pi host 提交的 Markdown 超过上限".into());
                }
                on_event(PiEvent::Progress(format!("已提交 {path}，准备交付检查…")));
                submitted = Some(Submission {
                    path,
                    summary: short(&summary),
                    content,
                    sha256,
                    bytes,
                });
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
                    verify_submission(
                        &submission.path,
                        submission.bytes,
                        &submission.sha256,
                        &submission.content,
                    )?;
                    // 完成事件之后仍须确认进程正常退出，不能接受“先报成功再崩溃”。
                    successful_exit(&mut guard, &cancel)?;
                    return Ok(PiDelivery {
                        before: job.before,
                        request: job.task.request,
                        content: submission.content,
                        path: submission.path,
                        summary: submission.summary,
                        sha256: submission.sha256,
                        bytes: submission.bytes,
                        workspace_label: job.workspace.label,
                        elapsed: started.elapsed(),
                    });
                }
                if submitted.is_some() {
                    return Err("Pi host 前后矛盾：送来了成品却报告未提交".into());
                }
                return Err(
                    "模型结束了本轮但没有提交成品（未调用 submit_document），未交付任何提案。"
                        .into(),
                );
            }
            HostEvent::Cancelled { message, .. } => {
                let message = if message.trim().is_empty() {
                    "已停止 Pi 智能写作，未交付任何提案。".to_string()
                } else {
                    short(&message)
                };
                return Err(message);
            }
            HostEvent::Error { code, message, .. } => {
                let code = code.unwrap_or_else(|| "host_error".into());
                return Err(host_failure(
                    format!(
                        "Pi host 失败（{code}）：{}",
                        scrub(&short(&message), &job.model.api_key)
                    ),
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
            None if started.elapsed() >= Duration::from_secs(2) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// 真实 Node 小夹具：只验证协议、进程回收与交付判定；不访问任何模型端点。
    const FIXTURE: &str = r##"
import { createHash } from "node:crypto";
import readline from "node:readline";
const rl = readline.createInterface({ input: process.stdin });
let handled = false;
rl.on("line", (line) => {
  if (!handled) {
    handled = true;
    const init = JSON.parse(line);
    const base = { protocol: 1, requestId: init.requestId, runId: init.runId };
    const emit = (o) => process.stdout.write(JSON.stringify({ ...base, ...o }) + "\n");
    emit({ type: "ready", hostVersion: "test", nodeVersion: process.versions.node, model: init.model.modelId });
    const mode = init.task.request;
    const content = "# 成品\n\n正文。\n";
    const sha256 = createHash("sha256").update(content, "utf8").digest("hex");
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
    if (mode === "hang") {
      return;
    }
    emit({ type: "candidate", path: "work/final.md", content, sha256, bytes: Buffer.byteLength(content) });
    emit({ type: "submitted", path: "work/final.md", summary: "夹具", content, sha256, bytes: Buffer.byteLength(content) });
    emit({ type: "completed", submitted: true });
    setTimeout(() => process.exit(mode === "badexit" ? 7 : 0), 20);
  } else {
    try {
      if (JSON.parse(line).type === "cancel") process.exit(3);
    } catch {}
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

    fn fixture(dir: &Path) -> PathBuf {
        let path = dir.join("host-fixture.mjs");
        std::fs::write(&path, FIXTURE).unwrap();
        path
    }

    fn job(dir: &Path, mode: &str) -> PiJob {
        let root = dir.join("workspace");
        std::fs::create_dir_all(root.join("input")).unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("input/current.md"), "# 当前\n").unwrap();
        std::fs::write(root.join("input/request.md"), mode).unwrap();
        PiJob {
            runtime: PiRuntime {
                node: PathBuf::from("node"),
                host: fixture(dir),
            },
            workspace: PiWorkspace {
                root,
                state: dir.join("session.sqlite"),
                label: "test/workspace".into(),
            },
            model: PiModel {
                base_url: "http://127.0.0.1:9/v1".into(),
                api_key: "sk-test-secret-1234".into(),
                model_id: "fixture".into(),
                context_window: 8192,
                max_tokens: 1024,
                reasoning: false,
                thinking_level: None,
            },
            task: PiTask {
                request: mode.into(),
                kind: "通知".into(),
                system_date: "2026-11-01".into(),
            },
            before: "# 当前\n".into(),
            request_id: "req-1".into(),
            run_id: "run-1".into(),
            timeout: Duration::from_secs(10),
        }
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
        let delivery = run(
            job(temp.path(), "ok"),
            Arc::new(AtomicBool::new(false)),
            |event| {
                sink.lock().unwrap().push(format!("{event:?}"));
            },
        )
        .unwrap();
        assert_eq!(delivery.content, "# 成品\n\n正文。\n");
        assert_eq!(delivery.path, "work/final.md");
        assert!(delivery.bytes > 0);
        let events = events.lock().unwrap();
        assert!(events.iter().any(|line| line.contains("Candidate")));
        assert!(events.iter().any(|line| line.contains("已提交")));
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
        let error = run(stalled, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("超时"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn completed_event_cannot_hide_a_failed_exit() {
        if !node_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let error = run(
            job(temp.path(), "badexit"),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap_err();
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
        let error = run(job(temp.path(), "ok"), cancel, |event| {
            if matches!(event, PiEvent::Progress(ref message) if message.starts_with("已提交 "))
            {
                flag.store(true, Ordering::Relaxed);
            }
        })
        .unwrap_err();
        assert!(error.contains("已停止"), "{error}");
    }

    #[test]
    fn a_run_without_submit_is_a_failure() {
        if !node_available() {
            eprintln!("跳过：本机没有 Node");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let error = run(
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
        let error = run(
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
        let error = run(
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
        let handle = std::thread::spawn(move || run(job(temp.path(), "hang"), cancel, |_| {}));
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
        let error = run(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
        assert!(error.contains("提前退出"), "{error}");
    }
}
