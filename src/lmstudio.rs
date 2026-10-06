use crate::models::{LmStudioConfig, ModelKind};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

pub mod context;
mod converse;
mod stream;
pub use context::ContextOverflow;
pub use converse::{ConverseError, converse_stream};
pub use stream::{Finish, StreamDelta, generate_stream};

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
    /// `length` 表示被输出上限截断，`stop` 是正常结束。缺省表示服务端没给。
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    content: Option<String>,
    /// 思考型模型（Qwen3 等）把推理过程放在这里，正文可能是空的。
    /// 有它而没有 `content`，说明预算全花在思考上了。
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ModelInfo {
    id: String,
    /// vLLM 给的上下文窗口。
    max_model_len: Option<u64>,
    /// LM Studio 原生接口给的两个：已加载的与最大的，前者优先。
    loaded_context_length: Option<u64>,
    max_context_length: Option<u64>,
    /// OpenRouter 等的写法。
    context_length: Option<u64>,
}

impl ModelInfo {
    fn context_window(&self) -> Option<usize> {
        [
            self.max_model_len,
            self.loaded_context_length,
            self.max_context_length,
            self.context_length,
        ]
        .into_iter()
        .flatten()
        .next()
        .map(|n| n as usize)
    }
}

fn client(config: &LmStudioConfig) -> Result<Client> {
    crate::net::client(&config.base_url, config.timeout_seconds)
}

fn endpoint(config: &LmStudioConfig, path: &str) -> String {
    format!("{}/{}", config.base_url.trim_end_matches('/'), path)
}

/// 列出任意 OpenAI 兼容端点已加载的模型。各提供商探测时各用各的地址。
pub fn list_models_at(base_url: &str, api_key: &str, timeout_seconds: u64) -> Result<Vec<String>> {
    let client = crate::net::client(base_url, timeout_seconds)?;
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut request = client.get(url);
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let response = request.send().context("无法连接模型服务")?;
    if !response.status().is_success() {
        bail!("模型服务返回 HTTP {}", response.status());
    }
    let data = response
        .json::<ModelsResponse>()
        .context("模型列表格式无法解析")?
        .data;
    // 顺手记下各模型的上下文窗口，自动模式按它算。
    context::record_service(
        base_url,
        &data
            .iter()
            .map(|m| (m.id.clone(), m.context_window()))
            .collect::<Vec<_>>(),
    );
    let mut models = data.into_iter().map(|m| m.id).collect::<Vec<_>>();
    models.sort();
    Ok(models)
}

/// LM Studio 原生接口（≥0.3.6）自报的模型用途：`llm` / `vlm` / `embeddings`。
///
/// OpenAI 兼容的 `/v1/models` 不给类型，只有这个接口给。不是 LM Studio
/// （404 或格式不对）时返回 None，探测静默跳过，不影响模型清单本身。
pub fn native_model_kinds(
    base_url: &str,
    api_key: &str,
    timeout_seconds: u64,
) -> Option<HashMap<String, ModelKind>> {
    #[derive(Deserialize)]
    struct NativeModels {
        data: Vec<NativeModel>,
    }
    #[derive(Deserialize)]
    struct NativeModel {
        id: String,
        #[serde(rename = "type")]
        kind: String,
    }
    let client = crate::net::client(base_url, timeout_seconds).ok()?;
    // 原生接口挂在主机根路径上，不在 /v1 下。
    let root = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let mut request = client.get(format!("{root}/api/v0/models"));
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let response = request.send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let data = response.json::<NativeModels>().ok()?.data;
    let kinds = data
        .into_iter()
        .map(|model| {
            // llm / vlm 都按对话；reranker 它多半也报 llm，由名字兜底（classify_model）。
            let kind = match model.kind.as_str() {
                "embeddings" => ModelKind::Embedding,
                _ => ModelKind::Chat,
            };
            (model.id, kind)
        })
        .collect();
    Some(kinds)
}

pub fn generate(config: &LmStudioConfig, system: &str, user: &str) -> Result<String> {
    generate_with(config, system, user, config.temperature, config.max_tokens)
}

/// 失败重试一次的对话补全，供逐句复核这类「一轮几十次调用」的场景使用。
///
/// 只重一次，且不做退避：本地服务偶尔一次连接被拒或读超时是常事，重一次基本
/// 就好；真的挂了就该立刻把错误报上去，让用户去看服务，而不是在这里磨十几秒
/// 之后再说同一句话。起草那条路径不用它——起草一次就是一次，失败了用户自己会
/// 再点一次，悄悄重试反而会让人以为模型在慢慢想。
pub fn generate_retrying(
    config: &LmStudioConfig,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
    options: ChatOptions,
) -> Result<String> {
    match generate_with_options(config, system, user, temperature, max_tokens, options) {
        Ok(text) => Ok(text),
        Err(first) => generate_with_options(config, system, user, temperature, max_tokens, options)
            .with_context(|| format!("重试前的首次失败：{first:#}")),
    }
}

/// 本进程内是否已确认「这个服务端不认关思考的开关」。
///
/// 各家 OpenAI 兼容服务对未知字段的态度不一样：多数忽略，少数直接 400。
/// 被拒过一次就不再带，否则每句话都要白花一个来回。
static THINKING_SWITCH_REJECTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 一次补全的可选开关。
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatOptions {
    /// 关掉思考型模型的推理输出。
    ///
    /// Qwen3 一类的模型默认开着 thinking，会把输出预算全花在推理上，正文返回
    /// 空值。这个开关**由应用发出去**，不该要求用户自己去改服务端配置——
    /// 各家的关法都不一样，让用户去找等于把问题推回去。
    pub disable_thinking: bool,
}

/// 用指定的温度与输出上限跑一次对话补全。
///
/// 知识库的 LLM 重排要低温、短输出，不能沿用起草那套采样参数——起草要
/// 发挥，打分只要稳定。
pub fn generate_with(
    config: &LmStudioConfig,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<String> {
    generate_with_options(
        config,
        system,
        user,
        temperature,
        max_tokens,
        ChatOptions::default(),
    )
}

/// 带开关的补全。
///
/// 要求关思考时先带上各家的开关发一次；服务端若因不认这些字段而 4xx，
/// 就记下来并原样重发一次，之后不再带。**降级是自动的**，用户不必知道
/// 自己那套服务支持哪种写法。
pub fn generate_with_options(
    config: &LmStudioConfig,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
    options: ChatOptions,
) -> Result<String> {
    use std::sync::atomic::Ordering::Relaxed;
    let with_switch = options.disable_thinking && !THINKING_SWITCH_REJECTED.load(Relaxed);
    let input = context::estimate_tokens(system) + context::estimate_tokens(user);
    within_window(config, input, max_tokens, |limit| {
        match complete_once(config, system, user, temperature, limit, with_switch) {
            Err(ChatError::Rejected(error)) if with_switch => {
                // 只可能是那几个多出来的字段惹的祸：不带它们再发一次。
                THINKING_SWITCH_REJECTED.store(true, Relaxed);
                complete_once(config, system, user, temperature, limit, false)
                    .map_err(|second| second.switch_retried(&error))
            }
            other => other,
        }
    })
    .map_err(ChatError::into_inner)
}

/// 区分「服务端嫌请求不合法」和别的失败：只有前者值得去掉开关重试。
/// 输入超出上下文不算前者（[`ContextOverflow`] 放在 `Other` 里），另由 [`within_window`] 处理。
enum ChatError {
    Rejected(anyhow::Error),
    Other(anyhow::Error),
}

impl ChatError {
    fn into_inner(self) -> anyhow::Error {
        match self {
            ChatError::Rejected(error) | ChatError::Other(error) => error,
        }
    }

    /// 去掉关思考开关重发后仍失败：带上首次失败的原因。
    fn switch_retried(self, first: &anyhow::Error) -> Self {
        let note = format!("（已去掉关闭思考的开关重试；带开关时的首次失败：{first:#}）");
        match self {
            ChatError::Rejected(error) => ChatError::Rejected(error.context(note)),
            ChatError::Other(error) => ChatError::Other(error.context(note)),
        }
    }

    /// 非 2xx 响应：输入超长单独认出来，其余 4xx 算「嫌请求不合法」。
    fn from_status(status: reqwest::StatusCode, body: &str) -> Self {
        if status.is_client_error()
            && let Some(overflow) = context::parse_overflow(body)
        {
            return ChatError::Other(anyhow::Error::new(overflow));
        }
        let error = anyhow::anyhow!("模型服务返回 HTTP {status}：{body}");
        // 只有 4xx 才可能是「不认识多带的那几个字段」，值得去掉重试；
        // 5xx 是服务端自己的问题，重试也一样。
        if status.is_client_error() {
            ChatError::Rejected(error)
        } else {
            ChatError::Other(error)
        }
    }
}

/// 服务端报的超长（还没按它重试过）。
pub(crate) fn server_overflow(error: &anyhow::Error) -> Option<ContextOverflow> {
    error
        .downcast_ref::<ContextOverflow>()
        .filter(|overflow| overflow.from_server)
        .cloned()
}

/// 发请求前按上下文窗口现算输出上限（`docs/ai-agent-workbench.md` 16.15 A.3）：本地估算就放不下的
/// 不发；服务端报超长时记下它说的上限、重算后重发一次（A.7）。
fn within_window<T>(
    config: &LmStudioConfig,
    input: usize,
    max_tokens: u32,
    mut send: impl FnMut(u32) -> std::result::Result<T, ChatError>,
) -> std::result::Result<T, ChatError> {
    let window = context::peek_window(config);
    let Some(limit) = context::output_limit(window.tokens, input, max_tokens) else {
        let overflow = context::local_overflow(window, input);
        return Err(ChatError::Other(anyhow::Error::new(overflow)));
    };
    match send(limit) {
        Err(ChatError::Other(error)) => match server_overflow(&error) {
            Some(overflow) => match context::after_overflow(config, &overflow, input, max_tokens) {
                Ok(limit) => send(limit),
                Err(local) => Err(ChatError::Other(anyhow::Error::new(local))),
            },
            None => Err(ChatError::Other(error)),
        },
        other => other,
    }
}

fn complete_once(
    config: &LmStudioConfig,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
    disable_thinking: bool,
) -> std::result::Result<String, ChatError> {
    if config.model.trim().is_empty() {
        return Err(ChatError::Other(anyhow::anyhow!("请先在设置中选择模型")));
    }
    let payload = chat_payload(
        config,
        system,
        user,
        temperature,
        max_tokens,
        disable_thinking,
        false,
    );

    let client = client(config).map_err(ChatError::Other)?;
    let mut request = client
        .post(endpoint(config, "chat/completions"))
        .json(&payload);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }
    let response = request
        .send()
        .context("调用模型服务失败")
        .map_err(ChatError::Other)?;
    let status = response.status();
    let body = response
        .text()
        .context("读取模型服务响应失败")
        .map_err(ChatError::Other)?;
    if !status.is_success() {
        return Err(ChatError::from_status(status, &body));
    }

    let parsed: ChatResponse = serde_json::from_str(&body)
        .context("模型服务响应不是兼容的 Chat Completions 格式")
        .map_err(ChatError::Other)?;
    let Some(choice) = parsed.choices.into_iter().next() else {
        return Err(ChatError::Other(anyhow::anyhow!(
            "模型服务返回了空的 choices，请检查模型是否已加载"
        )));
    };
    let truncated = choice.finish_reason.as_deref() == Some("length");
    let content = choice.message.content.unwrap_or_default();
    if !content.trim().is_empty() {
        return Ok(content);
    }

    let thinking = choice
        .message
        .reasoning_content
        .is_some_and(|text| !text.trim().is_empty());
    Err(ChatError::Other(empty_content_error(
        max_tokens,
        thinking,
        truncated,
        disable_thinking,
    )))
}

/// 组装一次 Chat Completions 请求体。流式与非流式只差 `stream` 一个字段。
fn chat_payload(
    config: &LmStudioConfig,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
    disable_thinking: bool,
    stream: bool,
) -> serde_json::Value {
    let mut payload = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": stream
    });
    if disable_thinking {
        let fields = payload.as_object_mut().expect("payload 必然是对象");
        // 三家写法一起带，不认的通常会被忽略；全被拒时由调用方去掉重试。
        // vLLM / SGLang / 较新的 LM Studio：透传给 chat template。
        fields.insert(
            "chat_template_kwargs".into(),
            json!({"enable_thinking": false}),
        );
        // Ollama 的原生开关。
        fields.insert("think".into(), json!(false));
        // 阿里云百炼等把它放在顶层。
        fields.insert("enable_thinking".into(), json!(false));
    }
    payload
}

/// 模型没给正文（只有思考过程或什么都没有）。调用方可以据此重试一次：思考型模型偶尔会在
/// 思考里把话说完、正文留空，再问一次通常就好。
#[derive(Debug)]
pub struct EmptyContent(pub String);

impl std::fmt::Display for EmptyContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EmptyContent {}

/// 模型没给正文时的报错。
///
/// 正文为空有三种成因，报出来要能直接指向下一步怎么办，而不是笼统一句
/// 「未返回正文」让人去翻服务端日志。
fn empty_content_error(
    max_tokens: u32,
    thinking: bool,
    truncated: bool,
    switch_sent: bool,
) -> anyhow::Error {
    anyhow::Error::new(EmptyContent(empty_content_text(
        max_tokens,
        thinking,
        truncated,
        switch_sent,
    )))
}

fn empty_content_text(
    max_tokens: u32,
    thinking: bool,
    truncated: bool,
    switch_sent: bool,
) -> String {
    if thinking && switch_sent {
        format!(
            "模型只输出了思考过程，没有正文：{max_tokens} 的输出上限被推理占满。             应用已自动带上关闭思考的开关（chat_template_kwargs.enable_thinking、             think、enable_thinking 三种写法），你的服务端似乎都不认。             请在服务端关掉思考模式，或换一个非思考模型来做文字复核。"
        )
    } else if thinking && !truncated {
        // 没撞上限却只有思考：模型在思考里把话说完了，正文留空（MiniMax-M2.7 偶见）。
        "模型只输出了思考过程，没有正文（没有撞上输出上限，像是把回答写在了思考里）。已经重试过一次；\
         再遇到可以重新生成，或换一个模型。"
            .to_string()
    } else if thinking {
        // 起草不关思考（想得周全些写得更好），预算不够时就会这样。
        format!(
            "模型只输出了思考过程，没有正文：{max_tokens} 的输出上限被推理占满。             请在设置里调大「最大输出」，或在服务端关掉思考模式、换一个非思考模型。"
        )
    } else if truncated {
        format!("模型输出在 {max_tokens} token 处被截断，且截断前没有正文")
    } else {
        "模型服务未返回正文（choices[0].message.content 为空）".to_string()
    }
}
