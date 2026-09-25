//! 各文种生成器：白头件、红头呈批件与会议议程。
//!
//! 由 src/export/latex.rs 拆分而来：本文件是模块 `export::latex::papers`，与其它子模块共享
//! `export::latex` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::element_display::{
    addressee_display, date_display_parts, is_preview_version, number_display_parts,
    signing_unit_display,
};
use crate::export::latex::{
    attachment_summary_tex, element_arg, latex_name, marked_tex_escape,
    official_letter_sections_to_tex_with_barrier_with_numbering,
    official_letter_sections_to_tex_with_numbering, red_approval_title_content_tex,
    security_commands, tex_escape, tex_spread_signature, title_content_tex,
};
use crate::export::{MarkdownBlock, parse_markdown_with_lines_with_numbering, plain_text};
use crate::models::{DraftInput, LetterVersion, ListNumbering, NumberingConfig};
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use crate::visual_diff::elements::{DateMarks, FieldMark};

/// 落款单位的多行命令参数：每个单位一行、行间空一行（`\par\vspace{\baselineskip}`）。
/// 带要素标注时按行替换——旧值删除线、新值加框，整行删旧插新；旧版多出来的行
/// 整行画删除线留在纸面。没变的行走原生分散对齐，与从前逐字节一致。
fn signature_units_tex(units: &[String], marks: &[FieldMark]) -> String {
    let mut lines: Vec<String> = units
        .iter()
        .enumerate()
        .map(|(index, unit)| match marks.get(index) {
            Some(mark) if mark.changed() => marked_tex_escape(&mark.marked()),
            _ => tex_spread_signature(unit),
        })
        .collect();
    for mark in marks.iter().skip(units.len()) {
        if mark.changed() {
            lines.push(marked_tex_escape(&mark.marked()));
        }
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index > 0 {
                format!("\\par\\vspace{{\\baselineskip}}{line}")
            } else {
                line
            }
        })
        .collect()
}

/// 成文日期三条命令（带要素标注：变了的部件整体删旧插新）。日期未填时不写
/// 命令，沿用类默认；自由文本日期切不开——原样时不写，变了把整串删旧插新写进
/// `\SignatureYear`（年月日三字仍由类补）。
fn date_commands_tex(input: &DraftInput, elements: &ElementMarks) -> String {
    let preview = is_preview_version(input);
    let placeholder = "\\makebox[1em][c]{}";
    let date_parts = date_display_parts(input);
    if !elements.date().changed() {
        // 要素没变（含定稿导出）：原生三个命令，日期未填 / 切不开时照旧不写。
        let [year, month, day] = date_parts.as_slice() else {
            return String::new();
        };
        let day_native = if preview {
            placeholder.to_string()
        } else {
            tex_escape(day)
        };
        return format!(
            "\\renewcommand{{\\SignatureYear}}{{{}}}\n\\renewcommand{{\\SignatureMonth}}{{{}}}\n\\renewcommand{{\\SignatureDay}}{{{}}}\n",
            tex_escape(year),
            tex_escape(month),
            day_native
        );
    }
    match elements.date() {
        DateMarks::Parts(marks) => {
            let [year, month, day] = date_parts.as_slice() else {
                return String::new();
            };
            let day_native = if preview {
                placeholder.to_string()
            } else {
                tex_escape(day)
            };
            format!(
                "\\renewcommand{{\\SignatureYear}}{{{}}}\n\\renewcommand{{\\SignatureMonth}}{{{}}}\n\\renewcommand{{\\SignatureDay}}{{{}}}\n",
                element_arg(&marks[0], || tex_escape(year)),
                element_arg(&marks[1], || tex_escape(month)),
                element_arg(&marks[2], || day_native),
            )
        }
        DateMarks::Whole(whole) => format!(
            "\\renewcommand{{\\SignatureYear}}{{{}}}\n\\renewcommand{{\\SignatureMonth}}{{}}\n\\renewcommand{{\\SignatureDay}}{{}}\n",
            marked_tex_escape(&whole.marked())
        ),
    }
}

#[allow(dead_code)] // 默认编号的兼容入口，测试使用。
pub(crate) fn white_paper_tex(input: &DraftInput, markdown: &str, display: &UnitDisplay) -> String {
    white_paper_tex_with_numbering(
        input,
        markdown,
        display,
        &NumberingConfig::default(),
        &ElementMarks::default(),
    )
}

pub(crate) fn white_paper_tex_with_numbering(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
) -> String {
    let (blocks, block_lines) = parse_markdown_with_lines_with_numbering(markdown, numbering);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 正文与函稿一致（含标题编号与紧缩合并）；附件区段保留，与函稿同链路落版。
    let (mut body, attachments) = official_letter_sections_to_tex_with_numbering(
        &blocks,
        &block_lines,
        input.profile.style_mode,
        numbering,
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
    let security = security_commands(input, elements.security());
    // 呈报领导（楷体顶格）按人员编码排序、相同职务合并后写入 \Recipient。
    let leaders = addressee_display(input, display);
    // 落款单位：每个单位一行、行间空一行（便于签字），整体右对齐；显示文本
    // 少于 5 字时逐字用 `\hspace*` 分散对齐到 5 字宽，与预览/Word 各端一致。
    let units = signing_unit_display(input, display)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect::<Vec<_>>();
    let signature_unit = signature_units_tex(&units, elements.signing_units());
    // 规格 §3.3：预览版占位区域统一 1em 宽，成文日期“日”留空，与公函一致。
    // 成文日期未填时沿用类默认：年份取当前年、日期留空待填。
    let date_commands = date_commands_tex(input, elements);

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
        leaders = element_arg(elements.recipient(), || tex_escape(&leaders)),
        body = body,
        attachment_command = attachment_command,
        signature_unit = signature_unit,
        date_commands = date_commands,
    )
}

/// 红头呈批件的独立入口；首页框架在后续专用实现中生成。
#[allow(dead_code)] // 默认编号的兼容入口，测试使用。
pub(crate) fn red_head_approval_tex(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
) -> String {
    red_head_approval_tex_with_numbering(
        input,
        markdown,
        display,
        &NumberingConfig::default(),
        &ElementMarks::default(),
    )
}

pub(crate) fn red_head_approval_tex_with_numbering(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
) -> String {
    let (blocks, block_lines) = parse_markdown_with_lines_with_numbering(markdown, numbering);
    let title = blocks
        .iter()
        .find_map(|block| match block {
            MarkdownBlock::Title(title) => Some(title.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 表格与图片不得留在首页：在正文区第一个表格/图片之前插入换页屏障。
    let (mut body, attachments) = official_letter_sections_to_tex_with_barrier_with_numbering(
        &blocks,
        &block_lines,
        input.profile.style_mode,
        Some("\\RedPageOneBarrier"),
        numbering,
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
    let security = security_commands(input, elements.security());
    let leaders = addressee_display(input, display);
    let issuing = display.full_name(&input.profile.issuing_unit);
    let signature_units = signing_unit_display(input, display)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect::<Vec<_>>();
    // 成文日期要居中于“落款单位 + 签字空间”，TeX 量不出 vbox 的自然宽度，
    // 这里按字数算好最宽一行的宽度写进类文件。写成毫米而不是 em：这条
    // \setlength 在导言区执行，那里的字号不是三号。
    // 带要素标注时每行是「旧值 + 新值」并排，宽度得按标注后的文本算，
    // 否则定宽摆位的落款与日期居中会对不上（Word / 预览右对齐，不涉及）。
    let width_units = if elements.signing_units().iter().any(|mark| mark.changed()) {
        elements
            .signing_units()
            .iter()
            .map(|mark| crate::export::strip_redline(&mark.marked()))
            .collect()
    } else {
        signature_units.clone()
    };
    let signature_unit_width_mm = crate::export::red_signature_unit_width_mm(&width_units);
    let signature_unit = signature_units_tex(&signature_units, elements.signing_units());
    let preview = input.profile.letter_version == LetterVersion::Preview;
    let placeholder = "\\makebox[1em][c]{}";
    let (department_code, document_year, document_serial) = number_display_parts(input);
    let document_number = if preview {
        placeholder.to_string()
    } else {
        tex_escape(&document_serial)
    };
    // 发文字号三个部件各自标注（TeX 里是三个命令，中间夹类里写死的〔〕号）。
    let [code_mark, number_year_mark, serial_mark] = elements.number();
    let department_arg = element_arg(code_mark, || tex_escape(&department_code));
    let document_year_arg = element_arg(number_year_mark, || tex_escape(&document_year));
    let number_arg = element_arg(serial_mark, || document_number.clone());
    let date_commands = date_commands_tex(input, elements);
    let entries = crate::models::joint_responsible_entries(&input.profile);
    let record_rows = red_approval_record_display_rows(&entries, display);
    // 三栏宽度按各行实际内容一次算定并注入类文件（与 Word/预览同源）：联系人栏
    // 固定 8 em 永不压缩，电话栏按最长号码定宽，承办单位栏吃版心余量。
    let record_columns = crate::export::red_record_columns(&record_rows);
    let responsible_rows = red_approval_responsible_rows_tex(&record_rows);
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
\setlength{{\RedRecordUnitWidth}}{{{record_unit_width:.3}mm}}
\setlength{{\RedRecordContactWidth}}{{{record_contact_width:.3}mm}}
\setlength{{\RedRecordPhoneWidth}}{{{record_phone_width:.3}mm}}
{date_commands}
\SetRedResponsibleContent{{
{responsible_rows}
}}
\begin{{document}}
\makeredapproval
\end{{document}}
"#,
        issuing = tex_escape(&issuing),
        document_year = document_year_arg,
        department = department_arg,
        number = number_arg,
        security = security,
        title = tex_escape(&title_plain),
        title_content = red_approval_title_content_tex(title),
        leaders = element_arg(elements.recipient(), || tex_escape(&leaders)),
        body = body,
        attachment_command = attachment_command,
        signature_unit = signature_unit,
        signature_unit_width = signature_unit_width_mm,
        record_unit_width = crate::export::RedRecordColumns::mm(record_columns.unit),
        record_contact_width = crate::export::RedRecordColumns::mm(record_columns.contact),
        record_phone_width = crate::export::RedRecordColumns::mm(record_columns.phone),
        date_commands = date_commands,
        responsible_rows = responsible_rows,
    )
}

/// 承办区各行的显示文本（单位简称、姓名、电话）：栏宽算法与 TeX 行生成共用
/// 同一份数据，空稿件回落一行空条目。
fn red_approval_record_display_rows(
    entries: &[crate::models::JointResponsibleEntry],
    display: &UnitDisplay,
) -> Vec<[String; 3]> {
    let fallback = crate::models::JointResponsibleEntry::default();
    let entries = if entries.is_empty() {
        std::slice::from_ref(&fallback)
    } else {
        entries
    };
    entries
        .iter()
        .map(|entry| {
            [
                display.abbr(&entry.unit),
                entry.name.clone(),
                entry.phone.clone(),
            ]
        })
        .collect()
}

/// 承办区 TeX 行：三栏定宽、标签由类文件负责且只出现在首行，续行走
/// `\RedRecordRowCont` 保持取值上下对齐；内容超宽时由 `\RedFit` 横向压缩，绝不换行。
pub(crate) fn red_approval_responsible_rows_tex(rows: &[[String; 3]]) -> String {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let command = if index == 0 {
                "\\RedRecordRow"
            } else {
                "\\RedRecordRowCont"
            };
            format!(
                "{command}{{{}}}{{{}}}{{{}}}",
                tex_escape(&row[0]),
                latex_name(&row[1]),
                tex_escape(&row[2]),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)] // 默认编号的兼容入口，测试使用。
pub(crate) fn meeting_agenda_tex(input: &DraftInput, markdown: &str) -> String {
    meeting_agenda_tex_with_numbering(
        input,
        markdown,
        &NumberingConfig::default(),
        &ElementMarks::default(),
    )
}

pub(crate) fn meeting_agenda_tex_with_numbering(
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
) -> String {
    // 会议议程事项固定使用「1. 2. 3.」阿拉伯数字编号（见起草提示词与校验规则），
    // 不随设置里的列表编号样式变化。
    let mut agenda_numbering = *numbering;
    agenda_numbering.list1 = ListNumbering::DecimalDot;
    agenda_numbering.list2 = ListNumbering::DecimalDot;
    let (blocks, block_lines) =
        parse_markdown_with_lines_with_numbering(markdown, &agenda_numbering);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 正文排版与白头件/函稿一致（含标题编号与紧缩合并）；会议议程无附件、无落款。
    let (body, _) = official_letter_sections_to_tex_with_numbering(
        &blocks,
        &block_lines,
        input.profile.style_mode,
        &agenda_numbering,
    );
    let security = security_commands(input, elements.security());

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
