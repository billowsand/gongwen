//! 研究报告花脸稿：把正文里的哨兵字符（`export::text` 的 `REDLINE_*`）换成片段上的
//! 标注，交给模板画删除线、新增框。
//!
//! 哨兵是视觉 diff 引擎插进 Markdown 的私用区码位，mdx 解析时当普通文字原样带过来，
//! 可能落在任何一段文字里，也可能一对哨兵跨过加粗、链接这些片段。这里按文档顺序
//! 走每一串行内片段（JSON 里元素带 `t` 的数组），带着当前状态切开文字片段、给每个
//! 叶子片段标上 `m`（`del` / `add`）；公式、代码这类原子片段里的哨兵剥掉，整个片段
//! 按里面的第一个哨兵标注。题注、文框名称这类纯文字字段只剥哨兵。没有哨兵的文档
//! 原样不动。

use serde_json::{Map, Value};

use crate::export::{REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    None,
    Del,
    Add,
}

impl Mark {
    fn tag(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Del => Some("del"),
            Self::Add => Some("add"),
        }
    }
}

fn is_sentinel(ch: char) -> bool {
    matches!(
        ch,
        REDLINE_DEL_OPEN | REDLINE_DEL_CLOSE | REDLINE_ADD_OPEN | REDLINE_ADD_CLOSE
    )
}

fn step(mark: Mark, ch: char) -> Mark {
    match ch {
        REDLINE_DEL_OPEN => Mark::Del,
        REDLINE_ADD_OPEN => Mark::Add,
        REDLINE_DEL_CLOSE | REDLINE_ADD_CLOSE => Mark::None,
        _ => mark,
    }
}

/// 整篇数据里有没有哨兵：没有就不必走一遍。
fn has_sentinels(value: &Value) -> bool {
    match value {
        Value::String(s) => s.chars().any(is_sentinel),
        Value::Array(items) => items.iter().any(has_sentinels),
        Value::Object(map) => map.values().any(has_sentinels),
        _ => false,
    }
}

/// 换掉整篇数据里的哨兵。
pub(crate) fn apply(data: &mut Value) {
    if has_sentinels(data) {
        walk(data);
    }
}

fn is_runs(items: &[Value]) -> bool {
    !items.is_empty() && items.iter().all(|item| item.get("t").is_some())
}

fn walk(value: &mut Value) {
    match value {
        Value::Array(items) if is_runs(items) => {
            let mut mark = Mark::None;
            *items = mark_runs(std::mem::take(items), &mut mark);
        }
        Value::Array(items) => items.iter_mut().for_each(walk),
        Value::Object(map) => {
            // 独立公式块：哨兵剥掉，整块标注。
            if map.get("k").and_then(Value::as_str) == Some("math") {
                atom(map, "v", &mut Mark::None);
            }
            for child in map.values_mut() {
                walk(child);
            }
        }
        Value::String(s) if s.chars().any(is_sentinel) => {
            *s = s.chars().filter(|ch| !is_sentinel(*ch)).collect();
        }
        _ => {}
    }
}

/// 一串行内片段：文字按哨兵切开，加粗、斜体递归（状态跨片段延续），其余是原子。
fn mark_runs(runs: Vec<Value>, mark: &mut Mark) -> Vec<Value> {
    let mut out = Vec::with_capacity(runs.len());
    for mut run in runs {
        let Some(map) = run.as_object_mut() else {
            out.push(run);
            continue;
        };
        match map.get("t").and_then(Value::as_str) {
            Some("s") => {
                let text = map.get("v").and_then(Value::as_str).unwrap_or_default();
                let mut piece = String::new();
                let emit = |piece: &mut String, mark: Mark, out: &mut Vec<Value>| {
                    if piece.is_empty() {
                        return;
                    }
                    let mut map = Map::new();
                    map.insert("t".into(), "s".into());
                    map.insert("v".into(), std::mem::take(piece).into());
                    if let Some(tag) = mark.tag() {
                        map.insert("m".into(), tag.into());
                    }
                    out.push(Value::Object(map));
                };
                for ch in text.chars() {
                    if is_sentinel(ch) {
                        emit(&mut piece, *mark, &mut out);
                        *mark = step(*mark, ch);
                    } else {
                        piece.push(ch);
                    }
                }
                emit(&mut piece, *mark, &mut out);
            }
            Some("b" | "i") => {
                if let Some(Value::Array(children)) = map.get_mut("c") {
                    *children = mark_runs(std::mem::take(children), mark);
                }
                out.push(run);
            }
            _ => {
                for key in ["v", "url"] {
                    atom(map, key, mark);
                }
                if let Some(Value::Array(keys)) = map.get_mut("keys") {
                    for key in keys {
                        if let Value::String(s) = key {
                            *s = s.chars().filter(|ch| !is_sentinel(*ch)).collect();
                        }
                    }
                }
                if let Some(tag) = mark.tag() {
                    map.entry("m").or_insert(tag.into());
                }
                out.push(run);
            }
        }
    }
    out
}

/// 原子片段里的文字字段：剥掉哨兵；遇到的第一个开启哨兵决定整个片段的标注，
/// 末尾留下的状态延续到后面的片段。
fn atom(map: &mut Map<String, Value>, key: &str, mark: &mut Mark) {
    let Some(Value::String(text)) = map.get_mut(key) else {
        return;
    };
    if !text.chars().any(is_sentinel) {
        return;
    }
    let mut first = None;
    let mut clean = String::with_capacity(text.len());
    for ch in text.chars() {
        if is_sentinel(ch) {
            *mark = step(*mark, ch);
            if first.is_none() && *mark != Mark::None {
                first = Some(*mark);
            }
        } else {
            clean.push(ch);
        }
    }
    *text = clean;
    if let Some(tag) = first.and_then(Mark::tag) {
        map.insert("m".into(), tag.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_text_and_carries_marks_across_bold() {
        let mut data = json!({"blocks": [{"k": "par", "c": [
            {"t": "s", "v": "甲\u{E000}乙"},
            {"t": "b", "c": [{"t": "s", "v": "丙\u{E001}丁"}]},
            {"t": "s", "v": "\u{E002}戊\u{E003}"},
            {"t": "math", "v": "x", "d": false},
        ]}], "cover": {"doc-type": "报告"}});
        apply(&mut data);
        let c = &data["blocks"][0]["c"];
        assert_eq!(c[0], json!({"t": "s", "v": "甲"}));
        assert_eq!(c[1], json!({"t": "s", "v": "乙", "m": "del"}));
        assert_eq!(c[2]["c"][0], json!({"t": "s", "v": "丙", "m": "del"}));
        assert_eq!(c[2]["c"][1], json!({"t": "s", "v": "丁"}));
        assert_eq!(c[3], json!({"t": "s", "v": "戊", "m": "add"}));
        assert!(c[4].get("m").is_none());
    }

    #[test]
    fn atoms_and_plain_fields_lose_their_sentinels() {
        let mut data = json!({"blocks": [
            {"k": "math", "v": "\u{E002}x^2\u{E003}"},
            {"k": "figure", "caption": "图\u{E000}题\u{E001}"},
            {"k": "par", "c": [{"t": "math", "v": "\u{E000}y\u{E001}", "d": false}]},
        ]});
        apply(&mut data);
        assert_eq!(data["blocks"][0]["v"], "x^2");
        assert_eq!(data["blocks"][0]["m"], "add");
        assert_eq!(data["blocks"][1]["caption"], "图题");
        assert_eq!(data["blocks"][2]["c"][0]["v"], "y");
        assert_eq!(data["blocks"][2]["c"][0]["m"], "del");
    }
}
