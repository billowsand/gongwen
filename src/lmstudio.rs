use crate::models::LmStudioConfig;
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

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
}

fn client(config: &LmStudioConfig) -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(config.timeout_seconds.max(5)))
        .build()
        .context("创建 HTTP 客户端失败")
}

fn endpoint(config: &LmStudioConfig, path: &str) -> String {
    format!("{}/{}", config.base_url.trim_end_matches('/'), path)
}

pub fn list_models(config: &LmStudioConfig) -> Result<Vec<String>> {
    list_models_at(&config.base_url, &config.api_key, config.timeout_seconds)
}

/// 列出任意 OpenAI 兼容端点已加载的模型。知识库的 embedding / rerank 配置
/// 与对话模型相互独立，探测时各用各的地址。
pub fn list_models_at(base_url: &str, api_key: &str, timeout_seconds: u64) -> Result<Vec<String>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(timeout_seconds.max(5)))
        .build()
        .context("创建 HTTP 客户端失败")?;
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut request = client.get(url);
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let response = request.send().context("无法连接模型服务")?;
    if !response.status().is_success() {
        bail!("模型服务返回 HTTP {}", response.status());
    }
    let mut models = response
        .json::<ModelsResponse>()
        .context("模型列表格式无法解析")?
        .data
        .into_iter()
        .map(|m| m.id)
        .collect::<Vec<_>>();
    models.sort();
    Ok(models)
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
    match complete_once(config, system, user, temperature, max_tokens, with_switch) {
        Err(ChatError::Rejected(error)) if with_switch => {
            // 只可能是那几个多出来的字段惹的祸：不带它们再发一次。
            THINKING_SWITCH_REJECTED.store(true, Relaxed);
            complete_once(config, system, user, temperature, max_tokens, false).map_err(|second| {
                match second {
                    ChatError::Rejected(second) | ChatError::Other(second) => second.context(
                        format!("（已去掉关闭思考的开关重试；带开关时的首次失败：{error:#}）"),
                    ),
                }
            })
        }
        Err(ChatError::Rejected(error) | ChatError::Other(error)) => Err(error),
        Ok(text) => Ok(text),
    }
}

/// 区分「服务端嫌请求不合法」和别的失败：只有前者值得去掉开关重试。
enum ChatError {
    Rejected(anyhow::Error),
    Other(anyhow::Error),
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
    let mut payload = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": false
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
        let error = anyhow::anyhow!("模型服务返回 HTTP {status}：{body}");
        // 只有 4xx 才可能是「不认识多带的那几个字段」，值得去掉重试；
        // 5xx 是服务端自己的问题，重试也一样。
        return Err(if status.is_client_error() {
            ChatError::Rejected(error)
        } else {
            ChatError::Other(error)
        });
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

    // 正文为空有三种成因，报出来要能直接指向下一步怎么办，而不是笼统一句
    // 「未返回正文」让人去翻服务端日志。
    let thinking = choice
        .message
        .reasoning_content
        .is_some_and(|text| !text.trim().is_empty());
    Err(ChatError::Other(if thinking {
        anyhow::anyhow!(
            "模型只输出了思考过程，没有正文：{max_tokens} 的输出上限被推理占满。\
             应用已自动带上关闭思考的开关（chat_template_kwargs.enable_thinking、\
             think、enable_thinking 三种写法），你的服务端似乎都不认。\
             请在服务端关掉思考模式，或换一个非思考模型来做文字复核。"
        )
    } else if truncated {
        anyhow::anyhow!("模型输出在 {max_tokens} token 处被截断，且截断前没有正文")
    } else {
        anyhow::anyhow!("模型服务未返回正文（choices[0].message.content 为空）")
    }))
}
