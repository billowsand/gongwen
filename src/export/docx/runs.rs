//! OOXML run 级构建：字体、正文/黑体/密级 run 与标题/表格 run。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::runs`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{split_period_digits};
use crate::export::{inline_segments, plain_text};
use crate::export::docx::{BODY_SIZE, FOOTER_SIZE, PAREN_SIZE};
use docx_rs::*;

pub(crate) fn chinese_fonts(name: &str) -> RunFonts {
    RunFonts::new()
        .ascii("Times New Roman")
        .hi_ansi("Times New Roman")
        .east_asia(name)
}

pub(crate) fn body_run(text: impl Into<String>) -> Run {
    Run::new()
        .add_text(text)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(BODY_SIZE)
}

/// 正文 run 序列：Markdown 加粗保留为同字体自动加粗；完整圆括号/方头括号及其中内容
/// 用楷体_GB2312 四号，其余用仿宋三号。
/// 标题（文档标题、各级标题、附件标签）不经由此处，不受此规则影响。
pub(crate) fn body_runs(text: &str) -> Vec<Run> {
    let segments = inline_segments(text);
    if segments.is_empty() {
        return vec![body_run("")];
    }
    segments
        .into_iter()
        .map(|segment| {
            let mut run = if segment.parenthesized {
                Run::new()
                    .add_text(segment.text)
                    .fonts(chinese_fonts("楷体_GB2312"))
                    .size(PAREN_SIZE)
            } else {
                body_run(segment.text)
            };
            if segment.bold {
                // 不切换为单独粗体字体，只设置加粗属性，由当前字体自动加粗。
                run = run.bold();
            }
            run
        })
        .collect()
}

pub(crate) fn heiti_run(text: impl Into<String>) -> Run {
    Run::new()
        .add_text(text)
        .fonts(chinese_fonts("黑体"))
        .size(BODY_SIZE)
        .bold()
}

/// 密级行 run 序列：数字年限的保密期限，前导数字用等宽西文字体（对应 LaTeX 的
/// `\ttfamily`），其余用行内基准字体；指人专办以黑体加粗追加在末尾。
pub(crate) fn security_runs(level: &str, period: &str, special: &str, base: &str, bold: bool) -> Vec<Run> {
    let base_run = |text: &str| {
        let mut run = Run::new()
            .add_text(text.to_string())
            .fonts(chinese_fonts(base))
            .size(BODY_SIZE);
        if bold {
            run = run.bold();
        }
        run
    };
    let (digits, rest) = split_period_digits(period);
    let mut runs = vec![base_run(&format!("{level}★"))];
    if !digits.is_empty() {
        let mut run = Run::new()
            .add_text(digits)
            .fonts(
                RunFonts::new()
                    .ascii("Courier New")
                    .hi_ansi("Courier New")
                    .east_asia(base),
            )
            .size(BODY_SIZE);
        if bold {
            run = run.bold();
        }
        runs.push(run);
    }
    if !rest.is_empty() {
        runs.push(base_run(rest));
    }
    if !special.is_empty() {
        runs.push(heiti_run(special));
    }
    runs
}

/// 落款单位 run 序列：少于 5 字时逐字设置字符间距（单位缇，1/20 磅）分散对齐到
/// 5 字宽——16pt 字号下 1em=320 缇，总宽恰好为 5 个字；否则整串一个 run。
pub(crate) fn spread_runs(text: &str) -> Vec<Run> {
    match crate::units::spread_gap(text) {
        Some(gap) => {
            let spacing = (gap * 320.0).round() as i32;
            let chars = text.chars().collect::<Vec<_>>();
            chars
                .iter()
                .enumerate()
                .map(|(index, ch)| {
                    let mut run = body_run(ch.to_string());
                    // 最后一个字后面没有字符，不再设间距，避免总宽超出 5 字。
                    if index + 1 < chars.len() {
                        run = run.character_spacing(spacing);
                    }
                    run
                })
                .collect()
        }
        None => vec![body_run(text)],
    }
}

pub(crate) fn record_run(text: &str) -> Run {
    Run::new()
        .add_text(text)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(FOOTER_SIZE)
}

/// 规格 §3.2/§6 姓名宽度：2 字姓名中间加全角空格占 3 字宽，4 字姓名用更小字号近似压缩到 3 字宽。
pub(crate) fn docx_name(value: &str, base_size: usize) -> (String, usize) {
    let chars = value.chars().collect::<Vec<_>>();
    match chars.len() {
        2 => (format!("{}\u{2003}{}", chars[0], chars[1]), base_size),
        4 => (value.to_string(), (base_size as f32 * 0.75) as usize),
        _ => (value.to_string(), base_size),
    }
}

/// 公文主标题 run：小标宋，字号按排布方案给出（半磅）。
pub(crate) fn title_run(text: &str, size: usize) -> Run {
    Run::new()
        .add_text(plain_text(text))
        .fonts(chinese_fonts("方正小标宋简体"))
        .size(size)
}

pub(crate) fn table_run_sized(text: &str, header: bool, size: usize) -> Run {
    let mut run = Run::new()
        .add_text(plain_text(text))
        .fonts(chinese_fonts(if header {
            "黑体"
        } else {
            "仿宋_GB2312"
        }))
        .size(size);
    if header {
        run = run.bold();
    }
    run
}

pub(crate) fn table_runs_sized(text: &str, header: bool, size: usize) -> Vec<Run> {
    if header {
        return vec![table_run_sized(text, true, size)];
    }
    let segments = inline_segments(text);
    if segments.is_empty() {
        return vec![table_run_sized("", false, size)];
    }
    segments
        .into_iter()
        .map(|segment| {
            let mut run = table_run_sized(&segment.text, false, size);
            if segment.bold {
                run = run.bold();
            }
            run
        })
        .collect()
}
