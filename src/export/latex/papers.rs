//! 各文种生成器：白头件、红头呈批件与会议议程。
//!
//! 由 src/export/latex.rs 拆分而来：本文件是模块 `export::latex::papers`，与其它子模块共享
//! `export::latex` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{DraftInput, LetterVersion, StyleMode};
use crate::units::{UnitDisplay};
use crate::export::{MarkdownBlock, chinese_date_parts, parse_markdown_with_lines, plain_text};
use crate::export::latex::{official_letter_sections_to_tex, official_letter_sections_to_tex_with_barrier, latex_name, tex_escape, security_commands, attachment_summary_tex, title_content_tex, red_approval_title_content_tex, tex_spread_signature};

pub(crate) fn white_paper_tex(input: &DraftInput, markdown: &str, display: &UnitDisplay) -> String {
    let (blocks, block_lines) = parse_markdown_with_lines(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 正文与函稿一致（含标题编号与紧缩合并）；附件区段保留，与函稿同链路落版。
    let (mut body, attachments) = official_letter_sections_to_tex(
        &blocks,
        &block_lines,
        input.profile.style_mode == StyleMode::Compact,
    );
    // 附件概要：正文结束后、落款之前列出附件名称。
    if let Some(summary) = attachment_summary_tex(&blocks) {
        body.push_str(&summary);
    }
    let attachment_command = if attachments.trim().is_empty() {
        String::new()
    } else {
        format!("\\SetAttachmentContent{{\n{attachments}\n}}\n")
    };
    let security = security_commands(input);
    // 呈报领导（楷体顶格）按人员编码排序、相同职务合并后写入 \Recipient。
    let leaders = display
        .reporting_leaders(&input.profile.reporting_leaders)
        .trim()
        .trim_end_matches('：')
        .to_string();
    // 落款单位：每个单位一行、行间空一行（便于签字），整体右对齐；显示文本
    // 少于 5 字时逐字用 `\hspace*` 分散对齐到 5 字宽，与预览/Word 各端一致。
    let signature_unit = display
        .white_paper_signature_units(input)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .enumerate()
        .map(|(index, unit)| {
            let line = tex_spread_signature(&unit);
            if index > 0 {
                format!("\\par\\vspace{{\\baselineskip}}{line}")
            } else {
                line
            }
        })
        .collect::<String>();
    // 规格 §3.3：预览版占位区域统一 1em 宽，成文日期“日”留空，与公函一致。
    let preview = input.profile.letter_version == LetterVersion::Preview;
    let preview_placeholder = "\\makebox[1em][c]{}";
    // 成文日期未填时沿用类默认：年份取当前年、日期留空待填。
    let date_commands = match chinese_date_parts(&input.date) {
        Some((year, month, day)) => {
            let day = if preview {
                preview_placeholder.to_string()
            } else {
                tex_escape(day)
            };
            format!(
                "\\renewcommand{{\\SignatureYear}}{{{}}}\n\\renewcommand{{\\SignatureMonth}}{{{}}}\n\\renewcommand{{\\SignatureDay}}{{{}}}\n",
                tex_escape(year),
                tex_escape(month),
                day
            )
        }
        None => String::new(),
    };

    format!(
        r#"%!TEX program = xelatex
\documentclass[proof,noforcenewpage,whitepaper]{{gonghan-gwa}}
{security}\renewcommand{{\DocumentTitle}}{{{title}}}
\renewcommand{{\TitleContent}}{{{title_content}}}
\renewcommand{{\Recipient}}{{{leaders}}}
\renewcommand{{\MainContent}}{{
{body}
}}
{attachment_command}\renewcommand{{\SignatureUnit}}{{{signature_unit}}}
{date_commands}\begin{{document}}
\makeletter
\end{{document}}
"#,
        title = tex_escape(title),
        title_content = title_content_tex(title),
        security = security,
        leaders = tex_escape(&leaders),
        body = body,
        attachment_command = attachment_command,
        signature_unit = signature_unit,
        date_commands = date_commands,
    )
}

/// 红头呈批件的独立入口；首页框架在后续专用实现中生成。
pub(crate) fn red_head_approval_tex(input: &DraftInput, markdown: &str, display: &UnitDisplay) -> String {
    let (blocks, block_lines) = parse_markdown_with_lines(markdown);
    let title = blocks
        .iter()
        .find_map(|block| match block {
            MarkdownBlock::Title(title) => Some(title.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 表格与图片不得留在首页：在正文区第一个表格/图片之前插入换页屏障。
    let (mut body, attachments) = official_letter_sections_to_tex_with_barrier(
        &blocks,
        &block_lines,
        input.profile.style_mode == StyleMode::Compact,
        Some("\\RedPageOneBarrier"),
    );
    if let Some(summary) = attachment_summary_tex(&blocks) {
        // 附件概要自带两行垂直留白，无法仅靠正文行数额度安全判断；红头呈批件
        // 统一从第二页开始排概要，避免这段固定留白把末行推到承办区红线上。
        body.push_str(&format!("\n\\RedPageOneBarrier\n{summary}"));
    }
    let attachment_command = if attachments.trim().is_empty() {
        String::new()
    } else {
        format!("\\SetAttachmentContent{{\n{attachments}\n}}\n")
    };
    let security = security_commands(input);
    let leaders = display
        .reporting_leaders(&input.profile.reporting_leaders)
        .trim()
        .trim_end_matches('：')
        .to_string();
    let issuing = display.full_name(&input.profile.issuing_unit);
    let signature_units = display
        .white_paper_signature_units(input)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect::<Vec<_>>();
    // 成文日期要居中于“落款单位 + 签字空间”，TeX 量不出 vbox 的自然宽度，
    // 这里按字数算好最宽一行的宽度写进类文件。写成毫米而不是 em：这条
    // \setlength 在导言区执行，那里的字号不是三号。
    let signature_unit_width_mm = crate::export::red_signature_unit_width_mm(&signature_units);
    let signature_unit = signature_units
        .iter()
        .enumerate()
        .map(|(index, unit)| {
            let line = tex_spread_signature(unit);
            if index > 0 {
                format!("\\par\\vspace{{\\baselineskip}}{line}")
            } else {
                line
            }
        })
        .collect::<String>();
    let preview = input.profile.letter_version == LetterVersion::Preview;
    let placeholder = "\\makebox[1em][c]{}";
    let document_number = if preview {
        placeholder.to_string()
    } else {
        tex_escape(&input.profile.document_number)
    };
    let date_commands = match chinese_date_parts(&input.date) {
        Some((year, month, day)) => format!(
            "\\renewcommand{{\\SignatureYear}}{{{}}}\n\\renewcommand{{\\SignatureMonth}}{{{}}}\n\\renewcommand{{\\SignatureDay}}{{{}}}\n",
            tex_escape(year),
            tex_escape(month),
            if preview {
                placeholder.to_string()
            } else {
                tex_escape(day)
            },
        ),
        None => String::new(),
    };
    let entries = crate::models::joint_responsible_entries(&input.profile);
    let responsible_rows = red_approval_responsible_rows_tex(&entries, display);
    let title_plain = plain_text(title);

    format!(
        r#"%!TEX program = xelatex
\documentclass[proof,noforcenewpage,redapproval]{{gonghan-gwa}}
\renewcommand{{\IssuingUnit}}{{{issuing}}}
\renewcommand{{\Year}}{{{document_year}}}
\renewcommand{{\DepartmentCode}}{{{department}}}
\renewcommand{{\DocumentNumber}}{{{number}}}
{security}\renewcommand{{\DocumentTitle}}{{{title}}}
\renewcommand{{\TitleContent}}{{{title_content}}}
\renewcommand{{\Recipient}}{{{leaders}}}
\renewcommand{{\MainContent}}{{
{body}
}}
{attachment_command}\renewcommand{{\SignatureUnit}}{{{signature_unit}}}
\setlength{{\RedSignatureUnitWidth}}{{{signature_unit_width:.3}mm}}
{date_commands}
\SetRedResponsibleContent{{
{responsible_rows}
}}
\begin{{document}}
\makeredapproval
\end{{document}}
"#,
        issuing = tex_escape(&issuing),
        document_year = tex_escape(&input.document_year()),
        department = tex_escape(&input.profile.department_code),
        number = document_number,
        security = security,
        title = tex_escape(&title_plain),
        title_content = red_approval_title_content_tex(title),
        leaders = tex_escape(&leaders),
        body = body,
        attachment_command = attachment_command,
        signature_unit = signature_unit,
        signature_unit_width = signature_unit_width_mm,
        date_commands = date_commands,
        responsible_rows = responsible_rows,
    )
}

pub(crate) fn red_approval_responsible_rows_tex(
    entries: &[crate::models::JointResponsibleEntry],
    display: &UnitDisplay,
) -> String {
    let fallback = crate::models::JointResponsibleEntry::default();
    let rows = if entries.is_empty() {
        std::slice::from_ref(&fallback)
    } else {
        entries
    };
    // 三栏定宽、标签由类文件负责；单位名超出栏宽时由 \RedFit 横向压缩，绝不换行。
    rows.iter()
        .map(|entry| {
            format!(
                "\\RedRecordRow{{{}}}{{{}}}{{{}}}",
                tex_escape(&display.abbr(&entry.unit)),
                latex_name(&entry.name),
                tex_escape(&entry.phone),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn meeting_agenda_tex(input: &DraftInput, markdown: &str) -> String {
    let (blocks, block_lines) = parse_markdown_with_lines(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 正文排版与白头件/函稿一致（含标题编号与紧缩合并）；会议议程无附件、无落款。
    let (body, _) = official_letter_sections_to_tex(
        &blocks,
        &block_lines,
        input.profile.style_mode == StyleMode::Compact,
    );
    let security = security_commands(input);

    format!(
        r#"%!TEX program = xelatex
\documentclass[proof,noforcenewpage,meetingagenda]{{gonghan-gwa}}
{security}\renewcommand{{\DocumentTitle}}{{{title}}}
\renewcommand{{\TitleContent}}{{{title_content}}}
\renewcommand{{\MainContent}}{{
{body}
}}
\begin{{document}}
\makeletter
\end{{document}}
"#,
        title = tex_escape(title),
        title_content = title_content_tex(title),
        security = security,
        body = body,
    )
}
