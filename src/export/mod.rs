//! 导出：把公文导出为 docx / latex / md 三种成品，并提供 Markdown 解析、
//! 标题计数、红头版记度量与文本工具。
//!
//! 各功能域已拆分到 `export/` 子模块（Markdown 解析、红头版记、标题计数、
//! 文本工具），根文件保留导出入口与文件名工具。

pub(crate) mod bibliography;
pub(crate) mod caption;
mod docx;
pub(crate) mod element_display;
pub(crate) mod minutes;
pub(crate) mod print_trace;
pub(crate) mod research;
mod research_theme;
pub(crate) mod table;
pub(crate) mod title;
pub(crate) mod typst;
pub(crate) use docx::record::automatic_print_copies;
#[allow(unused_imports)]
pub(crate) use docx::{write_docx, write_docx_with_numbering};
// 花脸稿一致性测试直接驱动两个导出器的正文入口比对 (文字, 类型) 序列。
#[cfg(test)]
pub(crate) use docx::body_runs;
#[cfg(test)]
pub(crate) use docx::marked_runs;
pub(crate) use research::write_docx as write_docx_research;

use crate::models::{
    DraftInput, ExportSelection, FontConfig, NumberingConfig, TemplateKind, TransmitDefaults,
    split_units,
};
use crate::typst_engine;
use crate::units::UnitDisplay;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub(crate) mod crossref;
mod headings;
mod parse;
mod red;
mod text;

pub(crate) use headings::{
    HeadingCounters, clean_heading_number, official_heading_prefix, official_heading_text,
    render_heading_number, render_list_number,
};
#[allow(unused_imports)]
pub(crate) use parse::{
    ColumnAlign, LineAlign, LocatedBlock, MarkdownBlock, MarkdownSection, QuoteBox, QuoteLine,
    QuoteLineKind, ResearchSection, TableSpan, block_span_for_line, block_start_lines,
    body_heading_max_level, circled_number, compact_heading_flags, flatten_quotes, indent_columns,
    list_item_number_ranges, normalize_ordered_list_punctuation, parse_align_marker,
    parse_indented_ordered_item, parse_list_item, parse_markdown, parse_markdown_located,
    parse_markdown_located_report, parse_markdown_located_research,
    parse_markdown_located_with_numbering, parse_markdown_with_lines,
    parse_markdown_with_lines_and_prefixes, parse_markdown_with_lines_with_numbering,
    parse_markdown_with_numbering, parse_numbered_table_marker, parse_ordered_item,
    parse_research_marker, parse_section_marker, parse_table_cells, parse_unnumbered_marker,
    renumber_ordered_groups, research_report_title_span, research_report_titles,
    research_unnumbered_headings, source_lines, table_span_at,
};
pub(crate) use red::{
    RED_APPROVAL_GUTTER_TWIPS, RED_APPROVAL_NARROW_MM, RED_APPROVAL_RULE_MM,
    RED_APPROVAL_RULE_TWIPS, RED_RECORD_LABEL_CONTACT_TWIPS, RED_RECORD_LABEL_UNIT_TWIPS,
    RedRecordColumns, SIGNATURE_ROOM_MM, SIGNATURE_ROOM_TWIPS, red_approval_body_metrics,
    red_approval_wrap_lines, red_record_columns, red_record_phone_twips, red_record_scale_units,
    red_record_signing_columns, red_record_unit_twips, red_signature_unit_width_em,
    red_signature_unit_width_mm, red_signature_unit_width_twips,
};
#[cfg(test)]
pub(crate) use red::{
    RED_RECORD_EM_TWIPS, RED_RECORD_LABEL_PHONE_TWIPS, RED_RECORD_MIN_UNIT_TWIPS,
    RED_RECORD_TOTAL_TWIPS, red_record_name_twips,
};
#[cfg(test)]
pub(crate) use text::InlineSegment;
#[cfg(test)]
pub(crate) use text::parenthesized_ranges;
pub(crate) use text::{
    REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN, RedlineKind,
    attachment_names, attachment_title_name, balance_line_marks, chinese_date_parts,
    inline_char_spans, inline_math_spans, inline_segments, inline_visible_char_index,
    inline_visible_char_indices, is_redline_sentinel, legacy_attachment_label, mark_added,
    mark_deleted, normalize_chinese_quotes, number_to_chinese, numbered_inline_segments,
    plain_text, redline_chunks, redline_slice_lines, strip_redline, table_columns,
    whole_chunk_kind,
};

pub(crate) fn joint_main_index(input: &DraftInput) -> Option<usize> {
    let units = split_units(&input.profile.joint_issuing_units);
    let main = input.profile.main_issuing_unit.trim();
    if units.is_empty() || main.is_empty() {
        return None;
    }
    units.iter().position(|unit| unit == main)
}

/// 当前稿件实际承载代章属性的单位：联合发文取主发文单位，其他公函取落款单位，
/// 落款单位为空时回落发文单位。是否标注由标准词库中的单位属性唯一决定。
/// 代章只对公函生效——其他文种不盖章，无需标注。
pub(crate) fn seals_on_behalf(input: &DraftInput, display: &UnitDisplay) -> bool {
    if input.kind != TemplateKind::OfficialLetter {
        return false;
    }
    let unit = if input.profile.joint_issuance_mode == crate::models::JointIssuanceMode::Mode1 {
        let units = split_units(&input.profile.joint_issuing_units);
        let main = input.profile.main_issuing_unit.trim();
        if units.iter().any(|unit| unit == main) {
            main.to_string()
        } else {
            units.first().cloned().unwrap_or_default()
        }
    } else if !input.profile.signing_unit.trim().is_empty() {
        input.profile.signing_unit.trim().to_string()
    } else {
        input.profile.issuing_unit.trim().to_string()
    };
    display.seals_on_behalf(&unit)
}

/// 联合发文中实际需要追加“（代章）”的单位位置（联合发文仅公函）。旧稿若未保存主发文单位，
/// 与红头规则一致回落到第一个联合发文单位。
pub(crate) fn joint_seal_index(input: &DraftInput, display: &UnitDisplay) -> Option<usize> {
    if !seals_on_behalf(input, display) {
        return None;
    }
    joint_main_index(input)
        .or_else(|| (!split_units(&input.profile.joint_issuing_units).is_empty()).then_some(0))
}

/// 联合发文落款：主发文单位所在列（0=左列，1=右列）。预览/PDF/DOCX 三处共用，
/// 把成文日期（含代章）压在主发文单位下方而不是整块居中。
/// 返回 `None` 表示主单位不在联合单位列表中、未指定，或是跨两列的最后一个单位，
/// 此时日期仍整体居中。
pub(crate) fn joint_main_column(input: &DraftInput) -> Option<usize> {
    let units = split_units(&input.profile.joint_issuing_units);
    let index = joint_main_index(input)?;
    // 超过两个且为奇数时最后一个单位跨两列居中（规格 §2.5），日期随之居中。
    let odd_last = units.len() > 2 && units.len() % 2 == 1;
    if odd_last && index + 1 == units.len() {
        return None;
    }
    Some(index % 2)
}

/// 份号逐份编制时一共排几份：共印份数（至少 1）。
pub(crate) fn copy_count(input: &DraftInput) -> u32 {
    automatic_print_copies(input).max(1) as u32
}

/// 份号与收文单位的对应清单：份号顺序与 [`copy_count`] 一致，依次为主送、抄送、承办单位。
/// 未开逐份编号或文种无份号时返回 `None`。
pub(crate) fn copy_roster(input: &DraftInput) -> Option<String> {
    if !input.profile.number_copies || !input.kind.has_copy_numbering() {
        return None;
    }
    let responsible = if self::docx::is_joint_mode_one(input) {
        &input.profile.joint_responsible_units
    } else {
        &input.profile.responsible_unit
    };
    let groups = if input.kind == TemplateKind::MeetingMinutes {
        self::docx::record::minutes_recipients(input).to_vec()
    } else {
        vec![
            ("主送", input.profile.recipient.as_str()),
            ("抄送", input.profile.copies_to.as_str()),
            ("承办", responsible.as_str()),
        ]
    };
    let mut out = String::from(
        "份号	单位	类别
",
    );
    let display = UnitDisplay::new(&input.profile.vocabulary_snapshot).for_input(input);
    let mut serial = 0u32;
    for (label, value) in groups {
        for unit in split_units(value) {
            let unit = if unit.starts_with("@person:") {
                display.person_name(&unit)
            } else {
                display.full_name(&unit)
            };
            serial += 1;
            out.push_str(&format!(
                "{}	{unit}	{label}
",
                crate::models::format_copy_number(serial)
            ));
        }
    }
    (serial > 0).then_some(out)
}

/// 一篇稿件对外的标题：标签、状态栏、稿件库记录、导出文件名都用它。
///
/// 研究报告与封面同一口径（[`research::cover_title`]）：文档要素的「文件名称」
/// 优先，留空才取正文区段的 `#`，部分、附录区的 `#` 不算；其余文种正文的 `#`
/// 优先，其次是要素里的标题提示。
pub(crate) fn document_title(input: &DraftInput, markdown: &str) -> String {
    if input.kind.is_research() {
        return research::cover_title(input, markdown)
            .unwrap_or_else(|| "未命名研究报告".to_string());
    }
    extract_title(markdown, &input.title_hint)
}

pub fn extract_title(markdown: &str, fallback: &str) -> String {
    markdown
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if fallback.trim().is_empty() {
                "未命名公文"
            } else {
                fallback.trim()
            }
        })
        .to_string()
}

/// Markdown 保留“标题 + 正文 + 可选附件”。密级、文号、主送、落款、成文日期、抄送和版记
/// 都由 DOCX/PDF 导出器按锁定元数据渲染，这里不再重复写入。
pub fn finalize_markdown(input: &DraftInput, generated: &str) -> String {
    // 研究报告先出去：有序列表统一收尾标点（`；`/`。`）是公文的行文规范，
    // 研报正文里常有以单位、英文缩写或公式结尾的条目，套上去反而是错的。
    // 标题也不补：封面题名由文档要素的「文件名称」维护，正文区的 `#` 只是它的
    // 兜底写法，写不写由作者决定，程序不代填。
    if input.kind.is_research() {
        return format!("{}\n", generated.trim());
    }
    let mut text = normalize_ordered_list_punctuation(generated.trim());
    if !text.lines().any(|line| line.starts_with("# ")) {
        let title = if input.title_hint.trim().is_empty() {
            "【待核实：标题】"
        } else {
            input.title_hint.trim()
        };
        text = format!("# {title}\n\n{text}");
    }
    format!("{}\n", text.trim())
}

/// Markdown 源码包：正文 `.md` 加上它引用的全部资源，压成一个 zip。
///
/// 公文和研究报告统一走这里。稿子里插了图，光给一份 `.md` 是发不出去的——收件人
/// 打开就是一串坏掉的图片引用；分成一堆散文件又不便于传。研究报告还多一份
/// `references.bib`，正文的 frontmatter 也在这一步补上，解压后可以直接再喂给 mdx。
fn write_markdown_archive(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    stem: &str,
    numbering: &NumberingConfig,
) -> Result<()> {
    let bibliography = input
        .kind
        .is_research()
        .then(|| input.research.bibliography_content.trim())
        .filter(|content| !content.is_empty());
    let document = if input.kind.is_research() {
        research::markdown_source(input, markdown, bibliography.is_some(), numbering)
    } else {
        markdown.to_string()
    };

    let file = fs::File::create(path)
        .with_context(|| format!("无法创建 Markdown 源码包：{}", path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(format!("{stem}.md"), options)?;
    zip.write_all(document.as_bytes())?;

    // 图片按 markdown 里写的相对路径入包（通常是 `images/xxx.png`），解压后
    // 引用不用改就能用。图片可能已在稿件库的资源表里，存在性与字节都走图片
    // 资源层；引用缺失时跳过，与从前复制到导出目录的行为一致：少一张图不该让
    // 整次导出失败。
    for src in crate::images::image_refs(&document) {
        if !crate::images::exists(&src) {
            continue;
        }
        let bytes = crate::images::read(&src).with_context(|| format!("无法读取图片 {src}"))?;
        zip.start_file(crate::images::normalize_ref(&src), options)?;
        zip.write_all(&bytes)?;
    }

    if let Some(content) = bibliography {
        zip.start_file("references.bib", options)?;
        zip.write_all(content.as_bytes())?;
    }

    zip.finish()
        .with_context(|| format!("无法写完 Markdown 源码包：{}", path.display()))?;
    Ok(())
}

#[allow(dead_code)] // 默认编号的兼容入口，测试与部分旧调用使用。
pub fn export_all(
    output_dir: &Path,
    input: &DraftInput,
    markdown: &str,
    selection: &ExportSelection,
    display: &UnitDisplay,
    fonts: &FontConfig,
) -> Result<Vec<PathBuf>> {
    export_all_with_numbering(
        output_dir,
        input,
        markdown,
        selection,
        display,
        fonts,
        &NumberingConfig::default(),
    )
}

/// 与 [`export_all`] 相同，另按设置里的编号样式生成标题与列表编号。
#[allow(clippy::too_many_arguments)]
pub fn export_all_with_numbering(
    output_dir: &Path,
    input: &DraftInput,
    markdown: &str,
    selection: &ExportSelection,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
) -> Result<Vec<PathBuf>> {
    let artifacts = export_artifacts(
        output_dir,
        input,
        markdown,
        selection,
        display,
        fonts,
        numbering,
        None,
        &TransmitDefaults::default(),
    )?;
    if let Some(error) = artifacts.compile_error {
        anyhow::bail!("{error}");
    }
    Ok(artifacts.files)
}

/// 一次导出的产物。
pub(crate) struct ExportArtifacts {
    pub files: Vec<PathBuf>,
    /// PDF 排版失败不阻断其余格式，界面保留已生成文件并单独显示失败原因。
    pub compile_error: Option<String>,
    /// 排出了 PDF 时的附带信息（孤行探针报告、排版警告）；只出 Typst 包时，
    /// 这里也装着出包警告（缺图等），由界面照常提示。
    pub typst: Option<typst_engine::TypstOutcome>,
    /// 公函两类随行单据的产出份数。界面靠它把「出了哪些单据」说清楚，
    /// 不用去猜文件名。
    pub sheets: SheetOutputs,
}

/// 公函随行单据的产出情况：纸质《公文签收记录单》一份，电子《传发任务单》
/// 每家接收单位一份。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SheetOutputs {
    /// 纸质签收单的份数（Word 与 PDF 各算一份；同一份内容出了两种格式）。
    pub receipt: usize,
    /// 电子传发任务单的份数。
    pub transmit: usize,
}

impl SheetOutputs {
    /// 状态栏里的一句交代：出了哪几类单据、各几份。
    pub(crate) fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.receipt > 0 {
            parts.push(format!("签收记录单 {} 份", self.receipt));
        }
        if self.transmit > 0 {
            parts.push(format!("传发任务单 {} 份", self.transmit));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("（含{}）", parts.join("、"))
    }
}

mod naming;
pub(crate) use naming::document_stem_prefix;
#[cfg(test)]
use naming::{EXPORT_FILE_SUFFIXES, MAX_EXPORT_FILENAME_CHARS};
use naming::{export_file_stem, transmit_file_name};

/// 保存框的默认文件名与普通导出使用同一命名规则。
pub(crate) fn suggested_file_name(
    input: &DraftInput,
    title: &str,
    kind: crate::draft_page::ExportKind,
) -> Result<String> {
    let stem = export_file_stem(input, title, None, "")?;
    let suffix = match kind {
        crate::draft_page::ExportKind::Pdf => ".pdf",
        crate::draft_page::ExportKind::Word => ".docx",
        crate::draft_page::ExportKind::Typst => "-Typst包.zip",
    };
    Ok(format!("{stem}{suffix}"))
}

/// 导出勾选的格式。PDF 由 Typst 在进程内直接排出（研究报告走 `typst::research`）。
///
/// `transmit_defaults` 是设置页「加密传输单」里的默认填写内容，只对公函的
/// 《传发任务单》有效；别的文种不看它。
#[allow(clippy::too_many_arguments)]
pub(crate) fn export_artifacts(
    output_dir: &Path,
    input: &DraftInput,
    markdown: &str,
    selection: &ExportSelection,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    version_name: Option<&str>,
    transmit_defaults: &TransmitDefaults,
) -> Result<ExportArtifacts> {
    display
        .for_input(input)
        .ensure_resolved(input)
        .map_err(anyhow::Error::msg)?;
    let title = document_title(input, markdown);
    // 一个稿件一个固定目录；历史 overwrite 配置不再改变导出行为。
    let export_stem = document_stem_prefix(input, &title);
    let receipt = selection.receipt && input.kind == TemplateKind::OfficialLetter;
    if selection.pdf || receipt {
        selection.print_trace.validate()?;
    }
    let file_stem = export_file_stem(input, &title, version_name, "")?;
    if !mdx::memory_files::output_is_captured() {
        fs::create_dir_all(output_dir)
            .with_context(|| format!("无法创建输出目录：{}", output_dir.display()))?;
    }
    let document_dir = output_dir.join(&export_stem);
    if !mdx::memory_files::output_is_captured() {
        fs::create_dir_all(&document_dir)
            .with_context(|| format!("无法创建文稿导出目录：{}", document_dir.display()))?;
    }
    let mut files = Vec::new();
    // 公函两类随行单据的份数；非公函或没勾选时保持为零。
    let mut sheet_outputs = SheetOutputs::default();

    if selection.markdown {
        let path = document_dir.join(format!("{file_stem}-源码包.zip"));
        write_markdown_archive(&path, input, markdown, &file_stem, numbering)?;
        files.push(path);
    }
    // 公函的《公文签收记录单》：独立文件，随本次选中的 Word / PDF 各出一份，
    // 与原公函同目录，绝不拼进公函正文。别的文种一律忽略。
    if selection.docx {
        let path = document_dir.join(format!("{file_stem}.docx"));
        if input.kind.is_research() {
            research::write_docx(&path, input, markdown, numbering)?;
        } else {
            docx::write_docx_with_numbering(
                &path,
                input,
                markdown,
                display,
                fonts,
                numbering,
                &crate::visual_diff::ElementMarks::default(),
            )?;
        }
        files.push(path);
    }
    let mut typst = None;
    let mut compile_error = None;
    if selection.pdf {
        let path = document_dir.join(format!("{file_stem}.pdf"));
        let outcome = write_pdf(
            &path,
            input,
            markdown,
            display,
            fonts,
            numbering,
            &Default::default(),
        );
        match outcome {
            Ok(outcome) => {
                files.push(path);
                typst = Some(outcome);
            }
            Err(error) => compile_error = Some(format!("{error:#}")),
        }
    }
    if selection.typst {
        let path = document_dir.join(format!("{file_stem}-Typst包.zip"));
        let warnings = typst::package::write_package(
            &path,
            input,
            markdown,
            display,
            numbering,
            &Default::default(),
        )?;
        files.push(path);
        // 没排 PDF 时也塞一个空结果，让缺图等出包警告照常进审校提示。
        match &mut typst {
            Some(outcome) => outcome.warnings.extend(warnings),
            None => {
                typst = Some(typst_engine::TypstOutcome {
                    warnings,
                    ..Default::default()
                })
            }
        }
    }
    // 公函的两类随行单据（见 docs/signoff-transmission-design.md）：
    // - 纸质《公文签收记录单》：一份，收件人手写签名与日期；
    // - 电子《传发任务单》：每家接收单位一份，不用签字，只出 PDF。
    // 两者都与原公函分开成文件，绝不拼进公函正文。
    if receipt {
        let mut sheets = SheetOutputs::default();
        if selection.docx {
            let path = document_dir.join(format!("{file_stem}_签收记录单.docx"));
            docx::write_receipt_docx(&path, input, markdown, display)
                .context("生成《公文签收记录单》Word 失败")?;
            files.push(path);
            sheets.receipt += 1;
        }
        // 纸单至少要有一份：Word 与 PDF 都没选时也出一份 PDF，
        // 免得勾了这一项却看不到纸单。
        if selection.pdf || !selection.docx {
            let path = document_dir.join(format!("{file_stem}_签收记录单.pdf"));
            match typst::receipt::write_pdf(&path, input, markdown, display, fonts) {
                Ok(outcome) => {
                    files.push(path);
                    sheets.receipt += 1;
                    if let Some(main) = typst.as_mut() {
                        main.warnings.extend(outcome.warnings);
                    }
                }
                Err(error) => {
                    // 签收单排版失败只追加错误，不抹掉主 PDF 的错误。
                    append_error(
                        &mut compile_error,
                        format!("《公文签收记录单》排版失败：{error:#}"),
                    );
                }
            }
        }
        sheets.transmit = write_transmission_sheets(
            &document_dir,
            input,
            markdown,
            display,
            fonts,
            numbering,
            transmit_defaults,
            &mut files,
            &mut typst,
            &mut compile_error,
        );
        sheet_outputs = sheets;
    }
    if selection.print_trace.enabled && (selection.pdf || receipt) {
        print_trace::stamp_outputs(&files, &selection.print_trace, input, markdown)?;
        if let Some(main) = typst.as_mut().filter(|main| !main.pdf.is_empty()) {
            main.pdf =
                mdx::memory_files::read_output(&document_dir.join(format!("{file_stem}.pdf")))?;
        }
    }
    if !files.is_empty() && !mdx::memory_files::output_is_captured() {
        naming::record_outputs(
            &document_dir,
            input,
            &file_stem,
            &files,
            receipt,
            compile_error.is_none(),
        )?;
    }
    Ok(ExportArtifacts {
        files,
        typst,
        compile_error,
        sheets: sheet_outputs,
    })
}

/// 追加一条导出错误：已有的错误留在前面，两条都留。
fn append_error(compile_error: &mut Option<String>, message: String) {
    *compile_error = Some(match compile_error.take() {
        Some(existing) => format!("{existing}\n{message}"),
        None => message,
    });
}

/// 出电子《传发任务单》：每家接收单位一份，只出 PDF。返回实际出成的份数。
///
/// 模板的「纸质文件（N 页）」要填正文页数，自动取：导了 PDF 就按正文排版的结果算，
/// 没导 PDF 时为拿页数多排一次版（只排不写文件，几十毫秒）。
///
/// 排版失败只记错误，不影响其余产物。
#[allow(clippy::too_many_arguments)]
fn write_transmission_sheets(
    dir: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    defaults: &TransmitDefaults,
    files: &mut Vec<PathBuf>,
    typst: &mut Option<typst_engine::TypstOutcome>,
    compile_error: &mut Option<String>,
) -> usize {
    let receivers = typst::transmit::receivers(input, display);
    if receivers.is_empty() {
        return 0;
    }
    let pages = match typst::page_count(input, markdown, display, fonts, numbering) {
        Ok(pages) => pages.max(1),
        Err(error) => {
            append_error(
                compile_error,
                format!("计算正文页数失败，传发任务单的页数按 1 页填：{error:#}"),
            );
            1
        }
    };
    // 上一份单子用过的版面档位：同一批的单位内容几乎一样，接着往下试即可。
    let mut hint = 0;
    let mut written = 0;
    for receiver in &receivers {
        let path = dir.join(transmit_file_name(&receiver.unit));
        match typst::transmit::write_pdf(
            &path, input, markdown, display, fonts, receiver, pages, hint, defaults,
        ) {
            Ok((outcome, index)) => {
                hint = index;
                files.push(path);
                written += 1;
                if let Some(main) = typst.as_mut() {
                    main.warnings.extend(outcome.warnings);
                }
            }
            Err(error) => {
                append_error(
                    compile_error,
                    format!("《传发任务单》排版失败（{}）：{error:#}", receiver.unit),
                );
            }
        }
    }
    written
}

/// 在内存中排一份 PDF：研究报告走 `typst::research`，其余文种走公文模板。
/// 不写正文 PDF，也不生成《份号清单.txt》等附属文件。
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_pdf(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &crate::visual_diff::ElementMarks,
) -> Result<typst_engine::TypstOutcome> {
    if input.kind.is_research() {
        self::typst::research::compile_pdf(input, markdown, numbering)
    } else {
        self::typst::compile_pdf(input, markdown, display, fonts, numbering, elements)
    }
}

/// 排一份 PDF 写到 `path`；只有用户明确选择的输出文件与份号清单会落盘。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_pdf(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &crate::visual_diff::ElementMarks,
) -> Result<typst_engine::TypstOutcome> {
    let outcome = compile_pdf(input, markdown, display, fonts, numbering, elements)?;
    mdx::memory_files::write_output(path, &outcome.pdf)
        .with_context(|| format!("无法写入 PDF：{}", path.display()))?;
    if !input.kind.is_research()
        && let Some(roster) = copy_roster(input)
    {
        // 清单含份号、主送/抄送/承办单位，和 PDF 同等敏感：原子写 + Unix 0600，
        // 不能默认 0644 摊在输出目录里。写失败不影响 PDF。
        let roster_path = path.with_extension("份号清单.txt");
        let _ = mdx::memory_files::write_output(&roster_path, roster.as_bytes());
    }
    Ok(outcome)
}

/// 把一份文档排成 PDF，写在 `dir/stem.pdf`，返回 PDF 路径。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_pdf_for_kind(
    dir: &Path,
    stem: &str,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &crate::visual_diff::ElementMarks,
) -> Result<PathBuf> {
    let path = dir.join(format!("{stem}.pdf"));
    write_pdf(&path, input, markdown, display, fonts, numbering, elements)?;
    Ok(path)
}

pub fn safe_filename(value: &str) -> String {
    clean_filename(value).chars().take(80).collect()
}

fn clean_filename(value: &str) -> String {
    let mut name = value
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    name = name.trim().trim_end_matches('.').to_string();
    if name.is_empty() {
        return "未命名公文".into();
    }
    // Windows 的保留设备名（CON、PRN、NUL、COM1…）不能当文件名：带扩展名也不行
    // （CON.pdf 照样打不开），加个前缀避开。
    if is_reserved_windows_name(&name) {
        name = format!("_{name}");
    }
    name
}

/// Windows 保留设备名：主干（第一个点之前）与保留名相同即算。
fn is_reserved_windows_name(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name);
    RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    /// 研究报告的标题与封面同一口径：「文件名称」优先，留空才取正文区的 `#`，
    /// 部分区的 `#` 不算；其余文种仍是正文 `#` 优先。
    #[test]
    fn document_title_follows_the_cover_for_research_reports() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::ResearchReport;
        input.title_hint = "封面题名".to_string();
        let markdown = "# 正文题名\n\n## 背景\n";
        assert_eq!(document_title(&input, markdown), "封面题名");

        input.title_hint.clear();
        assert_eq!(document_title(&input, markdown), "正文题名");
        assert_eq!(
            document_title(&input, "<!-- [部分] -->\n\n# 现状分析\n\n## 背景\n"),
            "未命名研究报告"
        );

        input.kind = TemplateKind::PlainDocument;
        input.title_hint = "封面题名".to_string();
        assert_eq!(document_title(&input, markdown), "正文题名");
    }

    #[test]
    fn filename_is_windows_safe() {
        assert_eq!(safe_filename("关于A/B:测试?的函"), "关于A_B_测试_的函");
    }

    /// 段内软换行只是编辑器里折了一行，排版时仍是一段：中文接缝不能多出空格，
    /// 英文接缝必须留空格，否则两个单词会粘成一个。
    #[test]
    fn soft_line_breaks_join_chinese_without_a_space_but_keep_one_between_words() {
        let blocks = parse_markdown("请各单位于9月20日前\n报送有关材料。");
        assert_eq!(
            blocks,
            vec![MarkdownBlock::Paragraph(
                "请各单位于9月20日前报送有关材料。".to_string()
            )]
        );

        let blocks = parse_markdown("the quick brown\nfox jumps");
        assert_eq!(
            blocks,
            vec![MarkdownBlock::Paragraph(
                "the quick brown fox jumps".to_string()
            )]
        );

        // 中西文混排：接缝一侧是汉字就不补空格，字间距交给排版器。
        let blocks = parse_markdown("文件编号ABC123\n请按此填报。");
        assert_eq!(
            blocks,
            vec![MarkdownBlock::Paragraph(
                "文件编号ABC123请按此填报。".to_string()
            )]
        );

        // 全角标点也算宽体字：句末顿号后换行同样不补空格。
        let blocks = parse_markdown("甲、\n乙。");
        assert_eq!(
            blocks,
            vec![MarkdownBlock::Paragraph("甲、乙。".to_string())]
        );
    }

    /// 承办区栏宽按内容统筹：联系人栏恒 8 em，电话栏按最长号码定宽，余量归承办单位，
    /// 三栏始终正好铺满版心。
    #[test]
    fn red_record_columns_follow_content_and_fill_the_content_width() {
        // 典型一行：12 位号码 → 电话栏 960+12×160 = 2880，联系人栏 2560，承办单位栏 3404。
        let columns = red_record_columns(&[[
            "综合处".to_string(),
            "张三".to_string(),
            "010-12345678".to_string(),
        ]]);
        assert_eq!(
            columns,
            RedRecordColumns {
                unit: 3_404,
                contact: 2_560,
                phone: 2_880,
            }
        );
        assert_eq!(
            columns.unit + columns.contact + columns.phone,
            RED_RECORD_TOTAL_TWIPS
        );
        // 短号码把余量让给承办单位栏：电话栏 960+5×160 = 1760，承办单位栏 4524。
        let short = red_record_columns(&[[
            "综合处".to_string(),
            "张三".to_string(),
            "010-1".to_string(),
        ]]);
        assert_eq!(short.phone, 1_760);
        assert_eq!(short.unit, RED_RECORD_TOTAL_TWIPS - 2_560 - 1_760);
        // 多条目时电话栏看所有行：续行没有标签，但号码更长时仍由续行决定栏宽。
        let multi = red_record_columns(&[
            [
                "综合处".to_string(),
                "张三".to_string(),
                "010-1".to_string(),
            ],
            [
                "业务处".to_string(),
                "李四".to_string(),
                "010-12345678".to_string(),
            ],
        ]);
        assert_eq!(multi.phone, 12 * 160);
        // 超长号码封顶：承办单位栏至少保住 6 em，号码自己压缩兜底。
        let long =
            red_record_columns(&[["综合处".to_string(), "张三".to_string(), "0".repeat(40)]]);
        assert_eq!(long.unit, RED_RECORD_MIN_UNIT_TWIPS);
    }

    /// 联系人栏可用宽度正好 = 标签 4 em + 姓名 3 em：2–4 字的姓名一律不压缩。
    #[test]
    fn red_record_contact_column_never_compresses_names() {
        let columns = red_record_columns(&[[
            "综合处".to_string(),
            "张三".to_string(),
            "010-12345678".to_string(),
        ]]);
        for name in ["张三", "张三丰", "欧阳司马"] {
            let natural = RED_RECORD_LABEL_CONTACT_TWIPS + red_record_name_twips(name);
            assert_eq!(
                red_record_scale_units(natural, columns.contact_usable()),
                100,
                "{name} 不应触发压缩"
            );
        }
    }

    /// 承办单位一律不换行：放得下不动、放不下按比例压窄，且始终留出栏间空白。
    #[test]
    fn red_record_scale_compresses_only_when_too_wide() {
        let columns = red_record_columns(&[[
            "综合处".to_string(),
            "张三".to_string(),
            "010-12345678".to_string(),
        ]]);
        // “承办单位：综合处”8 字 = 2560 缇，可用 3404-320 = 3084 缇，放得下。
        assert_eq!(
            red_record_scale_units(red_record_unit_twips("综合处"), columns.unit_usable()),
            100
        );
        // 15 字 = 4800 缇，超出可用宽度，应压到 64%（3084/4800）。
        let natural = red_record_unit_twips("教师工作与师资管理处");
        let long = red_record_scale_units(natural, columns.unit_usable());
        assert!((62..=66).contains(&long), "应按比例压窄：{long}");
        // 压缩后的宽度不得超过“栏宽减一个字”，保证与下一栏标签之间有间隔。
        assert!(natural * long / 100 <= columns.unit_usable());
        // 电话栏按内容定宽：常见 12 位号码原样排下，不做压缩。
        let phone_natural = RED_RECORD_LABEL_PHONE_TWIPS + 12 * RED_RECORD_EM_TWIPS / 2;
        assert_eq!(
            red_record_scale_units(phone_natural, columns.phone_usable()),
            100
        );
    }

    /// 签字空间在三端必须同宽：mm（预览/PDF）与缇（Word）不能各说各话。
    #[test]
    fn signature_room_is_the_same_width_in_every_unit() {
        assert_eq!(
            (SIGNATURE_ROOM_MM / 25.4 * 1440.0).round() as usize,
            SIGNATURE_ROOM_TWIPS
        );
    }

    /// 落款单位块宽度：少于 5 字的按分散后的 5 字宽算，多单位取最宽的一个。
    #[test]
    fn red_signature_unit_width_takes_the_widest_spread_unit() {
        // 3 字会被分散到 5 字宽，6 字按自然宽度，取较大者 6。
        let units = vec!["网信办".to_string(), "星海省教育厅".to_string()];
        assert_eq!(red_signature_unit_width_em(&units), 6.0);
        assert_eq!(
            red_signature_unit_width_twips(&units),
            6 * RED_RECORD_EM_TWIPS
        );
        // 毫米与缇必须指向同一个宽度（PDF 用毫米，Word 用缇）。
        let mm = red_signature_unit_width_mm(&units);
        assert!((mm - 33.867).abs() < 0.01, "{mm}");
        assert_eq!(
            (mm / 25.4 * 1440.0).round() as usize,
            red_signature_unit_width_twips(&units)
        );
        // 全是短单位时统一按分散后的 5 字宽。
        assert_eq!(red_signature_unit_width_em(&["办公室".to_string()]), 5.0);
        assert_eq!(red_signature_unit_width_em(&[]), 0.0);
        // 长单位名在「版心 − 签字空间」一栏里折行，宽度封顶，日期不出版心。
        let long = vec!["中央网信办新闻舆论处舆情分析研究中心综合协调办公室".to_string()];
        assert_eq!(
            red_signature_unit_width_twips(&long),
            RED_RECORD_TOTAL_TWIPS - SIGNATURE_ROOM_TWIPS
        );
    }

    /// 首页正文行数随标题行数与承办条目数递减，并保留下限。
    #[test]
    fn red_approval_wrap_lines_shrink_with_title_and_records() {
        assert_eq!(red_approval_wrap_lines(1, 1), 12);
        assert_eq!(red_approval_wrap_lines(1, 2), 11);
        assert_eq!(red_approval_wrap_lines(2, 4), 8);
        // 极端情况下仍留出最少行数，不会归零。
        assert_eq!(red_approval_wrap_lines(9, 9), 4);
    }

    /// 三端共用的正文估算：行数、首个表格图片的位置、以及由此得出的两个判断。
    #[test]
    fn red_approval_body_metrics_drive_page_break_and_notice() {
        // 短正文：不到首页额度，落款另起一页并标“此页无正文”。
        let short = parse_markdown("# 标题\n\n情况如下。妥否，请指示。");
        let metrics = red_approval_body_metrics(&short);
        assert_eq!(metrics.lines_before_float, None);
        assert!(!metrics.reaches_second_page(11));
        assert!(!metrics.float_needs_page_break(11));

        // 长正文：超出首页额度，落款紧随正文，不再制造空白页。
        let long = parse_markdown(&format!("# 标题\n\n{}", "填充版面的正文。".repeat(60)));
        let metrics = red_approval_body_metrics(&long);
        assert!(metrics.lines > 11);
        assert!(metrics.reaches_second_page(11));

        // 正文含表格：无论长短都算作跨页，且表格要被赶出首页。
        let table = parse_markdown("# 标题\n\n短正文。\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n");
        let metrics = red_approval_body_metrics(&table);
        assert_eq!(metrics.lines_before_float, Some(1));
        assert!(metrics.reaches_second_page(11));
        assert!(metrics.float_needs_page_break(11));

        // 附件区的表格不受影响：附件本来就另起一页，没有批示栏要避让。
        let attachment = parse_markdown(
            "# 标题\n\n短正文。\n<!-- [附件] -->\n# 附件\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n",
        );
        assert_eq!(
            red_approval_body_metrics(&attachment).lines_before_float,
            None
        );
    }

    #[test]
    fn parses_chinese_document_date_parts() {
        assert_eq!(chinese_date_parts("2026年8月5日"), Some(("2026", "8", "5")));
        assert_eq!(chinese_date_parts("待定"), None);
    }

    /// 标题编号规则与导出器/预览一致——区段标记与每个附件标题处都重置计数器，
    /// 附件区整体上移一级编号。
    #[test]
    fn heading_prefixes_restart_after_section_markers() {
        let mut counters = HeadingCounters::default();
        let lines = [
            "# 测试函",
            "<!-- [正文] -->",
            "## 一、总体要求",
            "### （一）具体事项",
            "正文段落。",
            "<!-- [附件] -->",
            "# 统计表",
            "## 一、填报说明",
            "### 子项",
            "附件内容。",
            "<!-- [附件] -->",
            "# 说明材料",
            "## 二、其他说明",
        ];
        let prefixes = lines
            .iter()
            .map(|line| counters.next(line))
            .collect::<Vec<_>>();
        assert_eq!(prefixes[0], None, "文档标题不编号");
        assert_eq!(prefixes[1], None, "区段标记不编号");
        assert_eq!(prefixes[2].as_deref(), Some("一、"));
        assert_eq!(prefixes[3].as_deref(), Some("（一）"));
        assert_eq!(prefixes[4], None, "正文段落不编号");
        assert_eq!(prefixes[5], None, "附件区段标记不编号");
        assert_eq!(prefixes[6], None, "附件正式标题（#）不编号");
        assert_eq!(
            prefixes[7].as_deref(),
            Some("一、"),
            "附件内容标题与正文同级并从一、起"
        );
        assert_eq!(prefixes[8].as_deref(), Some("（一）"));
        assert_eq!(prefixes[9], None);
        assert_eq!(prefixes[10], None, "第二个附件标记");
        assert_eq!(prefixes[11], None, "第二个附件正式标题");
        assert_eq!(
            prefixes[12].as_deref(),
            Some("一、"),
            "每个附件之后的内容都重新编号"
        );

        // 旧格式附件内容层级会在显示期上移一级。
        let mut counters = HeadingCounters::default();
        counters.next("# 测试函");
        assert_eq!(counters.next("## 一"), Some("一、".to_string()));
        counters.next("<!-- [附件] -->");
        assert_eq!(counters.next("# 附件1"), None);
        assert_eq!(counters.next("## 统计表"), None);
        assert_eq!(counters.next("### 填报说明").as_deref(), Some("一、"));

        // 正文标记回到正文区，同样重新起算。
        let mut counters = HeadingCounters::default();
        counters.next("# 测试函");
        counters.next("## 一");
        counters.next("## 二");
        assert_eq!(counters.next("<!-- [正文] -->"), None);
        assert_eq!(counters.next("## 重新起算").as_deref(), Some("一、"));
    }

    /// 文档标题与附件正式标题需要按方正小标宋二号居中渲染（centered_title 标志），
    /// 附件标识与内容标题不居中。
    #[test]
    fn formal_titles_are_marked_for_centered_rendering() {
        let mut counters = HeadingCounters::default();
        assert!(!counters.centered_title());
        counters.next("# 测试函");
        assert!(counters.centered_title(), "文档标题需居中");
        counters.next("正文段落。");
        assert!(!counters.centered_title());
        counters.next("<!-- [附件] -->");
        assert!(!counters.centered_title());
        counters.next("# 附件1");
        assert!(!counters.centered_title(), "附件标识不居中");
        counters.next("## 统计表");
        assert!(counters.centered_title(), "附件正式标题需居中");
        counters.next("### 一、填报说明");
        assert!(!counters.centered_title(), "附件内容标题不居中");
    }

    /// 联合发文日期压在主发文单位所在列下方；主单位跨列时整体居中。
    #[test]
    fn joint_main_column_follows_the_main_unit() {
        let mut input = DraftInput::default();
        input.profile.joint_issuing_units = "甲单位、乙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        assert_eq!(joint_main_column(&input), Some(0));
        input.profile.main_issuing_unit = "乙单位".into();
        assert_eq!(joint_main_column(&input), Some(1));

        // 超过两个且为奇数时，最后一个单位跨两列，日期随之整体居中。
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        assert_eq!(joint_main_column(&input), Some(0));
        input.profile.main_issuing_unit = "丙单位".into();
        assert_eq!(joint_main_column(&input), None);

        // 主单位未指定或不在列表中时整体居中。
        input.profile.main_issuing_unit = "丁单位".into();
        assert_eq!(joint_main_column(&input), None);
        input.profile.main_issuing_unit.clear();
        assert_eq!(joint_main_column(&input), None);
    }

    #[test]
    fn joint_main_index_locates_the_main_unit() {
        let mut input = DraftInput::default();
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        assert_eq!(joint_main_index(&input), Some(1));
        input.profile.main_issuing_unit = "丁单位".into();
        assert_eq!(joint_main_index(&input), None);
        input.profile.main_issuing_unit.clear();
        assert_eq!(joint_main_index(&input), None);
    }

    /// 每个块记下的字节范围必须正好切出它自己的源码：界面预览点击回跳全靠它。
    #[test]
    fn block_lines_and_spans_agree_with_each_other() {
        let markdown = "# 标题\n\n第一段首行\n第一段次行\n\n- 列表项\n\n第三段。\n";
        let (blocks, lines) = parse_markdown_with_lines(markdown);
        assert_eq!(blocks.len(), lines.len());
        // 行号就是 \GwaTail 写进 TeX 的那个值，必须指向该块自己的第一行。
        assert_eq!(lines, vec![1, 3, 6, 8]);
        for line in lines {
            let span = block_span_for_line(markdown, line).expect("每个块都该能反查回范围");
            let first_line = markdown[span.clone()].lines().next().unwrap();
            assert_eq!(
                markdown.lines().nth(line - 1).unwrap(),
                first_line,
                "第 {line} 行反查到的范围应从这一行开始"
            );
        }
    }

    #[test]
    fn block_span_covers_the_whole_soft_wrapped_paragraph() {
        // 孤行提示指向整段：点击后要把这段全选中，用户才知道改哪里。
        let markdown = "# 标题\n\n第一段首行\n第一段次行\n\n第二段。\n";
        let span = block_span_for_line(markdown, 3).unwrap();
        assert_eq!(&markdown[span], "第一段首行\n第一段次行");
        assert!(
            block_span_for_line(markdown, 4).is_none(),
            "段内后续行不单独成块"
        );
    }

    #[test]
    fn located_blocks_point_back_at_their_own_source() {
        let markdown = "# 标题\n\n<!-- [正文] -->\n\n第一段首行\n第一段次行\n\n- 列表项\n\n| 甲 | 乙 |\n|---|---|\n| 1 | 2 |\n";
        let located = parse_markdown_located(markdown);
        let slices = located
            .iter()
            .map(|block| &markdown[block.range.clone()])
            .collect::<Vec<_>>();
        assert_eq!(
            slices,
            [
                "# 标题",
                "<!-- [正文] -->",
                // 段落跨行时范围覆盖合并进来的每一行。
                "第一段首行\n第一段次行",
                "- 列表项",
                // 表格从表头一直到最后一行数据。
                "| 甲 | 乙 |\n|---|---|\n| 1 | 2 |",
            ]
        );
    }

    /// 带位置的解析必须与原来的解析给出完全相同的块序列。
    #[test]
    fn located_parsing_matches_plain_parsing() {
        let markdown = "# 标题\n\n正文（含括号）。\n\n## 小标题\n\n<div class=\"x\">\n内容\n</div>\n\n<!-- [附件] -->\n\n# 附件1\n";
        let located = parse_markdown_located(markdown)
            .into_iter()
            .map(|block| block.block)
            .collect::<Vec<_>>();
        assert_eq!(located, parse_markdown(markdown));
    }

    #[test]
    fn parses_basic_blocks() {
        let blocks = parse_markdown("# 标题\n\n## 一、事项\n正文\n\n- 要点");
        assert!(matches!(&blocks[0], MarkdownBlock::Title(t) if t == "标题"));
        assert!(matches!(&blocks[1], MarkdownBlock::Heading(2, t) if t == "事项"));
        assert!(matches!(&blocks[2], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn parses_body_and_attachment_markers_and_all_heading_levels() {
        let blocks = parse_markdown(
            "# 函件标题\n<!-- [正文] -->\n## 一、正文事项\n### （一）具体要求\n<!-- [附件] -->\n# 表格\n## 一、说明",
        );
        assert!(matches!(
            blocks[1],
            MarkdownBlock::Marker(MarkdownSection::Body)
        ));
        assert!(matches!(&blocks[2], MarkdownBlock::Heading(2, text) if text == "正文事项"));
        assert!(matches!(&blocks[3], MarkdownBlock::Heading(3, text) if text == "具体要求"));
        assert!(matches!(
            blocks[4],
            MarkdownBlock::Marker(MarkdownSection::Attachment)
        ));
        assert!(matches!(&blocks[5], MarkdownBlock::Title(text) if text == "表格"));
        assert!(matches!(&blocks[6], MarkdownBlock::Heading(2, text) if text == "说明"));

        // 历史稿读取时规范化为相同结构，不要求用户手工迁移。
        let legacy = parse_markdown("# 函件标题\n<!-- [附件] -->\n# 附件1：表格\n### 一、说明");
        assert!(matches!(&legacy[2], MarkdownBlock::Title(text) if text == "表格"));
        assert!(matches!(&legacy[3], MarkdownBlock::Heading(2, text) if text == "说明"));
    }

    #[test]
    fn body_heading_max_level_ignores_title_and_attachment_headings() {
        // 正文区最深 4 级；附件区虽含 5 级标题，但不计入正文最大层级。
        let blocks = parse_markdown(
            "# 函件标题\n## 一、正文事项\n### （一）具体要求\n#### 1. 更细\n<!-- [附件] -->\n# 附件1：表格\n##### 1. 说明",
        );
        assert_eq!(body_heading_max_level(&blocks), 4);
        // 无正文标题时返回 0，紧缩合并自然不触发。
        let blocks = parse_markdown("# 函件标题\n正文。\n<!-- [附件] -->\n## 表一");
        assert_eq!(body_heading_max_level(&blocks), 0);
    }

    #[test]
    fn parses_gfm_table_as_a_single_block() {
        let blocks = parse_markdown(
            "# 附件测试\n| 序号 | 名称 | 说明 |\n| --- | --- | --- |\n| 1 | 甲 | 较长说明 |\n| 2 | 乙 | 另一说明 |",
        );
        assert!(
            matches!(&blocks[1], MarkdownBlock::Table { rows, .. } if rows.len() == 3 && rows[0].len() == 3)
        );
    }

    /// 一份三列表的序号表源码：标记行之后跟表头，正文里另起两处分组。
    ///
    /// 首列整个留给编号（数据行的那一格留空），分组行照 GFM 的规矩仍是完整的
    /// 表格行（首格写标题，其余留空）——只写一格的行会被当成表格结束。
    const NUMBERED_TABLE: &str = "\
<!-- [序号表] -->
| 序号 | 标题 | 内容 |
| --- | --- | --- |
| 大标题一 |  |  |
|  | 内容1 |  |
|  | 内容2 |  |
| 大标题二 |  |  |
|  | 内容3 |  |";

    /// 序号表：首列按组自动编号，只有首格有内容的行整行合并。
    #[test]
    fn numbered_table_numbers_rows_and_merges_group_rows() {
        let blocks = parse_markdown(NUMBERED_TABLE);
        // 标记行本身按 Html 处理，纸面上不落字。
        assert_eq!(blocks[0], MarkdownBlock::Html("<!-- [序号表] -->".into()));
        let MarkdownBlock::Table {
            rows,
            spans,
            numbered,
            ..
        } = &blocks[1]
        else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert!(numbered, "带标记的表格要认成序号表");
        assert_eq!(rows[1], ["（一）大标题一", "", ""]);
        assert_eq!(rows[2][0], "1");
        assert_eq!(rows[3][0], "2");
        assert_eq!(rows[4], ["（二）大标题二", "", ""]);
        assert_eq!(rows[5][0], "1", "进入下一组后从 1 重新编");
        assert_eq!(
            spans,
            &[
                TableSpan {
                    row: 1,
                    column: 0,
                    row_span: 1,
                    column_span: 3,
                },
                TableSpan {
                    row: 4,
                    column: 0,
                    row_span: 1,
                    column_span: 3,
                },
            ]
        );
    }

    /// 手写的序号和分组编号一律重排：增删行之后不用自己维护。
    #[test]
    fn numbered_table_rewrites_hand_written_numbers() {
        let blocks = parse_markdown(
            "<!-- [序号表] -->
| 序号 | 标题 | 内容 |
| --- | --- | --- |
| （一）大标题 |||
| 1. 标题一 | 甲 |
| 9 | 标题二 | 乙 |
| 二、大标题（手写体） |||",
        );
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[1] else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert_eq!(rows[1][0], "（一）大标题", "手写的（一）清掉后重新生成");
        assert_eq!(rows[2][0], "1", "手写的 1. 作废");
        assert_eq!(rows[3][0], "2", "手写的 9 改成组内位次");
        assert_eq!(rows[4][0], "（二）大标题（手写体）");
        // 手写的整行合并换成同一个整行跨度，不叠加。
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert!(spans.iter().all(|span| span.column_span == 3));
    }

    /// 分组编号的样式跟着设置走，与标题编号同一套选项。
    #[test]
    fn numbered_table_group_style_follows_the_setting() {
        let numbering = NumberingConfig {
            table_group: crate::models::HeadingNumbering::Chinese,
            ..NumberingConfig::default()
        };
        let blocks = parse_markdown_with_numbering(NUMBERED_TABLE, &numbering);
        let MarkdownBlock::Table { rows, .. } = &blocks[1] else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert_eq!(rows[1][0], "一、大标题一");
        assert_eq!(rows[4][0], "二、大标题二");
    }

    /// 表头首格留空时补「序号」；整行全空的行照常给号，不当分组行。
    #[test]
    fn numbered_table_fills_the_header_and_numbers_blank_rows() {
        let blocks = parse_markdown(
            "<!-- [序号表] -->
|  | 标题 | 内容 |
| --- | --- | --- |
|  |  |  |
| 标题1 | 内容1 |  |",
        );
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[1] else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert_eq!(rows[0][0], "序号");
        assert_eq!(rows[1][0], "1", "全空行仍是编号行");
        assert_eq!(rows[2][0], "2");
        assert!(spans.is_empty(), "全空行不该被当成分组行：{spans:?}");
    }

    /// 多行表头：分隔行之前的各行都算表头；表头内部可以 `^^` 纵向合并、`||` 横向合并，
    /// 表头与正文之间的 `^^` 不合并，按原文保留。
    #[test]
    fn multi_row_header_with_merged_cells() {
        let blocks = parse_markdown(
            "| 项目 | 2025年 || 2026年 ||
| ^^ | 收入 | 支出 | 收入 | 支出 |
| --- | --- | --- | --- | --- |
| 甲 | 1 | 2 | 3 | 4 |
| ^^ | 5 | 6 | 7 | 8 |
| 乙 | ^^ | 9 | 9 | 9 |",
        );
        let MarkdownBlock::Table {
            rows,
            header_rows,
            spans,
            ..
        } = &blocks[0]
        else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert_eq!(*header_rows, 2);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0], ["项目", "2025年", "", "2026年", ""]);
        assert_eq!(rows[1][0], "", "表头里的 ^^ 并入上方");
        let spans = spans
            .iter()
            .map(|span| (span.row, span.column, span.row_span, span.column_span))
            .collect::<Vec<_>>();
        assert_eq!(
            spans,
            vec![
                (0, 0, 2, 1),
                (0, 1, 1, 2),
                (0, 3, 1, 2),
                (2, 0, 2, 1),
                (3, 1, 2, 1)
            ]
        );
        // 正文里的 ^^ 照常合并（上面「乙」行第 2 列并入「5」）；
        // 但正文首行写 ^^ 时上方是表头，不合并。
        let blocks = parse_markdown(
            "| 项目 | 金额 |
| 名称 | 元 |
| --- | --- |
| ^^ | 1 |",
        );
        let MarkdownBlock::Table {
            rows,
            header_rows,
            spans,
            ..
        } = &blocks[0]
        else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert_eq!(*header_rows, 2);
        assert_eq!(rows[2][0], "^^", "表头与正文之间不纵向合并");
        assert!(spans.is_empty(), "{spans:?}");
    }

    /// 单行表头的旧写法不受影响；表格前一行普通文字不会被当成表头。
    #[test]
    fn single_row_header_is_unchanged() {
        let blocks = parse_markdown("正文一句。\n| A | B |\n| --- | --- |\n| 1 | 2 |");
        assert_eq!(blocks[0], MarkdownBlock::Paragraph("正文一句。".into()));
        let MarkdownBlock::Table {
            rows, header_rows, ..
        } = &blocks[1]
        else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert_eq!(*header_rows, 1);
        assert_eq!(rows.len(), 2);
    }

    /// 序号表配多行表头：首列整列空着时「序号」竖跨整组表头，编号从正文首行起。
    #[test]
    fn numbered_table_with_multi_row_header() {
        let blocks = parse_markdown(
            "<!-- [序号表] -->
|  | 指标 || 说明 |
|  | 计划 | 完成 | ^^ |
| --- | --- | --- | --- |
| 大标题 |  |  |  |
|  | 1 | 2 | 甲 |",
        );
        let MarkdownBlock::Table {
            rows,
            header_rows,
            spans,
            ..
        } = &blocks[1]
        else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert_eq!(*header_rows, 2);
        assert_eq!(rows[0][0], "序号");
        assert_eq!(rows[2][0], "（一）大标题");
        assert_eq!(rows[3][0], "1");
        let spans = spans
            .iter()
            .map(|span| (span.row, span.column, span.row_span, span.column_span))
            .collect::<Vec<_>>();
        assert_eq!(
            spans,
            vec![(0, 0, 2, 1), (0, 1, 1, 2), (0, 3, 2, 1), (2, 0, 1, 4)]
        );
    }

    /// 标记行只管紧邻的那张表格：中间隔了别的非空行就不算数。
    #[test]
    fn numbered_table_marker_only_applies_to_the_next_table() {
        let table = "| 序号 | 标题 | 内容 |
| --- | --- | --- |
| 大标题 |  |  |
| 标题1 | 内容1 |  |";
        // 隔空行仍算数。
        let blocks = parse_markdown(&format!("<!-- [序号表] -->\n\n{table}"));
        let MarkdownBlock::Table { numbered, .. } = &blocks[1] else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert!(numbered, "空行不该打断标记");

        // 中间插了正文就不算数，表格原样保留。
        let blocks = parse_markdown(&format!("<!-- [序号表] -->\n正文一句。\n\n{table}"));
        let MarkdownBlock::Table { rows, numbered, .. } = &blocks[2] else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert!(!numbered, "中间有正文时不该认成序号表");
        assert_eq!(rows[1][0], "大标题", "不当序号表就不动原文字");
    }

    /// 居中 / 居右标记：标记行不落纸面，其下每行各成一个对齐行，遇到空行恢复
    /// 正常排版；区内的 `#`、`1.` 不再当标题、列表认，区内换标记就换对齐方式。
    #[test]
    fn align_markers_cover_the_lines_up_to_the_next_blank_line() {
        let blocks = parse_markdown(
            "正文一段。\n<!-- [居中] -->\n第一行\n# 不是标题\n<!-- [居右] -->\n**右边**一行\n\n之后恢复正文。",
        );
        assert_eq!(
            blocks,
            vec![
                MarkdownBlock::Paragraph("正文一段。".into()),
                MarkdownBlock::Html("<!-- [居中] -->".into()),
                MarkdownBlock::Aligned {
                    align: LineAlign::Center,
                    text: "第一行".into(),
                },
                MarkdownBlock::Aligned {
                    align: LineAlign::Center,
                    text: "# 不是标题".into(),
                },
                MarkdownBlock::Html("<!-- [居右] -->".into()),
                MarkdownBlock::Aligned {
                    align: LineAlign::Right,
                    text: "**右边**一行".into(),
                },
                MarkdownBlock::Paragraph("之后恢复正文。".into()),
            ]
        );
        for marker in ["<!--[居中]-->", "<!-- 【居中】 -->", "<!-- [center] -->"] {
            assert_eq!(parse_align_marker(marker), Some(LineAlign::Center));
        }
        for marker in ["<!-- [居右] -->", "<!-- [右对齐] -->", "<!-- [RIGHT] -->"] {
            assert_eq!(parse_align_marker(marker), Some(LineAlign::Right));
        }
        assert_eq!(parse_align_marker("[居中]"), None, "只认 HTML 注释写法");
    }

    /// 用户手上的那种写法也照收：分组行写满竖线、序号列照旧手填数字，
    /// 加了标记之后编号与合并都由程序接管。
    #[test]
    fn numbered_table_accepts_the_hand_written_filler_style() {
        let blocks = parse_markdown(
            "<!-- [序号表] -->
| 序号| 标题 |  内容   |   备注  |
| --- | --- | --- | --- |
|  （一） 大标题  ||||
|  1   |  标题1   |     |     |
|  2   |  标题2   |     |     |
|  3   |     |     |     |
|  4   |     |     |     |
|  （二） 大标题  ||||
|  1   |     |     |     |
|  2   |     |     |     |
|  3   |     |     |     |",
        );
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[1] else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert_eq!(rows.len(), 10, "表头 + 九个正文行");
        assert_eq!(rows[1], ["（一）大标题", "", "", ""]);
        assert_eq!(rows[2], ["1", "标题1", "", ""]);
        assert_eq!(rows[3], ["2", "标题2", "", ""]);
        assert_eq!(rows[4][0], "3", "手填的 3 由程序重排");
        assert_eq!(rows[5][0], "4");
        assert_eq!(rows[6], ["（二）大标题", "", "", ""]);
        assert_eq!(rows[7][0], "1", "新的一组从 1 起");
        assert_eq!(rows[9][0], "3");
        // 手写的 |||| 换成同一个整行跨度，不叠加。
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert!(spans.iter().all(|span| span.column_span == 4));
    }

    /// 研究报告的序号表与公文同一套规矩：首列编号、分组行整行合并，分组编号
    /// 跟设置走。mdx 那边拿到的是 `research::markdown_with_frontmatter` 按这份
    /// 结果写好的源码，预览因此与纸面一致。
    #[test]
    fn research_parse_numbers_numbered_tables_like_official_ones() {
        let markdown = "<!-- [序号表] -->
| 序号 | 标题 | 内容 |
| --- | --- | --- |
| 大标题一 |  |  |
|  | 内容1 |  |";
        let marks = crate::export::crossref::ResearchMarks::default();
        let numbering = NumberingConfig {
            table_group: crate::models::HeadingNumbering::Chinese,
            ..NumberingConfig::default()
        };
        let blocks = parse_markdown_located_research(markdown, &marks, &numbering);
        let MarkdownBlock::Table {
            rows,
            spans,
            numbered,
            ..
        } = &blocks[1].block
        else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert!(numbered);
        assert_eq!(rows[1][0], "一、大标题一", "分组编号跟设置走");
        assert_eq!(rows[2][0], "1");
        assert_eq!(spans.len(), 1, "分组行整行合并：{spans:?}");
        assert_eq!(spans[0].column_span, 3);
    }

    /// 没有标记行的同款表格行为与从前逐字一致。
    #[test]
    fn a_table_without_the_marker_is_left_alone() {
        let plain = NUMBERED_TABLE.replace("<!-- [序号表] -->\n", "");
        let blocks = parse_markdown(&plain);
        let MarkdownBlock::Table {
            rows,
            spans,
            numbered,
            ..
        } = &blocks[0]
        else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert!(!numbered);
        assert_eq!(rows[1][0], "大标题一", "不生成分组编号");
        assert_eq!(rows[2], ["", "内容1", ""], "首列原样保留");
        assert!(spans.is_empty(), "不自动合并：{spans:?}");
    }

    /// 与 `^^` 纵向合并有牵扯的行不参与归一化，两者互不打架。
    #[test]
    fn numbered_table_leaves_vertical_merges_alone() {
        let blocks = parse_markdown(
            "<!-- [序号表] -->
| 序号 | 标题 | 内容 |
| --- | --- | --- |
| 标题1 | 甲 | 内容A |
| 标题2 | 乙 | ^^ |
| 标题3 | 丙 | ^^ |",
        );
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[1] else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert_eq!(rows[1][0], "1");
        assert_eq!(rows[2][0], "2");
        assert_eq!(rows[3][0], "3");
        assert_eq!(
            spans,
            &[TableSpan {
                row: 1,
                column: 2,
                row_span: 3,
                column_span: 1,
            }]
        );
    }

    /// 别的列在这一行起头往下 `^^` 合并时，这一行不当分组行：否则整行跨度会把
    /// 纵向合并顶掉，下面那一格的合并就静默丢了。
    #[test]
    fn numbered_table_keeps_vertical_merges_anchored_on_a_title_like_row() {
        let blocks = parse_markdown(
            "<!-- [序号表] -->
| 序号 | 标题 | 内容 |
| --- | --- | --- |
| 大标题 |  |  |
|  | 甲 | ^^ |",
        );
        let MarkdownBlock::Table { spans, .. } = &blocks[1] else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert_eq!(
            spans,
            &[TableSpan {
                row: 1,
                column: 2,
                row_span: 2,
                column_span: 1,
            }]
        );
    }

    /// 全角数字与 `1)`、`1）` 这类写法也是手写行号，不是分组标题。
    #[test]
    fn numbered_table_treats_full_width_and_paren_numbers_as_row_numbers() {
        let blocks = parse_markdown(
            "<!-- [序号表] -->
| 序号 | 标题 | 内容 |
| --- | --- | --- |
| １ |  |  |
| 2) |  |  |
| 3） |  |  |",
        );
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[1] else {
            panic!("应当解析为序号表：{blocks:?}");
        };
        assert!(spans.is_empty(), "{spans:?}");
        assert_eq!(rows[1][0], "1");
        assert_eq!(rows[2][0], "2");
        assert_eq!(rows[3][0], "3");
    }

    #[test]
    fn parses_horizontal_and_vertical_table_spans() {
        let blocks = parse_markdown(
            "| 类别 | 项目 | 说明 |
| --- | --- | --- |
| 横向合并 || 备注 |
| 纵向合并 | 事项一 | 甲 |
| ^^ | 事项二 | 乙 |",
        );
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[0] else {
            panic!("应当解析为合并表格：{blocks:?}");
        };
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1], ["横向合并", "", "备注"]);
        assert_eq!(rows[3][0], "", "^^ 应归一化为被覆盖的空格");
        assert_eq!(
            spans,
            &[
                TableSpan {
                    row: 1,
                    column: 0,
                    row_span: 1,
                    column_span: 2,
                },
                TableSpan {
                    row: 2,
                    column: 0,
                    row_span: 2,
                    column_span: 1,
                },
            ]
        );
    }

    #[test]
    fn parses_combined_span_at_the_end_of_a_row() {
        let blocks = parse_markdown(
            "| 甲 | 乙 | 丙 |
| --- | --- | --- |
| 跨三列 |||
| ^^ |||",
        );
        let MarkdownBlock::Table { rows, spans, .. } = &blocks[0] else {
            panic!("应当解析为合并表格：{blocks:?}");
        };
        assert_eq!(rows[1], ["跨三列", "", ""]);
        assert_eq!(
            spans,
            &[TableSpan {
                row: 1,
                column: 0,
                row_span: 2,
                column_span: 3,
            }]
        );
    }

    /// 分隔行里的冒号是列对齐，随表格一起带给导出器与预览。
    #[test]
    fn table_keeps_column_alignment_from_the_separator_row() {
        let blocks = parse_markdown(
            "| 序号 | 名称 | 金额 |
| :--- | :---: | ---: |
| 1 | 甲 | 12 |",
        );
        let MarkdownBlock::Table { rows, aligns, .. } = &blocks[0] else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert_eq!(rows.len(), 2, "分隔行不进正文");
        assert_eq!(
            aligns,
            &[ColumnAlign::Left, ColumnAlign::Center, ColumnAlign::Right]
        );

        // 不写冒号就是 Auto，交给智能列宽判定。
        let blocks = parse_markdown(
            "| 甲 | 乙 |
| --- | --- |
| 1 | 2 |",
        );
        let MarkdownBlock::Table { aligns, .. } = &blocks[0] else {
            panic!("应当解析为表格");
        };
        assert_eq!(aligns, &[ColumnAlign::Auto, ColumnAlign::Auto]);
    }

    /// 预览取的就是导出器算好的列宽，合并表格也要带上跨度，否则预览与 Word 各自一套。
    #[test]
    fn preview_table_columns_reuse_the_merged_aware_grid() {
        let blocks = parse_markdown(
            "| 甲 | 乙 | 丙 | 丁 |
| --- | --- | --- | --- |
| 一 | 二 | 三 | 四 |
| 横跨两列的一段较长说明文字。 || 末 | 尾 |",
        );
        let MarkdownBlock::Table {
            rows,
            aligns,
            spans,
            ..
        } = &blocks[0]
        else {
            panic!("应当解析为合并表格：{blocks:?}");
        };
        assert_eq!(spans.len(), 1);
        let fractions = table_columns(rows, aligns, spans, 1)
            .iter()
            .map(|column| column.fraction)
            .collect::<Vec<_>>();
        // 四列内容相近，合并格本来放得下，列宽应基本均匀；
        // 不传 spans 的旧路径会把首列算到 0.33、末列压到 0.17。
        assert!(
            fractions
                .iter()
                .all(|fraction| (fraction - 0.25).abs() < 0.05),
            "预览列宽应复用合并感知的网格：{fractions:?}"
        );
    }

    #[test]
    fn parses_image_reference_as_own_block() {
        let blocks = parse_markdown(
            "# 标题\n正文段落。\n![扫描件](images/20260809_120000_扫描件.png)\n结尾段落。",
        );
        assert!(matches!(
            &blocks[2],
            MarkdownBlock::Image { alt, src }
                if alt.as_str() == "扫描件" && src.as_str() == "images/20260809_120000_扫描件.png"
        ));
        // 图片行独立成块，不并入相邻段落。
        assert!(matches!(&blocks[1], MarkdownBlock::Paragraph(_)));
        assert!(matches!(&blocks[3], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn image_parsing_ignores_inline_and_malformed() {
        // 段内出现的 `![..]` 不被当作图片块。
        let blocks = parse_markdown("文中出现 ![图标](x.png) 引用");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
        // 残缺语法不匹配。
        let blocks = parse_markdown("![缺括号](images/a.png");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
        // src 含空白的引用不匹配（与 images::image_refs 约束一致）。
        let blocks = parse_markdown("![图](images/my file.png)");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn exports_all_three_formats() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某某单位".into();
        input.profile.recipient = "某某部门".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-12345678".into();
        let markdown =
            "# 关于开展测试工作的函\n\n某某部门：\n\n现就有关事项函告如下。\n\n特此函告。";
        // PDF 要随包字体；没有 runtime/fonts 的环境（CI）只出 md 与 docx。
        let fonts_available = crate::portable_runtime::find_font_dir().is_some();
        let selection = ExportSelection {
            pdf: fonts_available,
            ..ExportSelection::default()
        };
        let files = export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(files.len(), if fonts_available { 3 } else { 2 });
        assert!(files.iter().all(|path| path.metadata().unwrap().len() > 0));
        let document_dir = files[0].parent().unwrap();
        assert!(files.iter().all(|path| path.parent() == Some(document_dir)));
        // docx / pdf 与导出目录同名；md 源码包在同一个主干名后加 `-源码包`。
        let title = document_title(&input, markdown);
        let file_stem = export_file_stem(&input, &title, None, "").unwrap();
        for path in &files {
            let stem = path.file_stem().unwrap().to_string_lossy();
            assert!(
                stem == file_stem || stem == format!("{file_stem}-源码包"),
                "产物名应与导出目录同源：{stem}"
            );
        }

        let docx_path = files
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "docx"))
            .unwrap();
        let file = std::fs::File::open(docx_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        use std::io::Read;
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("关于开展测试工作的函"));
    }

    /// 把源码包里某个条目读成字符串；条目不存在返回 None。
    fn archive_entry(path: &Path, name: &str) -> Option<String> {
        use std::io::Read as _;
        let file = std::fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(name).ok()?;
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        Some(text)
    }

    #[test]
    fn markdown_export_survives_missing_image_refs() {
        // 图片引用指向不存在的文件时，源码包照常出：少一张图不该让整次导出失败。
        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: false,
            pdf: false,
            typst: false,
            receipt: false,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "# 标题\n\n正文。\n\n![图](images/不存在的.png)",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].extension().is_some_and(|ext| ext == "zip"));
        let stem = files[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .strip_suffix("-源码包.zip")
            .unwrap()
            .to_string();
        assert_eq!(
            archive_entry(&files[0], &format!("{stem}.md")).unwrap(),
            "# 标题\n\n正文。\n\n![图](images/不存在的.png)"
        );
    }

    /// 插了图的稿子，源码包里必须连图一起带走——只给一份 md，收件人打开就是
    /// 一串坏掉的图片引用。公文和研究报告统一这个行为。
    #[test]
    fn markdown_archive_carries_referenced_images() {
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_zip_{}.png", std::process::id());
        std::fs::write(dir.join(&name), b"fake-png").unwrap();

        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: false,
            pdf: false,
            typst: false,
            receipt: false,
            overwrite: true,
        };
        let markdown = format!("# 标题\n\n正文。\n\n![图](images/{name})");
        let files = export_all(
            temp.path(),
            &input,
            &markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let entry = archive_entry(&files[0], &format!("images/{name}"));
        let _ = std::fs::remove_file(dir.join(&name));
        assert_eq!(entry.as_deref(), Some("fake-png"), "源码包应包含引用的图片");
    }

    /// 研究报告的源码包额外带 frontmatter 与 references.bib，解压后能直接再
    /// 交给 mdx 转换一次。
    #[test]
    fn research_markdown_archive_carries_frontmatter_and_bibliography() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试研究".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        input.research.bibliography_content = "@book{demo, title={示例}}".into();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: false,
            pdf: false,
            typst: false,
            receipt: false,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "## 研究背景\n\n正文 [@demo]。",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let stem = files[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .strip_suffix("-源码包.zip")
            .unwrap()
            .to_string();
        let document = archive_entry(&files[0], &format!("{stem}.md")).unwrap();
        assert!(document.starts_with("---\n密级: 公开\n"), "{document}");
        assert!(document.contains("文件名称: 测试研究"));
        assert!(document.contains("bibliography: references.bib"));
        assert_eq!(
            archive_entry(&files[0], "references.bib").as_deref(),
            Some("@book{demo, title={示例}}")
        );
    }

    #[test]
    fn overwrite_reuses_the_same_paths() {
        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let markdown = "# 关于开展测试工作的函\n\n正文。\n";
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: false,
            pdf: false,
            typst: false,
            receipt: false,
            overwrite: true,
        };
        let first = export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let second = export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(first, second);

        let versioned = ExportSelection {
            overwrite: false,
            ..selection
        };
        let third = export_all(
            temp.path(),
            &input,
            markdown,
            &versioned,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(first, third);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn export_names_ignore_submission_versions() {
        let input = DraftInput {
            kind: TemplateKind::PlainDocument,
            ..Default::default()
        };
        for name in [None, Some("送审稿"), Some("定稿"), Some(&"版本".repeat(50))] {
            assert_eq!(
                export_file_stem(&input, "通知", name, "").unwrap(),
                "普通公文-通知"
            );
        }
    }

    #[test]
    fn export_names_keep_title_ends_and_fit_all_formats() {
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let title = format!("关于{}的函", "测试".repeat(80));
        let stem = export_file_stem(&input, &title, None, "").unwrap();
        assert!(stem.starts_with("公函-关于"));
        assert!(stem.contains('…'));
        assert!(stem.ends_with("的函-盖章版"));
        for suffix in EXPORT_FILE_SUFFIXES {
            assert!(format!("{stem}{suffix}").chars().count() <= MAX_EXPORT_FILENAME_CHARS);
        }
        let dir = document_stem_prefix(&input, &title);
        assert!(dir.chars().count() <= MAX_EXPORT_FILENAME_CHARS);
        assert!(dir.ends_with("的函"));
    }

    #[test]
    fn letter_and_approval_keep_one_directory_across_states_and_numbers() {
        use crate::models::LetterVersion;
        for (kind, code, label) in [
            (TemplateKind::OfficialLetter, "某办函", "公函"),
            (TemplateKind::RedHeadApproval, "某办呈", "红头呈批件"),
        ] {
            let mut input = DraftInput {
                kind,
                ..Default::default()
            };
            input.profile.department_code = code.into();
            input.profile.document_number = "12".into();
            input.date = "2026年11月6日".into();
            input.profile.letter_version = LetterVersion::Preview;
            let dir = document_stem_prefix(&input, "通知");
            assert_eq!(dir, format!("{label}-{code}-通知"));
            assert_eq!(
                export_file_stem(&input, "通知", None, "").unwrap(),
                format!("{label}-{code}-通知-签字版")
            );
            input.profile.letter_version = LetterVersion::Formal;
            assert_eq!(document_stem_prefix(&input, "通知"), dir);
            assert_eq!(
                export_file_stem(&input, "通知", None, "").unwrap(),
                format!("{label}-{code}〔2026〕12号-通知-盖章版")
            );
            input.profile.document_number = "123".into();
            assert_eq!(document_stem_prefix(&input, "通知"), dir);
        }
    }

    #[test]
    fn renumbering_cleans_registered_outputs_and_keeps_signature_and_user_files() {
        use crate::models::LetterVersion;
        let dir = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.department_code = "某办函".into();
        input.date = "2026年11月6日".into();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: true,
            pdf: false,
            typst: false,
            receipt: false,
            overwrite: false,
        };
        let run = |input: &DraftInput, body: &str| {
            export_artifacts(
                dir.path(),
                input,
                body,
                &selection,
                &UnitDisplay::new(&[]),
                &FontConfig::default(),
                &NumberingConfig::default(),
                Some("历史版本名称"),
                &TransmitDefaults::default(),
            )
            .unwrap()
            .files
        };
        input.profile.letter_version = LetterVersion::Preview;
        let signed = run(&input, "# 通知\n\n签字正文。");
        input.profile.letter_version = LetterVersion::Formal;
        input.profile.document_number = "1".into();
        let old = run(&input, "# 通知\n\n旧正文。");
        let folder = old[0].parent().unwrap();
        let user_file = folder.join("用户笔记.txt");
        std::fs::write(&user_file, "保留").unwrap();
        input.profile.document_number = "123".into();
        let current = run(&input, "# 通知\n\n最新正文。");
        assert!(old.iter().all(|p| !p.exists()));
        assert!(signed.iter().chain(&current).all(|p| p.is_file()));
        assert!(user_file.is_file());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let again = run(&input, "# 通知\n\n再次修改。");
        assert_eq!(current, again);
        let name = current[0].file_name().unwrap().to_string_lossy();
        let stem = name.strip_suffix("-源码包.zip").unwrap();
        assert!(
            archive_entry(&current[0], &format!("{stem}.md"))
                .unwrap()
                .contains("再次修改。")
        );
    }

    #[test]
    fn one_export_shares_a_single_file_stem_across_formats() {
        let temp = tempfile::tempdir().unwrap();
        let title = "关于开展A计划、持续推进重点事项（第一阶段）并做好后续工作的通知".repeat(2);
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: true,
            pdf: false,
            typst: true,
            receipt: true,
            overwrite: true,
        };
        let artifacts = export_artifacts(
            temp.path(),
            &input,
            &format!(
                "# {title}

正文。
"
            ),
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &NumberingConfig::default(),
            Some("送审稿"),
            &TransmitDefaults::default(),
        )
        .unwrap();
        let names = artifacts
            .files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let stem = names[1].strip_suffix(".docx").unwrap().to_string();
        for name in &names {
            assert!(name.starts_with(&stem), "{name} 与 {stem} 主干不同");
            assert!(name.chars().count() <= MAX_EXPORT_FILENAME_CHARS, "{name}");
        }
    }

    #[test]
    fn long_title_export_keeps_contents_and_limits_every_filename() {
        let temp = tempfile::tempdir().unwrap();
        let title = "关于开展A计划、持续推进重点事项（第一阶段）并做好后续工作的通知".repeat(3);
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let markdown = format!("# {title}\n\n正文保留。\n");
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: true,
            docx: true,
            pdf: false,
            typst: false,
            receipt: true,
            overwrite: true,
        };
        let artifacts = export_artifacts(
            temp.path(),
            &input,
            &markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &NumberingConfig::default(),
            Some("送审稿"),
            &TransmitDefaults::default(),
        )
        .unwrap();
        assert_eq!(artifacts.files.len(), 3);
        for path in &artifacts.files {
            assert!(path.file_name().unwrap().to_string_lossy().chars().count() <= 42);
        }
        let archive_stem = artifacts.files[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .strip_suffix("-源码包.zip")
            .unwrap()
            .to_string();
        assert!(format!("{archive_stem}.md").chars().count() <= 42);
        let source = archive_entry(&artifacts.files[0], &format!("{archive_stem}.md"))
            .expect("源码包缺少 Markdown 正文");
        assert!(source.contains(&title));

        let file = std::fs::File::open(&artifacts.files[1]).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        use std::io::Read;
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let xml_text = xml
            .split('>')
            .skip(1)
            .filter_map(|part| part.split_once('<').map(|(text, _)| text))
            .collect::<String>();
        assert!(xml_text.contains(&title));
    }

    #[test]
    fn overlong_fixed_name_fails_before_creating_output_directory() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("export");
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.department_code = "机关代字".repeat(20);
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: true,
            pdf: false,
            typst: false,
            receipt: false,
            overwrite: false,
        };
        let result = export_artifacts(
            &output,
            &input,
            "# 标题\n\n正文。",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &NumberingConfig::default(),
            Some(&"版本".repeat(40)),
            &TransmitDefaults::default(),
        );
        assert!(result.is_err());
        assert!(!output.exists());
    }

    /// 把正文按"完整括号"切段：全角 `（…）`、半角 `(…)` 或方头括号 `【…】`
    /// 配对出现的整段归为"括号内"，其余归为"括号外"；括号不配对（缺左或缺右）时按普通文本处理。
    /// 仅供测试内对照：生产代码走的是 `inline_segments`（保留 Markdown `**…**` 加粗语义）。
    fn split_parenthesized(text: &str) -> Vec<(&str, bool)> {
        let mut segments = Vec::new();
        let mut normal_start = 0usize;
        for (start, end) in parenthesized_ranges(text) {
            if normal_start < start {
                segments.push((&text[normal_start..start], false));
            }
            segments.push((&text[start..end], true));
            normal_start = end;
        }
        if normal_start < text.len() {
            segments.push((&text[normal_start..], false));
        }
        segments
    }

    #[test]
    fn splits_complete_parentheses_only() {
        assert_eq!(
            split_parenthesized("现就（有关事项）函告如下。"),
            [
                ("现就", false),
                ("（有关事项）", true),
                ("函告如下。", false)
            ]
        );
        assert_eq!(
            split_parenthesized("使用(a)半角与（b）全角。"),
            [
                ("使用", false),
                ("(a)", true),
                ("半角与", false),
                ("（b）", true),
                ("全角。", false)
            ]
        );
        // 不配对的括号按普通文本处理。
        assert_eq!(
            split_parenthesized("缺少（右括号"),
            [("缺少（右括号", false)]
        );
        // 嵌套括号整体算一个括号段。
        assert_eq!(
            split_parenthesized("测试（含（内层）内容）完毕"),
            [
                ("测试", false),
                ("（含（内层）内容）", true),
                ("完毕", false)
            ]
        );
        assert_eq!(
            split_parenthesized("正文【特别说明】继续（办理要求）。"),
            [
                ("正文", false),
                ("【特别说明】", true),
                ("继续", false),
                ("（办理要求）", true),
                ("。", false)
            ]
        );
        assert_eq!(
            split_parenthesized("混合（外层【内层】内容）结束"),
            [
                ("混合", false),
                ("（外层【内层】内容）", true),
                ("结束", false)
            ]
        );
    }

    #[test]
    fn normalizes_quote_styles_and_directions() {
        assert_eq!(
            normalize_chinese_quotes(r#"他说："第一句"，又说：“第二句“。"#),
            "他说：“第一句”，又说：“第二句”。"
        );
        assert_eq!(
            normalize_chinese_quotes("外层「文字『内层』文字」"),
            "外层“文字‘内层’文字”"
        );
        // 英文单词内部的撇号使用中文右单引号，但不参与成对引号方向切换。
        assert_eq!(normalize_chinese_quotes("don't 'quote'"), "don’t ‘quote’");
    }

    #[test]
    fn inline_bold_and_parentheses_can_cross_each_other() {
        assert_eq!(
            inline_segments("正文（**重点**内容）和**【加粗说明】**。"),
            [
                InlineSegment {
                    text: "正文".into(),
                    bold: false,
                    parenthesized: false,
                },
                InlineSegment {
                    text: "（".into(),
                    bold: false,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "重点".into(),
                    bold: true,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "内容）".into(),
                    bold: false,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "和".into(),
                    bold: false,
                    parenthesized: false,
                },
                InlineSegment {
                    text: "【加粗说明】".into(),
                    bold: true,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "。".into(),
                    bold: false,
                    parenthesized: false,
                },
            ]
        );
    }

    #[test]
    fn collects_attachment_names_from_heading_and_inline() {
        // 规范写法：每个附件标记后的 `#` 是正式标题。
        let blocks = parse_markdown(
            "# 测试函\n<!-- [正文] -->\n正文。\n<!-- [附件] -->\n# 统计表\n内容。\n<!-- [附件] -->\n# 说明材料\n内容。",
        );
        assert_eq!(attachment_names(&blocks), ["统计表", "说明材料"]);
        // 历史格式在读取期规范化，附件清单结果保持不变。
        let blocks = parse_markdown("# 测试函\n<!-- [附件] -->\n# 附件1：汇总表\n内容。");
        assert_eq!(attachment_names(&blocks), ["汇总表"]);
        // 没有名称的附件不入列。
        let blocks = parse_markdown(
            "# 测试函\n<!-- [附件] -->\n内容。\n<!-- [附件] -->\n# 说明材料\n内容。",
        );
        assert_eq!(attachment_names(&blocks), ["说明材料"]);
    }

    /// 公函的《公文签收记录单》：默认关闭；勾上后随 Word / PDF 各出一份独立文件，
    /// 与原公函同目录，绝不拼进公函。
    #[test]
    fn receipt_signoff_sheet_is_opt_in_and_kept_separate() {
        // 即便只勾 Word，签收单也要按 Typst 排一次数页数，需要随包字体；CI 没有
        // runtime/fonts 时跳过（PDF 相关的同族用例已有同样处理）。
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.department_code = "某民函".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "18".into();
        input.profile.recipient = "甲单位、乙单位".into();
        input.profile.copies_to = "丙单位".into();
        let markdown = "# 关于商洽的函\n\n甲单位、乙单位：\n\n现就有关事项函告如下。\n\n特此函告。";
        let run = |selection: &ExportSelection| {
            export_all(
                temp.path(),
                &input,
                markdown,
                selection,
                &UnitDisplay::new(&[]),
                &FontConfig::default(),
            )
            .unwrap()
        };
        let has_receipt = |files: &[std::path::PathBuf]| {
            files
                .iter()
                .any(|path| path.to_string_lossy().contains("签收记录单"))
        };

        // 默认关闭：不生成签收单。
        assert!(!ExportSelection::default().receipt);
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: true,
            pdf: false,
            typst: false,
            receipt: false,
            overwrite: true,
        };
        let files = run(&selection);
        assert!(!has_receipt(&files));

        // 勾上后：多出一份独立的 Word，命名是「<主干名>_签收记录单.docx」，与公函同目录。
        let selection = ExportSelection {
            receipt: true,
            ..selection
        };
        let files = run(&selection);
        let receipt = files
            .iter()
            .find(|path| path.to_string_lossy().contains("签收记录单"))
            .expect("没有生成签收记录单");
        assert!(receipt.to_string_lossy().ends_with("_签收记录单.docx"));
        assert_eq!(receipt.parent().unwrap(), files[0].parent().unwrap());

        // 只勾签收单（没有主格式）不算要导出。
        let only_receipt = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: false,
            pdf: false,
            typst: false,
            receipt: true,
            overwrite: true,
        };
        assert!(!only_receipt.any());
    }

    /// 别的文种一律不生成签收记录单。
    #[test]
    fn receipt_signoff_sheet_is_ignored_for_other_kinds() {
        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput {
            kind: TemplateKind::PlainDocument,
            ..Default::default()
        };
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: true,
            pdf: false,
            typst: false,
            receipt: true,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "# 通知\n\n正文。",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert!(
            files
                .iter()
                .all(|path| !path.to_string_lossy().contains("签收记录单"))
        );
    }

    /// 公函勾了签收单且选了 PDF：多出一份独立的签收记录单 PDF。
    #[test]
    fn receipt_signoff_sheet_is_produced_alongside_the_pdf() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.recipient = "甲单位".into();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: false,
            pdf: true,
            typst: false,
            receipt: true,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "# 关于商洽的函\n\n甲单位：\n\n现就有关事项函告如下。",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let receipt = files
            .iter()
            .find(|path| path.to_string_lossy().ends_with("_签收记录单.pdf"))
            .expect("没有生成签收记录单 PDF");
        assert!(receipt.metadata().unwrap().len() > 0);
        assert_eq!(receipt.parent().unwrap(), files[0].parent().unwrap());
    }

    /// 公函勾了单据且选了 PDF：纸质签收单一份，电子传发任务单每家接收单位一份。
    #[test]
    fn both_sheet_kinds_are_produced_for_every_receiving_unit() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.recipient = "甲单位、乙单位".into();
        input.profile.copies_to = "丙单位".into();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: false,
            pdf: true,
            typst: false,
            receipt: true,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "# 关于商洽的函\n\n甲单位、乙单位：\n\n现就有关事项函告如下。",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        // 纸质签收单：一份。
        assert_eq!(
            files
                .iter()
                .filter(|path| path.to_string_lossy().ends_with("_签收记录单.pdf"))
                .count(),
            1
        );
        // 电子传输单：主送两家、抄送一家，各一份。
        let sheets: Vec<&PathBuf> = files
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("传发任务单-"))
            })
            .collect();
        assert_eq!(sheets.len(), 3, "每家接收单位一份：{sheets:?}");
        let names: Vec<String> = sheets
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.contains(&"传发任务单-甲单位.pdf".to_string()),
            "{names:?}"
        );
        assert!(
            names.contains(&"传发任务单-丙单位.pdf".to_string()),
            "{names:?}"
        );
        for sheet in &sheets {
            assert!(sheet.metadata().unwrap().len() > 0);
        }
    }

    /// 随行单据的份数要能说出来：签收单一份、传发任务单每家接收单位一份，
    /// 状态栏据此交代「含哪几类单据、各几份」。
    #[test]
    fn the_sheet_counts_are_reported_for_the_status_line() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.recipient = "甲单位、乙单位".into();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: false,
            pdf: true,
            typst: false,
            receipt: true,
            overwrite: true,
        };
        let artifacts = export_artifacts(
            temp.path(),
            &input,
            "# 关于商洽的函\n\n甲单位、乙单位：\n\n现就有关事项函告如下。",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &NumberingConfig::default(),
            None,
            &TransmitDefaults::default(),
        )
        .unwrap();
        assert_eq!(artifacts.sheets.receipt, 1);
        assert_eq!(artifacts.sheets.transmit, 2);
        assert_eq!(
            artifacts.sheets.describe(),
            "（含签收记录单 1 份、传发任务单 2 份）"
        );
    }

    /// 没勾单据时不提单据：状态栏那句话只讲文件本身。
    #[test]
    fn no_sheets_means_nothing_to_say_about_them() {
        let sheets = SheetOutputs::default();
        assert_eq!(sheets.describe(), "");
        assert_eq!(
            SheetOutputs {
                receipt: 1,
                transmit: 0
            }
            .describe(),
            "（含签收记录单 1 份）"
        );
    }

    /// 只导 Word、不导 PDF 时，电子传输单照样出（它本来就是 PDF）。
    #[test]
    fn the_transmission_sheet_is_produced_even_without_a_pdf_export() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.recipient = "甲单位".into();
        let selection = ExportSelection {
            print_trace: Default::default(),
            markdown: false,
            docx: true,
            pdf: false,
            typst: false,
            receipt: true,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "# 关于商洽的函\n\n甲单位：\n\n现就有关事项函告如下。",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert!(
            files
                .iter()
                .any(|path| path.to_string_lossy().ends_with("传发任务单-甲单位.pdf")),
            "只导 Word 时也要出电子传输单：{files:?}"
        );
    }

    /// 传输单的文件名不超 42 字符上限：单位名过长时截短。
    #[test]
    fn the_transmission_sheet_name_fits_the_filename_budget() {
        let name = transmit_file_name(&"很长的接收单位名称".repeat(6));
        assert!(name.starts_with("传发任务单-"), "{name}");
        assert!(name.ends_with(".pdf"), "{name}");
        assert!(name.chars().count() <= MAX_EXPORT_FILENAME_CHARS, "{name}");
    }
}
