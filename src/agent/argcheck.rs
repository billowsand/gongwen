//! 工具参数的签名与报错文字：内置工具与数据接口共用（`docs/agent-kernel-hardening.md` 3.1、3.2）。
//!
//! 两边的「合不合格」各查各的——数据接口按 OpenAPI 的 schema 严格查（参数要原样发给服务端），
//! 内置工具按工具自己读参数的宽松规则查（`"1、2，3"` 也算数字列表）——但模型看到的签名与报错
//! 长得一样：错在哪、参数签名、一个正确示例、请改好再调。

use serde_json::Value;

/// 一个参数的简短签名：`region: 文字（必填）`、`level: "省"|"市"`。文本协议与报错里用。
pub(crate) fn signature(name: &str, schema: &Value, required: bool) -> String {
    let kind = match schema.get("enum").and_then(Value::as_array) {
        Some(values) if !values.is_empty() => values
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("|"),
        _ => match schema.get("type").and_then(Value::as_str) {
            Some("string") => "文字".into(),
            Some("integer") => "整数".into(),
            Some("number") => "数字".into(),
            Some("boolean") => "true|false".into(),
            Some("array") => {
                let item = schema
                    .pointer("/items/type")
                    .and_then(Value::as_str)
                    .unwrap_or("值");
                format!("{item} 数组")
            }
            Some("object") => "JSON 对象".into(),
            _ => "JSON".into(),
        },
    };
    format!("{name}: {kind}{}", if required { "（必填）" } else { "" })
}

/// 一整个参数 schema（`{"type": "object", "properties": …, "required": […]}`）的签名，一行。
pub(crate) fn signatures(schema: &Value) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let parts: Vec<String> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, prop)| signature(name, prop, required.contains(&name.as_str())))
        .collect();
    if parts.is_empty() {
        "无参数".into()
    } else {
        parts.join(", ")
    }
}

/// 参数不合格时回给模型的话：错在哪、参数签名、正确示例（有的话）、请改好再调。
pub(crate) fn reject(
    problems: &[String],
    what: &str,
    schema: &Value,
    sample: Option<&str>,
) -> String {
    let mut message = format!(
        "参数不对：{}。这个{what}的参数是：{}",
        problems.join("；"),
        signatures(schema)
    );
    if let Some(sample) = sample.filter(|s| !s.trim().is_empty()) {
        message.push_str(&format!("。正确的例子：{sample}"));
    }
    message.push_str("。请改好参数再调一次。");
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn signatures_and_rejection_read_the_same_for_every_tool() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": {"type": "string", "enum": ["全省", "甲市"]},
                "year": {"type": "integer"},
                "tags": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["region"],
        });
        let line = signatures(&schema);
        assert!(line.contains(r#"region: "全省"|"甲市"（必填）"#), "{line}");
        assert!(line.contains("year: 整数") && line.contains("tags: string 数组"));
        assert_eq!(signatures(&json!({"type": "object"})), "无参数");
        let message = reject(
            &["缺少必填参数 region".into()],
            "接口",
            &schema,
            Some(r#"{"region":"全省"}"#),
        );
        assert!(message.starts_with("参数不对：缺少必填参数 region。这个接口的参数是："));
        assert!(message.ends_with(r#"。正确的例子：{"region":"全省"}。请改好参数再调一次。"#));
        assert!(!reject(&[], "工具", &schema, Some(" ")).contains("正确的例子"));
    }
}
