//! OOXML run 级构建：字体、正文/黑体/密级 run 与标题/表格 run。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::runs`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::docx::{BODY_SIZE, FOOTER_SIZE, PAREN_SIZE};
use crate::export::{RedlineKind, inline_segments, plain_text, redline_chunks};
use crate::models::split_period_digits;
use crate::visual_diff::elements::FieldMark;
use docx_rs::*;

pub(crate) fn chinese_fonts(name: &str) -> RunFonts {
    RunFonts::new().ascii(name).hi_ansi(name).east_asia(name)
}

/// 正文加粗怎么排：`None` 让 Word 用当前字体合成粗体（对应 TeX 的
/// `AutoFakeBold`），`Some(家族名)` 换用设置里选定的专用粗体字体。
pub(crate) type BoldFont<'a> = Option<&'a str>;

/// 给一个 run 加粗。换字体时不再叠加 `w:b`——真正的粗体字面本身就是粗的，
/// 再让 Word 合成一层会把笔画糊成一团。
pub(crate) fn apply_bold(run: Run, bold: BoldFont<'_>) -> Run {
    match bold {
        Some(family) => run.fonts(chinese_fonts(family)),
        None => run.bold(),
    }
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
pub(crate) fn body_runs(text: &str, bold: BoldFont<'_>) -> Vec<Run> {
    // 先按花脸稿哨兵切块。没有哨兵时只有一块 `Same`，与从前完全一致。
    let chunks = redline_chunks(text);
    let mut runs = Vec::new();
    for chunk in chunks {
        let segments = inline_segments(&chunk.text);
        if segments.is_empty() {
            continue;
        }
        for segment in segments {
            let mut run = if segment.parenthesized {
                Run::new()
                    .add_text(segment.text)
                    .fonts(chinese_fonts("楷体_GB2312"))
                    .size(PAREN_SIZE)
            } else {
                body_run(segment.text)
            };
            if segment.bold {
                run = apply_bold(run, bold);
            }
            run = apply_redline(run, chunk.kind);
            runs.push(run);
        }
    }
    if runs.is_empty() {
        return vec![body_run("")];
    }
    runs
}

/// 给一个 run 打上花脸稿标记（方案需求结论第 10 条：删除 = 红色直删除线，
/// 新增 = 蓝色方框，PDF / Word / 预览三处一致）。
///
/// Word 画不出穿过文字的波浪线——OOXML 的波浪只有下划线一种。删除用红色
/// 直删除线；新增用字符边框：Word 会把相邻且边框设置相同的 run 自动并成
/// 一个框，"多个字一个框"是天然就有的。
fn apply_redline(run: Run, kind: RedlineKind) -> Run {
    match kind {
        RedlineKind::Same => run,
        RedlineKind::Deleted => run.strike().color("C00000"),
        RedlineKind::Added => run.text_border(
            TextBorder::new()
                .border_type(BorderType::Single)
                .size(4)
                .space(1)
                .color("1F4E9E"),
        ),
    }
}

/// 标题 / 版记类块的 run 序列：按花脸稿哨兵切块，删除块加红色删除线、
/// 新增块加蓝色字符边框，其余与 `base` 构造的 run 完全一致（字体、字号、
/// 加粗都在 `base` 里）。没有哨兵时输出与从前单个 run 相同。视觉 diff
/// 引擎在解析后注入哨兵，标题、附件标题的改动因此也就地标注。
pub(crate) fn marked_runs(text: &str, base: impl Fn(&str) -> Run) -> Vec<Run> {
    let chunks = redline_chunks(text);
    if chunks.len() == 1 && chunks[0].kind == RedlineKind::Same {
        return vec![base(&plain_text(&chunks[0].text))];
    }
    chunks
        .iter()
        .map(|chunk| apply_redline(base(&plain_text(&chunk.text)), chunk.kind))
        .collect()
}

pub(crate) fn heiti_run(text: impl Into<String>) -> Run {
    Run::new()
        .add_text(text)
        .fonts(chinese_fonts("黑体"))
        .size(BODY_SIZE)
        .bold()
}

/// 密级行 run 序列：中文、西文及期限数字统一使用行内基准字体；
/// 指人专办以黑体加粗追加在末尾。
///
/// 要素标注（`mark`）：密级（含保密期限）整字段替换——变了就把「旧值删除线、
/// 新值加框」整段排成标注 run（期限数字不再单独分 run，整段样式一致），
/// 指人专办照旧追加。
pub(crate) fn security_runs(
    level: &str,
    period: &str,
    special: &str,
    base: &str,
    bold: bool,
    mark: &FieldMark,
) -> Vec<Run> {
    if mark.changed() {
        let mut runs = marked_runs(&mark.marked(), |text| security_base_run(text, base, bold));
        if !special.is_empty() {
            runs.push(heiti_run(special));
        }
        return runs;
    }
    let (digits, rest) = split_period_digits(period);
    // 保密期限为空的（“内部”件）只印密级二字，不出“★”。
    let heading = if period.trim().is_empty() {
        level.to_string()
    } else {
        format!("{level}★")
    };
    let mut runs = vec![security_base_run(&heading, base, bold)];
    if !digits.is_empty() {
        runs.push(security_base_run(digits, base, bold));
    }
    if !rest.is_empty() {
        runs.push(security_base_run(rest, base, bold));
    }
    if !special.is_empty() {
        runs.push(heiti_run(special));
    }
    runs
}

/// 密级行的基准 run：行内基准字体，可加粗。
pub(crate) fn security_base_run(text: &str, base: &str, bold: bool) -> Run {
    let mut run = Run::new()
        .add_text(text.to_string())
        .fonts(chinese_fonts(base))
        .size(BODY_SIZE);
    if bold {
        run = run.bold();
    }
    run
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

/// 表格 run。表头对齐 TeX 的 `row{1}={font=\heiti\enheiti}`：只换黑体，不加粗。
pub(crate) fn table_run_sized(text: &str, header: bool, size: usize) -> Run {
    Run::new()
        .add_text(plain_text(text))
        .fonts(chinese_fonts(if header {
            "黑体"
        } else {
            "仿宋_GB2312"
        }))
        .size(size)
}

pub(crate) fn table_runs_sized(
    text: &str,
    header: bool,
    size: usize,
    bold: BoldFont<'_>,
) -> Vec<Run> {
    if header {
        return vec![table_run_sized(text, true, size)];
    }
    // 与正文一样先按花脸稿哨兵切块：表格里改掉的时限、数字，正是最该让人一眼
    // 看见的地方。没有哨兵时只有一块，与从前完全一致。
    let mut runs = Vec::new();
    for chunk in redline_chunks(text) {
        for segment in inline_segments(&chunk.text) {
            let mut run = table_run_sized(&segment.text, false, size);
            if segment.bold {
                run = apply_bold(run, bold);
            }
            runs.push(apply_redline(run, chunk.kind));
        }
    }
    if runs.is_empty() {
        return vec![table_run_sized("", false, size)];
    }
    runs
}
