//! schema 小工具：`$ref` 解析、schema 与输入类型互转、从例子推断 schema、样例文字与值互转。

use crate::agent::api::InputKind;
use serde_json::{Map, Value, json};

/// 跟 `$ref` 最多几层（防循环引用）。
const MAX_REF_DEPTH: usize = 16;
/// 展开嵌套引用最多几层。
const MAX_DEEP: usize = 8;

/// 文件内引用 `#/components/schemas/X` 指到的值；外部引用、找不到的原样返回。
pub(super) fn deref<'a>(doc: &'a Value, value: &'a Value) -> &'a Value {
    let mut current = value;
    for _ in 0..MAX_REF_DEPTH {
        let Some(target) = current
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| reference.strip_prefix('#'))
            .and_then(|pointer| doc.pointer(pointer))
        else {
            return current;
        };
        current = target;
    }
    current
}

/// 把 schema 里的引用逐层展开成一份独立的副本（循环引用到一定深度就停，留着 `$ref`）。
pub(super) fn resolve_deep(doc: &Value, value: &Value) -> Value {
    resolve_at(doc, value, 0)
}

fn resolve_at(doc: &Value, value: &Value, depth: usize) -> Value {
    if depth > MAX_DEEP {
        return value.clone();
    }
    let value = deref(doc, value);
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), resolve_at(doc, item, depth + 1)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_at(doc, item, depth + 1))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `allOf` 合成一个对象 schema：属性与必填合并（只合一层，够请求体用）。
pub(super) fn flatten(schema: &Value) -> Value {
    let Some(parts) = schema.get("allOf").and_then(Value::as_array) else {
        return schema.clone();
    };
    let mut merged = schema.as_object().cloned().unwrap_or_default();
    merged.remove("allOf");
    let mut properties = merged
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut required: Vec<Value> = merged
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for part in parts {
        let part = flatten(part);
        if let Some(props) = part.get("properties").and_then(Value::as_object) {
            for (key, prop) in props {
                properties
                    .entry(key.clone())
                    .or_insert_with(|| prop.clone());
            }
        }
        for name in part
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !required.contains(name) {
                required.push(name.clone());
            }
        }
        if let Some(kind) = part.get("type") {
            merged.entry("type").or_insert_with(|| kind.clone());
        }
    }
    if !properties.is_empty() {
        merged.insert("properties".into(), Value::Object(properties));
        merged.entry("type").or_insert_with(|| json!("object"));
    }
    if !required.is_empty() {
        merged.insert("required".into(), Value::Array(required));
    }
    Value::Object(merged)
}

/// schema 的类型名（`type` 是数组时取第一个不是 null 的）。
pub(super) fn type_name(schema: &Value) -> Option<&str> {
    match schema.get("type") {
        Some(Value::String(kind)) => Some(kind),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|k| *k != "null"),
        _ => None,
    }
}

/// schema → 输入类型。
pub(super) fn kind_of(schema: &Value) -> InputKind {
    match type_name(schema) {
        Some("string") => InputKind::Text,
        Some("integer" | "number") => InputKind::Number,
        Some("boolean") => InputKind::Bool,
        Some("object" | "array") => InputKind::Json,
        _ if schema.get("properties").is_some()
            || schema.get("items").is_some()
            || schema.get("additionalProperties").is_some() =>
        {
            InputKind::Json
        }
        // 「对象或数组」：整段 JSON 的输入写回时就是这样。
        _ if ["anyOf", "oneOf"].iter().any(|key| {
            schema
                .get(*key)
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    !parts.is_empty() && parts.iter().all(|p| kind_of(p) == InputKind::Json)
                })
        }) =>
        {
            InputKind::Json
        }
        _ => match schema
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|e| e.first())
        {
            Some(Value::Number(_)) => InputKind::Number,
            Some(Value::Bool(_)) => InputKind::Bool,
            _ => InputKind::Text,
        },
    }
}

/// 输入类型 → 最简 schema。
pub(super) fn schema_of_kind(kind: InputKind) -> Value {
    match kind {
        InputKind::Text => json!({"type": "string"}),
        InputKind::Number => json!({"type": "number"}),
        InputKind::Bool => json!({"type": "boolean"}),
        InputKind::Json => json!({"anyOf": [{"type": "object"}, {"type": "array"}]}),
    }
}

/// 从例子推断 schema。
pub(super) fn infer(example: &Value) -> Value {
    match example {
        Value::Null => json!({"nullable": true}),
        Value::Bool(_) => json!({"type": "boolean"}),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({"type": "integer"}),
        Value::Number(_) => json!({"type": "number"}),
        Value::String(_) => json!({"type": "string"}),
        Value::Array(items) => match items.first() {
            Some(first) => json!({"type": "array", "items": infer(first)}),
            None => json!({"type": "array", "items": {}}),
        },
        Value::Object(map) => {
            let properties: Map<String, Value> = map
                .iter()
                .map(|(key, value)| (key.clone(), infer(value)))
                .collect();
            json!({"type": "object", "properties": properties})
        }
    }
}

/// schema 只允许一个值时返回它（`enum: [x]`）：这种参数是写死的，不做成输入。
pub(super) fn fixed(schema: &Value) -> Option<&Value> {
    match schema.get("enum").and_then(Value::as_array) {
        Some(values) if values.len() == 1 => values.first(),
        _ => None,
    }
}

/// 样例值 → 界面上的文字。
pub(super) fn example_text(kind: InputKind, value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other if kind == InputKind::Json => other.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

/// 界面上的样例文字 → 写进 OpenAPI 的值；保证 [`example_text`] 转回来一字不差。
pub(super) fn example_value(kind: InputKind, text: &str) -> Value {
    let parsed = match kind {
        InputKind::Text => None,
        InputKind::Number | InputKind::Bool | InputKind::Json => {
            serde_json::from_str::<Value>(text).ok().filter(|value| {
                let fits = match kind {
                    InputKind::Number => value.is_number(),
                    InputKind::Bool => value.is_boolean(),
                    _ => !value.is_string(),
                };
                fits && example_text(kind, value) == text
            })
        }
    };
    parsed.unwrap_or_else(|| Value::String(text.to_string()))
}

/// 参数、属性的样例：`example`、schema 的 `example` / `default`、`examples` 第一个、枚举第一个。
pub(super) fn example_of(holder: &Value, schema: &Value) -> Option<Value> {
    holder
        .get("example")
        .or_else(|| schema.get("example"))
        .or_else(|| {
            holder
                .get("examples")
                .and_then(Value::as_object)
                .and_then(|map| map.values().next())
                .and_then(|example| example.get("value"))
        })
        .or_else(|| {
            schema
                .get("examples")
                .and_then(Value::as_array)
                .and_then(|e| e.first())
        })
        .or_else(|| schema.get("default"))
        .or_else(|| {
            schema
                .get("enum")
                .and_then(Value::as_array)
                .and_then(|e| e.first())
        })
        .cloned()
}

/// 合法的输入变量名（`[A-Za-z_][A-Za-z0-9_]*`）：别的字符换成下划线；一个英文字母都没有的
/// （中文参数名）用 `fallback`。
pub(super) fn identifier(name: &str, fallback: &str) -> String {
    if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
        return fallback.to_string();
    }
    let mut out: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// 路径的一段变成 id 用的词：小写英文字母、数字，其余并成一个下划线；中文段为空。
pub(super) fn slug_segment(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// 地址里写的文字解码：`%E5%9C%B0` → 地。
pub(super) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_round_trip_exactly() {
        for (kind, text) in [
            (InputKind::Text, "123"),
            (InputKind::Number, "2025"),
            (InputKind::Number, "abc"),
            (InputKind::Bool, "true"),
            (InputKind::Bool, "是"),
            (InputKind::Json, r#"{"a":1}"#),
            (InputKind::Json, r#"{ "a": 1 }"#),
            (InputKind::Json, "[1,2]"),
        ] {
            let value = example_value(kind, text);
            assert_eq!(example_text(kind, &value), text, "{kind:?} {text}");
        }
        assert_eq!(example_value(InputKind::Number, "2025"), json!(2025));
    }

    #[test]
    fn decode_and_identifiers() {
        assert_eq!(percent_decode("%E5%9C%B0a%2Cb"), "地a,b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(identifier("X-Request-Id", "p"), "X_Request_Id");
        assert_eq!(identifier("2fa", "p"), "_2fa");
        assert_eq!(identifier("地区", "p1"), "p1");
    }
}
