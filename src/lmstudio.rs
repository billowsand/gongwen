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
}

#[derive(Debug, Deserialize)]
struct Message {
    content: Option<String>,
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
    parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .filter(|s| !s.trim().is_empty())
        .context("模型服务未返回正文")
}
