//! OOXML (docx) 导出：把公文按 `export::docx` 的版式写成 Word 文档。
//!
//! 各版式部件已拆分到 `export/docx/` 子模块（run/段落构建、页眉、落款、版记、
//! 红头呈批件、正文与议程），根文件保留版式常量、`write_docx` 入口与测试。

use crate::export::title;
use crate::export::title::TitlePlan;
use crate::export::{
    MarkdownBlock, MarkdownSection, attachment_names, compact_heading_flags, official_heading_text,
    parse_markdown_with_lines_with_numbering, plain_text,
};
#[cfg(test)]
use crate::models::StyleMode;
use crate::models::{DraftInput, FontConfig, NumberingConfig, TemplateKind};
use crate::units::UnitDisplay;
use anyhow::{Context, Result, bail};
use docx_rs::*;
use std::fs::File;
use std::path::Path;

mod content;
mod header;
mod paragraphs;
mod phone_record;
pub(crate) mod record;
mod red;
mod runs;
mod signature;

#[cfg(test)]
mod layout_checks;

pub(crate) use content::{
    add_official_content_block, add_smart_table_with_width, write_meeting_agenda_docx,
};
pub(crate) use header::add_official_page_footers;
#[cfg(test)]
pub(crate) use header::issuing_unit_header;
#[cfg(test)]
pub(crate) use paragraphs::image_paragraph_from_bytes;
pub(crate) use paragraphs::{
    agenda_blank_line, agenda_body_paragraph, aligned_paragraph,
    attachment_document_title_paragraph, attachment_label_paragraph, body_paragraph,
    compact_heading_paragraph, document_title_paragraph, heading_paragraph,
    heading_paragraph_with_number, image_paragraph, joint_closing_paragraph,
    joint_signature_cell_paragraph, letter_security_paragraph, ordered_list_paragraph,
    red_approval_title_paragraph, red_record_paragraph,
};
pub(crate) use record::add_footer_record;
pub(crate) use red::{
    red_approval_frame_table, red_approval_record_table, red_approval_top_rule_table,
};
pub(crate) use runs::{
    BoldFont, apply_bold, body_run, body_runs, chinese_fonts, docx_name, marked_runs, record_run,
    security_runs, spread_runs, table_run_sized, table_runs_sized, title_run,
};
pub(crate) use signature::{
    add_attachment_summary, add_joint_signature, add_white_paper_signature, is_joint_mode_one,
    main_issuing_unit, number_runs, signature_date_runs,
};

const BODY_SIZE: usize = 32; // 16 pt，OOXML 使用半磅
const TITLE_SIZE: usize = 44; // 22 pt，二号
const RED_APPROVAL_TITLE_SIZE: usize = 36; // 18 pt，小二号
pub(super) const TABLE_SIZE: usize = 28; // 14 pt，四号
const FOOTER_SIZE: usize = 28; // 14 pt，四号；版记字号独立固定，不随正文表格调整
const PAGE_NUMBER_SIZE: usize = 28; // 14 pt，四号
/// 页码行的固定行高（18 pt）。页脚只有这一行，页脚块的高度就等于它。
const PAGE_NUMBER_LINE_TWIPS: i32 = 360;
/// 版心下边缘距纸张下边缘的距离，即 `PageMargin::bottom`（35 mm）。
const BODY_BOTTOM_MARGIN_TWIPS: i32 = 1984;
/// GB/T 9704—2012 7.3.6：页码编排在版心下边缘之下，一字线上距版心下边缘 7 mm。
const PAGE_NUMBER_GAP_TWIPS: i32 = 397;
/// `PageMargin::footer` 量的是**纸张下边缘到页脚块下沿**的距离——这一点是用
/// Word 自己量出来的，不是照字面猜的：把 FooterDistance 设成 X、页脚段落行高设成
/// H 且不带段前段后间距时，Word 报出的页脚文字上沿正好落在「页高 − X − H」。
///
/// 所以要让页码行的**上沿**落在版心下边缘往下 7 mm 处，页脚距边界得再减去一个
/// 行高：35 mm − 7 mm − 18 pt。原先直接写 10 mm，页码整整低了 12 mm，
/// 几乎贴到纸边上。
const FOOTER_DISTANCE_TWIPS: i32 =
    BODY_BOTTOM_MARGIN_TWIPS - PAGE_NUMBER_GAP_TWIPS - PAGE_NUMBER_LINE_TWIPS;
// TeX 的 28.98 pt 换算为 Word 的 1/20 bp，四舍五入到 577 twips。
const BODY_LINE_TWIPS: u32 = 577;
/// 正文、附件概要或“此页无正文”与落款之间通常空 3 行（每行固定 560 缇）。
const CLOSING_GAP_TWIPS: u32 = 3 * BODY_LINE_TWIPS;
/// 密级行落在版心第一行时的行高。TeX 用 \topskip 把首行基线贴近版心顶端，
/// 不走 28.98pt 的正文基线距；Word 用一个更矮的固定行高等效（白头件、普通公文、
/// 会议议程三处一致）。函稿的密级行不是首行，仍用正文行距。
const SECURITY_FIRST_LINE_TWIPS: u32 = 377;
/// TeX \NoBodyNotice 在“（此页无正文）”之前固定留 58pt。
const NO_BODY_NOTICE_GAP_TWIPS: u32 = 1_156;
/// 红头呈批件首页密级行的段前间距。注意：密级段是正文流第一段（前面的三张
/// 浮动表不占在流高度），Word 与 WPS 都会丢弃页首段落的段前间距，这 300 缇
/// 实际不生效，排版推算不要把它算进去。
const RED_APPROVAL_SECURITY_BEFORE_TWIPS: u32 = 300;
/// 红头呈批件首页发文机关标志的段前间距：密级与红头之间空一行 29pt
/// （TeX 中密级基线在版心下 10mm、红头基线在 30mm，中间正好隔一行）。
const RED_APPROVAL_UNIT_BEFORE_TWIPS: u32 = 580;
/// 红头呈批件首页发文机关标志的行高（TeX 的 \HeaderFontSize 一行）。
const RED_APPROVAL_UNIT_LINE_TWIPS: u32 = 700;
/// 同上，文号行的段前间距。文号直接接红头、不空行（TeX：红头基线 30mm、
/// 文号基线 43mm，是相邻的两行）；Word 的固定行高行盒把字面抬高约 1.6mm，
/// 留 220 缇微调，使文号字面底与红线（48mm）之间保持约 3mm。
const RED_APPROVAL_NUMBER_BEFORE_TWIPS: u32 = 220;
/// 同上，文号与红色横线之间的固定段后间距。
const RED_APPROVAL_NUMBER_AFTER_TWIPS: u32 = 240;
/// 红头呈批件首页标题行盒的上沿，量的是版心顶端往下的距离。
///
/// 红色横线画在版心顶端以下 48 mm 处。TeX 那边标题字面顶沿落在横线下方 7.4 mm
/// （`\par\vspace*{55mm}` 再加小二字面在行盒里的位置），Word 的行盒把 18 pt 的
/// 字面压得更低，直接照抄 55 mm 会低出近 4 mm：2902 缇是拿同一份稿件两边各渲
/// 染一遍、量着 PDF 对齐后取的值。
///
/// 抬头三段（密级、发文机关、文号）都用固定行高排，把它们的实际高度从这里
/// 扣掉就是标题的段前间距；文号缺省时标题也不会跟着上移。
const RED_APPROVAL_TITLE_TOP_TWIPS: u32 = 2_902;
/// 正文中完整括号（全角/半角）及其中内容的字号：14 pt，四号，比正文小一号。
const PAREN_SIZE: usize = 28;
pub(super) const TABLE_CONTENT_WIDTH_TWIPS: usize = 8_844; // 156 mm 版心
/// 横页版心宽 225mm（普通公文的横页选项）。
pub(super) const LANDSCAPE_CONTENT_WIDTH_TWIPS: usize = 12_755;
/// 横页上下边距 28 / 26mm、左右（订口 / 翻口）37 / 35mm：竖页边距转过来。
const LANDSCAPE_MARGIN_TOP_TWIPS: i32 = 1_587;
const LANDSCAPE_BODY_BOTTOM_MARGIN_TWIPS: i32 = 1_474;
const LANDSCAPE_MARGIN_LEFT_TWIPS: i32 = 2_098;
const LANDSCAPE_MARGIN_RIGHT_TWIPS: i32 = 1_984;
/// 横页页脚距边界：页码一字线同样落在版心下 7mm（推导见 [`FOOTER_DISTANCE_TWIPS`]）。
const LANDSCAPE_FOOTER_DISTANCE_TWIPS: i32 =
    LANDSCAPE_BODY_BOTTOM_MARGIN_TWIPS - PAGE_NUMBER_GAP_TWIPS - PAGE_NUMBER_LINE_TWIPS;
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
/// TeX HeaderFontSize = 29 pt，约 28.89 bp，Word 半磅精度取 58。
const HEADER_SIZE: usize = 58;

/// 规格 §3.3：预览版所有占位区域统一 1em 宽，用一个全角空格表示。
pub(crate) use crate::export::element_display::PREVIEW_PLACEHOLDER;

#[allow(dead_code)] // 默认编号的兼容入口，测试与部分旧调用使用。
pub fn write_docx(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
) -> Result<()> {
    write_docx_with_numbering(
        path,
        input,
        markdown,
        display,
        &FontConfig::default(),
        &NumberingConfig::default(),
        &crate::visual_diff::ElementMarks::default(),
    )
}

/// 与 [`write_docx`] 相同，另按设置里的编号样式生成标题与列表编号。
pub fn write_docx_with_numbering(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &crate::visual_diff::ElementMarks,
) -> Result<()> {
    if !input.kind.uses_official_docx() {
        bail!("研究报告的 Word 由 mdx research 转换器生成，不走公文 Word 排版");
    }
    let rendered = crate::mermaid::materialize(
        markdown,
        crate::mermaid::Style::Official,
        crate::mermaid::Format::Png,
    )?;
    let markdown = rendered.as_str();
    // 加粗文字的排法：None 交给 Word 合成粗体（字体不变，等同点了加粗按钮），
    // Some 换用专用粗体字面。
    let bold = fonts.bold_family_docx();
    if input.kind == TemplateKind::PhoneRecord {
        return phone_record::write(path, input, markdown, fonts, numbering, elements);
    }
    if input.kind == TemplateKind::MeetingMinutes {
        // TODO(会议纪要第二期)：版头、名单与三行版记的 Word 排法。
        bail!("会议纪要的 Word 导出尚未接入，请先导出 PDF");
    }
    if input.kind == TemplateKind::MeetingAgenda {
        return write_meeting_agenda_docx(path, input, markdown, fonts, numbering, elements);
    }

    let (blocks, block_lines) = parse_markdown_with_lines_with_numbering(markdown, numbering);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());

    // 红头呈批件首页抬头（密级、发文机关、文号）排下来的总高度，用来反推标题
    // 的段前间距，见 RED_APPROVAL_TITLE_TOP_TWIPS。
    let mut red_approval_header_twips = 0u32;
    // 普通公文的横页选项：整份横排，表格、插图、标题都按横向版心宽排。
    let landscape = input.landscape();
    let content_width = crate::export::table::content_width_twips(landscape);
    let doc = if landscape {
        Docx::new()
            .page_size(16838, 11906)
            .page_orient(PageOrientationType::Landscape)
            .page_margin(PageMargin {
                top: LANDSCAPE_MARGIN_TOP_TWIPS,
                bottom: LANDSCAPE_BODY_BOTTOM_MARGIN_TWIPS,
                left: LANDSCAPE_MARGIN_LEFT_TWIPS,
                right: LANDSCAPE_MARGIN_RIGHT_TWIPS,
                header: 567,
                footer: LANDSCAPE_FOOTER_DISTANCE_TWIPS,
                gutter: 0,
            })
    } else {
        Docx::new().page_size(11906, 16838).page_margin(PageMargin {
            top: 2098,
            bottom: BODY_BOTTOM_MARGIN_TWIPS,
            left: 1587,
            right: 1474,
            // 红头改为页眉内的浮动表定位（不参与在流高度），页眉距边界不再影响
            // 正文起点，与会议议程一致取 10mm。
            header: 567,
            footer: FOOTER_DISTANCE_TWIPS,
            gutter: 0,
        })
    };
    let mut doc = doc
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(BODY_SIZE)
        .default_line_spacing(
            LineSpacing::new()
                .line(BODY_LINE_TWIPS as i32)
                .line_rule(LineSpacingType::Exact),
        );

    doc = add_official_page_footers(doc, input.profile.duplex_printing);
    match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => {
            // TeX：红头及反线在上方，随后份号/右对齐文号，再下一行密级。
            let unit = main_issuing_unit(input, display);
            doc = doc.first_header(header::letter_header(&unit));
            if input.kind == TemplateKind::OfficialLetter {
                let mut number = Paragraph::new()
                    .add_tab(
                        Tab::new()
                            .val(TabValueType::Right)
                            .pos(HEADER_WIDTH_TWIPS as usize),
                    )
                    // TeX：{\heiti \zihao{3} \ttfamily \SerialNumber{}}，黑体不加粗。
                    .add_run(
                        Run::new()
                            .add_text("01")
                            .fonts(chinese_fonts("黑体"))
                            .size(BODY_SIZE),
                    )
                    .add_run(body_run("").add_tab());
                if let Some(value_runs) = number_runs(input, " ", elements) {
                    for run in value_runs {
                        number = number.add_run(run);
                    }
                }
                doc = doc.add_paragraph(
                    number
                        .line_spacing(
                            LineSpacing::new()
                                .before(247)
                                .line(BODY_LINE_TWIPS as i32)
                                .line_rule(LineSpacingType::Exact),
                        )
                        .keep_next(true),
                );
            } else {
                doc = doc.add_paragraph(
                    Paragraph::new().line_spacing(
                        LineSpacing::new()
                            .line(247)
                            .line_rule(LineSpacingType::Exact),
                    ),
                );
            }
            if !input.profile.security_level.trim().is_empty() || elements.security().changed() {
                doc = doc.add_paragraph(
                    letter_security_paragraph(input, elements.security()).keep_next(true),
                );
            }
        }
        TemplateKind::WhitePaper | TemplateKind::PlainDocument => {
            let has_security =
                !input.profile.security_level.trim().is_empty() || elements.security().changed();
            if has_security {
                doc = doc.add_paragraph(
                    letter_security_paragraph(input, elements.security())
                        .line_spacing(
                            LineSpacing::new()
                                .line(SECURITY_FIRST_LINE_TWIPS as i32)
                                .line_rule(LineSpacingType::Exact),
                        )
                        .keep_next(true),
                );
            }
            // TeX：白头件密级后固定空 10 行（\vspace{10\baselineskip}，与有无密级无关）；
            // 普通公文只在写了密级时空 1 行，没有密级就直接排标题。
            let blank_count = if input.kind == TemplateKind::WhitePaper {
                10
            } else if has_security {
                1
            } else {
                0
            };
            for _ in 0..blank_count {
                doc = doc.add_paragraph(header::blank_line().keep_next(true));
            }
        }
        TemplateKind::RedHeadApproval => {
            // 红线、批示框、承办区三张浮动表排在正文流最前面（锚定随后的密级段，
            // 不占在流高度），不要放进首页页眉：WPS 对页眉内的浮动表支持有缺陷，
            // 高表会被整体丢弃并连带丢弃其它浮动表，且不触发文字绕排，首页版式
            // 全崩；挪到正文流后 Word 与 WPS 都能正确定位并绕排（首页页码由
            // add_official_page_footers 的 first_footer 保证，不依赖页眉）。
            doc = doc
                .add_table(red_approval_top_rule_table())
                .add_table(red_approval_frame_table(input))
                .add_table(red_approval_record_table(input, display));
            doc = doc.add_paragraph(
                letter_security_paragraph(input, elements.security()).line_spacing(
                    LineSpacing::new()
                        .before(RED_APPROVAL_SECURITY_BEFORE_TWIPS)
                        .line(BODY_LINE_TWIPS as i32)
                        .line_rule(LineSpacingType::Exact)
                        .after(0),
                ),
            );
            red_approval_header_twips += RED_APPROVAL_SECURITY_BEFORE_TWIPS + BODY_LINE_TWIPS;
            doc = doc.add_paragraph(
                header::issuing_unit_paragraph(&main_issuing_unit(input, display)).line_spacing(
                    LineSpacing::new()
                        .before(RED_APPROVAL_UNIT_BEFORE_TWIPS)
                        .line(RED_APPROVAL_UNIT_LINE_TWIPS as i32)
                        .line_rule(LineSpacingType::Exact),
                ),
            );
            red_approval_header_twips +=
                RED_APPROVAL_UNIT_BEFORE_TWIPS + RED_APPROVAL_UNIT_LINE_TWIPS;
            // TeX 红头呈批件首页：\DocumentNumber{}号，序号与“号”之间不留空格。
            // 行高与段后间距都写死：红色横线是按版心绝对坐标画的（48mm），标题
            // 却是流式排下来的，头部任何一段高度不定，标题与横线的间距就会漂。
            if let Some(value_runs) = number_runs(input, "", elements) {
                let mut paragraph = Paragraph::new().align(AlignmentType::Center).line_spacing(
                    LineSpacing::new()
                        .before(RED_APPROVAL_NUMBER_BEFORE_TWIPS)
                        .line(BODY_LINE_TWIPS as i32)
                        .line_rule(LineSpacingType::Exact)
                        .after(RED_APPROVAL_NUMBER_AFTER_TWIPS),
                );
                for run in value_runs {
                    paragraph = paragraph.add_run(run);
                }
                doc = doc.add_paragraph(paragraph);
                red_approval_header_twips += RED_APPROVAL_NUMBER_BEFORE_TWIPS
                    + BODY_LINE_TWIPS
                    + RED_APPROVAL_NUMBER_AFTER_TWIPS;
            }
        }
        TemplateKind::MeetingAgenda
        | TemplateKind::MeetingMinutes
        | TemplateKind::ResearchReport
        | TemplateKind::PhoneRecord => {
            // 这三个文种各有各的 Word 通道（前面已经分流），走到这里说明分流
            // 的判断与这里的模板分支对不上：报出来，不让程序崩在导出中途。
            bail!("该文种不走公文 Word 排版，请检查导出分流");
        }
    }
    let title_plain = plain_text(title);
    let title_capacity = if input.kind == TemplateKind::RedHeadApproval {
        title::red_approval_chars_per_line()
    } else {
        title::document_chars_per_line(landscape)
    };
    let plan = title::title_plan(&title_plain, title_capacity);
    let title_paragraph = if input.kind == TemplateKind::RedHeadApproval {
        red_approval_title_paragraph(title, &plan)
    } else {
        document_title_paragraph(title, &plan, landscape)
    };
    let title_before = if input.kind == TemplateKind::RedHeadApproval {
        // 标题字面顶沿落在红色横线下方 7.4mm，与 TeX 量出来的一致。
        RED_APPROVAL_TITLE_TOP_TWIPS.saturating_sub(red_approval_header_twips)
    } else {
        if matches!(
            input.kind,
            TemplateKind::OfficialLetter | TemplateKind::PhoneNotice
        ) {
            BODY_LINE_TWIPS
        } else {
            0
        }
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
                    .after(BODY_LINE_TWIPS)
                    .line(BODY_LINE_TWIPS as i32)
                    .line_rule(LineSpacingType::Exact),
            )
            .keep_next(true),
    );

    // 主送机关位的显示值（函稿主送 / 白头件呈报领导）与预览、TeX 同源。
    let addressee = crate::export::element_display::addressee_display(input, display);
    if (!addressee.is_empty() && !markdown.contains(addressee.as_str()))
        || (addressee.is_empty() && elements.recipient().changed())
    {
        doc = doc.add_paragraph(paragraphs::addressee_paragraph(
            &addressee,
            elements.recipient(),
        ));
    }

    let mut attachment_blocks = Vec::new();
    if input.kind.uses_letter_layout() || input.kind == TemplateKind::WhitePaper {
        let mut in_attachment = false;
        let mut seen_document_title = false;
        let mut counters = [0usize; 4];
        let compact_headings =
            compact_heading_flags(&blocks, &block_lines, input.profile.style_mode);
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
                    if compact_headings[index]
                        && let MarkdownBlock::Heading(level, heading) = block
                        && next_is_paragraph
                    {
                        if let Some(title) =
                            official_heading_text(*level, heading, &mut counters, numbering)
                        {
                            let MarkdownBlock::Paragraph(body) = &blocks[index + 1] else {
                                unreachable!()
                            };
                            doc = doc.add_paragraph(compact_heading_paragraph(
                                *level, &title, body, bold,
                            ));
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
                                        .line(BODY_LINE_TWIPS as i32)
                                        .line_rule(LineSpacingType::Exact),
                                ),
                            );
                        }
                        doc = add_official_content_block(
                            doc,
                            block,
                            &mut counters,
                            numbering,
                            bold,
                            content_width,
                        );
                    }
                }
            }
            index += 1;
        }
    } else {
        for block in &blocks {
            match block {
                // 引用块已由 `flatten_quotes` 拆成段落；代码块只有研究报告的解析出，
                // 都到不了这里
                MarkdownBlock::Title(_)
                | MarkdownBlock::Html(_)
                | MarkdownBlock::Marker(_)
                | MarkdownBlock::Diagram { .. }
                | MarkdownBlock::Code { .. }
                | MarkdownBlock::Quote { .. } => {}
                MarkdownBlock::Image { alt, src } => {
                    if let Some(paragraph) = image_paragraph(alt, src, content_width) {
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
                    doc = doc.add_paragraph(body_paragraph(text, bold));
                }
                MarkdownBlock::Aligned { align, text } => {
                    doc = doc.add_paragraph(aligned_paragraph(*align, text, bold));
                }
                MarkdownBlock::OrderedListItem { number, text } => {
                    doc = doc.add_paragraph(ordered_list_paragraph(
                        *number,
                        text,
                        numbering.list2,
                        bold,
                    ));
                }
                MarkdownBlock::Table {
                    rows,
                    aligns,
                    spans,
                    numbered,
                } => {
                    doc = add_smart_table_with_width(
                        doc,
                        rows,
                        aligns,
                        spans,
                        *numbered,
                        bold,
                        content_width,
                    )
                }
            }
        }
    }

    // 附件概要：正文结束后、落款之前列出附件名称（红头呈批件同样支持）。
    if input.kind.uses_letter_layout() || input.kind == TemplateKind::WhitePaper {
        let names = attachment_names(&blocks);
        if !names.is_empty() {
            doc = add_attachment_summary(doc, &names, bold);
        }
    }

    if crate::models::is_joint_signature(input) {
        doc = add_joint_signature(doc, input, display, elements);
    } else if input.kind == TemplateKind::WhitePaper {
        doc = add_white_paper_signature(
            doc,
            input,
            display,
            crate::export::SIGNATURE_ROOM_TWIPS,
            elements,
        );
    } else if input.kind == TemplateKind::RedHeadApproval {
        // 落款最早从第二页开始。正文只有首页那点内容时另起一页标「（此页无正文）」；
        // 正文本来就跨页时不再额外制造空白页，落款接在正文之后。
        if !red_body.reaches_second_page(red_wrap_lines) {
            doc = doc.add_paragraph(
                Paragraph::new()
                    .add_run(body_run("（此页无正文）"))
                    .align(AlignmentType::Both)
                    .indent(None, Some(SpecialIndentType::FirstLine(640)), None, None)
                    .page_break_before(true)
                    .line_spacing(
                        LineSpacing::new()
                            .before(NO_BODY_NOTICE_GAP_TWIPS)
                            .line(BODY_LINE_TWIPS as i32)
                            .line_rule(LineSpacingType::Exact),
                    ),
            );
        }
        doc = add_white_paper_signature(
            doc,
            input,
            display,
            crate::export::SIGNATURE_ROOM_TWIPS,
            elements,
        );
    } else if matches!(
        input.kind,
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice
    ) {
        // 落款单位的显示值与预览、TeX 同源：公函全称、电话通知简称（少于 5 字
        // 逐字加空格），联合发文模式 1 只剩 1 个发文单位时回落右侧单列。
        let signature = crate::export::element_display::signing_unit_display(input, display)
            .into_iter()
            .next()
            .unwrap_or_default();
        if !signature.is_empty()
            || elements.signing_units().iter().any(|mark| mark.changed())
            || elements.date().changed()
        {
            // 代章直接跟在落款单位后面同一行（如“星海省教育厅（代章）”），不另起一行。
            // 是否标注只由 seals_on_behalf 决定（仅公函；电话通知等其他文种不盖章）。
            // 落款单位变了按整字段替换（旧值删除线、新值加框）；代章不进标注。
            let unit_runs = match elements.signing_units().first() {
                Some(mark) if mark.changed() => {
                    let mut runs = marked_runs(&mark.marked(), |text| body_run(text));
                    if crate::export::seals_on_behalf(input, display) {
                        runs.push(body_run("（代章）"));
                    }
                    runs
                }
                _ => {
                    let unit = if crate::export::seals_on_behalf(input, display) {
                        format!("{signature}（代章）")
                    } else {
                        signature.clone()
                    };
                    vec![body_run(unit)]
                }
            };
            let mut unit_paragraph = Paragraph::new()
                .align(AlignmentType::Center)
                .indent(Some(2_608), None, Some(0), None)
                .keep_next(true)
                .line_spacing(
                    LineSpacing::new()
                        .before(CLOSING_GAP_TWIPS)
                        .line(BODY_LINE_TWIPS as i32)
                        .line_rule(LineSpacingType::Exact),
                );
            for run in unit_runs {
                unit_paragraph = unit_paragraph.add_run(run);
            }
            doc = doc.add_paragraph(unit_paragraph);
            let mut date_paragraph = Paragraph::new()
                .align(AlignmentType::Center)
                .indent(Some(2_608), None, Some(0), None)
                .line_spacing(
                    LineSpacing::new()
                        .line(BODY_LINE_TWIPS as i32)
                        .line_rule(LineSpacingType::Exact),
                );
            for run in signature_date_runs(input, elements) {
                date_paragraph = date_paragraph.add_run(run);
            }
            doc = doc.add_paragraph(date_paragraph);
        }
    }

    if !attachment_blocks.is_empty() {
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
                    doc = doc.add_paragraph(attachment_document_title_paragraph(text, landscape));
                }
                _ => {
                    doc = add_official_content_block(
                        doc,
                        block,
                        &mut counters,
                        numbering,
                        bold,
                        content_width,
                    )
                }
            }
        }
    }

    if input.kind == TemplateKind::OfficialLetter {
        doc = add_footer_record(doc, input, display, elements);
    }

    let file =
        File::create(path).with_context(|| format!("无法创建 Word 文件：{}", path.display()))?;
    let mut xml = doc.build();
    xml.document = record::with_record_gap(xml.document);
    xml.pack(file).context("写入 DOCX 包失败")?;
    Ok(())
}

#[cfg(test)]
#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests;
