//! 公文标题的排布：超出一行不多于 2 个全角字宽时**横向压缩字形**保持单行（字高不变），
//! 超出更多时用 jieba 分词在词边界均衡换行，词不被拆到两行。

use std::sync::OnceLock;

/// 标题基准字号：二号（22 pt）。汉字为方形，字宽=字高=字号。
pub const TITLE_BASE_SIZE_PT: usize = 22;
/// 版心宽度（约 15.6cm）：以 OOXML 半磅体系的 pt 计，8845 twip = 442.25 pt；
/// LaTeX 的 156mm 约 443.9 TeX pt，二者对应同一物理宽度，取较小值使压缩结果偏保守，
/// 两套引擎都放得下。
pub const TITLE_LINE_WIDTH_PT: f64 = 8_845.0 / 20.0;

static JIEBA: OnceLock<jieba_rs::Jieba> = OnceLock::new();

fn jieba() -> &'static jieba_rs::Jieba {
    JIEBA.get_or_init(jieba_rs::Jieba::new)
}

/// 标题的排布方案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TitlePlan {
    /// 单行，字号保持二号。
    SingleLine,
    /// 单行，字高保持二号、仅横向收窄字形到一行放得下。
    Compressed,
    /// 多行：jieba 分词后在词边界均衡换行，每行不超 `chars_per_line` 个全角字宽。
    Wrapped(Vec<String>),
}

/// 一行（二号字号）能容纳的汉字数。
pub fn chars_per_line() -> usize {
    (TITLE_LINE_WIDTH_PT / TITLE_BASE_SIZE_PT as f64).floor() as usize
}

/// 压缩单行时的横向缩放百分比：字高不变（仍为二号），仅横向收窄字形。
/// 100 = 原宽，小于 100 为横向压缩比例；按版心宽度与显示字宽反推并限制在不小于 80%。
pub fn compressed_scale_percent(title: &str) -> usize {
    let count = display_units(title).max(2) as f64 / 2.0;
    let scale = TITLE_LINE_WIDTH_PT / (count * TITLE_BASE_SIZE_PT as f64);
    (scale * 100.0).floor().clamp(80.0, 100.0) as usize
}

/// 决定标题排布：不超一行单行；超出一行不超过 2 个全角字宽横向压缩单行；否则 jieba 换行。
pub fn title_plan(title: &str, chars_per_line: usize) -> TitlePlan {
    let width = display_units(title);
    let line_units = chars_per_line * 2;
    if width <= line_units {
        return TitlePlan::SingleLine;
    }
    if width <= line_units + 4 {
        return TitlePlan::Compressed;
    }
    let words = jieba().cut(title, true);
    TitlePlan::Wrapped(wrap_words(&words, chars_per_line))
}

/// 标题的显示宽度（半角单位）：1 个全角字符 = 2，1 个半角英数 = 1，空白不计。
fn display_units(text: &str) -> usize {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum()
}

/// 在词边界、每行不超过 `chars_per_line` 个全角字宽的约束下，把词均衡地装入各行。
/// 两行标题直接枚举断点，取“首行不短于末行”且长短最接近的断法（头不轻脚不重）；
/// 三行及以上逐行贪心：每行填到“不小于余下均长”为止。
fn wrap_words(words: &[&str], chars_per_line: usize) -> Vec<String> {
    let line_units = chars_per_line * 2;
    let total: usize = words.iter().map(|word| display_units(word)).sum();
    if total == 0 {
        return Vec::new();
    }
    let n_lines = lines_needed(words, line_units);
    if n_lines <= 1 {
        return vec![words.concat()];
    }
    if n_lines == 2 {
        return wrap_two_lines(words, line_units);
    }
    wrap_greedy_balanced(words, n_lines, line_units)
}

/// 每行不超过 `limit_units` 个半角单位（词不拆开）时，贪心能分成的最少行数。
fn lines_needed(words: &[&str], limit_units: usize) -> usize {
    let mut lines = 0usize;
    let mut current = 0usize;
    for word in words {
        let word_len = display_units(word);
        if current != 0 && current + word_len > limit_units {
            lines += 1;
            current = 0;
        }
        current += word_len;
    }
    if current > 0 {
        lines += 1;
    }
    lines
}

/// 两行标题：枚举词边界断点，优先选“第一行不短于第二行”且长短差最小的断法；
/// 全部断点都“头轻脚重”时，退回选长短差最小者。
fn wrap_two_lines(words: &[&str], line_units: usize) -> Vec<String> {
    let mut head_len = 0usize;
    let mut best_heavy: Option<(usize, usize)> = None; // (长短差, 断点)
    let mut best_any: Option<(usize, usize)> = None;
    for split in 1..words.len() {
        head_len += display_units(words[split - 1]);
        let tail_len = words[split..]
            .iter()
            .map(|word| display_units(word))
            .sum::<usize>();
        if head_len > line_units || tail_len > line_units {
            continue;
        }
        let imbalance = head_len.abs_diff(tail_len);
        if head_len >= tail_len && best_heavy.is_none_or(|(best, _)| imbalance < best) {
            best_heavy = Some((imbalance, split));
        }
        if best_any.is_none_or(|(best, _)| imbalance < best) {
            best_any = Some((imbalance, split));
        }
    }
    let split = best_heavy
        .or(best_any)
        .map(|(_, split)| split)
        .unwrap_or(words.len() / 2);
    vec![words[..split].concat(), words[split..].concat()]
}

/// 三行及以上：逐行贪心，每行填到“不小于余下均长”为止（头不轻脚不重），
/// 同时确保余下内容仍能装进剩余行数。
fn wrap_greedy_balanced(words: &[&str], n_lines: usize, line_units: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut index = 0usize;
    for line_index in 0..n_lines {
        let remaining_lines = n_lines - line_index;
        let mut current = 0usize;
        let mut end = index;
        loop {
            if end >= words.len() {
                break;
            }
            let word_len = display_units(words[end]);
            if current != 0 && current + word_len > line_units {
                break;
            }
            if remaining_lines == 1 {
                end = words.len();
                break;
            }
            let rest: usize = words[end + 1..]
                .iter()
                .map(|word| display_units(word))
                .sum();
            // 余下必须仍能装进剩余行数-1 行。
            if lines_needed(&words[end + 1..], line_units) > remaining_lines - 1 {
                break;
            }
            // 本行已不短于余下均长即停，避免头轻脚重。
            if current >= rest.div_ceil(remaining_lines - 1) {
                break;
            }
            current += word_len;
            end += 1;
        }
        lines.push(words[index..end].concat());
        index = end;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_one_line_within_capacity() {
        assert_eq!(
            title_plan("关于开展测试工作的函", chars_per_line()),
            TitlePlan::SingleLine
        );
    }

    #[test]
    fn small_overflow_compresses_horizontally_only() {
        // 一行约 20 个全角字宽：21/22 个全角字宽属于“超出一行不超过 2 个全角字宽”。
        let title = "关于认真做好网络安全与信息化重点工作验收的函";
        assert_eq!(display_units(title), 44);
        assert_eq!(title_plan(title, 20), TitlePlan::Compressed);
        // 字高不变（字号仍是二号），仅横向收窄字形。
        let scale = compressed_scale_percent(title);
        assert!((80..100).contains(&scale), "横向缩放应小于原宽：{scale}");
    }

    #[test]
    fn half_width_characters_count_as_half_a_full_width_char() {
        assert_eq!(display_units("2026"), 4);
        assert_eq!(display_units("2026年"), 6);
        assert_eq!(display_units("关于2026年"), 10);

        // 32 个半角英数只占 16 个全角字宽，仍可单行。
        let half = "ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        assert_eq!(display_units(half), 32);
        assert_eq!(title_plan(half, chars_per_line()), TitlePlan::SingleLine);

        // 21 个汉字 + 2 个半角字符 = 22 个全角字宽，按实际宽度应走横向压缩。
        let base = "关于认真做好网络安全与信息化重点工作验收的函";
        let mixed = format!("{}AB", &base[..base.len() - "函".len()]);
        assert_eq!(display_units(&mixed), 44);
        assert_eq!(title_plan(&mixed, 20), TitlePlan::Compressed);
    }

    #[test]
    fn large_overflow_wraps_at_word_boundaries() {
        let title = "关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知";
        assert!(display_units(title) > 44);
        let TitlePlan::Wrapped(lines) = title_plan(title, 20) else {
            panic!("应走 jieba 换行");
        };
        assert!(lines.len() >= 2);
        for line in &lines {
            assert!(display_units(line) <= 40, "单行不得超过上限：{line}");
            assert!(!line.is_empty());
        }
        // 换行按词边界进行：拼回原标题，字符不增删。
        assert_eq!(lines.join(""), title);
    }

    #[test]
    fn wrapped_title_never_has_light_head_and_heavy_foot() {
        // 首行不短于末行（头不轻脚不重），且相邻行长短不悬殊。
        let titles = [
            "关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知",
            "关于开展2026年度网络安全考核评估暨重点工作任务落实情况检查的通知",
            "关于认真做好信息化建设及网络安全管理有关重点事项整改落实工作的通知",
        ];
        for title in titles {
            let TitlePlan::Wrapped(lines) = title_plan(title, 20) else {
                panic!("应走 jieba 换行：{title}");
            };
            assert_eq!(lines.join(""), title);
            let first = display_units(&lines[0]);
            let last = display_units(lines.last().unwrap());
            assert!(
                first >= last,
                "首行（{first} 半角单位）不得短于末行（{last} 半角单位）：{title} -> {lines:?}"
            );
            for line in &lines {
                assert!(display_units(line) <= 40, "单行超上限：{line}");
            }
        }
    }

    #[test]
    fn wrapped_lines_rejoin_to_original() {
        let title = "关于组织召开2026年度网络安全工作推进会商会议的通知";
        let TitlePlan::Wrapped(lines) = title_plan(title, 20) else {
            panic!("应走 jieba 换行");
        };
        assert_eq!(lines.join(""), title, "换行不得增删字符");
        for line in &lines {
            assert!(display_units(line) <= 40, "单行超上限：{line}");
        }
    }
}
