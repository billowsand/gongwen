//! 编译：OpenAPI 操作 → 运行时的 [`ApiEndpoint`]。
//!
//! 参数、请求体字段做成输入变量，地址、请求头、请求体按变量拼成模板；`enum` 只有一个值的
//! 参数是写死的，直接写进模板。鉴权按 `securitySchemes` 写成 `{secret:名字}`。表达不了的
//! 部分在 `x-gongwen-template` 里留着原样模板（只在写回时出现，见 [`super::raise`]）。

use super::schema::{
    self, deref, example_of, example_text, fixed, flatten, identifier, kind_of, resolve_deep,
};
use super::{Service, ServiceEnv, default_id, unique};
use crate::agent::api::{
    ApiDestination, ApiEndpoint, ApiExample, ApiHeader, ApiInput, ApiMapping, ApiMethod,
    ApiSuccess, BodyKind, DEFAULT_TIMEOUT, InputKind, OpOrigin, percent_encode,
};
use crate::agent::board::value_to_text;
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

pub(super) const X_ID: &str = "x-gongwen-id";
pub(super) const X_NAME: &str = "x-gongwen-name";
pub(super) const X_READONLY: &str = "x-gongwen-readonly";
pub(super) const X_AI: &str = "x-gongwen-ai";
pub(super) const X_SUCCESS: &str = "x-gongwen-success";
pub(super) const X_EVIDENCE: &str = "x-gongwen-evidence";
pub(super) const X_DESTINATION: &str = "x-gongwen-destination";
pub(super) const X_TIMEOUT: &str = "x-gongwen-timeout";
pub(super) const X_TESTS: &str = "x-gongwen-tests";
pub(super) const X_SECRET: &str = "x-gongwen-secret";
pub(super) const X_INPUTS: &str = "x-gongwen-inputs";
pub(super) const X_TEMPLATE: &str = "x-gongwen-template";

/// 路径里的参数：`/stat/{region}`。
static PATH_PARAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^{}]+)\}").expect("路径参数正则"));

/// 服务当前的服务器地址：环境里配的优先，没有就用 `servers` 第一个（变量取默认值）。
/// 结尾不带 `/`。
pub(crate) fn base_url(service: &Service, env: Option<&ServiceEnv>) -> String {
    let current = env.and_then(ServiceEnv::current);
    if let Some(env) = current
        && !env.base_url.trim().is_empty()
    {
        return env.base_url.trim().trim_end_matches('/').to_string();
    }
    let Some(server) = service.doc.pointer("/servers/0") else {
        return String::new();
    };
    let mut url = server
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if let Some(variables) = server.get("variables").and_then(Value::as_object) {
        for (name, variable) in variables {
            let value = current
                .and_then(|env| env.variables.get(name).cloned())
                .or_else(|| variable.get("default").map(value_to_text))
                .unwrap_or_default();
            url = url.replace(&format!("{{{name}}}"), &value);
        }
    }
    url.trim_end_matches('/').to_string()
}

/// 编译一个服务的全部操作。
pub(super) fn lower_service(service: &Service, env: Option<&ServiceEnv>) -> Vec<ApiEndpoint> {
    let base = base_url(service, env);
    service
        .operations()
        .into_iter()
        .filter_map(|(path, method)| lower_op(service, &base, &path, method))
        .collect()
}

/// 按方法与地址默认算不算只查询。
pub(super) fn default_readonly(method: ApiMethod, path: &str) -> bool {
    method.reads() && crate::agent::api_import::write_word(path).is_none()
}

/// 鉴权方式没写 `x-gongwen-secret` 时用的密钥名。
pub(super) fn secret_of(scheme_name: &str, scheme: &Value) -> String {
    scheme
        .get(X_SECRET)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| crate::agent::api_import::redact::secret_name(scheme_name))
}

/// JSON 指针里的一段要转义。
pub(super) fn pointer_token(text: &str) -> String {
    text.replace('~', "~0").replace('/', "~1")
}

/// 根上的业务成功判据。
pub(super) fn success_of(value: Option<&Value>) -> ApiSuccess {
    let Some(value) = value else {
        return ApiSuccess::default();
    };
    let equals = match value.get("equals") {
        Some(Value::Array(items)) => items
            .iter()
            .map(value_to_text)
            .collect::<Vec<_>>()
            .join("|"),
        Some(other) => value_to_text(other),
        None => String::new(),
    };
    ApiSuccess {
        pointer: value
            .get("pointer")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        equals,
    }
}

/// 超时：操作上的、根上的，都没有用默认值。
pub(super) fn timeout_of(op: &Value, doc: &Value) -> u64 {
    op.get(X_TIMEOUT)
        .or_else(|| doc.get(X_TIMEOUT))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// 拼模板时的中间状态。
struct Builder {
    inputs: Vec<ApiInput>,
    notes: Vec<String>,
    query: Vec<String>,
    headers: Vec<ApiHeader>,
    cookies: Vec<String>,
}

impl Builder {
    /// 加一个输入变量，返回变量名（与已有的重名时加后缀）。
    fn add_input(
        &mut self,
        wire: &str,
        preferred: Option<&str>,
        schema: Value,
        holder: &Value,
        required: bool,
        fallback_example: Option<&Value>,
    ) -> String {
        let kind = kind_of(&schema);
        let description = holder
            .get("description")
            .or_else(|| schema.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let example = example_of(holder, &schema)
            .or_else(|| fallback_example.cloned())
            .map(|value| example_text(kind, &value))
            .unwrap_or_default();
        let fallback = format!("p{}", self.inputs.len() + 1);
        let base = match preferred {
            Some(name) if valid_identifier(name) => name.to_string(),
            _ => identifier(wire, &fallback),
        };
        let name = unique(
            &base,
            |candidate| self.inputs.iter().any(|input| input.name == candidate),
            "_",
        );
        self.inputs.push(ApiInput {
            name: name.clone(),
            kind,
            description,
            required,
            example,
            comma: false,
            schema: Some(schema),
            wire: wire.to_string(),
        });
        name
    }
}

pub(super) fn valid_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 写死的值在地址、请求头里的文字。
fn literal_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 参数的 schema：`schema` 或 `content` 里第一个的 schema，引用展开。
fn param_schema(doc: &Value, param: &Value) -> Value {
    let raw = param.get("schema").or_else(|| {
        param
            .get("content")
            .and_then(Value::as_object)
            .and_then(|content| content.values().next())
            .and_then(|media| media.get("schema"))
    });
    raw.map(|schema| flatten(&resolve_deep(doc, schema)))
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// 路径级与操作级的参数合并（操作级按 名字 + 位置 覆盖路径级），引用展开。
fn merged_params(doc: &Value, item: &Value, op: &Value) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    for list in [item.get("parameters"), op.get("parameters")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
    {
        for param in list {
            let param = deref(doc, param).clone();
            let key = (param.get("name").cloned(), param.get("in").cloned());
            match params
                .iter_mut()
                .find(|p| (p.get("name").cloned(), p.get("in").cloned()) == key)
            {
                Some(existing) => *existing = param,
                None => params.push(param),
            }
        }
    }
    params
}

/// 编译一个操作。路径或方法不存在时返回 None。
pub(crate) fn lower_op(
    service: &Service,
    base: &str,
    path: &str,
    method: ApiMethod,
) -> Option<ApiEndpoint> {
    let doc = &service.doc;
    let item = deref(doc, doc.get("paths")?.get(path)?);
    let op = item.get(method.key()).filter(|op| op.is_object())?;
    let template = op.get(X_TEMPLATE).cloned().unwrap_or(Value::Null);
    let mut builder = Builder {
        inputs: Vec::new(),
        notes: Vec::new(),
        query: Vec::new(),
        headers: Vec::new(),
        cookies: Vec::new(),
    };
    let params = merged_params(doc, item, op);

    // 路径：先把路径里出现、却没声明的参数补成必填文字。
    let mut path_template = template
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or(path)
        .to_string();
    let declared: Vec<String> = params
        .iter()
        .filter(|p| p.get("in").and_then(Value::as_str) == Some("path"))
        .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let undeclared: Vec<String> = PATH_PARAM
        .captures_iter(path)
        .map(|caps| caps[1].to_string())
        .filter(|name| !declared.contains(name))
        .collect();

    for param in &params {
        let Some(wire) = param.get("name").and_then(Value::as_str) else {
            continue;
        };
        let location = param.get("in").and_then(Value::as_str).unwrap_or_default();
        let schema = param_schema(doc, param);
        let required = location == "path"
            || param
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let preferred = param.get(X_NAME).and_then(Value::as_str);
        let literal = fixed(&schema).cloned();
        let token = |builder: &mut Builder| match &literal {
            Some(value) => percent_encode(&literal_text(value)),
            None => format!(
                "{{{}}}",
                builder.add_input(wire, preferred, schema.clone(), param, required, None)
            ),
        };
        match location {
            "path" => {
                let value = token(&mut builder);
                if template.get("path").is_none() {
                    path_template = path_template.replace(&format!("{{{wire}}}"), &value);
                }
            }
            "query" => {
                let style = param.get("style").and_then(Value::as_str).unwrap_or("form");
                if style != "form" {
                    builder
                        .notes
                        .push(format!("地址参数 {wire} 的写法 {style} 暂不支持"));
                }
                let explode = param
                    .get("explode")
                    .and_then(Value::as_bool)
                    .unwrap_or(style == "form");
                let value = token(&mut builder);
                if !explode
                    && literal.is_none()
                    && let Some(input) = builder.inputs.last_mut()
                {
                    input.comma = true;
                }
                builder
                    .query
                    .push(format!("{}={value}", percent_encode(wire)));
            }
            "header" => {
                // OpenAPI 规定这三个请求头写成参数时忽略：它们由请求体格式与鉴权决定。
                if ["accept", "content-type", "authorization"]
                    .contains(&wire.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                let value = match &literal {
                    Some(value) => literal_text(value),
                    None => format!(
                        "{{{}}}",
                        builder.add_input(wire, preferred, schema.clone(), param, required, None)
                    ),
                };
                builder.headers.push(ApiHeader {
                    name: wire.to_string(),
                    value,
                });
            }
            "cookie" => {
                let value = token(&mut builder);
                builder.cookies.push(format!("{wire}={value}"));
            }
            _ => {}
        }
    }
    for wire in &undeclared {
        let name = builder.add_input(
            wire,
            None,
            schema::schema_of_kind(InputKind::Text),
            &Value::Null,
            true,
            None,
        );
        path_template = path_template.replace(&format!("{{{wire}}}"), &format!("{{{name}}}"));
    }

    lower_security(doc, op, &mut builder);

    for pair in template
        .get("query")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        builder.query.push(pair.to_string());
    }
    if !builder.cookies.is_empty() {
        let cookie = builder.cookies.join("; ");
        builder.headers.push(ApiHeader {
            name: "Cookie".into(),
            value: cookie,
        });
    }
    for header in template
        .get("headers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Ok(header) = serde_json::from_value::<ApiHeader>(header.clone()) {
            builder.headers.push(header);
        }
    }

    let (body, body_kind) = lower_body(doc, op, method, &template, &mut builder);

    for input in template
        .get("inputs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Ok(input) = serde_json::from_value::<ApiInput>(input.clone())
            && !builder.inputs.iter().any(|i| i.name == input.name)
        {
            builder.inputs.push(input);
        }
    }
    if let Some(order) = op.get(X_INPUTS).and_then(Value::as_array) {
        let position = |name: &str| {
            order
                .iter()
                .position(|n| n.as_str() == Some(name))
                .unwrap_or(usize::MAX)
        };
        builder.inputs.sort_by_key(|input| position(&input.name));
    }

    let mut url = format!("{base}{path_template}");
    if !builder.query.is_empty() {
        url.push('?');
        url.push_str(&builder.query.join("&"));
    }
    let operation_id = op
        .get("operationId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| super::normalize::generated_operation_id(method, path));
    let id = op
        .get(X_ID)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| default_id(&service.id, &operation_id));
    let name = op
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| operation_id.clone(), str::to_string);
    let examples = lower_tests(doc, op, &builder.inputs);
    Some(ApiEndpoint {
        id,
        name,
        description: op
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        method,
        url,
        inputs: builder.inputs,
        headers: builder.headers,
        body,
        mapping: op
            .get(X_EVIDENCE)
            .and_then(|value| serde_json::from_value::<ApiMapping>(value.clone()).ok())
            .unwrap_or_default(),
        success: success_of(op.get(X_SUCCESS).or_else(|| doc.get(X_SUCCESS))),
        destination: match op.get(X_DESTINATION).and_then(Value::as_str) {
            Some("variable") => ApiDestination::Variable,
            _ => ApiDestination::Evidence,
        },
        timeout_seconds: timeout_of(op, doc),
        examples,
        body_kind,
        readonly: op
            .get(X_READONLY)
            .and_then(Value::as_bool)
            .unwrap_or_else(|| default_readonly(method, path)),
        ai: op.get(X_AI).and_then(Value::as_bool).unwrap_or(true),
        unsupported: builder.notes.join("；"),
        origin: Some(OpOrigin {
            service: service.id.clone(),
            path: path.to_string(),
            method,
            op: op.clone(),
        }),
    })
}

/// 鉴权：用操作上的 `security`，没有就用根上的；只取第一种方案（几种任选其一时）。
fn lower_security(doc: &Value, op: &Value, builder: &mut Builder) {
    let Some(requirement) = op
        .get("security")
        .or_else(|| doc.get("security"))
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_object)
    else {
        return;
    };
    for scheme_name in requirement.keys() {
        let pointer = format!("/components/securitySchemes/{}", pointer_token(scheme_name));
        let Some(scheme) = doc.pointer(&pointer).map(|s| deref(doc, s)) else {
            builder
                .notes
                .push(format!("鉴权方式 {scheme_name} 在文档里找不到定义"));
            continue;
        };
        let secret = format!("{{secret:{}}}", secret_of(scheme_name, scheme));
        let kind = scheme
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let location = scheme.get("in").and_then(Value::as_str).unwrap_or_default();
        let name = scheme
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match (kind, location) {
            ("apiKey", "header") => builder.headers.push(ApiHeader {
                name: name.to_string(),
                value: secret,
            }),
            ("apiKey", "query") => builder
                .query
                .push(format!("{}={secret}", percent_encode(name))),
            ("apiKey", "cookie") => builder.cookies.push(format!("{name}={secret}")),
            ("http", _) => {
                let scheme_word = scheme
                    .get("scheme")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let prefix = match scheme_word.as_str() {
                    "bearer" => "Bearer",
                    "basic" => "Basic",
                    other => {
                        builder
                            .notes
                            .push(format!("鉴权方式 http {other} 暂不支持"));
                        continue;
                    }
                };
                builder.headers.push(ApiHeader {
                    name: "Authorization".into(),
                    value: format!("{prefix} {secret}"),
                });
            }
            ("oauth2" | "openIdConnect", _) => builder.headers.push(ApiHeader {
                name: "Authorization".into(),
                value: format!("Bearer {secret}"),
            }),
            _ => builder
                .notes
                .push(format!("鉴权方式 {scheme_name}（{kind}）暂不支持")),
        }
    }
}

/// 请求体：对象的顶层字段逐个做成输入；不是对象的整段做成一个输入 `body`。
fn lower_body(
    doc: &Value,
    op: &Value,
    method: ApiMethod,
    template: &Value,
    builder: &mut Builder,
) -> (String, BodyKind) {
    if let Some(body) = template.get("body").and_then(Value::as_str) {
        let kind = if template.get("form").and_then(Value::as_bool) == Some(true) {
            BodyKind::Form
        } else {
            BodyKind::Json
        };
        return (body.to_string(), kind);
    }
    let Some(request_body) = op.get("requestBody").map(|rb| deref(doc, rb)) else {
        return (String::new(), BodyKind::Json);
    };
    if !method.has_body() {
        return (String::new(), BodyKind::Json);
    }
    let Some(content) = request_body.get("content").and_then(Value::as_object) else {
        return (String::new(), BodyKind::Json);
    };
    let picked = content
        .iter()
        .find(|(ct, _)| ct.contains("json"))
        .map(|(ct, media)| (ct, media, BodyKind::Json))
        .or_else(|| {
            content
                .iter()
                .find(|(ct, _)| ct.starts_with("application/x-www-form-urlencoded"))
                .map(|(ct, media)| (ct, media, BodyKind::Form))
        });
    let Some((_, media, kind)) = picked else {
        let types: Vec<&str> = content.keys().map(String::as_str).collect();
        builder
            .notes
            .push(format!("请求体格式 {} 暂不支持", types.join(" / ")));
        return (String::new(), BodyKind::Json);
    };
    let schema = media
        .get("schema")
        .map(|s| flatten(&resolve_deep(doc, s)))
        .unwrap_or_else(|| Value::Object(Map::new()));
    let example = media.get("example").cloned().or_else(|| {
        media
            .get("examples")
            .and_then(Value::as_object)
            .and_then(|map| map.values().next())
            .and_then(|example| deref(doc, example).get("value"))
            .cloned()
    });
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .filter(|props| !props.is_empty());
    let body = match properties {
        Some(properties) => {
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let mut object = Map::new();
            for (wire, prop) in properties {
                let prop = flatten(prop);
                if let Some(value) = fixed(&prop) {
                    object.insert(wire.clone(), value.clone());
                    continue;
                }
                let fallback = example.as_ref().and_then(|e| e.get(wire));
                let name = builder.add_input(
                    wire,
                    prop.get(X_NAME).and_then(Value::as_str),
                    prop.clone(),
                    &prop,
                    required.contains(&wire.as_str()),
                    fallback,
                );
                object.insert(wire.clone(), Value::String(format!("{{{name}}}")));
            }
            Value::Object(object)
        }
        None => {
            let required = request_body
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let name = builder.add_input(
                "body",
                request_body.get(X_NAME).and_then(Value::as_str),
                schema.clone(),
                &Value::Null,
                required,
                example.as_ref(),
            );
            Value::String(format!("{{{name}}}"))
        }
    };
    (
        serde_json::to_string_pretty(&body).unwrap_or_default(),
        kind,
    )
}

/// 测试用例 → 样例组；没有测试用例时用请求体的命名例子。
fn lower_tests(doc: &Value, op: &Value, inputs: &[ApiInput]) -> Vec<ApiExample> {
    let kind_of_input = |name: &str| {
        inputs
            .iter()
            .find(|input| input.name == name)
            .map_or(InputKind::Text, |input| input.kind)
    };
    if let Some(tests) = op.get(X_TESTS).and_then(Value::as_array) {
        return tests
            .iter()
            .map(|test| ApiExample {
                name: test
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                note: test
                    .get("note")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                args: test
                    .get("args")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(name, value)| (name.clone(), example_text(kind_of_input(name), value)))
                    .collect(),
                // 认不出的期望（别处手写的、格式不对的）不收，免得判定时出怪结果。
                expect: test
                    .get("expect")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| serde_json::from_value(e.clone()).ok())
                    .collect(),
            })
            .collect();
    }
    let Some(examples) = op
        .get("requestBody")
        .map(|rb| deref(doc, rb))
        .and_then(|rb| rb.get("content"))
        .and_then(Value::as_object)
        .and_then(|content| content.values().find_map(|m| m.get("examples")))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    examples
        .iter()
        .filter_map(|(key, example)| {
            let example = deref(doc, example);
            let value = example.get("value")?.as_object()?;
            let args = value
                .iter()
                .filter_map(|(wire, value)| {
                    let input = inputs.iter().find(|input| input.wire == *wire)?;
                    Some((input.name.clone(), example_text(input.kind, value)))
                })
                .collect();
            Some(ApiExample {
                name: example
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or(key)
                    .to_string(),
                note: example
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                args,
                ..Default::default()
            })
        })
        .collect()
}
