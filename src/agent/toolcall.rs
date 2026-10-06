//! 自主步骤的对话与工具调用格式（`docs/ai-agent-workbench.md` 16.14、决定 F17）。
//!
//! 两种协议：
//! - **原生**：OpenAI function calling，请求带 `tools`，回复里是 `tool_calls`；
//! - **文本**：工具说明写进系统提示，模型在正文里输出 `<tool_call>{"name": …, "arguments": {…}}</tool_call>`，
//!   工具结果以 `<tool_result>` 作为用户消息回给模型。服务端不认 `tools`、或模型把调用写在正文里时用它。
//!
//! 这里只放纯逻辑：对话轮次、工具名转换、两种协议下的消息拼法与文本调用的解析。

use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

/// 用哪种协议调工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Protocol {
    #[default]
    Native,
    Text,
}

/// 模型要调的一个工具。`name` 是发给模型的名字（`kb_search`），不是工具 id。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: Value,
}

/// 发给模型的一个工具。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolSpec {
    pub(crate) name: String,
    pub(crate) description: String,
    /// (参数名, 是否必填, 说明)。文本协议用它列参数；原生协议没有 `schema` 时也用它。
    pub(crate) params: Vec<(String, bool, String)>,
    /// 参数的完整 JSON Schema（类型、枚举、嵌套、必填）。有就原样发给原生协议；数据接口工具
    /// 现在有（`apidef::tooling`），内置工具以后逐个补（`docs/agent-kernel-hardening.md` 第 1 期）。
    pub(crate) schema: Option<Value>,
}

impl ToolSpec {
    /// OpenAI `tools` 里的一项。有 `schema` 用它；没有的只列参数说明、不标类型（内置工具的输入
    /// 本来就是宽松的 JSON，由工具自己解析）。
    pub(crate) fn to_native(&self) -> Value {
        if let Some(schema) = &self.schema {
            return json!({
                "type": "function",
                "function": {
                    "name": self.name,
                    "description": self.description,
                    "parameters": schema,
                }
            });
        }
        let properties: serde_json::Map<String, Value> = self
            .params
            .iter()
            .map(|(name, _, doc)| (name.clone(), json!({ "description": doc })))
            .collect();
        let required: Vec<&str> = self
            .params
            .iter()
            .filter(|(_, required, _)| *required)
            .map(|(name, ..)| name.as_str())
            .collect();
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                }
            }
        })
    }

    fn to_text(&self) -> String {
        let params = if self.params.is_empty() {
            "无参数".to_string()
        } else {
            self.params
                .iter()
                .map(|(name, required, doc)| {
                    format!(
                        "{name}（{}）：{doc}",
                        if *required { "必填" } else { "可选" }
                    )
                })
                .collect::<Vec<_>>()
                .join("；")
        };
        format!("- {}：{}。参数：{params}", self.name, self.description)
    }
}

/// 对话里的一轮。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Turn {
    System(String),
    User(String),
    Assistant {
        content: String,
        calls: Vec<ToolCall>,
    },
    Tool {
        id: String,
        name: String,
        content: String,
    },
}

/// 模型的一次回复。
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Reply {
    /// 正文（已剔除文本协议里的调用块）。
    pub(crate) content: String,
    pub(crate) calls: Vec<ToolCall>,
    pub(crate) truncated: bool,
    /// 这次实际用的协议：原生被拒或模型写在正文里时是文本，之后都按它来。
    pub(crate) protocol: Protocol,
}

/// 工具名最长多少（OpenAI 的限制）。
const MAX_WIRE_NAME: usize = 64;

/// 工具 id → 发给模型的名字：`kb.search` → `kb_search`，`http.call:stat` → `http_call__stat`。
/// 只用 ASCII 字母、数字、下划线、短横线；限定名里有别的字符（中文接口 id）或整体超长时，
/// 截短并加上原 id 的哈希，保证不同的 id 不会撞成同一个名字。
pub(crate) fn wire_name(id: &str) -> String {
    let (base, qualifier) = id.split_once(':').map_or((id, None), |(b, q)| (b, Some(q)));
    let mut name = base.replace('.', "_");
    let mut lossy = false;
    if let Some(qualifier) = qualifier {
        name.push_str("__");
        for c in qualifier.chars() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                name.push(c);
            } else {
                // 点换成下划线不丢信息以外的字符（中文、空格……）都算有损。
                lossy |= c != '.';
                name.push('_');
            }
        }
    }
    if lossy || name.len() > MAX_WIRE_NAME {
        use sha2::{Digest, Sha256};
        let hash: String = Sha256::digest(id.as_bytes())[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        name.truncate(MAX_WIRE_NAME - hash.len() - 1);
        name.push('_');
        name.push_str(&hash);
    }
    name
}

/// 原生协议的消息列表。
pub(crate) fn native_messages(turns: &[Turn]) -> Vec<Value> {
    turns
        .iter()
        .map(|turn| match turn {
            Turn::System(text) => json!({"role": "system", "content": text}),
            Turn::User(text) => json!({"role": "user", "content": text}),
            Turn::Assistant { content, calls } if calls.is_empty() => {
                json!({"role": "assistant", "content": content})
            }
            Turn::Assistant { content, calls } => json!({
                "role": "assistant",
                "content": content,
                "tool_calls": calls.iter().map(|call| json!({
                    "id": call.id,
                    "type": "function",
                    "function": {"name": call.name, "arguments": call.arguments.to_string()},
                })).collect::<Vec<_>>(),
            }),
            Turn::Tool { id, content, .. } => {
                json!({"role": "tool", "tool_call_id": id, "content": content})
            }
        })
        .collect()
}

/// 文本协议的系统提示附加段。
pub(crate) fn text_protocol_prompt(tools: &[ToolSpec]) -> String {
    let list = tools
        .iter()
        .map(ToolSpec::to_text)
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "【可用工具】\n{list}\n\n\
         【调用方式】要调用工具时，输出下面这样的一段（可以连续输出多段），参数是 JSON 对象：\n\
         <tool_call>{{\"name\": \"工具名\", \"arguments\": {{\"参数名\": \"值\"}}}}</tool_call>\n\
         输出调用后就停下，等工具结果。全部做完后调用 finish 工具。"
    )
}

/// 文本协议的消息列表：工具说明并进系统提示，工具调用与结果都变成正文。
pub(crate) fn text_messages(turns: &[Turn], tools: &[ToolSpec]) -> Vec<Value> {
    let spec = text_protocol_prompt(tools);
    let mut out = Vec::new();
    let mut has_system = false;
    for turn in turns {
        match turn {
            Turn::System(text) => {
                has_system = true;
                out.push(json!({"role": "system", "content": format!("{text}\n\n{spec}")}));
            }
            Turn::User(text) => out.push(json!({"role": "user", "content": text})),
            Turn::Assistant { content, calls } => {
                let mut text = content.clone();
                for call in calls {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!(
                        "<tool_call>{}</tool_call>",
                        json!({"name": call.name, "arguments": call.arguments})
                    ));
                }
                out.push(json!({"role": "assistant", "content": text}));
            }
            Turn::Tool { name, content, .. } => out.push(json!({
                "role": "user",
                "content": format!("<tool_result name=\"{name}\">\n{content}\n</tool_result>"),
            })),
        }
    }
    if !has_system {
        out.insert(0, json!({"role": "system", "content": spec}));
    }
    out
}

/// 把对话压成一问一答（给只会 `complete` 的模型，测试用的脚本模型就是这样）。
pub(crate) fn flatten(turns: &[Turn], tools: &[ToolSpec]) -> (String, String) {
    let messages = text_messages(turns, tools);
    let system = messages
        .first()
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string();
    let user = messages
        .iter()
        .skip(1)
        .map(|m| {
            let role = match m["role"].as_str() {
                Some("assistant") => "【你】",
                _ => "【用户】",
            };
            format!("{role}\n{}", m["content"].as_str().unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    (system, user)
}

static TOOL_CALL_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<tool_call>\s*(.*?)\s*(?:</tool_call>|$)").expect("调用块正则")
});
static FENCED_JSON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```(?:json)?\s*(\{.*?\})\s*```").expect("代码块正则"));

/// 从正文里认出文本协议的调用，返回 (调用, 剔除调用后的正文)。
///
/// 认 `<tool_call>…</tool_call>`（Qwen / Hermes 一类模型没开工具解析时也这么写）；没有的话再认
/// 含 `name` 与 `arguments` 的 ```json 代码块。参数可以是对象，也可以是 JSON 字符串。
pub(crate) fn parse_text_calls(content: &str) -> (Vec<ToolCall>, String) {
    let mut calls = Vec::new();
    let mut rest = content.to_string();
    let parse = |raw: &str| {
        let value: Value = serde_json::from_str(raw.trim()).ok()?;
        let name = value
            .get("name")
            .or_else(|| value.get("tool"))
            .and_then(Value::as_str)?
            .trim()
            .to_string();
        let arguments = match value
            .get("arguments")
            .or_else(|| value.get("parameters"))
            .or_else(|| value.get("args"))
        {
            Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(json!({})),
            Some(Value::Null) | None => json!({}),
            Some(other) => other.clone(),
        };
        Some(ToolCall {
            id: String::new(),
            name,
            arguments,
        })
    };
    if TOOL_CALL_BLOCK.is_match(content) {
        for capture in TOOL_CALL_BLOCK.captures_iter(content) {
            if let Some(call) = parse(&capture[1]) {
                calls.push(call);
            }
        }
        rest = TOOL_CALL_BLOCK.replace_all(content, "").into_owned();
    } else {
        for capture in FENCED_JSON.captures_iter(content) {
            if let Some(call) = parse(&capture[1]) {
                calls.push(call);
            }
        }
        if !calls.is_empty() {
            rest = FENCED_JSON.replace_all(content, "").into_owned();
        }
    }
    for (index, call) in calls.iter_mut().enumerate() {
        call.id = format!("call_{}", index + 1);
    }
    (calls, rest.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ToolSpec {
        ToolSpec {
            name: "kb_search".into(),
            description: "检索知识库".into(),
            params: vec![
                ("query".into(), true, "检索词".into()),
                ("top".into(), false, "最多几段".into()),
            ],
            schema: None,
        }
    }

    #[test]
    fn wire_names_have_no_dots_or_colons() {
        assert_eq!(wire_name("kb.search"), "kb_search");
        assert_eq!(wire_name("http.call:stat"), "http_call__stat");
        let chinese = wire_name("http.call:省统计");
        assert!(
            chinese.starts_with("http_call_____") && chinese.len() > 14,
            "{chinese}"
        );
        assert_ne!(chinese, wire_name("http.call:市统计"), "中文 id 不再撞名");
        assert_eq!(wire_name("http.call:svc.op"), "http_call__svc_op");
        assert!(wire_name(&format!("http.call:{}", "a".repeat(80))).len() <= 64);
        assert_eq!(wire_name("finish"), "finish");
    }

    #[test]
    fn native_specs_and_messages_follow_the_openai_shape() {
        let native = spec().to_native();
        assert_eq!(native["function"]["name"], "kb_search");
        assert_eq!(
            native["function"]["parameters"]["required"],
            json!(["query"])
        );
        let call = ToolCall {
            id: "c1".into(),
            name: "kb_search".into(),
            arguments: json!({"query": "防火"}),
        };
        let messages = native_messages(&[
            Turn::System("S".into()),
            Turn::User("U".into()),
            Turn::Assistant {
                content: String::new(),
                calls: vec![call],
            },
            Turn::Tool {
                id: "c1".into(),
                name: "kb_search".into(),
                content: "结果".into(),
            },
        ]);
        assert_eq!(
            messages[2]["tool_calls"][0]["function"]["arguments"],
            r#"{"query":"防火"}"#
        );
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "c1");
    }

    #[test]
    fn text_messages_carry_the_spec_and_tool_results_as_user_text() {
        let messages = text_messages(
            &[
                Turn::System("你是助手".into()),
                Turn::User("查一下".into()),
                Turn::Assistant {
                    content: "好".into(),
                    calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "kb_search".into(),
                        arguments: json!({"query": "防火"}),
                    }],
                },
                Turn::Tool {
                    id: "call_1".into(),
                    name: "kb_search".into(),
                    content: "三段".into(),
                },
            ],
            &[spec()],
        );
        let system = messages[0]["content"].as_str().unwrap();
        assert!(system.starts_with("你是助手"));
        assert!(system.contains("- kb_search：检索知识库。参数：query（必填）：检索词"));
        let assistant = messages[2]["content"].as_str().unwrap();
        assert!(assistant.starts_with("好\n<tool_call>{"), "{assistant}");
        let (calls, _) = parse_text_calls(assistant);
        assert_eq!(calls[0].name, "kb_search", "写回去的调用能原样认回来");
        assert_eq!(calls[0].arguments["query"], "防火");
        assert_eq!(messages[3]["role"], "user");
        assert!(
            messages[3]["content"]
                .as_str()
                .unwrap()
                .contains("<tool_result name=\"kb_search\">")
        );
    }

    #[test]
    fn text_calls_are_parsed_in_several_shapes() {
        let (calls, rest) = parse_text_calls(
            "先查一下。\n<tool_call>{\"name\": \"kb_search\", \"arguments\": {\"query\": \"防火\"}}</tool_call>\n\
             <tool_call>\n{\"name\": \"finish\", \"arguments\": \"{\\\"summary\\\": \\\"好\\\"}\"}\n</tool_call>",
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "kb_search");
        assert_eq!(calls[0].arguments["query"], "防火");
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(
            calls[1].arguments["summary"], "好",
            "参数写成 JSON 字符串也认"
        );
        assert_eq!(rest, "先查一下。");

        let (calls, _) =
            parse_text_calls("```json\n{\"name\": \"ws_read\", \"arguments\": {}}\n```");
        assert_eq!(calls[0].name, "ws_read");
        // 没收尾的调用块（撞上输出上限）也尽量认。
        let (calls, _) = parse_text_calls("<tool_call>{\"name\": \"finish\", \"arguments\": {}}");
        assert_eq!(calls.len(), 1);
        let (calls, rest) = parse_text_calls("全部做完了，没有要改的。");
        assert!(calls.is_empty());
        assert_eq!(rest, "全部做完了，没有要改的。");
        let (calls, _) = parse_text_calls("<tool_call>不是 JSON</tool_call>");
        assert!(calls.is_empty());
    }

    #[test]
    fn flattening_keeps_the_order_of_the_conversation() {
        let (system, user) = flatten(
            &[
                Turn::System("S".into()),
                Turn::User("任务".into()),
                Turn::Assistant {
                    content: String::new(),
                    calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "kb_search".into(),
                        arguments: json!({}),
                    }],
                },
                Turn::Tool {
                    id: "call_1".into(),
                    name: "kb_search".into(),
                    content: "结果".into(),
                },
            ],
            &[spec()],
        );
        assert!(system.contains("【可用工具】"));
        let task = user.find("任务").unwrap();
        let call = user.find("<tool_call>").unwrap();
        let result = user.find("<tool_result").unwrap();
        assert!(task < call && call < result, "{user}");
    }
}
