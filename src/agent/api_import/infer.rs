//! 从一份返回（文档里的示例，或实测拿到的真实返回）推断返回映射与业务成功判据。
//!
//! 纯程序判断：找条目列表、按常见字段名认标题 / 正文 / 编号 / 出处、按常见写法认成功字段。
//! 只填还空着的项，不覆盖文档或人填过的。

use crate::agent::api::{ApiMapping, ApiSuccess};
use serde_json::Value;

/// 推断结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Inferred {
    pub(crate) mapping: ApiMapping,
    pub(crate) success: Option<ApiSuccess>,
}

const TITLE_KEYS: &[&str] = &["title", "name", "subject", "caption", "标题", "名称"];
const TEXT_KEYS: &[&str] = &[
    "content",
    "text",
    "summary",
    "body",
    "abstract",
    "description",
    "desc",
    "正文",
    "内容",
    "摘要",
];
const ID_KEYS: &[&str] = &["id", "uuid", "doc_id", "docid", "编号", "序号"];
const SOURCE_KEYS: &[&str] = &[
    "source",
    "url",
    "link",
    "issuer",
    "issuing_unit",
    "publisher",
    "dept",
    "出处",
    "来源",
    "发文机关",
];
/// 成功字段：(字段名, 认作成功的取值)。
const SUCCESS_KEYS: &[(&str, &[&str])] = &[
    ("code", &["0", "200", "00000"]),
    ("errcode", &["0"]),
    ("err_code", &["0"]),
    ("errno", &["0"]),
    ("ret", &["0"]),
    ("resultCode", &["0", "200"]),
    ("status", &["0", "200", "success", "ok", "SUCCESS", "OK"]),
    ("success", &["true"]),
    ("ok", &["true"]),
];
/// 拼正文模板时最多用几个字段。
const TEMPLATE_FIELDS: usize = 8;

pub(crate) fn infer(root: &Value) -> Inferred {
    let success = success_of(root);
    let (list, sample) = match find_list(root) {
        Some((pointer, sample)) => (pointer, Some(sample)),
        None => match root.get("data") {
            Some(data @ Value::Object(_)) => ("/data".to_string(), Some(data)),
            _ => (String::new(), root.is_object().then_some(root)),
        },
    };
    let mut mapping = ApiMapping {
        list,
        ..ApiMapping::default()
    };
    if let Some(Value::Object(item)) = sample {
        let keys: Vec<&String> = item.keys().collect();
        let pick = |wanted: &[&str], suffixes: &[&str]| -> Option<String> {
            wanted
                .iter()
                .find_map(|w| keys.iter().find(|k| k.eq_ignore_ascii_case(w)))
                .or_else(|| {
                    keys.iter().find(|k| {
                        let lower = k.to_ascii_lowercase();
                        suffixes.iter().any(|s| lower.ends_with(s))
                    })
                })
                .map(|k| k.to_string())
        };
        mapping.title = pick(TITLE_KEYS, &["title", "name"]).unwrap_or_default();
        mapping.id = pick(ID_KEYS, &["_id", "id"]).unwrap_or_default();
        mapping.source = pick(SOURCE_KEYS, &["source", "url"]).unwrap_or_default();
        mapping.text = pick(TEXT_KEYS, &["content", "text", "summary"]).unwrap_or_else(|| {
            // 没有正文字段（统计数据之类）：把其余字段拼成「字段：{字段}」的模板。
            let used = [&mapping.title, &mapping.id, &mapping.source];
            item.iter()
                .filter(|(key, value)| {
                    !used.contains(key)
                        && !matches!(value, Value::Object(_) | Value::Array(_))
                        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                })
                .take(TEMPLATE_FIELDS)
                .map(|(key, _)| format!("{key}：{{{key}}}"))
                .collect::<Vec<_>>()
                .join("；")
        });
    }
    Inferred { mapping, success }
}

fn success_of(root: &Value) -> Option<ApiSuccess> {
    let object = root.as_object()?;
    SUCCESS_KEYS.iter().find_map(|(key, oks)| {
        let value = object.get(*key)?;
        let text = match value {
            Value::Bool(flag) => flag.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.trim().to_string(),
            _ => return None,
        };
        oks.contains(&text.as_str()).then(|| ApiSuccess {
            pointer: format!("/{}", escape(key)),
            equals: text,
        })
    })
}

/// 广度优先找第一个「元素是对象」的数组，返回 (JSON 指针, 第一个元素)。
fn find_list(root: &Value) -> Option<(String, &Value)> {
    let mut queue: std::collections::VecDeque<(String, &Value)> =
        std::collections::VecDeque::from([(String::new(), root)]);
    while let Some((pointer, value)) = queue.pop_front() {
        match value {
            Value::Array(items) => {
                if let Some(first) = items.iter().find(|item| item.is_object()) {
                    return Some((pointer, first));
                }
            }
            Value::Object(map) => {
                for (key, child) in map {
                    queue.push_back((format!("{pointer}/{}", escape(key)), child));
                }
            }
            _ => {}
        }
    }
    None
}

/// JSON 指针转义：`~` → `~0`，`/` → `~1`。
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// 把推断结果补进现有映射，只填空着的项。返回补了哪些项（给人看）。
pub(crate) fn fill_mapping(
    mapping: &mut ApiMapping,
    success: &mut ApiSuccess,
    inferred: &Inferred,
) -> Vec<&'static str> {
    let mut filled = Vec::new();
    let mut take = |own: &mut String, other: &str, label: &'static str| {
        if own.trim().is_empty() && !other.is_empty() {
            *own = other.to_string();
            filled.push(label);
        }
    };
    take(&mut mapping.list, &inferred.mapping.list, "列表位置");
    take(&mut mapping.title, &inferred.mapping.title, "标题");
    take(&mut mapping.text, &inferred.mapping.text, "正文");
    take(&mut mapping.id, &inferred.mapping.id, "编号");
    take(&mut mapping.source, &inferred.mapping.source, "出处");
    if !success.is_set()
        && let Some(found) = &inferred.success
    {
        *success = found.clone();
        filled.push("成功判据");
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_wrapped_list_is_found_with_its_fields_and_success_code() {
        let reply = json!({
            "code": 0,
            "msg": "ok",
            "data": {"total": 2, "records": [
                {"docId": "a1", "policyTitle": "关于支持中小企业的意见", "content": "……", "issuing_unit": "省政府"}
            ]}
        });
        let inferred = infer(&reply);
        assert_eq!(inferred.mapping.list, "/data/records");
        assert_eq!(inferred.mapping.title, "policyTitle");
        assert_eq!(inferred.mapping.text, "content");
        assert_eq!(inferred.mapping.id, "docId");
        assert_eq!(inferred.mapping.source, "issuing_unit");
        assert_eq!(
            inferred.success,
            Some(ApiSuccess {
                pointer: "/code".into(),
                equals: "0".into()
            })
        );
    }

    #[test]
    fn statistics_without_a_text_field_get_a_template() {
        let reply = json!([{"region": "全省", "year": 2025, "count": 12}]);
        let inferred = infer(&reply);
        assert_eq!(inferred.mapping.list, "");
        assert_eq!(
            inferred.mapping.text,
            "region：{region}；year：{year}；count：{count}"
        );
        assert!(inferred.success.is_none());

        let failed = json!({"code": 500, "msg": "错误"});
        assert!(infer(&failed).success.is_none(), "失败样例认不出成功取值");
    }

    #[test]
    fn filling_keeps_what_is_already_there() {
        let mut mapping = ApiMapping {
            title: "{region}统计".into(),
            ..ApiMapping::default()
        };
        let mut success = ApiSuccess::default();
        let inferred =
            infer(&json!({"success": true, "data": {"items": [{"name": "甲", "content": "乙"}]}}));
        let filled = fill_mapping(&mut mapping, &mut success, &inferred);
        assert_eq!(mapping.title, "{region}统计");
        assert_eq!(mapping.list, "/data/items");
        assert_eq!(filled, ["列表位置", "正文", "成功判据"]);
        assert_eq!(success.pointer, "/success");
    }
}
