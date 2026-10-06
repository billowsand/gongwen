//! Postman 集合（v2.0 / v2.1）与环境文件 → OpenAPI 3.0。
//!
//! - 文件夹 → 标签；`{{baseUrl}}/a/:id` → 服务器地址 + 路径 `/a/{id}`；
//! - 查询参数、请求头里的 `{{变量}}` 做成参数，变量值当样例；写死的请求头做成单值参数；
//! - 鉴权（集合 → 文件夹 → 请求逐层继承）→ `securitySchemes`，凭据提进密钥表；
//! - raw JSON 请求体按例子推断 schema，保存的响应当返回样例并推断返回结构；
//! - 前置脚本、测试脚本不执行，只在报告里说明。

use super::OPENAPI_VERSION;
use super::schema::{identifier, infer};
use crate::agent::api::ApiMethod;
use crate::agent::board::value_to_text;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// 转出来的一个服务（同一个集合里服务器地址不同的分开）。
pub(super) struct Converted {
    pub(super) doc: Value,
}

/// 转换结果。
pub(super) struct Outcome {
    pub(super) services: Vec<Converted>,
    /// 提出来的凭据：密钥名 → 值。
    pub(super) secrets: Vec<(String, String)>,
}

/// 环境文件 → 变量表；标成 secret 的另记。
pub(super) fn env_vars(env: &Value) -> (String, BTreeMap<String, String>) {
    let name = env
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("环境")
        .to_string();
    let vars = env
        .get("values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|v| v.get("enabled").and_then(Value::as_bool) != Some(false))
        .filter_map(|v| {
            Some((
                v.get("key")?.as_str()?.to_string(),
                v.get("value").map(value_to_text).unwrap_or_default(),
            ))
        })
        .collect();
    (name, vars)
}

/// 名字像凭据。
fn secret_like(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "apikey",
        "api_key",
        "api-key",
        "authorization",
        "credential",
        "signature",
    ]
    .iter()
    .any(|word| lower.contains(word))
        || lower.ends_with("key")
}

/// `{{x}}` → Some("x")。
fn whole_var(text: &str) -> Option<&str> {
    let inner = text.trim().strip_prefix("{{")?.strip_suffix("}}")?;
    (!inner.contains("{{")).then_some(inner.trim())
}

/// 把文字里的 `{{x}}` 换成变量值（最多套几层）；没有值的留着。
fn resolve(text: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = text.to_string();
    for _ in 0..4 {
        let mut changed = false;
        for (key, value) in vars {
            let pattern = format!("{{{{{key}}}}}");
            if out.contains(&pattern) {
                out = out.replace(&pattern, value);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

fn description_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => map
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
    .filter(|text| !text.trim().is_empty())
}

/// Base64（凭据里 `用户名:密码` 要编码后放进 Basic 请求头）。
fn base64(input: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// 拆开的地址。
struct Url {
    base: String,
    /// 服务器地址来自哪个变量（没有值时报告用）。
    base_var: Option<String>,
    path: String,
    query: Vec<(String, String, bool, Option<String>)>,
    path_vars: BTreeMap<String, (String, Option<String>)>,
}

fn parse_url(url: &Value, vars: &BTreeMap<String, String>) -> Url {
    let raw = match url {
        Value::String(text) => text.clone(),
        other => other
            .get("raw")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    let raw = raw.split('#').next().unwrap_or_default().trim().to_string();
    let (head, raw_query) = match raw.split_once('?') {
        Some((head, query)) => (head.to_string(), query.to_string()),
        None => (raw.clone(), String::new()),
    };
    // 服务器地址：开头的 {{变量}}，或 协议://主机[:端口]。
    let (base, base_var, rest) = if let Some(after) = head.strip_prefix("{{")
        && let Some(end) = after.find("}}")
    {
        let var = after[..end].trim().to_string();
        let resolved = resolve(&format!("{{{{{var}}}}}"), vars);
        let base = if resolved.contains("{{") {
            String::new()
        } else {
            resolved
        };
        (base, Some(var), after[end + 2..].to_string())
    } else {
        let resolved = resolve(&head, vars);
        let (scheme, rest) = match resolved.split_once("://") {
            Some((scheme, rest)) => (scheme.to_string(), rest.to_string()),
            None => ("http".to_string(), resolved.clone()),
        };
        let at = rest.find('/').unwrap_or(rest.len());
        (
            format!("{scheme}://{}", &rest[..at]),
            None,
            rest[at..].to_string(),
        )
    };
    // 服务器地址里带的路径（`http://h/api`）归服务器地址。
    let base = base.trim_end_matches('/').to_string();
    let mut path = String::new();
    for segment in rest.split('/').filter(|s| !s.is_empty()) {
        path.push('/');
        if let Some(name) = segment.strip_prefix(':') {
            path.push_str(&format!("{{{name}}}"));
        } else if let Some(name) = whole_var(segment) {
            path.push_str(&format!("{{{name}}}"));
        } else {
            path.push_str(segment);
        }
    }
    if path.is_empty() {
        path.push('/');
    }
    let mut query = Vec::new();
    match url.get("query").and_then(Value::as_array) {
        Some(items) => {
            for item in items {
                let Some(key) = item.get("key").and_then(Value::as_str) else {
                    continue;
                };
                query.push((
                    key.to_string(),
                    item.get("value").map(value_to_text).unwrap_or_default(),
                    item.get("disabled").and_then(Value::as_bool) != Some(true),
                    description_of(item.get("description")),
                ));
            }
        }
        None => {
            for pair in raw_query.split('&').filter(|p| !p.is_empty()) {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                query.push((key.to_string(), value.to_string(), true, None));
            }
        }
    }
    let path_vars = url
        .get("variable")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some((
                v.get("key")?.as_str()?.to_string(),
                (
                    v.get("value").map(value_to_text).unwrap_or_default(),
                    description_of(v.get("description")),
                ),
            ))
        })
        .collect();
    Url {
        base,
        base_var,
        path,
        query,
        path_vars,
    }
}

/// 鉴权 → (鉴权方式, 密钥名, 密钥值)。`None` 表示不鉴权。
fn auth_scheme(
    auth: &Value,
    vars: &BTreeMap<String, String>,
    report: &mut Vec<String>,
) -> Option<(Value, String, String)> {
    let kind = auth.get("type").and_then(Value::as_str).unwrap_or("noauth");
    let field = |name: &str| -> Option<String> {
        auth.get(kind)
            .and_then(Value::as_array)?
            .iter()
            .find(|entry| entry.get("key").and_then(Value::as_str) == Some(name))
            .and_then(|entry| entry.get("value"))
            .map(value_to_text)
    };
    // 值是 {{变量}} 时密钥名取变量名，值取变量当前值；写死的值直接提成密钥。
    let secret = |value: &str, fallback: &str| -> (String, String) {
        match whole_var(value) {
            Some(var) => (
                identifier(var, fallback),
                vars.get(var).cloned().unwrap_or_default(),
            ),
            None => (fallback.to_string(), value.to_string()),
        }
    };
    match kind {
        "noauth" => None,
        "bearer" => {
            let (name, value) = secret(&field("token").unwrap_or_default(), "token");
            Some((json!({"type": "http", "scheme": "bearer"}), name, value))
        }
        "apikey" => {
            let key = field("key").unwrap_or_else(|| "X-API-Key".into());
            let place = field("in").unwrap_or_else(|| "header".into());
            let place = if place == "query" { "query" } else { "header" };
            let fallback = crate::agent::api_import::redact::secret_name(&key);
            let (name, value) = secret(&field("value").unwrap_or_default(), &fallback);
            Some((
                json!({"type": "apiKey", "in": place, "name": key}),
                name,
                value,
            ))
        }
        "basic" => {
            let user = resolve(&field("username").unwrap_or_default(), vars);
            let password = resolve(&field("password").unwrap_or_default(), vars);
            Some((
                json!({"type": "http", "scheme": "basic"}),
                "basic".into(),
                base64(&format!("{user}:{password}")),
            ))
        }
        "oauth2" => {
            let (name, value) = secret(&field("accessToken").unwrap_or_default(), "token");
            report.push("OAuth2 鉴权只能粘贴现成的令牌，换令牌的流程不支持".into());
            Some((json!({"type": "http", "scheme": "bearer"}), name, value))
        }
        other => {
            report.push(format!(
                "Postman 鉴权方式 {other} 不支持，相关请求按不鉴权导入"
            ));
            None
        }
    }
}

/// 一个服务在转换中的状态。
struct Building {
    base: String,
    base_var: Option<String>,
    paths: Map<String, Value>,
    schemes: Map<String, Value>,
}

/// 转换。`vars` 是集合变量与环境变量（环境的覆盖集合的）。
pub(super) fn convert(
    collection: &Value,
    env: &BTreeMap<String, String>,
    report: &mut Vec<String>,
) -> Outcome {
    let mut vars: BTreeMap<String, String> = collection
        .get("variable")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some((
                v.get("key")?.as_str()?.to_string(),
                v.get("value").map(value_to_text).unwrap_or_default(),
            ))
        })
        .collect();
    vars.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
    let title = collection
        .pointer("/info/name")
        .and_then(Value::as_str)
        .unwrap_or("Postman 集合")
        .to_string();
    let mut state = State {
        vars,
        services: Vec::new(),
        secrets: Vec::new(),
        scripts: 0,
        skipped: Vec::new(),
    };
    let auth = collection.get("auth").cloned();
    count_scripts(collection, &mut state.scripts);
    walk(
        collection.get("item"),
        &[],
        auth.as_ref(),
        &mut state,
        report,
    );
    if state.scripts > 0 {
        report.push(format!(
            "{} 段前置 / 测试脚本没有执行（本程序不跑脚本）",
            state.scripts
        ));
    }
    if !state.skipped.is_empty() {
        report.push(format!(
            "没收的请求：{}",
            state
                .skipped
                .iter()
                .take(6)
                .cloned()
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    let many = state.services.len() > 1;
    let services = state
        .services
        .into_iter()
        .map(|building| {
            if building.base.is_empty() {
                report.push(match &building.base_var {
                    Some(var) => format!(
                        "服务器地址变量 {{{{{var}}}}} 没有值：一起导入环境文件，或保存后在接口地址里补上"
                    ),
                    None => "有请求没写服务器地址，保存后在接口地址里补上".into(),
                });
            }
            let name = if many && !building.base.is_empty() {
                format!("{title}（{}）", building.base)
            } else {
                title.clone()
            };
            let mut doc = json!({
                "openapi": OPENAPI_VERSION,
                "info": {"title": name, "version": "1.0"},
                "paths": building.paths,
            });
            if let Some(description) = description_of(collection.pointer("/info/description")) {
                doc["info"]["description"] = json!(description);
            }
            if !building.base.is_empty() {
                doc["servers"] = json!([{ "url": building.base }]);
            }
            if !building.schemes.is_empty() {
                doc["components"] = json!({"securitySchemes": building.schemes});
            }
            Converted { doc }
        })
        .collect();
    Outcome {
        services,
        secrets: state.secrets,
    }
}

struct State {
    vars: BTreeMap<String, String>,
    services: Vec<Building>,
    secrets: Vec<(String, String)>,
    scripts: usize,
    skipped: Vec<String>,
}

fn count_scripts(value: &Value, count: &mut usize) {
    match value {
        Value::Object(map) => {
            if let Some(events) = map.get("event").and_then(Value::as_array) {
                *count += events
                    .iter()
                    .filter(|e| {
                        e.pointer("/script/exec").is_some_and(|exec| match exec {
                            Value::Array(lines) => lines
                                .iter()
                                .any(|l| l.as_str().is_some_and(|l| !l.trim().is_empty())),
                            Value::String(text) => !text.trim().is_empty(),
                            _ => false,
                        })
                    })
                    .count();
            }
            if let Some(items) = map.get("item") {
                count_scripts(items, count);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| count_scripts(item, count)),
        _ => {}
    }
}

fn walk(
    items: Option<&Value>,
    folders: &[String],
    auth: Option<&Value>,
    state: &mut State,
    report: &mut Vec<String>,
) {
    for item in items.and_then(Value::as_array).into_iter().flatten() {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let own_auth = item.get("auth").or(auth);
        if item.get("item").is_some() {
            let mut path = folders.to_vec();
            path.push(name);
            walk(item.get("item"), &path, own_auth, state, report);
            continue;
        }
        if let Some(request) = item.get("request") {
            let auth = request.get("auth").or(own_auth);
            request_op(item, request, &name, folders, auth, state, report);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn request_op(
    item: &Value,
    request: &Value,
    name: &str,
    folders: &[String],
    auth: Option<&Value>,
    state: &mut State,
    report: &mut Vec<String>,
) {
    let (method_text, url) = match request {
        Value::String(url) => ("GET".to_string(), Value::String(url.clone())),
        other => (
            other
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .to_string(),
            other.get("url").cloned().unwrap_or(Value::Null),
        ),
    };
    let Some(method) = ApiMethod::from_key(&method_text) else {
        state
            .skipped
            .push(format!("{name}（{method_text} 不支持）"));
        return;
    };
    let url = parse_url(&url, &state.vars);
    let vars = state.vars.clone();
    let mut params: Vec<Value> = Vec::new();
    let mut requirement = Map::new();
    let mut schemes_to_add: Vec<(String, Value)> = Vec::new();

    // 路径参数
    for caps in url
        .path
        .split('/')
        .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
    {
        let (value, description) = url.path_vars.get(caps).cloned().unwrap_or_default();
        let value = resolve(&value, &vars);
        let mut param =
            json!({"name": caps, "in": "path", "required": true, "schema": {"type": "string"}});
        if !value.is_empty() && !value.contains("{{") {
            param["example"] = json!(value);
        } else if let Some(example) = vars.get(caps) {
            param["example"] = json!(example);
        }
        if let Some(description) = description {
            param["description"] = json!(description);
        }
        params.push(param);
    }
    // 查询参数
    for (key, value, enabled, description) in &url.query {
        if let Some(var) = whole_var(value)
            && secret_like(var)
        {
            let secret = identifier(var, "key");
            let scheme = json!({"type": "apiKey", "in": "query", "name": key});
            schemes_to_add.push((secret.clone(), scheme));
            requirement.insert(secret.clone(), json!([]));
            state
                .secrets
                .push((secret, vars.get(var).cloned().unwrap_or_default()));
            continue;
        }
        let example = resolve(value, &vars);
        let mut param = json!({"name": key, "in": "query", "schema": {"type": "string"}});
        if *enabled {
            param["required"] = json!(true);
        }
        if !example.is_empty() && !example.contains("{{") {
            param["example"] = json!(example);
        }
        if let Some(description) = description {
            param["description"] = json!(description);
        }
        params.push(param);
    }
    // 请求头
    for header in request
        .get("header")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|h| h.get("disabled").and_then(Value::as_bool) != Some(true))
    {
        let Some(key) = header.get("key").and_then(Value::as_str) else {
            continue;
        };
        let value = header.get("value").map(value_to_text).unwrap_or_default();
        let lower = key.to_ascii_lowercase();
        if matches!(lower.as_str(), "content-type" | "accept") {
            continue;
        }
        let bearer = lower == "authorization"
            && value
                .get(..7)
                .is_some_and(|p| p.eq_ignore_ascii_case("bearer "));
        if bearer {
            let token = value[7..].trim();
            let (secret, secret_value) = match whole_var(token) {
                Some(var) => (
                    identifier(var, "token"),
                    vars.get(var).cloned().unwrap_or_default(),
                ),
                None => ("token".to_string(), token.to_string()),
            };
            schemes_to_add.push((secret.clone(), json!({"type": "http", "scheme": "bearer"})));
            requirement.insert(secret.clone(), json!([]));
            state.secrets.push((secret, secret_value));
            continue;
        }
        if lower == "authorization"
            || secret_like(key)
            || whole_var(&value).is_some_and(secret_like)
        {
            let fallback = crate::agent::api_import::redact::secret_name(key);
            let (secret, secret_value) = match whole_var(&value) {
                Some(var) => (
                    identifier(var, &fallback),
                    vars.get(var).cloned().unwrap_or_default(),
                ),
                None => (fallback, value.clone()),
            };
            schemes_to_add.push((
                secret.clone(),
                json!({"type": "apiKey", "in": "header", "name": key}),
            ));
            requirement.insert(secret.clone(), json!([]));
            state.secrets.push((secret, secret_value));
            continue;
        }
        let mut param = json!({"name": key, "in": "header", "required": true});
        match whole_var(&value) {
            Some(var) => {
                param["schema"] = json!({"type": "string"});
                if let Some(example) = vars.get(var) {
                    param["example"] = json!(example);
                }
            }
            None => param["schema"] = json!({"type": "string", "enum": [resolve(&value, &vars)]}),
        }
        if let Some(description) = description_of(header.get("description")) {
            param["description"] = json!(description);
        }
        params.push(param);
    }
    // 鉴权
    if requirement.is_empty()
        && let Some(auth) = auth
        && let Some((scheme, secret, value)) = auth_scheme(auth, &vars, report)
    {
        schemes_to_add.push((secret.clone(), scheme));
        requirement.insert(secret.clone(), json!([]));
        state.secrets.push((secret, value));
    }

    let mut op = Map::new();
    if !name.is_empty() {
        op.insert("summary".into(), json!(name));
    }
    if let Some(description) = description_of(request.get("description")) {
        op.insert("description".into(), json!(description));
    }
    if !folders.is_empty() {
        op.insert("tags".into(), json!([folders.join(" / ")]));
    }
    if !params.is_empty() {
        op.insert("parameters".into(), Value::Array(params));
    }
    if let Some(body) = request.get("body")
        && let Some(request_body) = body_of(body, &vars, report, name)
    {
        op.insert("requestBody".into(), request_body);
    }
    if !requirement.is_empty() {
        op.insert("security".into(), json!([requirement]));
    }
    op.insert("responses".into(), responses_of(item));

    let index = match state.services.iter().position(|s| s.base == url.base) {
        Some(index) => index,
        None => {
            state.services.push(Building {
                base: url.base.clone(),
                base_var: url.base_var.clone(),
                paths: Map::new(),
                schemes: Map::new(),
            });
            state.services.len() - 1
        }
    };
    let building = &mut state.services[index];
    for (secret, scheme) in schemes_to_add {
        building.schemes.entry(secret).or_insert(scheme);
    }
    let item_entry = building
        .paths
        .entry(url.path.clone())
        .or_insert_with(|| json!({}));
    if item_entry.get(method.key()).is_some() {
        state.skipped.push(format!(
            "{name}（和别的请求同为 {} {}）",
            method.label(),
            url.path
        ));
        return;
    }
    item_entry[method.key()] = Value::Object(op);
}

fn body_of(
    body: &Value,
    vars: &BTreeMap<String, String>,
    report: &mut Vec<String>,
    name: &str,
) -> Option<Value> {
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or_default();
    match mode {
        "raw" => {
            let raw = body.get("raw").and_then(Value::as_str).unwrap_or_default();
            if raw.trim().is_empty() {
                return None;
            }
            let resolved = resolve(raw, vars);
            match serde_json::from_str::<Value>(&resolved) {
                Ok(example) => Some(json!({
                    "required": true,
                    "content": {"application/json": {"schema": infer(&example), "example": example}},
                })),
                Err(_) => {
                    let language = body
                        .pointer("/options/raw/language")
                        .and_then(Value::as_str)
                        .unwrap_or("text");
                    let content_type = match language {
                        "json" => "application/json",
                        "xml" => "application/xml",
                        _ => "text/plain",
                    };
                    if content_type == "application/json" {
                        report.push(format!("请求「{name}」的 JSON 请求体解析不了，按文字导入"));
                    }
                    Some(json!({
                        "content": {content_type: {"schema": {"type": "string"}, "example": raw}},
                    }))
                }
            }
        }
        "urlencoded" | "formdata" => {
            let mut properties = Map::new();
            let mut required = Vec::new();
            for field in body
                .get(mode)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(key) = field.get("key").and_then(Value::as_str) else {
                    continue;
                };
                let mut schema = if field.get("type").and_then(Value::as_str) == Some("file") {
                    json!({"type": "string", "format": "binary"})
                } else {
                    json!({"type": "string"})
                };
                let value = resolve(
                    &field.get("value").map(value_to_text).unwrap_or_default(),
                    vars,
                );
                if !value.is_empty() && !value.contains("{{") {
                    schema["example"] = json!(value);
                }
                if let Some(description) = description_of(field.get("description")) {
                    schema["description"] = json!(description);
                }
                if field.get("disabled").and_then(Value::as_bool) != Some(true) {
                    required.push(json!(key));
                }
                properties.insert(key.to_string(), schema);
            }
            let content_type = if mode == "urlencoded" {
                "application/x-www-form-urlencoded"
            } else {
                "multipart/form-data"
            };
            let mut schema = json!({"type": "object", "properties": properties});
            if !required.is_empty() {
                schema["required"] = Value::Array(required);
            }
            Some(json!({"required": true, "content": {content_type: {"schema": schema}}}))
        }
        "" => None,
        other => {
            report.push(format!(
                "请求「{name}」的请求体写法 {other} 不支持，没有导入请求体"
            ));
            None
        }
    }
}

/// 保存的响应 → 返回样例与推断的返回结构。
fn responses_of(item: &Value) -> Value {
    let mut responses = Map::new();
    for response in item
        .get("response")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let code = response
            .get("code")
            .and_then(Value::as_u64)
            .unwrap_or(200)
            .to_string();
        if responses.contains_key(&code) {
            continue;
        }
        let description = response
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("返回")
            .to_string();
        let body = response
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut entry = json!({"description": description});
        if let Ok(example) = serde_json::from_str::<Value>(body) {
            entry["content"] =
                json!({"application/json": {"schema": infer(&example), "example": example}});
        } else if !body.trim().is_empty() {
            entry["content"] =
                json!({"text/plain": {"schema": {"type": "string"}, "example": body}});
        }
        responses.insert(code, entry);
    }
    if responses.is_empty() {
        responses.insert("200".into(), json!({"description": "成功"}));
    }
    Value::Object(responses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64("user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64("a"), "YQ==");
        assert_eq!(base64("ab"), "YWI=");
        assert_eq!(base64(""), "");
    }

    #[test]
    fn url_with_variable_base_and_path_vars() {
        let vars = BTreeMap::from([(
            "baseUrl".to_string(),
            "http://10.0.0.8:8080/api/".to_string(),
        )]);
        let url = parse_url(
            &json!({"raw": "{{baseUrl}}/stat/:region?year=2025&k={{key}}"}),
            &vars,
        );
        assert_eq!(url.base, "http://10.0.0.8:8080/api");
        assert_eq!(url.path, "/stat/{region}");
        assert_eq!(url.query.len(), 2);
        let literal = parse_url(&json!("https://h.example.com/a/b?x=1"), &BTreeMap::new());
        assert_eq!(literal.base, "https://h.example.com");
        assert_eq!(literal.path, "/a/b");
    }
}
