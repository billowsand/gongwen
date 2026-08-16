//! OOXML (docx) 导出：把公文按 `export::docx` 的版式写成 Word 文档。
//!
//! 各版式部件已拆分到 `export/docx/` 子模块（run/段落构建、页眉、落款、版记、
//! 红头呈批件、正文与议程），根文件保留版式常量、`write_docx` 入口与测试。

use crate::export::title;
use crate::export::title::TitlePlan;
use crate::export::{
    MarkdownBlock, MarkdownSection, attachment_names, body_heading_max_level,
    official_heading_text, parse_markdown, plain_text,
};
use crate::models::{DraftInput, StyleMode, TemplateKind, split_units};
use crate::units::UnitDisplay;
use anyhow::{Context, Result};
use docx_rs::*;
use std::fs::File;
use std::path::Path;

mod content;
mod header;
mod paragraphs;
mod record;
mod red;
mod runs;
mod signature;

pub(crate) use content::{add_official_content_block, add_smart_table, write_meeting_agenda_docx};
pub(crate) use header::{add_official_page_footers, issuing_unit_header};
#[cfg(test)]
pub(crate) use paragraphs::image_paragraph_from_bytes;
pub(crate) use paragraphs::{
    agenda_blank_line, agenda_body_paragraph, agenda_labeled_paragraph,
    attachment_document_title_paragraph, attachment_label_paragraph, body_paragraph,
    compact_heading_paragraph, document_title_paragraph, heading_paragraph, image_paragraph,
    joint_closing_paragraph, joint_signature_cell_paragraph, label_paragraph,
    letter_security_paragraph, ordered_list_paragraph, red_approval_title_paragraph,
    red_record_paragraph,
};
pub(crate) use record::add_footer_record;
pub(crate) use red::{
    red_approval_frame_table, red_approval_record_table, red_approval_top_rule_table,
};
pub(crate) use runs::{
    body_run, body_runs, chinese_fonts, docx_name, heiti_run, record_run, security_runs,
    spread_runs, table_run_sized, table_runs_sized, title_run,
};
pub(crate) use signature::{
    add_attachment_summary, add_joint_signature, add_white_paper_signature, is_joint_mode_one,
    main_issuing_unit, official_document_number, official_signature_date,
};

const BODY_SIZE: usize = 32; // 16 pt，OOXML 使用半磅
const TITLE_SIZE: usize = 44; // 22 pt，二号
const RED_APPROVAL_TITLE_SIZE: usize = 36; // 18 pt，小二号
pub(super) const TABLE_SIZE: usize = 28; // 14 pt，四号
const FOOTER_SIZE: usize = 28; // 14 pt，四号；版记字号独立固定，不随正文表格调整
const PAGE_NUMBER_SIZE: usize = 36; // 18 pt，四号
/// 正文、附件概要或“此页无正文”与落款之间通常空 3 行（每行固定 560 缇）。
const CLOSING_GAP_TWIPS: u32 = 3 * 560;
/// 正文中完整括号（全角/半角）及其中内容的字号：14 pt，四号，比正文小一号。
const PAREN_SIZE: usize = 28;
pub(super) const TABLE_CONTENT_WIDTH_TWIPS: usize = 8_844; // 156 mm 版心
/// 版记一个 em 的宽度（twips）：四号 14 pt，1 pt = 20 twips。
const FOOTER_EM_TWIPS: usize = FOOTER_SIZE * 10;
/// 联系电话列宽：5 个全角字标签 + 11 位半角数字（5.5em）+ 0.5em 余量。
const RECORD_PHONE_COLUMN_TWIPS: usize = 11 * FOOTER_EM_TWIPS;
/// 承办单位、联系人两列均分剩余宽度。
const RECORD_OTHER_COLUMN_TWIPS: usize =
    (TABLE_CONTENT_WIDTH_TWIPS - RECORD_PHONE_COLUMN_TWIPS) / 2;
const AGENDA_NUMBERING_ID: usize = 17;
const JOINT_SIGNATURE_SEAL_GAP_TWIPS: f32 = 2_551.0; // 45 mm 公章安全高度

/// 红头（发文机关标志）可用的版心宽度：A4 页宽减去左右页边距，约 15.6cm。
const HEADER_WIDTH_TWIPS: i32 = 11906 - 1587 - 1474;
/// 红头字号：二号（24 pt）。
const HEADER_SIZE: usize = 48;
/// 字数过多时允许缩到的最小字号：三号（16 pt），再小就不合公文规范。
const HEADER_MIN_SIZE: usize = 32;

/// 规格 §3.3：预览版所有占位区域统一 1em 宽，用一个全角空格表示。
const PREVIEW_PLACEHOLDER: &str = "\u{2003}";

pub fn write_docx(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
) -> Result<()> {
    if input.kind == TemplateKind::MeetingAgenda {
        return write_meeting_agenda_docx(path, input, markdown);
    }

    let blocks = parse_markdown(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());

    let mut doc = Docx::new()
        .page_size(11906, 16838)
        .page_margin(PageMargin {
            top: 2098,
            bottom: 1984,
            left: 1587,
            right: 1474,
            header: 567,
            footer: 567,
            gutter: 0,
        })
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(BODY_SIZE)
        .default_line_spacing(
            LineSpacing::new()
                .line(560)
                .line_rule(LineSpacingType::Exact),
        );

    if input.kind.uses_letter_layout() {
        doc = add_official_page_footers(doc, input.profile.duplex_printing);
        if input.kind == TemplateKind::RedHeadApproval {
            // 即使不标密也保留密级行的垂直槽位，使红头稳定落在参考稿约 67mm
            // 的位置；有密级时就在该槽位显示，不改变后续元素坐标。
            doc = doc.add_paragraph(
                letter_security_paragraph(input).line_spacing(
                    LineSpacing::new()
                        .line(560)
                        .line_rule(LineSpacingType::Exact)
                        .after(560),
                ),
            );
        } else if !input.profile.security_level.trim().is_empty() {
            doc = doc.add_paragraph(letter_security_paragraph(input));
        }
        if input.kind != TemplateKind::PlainDocument {
            let unit = main_issuing_unit(input, display);
            if !unit.is_empty() {
                let layout = issuing_unit_header(&unit);
                doc = doc.add_paragraph(
                    Paragraph::new()
                        .add_run(
                            Run::new()
                                .add_text(&unit)
                                .fonts(chinese_fonts("方正小标宋简体"))
                                .size(layout.size)
                                .color("C00000"),
                        )
                        // 分散对齐让 Word 自己把字距均匀撑开；缩进决定撑开的范围。
                        .align(AlignmentType::Distribute)
                        .indent(
                            Some(layout.side_indent),
                            None,
                            Some(layout.side_indent),
                            None,
                        )
                        .line_spacing(LineSpacing::new().after(180)),
                );
            }
        }
        if input.kind.has_document_number()
            && let Some(number) = official_document_number(input)
        {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run(number))
                    .align(AlignmentType::Center)
                    .line_spacing(LineSpacing::new().after(240)),
            );
        }
        if input.kind == TemplateKind::RedHeadApproval {
            doc = doc
                .add_table(red_approval_top_rule_table())
                .add_table(red_approval_frame_table(input))
                .add_table(red_approval_record_table(input, display));
        }
    }

    let title_plain = plain_text(title);
    let title_capacity = if input.kind == TemplateKind::RedHeadApproval {
        title::red_approval_chars_per_line()
    } else {
        title::chars_per_line()
    };
    let plan = title::title_plan(&title_plain, title_capacity);
    let title_paragraph = if input.kind == TemplateKind::RedHeadApproval {
        red_approval_title_paragraph(title, &plan)
    } else {
        document_title_paragraph(title, &plan)
    };
    let title_before = if input.kind == TemplateKind::RedHeadApproval {
        480
    } else {
        120
    };
    // 红头呈批件首页版面：正文可用行数、正文是否跨页、表格图片是否要被赶出首页，
    // 三端共用 export::red_approval_* 的同一套估算，避免各端判据不一致。
    let red_wrap_lines = crate::export::red_approval_wrap_lines(
        match &plan {
            TitlePlan::Wrapped(lines) => lines.len().max(1),
            _ => 1,
        },
        crate::models::joint_responsible_entries(&input.profile)
            .len()
            .max(1),
    );
    let red_body = crate::export::red_approval_body_metrics(&blocks);
    let mut red_float_break_pending = input.kind == TemplateKind::RedHeadApproval
        && red_body.float_needs_page_break(red_wrap_lines);
    doc = doc.add_paragraph(
        title_paragraph
            .line_spacing(
                LineSpacing::new()
                    .before(title_before)
                    .after(360)
                    .line(560)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true),
    );

    let addressee = match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => display.join_hierarchical_for(
            &split_units(&input.profile.recipient),
            input.uses_external_unit_names(),
        ),
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            display.reporting_leaders(&input.profile.reporting_leaders)
        }
        TemplateKind::PlainDocument | TemplateKind::MeetingAgenda => String::new(),
    };
    if !addressee.is_empty() && !markdown.contains(addressee.as_str()) {
        doc = doc.add_paragraph(label_paragraph(&format!(
            "{}：",
            addressee.trim_end_matches('：')
        )));
    }

    let mut attachment_blocks = Vec::new();
    if input.kind.uses_letter_layout() {
        let mut in_attachment = false;
        let mut seen_document_title = false;
        let mut counters = [0usize; 4];
        let compact = input.profile.style_mode == StyleMode::Compact;
        // 紧缩风格合并正文区 # 号最多的那一级标题；附件区标题不计入。
        let compact_heading_level = body_heading_max_level(&blocks);
        let mut index = 0usize;
        while index < blocks.len() {
            let block = &blocks[index];
            match block {
                MarkdownBlock::Title(_) if !seen_document_title && !in_attachment => {
                    seen_document_title = true;
                }
                MarkdownBlock::Marker(section) => {
                    in_attachment = matches!(section, MarkdownSection::Attachment);
                    counters = [0; 4];
                    if in_attachment {
                        attachment_blocks.push(block);
                    }
                }
                _ if in_attachment => attachment_blocks.push(block),
                _ => {
                    // 紧缩风格：正文区 # 号最多的那一级标题后紧跟正文段落时合并为一行。
                    let next_is_paragraph = blocks.get(index + 1).is_some_and(|next| {
                        matches!(next, MarkdownBlock::Paragraph(text)
                            if !text.trim().is_empty()
                                && !text.contains("<div")
                                && !text.contains("</div"))
                    });
                    if compact
                        && let MarkdownBlock::Heading(level, heading) = block
                        && *level == compact_heading_level
                        && next_is_paragraph
                    {
                        if let Some(title) = official_heading_text(*level, heading, &mut counters) {
                            let MarkdownBlock::Paragraph(body) = &blocks[index + 1] else {
                                unreachable!()
                            };
                            doc =
                                doc.add_paragraph(compact_heading_paragraph(*level, &title, body));
                        }
                        index += 1; // 跳过紧随的正文段落
                    } else {
                        // 红头呈批件首页右侧是批示栏，表格和图片按整幅版心排版会压
                        // 过去；正文区第一个表格/图片若落在首页就先换页。
                        if red_float_break_pending
                            && matches!(
                                block,
                                MarkdownBlock::Table { .. } | MarkdownBlock::Image { .. }
                            )
                        {
                            red_float_break_pending = false;
                            doc = doc.add_paragraph(
                                Paragraph::new().page_break_before(true).line_spacing(
                                    LineSpacing::new()
                                        .line(560)
                                        .line_rule(LineSpacingType::Exact),
                                ),
                            );
                        }
                        doc = add_official_content_block(doc, block, &mut counters);
                    }
                }
            }
            index += 1;
        }
    } else {
        for block in &blocks {
            match block {
                MarkdownBlock::Title(_) | MarkdownBlock::Html(_) | MarkdownBlock::Marker(_) => {}
                MarkdownBlock::Image { alt, src } => {
                    if let Some(paragraph) = image_paragraph(alt, src) {
                        doc = doc.add_paragraph(paragraph);
                    }
                }
                MarkdownBlock::Heading(level, text) => {
                    doc = doc.add_paragraph(heading_paragraph(*level, text));
                }
                MarkdownBlock::Paragraph(text) => {
                    if text.trim().is_empty() || text.contains("<div") || text.contains("</div") {
                        continue;
                    }
                    doc = doc.add_paragraph(body_paragraph(text));
                }
                MarkdownBlock::ListItem(text) => {
                    doc = doc.add_paragraph(label_paragraph(text).indent(
                        Some(420),
                        None,
                        None,
                        None,
                    ));
                }
                MarkdownBlock::OrderedListItem { number, text } => {
                    doc = doc.add_paragraph(ordered_list_paragraph(*number, text));
                }
                MarkdownBlock::Table { rows, aligns } => doc = add_smart_table(doc, rows, aligns),
            }
        }
    }

    // 附件概要：正文结束后、落款之前列出附件名称（红头呈批件同样支持）。
    if input.kind.uses_letter_layout() {
        let names = attachment_names(&blocks);
        if !names.is_empty() {
            doc = add_attachment_summary(doc, &names);
        }
    }

    if crate::models::is_joint_signature(input) {
        doc = add_joint_signature(doc, input, display);
    } else if input.kind == TemplateKind::WhitePaper {
        doc = add_white_paper_signature(doc, input, display, 0);
    } else if input.kind == TemplateKind::RedHeadApproval {
        // 落款最早从第二页开始。正文只有首页那点内容时另起一页标「（此页无正文）」；
        // 正文本来就跨页时不再额外制造空白页，落款接在正文之后。
        if !red_body.reaches_second_page(red_wrap_lines) {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run("（此页无正文）"))
                    .indent(Some(640), None, None, None)
                    .page_break_before(true)
                    .line_spacing(
                        LineSpacing::new()
                            .line(560)
                            .line_rule(LineSpacingType::Exact),
                    ),
            );
        }
        doc = add_white_paper_signature(doc, input, display, crate::export::SIGNATURE_ROOM_TWIPS);
    } else if matches!(
        input.kind,
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice
    ) {
        let raw_signature = if input.profile.signing_unit.trim().is_empty() {
            if is_joint_mode_one(input) {
                // 联合发文模式 1 只剩 1 个发文单位：回落右侧落款，单位取该唯一发文单位。
                split_units(&input.profile.joint_issuing_units)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| input.profile.issuing_unit.trim().to_string())
            } else {
                input.profile.issuing_unit.trim().to_string()
            }
        } else {
            input.profile.signing_unit.trim().to_string()
        };
        // 规格 §3.1：公函落款显示全称；电话通知落款显示简称（少于 5 字逐字加空格）。
        let signature = if input.kind == TemplateKind::PhoneNotice {
            display.abbr_spaced(&raw_signature)
        } else {
            display.full_name_for(&raw_signature, input.uses_external_unit_names())
        };
        if !signature.is_empty() {
            // 代章直接跟在落款单位后面同一行（如“星海省教育厅（代章）”），不另起一行。
            // 是否标注只由 seals_on_behalf 决定（仅公函；电话通知等其他文种不盖章）。
            let unit = if crate::export::seals_on_behalf(input, display) {
                format!("{signature}（代章）")
            } else {
                signature
            };
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run(unit))
                    .align(AlignmentType::Right)
                    .line_spacing(
                        LineSpacing::new()
                            .before(CLOSING_GAP_TWIPS)
                            .line(560)
                            .line_rule(LineSpacingType::Exact),
                    ),
            );
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run(official_signature_date(input)))
                    .align(AlignmentType::Right)
                    .line_spacing(
                        LineSpacing::new()
                            .line(560)
                            .line_rule(LineSpacingType::Exact),
                    ),
            );
        }
    }

    if input.kind.uses_letter_layout() && !attachment_blocks.is_empty() {
        doc = doc.add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));
        let mut counters = [0usize; 4];
        let attachment_count = attachment_blocks
            .iter()
            .filter(|block| matches!(block, MarkdownBlock::Marker(MarkdownSection::Attachment)))
            .count();
        let mut attachment_index = 0usize;
        for block in attachment_blocks {
            match block {
                MarkdownBlock::Marker(MarkdownSection::Attachment) => {
                    if attachment_index > 0 {
                        doc = doc.add_paragraph(
                            Paragraph::new().add_run(Run::new().add_break(BreakType::Page)),
                        );
                    }
                    attachment_index += 1;
                    counters = [0; 4];
                    let label = if attachment_count == 1 {
                        "附件".to_string()
                    } else {
                        format!("附件{attachment_index}")
                    };
                    doc = doc.add_paragraph(attachment_label_paragraph(&label));
                }
                MarkdownBlock::Title(text) => {
                    counters = [0; 4];
                    doc = doc.add_paragraph(attachment_document_title_paragraph(text));
                }
                _ => doc = add_official_content_block(doc, block, &mut counters),
            }
        }
    }

    if input.kind == TemplateKind::OfficialLetter {
        doc = add_footer_record(doc, input, display);
    }

    let file =
        File::create(path).with_context(|| format!("无法创建 Word 文件：{}", path.display()))?;
    doc.build().pack(file).context("写入 DOCX 包失败")?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::models::{
        JointContact, JointIssuanceMode, LetterVersion, VocabularyCategory, VocabularyEntry,
    };
    use regex::Regex;
    use std::io::Read;

    /// 测试大多使用无层级的扁平单位，空词库让 `UnitDisplay` 回落为规范名称。
    fn write_docx_ok(path: &Path, input: &DraftInput, markdown: &str) -> Result<()> {
        write_docx(path, input, markdown, &UnitDisplay::new(&[]))
    }

    /// 动态生成一张 1x1 红色 PNG 的字节。
    fn tiny_png() -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        cursor.into_inner()
    }

    #[test]
    fn image_paragraph_embeds_bitmap_and_docx_contains_media() {
        let temp = tempfile::tempdir().unwrap();
        let paragraph = image_paragraph_from_bytes("示意图", "示意图.png", false, &tiny_png())
            .expect("有效 PNG 应生成图片段落");
        let mut doc = Docx::new();
        doc = doc.add_paragraph(paragraph);
        let docx_path = temp.path().join("with-image.docx");
        doc.build().pack(File::create(&docx_path).unwrap()).unwrap();
        let file = File::open(&docx_path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        let media: Vec<String> = archive
            .file_names()
            .filter(|name| name.starts_with("word/media/"))
            .map(str::to_string)
            .collect();
        assert!(!media.is_empty(), "docx 应含 word/media/ 图片文件");
        let xml = zip_text(&docx_path, "word/document.xml");
        assert!(xml.contains("<w:drawing>"), "图片段落应含 drawing 元素");
    }

    #[test]
    fn image_paragraph_renders_pdf_as_attachment_note() {
        let temp = tempfile::tempdir().unwrap();
        let paragraph = image_paragraph_from_bytes("", "扫描件.pdf", true, b"%PDF-1.4")
            .expect("PDF 应生成附件说明段落");
        let mut doc = Docx::new();
        doc = doc.add_paragraph(paragraph);
        let docx_path = temp.path().join("with-pdf.docx");
        doc.build().pack(File::create(&docx_path).unwrap()).unwrap();
        let xml = zip_text(&docx_path, "word/document.xml");
        assert!(xml.contains("【附件】扫描件.pdf"));
        // PDF 不产生媒体文件。
        let file = File::open(&docx_path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();
        assert!(
            archive
                .file_names()
                .all(|name| !name.starts_with("word/media/")),
            "PDF 附件说明不应产生媒体文件"
        );
    }

    #[test]
    fn image_paragraph_ignores_undecodable_bytes() {
        assert!(
            image_paragraph_from_bytes("", "坏图.png", false, b"not an image").is_none(),
            "无法解码的字节应跳过而不是 panic"
        );
    }

    #[test]
    fn image_paragraph_skips_unresolvable_src() {
        // 非法路径（穿越）解析失败时跳过，不 panic。
        assert!(image_paragraph("", "../etc/passwd").is_none());
    }

    #[test]
    fn short_issuing_unit_keeps_its_size_and_stays_centered() {
        let layout = issuing_unit_header("国务院");
        assert_eq!(layout.size, HEADER_SIZE, "字数少时不得改变字号");
        // 三个字加两个一字宽的字距，居中摆放。
        let block = 3 * 480 + 2 * 480;
        assert_eq!(layout.side_indent, (HEADER_WIDTH_TWIPS - block) / 2);
        assert!(layout.side_indent > 0, "短名称应留出左右缩进而不是铺满版心");
    }

    #[test]
    fn medium_issuing_unit_fills_the_line_without_shrinking() {
        // 10 个字：一字宽的字距已经超出版心，因此正好撑满，字号不变。
        let layout = issuing_unit_header("中华人民共和国教育部");
        assert_eq!(layout.size, HEADER_SIZE);
        assert_eq!(layout.side_indent, 0);
    }

    #[test]
    fn long_issuing_unit_shrinks_instead_of_being_squashed() {
        let unit = "某某省人民政府政务服务和数字化建设管理局办公室综合处";
        let layout = issuing_unit_header(unit);
        assert!(layout.size < HEADER_SIZE, "排不下时应缩小字号");
        assert!(layout.size >= HEADER_MIN_SIZE, "不得小于三号");
        assert_eq!(layout.side_indent, 0);
        let count = unit.chars().count() as i32;
        assert!(
            count * layout.size as i32 * 10 <= HEADER_WIDTH_TWIPS,
            "缩小后必须能排进版心"
        );
    }

    #[test]
    fn very_long_issuing_unit_stops_at_the_minimum_size() {
        let layout = issuing_unit_header(&"某".repeat(60));
        assert_eq!(layout.size, HEADER_MIN_SIZE);
    }

    #[test]
    fn single_character_issuing_unit_is_centered() {
        let layout = issuing_unit_header("函");
        assert_eq!(layout.size, HEADER_SIZE);
        assert_eq!(layout.side_indent, (HEADER_WIDTH_TWIPS - 480) / 2);
    }

    #[test]
    fn hierarchical_units_expand_to_full_names_in_header_and_addressee() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hierarchy.docx");
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "新闻舆论处".into();
        input.profile.recipient = "新闻舆论处、信访接待处".into();
        input.profile.responsible_unit = "新闻舆论处".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "中央网信办".into(),
                category: VocabularyCategory::Unit,
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "新闻舆论处".into(),
                category: VocabularyCategory::Unit,
                parent: "中央网信办".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "信访接待处".into(),
                category: VocabularyCategory::Unit,
                parent: "中央网信办".into(),
                ..Default::default()
            },
        ];
        let display = UnitDisplay::new(&vocabulary);
        write_docx(&path, &input, "# 测试函\n\n正文。", &display).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 规格 §2.2：发文单位红头补全上级全称。
        assert!(xml.contains("中央网信办新闻舆论处"), "红头应补全上级全称");
        // 同属一个上级的主送单位：顿号连接且不重复上级。
        assert!(xml.contains("中央网信办新闻舆论处、信访接待处"));
        // 规格 §3.1：版记承办单位显示简称（无简称时回落规范名称）。
        assert!(xml.contains("承办单位：新闻舆论处"));
    }

    #[test]
    fn external_letter_uses_external_names_but_keeps_responsible_abbr() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("external.docx");
        let mut input = DraftInput::default();
        input.profile.correspondence_scope = crate::models::CorrespondenceScope::External;
        input.profile.issuing_unit = "新闻舆论处".into();
        input.profile.recipient = "信访接待处".into();
        input.profile.responsible_unit = "新闻舆论处".into();
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "中央网信办".into(),
                external_name: "国家互联网信息办公室".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "新闻舆论处".into(),
                external_name: "新闻传播管理处".into(),
                parent: "中央网信办".into(),
                abbr: "新舆处".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "信访接待处".into(),
                external_name: "公众服务处".into(),
                parent: "中央网信办".into(),
                ..Default::default()
            },
        ];
        write_docx(
            &path,
            &input,
            "# 外部函测试\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("国家互联网信息办公室新闻传播管理处"));
        assert!(xml.contains("国家互联网信息办公室公众服务处"));
        assert!(xml.contains("承办单位：新舆处"));
    }

    #[test]
    fn red_header_is_written_as_distributed_text_not_stretched_glyphs() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("letter.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "国务院".into();
        input.profile.recipient = "某某市教育局".into();
        write_docx_ok(&path, &input, "# 关于测试红头的函\n\n正文。\n").unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(
            xml.contains(r#"w:val="distribute""#),
            "红头应使用分散对齐，由 Word 均匀撑开字距"
        );
        let indent = (HEADER_WIDTH_TWIPS - (3 * 480 + 2 * 480)) / 2;
        assert!(
            xml.contains(&format!(r#"w:left="{indent}""#)),
            "短名称应靠左右缩进居中，实际 XML：{xml}"
        );
        assert!(
            xml.contains(&format!(r#"w:val="{HEADER_SIZE}""#)),
            "字数少时不得缩小字号"
        );
    }

    #[test]
    fn official_letter_docx_numbers_headings_and_starts_attachments_on_a_new_page() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("letter-with-attachment.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n<!-- [正文] -->\n## 一、总体要求\n### （一）具体事项\n正文。\n<!-- [附件] -->\n# 附件1\n## 统计表\n### 一、填报说明\n附件内容。\n# 附件2\n## 说明材料\n附件二内容。",
        )
        .unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("一、总体要求"));
        assert!(xml.contains("（一）具体事项"));
        assert!(xml.contains("附件1"));
        assert!(xml.contains("统计表"));
        assert!(xml.contains("附件2"));
        assert_eq!(xml.matches("一、填报说明").count(), 1);
        assert_eq!(xml.matches(r#"w:type="page""#).count(), 2, "{xml}");
        // 附件概要：正文结束后、落款前按顺序列出附件名称，与正文空两行、首行缩进两个汉字。
        assert!(xml.contains("附件1：统计表"), "{xml}");
        assert!(xml.contains("　　2：说明材料"), "{xml}");
        let summary = paragraph_containing(&xml, "附件1：统计表");
        assert!(summary.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(!summary.contains("w:eastAsia=\"黑体\""));
        assert!(summary.contains(r#"w:firstLine="640""#), "{summary}");
        let gap = gap_between(&xml, "正文。", "附件1：统计表");
        assert_eq!(gap.matches("<w:p ").count(), 2, "概要前应空两行：{gap}");
        assert!(!gap.contains("<w:t"));
        let after_summary = &xml[xml.find("附件1：统计表").unwrap()..];
        let signature = paragraph_containing(after_summary, "某单位");
        assert!(
            signature.contains(&format!(r#"w:before="{CLOSING_GAP_TWIPS}""#)),
            "附件概要与落款之间通常应空 3 行：{signature}"
        );
        let body_heading = paragraph_containing(&xml, "一、总体要求");
        assert!(body_heading.contains(r#"w:firstLine="640""#));
        // 附件区真正的“附件1”黑体标签与“统计表”小标宋标题均位于概要之后。
        let after_summary = &xml[xml.find("附件1：统计表").unwrap() + "附件1：统计表".len()..];
        let attachment_label = paragraph_containing(after_summary, "附件1");
        assert!(attachment_label.contains("w:eastAsia=\"黑体\""));
        assert!(attachment_label.contains("w:ascii=\"SimHei\""));
        assert!(attachment_label.contains("w:hAnsi=\"SimHei\""));
        assert!(attachment_label.contains("w:val=\"left\""));
        assert!(!attachment_label.contains("w:firstLine"));
        let attachment_title = paragraph_containing(after_summary, "统计表");
        assert!(attachment_title.contains("w:eastAsia=\"方正小标宋简体\""));
        assert!(attachment_title.contains("w:val=\"center\""));
        let attachment_heading = paragraph_containing(&xml, "一、填报说明");
        assert!(attachment_heading.contains(r#"w:firstLine="640""#));
        assert!(!xml.contains("一、一、"));
    }

    #[test]
    fn official_letter_preview_masks_serial_and_day_but_formal_keeps_them() {
        let temp = tempfile::tempdir().unwrap();
        let formal_path = temp.path().join("formal.docx");
        let preview_path = temp.path().join("preview.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.department_code = "某政函".into();
        input.profile.document_number = "12".into();
        input.date = "2026年8月5日".into();

        write_docx_ok(&formal_path, &input, "# 测试函\n\n正文。").unwrap();
        let formal = zip_text(&formal_path, "word/document.xml");
        assert!(formal.contains("某政函〔2026〕12号"));
        assert!(formal.contains("2026年8月5日"));

        input.profile.letter_version = LetterVersion::Preview;
        write_docx_ok(&preview_path, &input, "# 测试函\n\n正文。").unwrap();
        let preview = zip_text(&preview_path, "word/document.xml");
        // 规格 §3.3：预览版占位统一 1em。
        assert!(preview.contains("某政函〔2026〕\u{2003}号"));
        assert!(preview.contains("2026年8月\u{2003}日"));
        assert!(!preview.contains("某政函〔2026〕12号"));
        assert!(!preview.contains("2026年8月5日"));
    }

    #[test]
    fn phone_notice_keeps_letter_layout_but_omits_number_and_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("phone-notice.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PhoneNotice;
        input.profile.issuing_unit = "某某省教育厅".into();
        input.profile.department_code = "某教函".into();
        input.profile.document_number = "12".into();
        input.profile.recipient = "某某市教育局".into();
        input.profile.copies_to = "某某市财政局".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-12345678".into();
        write_docx_ok(
            &path,
            &input,
            "# 关于测试事项的电话通知\n<!-- [正文] -->\n## 工作要求\n正文。\n<!-- [附件] -->\n# 附件标题\n附件内容。",
        )
        .unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("某某省教育厅"));
        assert!(xml.contains("某某市教育局"));
        assert!(xml.contains("一、工作要求"));
        assert!(xml.contains("附件"));
        assert!(!xml.contains("某教函"));
        assert!(!xml.contains("〔"));
        assert!(!xml.contains("抄送："));
        assert!(!xml.contains("承办单位："));
        assert!(!xml.contains("张三"));
        assert!(!xml.contains("010-12345678"));
    }

    #[test]
    fn phone_notice_signature_uses_spaced_abbreviation() {
        // 规格 §3.1：电话通知落款显示简称，少于 5 字时逐字加半角空格。
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("phone-notice-abbr.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PhoneNotice;
        input.profile.issuing_unit = "中央宣传部".into();
        let vocabulary = vec![VocabularyEntry {
            canonical: "中央宣传部".into(),
            category: VocabularyCategory::Unit,
            abbr: "中宣部".into(),
            ..Default::default()
        }];
        let display = UnitDisplay::new(&vocabulary);
        write_docx(&path, &input, "# 电话通知\n\n正文。", &display).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 红头保留全称，落款输出“中 宣 部”（简称逐字加半角空格）。
        assert!(xml.contains("中央宣传部"), "红头应保留全称");
        let signature = paragraph_containing(&xml, "中 宣 部");
        assert!(signature.contains("w:val=\"right\""), "落款应右对齐");
        assert!(!xml.contains("中宣部"), "不得输出未加空格的简称");
    }

    #[test]
    fn white_paper_signature_stacks_multiple_units_right_aligned_with_spread() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("white-paper-multi.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.signing_unit = "星海省教育厅、教师工作处".into();
        input.profile.use_short_name_for_signature = true;
        input.date = "2026年8月7日".into();
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "星海省教育厅".into(),
                category: VocabularyCategory::Unit,
                abbr: "省教育厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "教师工作处".into(),
                category: VocabularyCategory::Unit,
                abbr: "教师处".into(),
                ..Default::default()
            },
        ];
        let display = UnitDisplay::new(&vocabulary);
        write_docx(&path, &input, "# 标题\n\n正文。", &display).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 两个单位分行列出；简称逐字成 run，字间距 320 缇（1em，三号 16pt 下分散到 5 字宽）。
        for ch in ["省", "教", "厅", "教", "师", "处"] {
            assert!(
                xml.contains(&format!("<w:t xml:space=\"preserve\">{ch}</w:t>")),
                "应含“{ch}”：{xml}"
            );
        }
        assert!(
            xml.contains("w:spacing w:val=\"320\""),
            "3 字简称应有 1em 字符间距：{xml}"
        );
        // 单位与日期段落都右对齐。
        assert!(xml.contains("w:val=\"right\""), "落款应右对齐：{xml}");
        // 不出现未分散的整串简称。
        assert!(!xml.contains("省教育厅"), "简称不应整串出现：{xml}");
    }

    #[test]
    fn white_paper_signature_single_unit_keeps_full_name() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("white-paper-single.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.signing_unit = "星海省教育厅".into();
        input.date = "2026年8月7日".into();
        write_docx_ok(&path, &input, "# 标题\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(
            xml.contains("星海省教育厅"),
            "单单位未选简称应输出全称：{xml}"
        );
    }

    #[test]
    fn official_letter_seal_on_behalf_follows_the_unit_on_the_same_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("seal-on-behalf.docx");
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "星海省教育厅".into();
        input.date = "2026年8月7日".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(!xml.contains("（代章）"), "单位未配置时不得标注代章");

        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "星海省教育厅".into(),
            seal_on_behalf: true,
            ..Default::default()
        }];
        write_docx(
            &path,
            &input,
            "# 测试函\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 落款单位出现在红头与落款两处，最后一次出现是落款；“（代章）”直接跟在
        // 落款单位后面同一行（“星海省教育厅（代章）”），成文日期在其后另起一行。
        let unit_pos = xml.rfind("星海省教育厅").expect("落款单位");
        let seal_pos = xml.find("（代章）").expect("代章标注");
        let date_pos = xml.find("2026年8月7日").expect("成文日期");
        assert!(unit_pos < seal_pos && seal_pos < date_pos);
        assert!(
            xml.contains("星海省教育厅（代章）"),
            "代章应与落款单位同一行：{xml}"
        );
        let seal = paragraph_containing(&xml, "（代章）");
        assert!(seal.contains("w:val=\"right\""), "代章应与落款同侧对齐");
    }

    #[test]
    fn plain_document_keeps_body_and_attachments_but_omits_all_decorative_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plain-document.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PlainDocument;
        input.profile.issuing_unit = "不应出现的发文单位".into();
        input.profile.recipient = "不应出现的主送单位".into();
        input.profile.signing_unit = "不应出现的落款单位".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        input.date = "2026年8月7日".into();
        write_docx_ok(
            &path,
            &input,
            "# 普通公文测试\n<!-- [正文] -->\n## 工作要求\n正文内容。\n<!-- [附件] -->\n# 附件1\n## 附件标题\n附件内容。",
        )
        .unwrap();

        let xml = zip_text(&path, "word/document.xml");
        assert!(runs_text(&xml).contains("秘密★10年"));
        assert!(xml.contains("普通公文测试"));
        assert!(xml.contains("一、工作要求"));
        assert!(xml.contains("正文内容。"));
        assert!(xml.contains("附件：附件标题"));
        assert!(xml.contains("附件内容。"));
        assert!(!xml.contains("不应出现的发文单位"));
        assert!(!xml.contains("不应出现的主送单位"));
        assert!(!xml.contains("不应出现的落款单位"));
        assert!(!xml.contains("2026年8月7日"));
        assert!(!xml.contains("指人专办"));
        assert!(!xml.contains("C00000"), "普通公文不得出现红头颜色");
    }

    #[test]
    fn joint_mode_one_uses_main_header_two_column_signatures_and_multiline_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("joint-letter.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        input.profile.department_code = "乙函".into();
        input.profile.document_number = "8".into();
        input.profile.recipient = "收文单位".into();
        input.profile.copies_to = "丁单位、戊单位".into();
        input.profile.joint_responsible_units = "甲处室、乙处室".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "甲处室".into(),
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact {
                unit: "乙处室".into(),
                name: "李四".into(),
                phone: "010-22222222".into(),
            },
        ];
        input.date = "2026年8月5日".into();
        write_docx_ok(&path, &input, "# 联合发文测试函\n\n正文。").unwrap();

        let xml = zip_text(&path, "word/document.xml");
        let header = paragraph_containing(&xml, "乙单位");
        assert!(header.contains("C00000"), "主发文单位应写入红头：{header}");
        assert!(!paragraph_containing(&xml, "乙单位").contains("甲单位、乙单位、丙单位"));
        assert!(xml.contains("甲单位"));
        assert!(xml.contains("丙单位"));
        assert!(xml.contains(r#"w:trHeight w:val="2551""#));
        // 规格 §2.5：三个单位时最后一个单位跨两列居中。
        assert!(xml.contains(r#"w:gridSpan w:val="2""#));
        assert!(xml.contains("2026年8月5日"));
        // 日期压在主发文单位“乙单位”（右列）所在单元格，而不是整行居中。
        let date_at = xml.find("2026年8月5日").unwrap();
        let cell_start = xml[..date_at].rfind("<w:tc>").unwrap();
        let cell_end = xml[date_at..].find("</w:tc>").unwrap() + date_at + "</w:tc>".len();
        let row_start = xml[..cell_start].rfind("<w:tr>").unwrap();
        let row_end = xml[cell_end..].find("</w:tr>").unwrap() + cell_end + "</w:tr>".len();
        let closing_row = &xml[row_start..row_end];
        assert_eq!(closing_row.matches("<w:tc>").count(), 2, "收尾行应有两列");
        let first_cell_end = closing_row.find("</w:tc>").unwrap() + "</w:tc>".len();
        assert!(
            closing_row[first_cell_end..].contains("2026年8月5日"),
            "日期应在主发文单位（右列）单元格内"
        );
        assert_eq!(xml.matches("承办单位：").count(), 1);
        assert_eq!(xml.matches("联系人：").count(), 1);
        assert_eq!(xml.matches("联系电话：").count(), 1);
        // 联系电话列预留 11em（标签 5em + 11 位半角数字 5.5em + 0.5em 余量），
        // 其余两列均分剩余宽度，11 位电话号码不再换行。
        assert_eq!(xml.matches(r#"w:gridCol w:w="2882""#).count(), 2);
        assert_eq!(xml.matches(r#"w:gridCol w:w="3080""#).count(), 1);
        assert!(
            xml.contains(r#"w:tcW w:w="3080""#),
            "联系电话单元格宽度应能容纳 11 位数字"
        );
        assert!(xml.contains(r#"w:tab w:val="right" w:pos="8844""#));
        assert!(xml.contains("（共印5份）"));
        // 版记首行抄送跨三列，单元格带 gridSpan=3；回行时用悬挂缩进对齐单位名称。
        assert!(xml.contains(r#"w:gridSpan w:val="3""#));
        assert!(paragraph_containing(&xml, "抄送：").contains(r#"w:hanging="840""#));
        // 规格 §3.2：第 2 行起承办单位、联系人用左缩进对齐第一行标签后的位置。
        assert!(xml.contains(r#"w:left="1400""#));
        assert!(xml.contains(r#"w:left="1120""#));
        assert!(paragraph_containing(&xml, "承办单位：").contains(r#"w:sz w:val="28""#));
        assert!(paragraph_containing(&xml, "（共印5份）").contains(r#"w:sz w:val="28""#));
        assert!(xml.contains("甲处室"));
        assert!(xml.contains("乙处室"));
        // 规格 §3.2：2 字姓名中间加全角空格，占 3 字宽。
        assert!(xml.contains("张\u{2003}三"));
        assert!(xml.contains("李\u{2003}四"));
        assert!(xml.contains("010-11111111"));
        assert!(xml.contains("010-22222222"));
        // 版记行高必须交给 Word 自动撑开：承办单位没维护简称时会回落较长的
        // 规范全称，固定行高会把折行后的第二行直接裁掉。
        let record_at = xml.find("承办单位：").unwrap();
        let record_row_start = xml[..record_at].rfind("<w:tr>").unwrap();
        assert!(
            !xml[record_row_start..record_at].contains("hRule=\"exact\""),
            "联合发文版记行不得使用固定行高：{}",
            &xml[record_row_start..record_at]
        );
    }

    #[test]
    fn joint_mode_one_single_unit_falls_back_to_right_aligned_signature() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("joint-single.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        input.profile.department_code = "甲函".into();
        input.profile.document_number = "9".into();
        input.profile.recipient = "收文单位".into();
        input.profile.joint_responsible_units = "甲处室".into();
        input.profile.joint_contacts = vec![JointContact {
            unit: "甲处室".into(),
            name: "张三".into(),
            phone: "010-11111111".into(),
        }];
        input.date = "2026年8月5日".into();
        write_docx_ok(&path, &input, "# 单单位联合函\n\n正文。").unwrap();

        let xml = zip_text(&path, "word/document.xml");
        // 最后一个“甲单位”出现在落款（红头在前）：应是右对齐的单独段落，
        // 而不是联合发文的左列居中两列表格。
        let mut search_from = 0;
        let mut last_sig = "";
        while let Some(at) = xml[search_from..].find("甲单位") {
            let abs = search_from + at;
            let start = xml[..abs].rfind("<w:p ").unwrap();
            let end = xml[abs..].find("</w:p>").unwrap() + abs + "</w:p>".len();
            last_sig = &xml[start..end];
            search_from = end;
        }
        assert!(!last_sig.is_empty(), "落款应出现“甲单位”");
        assert!(
            last_sig.contains(r#"w:jc w:val="right""#),
            "单单位联合发文落款应右对齐：{last_sig}"
        );
        assert!(
            !last_sig.contains("<w:tbl>"),
            "不应再走联合落款两列表格：{last_sig}"
        );
        // 联合落款的两列表格（72mm 列）不再出现。
        assert!(!xml.contains(r#"w:gridCol w:w="4422""#));
    }

    #[test]
    fn official_letter_docx_supports_simplex_and_duplex_page_numbers() {
        let temp = tempfile::tempdir().unwrap();
        let simplex_path = temp.path().join("simplex.docx");
        let duplex_path = temp.path().join("duplex.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;

        write_docx_ok(&simplex_path, &input, "# 测试函\n\n正文。").unwrap();
        let simplex_document = zip_text(&simplex_path, "word/document.xml");
        let simplex_settings = zip_text(&simplex_path, "word/settings.xml");
        let simplex_footer = zip_text(&simplex_path, "word/footer1.xml");
        let simplex_first_footer = zip_text(&simplex_path, "word/footer2.xml");
        assert!(simplex_document.contains(r#"w:type="default""#));
        assert!(simplex_document.contains(r#"w:type="first""#));
        assert!(!simplex_document.contains(r#"w:type="even""#));
        assert!(!simplex_settings.contains("w:evenAndOddHeaders"));
        assert!(simplex_footer.contains(r#"w:val="center""#));
        assert!(simplex_footer.contains("<w:instrText>PAGE</w:instrText>"));
        assert!(!simplex_first_footer.contains("<w:instrText>PAGE</w:instrText>"));

        input.profile.duplex_printing = true;
        write_docx_ok(&duplex_path, &input, "# 测试函\n\n正文。").unwrap();
        let duplex_document = zip_text(&duplex_path, "word/document.xml");
        let duplex_settings = zip_text(&duplex_path, "word/settings.xml");
        let odd_footer = zip_text(&duplex_path, "word/footer1.xml");
        let even_footer = zip_text(&duplex_path, "word/footer2.xml");
        let first_footer = zip_text(&duplex_path, "word/footer3.xml");
        assert!(duplex_document.contains(r#"w:type="default""#));
        assert!(duplex_document.contains(r#"w:type="even""#));
        assert!(duplex_document.contains(r#"w:type="first""#));
        assert!(duplex_settings.contains("w:evenAndOddHeaders"));
        assert!(odd_footer.contains(r#"w:val="right""#));
        assert!(even_footer.contains(r#"w:val="left""#));
        assert!(odd_footer.contains("<w:instrText>PAGE</w:instrText>"));
        assert!(even_footer.contains("<w:instrText>PAGE</w:instrText>"));
        assert!(!first_footer.contains("<w:instrText>PAGE</w:instrText>"));
    }

    #[test]
    fn attachment_table_uses_smart_fixed_grid_and_official_fonts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("letter-table.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n<!-- [附件] -->\n# 附件1\n## 统计表\n| 序号 | 名称 | 详细说明 |\n| --- | --- | --- |\n| 1 | 甲 | 这是一段**重要说明**。 |\n| 2 | 乙 | 另一段较长的说明文字。 |",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains(r#"w:tblLayout w:type="fixed""#));
        assert!(xml.contains(r#"w:gridCol w:w="560""#));
        assert!(xml.contains("w:eastAsia=\"黑体\""));
        assert!(xml.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(xml.contains("w:insideH"));
        assert!(xml.contains("w:insideV"));
        let bold_cell = paragraph_containing(&xml, "重要说明");
        assert!(
            bold_cell.contains("<w:b"),
            "表格 Markdown 加粗应保留：{bold_cell}"
        );
        assert!(!bold_cell.contains("**"));
    }

    #[test]
    fn compact_style_merges_heading_and_paragraph_into_one_paragraph() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("compact.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.style_mode = StyleMode::Compact;
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        write_docx_ok(&path, &input, "# 测试函\n\n## 任务目标\n测试正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 标题与正文并入同一段：黑体标题 run + 仿宋正文 run。
        let merged = paragraph_containing(&xml, "任务目标");
        assert!(
            merged.contains("一、任务目标。"),
            "标题应带编号与句号：{merged}"
        );
        assert!(
            merged.contains("测试正文"),
            "紧缩风格应把正文并入标题段：{merged}"
        );
        assert!(merged.contains("黑体"), "标题部分应使用标题字体：{merged}");
    }

    #[test]
    fn compact_style_merges_every_deepest_heading_paragraph_pair() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("compact-deep.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.style_mode = StyleMode::Compact;
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        // 正文区 # 号最多的是 3 级（###）：每个“3 级标题+段落”都合并，2 级标题保持独立。
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n\n## 一、总体要求\n开头段落。\n### （一）任务一\n任务一正文。\n### （二）任务二\n任务二正文。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 2 级标题保持独立：正文段落单独成段，不并入标题段。
        let level_two = paragraph_containing(&xml, "总体要求");
        assert!(level_two.contains("一、总体要求"));
        assert!(
            !level_two.contains("开头段落"),
            "2 级标题不应合并正文：{level_two}"
        );
        // 每个 3 级标题都与紧随正文合并，用楷体标题字体。
        for (title, body) in [
            ("（一）任务一", "任务一正文"),
            ("（二）任务二", "任务二正文"),
        ] {
            let merged = paragraph_containing(&xml, title);
            assert!(
                merged.contains(&format!("{title}。")),
                "标题应带编号与句号：{merged}"
            );
            assert!(
                merged.contains(body),
                "每个最深层标题都应合并正文：{merged}"
            );
            assert!(
                merged.contains("楷体_GB2312"),
                "3 级标题应使用楷体标题字体：{merged}"
            );
        }
    }

    #[test]
    fn empty_copies_to_leaves_the_record_line_without_a_dangling_label() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("no-copies.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.recipient = "某部门".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 与 gonghan-gwa.cls 的 \FooterCopiesLine 一致：没有抄送单位时整行不写“抄送：”。
        assert!(!xml.contains("抄送："), "无抄送单位时不应留下空标签");
        assert!(xml.contains("（共印2份）"));
        assert!(xml.contains("承办单位：办公室"));
    }

    #[test]
    fn red_head_approval_docx_has_floating_frame_records_small_title_and_second_page_signature() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("red-approval.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::RedHeadApproval;
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.issuing_unit = "某某委员会办公室".into();
        input.profile.department_code = "某办呈".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "12".into();
        input.profile.reporting_leaders = "张三、李四".into();
        input.profile.signing_unit = "某某委员会".into();
        input.profile.joint_responsible_units = "综合处、业务处".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "综合处".into(),
                name: "王五".into(),
                phone: "010-12345678".into(),
            },
            JointContact {
                unit: "业务处".into(),
                name: "赵六".into(),
                phone: "010-87654321".into(),
            },
        ];
        input.date = "2026年8月12日".into();
        write_docx_ok(
            &path,
            &input,
            "# 关于认真做好网络安全与信息化重点工作的请示\n\n现将有关情况呈报如下。妥否，请指示。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("某办呈〔2026〕12号"));
        assert!(xml.contains("批　示"));
        assert!(xml.contains("w:tblpXSpec=\"right\""));
        assert!(xml.contains("w:tblpY=\"2720\""));
        assert!(xml.contains("w:tblpYSpec=\"bottom\""));
        assert!(xml.contains("承办单位："));
        assert!(xml.contains("综合处"));
        assert!(xml.contains("业务处"));
        let title = paragraph_containing(&xml, "关于认真做好网络安全与");
        assert!(title.contains("w:sz w:val=\"36\""));
        assert!(title.contains("w:right=\"3175\""));
        let notice = paragraph_containing(&xml, "（此页无正文）");
        assert!(notice.contains("w:pageBreakBefore"));
        assert!(xml.find("（此页无正文）").unwrap() < xml.rfind("某某委员会").unwrap());
        // 落款单位右对齐，但右缩进 4cm 让出签字空间。从「此页无正文」之后找起，
        // 免得匹配到红头里的「某某委员会办公室」。
        let closing = &xml[xml.find("（此页无正文）").unwrap()..];
        let signature = paragraph_containing(closing, "某某委员会");
        assert!(
            signature.contains(r#"w:val="right""#)
                && signature.contains(&format!(
                    r#"w:right="{}""#,
                    crate::export::SIGNATURE_ROOM_TWIPS
                )),
            "落款单位右侧应留出签字空间：{signature}"
        );
        // 成文日期居中于“落款单位 + 签字空间”：左缩进到最宽单位的左沿，
        // 段落右缘仍是版心右缘，居中位置正好是这一整段的中点。
        let date = paragraph_containing(&xml, "2026年8月12日");
        let expected_left = TABLE_CONTENT_WIDTH_TWIPS
            - crate::export::SIGNATURE_ROOM_TWIPS
            - crate::export::red_signature_unit_width_twips(&["某某委员会".to_string()]);
        assert!(
            date.contains(r#"w:val="center""#)
                && date.contains(&format!(r#"w:left="{expected_left}""#)),
            "成文日期应居中于单位与签字空间之间：{date}"
        );
        // 承办区三栏不再等分：承办单位栏最宽，另两栏按内容定宽。
        assert!(
            xml.contains(&format!(
                "<w:gridCol w:w=\"{}\" w:type=\"dxa\" /><w:gridCol w:w=\"{}\" w:type=\"dxa\" /><w:gridCol w:w=\"{}\" w:type=\"dxa\" />",
                crate::export::RED_RECORD_UNIT_TWIPS,
                crate::export::RED_RECORD_CONTACT_TWIPS,
                crate::export::RED_RECORD_PHONE_TWIPS
            )),
            "承办区栏宽应与 LaTeX/预览同源：{xml}"
        );
    }

    /// 承办单位一律不换行：栏内放不下时整格按同一比例横向压窄（`w:w`），
    /// 行高固定一行，与 LaTeX 的 \RedFit 和首页承办区高度对齐。
    #[test]
    fn red_head_approval_record_compresses_long_units_instead_of_wrapping() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("red-approval-long-unit.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::RedHeadApproval;
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.issuing_unit = "某某委员会办公室".into();
        input.profile.signing_unit = "某某委员会".into();
        input.profile.joint_responsible_units = "综合处、教师工作与师资管理处".into();
        input.profile.joint_contacts = vec![
            JointContact {
                unit: "综合处".into(),
                name: "王五".into(),
                phone: "010-12345678".into(),
            },
            JointContact {
                unit: "教师工作与师资管理处".into(),
                name: "赵六".into(),
                phone: "010-87654321".into(),
            },
        ];
        input.date = "2026年8月12日".into();
        write_docx_ok(&path, &input, "# 标题\n\n正文。妥否，请指示。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let scale = crate::export::red_record_scale_percent(
            "承办单位：教师工作与师资管理处",
            crate::export::RED_RECORD_UNIT_USABLE_TWIPS,
        );
        assert!(scale < 100, "长单位名应触发压缩");
        assert!(
            xml.contains(&format!("<w:w w:val=\"{scale}\"")),
            "长单位名应按 {scale}% 横向压窄：{xml}"
        );
        // 短单位名不压缩，也不得整行被裁：行高固定为一行 28pt。
        assert!(
            xml.contains("<w:trHeight w:val=\"560\" w:hRule=\"exact\" />"),
            "{xml}"
        );
    }

    /// 红头呈批件正文里的表格要被赶出首页，否则会压过右侧批示栏。
    #[test]
    fn red_head_approval_body_table_starts_on_a_new_page() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("red-approval-table.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::RedHeadApproval;
        input.profile.kind = TemplateKind::RedHeadApproval;
        input.profile.issuing_unit = "某某委员会办公室".into();
        input.profile.signing_unit = "某某委员会".into();
        input.profile.joint_responsible_units = "综合处".into();
        input.date = "2026年8月12日".into();
        write_docx_ok(
            &path,
            &input,
            "# 标题\n\n短正文。\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n\n妥否，请指示。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let table = xml.find("甲").expect("正文应含表格");
        let break_before = xml[..table]
            .rfind("w:pageBreakBefore")
            .expect("表格前应有换页");
        assert!(
            xml[break_before..table].find("短正文").is_none(),
            "换页要排在表格之前、短正文之后：{xml}"
        );
        // 正文因表格延续到第二页，落款接在正文之后，不再制造“此页无正文”空白页。
        assert!(!xml.contains("（此页无正文）"), "{xml}");
    }

    fn zip_text(path: &Path, entry: &str) -> String {
        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut text = String::new();
        archive
            .by_name(entry)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    fn paragraph_containing<'a>(xml: &'a str, needle: &str) -> &'a str {
        let needle_at = xml.find(needle).unwrap();
        let start = xml[..needle_at].rfind("<w:p ").unwrap();
        let end = xml[needle_at..].find("</w:p>").unwrap() + needle_at + "</w:p>".len();
        &xml[start..end]
    }

    #[test]
    fn ordered_lists_use_circles_inline_and_two_char_first_line_indent_when_independent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ordered-lists.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PlainDocument;
        write_docx_ok(
            &path,
            &input,
            "# 测试\n\n正文：\n1. 第一项；\n1. 第二项，\n\n1. 独立甲，\n1. 独立乙；",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(xml.contains("正文：①第一项；②第二项。"), "{xml}");
        let first = paragraph_containing(&xml, "1.独立甲。");
        assert!(first.contains(r#"w:firstLine="640""#), "{first}");
        let second = paragraph_containing(&xml, "2.独立乙。");
        assert!(second.contains(r#"w:firstLine="640""#), "{second}");
        assert!(!xml.contains("1. 独立甲"), "编号与正文之间不得留空格");
    }

    /// 取一段 XML 里所有 `<w:t>` 文本拼接后的纯文本。密级行的数字年限拆成了
    /// “黑体 + 等宽数字 + 黑体”多个 run，整行断言用拼接后的文本更稳妥。
    fn runs_text(xml: &str) -> String {
        let re = Regex::new(r#"<w:t[^>]*>([^<]*)</w:t>"#).unwrap();
        re.captures_iter(xml)
            .map(|capture| capture[1].to_string())
            .collect()
    }

    fn gap_between<'a>(xml: &'a str, before: &str, after: &str) -> &'a str {
        let before_at = xml.find(before).unwrap();
        let before_end = xml[before_at..].find("</w:p>").unwrap() + before_at + "</w:p>".len();
        let after_at = xml.find(after).unwrap();
        let after_start = xml[..after_at].rfind("<w:p ").unwrap();
        &xml[before_end..after_start]
    }

    #[test]
    fn meeting_agenda_docx_has_required_fonts_indents_and_numbering() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agenda.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.title_hint = "专题研商会议议程".into();
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        let markdown = "# 专题研商会议议程\n\n一、时间地点：2026年8月5日（星期三）14:30，3C会议室。\n\n二、参加人员：张三同志、项目组成员。\n\n三、研讨内容：\n\n1. 汇报总体思路；\n2. 研究下一步工作。";

        write_docx_ok(&path, &input, markdown).unwrap();
        let document = zip_text(&path, "word/document.xml");
        let numberings = zip_text(&path, "word/numbering.xml");

        assert!(document.find("机密★").unwrap() < document.find("专题研商会议议程").unwrap());
        let security = paragraph_containing(&document, "机密★");
        assert!(security.contains("w:eastAsia=\"黑体\""));
        assert!(!security.contains("w:firstLine=\"640\""));
        let security_title_gap = gap_between(&document, "机密★", "专题研商会议议程");
        assert_eq!(
            security_title_gap.matches("<w:p ").count(),
            1,
            "{security_title_gap}"
        );
        assert!(!security_title_gap.contains("<w:t"));

        let title_body_gap = gap_between(&document, "专题研商会议议程", "一、时间地点：");
        assert_eq!(
            title_body_gap.matches("<w:p ").count(),
            1,
            "{title_body_gap}"
        );
        assert!(!title_body_gap.contains("<w:t"));

        let time = paragraph_containing(&document, "一、时间地点：");
        assert!(time.contains("w:eastAsia=\"黑体\""));
        assert!(time.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(time.contains("w:firstLine=\"640\""));

        let attendees = paragraph_containing(&document, "二、参加人员：");
        assert!(attendees.contains("w:eastAsia=\"黑体\""));
        assert!(attendees.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(attendees.contains("w:firstLine=\"640\""));

        let content = paragraph_containing(&document, "三、研讨内容：");
        assert!(content.contains("w:eastAsia=\"黑体\""));
        assert!(content.contains("w:firstLine=\"640\""));

        let first_item = paragraph_containing(&document, "汇报总体思路；");
        let second_item = paragraph_containing(&document, "研究下一步工作。");
        assert!(first_item.contains("<w:numPr>"));
        assert!(second_item.contains("<w:numPr>"));
        assert!(first_item.contains("w:firstLine=\"640\""));
        assert!(second_item.contains("w:firstLine=\"640\""));
        assert!(numberings.contains("w:val=\"%1.\""));
    }

    #[test]
    fn parenthesized_body_uses_kaiti_and_smaller_size() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("parens.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        write_docx_ok(
            &path,
            &input,
            "# 测试函\n\n他说：\"**重要事项**\"。现就（有关事项）及【特别说明】函告如下，参见附表（三）。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let paragraph = paragraph_containing(&xml, "现就");
        // 完整括号段用楷体_GB2312，字号为四号（28 半磅）；其余正文保持仿宋三号。
        assert!(
            paragraph.contains("w:eastAsia=\"楷体_GB2312\""),
            "{paragraph}"
        );
        assert!(
            paragraph.contains(&format!(r#"w:val="{PAREN_SIZE}""#)),
            "{paragraph}"
        );
        assert!(paragraph.contains("w:eastAsia=\"仿宋_GB2312\""));
        assert!(paragraph.contains(&format!(r#"w:val="{BODY_SIZE}""#)));
        assert!(paragraph.contains("“"));
        assert!(paragraph.contains("”"));
        assert!(!paragraph.contains("**"));
        assert!(
            paragraph.contains("<w:b"),
            "Markdown 加粗应生成加粗属性：{paragraph}"
        );
        assert!(paragraph.contains("【特别说明】"));
    }

    #[test]
    fn special_handling_marks_after_security_level_in_black_font() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("special.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let security = paragraph_containing(&xml, "秘密");
        let text = runs_text(security);
        assert!(security.contains("指人专办"));
        assert!(text.contains("★10年"));
        // “指人专办”位于“密级★保密期限”之后，且以黑体单独成 run。
        assert!(
            text.find("★10年").unwrap() < text.find("指人专办").unwrap(),
            "指人专办应排在保密期限之后：{text}"
        );
        assert!(security.contains("w:eastAsia=\"黑体\""));
        assert!(security.contains("w:eastAsia=\"仿宋_GB2312\""));
    }

    #[test]
    fn numeric_security_period_digits_use_monospace_font() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("security-mono.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let security = paragraph_containing(&xml, "秘密");
        assert!(runs_text(security).contains("秘密★10年"));
        // 数字“10”单独成 run，用等宽西文字体（对应 LaTeX 的 ttfamily）。
        let digit_run = paragraph_containing(&xml, ">10<");
        assert!(
            digit_run.contains("Courier New"),
            "数字应用等宽西文字体：{digit_run}"
        );
        assert!(
            security.contains("w:eastAsia=\"仿宋_GB2312\""),
            "“年”等其余部分仍用行内基准字体"
        );

        // 非数字期限（“长期”）不使用等宽字体。
        input.profile.security_period = "长期".into();
        write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        assert!(!xml.contains("Courier New"), "“长期”不应使用等宽字体");
    }

    #[test]
    fn meeting_agenda_special_handling_appends_to_heiti_security_line() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agenda-special.docx");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        write_docx_ok(&path, &input, "# 专题会议议程\n\n正文。").unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let security = paragraph_containing(&xml, "机密");
        let text = runs_text(security);
        // 会议议程密级行整行为黑体，“指人专办”跟在“密级★保密期限”之后。
        assert!(text.contains("机密★10年"));
        assert!(text.contains("指人专办"));
        assert!(
            text.find("★10年").unwrap() < text.find("指人专办").unwrap(),
            "指人专办应排在保密期限之后：{text}"
        );
        assert!(security.contains("w:eastAsia=\"黑体\""));
    }

    #[test]
    fn document_title_compresses_small_overflow_and_wraps_large() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();

        // 22 字标题（超出一行 2 字）→ 字高不变（字号仍为二号），仅横向缩放字形。
        let compress_path = temp.path().join("compress.docx");
        let title_text = "关于认真做好网络安全与信息化重点工作验收的函";
        write_docx_ok(&compress_path, &input, &format!("# {title_text}\n\n正文。")).unwrap();
        let xml = zip_text(&compress_path, "word/document.xml");
        let title = paragraph_containing(&xml, title_text);
        assert!(
            title.contains(&format!(r#"w:sz w:val="{TITLE_SIZE}""#)),
            "字高应保持二号：{title}"
        );
        let scale = title::compressed_scale_percent(title_text);
        assert!(
            title.contains(&format!(r#"<w:w w:val="{scale}""#)),
            "仅横向缩放 {scale}%：{title}"
        );

        // 35 字标题 → jieba 换行，行间用文本换行符（不拆词）。
        let wrap_path = temp.path().join("wrap.docx");
        write_docx_ok(
            &wrap_path,
            &input,
            "# 关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知\n\n正文。",
        )
        .unwrap();
        let xml = zip_text(&wrap_path, "word/document.xml");
        let first_line = paragraph_containing(&xml, "关于转发国家互联网信息办公室");
        assert!(
            first_line.contains(r#"w:type="textWrapping""#),
            "长标题应换行：{first_line}"
        );
    }
}
