//! 模型调用接口。研究式起草的每一步都经它调模型；测试时换成按脚本回复的假模型。

use super::toolcall::{self, Protocol, Reply, ToolCall, ToolSpec, Turn};
use crate::lmstudio::context::{self, Window, WindowSource};
use crate::lmstudio::{self, ChatOptions, Finish, StreamDelta};
use crate::models::{AppConfig, LmStudioConfig, ModelRefError};
use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

mod usage;
pub(crate) use usage::UsageTotals;

/// 服务端拒收过 `tools`：本次运行之后的自主步骤直接走文本协议，不再白白试一次。
static NATIVE_TOOLS_REJECTED: AtomicBool = AtomicBool::new(false);

/// 这一步该用哪个模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ModelRole {
    /// 起草与补全：要写得好，用起草模型。
    Draft,
    /// 列问题、核对来源、出澄清题：要稳定可复现，温度 0；配了复核模型就用它。
    Assist,
}

/// 一次补全的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Completion {
    pub(crate) content: String,
    /// 撞上输出上限，正文可能不完整。
    pub(crate) truncated: bool,
}

pub(crate) trait ModelBackend {
    fn usage(&self) -> UsageTotals {
        UsageTotals::default()
    }
    /// 跑一次补全，正文与思考的增量都经 `on_delta` 回调。用户点了停止时返回错误。
    fn complete(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion>;

    /// 同 [`complete`](Self::complete)，但要求按 `schema` 输出 JSON（`response_format`）。端点不认时
    /// 退回普通补全，所以调用方**必须**同时能按文本约定解析。默认实现就是普通补全（脚本模型）。
    fn complete_json(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        _schema: (&'static str, Value),
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        self.complete(role, system, user, on_delta)
    }

    /// 用户是否已经点了停止。流程在两步之间查它，不必等下一次模型调用才停。
    fn cancelled(&self) -> bool;

    /// 这个角色的模型上下文窗口，算子装箱时按它分预算（16.15 A）。
    fn window(&self, _role: ModelRole) -> Window {
        Window {
            tokens: context::DEFAULT_WINDOW,
            source: WindowSource::Default,
        }
    }

    /// 自主步骤的一轮对话（`docs/ai-agent-workbench.md` 16.14）。
    ///
    /// 默认实现把对话压成一问一答、按文本协议解析调用——只会 `complete` 的模型（测试里的脚本模型）
    /// 也能跑自主步骤。接真服务的实现覆盖它，优先用原生工具调用。
    fn converse(
        &self,
        role: ModelRole,
        turns: &[Turn],
        tools: &[ToolSpec],
        _protocol: Protocol,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Reply> {
        let (system, user) = toolcall::flatten(turns, tools);
        let completion = self.complete(role, &system, &user, on_delta)?;
        let (calls, content) = toolcall::parse_text_calls(&completion.content);
        Ok(Reply {
            content,
            calls,
            truncated: completion.truncated,
            protocol: Protocol::Text,
        })
    }
}

/// 接本机 / 内网的 OpenAI 兼容接口。
pub(crate) struct LmBackend {
    draft_backup: Result<Option<LmStudioConfig>, ModelRefError>,
    assist_backup: Result<Option<LmStudioConfig>, ModelRefError>,
    notices: Mutex<Vec<String>>,
    usage: Mutex<UsageTotals>,
    draft: Result<LmStudioConfig, ModelRefError>,
    assist: Result<LmStudioConfig, ModelRefError>,
    cancel: Arc<AtomicBool>,
}

impl LmBackend {
    pub(crate) fn new(config: &AppConfig, cancel: Arc<AtomicBool>) -> Self {
        Self {
            draft_backup: config.draft_backup_chat(),
            assist_backup: config.assist_backup_chat(),
            notices: Mutex::new(Vec::new()),
            usage: Mutex::new(UsageTotals::default()),
            draft: config.draft_chat(),
            assist: config.assist_chat(),
            cancel,
        }
    }

    /// 这个角色的接入配置。自动窗口还没问过服务时先问一次（本进程每个模型只问一次），
    /// 之后发请求时的输出上限就按服务报的窗口算。
    fn config(&self, role: ModelRole) -> anyhow::Result<&LmStudioConfig> {
        let config = match role {
            ModelRole::Draft => &self.draft,
            ModelRole::Assist => &self.assist,
        };
        match config {
            Ok(config) => {
                context::window(config);
                Ok(config)
            }
            Err(error) => Err(anyhow::Error::new(error.clone())),
        }
    }
}

impl LmBackend {
    pub(crate) fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }

    /// 仅传输失败或 5xx 可以换模型。4xx、超长、解析失败与空正文都保留原错。
    fn can_fallback(&self, error: &anyhow::Error, started: bool) -> bool {
        !started
            && !self.cancelled()
            && error.downcast_ref::<lmstudio::OutputStarted>().is_none()
            && (error
                .downcast_ref::<lmstudio::HttpFailure>()
                .is_some_and(|e| e.0.is_server_error())
                || error
                    .downcast_ref::<reqwest::Error>()
                    .is_some_and(|e| e.is_connect() || e.is_timeout() || e.is_body()))
    }

    /// 一次发送的记录包含失败与停止时已收到的正文、思考；工具参数另由结果补入。
    fn tracked<T>(
        &self,
        role: ModelRole,
        config: &LmStudioConfig,
        input: usize,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
        mut send: impl FnMut(
            &LmStudioConfig,
            &mut dyn FnMut(StreamDelta<'_>),
        ) -> anyhow::Result<(T, Option<lmstudio::Usage>, String)>,
    ) -> anyhow::Result<T> {
        let mut started_output = false;
        let primary = self.tracked_once(
            role,
            config,
            input,
            &mut |delta| {
                started_output = true;
                on_delta(delta);
            },
            &mut send,
        );
        let error = match primary {
            Ok(value) => return Ok(value),
            Err(error) if self.can_fallback(&error, started_output) => error,
            Err(error) => return Err(error),
        };
        let backup = match role {
            ModelRole::Draft => &self.draft_backup,
            ModelRole::Assist => &self.assist_backup,
        };
        let backup = match backup {
            Ok(Some(config)) => config,
            Ok(None) => return Err(error),
            Err(why) => return Err(error.context(format!("备用模型配置不可用：{why}"))),
        };
        context::window(backup);
        self.notices.lock().unwrap().push(format!(
            "主模型 {} 不可用（{error:#}），本次改用 {}",
            config.model, backup.model
        ));
        self.tracked_once(role, backup, input, on_delta, &mut send)
            .map_err(|why| error.context(format!("备用模型 {} 也失败：{why:#}", backup.model)))
    }

    fn tracked_once<T>(
        &self,
        role: ModelRole,
        config: &LmStudioConfig,
        input: usize,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
        send: &mut impl FnMut(
            &LmStudioConfig,
            &mut dyn FnMut(StreamDelta<'_>),
        ) -> anyhow::Result<(T, Option<lmstudio::Usage>, String)>,
    ) -> anyhow::Result<T> {
        let started = Instant::now();
        let mut output = String::new();
        let result = send(config, &mut |delta| {
            let (StreamDelta::Content(text) | StreamDelta::Reasoning(text)) = delta;
            output.push_str(text);
            on_delta(delta);
        });
        let usage = result.as_ref().ok().and_then(|(_, usage, _)| *usage);
        if let Ok((_, _, extra)) = &result {
            output.push_str(extra);
        }
        self.usage.lock().unwrap().record(
            role,
            &config.model,
            input,
            &output,
            usage,
            started.elapsed().as_millis() as u64,
        );
        result.map(|(value, _, _)| value)
    }

    fn complete_with(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        options: ChatOptions,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        let config = self.config(role)?;
        let mut attempt = 0;
        let outcome = loop {
            attempt += 1;
            match self.tracked(
                role,
                config,
                context::estimate_tokens(system) + context::estimate_tokens(user),
                on_delta,
                |config, delta| {
                    lmstudio::generate_stream(
                        config,
                        system,
                        user,
                        config.temperature,
                        config.max_tokens,
                        options.clone(),
                        &self.cancel,
                        delta,
                    )
                    .map(|outcome| {
                        let usage = outcome.usage;
                        (outcome, usage, String::new())
                    })
                },
            ) {
                // 思考型模型（实测 MiniMax-M2.7）偶尔在思考里把话说完、正文留空：再问一次。
                Err(error)
                    if attempt == 1
                        && error.downcast_ref::<lmstudio::EmptyContent>().is_some()
                        && !self.cancelled() =>
                {
                    continue;
                }
                other => break other?,
            }
        };
        if outcome.finish == Finish::Cancelled {
            anyhow::bail!("已停止生成");
        }
        Ok(Completion {
            content: outcome.content,
            truncated: outcome.finish == Finish::Length,
        })
    }
}

impl ModelBackend for LmBackend {
    fn usage(&self) -> UsageTotals {
        self.usage.lock().unwrap().clone()
    }
    fn complete(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        self.complete_with(role, system, user, ChatOptions::default(), on_delta)
    }

    fn complete_json(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        schema: (&'static str, Value),
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        let options = ChatOptions {
            json_schema: Some(schema),
            ..ChatOptions::default()
        };
        self.complete_with(role, system, user, options, on_delta)
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn window(&self, role: ModelRole) -> Window {
        let config = match role {
            ModelRole::Draft => &self.draft,
            ModelRole::Assist => &self.assist,
        };
        config.as_ref().map(context::window).unwrap_or(Window {
            tokens: context::DEFAULT_WINDOW,
            source: WindowSource::Default,
        })
    }

    /// 原生模式发 `tools`，被 4xx 拒收就改文本模式重发一次；回复里没有 `tool_calls` 而正文里写着
    /// `<tool_call>` 的（服务端没开工具解析），同样认出来并转文本模式。
    fn converse(
        &self,
        role: ModelRole,
        turns: &[Turn],
        tools: &[ToolSpec],
        protocol: Protocol,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Reply> {
        let config = self.config(role)?;
        let native = protocol == Protocol::Native && !NATIVE_TOOLS_REJECTED.load(Ordering::Relaxed);
        if native {
            let specs: Vec<Value> = tools.iter().map(ToolSpec::to_native).collect();
            match self.tracked(
                role,
                config,
                context::estimate_tokens(
                    &serde_json::json!(toolcall::native_messages(turns)).to_string(),
                ) + context::estimate_tokens(&serde_json::json!(specs).to_string()),
                on_delta,
                |config, delta| {
                    lmstudio::converse_stream(
                        config,
                        &toolcall::native_messages(turns),
                        Some(&specs),
                        config.temperature,
                        config.max_tokens,
                        &self.cancel,
                        delta,
                    )
                    .map(|outcome| {
                        let usage = outcome.usage;
                        let extra = outcome
                            .tool_calls
                            .iter()
                            .map(|c| format!("{} {}", c.name, c.arguments))
                            .collect::<Vec<_>>()
                            .join("\n");
                        (outcome, usage, extra)
                    })
                    .map_err(anyhow::Error::from)
                },
            ) {
                Ok(outcome) => {
                    if outcome.finish == Finish::Cancelled {
                        anyhow::bail!("已停止生成");
                    }
                    let truncated = outcome.finish == Finish::Length;
                    if outcome.tool_calls.is_empty() {
                        let (calls, content) = toolcall::parse_text_calls(&outcome.content);
                        let protocol = if calls.is_empty() {
                            Protocol::Native
                        } else {
                            Protocol::Text
                        };
                        return Ok(Reply {
                            content,
                            calls,
                            truncated,
                            protocol,
                        });
                    }
                    let calls = outcome
                        .tool_calls
                        .into_iter()
                        .map(|call| ToolCall {
                            arguments: serde_json::from_str(&call.arguments)
                                .unwrap_or_else(|_| serde_json::json!({})),
                            id: call.id,
                            name: call.name,
                        })
                        .collect();
                    return Ok(Reply {
                        content: outcome.content,
                        calls,
                        truncated,
                        protocol: Protocol::Native,
                    });
                }
                Err(error)
                    if error
                        .downcast_ref::<lmstudio::HttpFailure>()
                        .is_some_and(|e| e.0.is_client_error()) =>
                {
                    NATIVE_TOOLS_REJECTED.store(true, Ordering::Relaxed);
                }
                Err(error) => return Err(error),
            }
        }
        let outcome = self.tracked(
            role,
            config,
            context::estimate_tokens(
                &serde_json::json!(toolcall::text_messages(turns, tools)).to_string(),
            ),
            on_delta,
            |config, delta| {
                lmstudio::converse_stream(
                    config,
                    &toolcall::text_messages(turns, tools),
                    None,
                    config.temperature,
                    config.max_tokens,
                    &self.cancel,
                    delta,
                )
                .map(|outcome| {
                    let usage = outcome.usage;
                    (outcome, usage, String::new())
                })
                .map_err(anyhow::Error::from)
            },
        )?;
        if outcome.finish == Finish::Cancelled {
            anyhow::bail!("已停止生成");
        }
        let (calls, content) = toolcall::parse_text_calls(&outcome.content);
        Ok(Reply {
            content,
            calls,
            truncated: outcome.finish == Finish::Length,
            protocol: Protocol::Text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend_for(url: &str, backup: Option<&str>) -> LmBackend {
        let mut config = AppConfig::default();
        config.lm_studio.base_url = url.into();
        config.lm_studio.model = "主".into();
        config.lm_studio.context_window = 32000;
        let mut backend = LmBackend::new(&config, Arc::new(AtomicBool::new(false)));
        backend.draft_backup = Ok(backup.map(|url| LmStudioConfig {
            base_url: url.into(),
            model: "备用".into(),
            context_window: 32000,
            ..config.lm_studio.clone()
        }));
        backend
    }

    fn answer(text: &str) -> String {
        serde_json::json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}}).to_string()
    }

    #[test]
    fn disconnected_primary_uses_backup_and_records_actual_model() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let primary = format!("http://{}/v1", port.local_addr().unwrap());
        drop(port);
        let (backup, server) = serve(vec![("200 OK", "application/json", answer("备用工作稿"))]);
        let backend = backend_for(&primary, Some(&backup));
        let result = backend
            .complete_json(
                ModelRole::Draft,
                "s",
                "u",
                ("reply", serde_json::json!({"type":"object"})),
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(result.content, "备用工作稿");
        server.join().unwrap();
        assert!(backend.take_notices()[0].contains("本次改用 备用"));
        let usage = backend.usage();
        assert_eq!(usage.calls.len(), 2);
        assert_eq!(usage.draft_models(), ["备用"]);
        assert_eq!(usage.calls[1].tokens.total_tokens, 13);
    }

    #[test]
    fn server_failure_switches_only_this_call_and_next_call_retries_primary() {
        let (primary, main_server) = serve(vec![
            ("503 Service Unavailable", "application/json", "{}".into()),
            ("200 OK", "application/json", answer("主稿")),
        ]);
        let (backup, backup_server) = serve(vec![("200 OK", "application/json", answer("备用稿"))]);
        let backend = backend_for(&primary, Some(&backup));
        assert_eq!(
            backend
                .complete(ModelRole::Draft, "s", "u", &mut |_| {})
                .unwrap()
                .content,
            "备用稿"
        );
        assert_eq!(
            backend
                .complete(ModelRole::Draft, "s", "u", &mut |_| {})
                .unwrap()
                .content,
            "主稿"
        );
        main_server.join().unwrap();
        backup_server.join().unwrap();
        assert_eq!(backend.usage().draft_models(), ["备用", "主"]);
    }

    #[test]
    fn client_errors_and_partial_content_or_reasoning_never_use_backup() {
        for body in [None, Some("content"), Some("reasoning_content")] {
            let standby = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            standby.set_nonblocking(true).unwrap();
            let backup = format!("http://{}/v1", standby.local_addr().unwrap());
            let responses = if let Some(channel) = body {
                vec![(
                    "200 OK",
                    "text/event-stream",
                    format!("data: {{\"choices\":[{{\"delta\":{{\"{channel}\":\"开头\"}}}}]}}\n\n"),
                )]
            } else {
                vec![
                    ("400 Bad Request", "application/json", "{}".into()),
                    ("400 Bad Request", "application/json", "{}".into()),
                ]
            };
            let (primary, server) = serve(responses);
            let backend = backend_for(&primary, Some(&backup));
            let error = backend
                .complete(ModelRole::Draft, "s", "u", &mut |_| {})
                .unwrap_err();
            assert!(format!("{error:#}").contains(if body.is_some() {
                "断开连接"
            } else {
                "400"
            }));
            server.join().unwrap();
            assert!(standby.accept().is_err(), "备用未收到请求");
            assert!(backend.take_notices().is_empty());
        }
    }

    #[test]
    fn no_backup_and_failed_backup_preserve_primary_error() {
        for backup in [None, Some("http://127.0.0.1:1/v1")] {
            let (primary, server) = serve(vec![(
                "503 Service Unavailable",
                "application/json",
                "primary unavailable".into(),
            )]);
            let backend = backend_for(&primary, backup);
            let error = backend
                .complete(ModelRole::Draft, "s", "u", &mut |_| {})
                .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("503"));
            assert_eq!(message.contains("也失败"), backup.is_some());
            server.join().unwrap();
        }
    }

    #[test]
    fn context_overflow_and_cancellation_do_not_use_backup() {
        let backend = backend_for("http://127.0.0.1:1/v1", Some("http://127.0.0.1:2/v1"));
        let overflow = context::local_overflow(backend.window(ModelRole::Draft), 999999);
        assert!(!backend.can_fallback(&anyhow::Error::new(overflow), false));
        backend.cancel.store(true, Ordering::Relaxed);
        let error = anyhow::Error::new(lmstudio::HttpFailure(
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "停机".into(),
        ));
        assert!(!backend.can_fallback(&error, false));
    }

    #[test]
    fn converse_assist_uses_its_backup_and_accumulates_usage() {
        let (primary, main_server) = serve(vec![(
            "503 Service Unavailable",
            "application/json",
            "{}".into(),
        )]);
        let (backup, backup_server) =
            serve(vec![("200 OK", "application/json", answer("复核答复"))]);
        let mut backend = backend_for(&primary, Some(&backup));
        backend.assist_backup = backend.draft_backup.clone();
        let reply = backend
            .converse(
                ModelRole::Assist,
                &[Turn::User("复核".into())],
                &[],
                Protocol::Text,
                &mut |_| {},
            )
            .unwrap();
        assert_eq!(reply.content, "复核答复");
        assert_eq!(backend.usage().calls.len(), 2);
        assert!(
            backend
                .usage()
                .calls
                .iter()
                .all(|call| call.role == ModelRole::Assist)
        );
        main_server.join().unwrap();
        backup_server.join().unwrap();
    }

    #[test]
    fn assist_falls_back_to_the_draft_model_with_zero_temperature() {
        let mut config = AppConfig::default();
        config.lm_studio.model = "draft".into();
        config.lm_studio.max_tokens = 32000;
        config.lm_studio.context_window = 1_048_576;
        let assist = config.assist_chat().unwrap();
        assert_eq!(assist.model, "draft");
        assert_eq!(assist.temperature, 0.0);
        assert_eq!(assist.max_tokens, 32000);
        assert_eq!(assist.context_window, 1_048_576);

        config.revise_model.enabled = true;
        assert_eq!(config.assist_chat().unwrap().context_window, 1_048_576);
        config.revise_model.context_window = 262_144;
        assert_eq!(config.assist_chat().unwrap().context_window, 262_144);

        config.revise_model.model = "small".into();
        let assist = config.assist_chat().unwrap();
        assert_eq!(assist.model, "small");
        assert_eq!(assist.context_window, 262_144, "独立模型用自己的窗口");
        config.revise_model.context_window = 0;
        assert_eq!(config.assist_chat().unwrap().context_window, 0);
        assert_eq!(assist.max_tokens, 32000, "不用复核那套 512 的小上限");
    }

    /// 本机假服务：依次接 `responses.len()` 个请求，按顺序回话，返回收到的请求体。
    fn serve(
        responses: Vec<(&'static str, &'static str, String)>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            let mut responses = responses.into_iter();
            while let Some((status, content_type, body)) = responses.next() {
                let (mut stream, _) = listener.accept().unwrap();
                let mut peek = [0u8; 4];
                let mut n = 0;
                while n < 4 {
                    n = stream.peek(&mut peek).unwrap();
                }
                if &peek[..n] == b"GET " {
                    // 自动窗口先问 `/v1/models`：回 404，这次不算脚本里的一问。
                    stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .unwrap();
                    responses = std::iter::once((status, content_type, body))
                        .chain(responses)
                        .collect::<Vec<_>>()
                        .into_iter();
                    continue;
                }
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let n = stream.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + length {
                            bodies.push(String::from_utf8_lossy(&buf[end + 4..]).to_string());
                            break;
                        }
                    }
                }
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
            }
            bodies
        });
        (url, handle)
    }

    /// 原生工具调用照收；服务端拒收 `tools` 时改文本协议重发，之后一直用文本协议。
    /// （「拒收过」是进程级的开关，两段放在一个测试里按顺序验。）
    #[test]
    fn native_tool_calls_fall_back_to_the_text_protocol_when_rejected() {
        let tools = [ToolSpec {
            name: "kb_search".into(),
            description: "检索".into(),
            params: vec![("query".into(), true, "检索词".into())],
            schema: None,
        }];
        let turns = [Turn::System("S".into()), Turn::User("查防火".into())];
        let backend = |url: &str| {
            let mut config = AppConfig::default();
            config.lm_studio.base_url = url.into();
            config.lm_studio.model = "m".into();
            LmBackend::new(&config, Arc::new(AtomicBool::new(false)))
        };

        let native = serde_json::json!({"choices": [{"message": {"content": "", "tool_calls": [
            {"id": "x1", "type": "function", "function": {"name": "kb_search", "arguments": "{\"query\":\"防火\"}"}}
        ]}, "finish_reason": "tool_calls"}]})
        .to_string();
        let (url, server) = serve(vec![("200 OK", "application/json", native)]);
        let reply = backend(&url)
            .converse(
                ModelRole::Draft,
                &turns,
                &tools,
                Protocol::Native,
                &mut |_| {},
            )
            .unwrap();
        let bodies = server.join().unwrap();
        assert!(bodies[0].contains("\"tools\""));
        assert_eq!(reply.protocol, Protocol::Native);
        assert_eq!(reply.calls[0].id, "x1");
        assert_eq!(reply.calls[0].arguments["query"], "防火");

        let text = serde_json::json!({"choices": [{"message": {
            "content": "<tool_call>{\"name\": \"kb_search\", \"arguments\": {\"query\": \"防火期\"}}</tool_call>"
        }, "finish_reason": "stop"}]})
        .to_string();
        let (url, server) = serve(vec![
            (
                "400 Bad Request",
                "application/json",
                r#"{"error": "tools not supported"}"#.into(),
            ),
            ("200 OK", "application/json", text),
        ]);
        let reply = backend(&url)
            .converse(
                ModelRole::Draft,
                &turns,
                &tools,
                Protocol::Native,
                &mut |_| {},
            )
            .unwrap();
        let bodies = server.join().unwrap();
        assert!(bodies[0].contains("\"tools\""));
        assert!(!bodies[1].contains("\"tools\""), "重发不带 tools");
        assert!(bodies[1].contains("【可用工具】"), "工具说明写进了系统提示");
        assert_eq!(reply.protocol, Protocol::Text);
        assert_eq!(reply.calls[0].arguments["query"], "防火期");
    }
}
