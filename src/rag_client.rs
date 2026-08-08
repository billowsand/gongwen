//! 知识库的 HTTP 客户端：embedding 与 rerank。复刻 `lmstudio.rs` 的
//! client/endpoint/bearer 模式，reqwest blocking、同步调用。
//!
//! embedding 走 OpenAI 兼容 `POST {base_url}/embeddings`，一次可传多条 input；
//! rerank 不是 OpenAI 标准接口，各家（LM Studio / Jina / Cohere / TEI）路径与
//! 响应字段不一，故用 `serde_json::Value` 按 `RerankConfig` 里的可配键取值。

use crate::models::{EmbeddingConfig, RerankConfig};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
    index: usize,
}

fn client(timeout_seconds: u64) -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(timeout_seconds.max(5)))
        .build()
        .context("创建 HTTP 客户端失败")
}

fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}/{}", base_url.trim_end_matches('/'), path)
}

/// 批量取向量，返回与 texts 等长的向量列表（按输入顺序）。
pub fn embed(config: &EmbeddingConfig, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if config.model.trim().is_empty() {
        bail!("请先在设置中配置 embedding 模型");
    }
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let payload = json!({
        "model": config.model,
        "input": texts,
    });
    let mut request = client(config.timeout_seconds)?
        .post(endpoint(&config.base_url, "embeddings"))
        .json(&payload);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }
    let response = request.send().context("调用 embedding 服务失败")?;
    let status = response.status();
    let body = response.text().context("读取 embedding 响应失败")?;
    if !status.is_success() {
        bail!("embedding 服务返回 HTTP {}：{}", status, body);
    }
    let mut parsed: EmbeddingResponse =
        serde_json::from_str(&body).context("embedding 响应不是兼容格式")?;
    parsed.data.sort_by_key(|item| item.index);
    if parsed.data.len() != texts.len() {
        bail!(
            "embedding 返回条数 {} 与请求 {} 不一致",
            parsed.data.len(),
            texts.len()
        );
    }
    Ok(parsed.data.into_iter().map(|item| item.embedding).collect())
}

/// 用专用 rerank 端点给候选文档按与 query 的相关度重排。
/// 返回 (原下标, 相关分)，已按分降序。请求体取最广泛兼容的
/// {model, query, documents, [top_n]}；响应字段名走 config 可配键。
pub fn rerank(
    config: &RerankConfig,
    query: &str,
    documents: &[String],
    top_n: usize,
) -> Result<Vec<(usize, f32)>> {
    if config.model.trim().is_empty() {
        bail!("rerank 模型未配置");
    }
    if documents.is_empty() {
        return Ok(Vec::new());
    }
    let mut payload = json!({
        "model": config.model,
        "query": query,
        "documents": documents,
    });
    if !config.top_n_field.trim().is_empty() {
        payload[config.top_n_field.trim()] = json!(top_n.max(1));
    }
    let mut request = client(config.timeout_seconds)?
        .post(endpoint(&config.base_url, config.path.trim()))
        .json(&payload);
    if !config.api_key.trim().is_empty() {
        request = request.bearer_auth(config.api_key.trim());
    }
    let response = request.send().context("调用 rerank 服务失败")?;
    let status = response.status();
    let body = response.text().context("读取 rerank 响应失败")?;
    if !status.is_success() {
        bail!("rerank 服务返回 HTTP {}：{}", status, body);
    }
    parse_rerank_response(config, &body)
}

/// 送进 LLM 评分的候选片段截断长度。判相关度不需要读全文，截短能显著提速。
const LLM_RERANK_SNIPPET_CHARS: usize = 300;

/// 用对话大模型给候选片段打相关分，作为专用 rerank 端点的替代。
///
/// LM Studio 至今不提供 rerank 端点，而重排对检索质量的影响又实打实。
/// 这里复用已配好的对话模型：一次调用给全部候选打分（listwise），比逐条
/// 打分（pointwise）少 N 倍往返，也让模型能横向比较。
///
/// 返回 (原下标, 0..1 相关分)，按分降序。解析失败即报错，由调用方降级到
/// 融合分排序并提示用户——绝不静默吞掉。
pub fn rerank_with_llm(
    chat: &crate::models::LmStudioConfig,
    query: &str,
    documents: &[String],
    top_n: usize,
) -> Result<Vec<(usize, f32)>> {
    if documents.is_empty() {
        return Ok(Vec::new());
    }
    let system = "你是公文检索系统的相关性评审。只输出 JSON，不要解释、不要 Markdown 代码块。";
    let mut user = String::with_capacity(documents.len() * LLM_RERANK_SNIPPET_CHARS + 512);
    user.push_str("给定【检索需求】和若干【候选片段】，为每个候选片段给出 0-100 的相关分：\n");
    user.push_str(
        "100 = 直接可作为写作参考，0 = 完全无关。只看与检索需求的相关性，不评价文字好坏。\n\n",
    );
    user.push_str("【检索需求】\n");
    user.push_str(query.trim());
    user.push_str("\n\n【候选片段】\n");
    for (i, doc) in documents.iter().enumerate() {
        let snippet: String = doc.chars().take(LLM_RERANK_SNIPPET_CHARS).collect();
        user.push_str(&format!("[{i}] {}\n\n", snippet.replace('\n', " ")));
    }
    user.push_str(&format!(
        "输出一个 JSON 数组，对全部 {} 个候选各给一项，形如：\n[{{\"index\":0,\"score\":85}},{{\"index\":1,\"score\":20}}]",
        documents.len()
    ));

    // 低温 + 短输出：打分要稳定可复现，且只需要几十个 token。
    let max_tokens = (documents.len() as u32 * 24).clamp(128, 2048);
    let raw = crate::lmstudio::generate_with(chat, system, &user, 0.0, max_tokens)
        .context("调用对话模型重排失败")?;
    let mut scored = parse_llm_rerank(&raw, documents.len())?;
    scored.truncate(top_n.max(1));
    Ok(scored)
}

/// 从模型回复里抠出评分数组。模型常把 JSON 包在 ```json 围栏或前后闲话里，
/// 所以取第一个 `[` 到与之配对的 `]`，而不是直接整体反序列化。
fn parse_llm_rerank(raw: &str, count: usize) -> Result<Vec<(usize, f32)>> {
    let array = extract_json_array(raw)
        .with_context(|| format!("对话模型未按要求输出 JSON 数组：{}", preview(raw)))?;
    let value: Value = serde_json::from_str(&array)
        .with_context(|| format!("对话模型输出的 JSON 无法解析：{}", preview(&array)))?;
    let items = value.as_array().context("对话模型输出的不是 JSON 数组")?;
    let mut out: Vec<(usize, f32)> = Vec::with_capacity(items.len());
    let mut seen = std::collections::HashSet::new();
    for item in items {
        let Some(index) = item
            .get("index")
            .and_then(Value::as_u64)
            .map(|i| i as usize)
        else {
            continue;
        };
        // 越界下标直接丢弃，别让它错位映射到别的片段。
        if index >= count || !seen.insert(index) {
            continue;
        }
        let score = item
            .get("score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .clamp(0.0, 100.0) as f32
            / 100.0;
        out.push((index, score));
    }
    if out.is_empty() {
        bail!("对话模型没有给出任何有效评分：{}", preview(raw));
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// 取出第一个方括号包住的片段，按嵌套深度配对，字符串内的括号不算。
fn extract_json_array(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..start + offset + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// 截短错误信息里回显的模型输出，避免整段塞进状态栏。
fn preview(text: &str) -> String {
    let flat = text.trim().replace('\n', " ");
    let short: String = flat.chars().take(120).collect();
    if flat.chars().count() > 120 {
        format!("{short}…")
    } else {
        short
    }
}

/// 真发一次最小 rerank 请求，验证**端点路径与响应字段**都对得上。
///
/// 光用 `/v1/models` 探测是不够的：LM Studio 遇到不认识的路径会打日志
/// `Unexpected endpoint or method` 却照样返回 200，于是"连接成功"，实际
/// 每次 rerank 都在静默失败。这里必须走完整条链路才算数。
pub fn probe_rerank(config: &RerankConfig) -> Result<usize> {
    let ranked = rerank(config, PROBE_QUERY, &probe_docs(), 2)?;
    check_probe(&ranked)
}

/// 同样的探测，但走对话大模型重排。
pub fn probe_rerank_llm(chat: &crate::models::LmStudioConfig) -> Result<usize> {
    let ranked = rerank_with_llm(chat, PROBE_QUERY, &probe_docs(), 2)?;
    check_probe(&ranked)
}

const PROBE_QUERY: &str = "安全生产检查";

fn probe_docs() -> Vec<String> {
    vec![
        "关于开展安全生产大检查的通知".to_string(),
        "关于组织义务植树活动的函".to_string(),
    ]
}

fn check_probe(ranked: &[(usize, f32)]) -> Result<usize> {
    if ranked.is_empty() {
        bail!("重排返回了空结果，请检查端点路径与模型是否为重排模型");
    }
    if ranked.iter().any(|(index, _)| *index >= 2) {
        bail!("重排返回的下标超出候选范围，响应字段可能对应错了");
    }
    Ok(ranked.len())
}

/// 按 config 可配键解析 rerank 响应，适配各家字段命名。
fn parse_rerank_response(config: &RerankConfig, body: &str) -> Result<Vec<(usize, f32)>> {
    let value: Value = serde_json::from_str(body).context("rerank 响应不是 JSON")?;
    let results = value
        .get(config.response_results_key.trim())
        .and_then(Value::as_array)
        .with_context(|| {
            format!(
                "rerank 响应缺少结果数组字段 `{}`",
                config.response_results_key
            )
        })?;
    let mut out = Vec::with_capacity(results.len());
    for item in results {
        let index = item
            .get(config.response_index_key.trim())
            .and_then(Value::as_u64)
            .with_context(|| format!("rerank 结果缺少下标字段 `{}`", config.response_index_key))?
            as usize;
        let score = item
            .get(config.response_score_key.trim())
            .and_then(Value::as_f64)
            .with_context(|| format!("rerank 结果缺少分数字段 `{}`", config.response_score_key))?
            as f32;
        out.push((index, score));
    }
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RerankConfig {
        RerankConfig::default()
    }

    #[test]
    fn parse_jina_style_response() {
        let body =
            r#"{"results":[{"index":2,"relevance_score":0.9},{"index":0,"relevance_score":0.4}]}"#;
        let parsed = parse_rerank_response(&cfg(), body).unwrap();
        assert_eq!(parsed[0], (2, 0.9f32));
        assert_eq!(parsed[1], (0, 0.4f32));
    }

    #[test]
    fn parse_custom_keys() {
        let mut c = cfg();
        c.response_results_key = "data".into();
        c.response_index_key = "idx".into();
        c.response_score_key = "score".into();
        let body = r#"{"data":[{"idx":1,"score":0.7}]}"#;
        let parsed = parse_rerank_response(&c, body).unwrap();
        assert_eq!(parsed, vec![(1, 0.7f32)]);
    }

    #[test]
    fn parse_missing_results_key_errors() {
        let body = r#"{"unexpected":[]}"#;
        assert!(parse_rerank_response(&cfg(), body).is_err());
    }

    // ---------- 用对话大模型重排 ----------

    #[test]
    fn llm_rerank_parses_plain_json() {
        let raw = r#"[{"index":2,"score":90},{"index":0,"score":30},{"index":1,"score":10}]"#;
        let parsed = parse_llm_rerank(raw, 3).unwrap();
        assert_eq!(parsed[0], (2, 0.9f32), "应按分降序");
        assert_eq!(parsed[2].0, 1);
    }

    /// 模型爱把 JSON 包在 ```json 围栏里，或前后加一句解释。
    #[test]
    fn llm_rerank_tolerates_fences_and_chatter() {
        let raw = "好的，评分如下：\n```json\n[{\"index\":0,\"score\":75}]\n```\n希望有帮助。";
        let parsed = parse_llm_rerank(raw, 1).unwrap();
        assert_eq!(parsed, vec![(0, 0.75f32)]);
    }

    /// 越界或重复的下标必须丢掉——否则会错位映射到别的片段，
    /// 把不相干的内容当成高相关参考注入提示词。
    #[test]
    fn llm_rerank_drops_out_of_range_and_duplicate_indexes() {
        let raw = r#"[{"index":0,"score":80},{"index":9,"score":95},{"index":0,"score":10}]"#;
        let parsed = parse_llm_rerank(raw, 2).unwrap();
        assert_eq!(parsed, vec![(0, 0.8f32)]);
    }

    #[test]
    fn llm_rerank_clamps_scores() {
        let raw = r#"[{"index":0,"score":500},{"index":1,"score":-20}]"#;
        let parsed = parse_llm_rerank(raw, 2).unwrap();
        assert_eq!(parsed[0], (0, 1.0f32));
        assert_eq!(parsed[1], (1, 0.0f32));
    }

    /// 解析不出来要报错，让上层降级到融合分排序并提示用户，绝不静默返回空。
    #[test]
    fn llm_rerank_errors_on_unusable_output() {
        assert!(
            parse_llm_rerank("我无法完成这个任务。", 3).is_err(),
            "没有数组应报错"
        );
        assert!(parse_llm_rerank("[]", 3).is_err(), "空数组应报错");
        assert!(
            parse_llm_rerank(r#"[{"idx":0,"score":5}]"#, 3).is_err(),
            "字段名不对应报错"
        );
    }

    #[test]
    fn extract_json_array_handles_nesting_and_strings() {
        assert_eq!(
            extract_json_array("前言 [1, [2, 3]] 后记").unwrap(),
            "[1, [2, 3]]"
        );
        // 字符串里的方括号不参与配对。
        assert_eq!(
            extract_json_array(r#"[{"t":"a]b"}]"#).unwrap(),
            r#"[{"t":"a]b"}]"#
        );
        assert!(extract_json_array("没有数组").is_none());
        assert!(extract_json_array("[未闭合").is_none());
    }

    #[test]
    fn embedding_item_deserializes() {
        let body = r#"{"data":[{"embedding":[0.1,0.2],"index":0}],"model":"m"}"#;
        let parsed: EmbeddingResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.data[0].embedding, vec![0.1, 0.2]);
    }
}
