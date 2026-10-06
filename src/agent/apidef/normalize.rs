//! 认格式与规范化：OpenAPI 3.1、Swagger 2.0 → OpenAPI 3.0.3；补齐操作 id。

use super::OPENAPI_VERSION;
use super::schema::{deref, slug_segment};
use super::unique;
use crate::agent::api::{ApiMethod, ApiStore};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// 认出来的文件。
pub(super) enum Detected {
    /// OpenAPI 3.x 或 Swagger 2.0。
    OpenApi(Value),
    /// Postman 集合。
    Postman(Value),
    /// Postman 环境文件。
    PostmanEnv(Value),
    /// 旧版 `apis.json` / 本程序以前导出的接口配置。
    Legacy(ApiStore),
    /// 一段 cURL 命令。
    Curl,
    Unknown,
}

/// JSON 或 YAML 读成对象。
pub(super) fn parse_text(text: &str) -> Option<Value> {
    let text = text.trim_start_matches('\u{feff}');
    serde_json::from_str::<Value>(text)
        .ok()
        .or_else(|| serde_yaml::from_str::<Value>(text).ok())
        .filter(Value::is_object)
}

pub(super) fn detect(text: &str) -> Detected {
    let is_curl = text
        .lines()
        .any(|line| line.trim_start().starts_with("curl "));
    if is_curl {
        return Detected::Curl;
    }
    if let Some(value) = parse_text(text) {
        if value.get("openapi").is_some() || value.get("swagger").is_some() {
            return Detected::OpenApi(value);
        }
        let postman_schema = value
            .pointer("/info/schema")
            .and_then(Value::as_str)
            .is_some_and(|schema| schema.contains("postman"));
        if postman_schema
            || value.pointer("/info/_postman_id").is_some()
            || (value.get("info").is_some() && value.get("item").is_some_and(Value::is_array))
        {
            return Detected::Postman(value);
        }
        if value.get("values").is_some_and(Value::is_array)
            && (value.get("_postman_variable_scope").is_some() || value.get("name").is_some())
        {
            return Detected::PostmanEnv(value);
        }
        if value.get("endpoints").is_some_and(Value::is_array)
            && let Ok(store) = serde_json::from_value::<ApiStore>(value)
        {
            return Detected::Legacy(store);
        }
        return Detected::Unknown;
    }
    if text.contains("curl ") {
        return Detected::Curl;
    }
    Detected::Unknown
}

/// 规范成 OpenAPI 3.0.3。`report` 收丢掉、降级的东西。
pub(super) fn to_openapi30(doc: Value, report: &mut Vec<String>) -> Result<Value, String> {
    let mut doc = if doc.get("swagger").and_then(Value::as_str) == Some("2.0") {
        swagger2(&doc, report)
    } else {
        let version = doc
            .get("openapi")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if version.starts_with("3.1") {
            let mut doc = doc;
            downgrade31(&mut doc, report);
            doc
        } else if version.starts_with("3.0") {
            doc
        } else {
            return Err(format!(
                "不认识的版本「{version}」，只支持 OpenAPI 3.x 与 Swagger 2.0"
            ));
        }
    };
    doc["openapi"] = json!(OPENAPI_VERSION);
    if !doc.get("info").is_some_and(Value::is_object) {
        doc["info"] = json!({});
    }
    if doc.pointer("/info/title").and_then(Value::as_str).is_none() {
        doc["info"]["title"] = json!("接口");
    }
    if doc.pointer("/info/version").is_none() {
        doc["info"]["version"] = json!("1.0");
    }
    if !doc.get("paths").is_some_and(Value::is_object) {
        doc["paths"] = json!({});
    }
    if let Some(map) = doc.as_object_mut() {
        for key in ["webhooks", "jsonSchemaDialect"] {
            if map.remove(key).is_some() {
                report.push(format!("去掉了 {key}（OpenAPI 3.0 没有）"));
            }
        }
    }
    for method in ["options", "trace"] {
        let count = count_method(&doc, method);
        if count > 0 {
            report.push(format!("{count} 个 {} 操作不收", method.to_uppercase()));
        }
    }
    let external = external_refs(&doc);
    if !external.is_empty() {
        report.push(format!(
            "有 {} 处引用指向别的文件（{}），没有解析，相关参数按任意值处理",
            external.len(),
            external
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    let callbacks = count_key(&doc, "callbacks");
    if callbacks > 0 {
        report.push(format!("{callbacks} 处回调（callbacks）不支持，原样保留"));
    }
    ensure_operation_ids(&mut doc, report);
    Ok(doc)
}

fn count_method(doc: &Value, method: &str) -> usize {
    doc.get("paths")
        .and_then(Value::as_object)
        .map_or(0, |paths| {
            paths
                .values()
                .filter(|item| item.get(method).is_some())
                .count()
        })
}

fn count_key(value: &Value, key: &str) -> usize {
    match value {
        Value::Object(map) => {
            usize::from(map.contains_key(key))
                + map.values().map(|v| count_key(v, key)).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(|v| count_key(v, key)).sum(),
        _ => 0,
    }
}

fn external_refs(value: &Value) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    collect_external(value, &mut found);
    found
}

fn collect_external(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(reference) = map.get("$ref").and_then(Value::as_str)
                && !reference.starts_with('#')
            {
                found.insert(reference.to_string());
            }
            map.values().for_each(|v| collect_external(v, found));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_external(v, found)),
        _ => {}
    }
}

/// 3.1 → 3.0：`type: [x, "null"]` → `nullable`，`const` → 单值 `enum`，schema 的 `examples`
/// 数组取第一个作 `example`，数字形式的 `exclusiveMinimum` 改回布尔。
fn downgrade31(doc: &mut Value, report: &mut Vec<String>) {
    let mut changed = 0;
    walk31(doc, &mut changed);
    if changed > 0 {
        report.push(format!(
            "OpenAPI 3.1 的 {changed} 处 schema 写法降级成了 3.0"
        ));
    }
}

fn walk31(value: &mut Value, changed: &mut usize) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(kinds)) = map.get("type").cloned()
                && kinds.iter().all(Value::is_string)
            {
                let real: Vec<Value> = kinds
                    .iter()
                    .filter(|k| k.as_str() != Some("null"))
                    .cloned()
                    .collect();
                if real.len() != kinds.len() {
                    map.insert("nullable".into(), json!(true));
                }
                match real.as_slice() {
                    [one] => {
                        map.insert("type".into(), one.clone());
                    }
                    _ => {
                        map.remove("type");
                    }
                }
                *changed += 1;
            }
            if map.get("const").is_some_and(|c| !c.is_object())
                && let Some(constant) = map.remove("const")
            {
                map.insert("enum".into(), json!([constant]));
                *changed += 1;
            }
            if let Some(Value::Array(examples)) = map.get("examples").cloned() {
                map.remove("examples");
                if let Some(first) = examples.first() {
                    map.entry("example").or_insert_with(|| first.clone());
                }
                *changed += 1;
            }
            for (bound, exclusive) in [
                ("minimum", "exclusiveMinimum"),
                ("maximum", "exclusiveMaximum"),
            ] {
                if let Some(limit) = map.get(exclusive).filter(|v| v.is_number()).cloned() {
                    map.insert(bound.into(), limit);
                    map.insert(exclusive.into(), json!(true));
                    *changed += 1;
                }
            }
            for item in map.values_mut() {
                walk31(item, changed);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| walk31(item, changed)),
        _ => {}
    }
}

/// 2.0 参数里属于 schema 的字段。
const SCHEMA_FIELDS: &[&str] = &[
    "type",
    "format",
    "items",
    "enum",
    "default",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "multipleOf",
];

/// Swagger 2.0 → OpenAPI 3.0。
fn swagger2(source: &Value, report: &mut Vec<String>) -> Value {
    let mut doc = Map::new();
    doc.insert("openapi".into(), json!(OPENAPI_VERSION));
    doc.insert(
        "info".into(),
        source.get("info").cloned().unwrap_or_else(|| json!({})),
    );
    let scheme = source
        .pointer("/schemes/0")
        .and_then(Value::as_str)
        .unwrap_or("http");
    let base_path = source
        .get("basePath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('/');
    match source.get("host").and_then(Value::as_str) {
        Some(host) => {
            doc.insert(
                "servers".into(),
                json!([{ "url": format!("{scheme}://{host}{base_path}") }]),
            );
        }
        None if !base_path.is_empty() => {
            doc.insert("servers".into(), json!([{ "url": base_path }]));
        }
        None => {}
    }
    for key in ["tags", "security", "externalDocs"] {
        if let Some(value) = source.get(key) {
            doc.insert(key.into(), value.clone());
        }
    }
    if let Some(map) = source.as_object() {
        for (key, value) in map.iter().filter(|(k, _)| k.starts_with("x-")) {
            doc.insert(key.clone(), value.clone());
        }
    }
    let consumes = mimes(source.get("consumes"));
    let produces = mimes(source.get("produces"));

    let mut paths = Map::new();
    for (path, item) in source
        .get("paths")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let item = deref(source, item);
        let shared: Vec<Value> = item
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut new_item = Map::new();
        for (key, op) in item.as_object().into_iter().flatten() {
            if ApiMethod::from_key(key).is_none() && !matches!(key.as_str(), "options") {
                continue;
            }
            new_item.insert(
                key.clone(),
                swagger2_op(source, op, &shared, &consumes, &produces),
            );
        }
        paths.insert(path.clone(), Value::Object(new_item));
    }
    doc.insert("paths".into(), Value::Object(paths));

    let mut components = Map::new();
    if let Some(definitions) = source.get("definitions") {
        components.insert("schemas".into(), definitions.clone());
    }
    if let Some(definitions) = source.get("securityDefinitions").and_then(Value::as_object) {
        let schemes: Map<String, Value> = definitions
            .iter()
            .map(|(name, def)| (name.clone(), security_scheme2(def)))
            .collect();
        components.insert("securitySchemes".into(), Value::Object(schemes));
    }
    if !components.is_empty() {
        doc.insert("components".into(), Value::Object(components));
    }
    let mut doc = Value::Object(doc);
    rewrite_refs(&mut doc);
    report.push("Swagger 2.0 已转换成 OpenAPI 3.0".into());
    doc
}

fn mimes(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn swagger2_op(
    source: &Value,
    op: &Value,
    shared: &[Value],
    consumes: &[String],
    produces: &[String],
) -> Value {
    let mut new_op = Map::new();
    for (key, value) in op.as_object().into_iter().flatten() {
        if !matches!(
            key.as_str(),
            "parameters" | "responses" | "consumes" | "produces" | "schemes"
        ) {
            new_op.insert(key.clone(), value.clone());
        }
    }
    let consumes = {
        let own = mimes(op.get("consumes"));
        if own.is_empty() {
            consumes.to_vec()
        } else {
            own
        }
    };
    let produces = {
        let own = mimes(op.get("produces"));
        if own.is_empty() {
            produces.to_vec()
        } else {
            own
        }
    };
    // 路径级参数在前，操作级按 名字 + 位置 覆盖。
    let mut all: Vec<Value> = shared.iter().map(|p| deref(source, p).clone()).collect();
    for param in op
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let param = deref(source, param).clone();
        match all
            .iter_mut()
            .find(|p| p.get("name") == param.get("name") && p.get("in") == param.get("in"))
        {
            Some(existing) => *existing = param,
            None => all.push(param),
        }
    }
    let mut params = Vec::new();
    let mut form = Map::new();
    let mut form_required = Vec::new();
    let mut multipart = false;
    for param in all {
        let location = param.get("in").and_then(Value::as_str).unwrap_or_default();
        match location {
            "body" => {
                let content_type = consumes
                    .iter()
                    .find(|c| c.contains("json"))
                    .cloned()
                    .unwrap_or_else(|| "application/json".into());
                let mut body = json!({
                    "content": {content_type: {"schema": param.get("schema").cloned().unwrap_or_else(|| json!({}))}},
                });
                if let Some(required) = param.get("required") {
                    body["required"] = required.clone();
                }
                if let Some(description) = param.get("description") {
                    body["description"] = description.clone();
                }
                new_op.insert("requestBody".into(), body);
            }
            "formData" => {
                let name = param
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if param.get("type").and_then(Value::as_str) == Some("file") {
                    multipart = true;
                    form.insert(name.clone(), json!({"type": "string", "format": "binary"}));
                } else {
                    form.insert(name.clone(), param_schema2(&param));
                }
                if param.get("required").and_then(Value::as_bool) == Some(true) {
                    form_required.push(json!(name));
                }
            }
            _ => {
                let mut new_param = Map::new();
                for key in ["name", "in", "description", "required", "deprecated"] {
                    if let Some(value) = param.get(key) {
                        new_param.insert(key.into(), value.clone());
                    }
                }
                for (key, value) in param.as_object().into_iter().flatten() {
                    if key.starts_with("x-") {
                        new_param.insert(key.clone(), value.clone());
                    }
                }
                new_param.insert("schema".into(), param_schema2(&param));
                if param.get("type").and_then(Value::as_str) == Some("array") {
                    match param
                        .get("collectionFormat")
                        .and_then(Value::as_str)
                        .unwrap_or("csv")
                    {
                        "multi" => {
                            new_param.insert("explode".into(), json!(true));
                        }
                        "ssv" => {
                            new_param.insert("style".into(), json!("spaceDelimited"));
                        }
                        "pipes" => {
                            new_param.insert("style".into(), json!("pipeDelimited"));
                        }
                        _ => {
                            new_param.insert("explode".into(), json!(false));
                        }
                    }
                }
                params.push(Value::Object(new_param));
            }
        }
    }
    if !form.is_empty() {
        let content_type = if multipart || consumes.iter().any(|c| c.starts_with("multipart/")) {
            "multipart/form-data"
        } else {
            "application/x-www-form-urlencoded"
        };
        let mut schema = json!({"type": "object", "properties": form});
        if !form_required.is_empty() {
            schema["required"] = Value::Array(form_required);
        }
        new_op.insert(
            "requestBody".into(),
            json!({"content": {content_type: {"schema": schema}}}),
        );
    }
    if !params.is_empty() {
        new_op.insert("parameters".into(), Value::Array(params));
    }
    let content_type = produces
        .iter()
        .find(|c| c.contains("json"))
        .cloned()
        .unwrap_or_else(|| "application/json".into());
    let mut responses = Map::new();
    for (code, response) in op
        .get("responses")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let response = deref(source, response);
        let mut new_response = json!({
            "description": response.get("description").cloned().unwrap_or_else(|| json!("")),
        });
        if let Some(schema) = response.get("schema") {
            let mut media = json!({"schema": schema});
            if let Some(example) = response
                .get("examples")
                .and_then(Value::as_object)
                .and_then(|examples| {
                    examples
                        .iter()
                        .find(|(mime, _)| mime.contains("json"))
                        .or_else(|| examples.iter().next())
                })
                .map(|(_, example)| example)
            {
                media["example"] = example.clone();
            }
            new_response["content"] = json!({content_type.clone(): media});
        }
        responses.insert(code.clone(), new_response);
    }
    if responses.is_empty() {
        responses.insert("200".into(), json!({"description": "成功"}));
    }
    new_op.insert("responses".into(), Value::Object(responses));
    Value::Object(new_op)
}

fn param_schema2(param: &Value) -> Value {
    let mut schema = Map::new();
    for key in SCHEMA_FIELDS {
        if let Some(value) = param.get(*key) {
            schema.insert((*key).into(), value.clone());
        }
    }
    Value::Object(schema)
}

fn security_scheme2(def: &Value) -> Value {
    let mut scheme = match def.get("type").and_then(Value::as_str) {
        Some("basic") => json!({"type": "http", "scheme": "basic"}),
        Some("apiKey") => json!({
            "type": "apiKey",
            "in": def.get("in").cloned().unwrap_or_else(|| json!("header")),
            "name": def.get("name").cloned().unwrap_or_else(|| json!("Authorization")),
        }),
        Some("oauth2") => {
            let flow = match def.get("flow").and_then(Value::as_str) {
                Some("implicit") => "implicit",
                Some("password") => "password",
                Some("application") => "clientCredentials",
                _ => "authorizationCode",
            };
            let mut details =
                json!({"scopes": def.get("scopes").cloned().unwrap_or_else(|| json!({}))});
            for key in ["authorizationUrl", "tokenUrl"] {
                if let Some(url) = def.get(key) {
                    details[key] = url.clone();
                }
            }
            json!({"type": "oauth2", "flows": {flow: details}})
        }
        _ => def.clone(),
    };
    if let Some(description) = def.get("description") {
        scheme["description"] = description.clone();
    }
    scheme
}

fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref")
                && let Some(rest) = reference.strip_prefix("#/definitions/")
            {
                *reference = format!("#/components/schemas/{rest}");
            }
            map.values_mut().for_each(rewrite_refs);
        }
        Value::Array(items) => items.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

/// 操作 id 合不合用：英文字母、数字、下划线、短横线、点。
pub(super) fn valid_operation_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// 从方法与路径起一个操作 id：`GET /api/stat/{region}` → `get_api_stat_by_region`。
pub(super) fn generated_operation_id(method: ApiMethod, path: &str) -> String {
    let mut parts = vec![method.key().to_string()];
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let (by, inner) = match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(inner) => (true, inner),
            None => (false, segment),
        };
        let word = slug_segment(inner);
        if word.is_empty() {
            continue;
        }
        if by {
            parts.push("by".into());
        }
        parts.push(word);
    }
    let id = parts.join("_");
    id.chars().take(64).collect()
}

/// 每个操作都要有合用、不重复的 `operationId`：没有的、中文的、重复的按方法与路径起一个，
/// 原来的中文 id 没有摘要时挪去当摘要。
fn ensure_operation_ids(doc: &mut Value, report: &mut Vec<String>) {
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut renamed = 0;
    let Some(paths) = doc.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for (path, item) in paths.iter_mut() {
        for method in ApiMethod::ALL {
            let Some(op) = item.get_mut(method.key()).and_then(Value::as_object_mut) else {
                continue;
            };
            let current = op
                .get("operationId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if valid_operation_id(&current) && !taken.contains(&current) {
                taken.insert(current);
                continue;
            }
            let id = unique(
                &generated_operation_id(method, path),
                |candidate| taken.contains(candidate),
                "_",
            );
            if !current.is_empty() {
                renamed += 1;
                if op.get("summary").is_none() {
                    op.insert("summary".into(), json!(current));
                }
            }
            op.insert("operationId".into(), json!(id));
            taken.insert(id);
        }
    }
    if renamed > 0 {
        report.push(format!(
            "{renamed} 个操作 id 不是英文或有重复，按方法与路径重起了（原名放进了摘要）"
        ));
    }
}
