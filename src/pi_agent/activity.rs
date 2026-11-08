//! 侧栏只接收简短的动作与结果，工具正文和完整参数仍留在原来的交付链上。

use serde_json::Value;

#[derive(Debug)]
pub(crate) struct PiActivity {
    pub(crate) id: String,
    pub(crate) action: String,
    /// None 表示开始，空字符串表示结束但无需记入过程。
    pub(crate) outcome: Option<String>,
}

pub(super) fn label(text: &str, secret: &str) -> String {
    let safe = if secret.len() >= 4 {
        text.replace(secret, "［已隐藏］")
    } else {
        text.to_owned()
    };
    let mut chars = safe.trim().chars();
    let mut line: String = chars
        .by_ref()
        .take(60)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if chars.next().is_some() {
        line.push('…');
    }
    line
}

pub(super) fn tool_action(name: &str, args: &Value, secret: &str) -> String {
    let (verb, object) = match name {
        "kb_search" => ("查知识库", args["query"].as_str()),
        "ms_search" => ("查稿件库", args["query"].as_str()),
        "api_search" => ("找数据接口", args["query"].as_str()),
        "vocab_search" => ("查标准词库", args["query"].as_str()),
        "kb_read" => ("读知识库资料", None),
        "ms_read" => ("读库内稿件", None),
        "api_call" => ("查接口数据", args["api"].as_str()),
        "api_read" => ("续读接口结果", None),
        "check_document" => ("检查格式与引用", None),
        "doc_elements" => ("读取公文表单", None),
        "current_date" => ("读取当前日期", None),
        "style_list" => ("查可用风格", None),
        "style_read" => ("读风格档案", None),
        _ => ("调用写作工具", None),
    };
    match object
        .map(|value| label(value, secret))
        .filter(|s| !s.is_empty())
    {
        Some(object) => format!("{verb} · {object}"),
        None => verb.into(),
    }
}

pub(super) fn tool_outcome(name: &str, result: &Result<Value, String>, secret: &str) -> String {
    let Ok(value) = result else {
        return "调用失败".into();
    };
    let count = |key: &str| value[key].as_array().map_or(0, Vec::len);
    let title = |row: &Value| {
        row["title"]
            .as_str()
            .or_else(|| row["name"].as_str())
            .map(|text| label(text, secret))
            .filter(|text| !text.is_empty())
    };
    let with_title = |summary: String, title: Option<String>| match title {
        Some(title) => format!("{summary} · 《{title}》"),
        None => summary,
    };
    match name {
        "kb_search" | "ms_search" | "api_search" | "vocab_search" | "style_list" => {
            let count = count("results");
            let (source, unit) = match name {
                "kb_search" => ("相关资料", "段"),
                "ms_search" => ("相关稿件", "篇"),
                "api_search" => ("只读接口", "个"),
                "vocab_search" => ("匹配词条", "条"),
                _ => ("可用风格", "份"),
            };
            if count == 0 {
                return format!("未找到{source}");
            }
            let first = value["results"].get(0).and_then(title);
            with_title(format!("找到 {count} {unit}{source}"), first)
        }
        "kb_read" | "ms_read" | "api_read" if value["title"].is_string() => {
            let offset = value["offset"].as_u64().unwrap_or(0);
            let total = value["total_chars"].as_u64().unwrap_or(0);
            let end = value["next_offset"].as_u64().unwrap_or(total);
            let summary = if offset == 0 && end == total {
                "已读取全文".into()
            } else {
                format!("已读第 {}–{end} 字，共 {total} 字", offset + 1)
            };
            with_title(summary, title(value))
        }
        "api_call" | "api_read" => {
            let count = count("items");
            let summary = if count == 0 {
                "未返回数据条目".into()
            } else if value["truncated"].as_bool() == Some(true) {
                format!("返回 {count} 条数据，结果有截断")
            } else {
                format!("返回 {count} 条数据")
            };
            with_title(summary, title(value))
        }
        "style_read" if value["available"].as_bool() == Some(false) => "风格档案不可用".into(),
        "style_read" => with_title("已读取风格".into(), title(value)),
        "check_document" => format!(
            "检查完成 · {} 项拦截、{} 项提醒",
            count("hard"),
            count("soft")
        ),
        "doc_elements" => "已读取本稿表单".into(),
        "current_date" => "已读取本机日期".into(),
        _ => "已返回结果".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_activity_uses_query_count_and_title_without_payload() {
        assert_eq!(
            tool_action(
                "kb_search",
                &json!({"query": "消防\n整改", "text": "原文"}),
                ""
            ),
            "查知识库 · 消防 整改"
        );
        let result = Ok(json!({"results": [
            {"title":"检查指引","text":"涉密正文"}, {"title":"整改通知"}
        ]}));
        assert_eq!(
            tool_outcome("kb_search", &result, ""),
            "找到 2 段相关资料 · 《检查指引》"
        );
        assert_eq!(
            tool_outcome("kb_search", &Ok(json!({"results": []})), ""),
            "未找到相关资料"
        );
        assert_eq!(
            tool_outcome("kb_search", &Err("内部凭据".into()), ""),
            "调用失败"
        );
    }

    #[test]
    fn read_activity_distinguishes_page_from_full_text() {
        let page = Ok(json!({"title":"检查指引","offset":20,"next_offset":40,"total_chars":100}));
        assert_eq!(
            tool_outcome("kb_read", &page, ""),
            "已读第 21–40 字，共 100 字 · 《检查指引》"
        );
        assert_eq!(label("access-secret", "access-secret"), "［已隐藏］");
    }
}
