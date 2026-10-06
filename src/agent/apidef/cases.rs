//! 接口用例：参数 + 期望，证明接口可用（`docs/api-workbench.md` 第五节）。
//!
//! 每条用例先过默认期望（发得出去、HTTP 2xx、业务成功判据成立、不是网页——就是
//! [`api::trial`] 判调通的那一套），再逐条检查写明的期望。期望只有五种，够证明「接口按条件
//! 返回了对的东西」，不做脚本、不做正则。
//!
//! 「存为用例」时程序按这次的真实返回建议几条期望（[`suggest`]），人勾了才存；模型不写期望。

use crate::agent::api::{self, ApiEndpoint, ApiExample, ApiSecrets, Trial};
use crate::agent::board::value_to_text;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 一条期望。指针为空表示整个返回。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Expect {
    /// 有值，且不是空数组、空对象、空字符串。
    NotEmpty {
        #[serde(default)]
        pointer: String,
    },
    /// 是数组，长度不少于 `value`。
    MinItems {
        #[serde(default)]
        pointer: String,
        value: u64,
    },
    /// 取值相等：数字按数值、文字去首尾空白比，别的按 JSON 比。
    Equals {
        #[serde(default)]
        pointer: String,
        value: Value,
    },
    /// 取值的文字里含这段。
    Contains {
        #[serde(default)]
        pointer: String,
        value: String,
    },
    /// 有这个字段（值可以是 0、空）。
    Exists { pointer: String },
}

/// 期望的种类，界面上「加期望」选用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExpectKind {
    NotEmpty,
    MinItems,
    Equals,
    Contains,
    Exists,
}

impl ExpectKind {
    pub(crate) const ALL: [ExpectKind; 5] = [
        Self::NotEmpty,
        Self::MinItems,
        Self::Equals,
        Self::Contains,
        Self::Exists,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NotEmpty => "不为空",
            Self::MinItems => "至少几条",
            Self::Equals => "等于",
            Self::Contains => "包含",
            Self::Exists => "有字段",
        }
    }

    /// 要不要填值。
    pub(crate) fn needs_value(self) -> bool {
        matches!(self, Self::MinItems | Self::Equals | Self::Contains)
    }

    /// 按界面上填的指针与值拼出期望；值不合格返回原因。
    pub(crate) fn build(self, pointer: &str, value: &str) -> Result<Expect, String> {
        let pointer = pointer.trim().to_string();
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return Err(
                "位置要写成 JSON 指针，以 / 开头，例如 /data/items；留空表示整个返回".into(),
            );
        }
        let value = value.trim();
        Ok(match self {
            Self::NotEmpty => Expect::NotEmpty { pointer },
            Self::Exists if pointer.is_empty() => {
                return Err("「有字段」要写字段位置，例如 /data/total".into());
            }
            Self::Exists => Expect::Exists { pointer },
            Self::MinItems => Expect::MinItems {
                pointer,
                value: value.parse().map_err(|_| "条数要是整数".to_string())?,
            },
            Self::Equals => Expect::Equals {
                pointer,
                // 能读成 JSON（数字、真假、对象）就按 JSON，否则当文字。
                value: serde_json::from_str::<Value>(value)
                    .ok()
                    .filter(|v| !v.is_string())
                    .unwrap_or_else(|| Value::String(value.to_string())),
            },
            Self::Contains if value.is_empty() => return Err("「包含」要写包含的文字".into()),
            Self::Contains => Expect::Contains {
                pointer,
                value: value.to_string(),
            },
        })
    }
}

fn place(pointer: &str) -> String {
    if pointer.is_empty() {
        "整个返回".into()
    } else {
        pointer.to_string()
    }
}

fn short(text: &str) -> String {
    crate::agent::tools::short(text, 60)
}

impl Expect {
    /// 给人看的一句：「/data/items 至少 1 条」。
    pub(crate) fn label(&self) -> String {
        match self {
            Self::NotEmpty { pointer } => format!("{} 不为空", place(pointer)),
            Self::MinItems { pointer, value } => format!("{} 至少 {value} 条", place(pointer)),
            Self::Equals { pointer, value } => {
                format!("{} 等于 {}", place(pointer), short(&value_to_text(value)))
            }
            Self::Contains { pointer, value } => {
                format!("{} 包含「{}」", place(pointer), short(value))
            }
            Self::Exists { pointer } => format!("有字段 {pointer}"),
        }
    }

    /// 检查一份返回；不满足时说清期望与实际。
    pub(crate) fn check(&self, root: &Value) -> Result<(), String> {
        let pointer = match self {
            Self::NotEmpty { pointer }
            | Self::MinItems { pointer, .. }
            | Self::Equals { pointer, .. }
            | Self::Contains { pointer, .. }
            | Self::Exists { pointer } => pointer.as_str(),
        };
        let fail = |actual: String| Err(format!("期望{}，实际{actual}", self.label()));
        let Some(found) = root.pointer(pointer) else {
            return fail(format!("返回里没有 {}", place(pointer)));
        };
        match self {
            Self::NotEmpty { .. } => {
                let empty = match found {
                    Value::Null => true,
                    Value::String(text) => text.trim().is_empty(),
                    Value::Array(items) => items.is_empty(),
                    Value::Object(map) => map.is_empty(),
                    _ => false,
                };
                if empty { fail("为空".into()) } else { Ok(()) }
            }
            Self::MinItems { value, .. } => match found {
                Value::Array(items) if items.len() as u64 >= *value => Ok(()),
                Value::Array(items) => fail(format!(" {} 条", items.len())),
                _ => fail("不是列表".into()),
            },
            Self::Equals { value, .. } => {
                if same(found, value) {
                    Ok(())
                } else {
                    fail(format!("是 {}", short(&value_to_text(found))))
                }
            }
            Self::Contains { value, .. } => {
                let text = value_to_text(found);
                if text.contains(value.as_str()) {
                    Ok(())
                } else {
                    fail(format!("是「{}」", short(&text)))
                }
            }
            Self::Exists { .. } => Ok(()),
        }
    }
}

/// 期望值与实际值比：数字按数值、文字去首尾空白，数字与数字样的文字也算相等（`0` 与 `"0"`）。
fn same(actual: &Value, expected: &Value) -> bool {
    let number = |value: &Value| match value {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    match (actual, expected) {
        (Value::String(a), Value::String(b)) => a.trim() == b.trim(),
        _ if actual.is_number() || expected.is_number() => {
            matches!((number(actual), number(expected)), (Some(a), Some(b)) if a == b)
        }
        _ => actual == expected,
    }
}

/// 判一条用例：先过默认期望（`trial` 调通），再逐条检查写明的期望。
pub(crate) fn judge(trial: &Trial, expects: &[Expect]) -> Result<(), String> {
    if let Some(error) = &trial.error {
        return Err(error.clone());
    }
    if expects.is_empty() {
        return Ok(());
    }
    let body = trial
        .raw
        .as_ref()
        .map(|raw| raw.body.as_str())
        .unwrap_or("");
    let root: Value =
        serde_json::from_str(body).map_err(|_| "返回不是 JSON，没法检查期望".to_string())?;
    expects.iter().try_for_each(|expect| expect.check(&root))
}

/// 跑一条用例：用这组参数实测，再判期望。
pub(crate) fn run(
    endpoint: &ApiEndpoint,
    example: &ApiExample,
    secrets: &ApiSecrets,
) -> (Trial, Result<(), String>) {
    let trial = api::trial(endpoint, &endpoint.args_of(example), secrets);
    let verdict = judge(&trial, &example.expect);
    (trial, verdict)
}

/// 最多建议几条。
const MAX_SUGGESTIONS: usize = 6;
/// 计数一类的字段名。
const COUNT_KEYS: &[&str] = &[
    "total",
    "count",
    "totalCount",
    "total_count",
    "totalElements",
];

/// 按一次调通的真实返回建议期望（人勾了才存）：
/// - 列表有数据的：「至少 1 条」；
/// - 配了业务成功判据的：判据字段「等于」这次的值；
/// - 参数值出现在返回里的（查「全省」返回里有「全省」）：那个字段「包含」它；
/// - 有 total、count 这类计数字段的：「有字段」。
pub(crate) fn suggest(
    endpoint: &ApiEndpoint,
    args: &Map<String, Value>,
    body: &str,
) -> Vec<Expect> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let list = endpoint.mapping.list.trim();
    let list_pointer = if !list.is_empty() {
        Some(list.to_string())
    } else if root.is_array() {
        Some(String::new())
    } else {
        first_array(&root, "", 3)
    };
    if let Some(pointer) = &list_pointer {
        match root.pointer(pointer) {
            Some(Value::Array(items)) if !items.is_empty() => found.push(Expect::MinItems {
                pointer: pointer.clone(),
                value: 1,
            }),
            Some(Value::Object(map)) if !map.is_empty() => found.push(Expect::NotEmpty {
                pointer: pointer.clone(),
            }),
            _ => {}
        }
    }
    let success = endpoint.success.pointer.trim();
    if endpoint.success.is_set()
        && let Some(value) = root.pointer(success)
    {
        found.push(Expect::Equals {
            pointer: success.to_string(),
            value: value.clone(),
        });
    }
    // 参数值在返回里出现的位置：先在列表第一条里找，再在整个返回里找。
    let scope = list_pointer
        .as_deref()
        .map(|p| format!("{p}/0"))
        .filter(|p| root.pointer(p).is_some())
        .unwrap_or_default();
    let mut contains = 0;
    for value in args.values() {
        let text = value_to_text(value);
        let text = text.trim();
        if text.chars().count() < 2 || contains >= 2 {
            continue;
        }
        let hit = [scope.as_str(), ""]
            .iter()
            .find_map(|base| root.pointer(base).and_then(|v| find_text(v, base, text, 4)));
        if let Some(pointer) = hit {
            found.push(Expect::Contains {
                pointer,
                value: text.to_string(),
            });
            contains += 1;
        }
    }
    'count: for base in ["", "/data"] {
        for key in COUNT_KEYS {
            let pointer = format!("{base}/{key}");
            if root.pointer(&pointer).is_some_and(|v| v.is_number()) {
                found.push(Expect::Exists { pointer });
                break 'count;
            }
        }
    }
    let mut unique: Vec<Expect> = Vec::new();
    for expect in found {
        if !unique.contains(&expect) {
            unique.push(expect);
        }
    }
    unique.truncate(MAX_SUGGESTIONS);
    unique
}

/// 第一个不为空的数组的指针（广度优先，限深度）。
fn first_array(value: &Value, at: &str, depth: usize) -> Option<String> {
    let Value::Object(map) = value else {
        return None;
    };
    for (key, item) in map {
        if item.as_array().is_some_and(|items| !items.is_empty()) {
            return Some(format!("{at}/{}", token(key)));
        }
    }
    if depth == 0 {
        return None;
    }
    map.iter()
        .find_map(|(key, item)| first_array(item, &format!("{at}/{}", token(key)), depth - 1))
}

/// 文字里含 `text` 的第一个字段的指针。
fn find_text(value: &Value, at: &str, text: &str, depth: usize) -> Option<String> {
    match value {
        Value::String(s) if s.contains(text) => Some(at.to_string()),
        Value::Number(n) if n.to_string() == text => Some(at.to_string()),
        Value::Object(map) if depth > 0 => map.iter().find_map(|(key, item)| {
            find_text(item, &format!("{at}/{}", token(key)), text, depth - 1)
        }),
        Value::Array(items) if depth > 0 => items
            .iter()
            .take(3)
            .enumerate()
            .find_map(|(i, item)| find_text(item, &format!("{at}/{i}"), text, depth - 1)),
        _ => None,
    }
}

fn token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::api::{ApiMapping, ApiSuccess, RawResponse};
    use serde_json::json;

    fn trial(body: Value) -> Trial {
        Trial {
            raw: Some(RawResponse {
                status: 200,
                body: body.to_string(),
            }),
            ..Trial::default()
        }
    }

    #[test]
    fn expectations_pass_and_fail_with_reasons() {
        let body =
            json!({"code": 0, "data": {"total": 0, "items": [{"region": "全省", "count": 3}]}});
        let pass = [
            Expect::NotEmpty {
                pointer: "/data/items".into(),
            },
            Expect::MinItems {
                pointer: "/data/items".into(),
                value: 1,
            },
            Expect::Equals {
                pointer: "/code".into(),
                value: json!("0"),
            },
            Expect::Equals {
                pointer: "/data/items/0/count".into(),
                value: json!(3.0),
            },
            Expect::Contains {
                pointer: "/data/items/0/region".into(),
                value: "全".into(),
            },
            Expect::Exists {
                pointer: "/data/total".into(),
            },
        ];
        assert_eq!(judge(&trial(body.clone()), &pass), Ok(()));
        let fail = Expect::MinItems {
            pointer: "/data/items".into(),
            value: 2,
        };
        assert_eq!(
            judge(&trial(body.clone()), &[fail]).unwrap_err(),
            "期望/data/items 至少 2 条，实际 1 条"
        );
        let missing = Expect::Exists {
            pointer: "/data/sum".into(),
        };
        assert!(
            judge(&trial(body), &[missing])
                .unwrap_err()
                .contains("返回里没有 /data/sum")
        );
        let broken = Trial {
            error: Some("接口返回 500".into()),
            ..Trial::default()
        };
        assert_eq!(
            judge(&broken, &[]).unwrap_err(),
            "接口返回 500",
            "默认期望先过"
        );
    }

    #[test]
    fn built_from_the_form_and_stored_as_tagged_json() {
        let expect = ExpectKind::Equals.build("/code", "0").unwrap();
        assert_eq!(
            expect,
            Expect::Equals {
                pointer: "/code".into(),
                value: json!(0)
            }
        );
        assert_eq!(
            ExpectKind::Equals.build("/msg", "成功").unwrap(),
            Expect::Equals {
                pointer: "/msg".into(),
                value: json!("成功")
            }
        );
        assert!(
            ExpectKind::MinItems.build("data", "1").is_err(),
            "指针要以 / 开头"
        );
        assert!(ExpectKind::MinItems.build("/data", "几条").is_err());
        assert_eq!(
            serde_json::to_value(Expect::MinItems {
                pointer: "/a".into(),
                value: 1
            })
            .unwrap(),
            json!({"kind": "min_items", "pointer": "/a", "value": 1})
        );
    }

    #[test]
    fn suggestions_follow_the_real_reply() {
        let endpoint = ApiEndpoint {
            mapping: ApiMapping {
                list: "/data/items".into(),
                ..ApiMapping::default()
            },
            success: ApiSuccess {
                pointer: "/code".into(),
                equals: "0".into(),
            },
            ..ApiEndpoint::default()
        };
        let body =
            json!({"code": 0, "data": {"total": 2, "items": [{"region": "全省", "year": 2025}]}});
        let args = json!({"region": "全省", "year": "2025"})
            .as_object()
            .unwrap()
            .clone();
        let found = suggest(&endpoint, &args, &body.to_string());
        assert_eq!(
            found,
            [
                Expect::MinItems {
                    pointer: "/data/items".into(),
                    value: 1
                },
                Expect::Equals {
                    pointer: "/code".into(),
                    value: json!(0)
                },
                Expect::Contains {
                    pointer: "/data/items/0/region".into(),
                    value: "全省".into()
                },
                Expect::Contains {
                    pointer: "/data/items/0/year".into(),
                    value: "2025".into()
                },
                Expect::Exists {
                    pointer: "/data/total".into()
                },
            ]
        );
        let bare = suggest(
            &ApiEndpoint::default(),
            &Map::new(),
            r#"{"list": [{"a": 1}]}"#,
        );
        assert_eq!(
            bare,
            [Expect::MinItems {
                pointer: "/list".into(),
                value: 1
            }]
        );
        assert!(suggest(&endpoint, &args, "<html>").is_empty());
    }
}
