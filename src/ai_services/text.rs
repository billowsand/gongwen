//! 共享的文字处理：截断、结构化结果转文字、把正文切成句子。
//!
//! 都从旧 `agent::tools::short`、`agent::board::value_to_text`、`agent::gaps::sentence_spans`
//! 原样搬来，Unicode、标点与字数边界行为不变——预览、侧栏、API 工作台与 Pi 共用同一份。

use super::citations::CITATION;
use serde_json::Value;
use std::ops::Range;

/// 截断成给人看的短文字。
pub(crate) fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "…"
}

/// 结构化结果 → 给模型看的文字：字符串原样；列表逐条编号（对象取标题与正文）；对象按「键：值」。
pub(crate) fn value_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => if *flag { "是" } else { "否" }.into(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| format!("{}. {}", index + 1, item_line(item)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .iter()
            .map(|(key, v)| format!("{key}：{}", value_to_text(v)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// 列表里的一项写成一行：对象取标题与说明，其余照 [`value_to_text`]。
pub(crate) fn item_line(item: &Value) -> String {
    let Value::Object(map) = item else {
        return value_to_text(item);
    };
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map(str::to_string)
    };
    match (
        pick(&["title", "name", "canonical"]),
        pick(&["text", "summary", "note"]),
    ) {
        (Some(title), Some(text)) => format!("{title}：{text}"),
        (Some(title), None) => title,
        (None, Some(text)) => text,
        (None, None) => value_to_text(item).replace('\n', "；"),
    }
}

/// 句末标点。
pub(crate) fn is_sentence_end(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '；' | '!' | '?')
}

/// 把正文切成句子（字节区间）。按行、再按句末标点切；句号后紧跟的引用标记 `[K3]`
/// 归前一句——模型常把标记写在句号后面。首尾空白不计入。
pub(crate) fn sentence_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut line_start = 0usize;
    for line in text.split('\n') {
        let mut start = 0usize;
        let mut iter = line.char_indices().peekable();
        while let Some((index, ch)) = iter.next() {
            if !is_sentence_end(ch) {
                continue;
            }
            let mut end = index + ch.len_utf8();
            if let Some(found) = CITATION.find(&line[end..])
                && found.start() == 0
            {
                end += found.end();
                while iter.peek().is_some_and(|(next, _)| *next < end) {
                    iter.next();
                }
            }
            push_trimmed(line, line_start, start..end, &mut spans);
            start = end;
        }
        push_trimmed(line, line_start, start..line.len(), &mut spans);
        line_start += line.len() + 1;
    }
    spans
}

fn push_trimmed(line: &str, line_start: usize, range: Range<usize>, out: &mut Vec<Range<usize>>) {
    let raw = &line[range.clone()];
    let lead = raw.len() - raw.trim_start().len();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let start = line_start + range.start + lead;
    out.push(start..start + trimmed.len());
}
