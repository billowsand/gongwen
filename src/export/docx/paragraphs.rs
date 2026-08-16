//! OOXML 段落构建：正文、标题、附件、图片与议程段落。
//!
//! 由 src/export/docx.rs 拆分而来：本文件是模块 `export::docx::paragraphs`，与其它子模块共享
//! `export::docx` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::docx::{
    BODY_SIZE, CLOSING_GAP_TWIPS, RED_APPROVAL_TITLE_SIZE, TABLE_CONTENT_WIDTH_TWIPS, TITLE_SIZE,
    body_run, body_runs, chinese_fonts, heiti_run, security_runs, title_run,
};
use crate::export::plain_text;
use crate::export::title;
use crate::export::title::TitlePlan;
use crate::images;
use crate::models::{DraftInput, TemplateKind};
use docx_rs::*;
use image::GenericImageView;

pub(crate) fn body_paragraph(text: &str) -> Paragraph {
    let mut paragraph = Paragraph::new()
        .align(AlignmentType::Both)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .widow_control(true);
    for run in body_runs(text) {
        paragraph = paragraph.add_run(run);
    }
    paragraph
}

pub(crate) fn label_paragraph(text: &str) -> Paragraph {
    let mut paragraph = Paragraph::new().line_spacing(
        LineSpacing::new()
            .line(560)
            .line_rule(LineSpacingType::Exact),
    );
    for run in body_runs(text) {
        paragraph = paragraph.add_run(run);
    }
    paragraph
}

/// 函稿/电话通知顶格的密级行：密级 + ★ + 保密期限。勾选“指人专办”时，
/// 在“密级★保密期限”后空一个全角空格，再以黑体标注“指人专办”四个字。
/// 数字年限的保密期限数字部分用等宽西文字体（`security_runs`）。
pub(crate) fn letter_security_paragraph(input: &DraftInput) -> Paragraph {
    let level = input.profile.security_level.trim();
    let period = input.profile.security_period.trim();
    let special = if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        "　指人专办"
    } else {
        ""
    };
    let mut paragraph = Paragraph::new().line_spacing(
        LineSpacing::new()
            .line(560)
            .line_rule(LineSpacingType::Exact),
    );
    for run in security_runs(level, period, special, "仿宋_GB2312", false) {
        paragraph = paragraph.add_run(run);
    }
    paragraph
}

pub(crate) fn heading_paragraph(level: u8, text: &str) -> Paragraph {
    let font = match level {
        2 => "黑体",
        3 => "楷体_GB2312",
        _ => "仿宋_GB2312",
    };
    let mut run = Run::new()
        .add_text(plain_text(text))
        .fonts(chinese_fonts(font))
        .size(BODY_SIZE);
    if matches!(level, 2 | 5) {
        run = run.bold();
    }
    Paragraph::new()
        .add_run(run)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

/// 紧缩风格（规格 §4.2）：正文区 # 号最多的那一级标题与紧随其后的正文合并为一行，
/// 标题部分用该级标题字体并带编号与句号，正文部分用仿宋正文字体。
pub(crate) fn compact_heading_paragraph(level: u8, title: &str, body: &str) -> Paragraph {
    // 与 `heading_paragraph` 的层级字体保持一致：2 级黑体、3 级楷体、其余仿宋（5 级加粗）。
    let font = match level {
        2 => "黑体",
        3 => "楷体_GB2312",
        _ => "仿宋_GB2312",
    };
    let mut run = Run::new()
        .add_text(plain_text(&format!("{title}。")))
        .fonts(chinese_fonts(font))
        .size(BODY_SIZE);
    if matches!(level, 2 | 5) {
        run = run.bold();
    }
    let mut paragraph = Paragraph::new()
        .add_run(run)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .widow_control(true);
    for body_run in body_runs(body) {
        paragraph = paragraph.add_run(body_run);
    }
    paragraph
}

pub(crate) fn attachment_label_paragraph(text: &str) -> Paragraph {
    let fonts = RunFonts::new()
        .ascii("SimHei")
        .hi_ansi("SimHei")
        .east_asia("黑体");
    Paragraph::new()
        .add_run(
            Run::new()
                .add_text(plain_text(text))
                .fonts(fonts)
                .size(BODY_SIZE)
                .bold(),
        )
        .align(AlignmentType::Left)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

pub(crate) fn joint_closing_paragraph(text: &str, before: u32) -> Paragraph {
    Paragraph::new()
        .add_run(body_run(text))
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .before(before)
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
}

pub(crate) fn joint_signature_cell_paragraph(value: &str, row_index: usize) -> Paragraph {
    Paragraph::new()
        .add_run(body_run(value))
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .before(if row_index == 0 { CLOSING_GAP_TWIPS } else { 0 })
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
}

pub(crate) fn attachment_document_title_paragraph(text: &str) -> Paragraph {
    Paragraph::new()
        .add_run(
            Run::new()
                .add_text(plain_text(text))
                .fonts(chinese_fonts("方正小标宋简体"))
                .size(TITLE_SIZE),
        )
        .align(AlignmentType::Center)
        .line_spacing(
            LineSpacing::new()
                .after(360)
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

/// 公文主标题段，按排布方案渲染：
/// 单行保持二号；超出 1-2 字横向收窄字形（字高不变）保持单行；超出更多用 jieba
/// 换行（词不拆开、行长短均衡），各行仍用二号。调用方自行叠加行距与 keep_next。
pub(crate) fn document_title_paragraph(title: &str, plan: &TitlePlan) -> Paragraph {
    let mut paragraph = Paragraph::new().align(AlignmentType::Center);
    match plan {
        TitlePlan::SingleLine => {
            paragraph = paragraph.add_run(title_run(title, TITLE_SIZE));
        }
        TitlePlan::Compressed => {
            // 只横向缩放（w:w 字符缩放），字号保持二号、字高不变。
            let scale = title::compressed_scale_percent(title);
            paragraph = paragraph.add_run(title_run(title, TITLE_SIZE).stretch(scale as i32));
        }
        TitlePlan::Wrapped(lines) => {
            for (index, line) in lines.iter().enumerate() {
                paragraph = paragraph.add_run(title_run(line, TITLE_SIZE));
                if index + 1 < lines.len() {
                    paragraph = paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
                }
            }
        }
    }
    paragraph
}

pub(crate) fn red_approval_title_paragraph(title: &str, plan: &TitlePlan) -> Paragraph {
    let mut paragraph = Paragraph::new().align(AlignmentType::Center);
    let run = |text: &str| title_run(text, RED_APPROVAL_TITLE_SIZE);
    match plan {
        TitlePlan::SingleLine => paragraph = paragraph.add_run(run(title)),
        TitlePlan::Compressed => {
            let scale = title::compressed_scale_percent_for(
                title,
                title::RED_APPROVAL_TITLE_WIDTH_PT,
                title::RED_APPROVAL_TITLE_SIZE_PT,
            );
            paragraph = paragraph.add_run(run(title).stretch(scale as i32));
        }
        TitlePlan::Wrapped(lines) => {
            for (index, line) in lines.iter().enumerate() {
                paragraph = paragraph.add_run(run(line));
                if index + 1 < lines.len() {
                    paragraph = paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
                }
            }
        }
    }
    // 首页左栏约 10cm，右侧 5.6cm 留给批示栏。
    paragraph.indent(Some(0), None, Some(3_175), None)
}

/// 承办区一格：红色标签 + 黑色取值，整格不许换行。
///
/// 标签与取值的自然宽度超出栏宽时，两个 run 一起按同一比例横向压窄字形
/// （`w:w`，字高不变），与 LaTeX 侧的 `\RedFit` 和标题压缩同一套做法。
pub(crate) fn red_record_paragraph(
    label: &str,
    value: &str,
    alignment: AlignmentType,
    usable_twips: usize,
) -> Paragraph {
    let scale = crate::export::red_record_scale_percent(&format!("{label}{value}"), usable_twips);
    let mut label_run = Run::new()
        .add_text(label)
        .fonts(chinese_fonts("仿宋_GB2312"))
        .size(BODY_SIZE)
        .color("FF0000");
    let mut value_run = body_run(value);
    if scale < 100 {
        label_run = label_run.stretch(scale as i32);
        value_run = value_run.stretch(scale as i32);
    }
    Paragraph::new()
        .add_run(label_run)
        .add_run(value_run)
        .align(alignment)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
}

/// 图片段落：位图嵌入为 Word 图片（宽度=版心 156 mm，等比缩放，小图保持原尺寸）；
/// PDF 输出附件说明段落——docx-rs 无法把 PDF 作为图片嵌入。
pub(crate) fn image_paragraph(alt: &str, src: &str) -> Option<Paragraph> {
    let path = images::resolve(src).ok()?;
    let bytes = std::fs::read(&path).ok()?;
    let file_name = src.rsplit('/').next().unwrap_or(src).to_string();
    let is_pdf = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    image_paragraph_from_bytes(alt, &file_name, is_pdf, &bytes)
}

pub(crate) fn image_paragraph_from_bytes(
    alt: &str,
    file_name: &str,
    is_pdf: bool,
    bytes: &[u8],
) -> Option<Paragraph> {
    if is_pdf {
        let label = if alt.trim().is_empty() {
            format!("【附件】{file_name}（PDF 附件，请见原文）")
        } else {
            format!("【附件】{file_name}（{alt}，PDF 附件，请见原文）")
        };
        return Some(Paragraph::new().add_run(body_run(label)));
    }
    // 先完整解码一次：既取像素尺寸算缩放，也验证文件可被 docx-rs 的 Pic::new 转 PNG，
    // 避免 Pic::new 内部的 expect 在坏文件上 panic。
    let Ok(image) = image::load_from_memory(bytes) else {
        return None;
    };
    let (width_px, height_px) = image.dimensions();
    if width_px == 0 || height_px == 0 {
        return None;
    }
    // 版心 8844 twips = 156 mm；1 twip = 635 EMU，96 dpi 下 1 px = 9525 EMU。
    let max_width_emu = (TABLE_CONTENT_WIDTH_TWIPS as u64) * 635;
    let natural_width_emu = (width_px as u64) * 9525;
    let natural_height_emu = (height_px as u64) * 9525;
    let width_emu = natural_width_emu.min(max_width_emu);
    let height_emu = (natural_height_emu * width_emu / natural_width_emu) as u32;
    let pic = Pic::new(bytes).size(width_emu as u32, height_emu);
    Some(Paragraph::new().add_run(Run::new().add_image(pic)))
}

pub(crate) fn agenda_body_paragraph() -> Paragraph {
    Paragraph::new()
        .align(AlignmentType::Both)
        .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .widow_control(true)
}

pub(crate) fn agenda_blank_line() -> Paragraph {
    Paragraph::new()
        .line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        )
        .keep_next(true)
}

pub(crate) fn agenda_labeled_paragraph(line: &str) -> Paragraph {
    let (label, value) = line
        .split_once('：')
        .or_else(|| line.split_once(':'))
        .unwrap_or((line, ""));
    let mut paragraph = agenda_body_paragraph().add_run(heiti_run(format!("{label}：")));
    if !value.trim().is_empty() {
        for run in body_runs(value.trim()) {
            paragraph = paragraph.add_run(run);
        }
    }
    paragraph
}
