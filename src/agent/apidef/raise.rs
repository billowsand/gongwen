//! 写回：界面改过的 [`ApiEndpoint`] → OpenAPI 操作（编译的反方向）。
//!
//! 存盘时逐个接口看：和从文件重新编译的结果一样的，文件不动；只改了名称、说明、映射、
//! 样例这类的，只改操作上对应的字段（引用、标签、返回结构都留着）；改了请求的，按模板
//! 重新拆成参数、请求体与鉴权。拆不开的部分（混着变量的请求头、变量嵌在深处的请求体……）
//! 放进 `x-gongwen-template` 原样保留，保证请求一字不差。
//!
//! 旧版 `apis.json` 的迁移也走这里：那些接口都没有来源，全部按服务器分组新建服务。

use super::lower::{
    X_AI, X_AI_TESTS, X_DESTINATION, X_EVIDENCE, X_ID, X_INPUTS, X_NAME, X_READONLY, X_SECRET,
    X_SUCCESS, X_TEMPLATE, X_TESTS, default_readonly, lower_op, pointer_token, secret_of,
    success_of, timeout_of,
};
use super::schema::{deref, example_value, identifier, kind_of, percent_decode, schema_of_kind};
use super::{Service, base_url, default_id, empty_doc, lower_all, service_id, unique};
use crate::agent::api::{
    self, ApiDestination, ApiEndpoint, ApiInput, ApiMapping, ApiMethod, ApiStore, BodyKind,
    InputKind, OpOrigin,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// 每个接口怎么处理。
enum Plan {
    /// 和文件里的一样。
    Keep,
    /// 只改了名称、说明、映射、样例这类，请求没变。
    Meta(OpOrigin),
    /// 请求变了，或者是新接口。
    Rebuild,
}

/// 把 `store.endpoints` 的修改写回 `store.services`，再重新编译（`endpoints` 顺序不变）。
/// 返回说明（拆不开、改了位置之类），给界面看。
pub(crate) fn absorb(store: &mut ApiStore) -> Vec<String> {
    let mut notes = Vec::new();
    let plans: Vec<Plan> = store
        .endpoints
        .iter()
        .map(|endpoint| plan(store, endpoint))
        .collect();

    // 没人引用的操作（删掉的、要重写的）先拿掉。
    let mut keep: BTreeSet<(String, String, ApiMethod)> = BTreeSet::new();
    for (endpoint, plan) in store.endpoints.iter().zip(&plans) {
        match plan {
            Plan::Keep => {
                if let Some(origin) = &endpoint.origin {
                    keep.insert((origin.service.clone(), origin.path.clone(), origin.method));
                }
            }
            Plan::Meta(origin) => {
                keep.insert((origin.service.clone(), origin.path.clone(), origin.method));
            }
            Plan::Rebuild => {}
        }
    }
    for service in &mut store.services {
        let id = service.id.clone();
        let Some(paths) = service.doc.get_mut("paths").and_then(Value::as_object_mut) else {
            continue;
        };
        for (path, item) in paths.iter_mut() {
            if let Some(item) = item.as_object_mut() {
                for method in ApiMethod::ALL {
                    if !keep.contains(&(id.clone(), path.clone(), method)) {
                        item.remove(method.key());
                    }
                }
            }
        }
        paths.retain(|_, item| {
            ApiMethod::ALL
                .iter()
                .any(|method| item.get(method.key()).is_some())
        });
    }

    let endpoints = store.endpoints.clone();
    for (endpoint, plan) in endpoints.iter().zip(plans) {
        match plan {
            Plan::Keep => {}
            Plan::Meta(origin) => {
                let Some(index) = find(store, &origin.service) else {
                    continue;
                };
                let mut op = origin.op.as_object().cloned().unwrap_or_default();
                apply_meta(
                    &mut op,
                    endpoint,
                    &store.services[index],
                    origin.method,
                    &origin.path,
                );
                set_op(
                    &mut store.services[index].doc,
                    &origin.path,
                    origin.method,
                    op,
                );
            }
            Plan::Rebuild => rebuild(store, endpoint, &mut notes),
        }
    }

    store
        .services
        .retain(|service| !service.operations().is_empty());
    let fresh = lower_all(store);
    let mut used = vec![false; fresh.len()];
    for endpoint in &mut store.endpoints {
        if let Some(index) = fresh.iter().position(|f| f.id == endpoint.id) {
            *endpoint = fresh[index].clone();
            used[index] = true;
        }
    }
    store.endpoints.retain(|endpoint| endpoint.origin.is_some());
    for (index, endpoint) in fresh.into_iter().enumerate() {
        if !used[index] {
            store.endpoints.push(endpoint);
        }
    }
    notes
}

fn find(store: &ApiStore, id: &str) -> Option<usize> {
    store.services.iter().position(|service| service.id == id)
}

fn base_of(store: &ApiStore, index: usize) -> String {
    let service = &store.services[index];
    base_url(service, store.envs.get(&service.id))
}

/// 地址在这个服务器地址下面（按段对齐，不能 `http://a.b` 配 `http://a.bc`）。
fn under(url: &str, base: &str) -> bool {
    base.starts_with("http")
        && url.starts_with(base)
        && matches!(url[base.len()..].chars().next(), None | Some('/' | '?'))
}

/// 去掉只影响说明、不影响请求的字段，比较请求变没变。
fn structure(endpoint: &ApiEndpoint) -> ApiEndpoint {
    ApiEndpoint {
        method: endpoint.method,
        url: endpoint.url.clone(),
        inputs: endpoint.inputs.clone(),
        headers: endpoint.headers.clone(),
        body: endpoint.body.clone(),
        body_kind: endpoint.body_kind,
        unsupported: endpoint.unsupported.clone(),
        ..ApiEndpoint::default()
    }
}

fn plan(store: &ApiStore, endpoint: &ApiEndpoint) -> Plan {
    let Some(origin) = &endpoint.origin else {
        return Plan::Rebuild;
    };
    let Some(index) = find(store, &origin.service) else {
        return Plan::Rebuild;
    };
    let base = base_of(store, index);
    let Some(fresh) = lower_op(&store.services[index], &base, &origin.path, origin.method) else {
        return Plan::Rebuild;
    };
    if fresh == *endpoint {
        return Plan::Keep;
    }
    // 发不了的接口（multipart 之类）请求部分是编译时拼的，不往回拆，只改说明类字段。
    let same_request = structure(&fresh) == structure(endpoint);
    if same_request || !fresh.unsupported.is_empty() {
        return Plan::Meta(origin.clone());
    }
    Plan::Rebuild
}

fn set_op(doc: &mut Value, path: &str, method: ApiMethod, op: Map<String, Value>) {
    if !doc.get("paths").is_some_and(Value::is_object) {
        doc["paths"] = json!({});
    }
    let paths = doc["paths"].as_object_mut().expect("paths 是对象");
    let item = paths.entry(path.to_string()).or_insert_with(|| json!({}));
    if !item.is_object() {
        *item = json!({});
    }
    item[method.key()] = Value::Object(op);
}

/// 这条路径上已经有这个方法的操作了。
fn occupied(doc: &Value, path: &str, method: ApiMethod) -> bool {
    doc.get("paths")
        .and_then(|paths| paths.get(path))
        .and_then(|item| item.get(method.key()))
        .is_some()
}

/// 路径级参数下放到各个操作上：往这条路径上加新操作前做，免得新操作平白多出参数。
fn push_down_path_params(doc: &mut Value, path: &str) {
    let Some(item) = doc
        .get_mut("paths")
        .and_then(|paths| paths.get_mut(path))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let Some(Value::Array(shared)) = item.remove("parameters") else {
        return;
    };
    for method in ApiMethod::ALL {
        let Some(op) = item.get_mut(method.key()).and_then(Value::as_object_mut) else {
            continue;
        };
        let own = op
            .entry("parameters")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        let mut merged: Vec<Value> = shared
            .iter()
            .filter(|p| {
                !own.iter()
                    .any(|o| o.get("name") == p.get("name") && o.get("in") == p.get("in"))
            })
            .cloned()
            .collect();
        merged.extend(own);
        op.insert("parameters".into(), Value::Array(merged));
    }
}

/// 名称、说明、只查询、成功判据、映射、超时、样例与 id 写到操作上。
fn apply_meta(
    op: &mut Map<String, Value>,
    endpoint: &ApiEndpoint,
    service: &Service,
    method: ApiMethod,
    path: &str,
) {
    let doc = &service.doc;
    set_or_remove(op, "summary", text_value(endpoint.name.trim()));
    set_or_remove(op, "description", text_value(&endpoint.description));
    let operation_id = op
        .get("operationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    set_or_remove(
        op,
        X_ID,
        (endpoint.id != default_id(&service.id, &operation_id))
            .then(|| Value::String(endpoint.id.clone())),
    );
    set_or_remove(
        op,
        X_READONLY,
        (endpoint.readonly != default_readonly(method, path))
            .then_some(Value::Bool(endpoint.readonly)),
    );
    set_or_remove(op, X_AI, (!endpoint.ai).then_some(Value::Bool(false)));
    let root_success = success_of(doc.get(X_SUCCESS));
    set_or_remove(
        op,
        X_SUCCESS,
        (endpoint.success != root_success).then(|| {
            if endpoint.success.is_set() {
                json!({
                    "pointer": endpoint.success.pointer.trim(),
                    "equals": endpoint
                        .success
                        .equals
                        .split('|')
                        .map(|piece| example_value(InputKind::Json, piece.trim()))
                        .collect::<Vec<_>>(),
                })
            } else {
                json!({})
            }
        }),
    );
    set_or_remove(
        op,
        X_EVIDENCE,
        (endpoint.mapping != ApiMapping::default()).then(|| {
            let mut mapping = serde_json::to_value(&endpoint.mapping).unwrap_or_default();
            if let Some(map) = mapping.as_object_mut() {
                map.retain(|_, value| value.as_str().is_some_and(|text| !text.is_empty()));
            }
            mapping
        }),
    );
    set_or_remove(
        op,
        X_DESTINATION,
        (endpoint.destination == ApiDestination::Variable).then(|| json!("variable")),
    );
    let root_timeout = timeout_of(&Value::Null, doc);
    op.remove(super::lower::X_TIMEOUT);
    if endpoint.timeout_seconds != root_timeout {
        op.insert(
            super::lower::X_TIMEOUT.into(),
            json!(endpoint.timeout_seconds),
        );
    }
    let has_body_examples = op
        .get("requestBody")
        .map(|rb| deref(doc, rb))
        .and_then(|rb| rb.get("content"))
        .and_then(Value::as_object)
        .is_some_and(|content| content.values().any(|m| m.get("examples").is_some()));
    let tests: Vec<Value> = endpoint
        .examples
        .iter()
        .map(|example| {
            let args: Map<String, Value> = example
                .args
                .iter()
                .map(|(name, text)| {
                    let kind = endpoint
                        .inputs
                        .iter()
                        .find(|input| input.name == *name)
                        .map_or(InputKind::Text, |input| input.kind);
                    (name.clone(), example_value(kind, text))
                })
                .collect();
            let mut test = json!({"name": example.name, "args": args});
            if !example.note.is_empty() {
                test["note"] = json!(example.note);
            }
            if !example.expect.is_empty() {
                test["expect"] = serde_json::to_value(&example.expect).unwrap_or_default();
            }
            test
        })
        .collect();
    // 请求体里有命名例子时，空的测试用例也要写明，不然编译时会拿那些例子补回来。
    set_or_remove(
        op,
        X_TESTS,
        (!tests.is_empty() || has_body_examples).then_some(Value::Array(tests)),
    );
    set_or_remove(
        op,
        X_AI_TESTS,
        (!endpoint.ai_cases.is_empty()).then(|| {
            Value::Array(
                endpoint
                    .ai_cases
                    .iter()
                    .map(|case| json!({"question": case.question, "expect": {"args": case.expect_args}}))
                    .collect(),
            )
        }),
    );
    if !op.get("responses").is_some_and(Value::is_object) {
        op.insert("responses".into(), json!({"200": {"description": "成功"}}));
    }
}

fn text_value(text: &str) -> Option<Value> {
    (!text.is_empty()).then(|| Value::String(text.to_string()))
}

fn set_or_remove(op: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    match value {
        Some(value) => {
            op.insert(key.to_string(), value);
        }
        None => {
            op.remove(key);
        }
    }
}

/// `http://10.0.0.9:8080/api/x` → (`http://10.0.0.9:8080`, `/api/x`)；不是 http 地址返回 None。
fn split_host(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let at = rest.find(['/', '?']).unwrap_or(rest.len());
    let host = &rest[..at];
    if host.is_empty() {
        return None;
    }
    Some((
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            host.to_ascii_lowercase()
        ),
        rest[at..].to_string(),
    ))
}

/// 接口该归哪个服务：原来的服务器地址还对得上就留在原服务；对得上别的服务就挪过去；
/// 原服务的地址整个换了就改原服务的环境地址；都不是就按服务器新建一个服务。
fn target(store: &mut ApiStore, endpoint: &ApiEndpoint, notes: &mut Vec<String>) -> usize {
    let url = endpoint.url.trim();
    let origin = endpoint
        .origin
        .as_ref()
        .and_then(|origin| find(store, &origin.service));
    if let Some(index) = origin
        && under(url, &base_of(store, index))
    {
        return index;
    }
    let best = (0..store.services.len())
        .map(|index| (index, base_of(store, index)))
        .filter(|(_, base)| under(url, base))
        // 地址最长的；一样长（副本服务）取靠前的原服务。
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then(b.0.cmp(&a.0)))
        .map(|(index, _)| index);
    if let Some(index) = best {
        return index;
    }
    let Some((host, path)) = split_host(url) else {
        // 还没填服务器地址：放进「未填服务器地址」。
        if let Some(index) = store
            .services
            .iter()
            .position(|s| base_of_doc(s).is_empty())
        {
            return index;
        }
        let id = service_id("unsorted", "", store);
        store.services.push(Service {
            id,
            doc: empty_doc("未填服务器地址", ""),
        });
        return store.services.len() - 1;
    };
    if let Some(index) = origin {
        let old = base_of(store, index);
        let old_path = split_host(&old).map(|(_, p)| p).unwrap_or_default();
        let new_base = if !old_path.is_empty() && path.starts_with(&old_path) {
            format!("{host}{old_path}")
        } else {
            host.clone()
        };
        let id = store.services[index].id.clone();
        store
            .envs
            .services
            .entry(id)
            .or_default()
            .set_base(&new_base);
        notes.push(format!(
            "服务「{}」的服务器地址改成了 {new_base}（这个服务的接口都跟着改）",
            store.services[index].title()
        ));
        return index;
    }
    let title = host
        .split_once("://")
        .map_or(host.as_str(), |(_, h)| h)
        .to_string();
    let id = service_id(&title, &host, store);
    store.services.push(Service {
        id,
        doc: empty_doc(&title, &host),
    });
    store.services.len() - 1
}

/// 文件里写的服务器地址（不看环境）。
fn base_of_doc(service: &Service) -> String {
    base_url(service, None)
}

/// 同一服务器上、这条路径这个方法还空着的服务；都占了就复制一个服务出来。
/// OpenAPI 里同一路径同一方法只能有一个操作，旧接口里同一地址按请求体分成几个的放到副本里。
fn free_service(store: &mut ApiStore, index: usize, path: &str, method: ApiMethod) -> usize {
    if !occupied(&store.services[index].doc, path, method) {
        return index;
    }
    let base = base_of(store, index);
    if let Some(sibling) = (0..store.services.len()).find(|&i| {
        i != index && base_of(store, i) == base && !occupied(&store.services[i].doc, path, method)
    }) {
        return sibling;
    }
    let original = store.services[index].clone();
    let mut doc = original.doc.clone();
    doc["paths"] = json!({});
    let title = format!("{}（{}）", original.title(), store.services.len() + 1);
    doc["info"]["title"] = json!(title);
    let id = unique(
        &original.id,
        |candidate| store.services.iter().any(|s| s.id == candidate),
        "-",
    );
    if let Some(env) = store.envs.services.get(&original.id).cloned() {
        store.envs.services.insert(id.clone(), env);
    }
    store.services.push(Service { id, doc });
    store.services.len() - 1
}

/// 按模板重新拆成 OpenAPI 操作并放进服务。
fn rebuild(store: &mut ApiStore, endpoint: &ApiEndpoint, notes: &mut Vec<String>) {
    let mut index = target(store, endpoint, notes);
    let base = base_of(store, index);
    let url = endpoint.url.trim();
    let rest = if under(url, &base) {
        url[base.len()..].to_string()
    } else {
        match split_host(url) {
            Some((_, rest)) => rest,
            None => url.to_string(),
        }
    };
    let (path_part, query_part) = match rest.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (rest.clone(), String::new()),
    };
    let mut path_part = if path_part.starts_with('/') {
        path_part
    } else {
        format!("/{path_part}")
    };
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut params: Vec<Value> = Vec::new();
    let mut security: Vec<(Value, String)> = Vec::new();
    let mut template = Map::new();
    let input = |name: &str| endpoint.inputs.iter().find(|input| input.name == name);

    // 路径
    if path_part.contains("{secret:") {
        template.insert("path".into(), json!(path_part));
        path_part = path_part.replace("{secret:", "{secret_");
        notes.push(format!("接口「{}」的路径里带密钥，原样保留", endpoint.name));
    } else {
        let mut key = path_part.clone();
        for caps in api::PLACEHOLDER.captures_iter(&path_part) {
            let name = &caps[2];
            let wire = match input(name) {
                Some(found) if !used.contains(name) => {
                    used.insert(name.to_string());
                    let wire = if found.wire.is_empty() {
                        name.to_string()
                    } else {
                        found.wire.clone()
                    };
                    params.push(param(&wire, "path", found));
                    wire
                }
                _ => name.to_string(),
            };
            key = key.replace(&caps[0], &format!("{{{wire}}}"));
        }
        path_part = key;
    }

    // 查询参数
    let mut query_template = Vec::new();
    let mut seen = BTreeSet::new();
    for pair in query_part.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let wire = percent_decode(key);
        if !seen.insert(wire.clone()) {
            query_template.push(json!(pair));
            continue;
        }
        if let Some(name) = api::sole_input(value)
            && let Some(found) = input(name)
            && !used.contains(name)
        {
            used.insert(name.to_string());
            params.push(param(&wire, "query", found));
        } else if let Some(secret) = api::sole_secret(value) {
            security.push((
                json!({"type": "apiKey", "in": "query", "name": wire}),
                secret.into(),
            ));
        } else if !api::has_placeholder(value) {
            params.push(fixed_param(&wire, "query", &percent_decode(value)));
        } else {
            query_template.push(json!(pair));
        }
    }

    // 请求头
    let mut header_template = Vec::new();
    let mut seen_headers = BTreeSet::new();
    for header in &endpoint.headers {
        let name = header.name.trim();
        if name.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        let value = header.value.as_str();
        let keep_raw = || json!({"name": name, "value": value});
        if !seen_headers.insert(lower.clone()) {
            header_template.push(keep_raw());
            continue;
        }
        if lower == "authorization" {
            let scheme = ["Bearer ", "Basic "].iter().find_map(|prefix| {
                let rest = value.get(..prefix.len())?;
                rest.eq_ignore_ascii_case(prefix)
                    .then(|| api::sole_secret(&value[prefix.len()..]))
                    .flatten()
                    .map(|secret| (prefix.trim().to_ascii_lowercase(), secret))
            });
            match scheme {
                Some((word, secret)) => {
                    security.push((json!({"type": "http", "scheme": word}), secret.into()))
                }
                None => match api::sole_secret(value) {
                    Some(secret) => security.push((
                        json!({"type": "apiKey", "in": "header", "name": name}),
                        secret.into(),
                    )),
                    None => header_template.push(keep_raw()),
                },
            }
            continue;
        }
        if matches!(lower.as_str(), "cookie" | "content-type" | "accept") {
            header_template.push(keep_raw());
            continue;
        }
        if let Some(secret) = api::sole_secret(value) {
            security.push((
                json!({"type": "apiKey", "in": "header", "name": name}),
                secret.into(),
            ));
        } else if let Some(input_name) = api::sole_input(value)
            && let Some(found) = input(input_name)
            && !used.contains(input_name)
        {
            used.insert(input_name.to_string());
            params.push(param(name, "header", found));
        } else if !api::has_placeholder(value) {
            params.push(fixed_param(name, "header", value));
        } else {
            header_template.push(keep_raw());
        }
    }

    // 请求体
    let mut request_body = None;
    if endpoint.method.has_body() && !endpoint.body.trim().is_empty() {
        match body_schema(endpoint, &mut used) {
            Some((schema, required, rename)) => {
                let content_type = match endpoint.body_kind {
                    BodyKind::Json => "application/json",
                    BodyKind::Form => "application/x-www-form-urlencoded",
                };
                let mut body = json!({
                    "required": required,
                    "content": {content_type: {"schema": schema}},
                });
                if let Some(name) = rename {
                    body[X_NAME] = json!(name);
                }
                request_body = Some(body);
            }
            None => {
                template.insert("body".into(), json!(endpoint.body));
                if endpoint.body_kind == BodyKind::Form {
                    template.insert("form".into(), json!(true));
                }
            }
        }
    }
    let unused: Vec<Value> = endpoint
        .inputs
        .iter()
        .filter(|input| !used.contains(&input.name))
        .filter_map(|input| serde_json::to_value(input).ok())
        .collect();
    if !query_template.is_empty() {
        template.insert("query".into(), Value::Array(query_template));
    }
    if !header_template.is_empty() {
        template.insert("headers".into(), Value::Array(header_template));
    }
    if !unused.is_empty() {
        template.insert("inputs".into(), Value::Array(unused));
    }

    let method = endpoint.method;
    index = free_service(store, index, &path_part, method);
    let service_id = store.services[index].id.clone();
    // 请求体的说明与例子沿用原来的（原文可能是引用，在原服务的文档里解）。
    let old_body = endpoint.origin.as_ref().and_then(|origin| {
        let rb = origin.op.get("requestBody")?;
        let doc = find(store, &origin.service).map(|i| &store.services[i].doc);
        Some(doc.map_or_else(|| rb.clone(), |doc| deref(doc, rb).clone()))
    });
    let doc = &mut store.services[index].doc;
    let requirement: Map<String, Value> = security
        .iter()
        .map(|(spec, secret)| (scheme_for(doc, spec, secret), json!([])))
        .collect();

    let mut op = match &endpoint.origin {
        Some(origin) => origin.op.as_object().cloned().unwrap_or_default(),
        None => Map::new(),
    };
    for key in [
        "parameters",
        "requestBody",
        "security",
        X_TEMPLATE,
        X_INPUTS,
    ] {
        op.remove(key);
    }
    if !params.is_empty() {
        op.insert("parameters".into(), Value::Array(params));
    }
    if let Some(mut body) = request_body {
        if let Some(old) = old_body {
            if let Some(description) = old.get("description") {
                body["description"] = description.clone();
            }
            if let Some(content) = body["content"].as_object_mut() {
                for (content_type, media) in content.iter_mut() {
                    for key in ["example", "examples"] {
                        if let Some(value) =
                            old.pointer(&format!("/content/{}/{key}", pointer_token(content_type)))
                        {
                            media[key] = value.clone();
                        }
                    }
                }
            }
        }
        op.insert("requestBody".into(), body);
    }
    let root_requirement = doc
        .get("security")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_object)
        .cloned();
    if requirement.is_empty() {
        if root_requirement.is_some_and(|r| !r.is_empty()) {
            op.insert("security".into(), json!([]));
        }
    } else if root_requirement.as_ref() != Some(&requirement) {
        op.insert("security".into(), json!([requirement]));
    }
    if !template.is_empty() {
        op.insert(X_TEMPLATE.into(), Value::Object(template));
    }
    let operation_id = operation_id_for(doc, &op, endpoint, &service_id, &path_part, method);
    op.insert("operationId".into(), json!(operation_id));
    push_down_path_params(doc, &path_part);
    let service_snapshot = Service {
        id: service_id.clone(),
        doc: doc.clone(),
    };
    apply_meta(&mut op, endpoint, &service_snapshot, method, &path_part);
    set_op(doc, &path_part, method, op.clone());

    // 输入的先后与编译出来的不一样时记下来。
    let base = base_of(store, index);
    if let Some(lowered) = lower_op(&store.services[index], &base, &path_part, method) {
        let names = |inputs: &[ApiInput]| -> Vec<String> {
            inputs.iter().map(|input| input.name.clone()).collect()
        };
        if names(&lowered.inputs) != names(&endpoint.inputs) {
            op.insert(X_INPUTS.into(), json!(names(&endpoint.inputs)));
            set_op(&mut store.services[index].doc, &path_part, method, op);
        }
    }
}

/// 操作 id：原来的还能用就用原来的，否则从接口 id 取。
fn operation_id_for(
    doc: &Value,
    op: &Map<String, Value>,
    endpoint: &ApiEndpoint,
    service_id: &str,
    path: &str,
    method: ApiMethod,
) -> String {
    let taken = |candidate: &str| {
        doc.get("paths")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .any(|(other_path, item)| {
                ApiMethod::ALL.iter().any(|other| {
                    (other_path.as_str(), *other) != (path, method)
                        && item
                            .get(other.key())
                            .and_then(|o| o.get("operationId"))
                            .and_then(Value::as_str)
                            == Some(candidate)
                })
            })
    };
    if let Some(existing) = op.get("operationId").and_then(Value::as_str)
        && super::normalize::valid_operation_id(existing)
        && !taken(existing)
    {
        return existing.to_string();
    }
    let from_id = endpoint
        .id
        .strip_prefix(&format!("{service_id}."))
        .unwrap_or(&endpoint.id);
    let base: String = from_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let base = if base.trim_matches('_').is_empty() {
        super::normalize::generated_operation_id(method, path)
    } else {
        base
    };
    unique(&base, taken, "_")
}

/// 一个输入的 schema：原来的类型没变就沿用原 schema（枚举、格式、嵌套结构），说明与样例另放。
fn input_schema(input: &ApiInput) -> Value {
    let mut schema = match &input.schema {
        Some(schema) if kind_of(schema) == input.kind => schema.clone(),
        _ => schema_of_kind(input.kind),
    };
    if let Some(map) = schema.as_object_mut() {
        map.remove("description");
        map.remove("example");
    }
    schema
}

/// 输入名与 OpenAPI 名字的默认对应对不上时，写 `x-gongwen-name`。
fn needs_rename(input: &ApiInput, wire: &str) -> bool {
    input.name != identifier(wire, "\u{0}")
}

fn param(wire: &str, location: &str, input: &ApiInput) -> Value {
    let mut param = json!({"name": wire, "in": location, "schema": input_schema(input)});
    if location == "path" || input.required {
        param["required"] = json!(true);
    }
    if !input.description.trim().is_empty() {
        param["description"] = json!(input.description);
    }
    if !input.example.is_empty() {
        param["example"] = example_value(input.kind, &input.example);
    }
    if needs_rename(input, wire) {
        param[X_NAME] = json!(input.name);
    }
    if input.comma {
        param["style"] = json!("form");
        param["explode"] = json!(false);
    }
    param
}

/// 写死的参数：只允许一个值。
fn fixed_param(wire: &str, location: &str, value: &str) -> Value {
    json!({
        "name": wire,
        "in": location,
        "required": true,
        "schema": {"type": "string", "enum": [value]},
    })
}

/// 请求体模板 → (schema, 必填, 整段请求体的输入名)；拆不开返回 None。
fn body_schema(
    endpoint: &ApiEndpoint,
    used: &mut BTreeSet<String>,
) -> Option<(Value, bool, Option<String>)> {
    let template: Value = serde_json::from_str(&endpoint.body).ok()?;
    let input = |name: &str| endpoint.inputs.iter().find(|input| input.name == name);
    match &template {
        Value::Object(map) => {
            let mut properties = Map::new();
            let mut required = Vec::new();
            let mut claimed = Vec::new();
            for (wire, value) in map {
                if let Some(name) = value.as_str().and_then(api::sole_input) {
                    let found = input(name)?;
                    if used.contains(name) || claimed.contains(&name) {
                        return None;
                    }
                    claimed.push(name);
                    let mut schema = input_schema(found);
                    if let Some(object) = schema.as_object_mut() {
                        if !found.description.trim().is_empty() {
                            object.insert("description".into(), json!(found.description));
                        }
                        if !found.example.is_empty() {
                            object.insert(
                                "example".into(),
                                example_value(found.kind, &found.example),
                            );
                        }
                        if needs_rename(found, wire) {
                            object.insert(X_NAME.into(), json!(found.name));
                        }
                    }
                    if found.required {
                        required.push(json!(wire));
                    }
                    properties.insert(wire.clone(), schema);
                } else if !contains_placeholder(value) {
                    properties.insert(wire.clone(), json!({"enum": [value]}));
                    required.push(json!(wire));
                } else {
                    return None;
                }
            }
            used.extend(claimed.into_iter().map(str::to_string));
            let mut schema = json!({"type": "object", "properties": properties});
            if !required.is_empty() {
                schema["required"] = Value::Array(required);
            }
            Some((schema, true, None))
        }
        Value::String(text) => {
            let name = api::sole_input(text)?;
            let found = input(name)?;
            if used.contains(name) {
                return None;
            }
            used.insert(name.to_string());
            let mut schema = input_schema(found);
            if let Some(object) = schema.as_object_mut() {
                if !found.description.trim().is_empty() {
                    object.insert("description".into(), json!(found.description));
                }
                if !found.example.is_empty() {
                    object.insert("example".into(), example_value(found.kind, &found.example));
                }
            }
            let rename = (name != "body").then(|| name.to_string());
            Some((schema, found.required, rename))
        }
        _ => None,
    }
}

fn contains_placeholder(value: &Value) -> bool {
    match value {
        Value::String(text) => api::has_placeholder(text),
        Value::Array(items) => items.iter().any(contains_placeholder),
        Value::Object(map) => map.values().any(contains_placeholder),
        _ => false,
    }
}

/// 找到或新建鉴权方式，返回它在 `securitySchemes` 里的名字。
fn scheme_for(doc: &mut Value, spec: &Value, secret: &str) -> String {
    let same = |scheme: &Value| {
        let field = |v: &Value, key: &str| {
            v.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
        };
        ["type", "in", "name", "scheme"]
            .iter()
            .all(|key| field(scheme, key) == field(spec, key))
    };
    if let Some(schemes) = doc
        .pointer("/components/securitySchemes")
        .and_then(Value::as_object)
    {
        for (name, scheme) in schemes {
            let scheme = deref(doc, scheme);
            if same(scheme) && secret_of(name, scheme) == secret {
                return name.clone();
            }
        }
    }
    if !doc.get("components").is_some_and(Value::is_object) {
        doc["components"] = json!({});
    }
    if !doc["components"]
        .get("securitySchemes")
        .is_some_and(Value::is_object)
    {
        doc["components"]["securitySchemes"] = json!({});
    }
    let schemes = doc["components"]["securitySchemes"]
        .as_object_mut()
        .expect("securitySchemes 是对象");
    let name = unique(secret, |candidate| schemes.contains_key(candidate), "_");
    let mut scheme = spec.clone();
    if crate::agent::api_import::redact::secret_name(&name) != secret {
        scheme[X_SECRET] = json!(secret);
    }
    schemes.insert(name.clone(), scheme);
    name
}
