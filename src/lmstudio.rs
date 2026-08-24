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
) -> Result<String> {
    match generate_with(config, system, user, temperature, max_tokens) {
        Ok(text) => Ok(text),
        Err(first) => generate_with(config, system, user, temperature, max_tokens)
            .with_context(|| format!("重试前的首次失败：{first:#}")),
    }
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
    if config.model.trim().is_empty() {
        bail!("请先在设置中选择模型");
    }
    let payload = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": false
    });
    let mut request = client(config)?
        .post(endpoint(config, "chat/completions"))
        .json(&payload);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }
    let response = request.send().context("调用模型服务失败")?;
    let status = response.status();
    let body = response.text().context("读取模型服务响应失败")?;
    if !status.is_success() {
        bail!("模型服务返回 HTTP {}：{}", status, body);
    }
    let parsed: ChatResponse =
        serde_json::from_str(&body).context("模型服务响应不是兼容的 Chat Completions 格式")?;
    let Some(choice) = parsed.choices.into_iter().next() else {
        bail!("模型服务返回了空的 choices，请检查模型是否已加载");
    };
    let truncated = choice.finish_reason.as_deref() == Some("length");
    let content = choice.message.content.unwrap_or_default();
    if !content.trim().is_empty() {
        return Ok(content);
    }

    // 正文为空有三种成因，给的话要能直接指向下一步怎么办，而不是笼统一句
    // 「未返回正文」让人去翻服务端日志。
    let thinking = choice
        .message
        .reasoning_content
        .is_some_and(|text| !text.trim().is_empty());
    if thinking {
        bail!(
            "模型只输出了思考过程，没有正文。多半是思考模式（如 Qwen3 的 thinking）\
             没关，{max_tokens} 的输出上限被推理占满。请在模型服务里关掉思考模式\
             （Ollama 加 think=false，vLLM/SGLang 传 enable_thinking=false，\
             LM Studio 在模型加载参数里关），或换一个非思考模型来做文字复核。"
        );
    }
    if truncated {
        bail!("模型输出在 {max_tokens} token 处被截断，且截断前没有正文");
    }
    bail!("模型服务未返回正文（choices[0].message.content 为空）")
}
