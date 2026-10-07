//! 接口 → 模型工具（`docs/api-workbench.md` 6.3、6.4、6.7）。
//!
//! 自主步骤与「试一下」里的「说一句话让 AI 调」都从这里拿工具，模型在两处看到的说明完全一致：
//! - 一个接口一个工具：名字 `http_call__<接口 id>`（只有 ASCII，超长截短加哈希），说明是用途 +
//!   几条用例的参数，参数是**完整的 JSON Schema**（类型、枚举、嵌套、必填，取自 OpenAPI 原文）；
//! - 调用前按 schema 校验（[`validate`]），不合格不发请求，把错误、参数签名与一个正确示例回给
//!   模型让它自己改；
//! - 一次能用的接口超过 [`TWO_LEVEL_AT`] 个时不逐个给，只给「搜接口」「调接口」两个工具
//!   （[`ApiTools::Search`]），免得撑爆小模型的上下文。
//!
//! 只有「只查询且给 AI 用」的接口会编成工具（[`usable`]），这是红线，不在这里放宽。

use crate::agent::api::{ApiEndpoint, ApiStore, InputKind};
use crate::agent::argcheck;
use crate::agent::board::value_to_text;
use crate::agent::toolcall::{ToolCall, ToolSpec, wire_name};
use serde_json::{Map, Value, json};

/// 一次能用的接口超过这么多，就改成两级访问。
pub(crate) const TWO_LEVEL_AT: usize = 12;
/// 「搜接口」一次最多返回几个。
const SEARCH_TOP: usize = 5;
/// 两级访问时的工具名。
pub(crate) const SEARCH_TOOL: &str = "api_search";
pub(crate) const CALL_TOOL: &str = "api_call";

/// 这个接口能不能给 AI 调：只查询、开放给 AI、配置没问题。
pub(crate) fn usable(endpoint: &ApiEndpoint) -> bool {
    endpoint.readonly && endpoint.ai && endpoint.unsupported.is_empty()
}

/// 技能里的接口声明展开成接口 id：`http.call:<id>`、`http.call:<服务>.*`、`http.call:*`。
/// 通配只展开给 AI 用的接口；点名的照写（没配的、不能给 AI 的由调用时报错，方便技能作者看到）。
pub(crate) fn expand(declared: &str, apis: &ApiStore) -> Vec<String> {
    let Some(target) = declared.strip_prefix("http.call:") else {
        return Vec::new();
    };
    let wildcard = |service: Option<&str>| -> Vec<String> {
        apis.endpoints
            .iter()
            .filter(|e| usable(e))
            .filter(|e| match service {
                None => true,
                Some(service) => e.origin.as_ref().is_some_and(|o| o.service == service),
            })
            .map(|e| e.id.clone())
            .collect()
    };
    match target {
        "*" => wildcard(None),
        _ => match target.strip_suffix(".*") {
            Some(service) => wildcard(Some(service)),
            None => vec![target.to_string()],
        },
    }
}

/// 技能允不允许调这个接口：点名的、`http.call` 整个开放的，或者通配命中的。
pub(crate) fn allowed(tools: &[String], api: &str, apis: &ApiStore) -> bool {
    tools.iter().any(|declared| {
        declared == "http.call"
            || *declared == format!("http.call:{api}")
            || (declared.starts_with("http.call:")
                && (declared.ends_with(".*") || declared == "http.call:*")
                && expand(declared, apis).iter().any(|id| id == api))
    })
}

/// 发给模型的参数 schema：每个输入变量一个属性。
pub(crate) fn parameters_schema(endpoint: &ApiEndpoint) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for input in &endpoint.inputs {
        let mut schema = match &input.schema {
            Some(schema) if kind_matches(schema, input.kind) => clean(schema),
            _ => kind_schema(input.kind),
        };
        let mut doc = input.description.trim().to_string();
        if doc.is_empty() {
            doc = input.name.clone();
        }
        if !input.example.trim().is_empty() {
            doc.push_str(&format!("（例如 {}）", input.example.trim()));
        }
        if let Some(map) = schema.as_object_mut() {
            map.insert("description".into(), json!(doc));
        }
        if input.required {
            required.push(json!(input.name));
        }
        properties.insert(input.name.clone(), schema);
    }
    json!({"type": "object", "properties": properties, "required": required})
}

fn kind_matches(schema: &Value, kind: InputKind) -> bool {
    super::schema::kind_of(schema) == kind
}

fn kind_schema(kind: InputKind) -> Value {
    match kind {
        InputKind::Text => json!({"type": "string"}),
        InputKind::Number => json!({"type": "number"}),
        InputKind::Bool => json!({"type": "boolean"}),
        InputKind::Json => json!({"anyOf": [{"type": "object"}, {"type": "array"}]}),
    }
}

/// JSON Schema 里模型用得上、各家服务端都认的那部分；OpenAPI 的 `nullable`、`example`、
/// `x-*` 等扩展去掉。
const KEPT: &[&str] = &[
    "type",
    "enum",
    "items",
    "properties",
    "required",
    "description",
    "format",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "default",
    "anyOf",
    "oneOf",
    "additionalProperties",
];

fn clean(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                if !KEPT.contains(&key.as_str()) {
                    continue;
                }
                let value = match key.as_str() {
                    "properties" => Value::Object(
                        value
                            .as_object()
                            .map(|props| props.iter().map(|(k, v)| (k.clone(), clean(v))).collect())
                            .unwrap_or_default(),
                    ),
                    "items" | "additionalProperties" if value.is_object() => clean(value),
                    "anyOf" | "oneOf" => Value::Array(
                        value
                            .as_array()
                            .map(|parts| parts.iter().map(clean).collect())
                            .unwrap_or_default(),
                    ),
                    _ => value.clone(),
                };
                out.insert(key.clone(), value);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// 整个接口的参数签名，一行。
pub(crate) fn signatures(endpoint: &ApiEndpoint) -> String {
    argcheck::signatures(&parameters_schema(endpoint))
}

/// 一条正确的调用示例（取第一条用例），报错时给模型照着改。
fn sample_args(endpoint: &ApiEndpoint) -> Option<String> {
    let example = endpoint.examples.first()?;
    Some(serde_json::to_string(&endpoint.args_of(example)).unwrap_or_default())
}

/// 接口编成的工具。
pub(crate) fn tool_spec(endpoint: &ApiEndpoint) -> ToolSpec {
    let mut description = format!("查询接口「{}」（只查询）", endpoint.name);
    if !endpoint.description.trim().is_empty() {
        description.push_str(&format!("：{}", endpoint.description.trim()));
    }
    // 几种用法各给一例，模型照着填（对象类参数尤其要看例子）。
    for example in endpoint.examples.iter().take(3) {
        let args = serde_json::to_string(&endpoint.args_of(example)).unwrap_or_default();
        description.push_str(&format!(
            "\n用法示例「{}」：{}",
            example.name,
            crate::agent::tools::short(&args, 400)
        ));
    }
    let schema = parameters_schema(endpoint);
    let params = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, prop)| {
            let required = endpoint
                .inputs
                .iter()
                .any(|input| input.name == *name && input.required);
            let doc = format!(
                "{}；{}",
                prop.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or(name),
                argcheck::signature(name, prop, required)
            );
            (name.clone(), required, doc)
        })
        .collect();
    ToolSpec {
        name: wire_name(&format!("http.call:{}", endpoint.id)),
        description,
        params,
        schema: Some(schema),
    }
}

/// 按 schema 校验模型给的参数。不合格返回给模型看的说明：错在哪、参数签名、正确示例。
pub(crate) fn validate(endpoint: &ApiEndpoint, args: &Map<String, Value>) -> Result<(), String> {
    let schema = parameters_schema(endpoint);
    let mut problems = Vec::new();
    for name in args.keys().filter(|name| *name != "api") {
        if !endpoint.inputs.iter().any(|input| input.name == *name) {
            problems.push(format!("没有参数 {name}"));
        }
    }
    for input in &endpoint.inputs {
        let value = match args.get(&input.name) {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) if text.trim().is_empty() => None,
            Some(value) => Some(value),
        };
        let Some(value) = value else {
            if input.required {
                problems.push(format!("缺少必填参数 {}", input.name));
            }
            continue;
        };
        if let Err(problem) = check_value(&input.name, value, &schema["properties"][&input.name]) {
            problems.push(problem);
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(argcheck::reject(
        &problems,
        "接口",
        &schema,
        sample_args(endpoint).as_deref(),
    ))
}

/// 一个值合不合 schema：类型、枚举、数组元素与对象必填（各查一层，够发现常见填错）。
fn check_value(name: &str, value: &Value, schema: &Value) -> Result<(), String> {
    // 整段 JSON 的参数，模型常把对象写成 JSON 字符串：先读出来再查。
    let parsed;
    let value = match value {
        Value::String(text)
            if matches!(
                schema.get("type").and_then(Value::as_str),
                Some("array" | "object")
            ) || schema.get("anyOf").is_some() =>
        {
            parsed = serde_json::from_str::<Value>(text.trim())
                .map_err(|_| format!("参数 {name} 要是 JSON（对象或数组），收到「{text}」"))?;
            &parsed
        }
        other => other,
    };
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        let text = value_to_text(value);
        if !options.iter().any(|option| value_to_text(option) == text) {
            let list: Vec<String> = options.iter().map(value_to_text).collect();
            return Err(format!(
                "参数 {name} 只能是 {} 之一，收到「{text}」",
                list.join("、")
            ));
        }
        return Ok(());
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("integer" | "number") => {
            let number = match value {
                Value::Number(n) => n.as_f64(),
                Value::String(text) => text.trim().parse::<f64>().ok(),
                _ => None,
            };
            match number {
                None => Err(format!(
                    "参数 {name} 要是数字，收到「{}」",
                    value_to_text(value)
                )),
                Some(n)
                    if schema.get("type").and_then(Value::as_str) == Some("integer")
                        && n.fract() != 0.0 =>
                {
                    Err(format!("参数 {name} 要是整数，收到 {n}"))
                }
                Some(_) => Ok(()),
            }
        }
        Some("boolean") => match value {
            Value::Bool(_) => Ok(()),
            Value::String(text) if matches!(text.trim(), "true" | "false") => Ok(()),
            other => Err(format!(
                "参数 {name} 要是 true 或 false，收到「{}」",
                value_to_text(other)
            )),
        },
        Some("array") => match value {
            Value::Array(items) => {
                let item_schema = schema.get("items").cloned().unwrap_or(Value::Null);
                items.iter().enumerate().try_for_each(|(i, item)| {
                    check_value(&format!("{name}[{i}]"), item, &item_schema)
                })
            }
            other => Err(format!(
                "参数 {name} 要是数组，收到「{}」",
                value_to_text(other)
            )),
        },
        Some("object") => match value {
            Value::Object(map) => {
                for field in schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !map.contains_key(field) {
                        return Err(format!("参数 {name} 缺少字段 {field}"));
                    }
                }
                Ok(())
            }
            other => Err(format!(
                "参数 {name} 要是 JSON 对象，收到「{}」",
                value_to_text(other)
            )),
        },
        Some("string") => match value {
            Value::Object(_) | Value::Array(_) => {
                Err(format!("参数 {name} 要是文字，收到一段 JSON"))
            }
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

/// 一组接口怎么交给模型。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ApiTools {
    /// 一个接口一个工具：(工具名, 接口 id)。
    Direct(Vec<(String, String)>),
    /// 接口太多：只给「搜接口」「调接口」，这里是能调的接口 id。
    Search(Vec<String>),
}

/// 模型的一次调用落到哪。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Resolved {
    /// 调这个接口，参数如下（不含 `api`）。
    Call {
        api: String,
        args: Map<String, Value>,
    },
    /// 直接回给模型的文字（搜接口的结果、拒绝的原因）。
    Reply(String),
    /// 不是接口工具，交给别的工具处理。
    Other,
}

impl ApiTools {
    /// 按接口个数选方式。`ids` 里不存在的接口照样编（说明里写清没配置），调用时报错。
    pub(crate) fn new(ids: Vec<String>) -> Self {
        if ids.len() > TWO_LEVEL_AT {
            Self::Search(ids)
        } else {
            Self::Direct(
                ids.into_iter()
                    .map(|id| (wire_name(&format!("http.call:{id}")), id))
                    .collect(),
            )
        }
    }

    /// 发给模型的工具说明。
    pub(crate) fn specs(&self, apis: &ApiStore) -> Vec<ToolSpec> {
        match self {
            Self::Direct(list) => list
                .iter()
                .map(|(name, id)| match apis.get(id) {
                    Some(endpoint) => tool_spec(endpoint),
                    None => ToolSpec {
                        name: name.clone(),
                        description: format!("接口「{id}」还没有配置，调用会失败"),
                        params: Vec::new(),
                        schema: None,
                    },
                })
                .collect(),
            Self::Search(ids) => vec![
                ToolSpec {
                    name: SEARCH_TOOL.into(),
                    description: format!(
                        "在能用的 {} 个数据接口里按关键词找接口，返回最相关的几个：接口 id、用途与参数。\
                         调接口前先用它找",
                        ids.len()
                    ),
                    params: vec![("query".into(), true, "要查什么，例如「森林火灾 起数」".into())],
                    schema: Some(json!({
                        "type": "object",
                        "properties": {"query": {"type": "string", "description": "要查什么，例如「森林火灾 起数」"}},
                        "required": ["query"],
                    })),
                },
                ToolSpec {
                    name: CALL_TOOL.into(),
                    description: "调用一个数据接口（只查询）。api 填 api_search 找到的接口 id，args 按它列出的参数填"
                        .into(),
                    params: vec![
                        ("api".into(), true, "接口 id".into()),
                        ("args".into(), true, "参数，JSON 对象".into()),
                    ],
                    schema: Some(json!({
                        "type": "object",
                        "properties": {
                            "api": {"type": "string", "description": "接口 id（api_search 返回的）"},
                            "args": {"type": "object", "description": "参数，按 api_search 列出的参数填"},
                        },
                        "required": ["api", "args"],
                    })),
                },
            ],
        }
    }

    /// 模型的一次调用落到哪个接口。
    pub(crate) fn resolve(&self, call: &ToolCall, apis: &ApiStore) -> Resolved {
        let args = match &call.arguments {
            Value::Object(map) => map.clone(),
            _ => Map::new(),
        };
        match self {
            Self::Direct(list) => match list.iter().find(|(name, _)| *name == call.name) {
                Some((_, id)) => Resolved::Call {
                    api: id.clone(),
                    args,
                },
                None => Resolved::Other,
            },
            Self::Search(ids) if call.name == SEARCH_TOOL => {
                let query = args.get("query").map(value_to_text).unwrap_or_default();
                Resolved::Reply(search(ids, apis, &query))
            }
            Self::Search(ids) if call.name == CALL_TOOL => {
                let api = args.get("api").map(value_to_text).unwrap_or_default();
                if !ids.contains(&api) {
                    return Resolved::Reply(format!(
                        "没有能用的接口「{api}」。先用 {SEARCH_TOOL} 找到接口 id 再调。"
                    ));
                }
                let args = match args.get("args") {
                    Some(Value::Object(map)) => map.clone(),
                    Some(Value::String(text)) => serde_json::from_str(text).unwrap_or_default(),
                    _ => Map::new(),
                };
                Resolved::Call { api, args }
            }
            Self::Search(_) => Resolved::Other,
        }
    }
}

/// 检索词切成关键词（空白与常见标点分隔，转小写）。
pub(crate) fn query_words(query: &str) -> Vec<String> {
    query
        .split(|c: char| c.is_whitespace() || "，,、；;。".contains(c))
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// 关键词命中打分，`name`、`rest` 要先转小写：名称里整词命中 4 分，其余文字里整词命中 2 分；
/// 整词都没命中时（中文没有空格，「查工作日」整句对不上）按两字一组再试，过半的组命中才给 1 分——
/// 不然「工作日」会因为「工作」二字跟所有写「工作稿」的工具打成平手。
pub(crate) fn match_score(words: &[String], name: &str, rest: &str) -> usize {
    words
        .iter()
        .map(|w| {
            let whole =
                name.contains(w.as_str()) as usize * 4 + rest.contains(w.as_str()) as usize * 2;
            if whole > 0 || w.chars().count() <= 2 {
                return whole;
            }
            let chars: Vec<char> = w.chars().collect();
            let pairs = chars.len() - 1;
            let hits = chars
                .windows(2)
                .filter(|pair| {
                    let pair: String = pair.iter().collect();
                    name.contains(&pair) || rest.contains(&pair)
                })
                .count();
            usize::from(hits * 2 > pairs)
        })
        .sum()
}

/// 接口参与搜索的文字：(名称, 用途 + id + 参数说明)，都已转小写。
pub(crate) fn search_text(endpoint: &ApiEndpoint) -> (String, String) {
    let rest = format!(
        "{} {} {}",
        endpoint.description,
        endpoint.id,
        endpoint
            .inputs
            .iter()
            .map(|i| i.description.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    );
    (endpoint.name.to_lowercase(), rest.to_lowercase())
}

/// 搜接口：按名称、用途、参数说明里命中的关键词打分，返回前几个的 id、用途与参数签名。
pub(crate) fn search(ids: &[String], apis: &ApiStore, query: &str) -> String {
    let words = query_words(query);
    let mut scored: Vec<(usize, &ApiEndpoint)> = ids
        .iter()
        .filter_map(|id| apis.get(id))
        .map(|endpoint| {
            let (name, rest) = search_text(endpoint);
            (match_score(&words, &name, &rest), endpoint)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    if scored.is_empty() {
        return format!("没找到和「{query}」相关的接口，换个说法再搜。");
    }
    let lines: Vec<String> = scored
        .iter()
        .take(SEARCH_TOP)
        .map(|(_, endpoint)| {
            format!(
                "- 接口 id：{}；{}：{}；参数：{}",
                endpoint.id,
                endpoint.name,
                crate::agent::tools::short(endpoint.description.trim(), 120),
                signatures(endpoint)
            )
        })
        .collect();
    format!(
        "找到 {} 个相关接口（列出前 {}）：\n{}\n用 {CALL_TOOL} 调用，api 填接口 id。",
        scored.len(),
        lines.len(),
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::api::{ApiExample, ApiInput};

    fn endpoint() -> ApiEndpoint {
        ApiEndpoint {
            id: "stat.fire".into(),
            name: "森林火灾统计".into(),
            description: "按地区、年份查森林火灾起数".into(),
            inputs: vec![
                ApiInput {
                    name: "region".into(),
                    required: true,
                    description: "地区".into(),
                    schema: Some(
                        json!({"type": "string", "enum": ["全省", "甲市"], "nullable": true}),
                    ),
                    ..ApiInput::default()
                },
                ApiInput {
                    name: "year".into(),
                    kind: InputKind::Number,
                    schema: Some(json!({"type": "integer", "example": 2025})),
                    ..ApiInput::default()
                },
                ApiInput {
                    name: "tags".into(),
                    kind: InputKind::Json,
                    schema: Some(json!({"type": "array", "items": {"type": "string"}})),
                    ..ApiInput::default()
                },
            ],
            examples: vec![ApiExample {
                name: "全省".into(),
                args: [("region".to_string(), "全省".to_string())].into(),
                ..ApiExample::default()
            }],
            ..ApiEndpoint::default()
        }
    }

    #[test]
    fn schema_keeps_types_and_enums_and_drops_openapi_extras() {
        let schema = parameters_schema(&endpoint());
        assert_eq!(
            schema["properties"]["region"]["enum"],
            json!(["全省", "甲市"])
        );
        assert!(schema["properties"]["region"].get("nullable").is_none());
        assert_eq!(schema["properties"]["year"]["type"], "integer");
        assert!(schema["properties"]["year"].get("example").is_none());
        assert_eq!(schema["required"], json!(["region"]));
        let spec = tool_spec(&endpoint());
        assert_eq!(spec.name, "http_call__stat_fire");
        assert_eq!(spec.to_native()["function"]["parameters"], schema);
        assert!(signatures(&endpoint()).contains(r#"region: "全省"|"甲市"（必填）"#));
    }

    #[test]
    fn validation_explains_and_shows_a_correct_example() {
        let e = endpoint();
        let ok = json!({"region": "全省", "year": "2025", "tags": "[\"a\"]"});
        assert_eq!(validate(&e, ok.as_object().unwrap()), Ok(()));
        let bad = json!({"region": "全国", "year": 2025.5, "tags": [{"a": 1}], "regoin": "x"});
        let message = validate(&e, bad.as_object().unwrap()).unwrap_err();
        for part in [
            "没有参数 regoin",
            "只能是 全省、甲市 之一",
            "要是整数",
            "tags[0]",
            "正确的例子",
        ] {
            assert!(message.contains(part), "{part}：{message}");
        }
        assert!(
            validate(&e, &Map::new())
                .unwrap_err()
                .contains("缺少必填参数 region")
        );
    }

    #[test]
    fn many_apis_switch_to_search_and_call() {
        let mut apis = ApiStore::default();
        for i in 0..20 {
            let mut e = endpoint();
            e.id = format!("svc.op{i}");
            e.name = if i == 7 {
                "人口统计".into()
            } else {
                format!("接口{i}")
            };
            e.description = if i == 7 {
                "按地区查常住人口".into()
            } else {
                "别的".into()
            };
            apis.endpoints.push(e);
        }
        let ids: Vec<String> = apis.endpoints.iter().map(|e| e.id.clone()).collect();
        let tools = ApiTools::new(ids);
        assert_eq!(tools.specs(&apis).len(), 2);
        let found = tools.resolve(
            &ToolCall {
                id: "1".into(),
                name: SEARCH_TOOL.into(),
                arguments: json!({"query": "常住人口"}),
            },
            &apis,
        );
        match found {
            Resolved::Reply(text) => assert!(text.contains("接口 id：svc.op7"), "{text}"),
            other => panic!("{other:?}"),
        }
        let call = tools.resolve(
            &ToolCall {
                id: "2".into(),
                name: CALL_TOOL.into(),
                arguments: json!({"api": "svc.op7", "args": {"region": "全省"}}),
            },
            &apis,
        );
        assert_eq!(
            call,
            Resolved::Call {
                api: "svc.op7".into(),
                args: json!({"region": "全省"}).as_object().unwrap().clone()
            }
        );
        let few = ApiTools::new(vec!["svc.op1".into()]);
        assert_eq!(few.specs(&apis)[0].name, "http_call__svc_op1");
    }

    #[test]
    fn wildcards_expand_to_usable_endpoints_of_a_service() {
        let mut apis = ApiStore::default();
        for (id, service, readonly) in [("a.x", "a", true), ("a.y", "a", false), ("b.z", "b", true)]
        {
            let mut e = endpoint();
            e.id = id.into();
            e.readonly = readonly;
            e.origin = Some(crate::agent::api::OpOrigin {
                service: service.into(),
                path: "/".into(),
                method: crate::agent::api::ApiMethod::Get,
                op: Value::Null,
            });
            apis.endpoints.push(e);
        }
        assert_eq!(expand("http.call:a.*", &apis), ["a.x"]);
        assert_eq!(expand("http.call:*", &apis), ["a.x", "b.z"]);
        assert_eq!(expand("http.call:a.y", &apis), ["a.y"], "点名的照写");
        let tools = vec!["http.call:a.*".to_string()];
        assert!(allowed(&tools, "a.x", &apis));
        assert!(!allowed(&tools, "b.z", &apis));
        assert!(!allowed(&tools, "a.y", &apis), "会改数据的通配不放进来");
    }
}
