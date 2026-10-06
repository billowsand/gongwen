//! 把文档里的请求转成接口模板：查询参数与请求体字段换成 `{变量}`，样例值留作测试用的
//! 样例，凭据换成 `{secret:名字}`。

use super::redact::{Secrets, is_secret_key};
use crate::agent::api::{ApiHeader, ApiInput, InputKind};
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

/// 路径里的变量：`{id}`、`:id`。
static PATH_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}|/:([A-Za-z_][A-Za-z0-9_]*)").expect("路径变量正则")
});

/// 发请求时由程序自己处理、不用写进配置的请求头。
const SKIPPED_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "host",
    "user-agent",
    "accept-encoding",
    "connection",
];

/// 变量名只能用英文字母、数字、下划线，且不以数字开头。
pub(super) fn ident(name: &str) -> String {
    let name: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = name.trim_matches('_').to_string();
    if name.is_empty() {
        "value".into()
    } else if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("p_{name}")
    } else {
        name
    }
}

/// 登记一个输入变量；同名的沿用（同一个参数在地址和请求体里都出现时只算一个）。
fn add_input(inputs: &mut Vec<ApiInput>, name: &str, kind: InputKind, example: &str) -> String {
    let name = ident(name);
    if !inputs.iter().any(|input| input.name == name) {
        inputs.push(ApiInput {
            name: name.clone(),
            kind,
            example: example.to_string(),
            ..ApiInput::default()
        });
    }
    name
}

/// `%E5%85%A8` 一类的转义还原成文字（解不开就原样）。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            let byte = (high * 16 + low) as u8;
            out.push(byte);
            i += 3;
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// 地址模板：路径变量登记成输入，查询参数换成 `{变量}`，凭据参数换成 `{secret:名字}`。
pub(super) fn template_url(url: &str, inputs: &mut Vec<ApiInput>, secrets: &mut Secrets) -> String {
    let (base, query) = url
        .split_once('?')
        .map_or((url, None), |(b, q)| (b, Some(q)));
    let base = PATH_VAR
        .replace_all(base, |caps: &regex::Captures<'_>| match caps.get(1) {
            Some(name) => format!(
                "{{{}}}",
                add_input(inputs, name.as_str(), InputKind::Text, "")
            ),
            None => format!("/{{{}}}", add_input(inputs, &caps[2], InputKind::Text, "")),
        })
        .into_owned();
    let Some(query) = query else {
        return base;
    };
    let pairs: Vec<String> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_decode(value);
            if is_secret_key(key) {
                format!("{key}={{secret:{}}}", secrets.add(key, &value))
            } else if value.starts_with('{') && value.ends_with('}') {
                // 文档本来就写成了 `{变量}`。
                let name = add_input(inputs, value.trim_matches(['{', '}']), InputKind::Text, "");
                format!("{key}={{{name}}}")
            } else {
                let name = add_input(inputs, key, InputKind::Text, &value);
                format!("{key}={{{name}}}")
            }
        })
        .collect();
    format!("{base}?{}", pairs.join("&"))
}

/// 请求头：凭据提成密钥，程序会自己带的头不写。
pub(super) fn template_headers(
    headers: &[(String, String)],
    secrets: &mut Secrets,
) -> Vec<ApiHeader> {
    headers
        .iter()
        .filter(|(name, _)| !SKIPPED_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
        .map(|(name, value)| ApiHeader {
            name: name.clone(),
            value: if is_secret_key(name) {
                secrets.header_value(name, value)
            } else {
                value.clone()
            },
        })
        .collect()
}

/// 请求体模板：叶子字段换成 `"{变量}"`，凭据字段换成 `"{secret:名字}"`。
/// 数组、以及 `json_keys` 里点名的顶层对象（文档说它是 map / object，键由调用方定）整段做成
/// 一个 JSON 参数，样例照录；其余对象往下拆。
pub(super) fn template_body(
    value: &Value,
    inputs: &mut Vec<ApiInput>,
    secrets: &mut Secrets,
    json_keys: &[String],
) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                let whole = matches!(child, Value::Array(_))
                    || (child.is_object() && json_keys.iter().any(|k| k == key));
                let templated = match child {
                    _ if whole => {
                        let name = add_input(inputs, key, InputKind::Json, &child.to_string());
                        Value::String(format!("{{{name}}}"))
                    }
                    Value::Object(_) => template_body(child, inputs, secrets, &[]),
                    Value::Null => Value::Null,
                    scalar => {
                        let text = match scalar {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        if is_secret_key(key) {
                            Value::String(format!("{{secret:{}}}", secrets.add(key, &text)))
                        } else if let Some(name) = lone_placeholder(&text) {
                            // 文档或模型已经写成 `"{变量}"`。
                            Value::String(format!(
                                "{{{}}}",
                                add_input(inputs, &name, InputKind::Text, "")
                            ))
                        } else {
                            let kind = match scalar {
                                Value::Number(_) => InputKind::Number,
                                Value::Bool(_) => InputKind::Bool,
                                _ => InputKind::Text,
                            };
                            let name = add_input(inputs, key, kind, &text);
                            Value::String(format!("{{{name}}}"))
                        }
                    }
                };
                out.insert(key.clone(), templated);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn lone_placeholder(text: &str) -> Option<String> {
    let inner = text.strip_prefix('{')?.strip_suffix('}')?;
    (!inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .then(|| inner.to_string())
}

/// 请求体是不是表单写法（`a=1&b=2`）。
pub(super) fn looks_like_form(body: &str) -> bool {
    let body = body.trim();
    !body.starts_with(['{', '[']) && body.contains('=')
}

/// 从路径里取一个英文 id：`/api/policy/search` → `policy_search`。
pub(super) fn id_from_path(url: &str) -> String {
    let path = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest.split_once('/').map_or("", |(_, p)| p));
    let path = path.split('?').next().unwrap_or_default();
    let segments: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty() && !s.contains(['{', ':']) && !s.eq_ignore_ascii_case("api"))
        .filter(|s| !s.starts_with('v') || !s[1..].chars().all(|c| c.is_ascii_digit()))
        .collect();
    let tail: Vec<&str> = segments.iter().rev().take(2).rev().copied().collect();
    let id = ident(&tail.join("_")).to_ascii_lowercase();
    if id == "value" { "api".into() } else { id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_parameters_become_inputs_and_tokens_become_secrets() {
        let mut inputs = Vec::new();
        let mut secrets = Secrets::default();
        let url = template_url(
            "http://x/api/policy/{policyId}/items?region=%E5%85%A8%E7%9C%81&page-size=10&access_token=abcdef123",
            &mut inputs,
            &mut secrets,
        );
        assert_eq!(
            url,
            "http://x/api/policy/{policyId}/items?region={region}&page-size={page_size}&access_token={secret:access_token}"
        );
        let names: Vec<&str> = inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["policyId", "region", "page_size"]);
        assert_eq!(inputs[1].example, "全省");
        assert_eq!(
            secrets.found,
            [("access_token".to_string(), "abcdef123".to_string())]
        );

        let mut inputs = Vec::new();
        assert_eq!(
            template_url("/api/doc/:id", &mut inputs, &mut secrets),
            "/api/doc/{id}"
        );
    }

    #[test]
    fn body_fields_become_typed_inputs() {
        let mut inputs = Vec::new();
        let mut secrets = Secrets::default();
        let body = template_body(
            &json!({"keyword": "中小企业", "page": {"size": 10, "exact": true}, "ids": [1, 2], "appSecret": "s3", "q": "{query}"}),
            &mut inputs,
            &mut secrets,
            &[],
        );
        assert_eq!(
            body,
            json!({"keyword": "{keyword}", "page": {"size": "{size}", "exact": "{exact}"}, "ids": "{ids}", "appSecret": "{secret:appsecret}", "q": "{query}"})
        );
        let kinds: Vec<(&str, InputKind, &str)> = inputs
            .iter()
            .map(|i| (i.name.as_str(), i.kind, i.example.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("keyword", InputKind::Text, "中小企业"),
                ("size", InputKind::Number, "10"),
                ("exact", InputKind::Bool, "true"),
                ("ids", InputKind::Json, "[1,2]"),
                ("query", InputKind::Text, ""),
            ],
            "数组整段做成 JSON 参数"
        );
    }

    #[test]
    fn ids_come_from_the_path() {
        assert_eq!(
            id_from_path("http://x:80/api/v1/policy/search?a=1"),
            "policy_search"
        );
        assert_eq!(id_from_path("/stat/{id}"), "stat");
        assert_eq!(id_from_path("http://x"), "api");
        assert!(looks_like_form("a=1&b=2") && !looks_like_form("{\"a\":1}"));
    }
}
