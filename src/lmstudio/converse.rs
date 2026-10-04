//! 多轮对话 + 工具调用的流式补全（自主步骤用，`docs/ai-agent-workbench.md` 16.14）。
//!
//! 与 [`super::generate_stream`] 同一套 SSE 解析与 `<think>` 切分，多了两样：请求带整段消息
//! 与可选的 `tools`；回复里的 `tool_calls` 按 `index` 把分片拼起来（名字在第一片，参数分好几片）。
//! 服务端用 4xx 拒收时返回 [`ConverseError::Rejected`]，调用方据此改走文本协议。

use super::stream::{SseLine, ThinkSplitter, parse_sse_line, stream_error};
use super::{Finish, StreamDelta};
use crate::models::LmStudioConfig;
use anyhow::{Context, anyhow};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};

/// 非流式回复：只取用得上的几层，其余按原样 JSON 读。
#[derive(serde::Deserialize)]
struct ChatResponseRaw {
    choices: Vec<Value>,
}

/// 回复里的一个工具调用。`arguments` 是原样的 JSON 文本。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WireToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConverseOutcome {
    /// 正文（已剔除 `<think>` 段）。
    pub content: String,
    pub tool_calls: Vec<WireToolCall>,
    pub finish: Finish,
}

#[derive(Debug)]
pub enum ConverseError {
    /// 服务端嫌请求不合法（多半是不认 `tools`）。
    Rejected(anyhow::Error),
    Other(anyhow::Error),
}

impl From<ConverseError> for anyhow::Error {
    fn from(error: ConverseError) -> Self {
        match error {
            ConverseError::Rejected(error) | ConverseError::Other(error) => error,
        }
    }
}

/// 跑一轮对话。`tools` 为 None 时不带工具（文本协议）。
///
/// 输出上限按上下文窗口现算；服务端报超长时按它说的上限重算、重发一次（同
/// [`super::generate_stream`]）。超长不算 [`ConverseError::Rejected`]，免得被当成「不认 tools」。
pub fn converse_stream(
    config: &LmStudioConfig,
    messages: &[Value],
    tools: Option<&[Value]>,
    temperature: f32,
    max_tokens: u32,
    cancel: &AtomicBool,
    on_delta: &mut dyn FnMut(StreamDelta<'_>),
) -> Result<ConverseOutcome, ConverseError> {
    let input = super::context::estimate_tokens(&Value::from(messages.to_vec()).to_string())
        + tools.map_or(0, |tools| {
            super::context::estimate_tokens(&Value::from(tools.to_vec()).to_string())
        });
    let window = super::context::peek_window(config);
    let Some(limit) = super::context::output_limit(window.tokens, input, max_tokens) else {
        let overflow = super::context::local_overflow(window, input);
        return Err(ConverseError::Other(anyhow::Error::new(overflow)));
    };
    match converse_once(
        config,
        messages,
        tools,
        temperature,
        limit,
        cancel,
        on_delta,
    ) {
        Err(ConverseError::Other(error)) => match super::server_overflow(&error) {
            Some(overflow) => {
                match super::context::after_overflow(config, &overflow, input, max_tokens) {
                    Ok(limit) => converse_once(
                        config,
                        messages,
                        tools,
                        temperature,
                        limit,
                        cancel,
                        on_delta,
                    ),
                    Err(local) => Err(ConverseError::Other(anyhow::Error::new(local))),
                }
            }
            None => Err(ConverseError::Other(error)),
        },
        other => other,
    }
}

fn converse_once(
    config: &LmStudioConfig,
    messages: &[Value],
    tools: Option<&[Value]>,
    temperature: f32,
    max_tokens: u32,
    cancel: &AtomicBool,
    on_delta: &mut dyn FnMut(StreamDelta<'_>),
) -> Result<ConverseOutcome, ConverseError> {
    if config.model.trim().is_empty() {
        return Err(ConverseError::Other(anyhow!("请先在 AI 管理页选择模型")));
    }
    let mut payload = json!({
        "model": config.model,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": true,
    });
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        payload["tools"] = Value::Array(tools.to_vec());
        payload["tool_choice"] = json!("auto");
    }
    let client = crate::net::stream_client(&config.base_url, config.timeout_seconds)
        .map_err(ConverseError::Other)?;
    let mut http = client
        .post(super::endpoint(config, "chat/completions"))
        .json(&payload);
    if !config.api_key.trim().is_empty() {
        http = http.bearer_auth(config.api_key.trim());
    }
    let response = http
        .send()
        .context("调用模型服务失败")
        .map_err(ConverseError::Other)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(match super::ChatError::from_status(status, &body) {
            super::ChatError::Rejected(error) => ConverseError::Rejected(error),
            super::ChatError::Other(error) => ConverseError::Other(error),
        });
    }
    let is_json = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"));
    let mut collector = Collector::new(on_delta);
    if is_json {
        let body = response
            .text()
            .context("读取模型服务响应失败")
            .map_err(ConverseError::Other)?;
        let parsed: ChatResponseRaw = serde_json::from_str(&body)
            .context("模型服务响应不是兼容的 Chat Completions 格式")
            .map_err(ConverseError::Other)?;
        let choice = parsed.choices.first().ok_or_else(|| {
            ConverseError::Other(anyhow!("模型服务返回了空的 choices，请检查模型是否已加载"))
        })?;
        let message = &choice["message"];
        for key in ["reasoning_content", "reasoning"] {
            if let Some(text) = message[key].as_str() {
                collector.reasoning(text);
            }
        }
        if let Some(text) = message["content"].as_str() {
            collector.content(text);
        }
        if let Some(calls) = message["tool_calls"].as_array() {
            for (index, call) in calls.iter().enumerate() {
                collector.tool_delta(index, call);
            }
        }
        collector.finish_reason(choice["finish_reason"].as_str());
    } else {
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                collector.cancelled = true;
                break;
            }
            line.clear();
            let read = reader
                .read_line(&mut line)
                .context("读取模型服务的流式响应失败")
                .map_err(ConverseError::Other)?;
            if read == 0 {
                break;
            }
            match parse_sse_line(&line) {
                SseLine::Skip => {}
                SseLine::Done => break,
                SseLine::Data(value) => {
                    if let Some(message) = stream_error(&value) {
                        return Err(ConverseError::Other(anyhow!(
                            "模型服务在生成中报错：{message}"
                        )));
                    }
                    let choice = &value["choices"][0];
                    let delta = &choice["delta"];
                    for key in ["reasoning_content", "reasoning"] {
                        if let Some(text) = delta[key].as_str() {
                            collector.reasoning(text);
                        }
                    }
                    if let Some(text) = delta["content"].as_str() {
                        collector.content(text);
                    }
                    if let Some(calls) = delta["tool_calls"].as_array() {
                        for (position, call) in calls.iter().enumerate() {
                            let index = call["index"].as_u64().map_or(position, |i| i as usize);
                            collector.tool_delta(index, call);
                        }
                    }
                    collector.finish_reason(choice["finish_reason"].as_str());
                }
            }
        }
    }
    Ok(collector.end())
}

struct Collector<'f> {
    on_delta: &'f mut dyn FnMut(StreamDelta<'_>),
    splitter: ThinkSplitter,
    content: String,
    calls: Vec<WireToolCall>,
    truncated: bool,
    cancelled: bool,
}

impl<'f> Collector<'f> {
    fn new(on_delta: &'f mut dyn FnMut(StreamDelta<'_>)) -> Self {
        Self {
            on_delta,
            splitter: ThinkSplitter::default(),
            content: String::new(),
            calls: Vec::new(),
            truncated: false,
            cancelled: false,
        }
    }

    fn reasoning(&mut self, text: &str) {
        if !text.is_empty() {
            (self.on_delta)(StreamDelta::Reasoning(text));
        }
    }

    fn content(&mut self, text: &str) {
        let mut pieces = Vec::new();
        self.splitter.feed(text, |reasoning, piece| {
            pieces.push((reasoning, piece.to_string()))
        });
        self.emit(pieces);
    }

    fn emit(&mut self, pieces: Vec<(bool, String)>) {
        for (reasoning, piece) in pieces {
            if reasoning {
                self.reasoning(&piece);
            } else {
                self.content.push_str(&piece);
                (self.on_delta)(StreamDelta::Content(&piece));
            }
        }
    }

    /// 一个工具调用的分片：`id`、`function.name` 只在第一片，`function.arguments` 分片累加。
    /// 参数是对象（个别服务端非流式这么给）时直接转成文本。
    fn tool_delta(&mut self, index: usize, call: &Value) {
        if self.calls.len() <= index {
            self.calls.resize_with(index + 1, WireToolCall::default);
        }
        let slot = &mut self.calls[index];
        if let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) {
            slot.id = id.to_string();
        }
        let function = &call["function"];
        if let Some(name) = function["name"].as_str() {
            slot.name.push_str(name);
        }
        match &function["arguments"] {
            Value::String(text) => slot.arguments.push_str(text),
            Value::Null => {}
            other => slot.arguments.push_str(&other.to_string()),
        }
    }

    fn finish_reason(&mut self, reason: Option<&str>) {
        if reason == Some("length") {
            self.truncated = true;
        }
    }

    fn end(mut self) -> ConverseOutcome {
        let mut pieces = Vec::new();
        self.splitter
            .flush(|reasoning, piece| pieces.push((reasoning, piece.to_string())));
        self.emit(pieces);
        let finish = if self.cancelled {
            Finish::Cancelled
        } else if self.truncated {
            Finish::Length
        } else {
            Finish::Stop
        };
        let mut calls: Vec<WireToolCall> = self
            .calls
            .into_iter()
            .filter(|call| !call.name.trim().is_empty())
            .collect();
        for (index, call) in calls.iter_mut().enumerate() {
            if call.id.is_empty() {
                call.id = format!("call_{}", index + 1);
            }
        }
        ConverseOutcome {
            content: self.content,
            tool_calls: calls,
            finish,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 起一个只回一次的本机假服务：记下请求体，按 `status` 与 `body` 回话。
    fn serve(
        status: &'static str,
        content_type: &'static str,
        body: String,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let request = loop {
                let n = stream.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if buf.len() >= head_end + 4 + length {
                        break text[head_end + 4..].to_string();
                    }
                }
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        (url, handle)
    }

    fn config(url: &str) -> LmStudioConfig {
        LmStudioConfig {
            base_url: url.into(),
            model: "m".into(),
            ..LmStudioConfig::default()
        }
    }

    #[test]
    fn streamed_tool_calls_are_stitched_by_index() {
        let events = [
            json!({"choices": [{"delta": {"reasoning_content": "想想"}}]}),
            json!({"choices": [{"delta": {"content": "先查。"}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "a", "function": {"name": "kb_search", "arguments": "{\"qu"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "ery\": \"防火\"}"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 1, "function": {"name": "finish", "arguments": "{}"}}]}, "finish_reason": "tool_calls"}]}),
        ];
        let body: String = events
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .chain(std::iter::once("data: [DONE]\n\n".to_string()))
            .collect();
        let (url, server) = serve("200 OK", "text/event-stream", body);
        let tools = vec![json!({"type": "function", "function": {"name": "kb_search"}})];
        let mut seen = Vec::new();
        let outcome = converse_stream(
            &config(&url),
            &[json!({"role": "user", "content": "查"})],
            Some(&tools),
            0.0,
            512,
            &AtomicBool::new(false),
            &mut |delta| seen.push(format!("{delta:?}")),
        )
        .unwrap();
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert_eq!(request["tools"][0]["function"]["name"], "kb_search");
        assert_eq!(request["tool_choice"], "auto");
        assert_eq!(outcome.content, "先查。");
        assert_eq!(outcome.finish, Finish::Stop);
        assert_eq!(outcome.tool_calls.len(), 2);
        assert_eq!(outcome.tool_calls[0].id, "a");
        assert_eq!(outcome.tool_calls[0].arguments, "{\"query\": \"防火\"}");
        assert_eq!(outcome.tool_calls[1].id, "call_2", "没给 id 的补一个");
        assert!(seen.iter().any(|d| d.contains("Reasoning(\"想想\")")));
    }

    #[test]
    fn a_plain_json_reply_with_tool_calls_is_understood() {
        let body = json!({"choices": [{"message": {"content": null, "tool_calls": [
            {"id": "x", "type": "function", "function": {"name": "ws_read", "arguments": {}}}
        ]}, "finish_reason": "tool_calls"}]})
        .to_string();
        let (url, server) = serve("200 OK", "application/json", body);
        let outcome = converse_stream(
            &config(&url),
            &[json!({"role": "user", "content": "读"})],
            None,
            0.0,
            512,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let request: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        assert!(request.get("tools").is_none(), "文本协议不带 tools");
        assert_eq!(outcome.tool_calls[0].name, "ws_read");
        assert_eq!(outcome.tool_calls[0].arguments, "{}");
    }

    #[test]
    fn a_client_error_means_tools_were_rejected() {
        let (url, server) = serve(
            "400 Bad Request",
            "application/json",
            r#"{"error": "\"auto\" tool choice requires --enable-auto-tool-choice"}"#.into(),
        );
        let error = converse_stream(
            &config(&url),
            &[json!({"role": "user", "content": "查"})],
            Some(&[json!({})]),
            0.0,
            512,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, ConverseError::Rejected(_)));
    }
}
