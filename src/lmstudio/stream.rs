//! 流式对话补全（SSE）。
//!
//! 起草、润色这类长输出改成边生成边显示，不再让人干等几十秒后结果突然弹出来。
//! 只改「原文怎么收」，不改「怎么审」：流结束后的清洗、规整与闸门与非流式
//! 完全一致，由调用方照旧去跑。
//!
//! 解析拆成两个纯函数部件，便于单测：
//! - [`parse_sse_line`]：一行 SSE → 数据 / 结束 / 跳过；
//! - [`ThinkSplitter`]：把夹在正文里的 `<think>…</think>` 切到思考通道，
//!   标签被切在两个分片之间也能认出来。

use super::{
    ChatError, ChatResponse, THINKING_SWITCH_REJECTED, chat_payload, context, empty_content_error,
    within_window,
};
use crate::models::LmStudioConfig;
use anyhow::{Context, anyhow};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};

/// 一段增量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDelta<'a> {
    /// 正文。
    Content(&'a str),
    /// 思考过程：`reasoning_content` 字段，或正文里 `<think>` 包起来的部分。
    Reasoning(&'a str),
}

/// 一次流式补全是怎么结束的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// 正常结束。
    Stop,
    /// 撞上了输出上限，正文可能不完整。
    Length,
    /// 用户点了停止。
    Cancelled,
}

/// 流式补全的结果。`content` 已剔除 `<think>` 段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamOutcome {
    pub usage: Option<super::Usage>,
    pub content: String,
    pub reasoning_chars: usize,
    pub finish: Finish,
}

/// 流式跑一次对话补全，每收到一段就回调 `on_delta`。
///
/// - `cancel` 每读一行检查一次；置位后断开连接（服务端随之停止推理），返回
///   [`Finish::Cancelled`] 与已收到的部分。阻塞读打断不了：模型还在处理长提示词、
///   一个字都没吐时，要等到下一行到达才生效，所以界面上的「停止」不能干等它。
/// - 服务端不认 `stream` 而回了整段 JSON 时自动按非流式解析，一次回调完。
/// - 关思考开关被 4xx 拒绝时去掉重发，规则与 [`super::generate_with_options`] 相同。
#[allow(clippy::too_many_arguments)]
pub fn generate_stream(
    config: &LmStudioConfig,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
    options: super::ChatOptions,
    cancel: &AtomicBool,
    mut on_delta: impl FnMut(StreamDelta<'_>),
) -> anyhow::Result<StreamOutcome> {
    let with_switch = options.disable_thinking && !THINKING_SWITCH_REJECTED.load(Ordering::Relaxed);
    let input = context::estimate_tokens(system) + context::estimate_tokens(user);
    let format = options
        .json_schema
        .as_ref()
        .map(|(name, schema)| super::response_format(name, schema));
    within_window(config, input, max_tokens, |limit| {
        let with_switch = with_switch && !THINKING_SWITCH_REJECTED.load(Ordering::Relaxed);
        let mut request = StreamRequest {
            config,
            system,
            user,
            temperature,
            max_tokens: limit,
            response_format: format.as_ref().filter(|_| !super::format_rejected(config)),
            include_usage: !super::field_rejected(config, "stream_options"),
        };
        let first = stream_once(&request, with_switch, cancel, &mut on_delta);
        // 带了 `response_format` 被 4xx 拒：先去掉它重发（记下这个端点不认），调用方按文本约定解析。
        let first = match first {
            Err(ChatError::Rejected(_)) if request.response_format.is_some() => {
                super::reject_format(config);
                request.response_format = None;
                stream_once(&request, with_switch, cancel, &mut on_delta)
            }
            other => other,
        };
        // 固定降级顺序：格式 → 用量 → 关思考；最多三次额外请求，超长重算另计一次。
        let first = match first {
            Err(ChatError::Rejected(_)) if request.include_usage => {
                super::reject_field(config, "stream_options");
                request.include_usage = false;
                stream_once(&request, with_switch, cancel, &mut on_delta)
            }
            other => other,
        };
        match first {
            Err(ChatError::Rejected(error)) if with_switch => {
                THINKING_SWITCH_REJECTED.store(true, Ordering::Relaxed);
                stream_once(&request, false, cancel, &mut on_delta)
                    .map_err(|second| second.switch_retried(&error))
            }
            other => other,
        }
    })
    .map_err(ChatError::into_inner)
}

struct StreamRequest<'a> {
    include_usage: bool,
    config: &'a LmStudioConfig,
    system: &'a str,
    user: &'a str,
    temperature: f32,
    max_tokens: u32,
    response_format: Option<&'a serde_json::Value>,
}

fn stream_once(
    request: &StreamRequest<'_>,
    disable_thinking: bool,
    cancel: &AtomicBool,
    on_delta: &mut dyn FnMut(StreamDelta<'_>),
) -> Result<StreamOutcome, ChatError> {
    let config = request.config;
    if config.model.trim().is_empty() {
        return Err(ChatError::Other(anyhow!("请先在设置中选择模型")));
    }
    let mut payload = chat_payload(
        config,
        request.system,
        request.user,
        request.temperature,
        request.max_tokens,
        disable_thinking,
        true,
    );
    if let Some(format) = request.response_format {
        payload["response_format"] = format.clone();
    }
    if request.include_usage {
        payload["stream_options"] = serde_json::json!({"include_usage": true});
    }
    let client = crate::net::stream_client(&config.base_url, config.timeout_seconds)
        .map_err(ChatError::Other)?;
    let mut http = client
        .post(super::endpoint(config, "chat/completions"))
        .json(&payload);
    http = super::request_headers(http, config);
    if !config.api_key.trim().is_empty() {
        http = http.bearer_auth(config.api_key.trim());
    }
    let response = http
        .send()
        .context("调用模型服务失败")
        .map_err(ChatError::Other)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(ChatError::from_status(status, &body));
    }
    let is_json = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"));

    let mut collector = Collector::new(on_delta);
    if is_json {
        // 服务端没理 `stream: true`，整段回来了：按非流式解析，一次喂完。
        let body = response
            .text()
            .context("读取模型服务响应失败")
            .map_err(ChatError::Other)?;
        let parsed: ChatResponse = serde_json::from_str(&body)
            .context("模型服务响应不是兼容的 Chat Completions 格式")
            .map_err(ChatError::Other)?;
        collector.usage = parsed.usage;
        let Some(choice) = parsed.choices.into_iter().next() else {
            return Err(ChatError::Other(anyhow!(
                "模型服务返回了空的 choices，请检查模型是否已加载"
            )));
        };
        if let Some(reasoning) = &choice.message.reasoning_content {
            collector.reasoning(reasoning);
        }
        if let Some(content) = &choice.message.content {
            collector.content(content);
        }
        collector.finish_reason(choice.finish_reason.as_deref());
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
                .map_err(ChatError::Other)?;
            if read == 0 {
                // 接受仅用 finish_reason 收尾的服务；没有任何结束标记的断流不能当成完整稿。
                if !collector.finished {
                    return Err(ChatError::Other(anyhow!("模型服务在结束标记之前断开连接")));
                }
                break;
            }
            match parse_sse_line(&line) {
                SseLine::Skip => {}
                SseLine::Done => {
                    collector.finished = true;
                    break;
                }
                SseLine::Data(value) => {
                    if let Some(usage) = value
                        .get("usage")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                    {
                        collector.usage = Some(usage);
                    }
                    if let Some(message) = stream_error(&value) {
                        return Err(ChatError::Other(anyhow!("模型服务在生成中报错：{message}")));
                    }
                    let choice = &value["choices"][0];
                    let delta = &choice["delta"];
                    // `reasoning_content` 是 DeepSeek / Qwen 系的写法，`reasoning` 是
                    // Ollama、OpenRouter 的写法，两个都认。
                    for key in ["reasoning_content", "reasoning"] {
                        if let Some(text) = delta[key].as_str() {
                            collector.reasoning(text);
                        }
                    }
                    if let Some(text) = delta["content"].as_str() {
                        collector.content(text);
                    }
                    collector.finish_reason(choice["finish_reason"].as_str());
                }
            }
        }
        // 走到这里时 reader 被丢掉，连接随之断开；停止时服务端据此结束推理。
    }
    collector.end(request.max_tokens, disable_thinking)
}

/// 一行 SSE 的解析结果。
#[derive(Debug, PartialEq)]
pub(super) enum SseLine {
    Data(Value),
    Done,
    /// 空行、`:` 开头的心跳注释、`event:` / `id:` 等用不上的字段，以及解析不了的数据。
    Skip,
}

pub(super) fn parse_sse_line(line: &str) -> SseLine {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(data) = line.strip_prefix("data:") else {
        return SseLine::Skip;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return SseLine::Done;
    }
    serde_json::from_str(data).map_or(SseLine::Skip, SseLine::Data)
}

/// 流里夹带的错误对象（`{"error": {...}}` 或 `{"error": "..."}`）。
pub(super) fn stream_error(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    Some(
        error["message"]
            .as_str()
            .or_else(|| error.as_str())
            .map_or_else(|| error.to_string(), str::to_string),
    )
}

/// 收集增量：正文过一遍 `<think>` 切分，同时累计结果。
struct Collector<'f> {
    finished: bool,
    usage: Option<super::Usage>,
    on_delta: &'f mut dyn FnMut(StreamDelta<'_>),
    splitter: ThinkSplitter,
    content: String,
    reasoning_chars: usize,
    truncated: bool,
    cancelled: bool,
}

impl<'f> Collector<'f> {
    fn new(on_delta: &'f mut dyn FnMut(StreamDelta<'_>)) -> Self {
        Self {
            finished: false,
            usage: None,
            on_delta,
            splitter: ThinkSplitter::default(),
            content: String::new(),
            reasoning_chars: 0,
            truncated: false,
            cancelled: false,
        }
    }

    fn reasoning(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.reasoning_chars += text.chars().count();
        (self.on_delta)(StreamDelta::Reasoning(text));
    }

    fn content(&mut self, text: &str) {
        let mut pieces = Vec::new();
        self.splitter.feed(text, |reasoning, piece| {
            pieces.push((reasoning, piece.to_string()))
        });
        self.emit(pieces);
    }

    fn finish_reason(&mut self, reason: Option<&str>) {
        self.finished |= reason.is_some();
        if reason == Some("length") {
            self.truncated = true;
        }
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

    fn end(mut self, max_tokens: u32, switch_sent: bool) -> Result<StreamOutcome, ChatError> {
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
        // 停下来的不报空正文：那是用户自己喊停的，不是模型出了问题。
        if finish != Finish::Cancelled && self.content.trim().is_empty() {
            return Err(ChatError::Other(empty_content_error(
                max_tokens,
                self.reasoning_chars > 0,
                self.truncated,
                switch_sent,
            )));
        }
        Ok(StreamOutcome {
            usage: self.usage,
            content: self.content,
            reasoning_chars: self.reasoning_chars,
            finish,
        })
    }
}

const OPEN_TAGS: [&str; 2] = ["<think>", "<thinking>"];
const CLOSE_TAGS: [&str; 2] = ["</think>", "</thinking>"];

/// 把正文里 `<think>…</think>`（或 `<thinking>`）包起来的部分切到思考通道。
///
/// 有的服务端不拆 `reasoning_content`，把思考过程原样夹在正文里。标签可能被
/// 切在两个分片中间（`<thi` + `nk>`），所以末尾若是某个标签的前缀，先扣住，
/// 等下一片来了再判断。
#[derive(Debug, Default)]
pub(super) struct ThinkSplitter {
    in_think: bool,
    pending: String,
}

impl ThinkSplitter {
    /// 喂一片正文，按顺序回调 `(是否思考, 文字)`；空文字不回调。
    pub(super) fn feed(&mut self, chunk: &str, mut out: impl FnMut(bool, &str)) {
        let mut buf = std::mem::take(&mut self.pending);
        buf.push_str(chunk);
        let mut rest = buf.as_str();
        loop {
            let tags = if self.in_think { CLOSE_TAGS } else { OPEN_TAGS };
            let found = tags
                .iter()
                .filter_map(|tag| rest.find(tag).map(|pos| (pos, tag.len())))
                .min_by_key(|(pos, _)| *pos);
            if let Some((pos, len)) = found {
                if pos > 0 {
                    out(self.in_think, &rest[..pos]);
                }
                self.in_think = !self.in_think;
                rest = &rest[pos + len..];
                continue;
            }
            let keep = partial_tag_suffix(rest, &tags);
            let emit = &rest[..rest.len() - keep];
            if !emit.is_empty() {
                out(self.in_think, emit);
            }
            self.pending = rest[rest.len() - keep..].to_string();
            return;
        }
    }

    /// 流结束：扣着的半截标签原样当文字吐出去。
    pub(super) fn flush(&mut self, mut out: impl FnMut(bool, &str)) {
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            out(self.in_think, &pending);
        }
    }
}

/// `text` 末尾若是某个标签的真前缀（如 `<thi`），返回它的字节长度，否则 0。
fn partial_tag_suffix(text: &str, tags: &[&str]) -> usize {
    let Some(start) = text.rfind('<') else {
        return 0;
    };
    let tail = &text[start..];
    if tags
        .iter()
        .any(|tag| tag.len() > tail.len() && tag.starts_with(tail))
    {
        tail.len()
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn split_all(chunks: &[&str]) -> Vec<(bool, String)> {
        let mut splitter = ThinkSplitter::default();
        let mut out: Vec<(bool, String)> = Vec::new();
        let mut push = |reasoning: bool, text: &str| match out.last_mut() {
            Some((last, buf)) if *last == reasoning => buf.push_str(text),
            _ => out.push((reasoning, text.to_string())),
        };
        for chunk in chunks {
            splitter.feed(chunk, &mut push);
        }
        splitter.flush(&mut push);
        out
    }

    #[test]
    fn sse_lines_are_classified() {
        assert_eq!(parse_sse_line("\n"), SseLine::Skip);
        assert_eq!(parse_sse_line(": keep-alive\n"), SseLine::Skip);
        assert_eq!(parse_sse_line("event: message\n"), SseLine::Skip);
        assert_eq!(parse_sse_line("data: [DONE]\r\n"), SseLine::Done);
        assert_eq!(parse_sse_line("data: 不是 JSON\n"), SseLine::Skip);
        assert_eq!(
            parse_sse_line("data:{\"a\":1}\n"),
            SseLine::Data(serde_json::json!({"a": 1}))
        );
    }

    #[test]
    fn stream_errors_are_read_in_both_shapes() {
        let object = serde_json::json!({"error": {"message": "模型未加载"}});
        assert_eq!(stream_error(&object).as_deref(), Some("模型未加载"));
        let plain = serde_json::json!({"error": "boom"});
        assert_eq!(stream_error(&plain).as_deref(), Some("boom"));
        assert_eq!(stream_error(&serde_json::json!({"choices": []})), None);
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(
            split_all(&["一、", "总体要求"]),
            vec![(false, "一、总体要求".into())]
        );
    }

    #[test]
    fn think_block_is_routed_to_reasoning() {
        assert_eq!(
            split_all(&["<think>先想想</think>\n# 标题"]),
            vec![(true, "先想想".into()), (false, "\n# 标题".into())]
        );
    }

    #[test]
    fn tags_split_across_chunks_are_recognised() {
        assert_eq!(
            split_all(&["<thi", "nk>盘", "算</thi", "nking", "", "</think>正文"]),
            vec![(true, "盘算</thinking".into()), (false, "正文".into())]
        );
        assert_eq!(
            split_all(&["<thin", "king>想</", "thinking>文"]),
            vec![(true, "想".into()), (false, "文".into())]
        );
    }

    #[test]
    fn a_lone_angle_bracket_is_not_swallowed() {
        assert_eq!(split_all(&["a<", "b"]), vec![(false, "a<b".into())]);
        // 流在半截标签处结束：原样吐出，不丢字。
        assert_eq!(split_all(&["结尾<thi"]), vec![(false, "结尾<thi".into())]);
    }

    /// 起一个只接一次请求的假服务端：读完请求，回 `response`（完整 HTTP 报文）。
    fn fake_server(response: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            stream.write_all(response.as_bytes()).unwrap();
            request
        });
        (format!("http://127.0.0.1:{port}/v1"), handle)
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if let Some(head_end) = text.find("\r\n\r\n") {
                let length = text[..head_end]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if buf.len() >= head_end + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// 依次接 `responses.len()` 个请求的假服务端，返回各次请求的 JSON 请求体。
    fn fake_server_seq(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let body = request.split_once("\r\n\r\n").map_or("", |(_, b)| b);
                bodies.push(serde_json::from_str(body).unwrap_or(Value::Null));
                stream.write_all(response.as_bytes()).unwrap();
            }
            bodies
        });
        (format!("http://127.0.0.1:{port}/v1"), handle)
    }

    /// 要 JSON 时带 `response_format`；端点 4xx 拒收就去掉重发，并记住这个端点，之后不再带。
    #[test]
    fn a_rejected_response_format_is_dropped_and_remembered() {
        let error = r#"{"error":"unknown field: response_format"}"#;
        let rejected = format!(
            "HTTP/1.1 400 Bad Request
Content-Type: application/json
Content-Length: {}
Connection: close

{error}",
            error.len()
        );
        let reply = || sse_response(&[&delta("好"), "data: [DONE]"]);
        let (url, server) = fake_server_seq(vec![rejected, reply(), reply()]);
        let config = config(url);
        let options = || super::super::ChatOptions {
            json_schema: Some(("q", serde_json::json!({"type": "object"}))),
            ..super::super::ChatOptions::default()
        };
        for _ in 0..2 {
            let outcome = generate_stream(
                &config,
                "S",
                "U",
                0.2,
                100,
                options(),
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
            assert_eq!(outcome.content, "好");
        }
        let bodies = server.join().unwrap();
        assert_eq!(bodies[0]["response_format"]["type"], "json_schema");
        assert_eq!(bodies[0]["response_format"]["json_schema"]["name"], "q");
        assert!(bodies[1].get("response_format").is_none(), "被拒后去掉重发");
        assert!(
            bodies[2].get("response_format").is_none(),
            "记住了，不再白试"
        );
        assert!(super::super::format_rejected(&config));
    }

    /// 32k 窗口的服务上，输出上限按剩余空间现算，不再是「输入 + 32000」必然超限；
    /// 服务端仍报超长时按它说的上限重算、重发一次。
    #[test]
    fn the_output_limit_follows_the_window_and_an_overflow_is_retried() {
        let error = r#"{"object":"error","message":"This model's maximum context length is 16384 tokens. However, you requested 39000 tokens (7000 in the messages, 32000 in the completion).","code":400}"#;
        let overflow = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{error}",
            error.len()
        );
        let reply = sse_response(&[&delta("好"), "data: [DONE]"]);
        let (url, server) = fake_server_seq(vec![overflow, reply]);
        let mut config = config(url);
        config.context_window = 32_768;
        let user = "字".repeat(5000);
        let outcome = generate_stream(
            &config,
            "S",
            &user,
            0.2,
            32_000,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.content, "好");
        let bodies = server.join().unwrap();
        let input = context::estimate_tokens("S") + context::estimate_tokens(&user);
        assert_eq!(bodies[0]["max_tokens"], 32_768 - input - 256);
        assert_eq!(bodies[1]["max_tokens"], 16_384 - 7350 - 256);
        // 之后都按服务端说的 16k 算，手动设的 32k 让位。
        assert_eq!(context::peek_window(&config).tokens, 16_384);

        // 本地估算就放不下的不发请求。
        let mut small = config.clone();
        small.base_url = "http://127.0.0.1:9/v1".into();
        small.context_window = 4096;
        let error = generate_stream(
            &small,
            "S",
            &user,
            0.2,
            32_000,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(error.downcast_ref::<context::ContextOverflow>().is_some());
        assert!(format!("{error}").contains("放不下"), "{error}");
    }

    fn sse_response(events: &[&str]) -> String {
        let body: String = events.iter().map(|event| format!("{event}\n\n")).collect();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
        )
    }

    fn config(base_url: String) -> LmStudioConfig {
        LmStudioConfig {
            base_url,
            model: "test-model".into(),
            timeout_seconds: 10,
            ..LmStudioConfig::default()
        }
    }

    fn delta(content: &str) -> String {
        format!(
            "data: {}",
            serde_json::json!({"choices": [{"delta": {"content": content}, "finish_reason": null}]})
        )
    }

    #[test]
    fn streams_content_and_reasoning_in_order() {
        let reasoning = format!(
            "data: {}",
            serde_json::json!({"choices": [{"delta": {"reasoning_content": "想一下"}}]})
        );
        let (url, server) = fake_server(sse_response(&[
            ": ping",
            &reasoning,
            &delta("# 关于"),
            &delta("冬季防火的通知"),
            "data: [DONE]",
        ]));
        let mut seen = Vec::new();
        let outcome = generate_stream(
            &config(url),
            "系统",
            "用户",
            0.2,
            512,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |delta| {
                seen.push(match delta {
                    StreamDelta::Content(text) => format!("C:{text}"),
                    StreamDelta::Reasoning(text) => format!("R:{text}"),
                })
            },
        )
        .unwrap();
        assert_eq!(outcome.content, "# 关于冬季防火的通知");
        assert_eq!(outcome.reasoning_chars, 3);
        assert_eq!(outcome.finish, Finish::Stop);
        assert_eq!(seen, ["R:想一下", "C:# 关于", "C:冬季防火的通知"]);
        let request = server.join().unwrap();
        assert!(request.contains("\"stream\":true"), "{request}");
    }

    #[test]
    fn usage_last_frame_and_rejected_stream_options_are_remembered() {
        let error = r#"{"error":"unknown field: stream_options"}"#;
        let rejected = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{error}",
            error.len()
        );
        let reply = || {
            sse_response(&[
                &delta("正文"),
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3,\"total_tokens\":15}}",
                "data: [DONE]",
            ])
        };
        let (url, server) = fake_server_seq(vec![rejected, reply(), reply()]);
        let config = config(url);
        for _ in 0..2 {
            let outcome = generate_stream(
                &config,
                "s",
                "u",
                0.2,
                64,
                super::super::ChatOptions::default(),
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap();
            assert_eq!(
                outcome.usage,
                Some(super::super::Usage {
                    prompt_tokens: 12,
                    completion_tokens: 3,
                    total_tokens: 15
                })
            );
        }
        let bodies = server.join().unwrap();
        assert_eq!(bodies[0]["stream_options"]["include_usage"], true);
        assert!(bodies[1].get("stream_options").is_none());
        assert!(bodies[2].get("stream_options").is_none());
    }

    #[test]
    fn length_finish_is_reported() {
        let last = format!(
            "data: {}",
            serde_json::json!({"choices": [{"delta": {"content": "半截"}, "finish_reason": "length"}]})
        );
        let (url, server) = fake_server(sse_response(&[&last]));
        let outcome = generate_stream(
            &config(url),
            "s",
            "u",
            0.2,
            8,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.finish, Finish::Length);
        assert_eq!(outcome.content, "半截");
        server.join().unwrap();
    }

    #[test]
    fn json_reply_falls_back_to_non_stream() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "<think>嗯</think>正文"}, "finish_reason": "stop"}]
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (url, server) = fake_server(response);
        let mut contents = String::new();
        let outcome = generate_stream(
            &config(url),
            "s",
            "u",
            0.2,
            64,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |delta| {
                if let StreamDelta::Content(text) = delta {
                    contents.push_str(text);
                }
            },
        )
        .unwrap();
        assert_eq!(outcome.content, "正文");
        assert_eq!(contents, "正文");
        assert_eq!(outcome.reasoning_chars, 1);
        server.join().unwrap();
    }

    #[test]
    fn cancel_stops_and_keeps_partial_content() {
        let (url, server) = fake_server(sse_response(&[
            &delta("第一段"),
            &delta("第二段"),
            &delta("第三段"),
            "data: [DONE]",
        ]));
        let cancel = AtomicBool::new(false);
        let outcome = generate_stream(
            &config(url),
            "s",
            "u",
            0.2,
            64,
            super::super::ChatOptions::default(),
            &cancel,
            |_| cancel.store(true, Ordering::Relaxed),
        )
        .unwrap();
        assert_eq!(outcome.finish, Finish::Cancelled);
        assert_eq!(outcome.content, "第一段");
        server.join().unwrap();
    }

    #[test]
    fn only_thinking_is_an_error() {
        let reasoning = format!(
            "data: {}",
            serde_json::json!({"choices": [{"delta": {"reasoning": "一直在想"}}]})
        );
        let (url, server) = fake_server(sse_response(&[&reasoning, "data: [DONE]"]));
        let error = generate_stream(
            &config(url),
            "s",
            "u",
            0.2,
            64,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("只输出了思考过程"),
            "{error:#}"
        );
        server.join().unwrap();
    }

    #[test]
    fn error_event_mid_stream_is_reported() {
        let (url, server) = fake_server(sse_response(&[
            &delta("开头"),
            "data: {\"error\":{\"message\":\"显存不足\"}}",
        ]));
        let error = generate_stream(
            &config(url),
            "s",
            "u",
            0.2,
            64,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("显存不足"), "{error:#}");
        server.join().unwrap();
    }

    /// 连真实的本机模型服务跑一次。默认忽略；用法：
    /// `GONGWEN_LIVE_LLM_URL=http://127.0.0.1:12345/v1 GONGWEN_LIVE_LLM_MODEL=qwen/qwen3.5-9b
    ///  cargo test --locked live_model_stream -- --ignored --nocapture`
    #[test]
    #[ignore = "需要本机模型服务"]
    fn live_model_stream() {
        let (Ok(url), Ok(model)) = (
            std::env::var("GONGWEN_LIVE_LLM_URL"),
            std::env::var("GONGWEN_LIVE_LLM_MODEL"),
        ) else {
            eprintln!("未设置 GONGWEN_LIVE_LLM_URL / GONGWEN_LIVE_LLM_MODEL，跳过");
            return;
        };
        let config = LmStudioConfig {
            base_url: url,
            model,
            timeout_seconds: 60,
            ..LmStudioConfig::default()
        };
        let started = std::time::Instant::now();
        let mut first_content = None;
        let mut batches = 0usize;
        let outcome = generate_stream(
            &config,
            "你是公文写作助手，只输出 Markdown 正文。",
            "用三句话写一段关于做好冬季森林防火工作的通知正文。",
            0.2,
            4096,
            super::super::ChatOptions::default(),
            &AtomicBool::new(false),
            |delta| {
                batches += 1;
                if let StreamDelta::Content(_) = delta {
                    first_content.get_or_insert(started.elapsed());
                }
            },
        )
        .unwrap();
        eprintln!(
            "增量 {batches} 段；思考 {} 字；首个正文字 {:?}；总耗时 {:?}；结束方式 {:?}
{}",
            outcome.reasoning_chars,
            first_content,
            started.elapsed(),
            outcome.finish,
            outcome.content
        );
        assert!(!outcome.content.trim().is_empty());
        assert!(batches > 1, "应当是分多段到达的");
    }
}
