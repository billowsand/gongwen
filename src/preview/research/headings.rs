//! 标题、题注、部分/章/节与浮动体间距。
//!
//! 由 src/preview/research.rs 拆分而来：本文件是模块 `preview::research::headings`，与其它子模块共享
//! `preview::research` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::preview::layout::{TextRun, line_block, line_block_runs_spaced};
use crate::preview::{
    INDENT_CHARS, Metrics, RESEARCH_BODY_PT, RESEARCH_CAPTION_PT, RESEARCH_CHAPTER_PT,
    RESEARCH_PART_PT, indent, math_flow,
};
use crate::theme;

use egui::Align;

/// 章号与章名之间的间隔：ctex 的 `chapter/aftername` 默认 `\quad`，一个汉字宽，
/// 用全角空格对得上。
pub(crate) const CHAPTER_GAP: &str = "\u{3000}";

/// 节号与节名之间的间隔：`\titleformat{\section}{...}{\thesection}{0.5em}{}`
/// 的第三个参数是 0.5em，只有章标题那档才是一个汉字。预览里的黑体西文是半角
/// 等宽（`.`、数字、空格都占 0.5em），所以一个 ASCII 空格正好是 0.5em；这里若
/// 用全角空格就宽出一倍——"1.1" 本身没有夹空格，看着宽是这个间隔撑的。
/// 不用 U+2002（en space）是因为黑体没有这个码位，会掉进回退字体。
pub(crate) const SECTION_GAP: &str = " ";

/// 图与图题之间的空：`md2tex.cls` 的 `\abovecaptionskip`。
pub(crate) const CAPTION_GAP_PT: f32 = 6.0;

/// 图题、表题的行距：`\zihao{-4}` 在 ctex 下的行距约 18 磅。占正文 24 磅的
/// 行框，字下方会多出一截空，表题就离表格远、离上一段近了。
pub(crate) const CAPTION_LINE_PT: f32 = 18.0;

/// 图、表与上下正文之间多留的空，对应 PDF 里 figure 的 `\intextsep`、longtblr
/// 的 presep / postsep。取值对照编译出的 PDF 量过（见 [`space_after_float`]）。
pub(crate) const FLOAT_SEP_PT: f32 = 8.0;

/// 图、表之后的空。预览里一行字的行距留白全在字的下方（egui 的行框从字顶
/// 起算），所以正文紧跟在表格边框、图题后面时字顶一点空都没有——表格前面有
/// 上一段自带的行距留白，后面没有。这里把正文一行的留白补上，前后才对称：
/// 表格下边框到下一段字顶约 18 磅，与 PDF 相当。
pub(crate) fn space_after_float(ui: &mut egui::Ui, metrics: &Metrics) {
    let leading = metrics.line - metrics.pt(RESEARCH_BODY_PT);
    ui.add_space(metrics.pt(FLOAT_SEP_PT) + leading);
}

/// 图题、表题：标签（"图 1.1""表 1.1"）黑体、题名宋体，都是小四，居中一行，
/// 标签后空半个字——与 `md2tex.cls` 里 caption 包和 tabularray 的两处设置一致。
/// 预览的黑体西文是半角，一个 ASCII 空格正好是 0.5em（同 [`SECTION_GAP`]）。
pub(crate) fn caption_line(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    tag: &str,
    number: &str,
    text: &str,
) {
    line_block_runs_spaced(
        ui,
        metrics,
        &[
            TextRun {
                text: &format!("{tag} {number} "),
                family: theme::FONT_HEITI,
                size: RESEARCH_CAPTION_PT,
            },
            TextRun {
                text,
                family: metrics.body_family,
                size: RESEARCH_CAPTION_PT,
            },
        ],
        Align::Center,
        metrics.pt(CAPTION_LINE_PT),
    );
}

/// 章标题：小二黑体居中，上下各空一行。
pub(crate) fn chapter_title(ui: &mut egui::Ui, metrics: &Metrics, text: &str) {
    ui.add_space(metrics.line);
    if metrics.terminal {
        let (prefix, title) = text
            .split_once(CHAPTER_GAP)
            .map_or((None, text), |(p, t)| (Some(p), t));
        let number = prefix.map(|p| {
            p.trim_start_matches("第")
                .trim_end_matches("章")
                .trim_start_matches("附录")
        });
        crate::preview::research_terminal::heading(ui, metrics, title, number, prefix, false);
        return;
    }
    title_line(
        ui,
        metrics,
        text,
        theme::FONT_HEITI,
        RESEARCH_CHAPTER_PT,
        Align::Center,
        "",
    );
    ui.add_space(metrics.line);
}

/// 标题类的一行字：含行内公式时交给 `math_flow` 排成公式（与 PDF 一致），
/// 否则照常按纯文本排。`lead` 是首行前缀（节标题的 2 字缩进）。
pub(crate) fn title_line(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    text: &str,
    family: &str,
    size: f32,
    align: Align,
    lead: &str,
) {
    if math_flow::has_math(text) {
        let style = math_flow::FlowStyle::heading(metrics, family, size, metrics.line, align, lead);
        math_flow::flow(ui, metrics, text, &style);
    } else {
        line_block(ui, metrics, &format!("{lead}{text}"), family, size, align);
    }
}

/// 部分标题：小一黑体居中，"第一部分"一行、题目一行（ctex 的 part 格式）。
/// PDF 里部分独占一页；预览是一张连续的纸，上下各留三行空白示意。
pub(crate) fn part_title(ui: &mut egui::Ui, metrics: &Metrics, heading: Option<&str>, text: &str) {
    ui.add_space(metrics.line * 3.0);
    if let Some(heading) = heading {
        line_block(
            ui,
            metrics,
            heading,
            theme::FONT_HEITI,
            RESEARCH_PART_PT,
            Align::Center,
        );
        ui.add_space(metrics.line * 0.5);
    }
    title_line(
        ui,
        metrics,
        text,
        theme::FONT_HEITI,
        RESEARCH_PART_PT,
        Align::Center,
        "",
    );
    ui.add_space(metrics.line * 3.0);
}

/// 节标题：黑体，字号随正文，缩进 2 字（`\titlespacing` 的 2em）。
pub(crate) fn section_title(ui: &mut egui::Ui, metrics: &Metrics, text: &str) {
    if metrics.terminal {
        crate::preview::research_terminal::section(ui, metrics, text);
        return;
    }
    let _ink = metrics
        .terminal
        .then(|| theme::paper::report_ink(theme::paper::ink_muted()));
    let lead = if metrics.terminal {
        String::new()
    } else {
        indent(INDENT_CHARS)
    };
    title_line(
        ui,
        metrics,
        text,
        theme::FONT_HEITI,
        RESEARCH_BODY_PT,
        Align::LEFT,
        &lead,
    );
}
