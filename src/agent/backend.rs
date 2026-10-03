//! 模型调用接口。研究式起草的每一步都经它调模型；测试时换成按脚本回复的假模型。

use super::toolcall::{self, Protocol, Reply, ToolCall, ToolSpec, Turn};
use crate::lmstudio::{self, ChatOptions, ConverseError, Finish, StreamDelta};
use crate::models::{AppConfig, LmStudioConfig};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 服务端拒收过 `tools`：本次运行之后的自主步骤直接走文本协议，不再白白试一次。
static NATIVE_TOOLS_REJECTED: AtomicBool = AtomicBool::new(false);

/// 这一步该用哪个模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// 跑一次补全，正文与思考的增量都经 `on_delta` 回调。用户点了停止时返回错误。
    fn complete(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion>;

    /// 用户是否已经点了停止。流程在两步之间查它，不必等下一次模型调用才停。
    fn cancelled(&self) -> bool;

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
    draft: LmStudioConfig,
    assist: LmStudioConfig,
    cancel: Arc<AtomicBool>,
}

impl LmBackend {
    pub(crate) fn new(config: &AppConfig, cancel: Arc<AtomicBool>) -> Self {
        Self {
            draft: config.lm_studio.clone(),
            assist: assist_config(config),
            cancel,
        }
    }
}

/// 辅助步骤用的接入配置：配了复核模型就用它，否则沿用起草模型；温度固定 0。
///
/// 输出上限沿用起草模型的设置，不用复核那套按句长算的小上限：思考型模型的推理也算在
/// 上限里，给小了正文出不来。
pub(crate) fn assist_config(config: &AppConfig) -> LmStudioConfig {
    let revise = &config.revise_model;
    let mut assist = if revise.enabled && !revise.model.trim().is_empty() {
        revise.resolve(&config.lm_studio)
    } else {
        config.lm_studio.clone()
    };
    assist.temperature = 0.0;
    assist.max_tokens = config.lm_studio.max_tokens;
    assist
}

impl ModelBackend for LmBackend {
    fn complete(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        let config = match role {
            ModelRole::Draft => &self.draft,
            ModelRole::Assist => &self.assist,
        };
        let outcome = lmstudio::generate_stream(
            config,
            system,
            user,
            config.temperature,
            config.max_tokens,
            ChatOptions::default(),
            &self.cancel,
            on_delta,
        )?;
        if outcome.finish == Finish::Cancelled {
            anyhow::bail!("已停止生成");
        }
        Ok(Completion {
            content: outcome.content,
            truncated: outcome.finish == Finish::Length,
        })
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
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
        let config = match role {
            ModelRole::Draft => &self.draft,
            ModelRole::Assist => &self.assist,
        };
        let native = protocol == Protocol::Native && !NATIVE_TOOLS_REJECTED.load(Ordering::Relaxed);
        if native {
            let specs: Vec<Value> = tools.iter().map(ToolSpec::to_native).collect();
            match lmstudio::converse_stream(
                config,
                &toolcall::native_messages(turns),
                Some(&specs),
                config.temperature,
                config.max_tokens,
                &self.cancel,
                on_delta,
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
                Err(ConverseError::Rejected(_)) => {
                    NATIVE_TOOLS_REJECTED.store(true, Ordering::Relaxed);
                }
                Err(ConverseError::Other(error)) => return Err(error),
            }
        }
        let outcome = lmstudio::converse_stream(
            config,
            &toolcall::text_messages(turns, tools),
            None,
            config.temperature,
            config.max_tokens,
            &self.cancel,
            on_delta,
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

    #[test]
    fn assist_falls_back_to_the_draft_model_with_zero_temperature() {
        let mut config = AppConfig::default();
        config.lm_studio.model = "draft".into();
        config.lm_studio.max_tokens = 32000;
        let assist = assist_config(&config);
        assert_eq!(assist.model, "draft");
        assert_eq!(assist.temperature, 0.0);
        assert_eq!(assist.max_tokens, 32000);

        config.revise_model.enabled = true;
        config.revise_model.model = "small".into();
        let assist = assist_config(&config);
        assert_eq!(assist.model, "small");
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
            for (status, content_type, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
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
