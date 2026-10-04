//! 单次请求的装箱（`docs/ai-agent-workbench.md` 16.15 A.4）：固定部分（系统提示、指令、用户原话、
//! 选区）一字不砍，可伸缩部分（正文、证据、风格范例、会话历史）按输入预算放。
//!
//! 输出上限由 `lmstudio` 在发请求时按剩余空间现算；这里只管输入别超：给输出留出窗口的四分之一
//! （夹在 2048 到 16384 之间），其余是输入预算。

use crate::lmstudio::context::{MIN_OUTPUT, estimate_tokens};

/// 给输出留多少 token。
pub(crate) fn output_reserve(window: usize) -> usize {
    (window / 4).clamp(MIN_OUTPUT, 16_384)
}

/// 输入能用多少 token。
pub(crate) fn input_budget(window: usize) -> usize {
    window.saturating_sub(output_reserve(window))
}

/// token 折成汉字数（估算是 1 字 1.1 token）。
pub(crate) fn tokens_to_chars(tokens: usize) -> usize {
    tokens * 10 / 11
}

/// 去掉固定部分后，可伸缩的那块还能放多少字。
pub(crate) fn room_chars(window: usize, fixed: &str) -> usize {
    tokens_to_chars(input_budget(window).saturating_sub(estimate_tokens(fixed)))
}

/// 把长文按段落切成每块不超过 `room` 字的几块，尽量在标题行前断开；单段超长的硬切。
pub(crate) fn split_by_budget(text: &str, room: usize) -> Vec<String> {
    let room = room.max(200);
    if text.chars().count() <= room {
        return vec![text.to_string()];
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    let flush = |current: &mut String, used: &mut usize, parts: &mut Vec<String>| {
        if !current.trim().is_empty() {
            parts.push(std::mem::take(current));
        }
        current.clear();
        *used = 0;
    };
    for line in text.split_inclusive('\n') {
        let len = line.chars().count();
        let heading = line.trim_start().starts_with('#');
        // 块已经过半时遇到标题就断开，让每块尽量是完整的章节。
        if used + len > room || (heading && used > room / 2) {
            flush(&mut current, &mut used, &mut parts);
        }
        if len > room {
            let chars: Vec<char> = line.chars().collect();
            for piece in chars.chunks(room) {
                parts.push(piece.iter().collect());
            }
            continue;
        }
        current.push_str(line);
        used += len;
    }
    flush(&mut current, &mut used, &mut parts);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_leave_room_for_output() {
        assert_eq!(output_reserve(32_768), 8192);
        assert_eq!(input_budget(32_768), 24_576);
        assert_eq!(output_reserve(4096), MIN_OUTPUT);
        assert_eq!(input_budget(131_072), 131_072 - 16_384);
        assert_eq!(room_chars(32_768, ""), 24_576 * 10 / 11);
    }

    #[test]
    fn long_text_is_split_at_paragraphs_and_headings() {
        let para = "一".repeat(150);
        let text = format!("# 一、总体要求\n{para}\n{para}\n# 二、重点任务\n{para}\n");
        let parts = split_by_budget(&text, 400);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert!(parts[0].starts_with("# 一、"));
        assert!(parts[1].starts_with("# 二、"));
        assert_eq!(parts.concat(), text, "切开后拼回去一字不差");

        let long = "二".repeat(1000);
        let parts = split_by_budget(&long, 300);
        assert_eq!(parts.len(), 4);
        assert!(parts.iter().all(|p| p.chars().count() <= 300));
        assert_eq!(split_by_budget("短文", 300), vec!["短文".to_string()]);
    }
}
