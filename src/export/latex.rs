use super::{
    MarkdownBlock, MarkdownSection, attachment_names, body_heading_max_level, chinese_date_parts,
    inline_segments, joint_main_column, number_to_chinese, parse_markdown, plain_text,
    table::{requires_landscape, to_longtblr},
    title::{self, TitlePlan},
};
use crate::models::{
    DraftInput, FontConfig, FontRole, JointIssuanceMode, LetterVersion, StyleMode, TemplateKind,
    split_period_digits, split_units,
};
use crate::units::UnitDisplay;
use anyhow::{Context, Result};
use std::{fs, path::Path};

const GONGHAN_CLASS: &str = include_str!("../../gonghan-gwa.cls");
const GONGHAN_CLASS_NAME: &str = "gonghan-gwa";

pub fn write_tex(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
) -> Result<()> {
    let content = match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => {
            official_letter_tex(input, markdown, display)
        }
        TemplateKind::PlainDocument => plain_document_tex(input, markdown),
        TemplateKind::WhitePaper => white_paper_tex(input, markdown, display),
        TemplateKind::MeetingAgenda => meeting_agenda_tex(input, markdown),
    };
    // 选了本机字体才注入钩子；没选时产出的 TeX 与从前逐字节一致。
    let content = match font_setup_hook(fonts) {
        Some(hook) => format!("{hook}{content}"),
        None => content,
    };
    fs::write(path, content).with_context(|| format!("无法写入 TeX 文件：{}", path.display()))?;
    // 五种模板均使用 gonghan-gwa.cls，随 tex 一并输出类文件。
    let class_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{GONGHAN_CLASS_NAME}.cls"));
    let packaged_class = GONGHAN_CLASS.replace("{gonghan}", "{gonghan-gwa}");
    let needs_update = fs::read_to_string(&class_path)
        .map(|existing| existing != packaged_class)
        .unwrap_or(true);
    if needs_update {
        fs::write(&class_path, packaged_class)
            .with_context(|| format!("无法写入 {}", class_path.display()))?;
    }
    Ok(())
}

/// 字体名里可能出现的、会被 TeX 当成控制字符的符号。字体名本身不该有这些，
/// 出现了也只可能是配置被手工改坏，直接剔除而不是让编译在别处报错。
fn sanitize_font_name(name: &str) -> String {
    name.chars()
        .filter(|ch| {
            !matches!(
                ch,
                '\\' | '{' | '}' | '%' | '#' | '$' | '&' | '~' | '^' | '_'
            )
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// 生成注入到 `\documentclass` 之前的字体设置钩子。类文件见到
/// `\GwaFontSetupHook` 有定义就改用它，顶替内置字体那段默认设置。
///
/// 钩子内部保留按名字、按文件两条分支：内置 Tectonic 编译时用拷进临时目录的
/// 字体文件，导出的 `.tex` 拿到别的机器上编译时则按字体名加载。没有配置的位置
/// 沿用内置字体，行为与不注入钩子时一致。
fn font_setup_hook(fonts: &FontConfig) -> Option<String> {
    if !fonts.any_active() {
        return None;
    }
    let file = |role: FontRole| match fonts.active(role) {
        Some(choice) => choice.compiled_file_name(role),
        None => role.bundled_file().to_string(),
    };
    let name = |role: FontRole| match fonts.active(role) {
        Some(choice) => sanitize_font_name(&choice.family),
        None => role.bundled_family().to_string(),
    };

    let (title_name, title_file) = (name(FontRole::Title), file(FontRole::Title));
    let (heiti_name, heiti_file) = (name(FontRole::Heading1), file(FontRole::Heading1));
    let (kai_name, kai_file) = (name(FontRole::Heading2), file(FontRole::Heading2));
    let (body_name, body_file) = (name(FontRole::Body), file(FontRole::Body));
    let (song_name, song_file) = (name(FontRole::PageNumber), file(FontRole::PageNumber));

    // 方正小标宋没有统一的字体名，未指定标题字体时沿用类文件的两段探测。
    let title_by_name = if title_name.is_empty() {
        r"        \IfFontExistsTF{FZXiaoBiaoSong-B05}{%
            \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05}%
            \newfontfamily\enbt{FZXiaoBiaoSong-B05}%
        }{%
            \IfFontExistsTF{FZXiaoBiaoSong-B05S}{%
                \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05S}%
                \newfontfamily\enbt{FZXiaoBiaoSong-B05S}%
            }{%
                \ClassError{gonghan-gwa}{未找到方正小标宋字体}{请安装 FZXiaoBiaoSong-B05 或 FZXiaoBiaoSong-B05S}%
            }%
        }%"
            .to_string()
    } else {
        format!(
            r"        \setCJKfamilyfont{{xbs}}{{{title_name}}}%
        \newfontfamily\enbt{{{title_name}}}%"
        )
    };

    Some(format!(
        r"\makeatletter
\def\GwaFontSetupHook{{%
    \ifx\GwaFontPath\@empty
        \setCJKmainfont[ItalicFont={{{kai_name}}}, AutoFakeBold=true]{{{body_name}}}%
{title_by_name}
        \setCJKfamilyfont{{kaiti}}[AutoFakeBold=true]{{{kai_name}}}%
        \setCJKfamilyfont{{songti}}{{{song_name}}}%
        \newfontfamily\ennumber{{{song_name}}}%
        \newfontfamily\ensong{{{song_name}}}%
        \newfontfamily\enheiti{{{heiti_name}}}%
        \setCJKfamilyfont{{heiti}}{{{heiti_name}}}%
        \setCJKfamilyfont{{fangsong}}[AutoFakeBold=true]{{{body_name}}}%
        \setmainfont[ItalicFont={{{kai_name}}}]{{{body_name}}}%
        \setCJKmonofont{{{heiti_name}}}%
        \setmonofont{{{heiti_name}}}%
    \else
        \setCJKmainfont[Path={{\GwaFontPath}}, ItalicFont={{{kai_file}}}, AutoFakeBold=true]{{{body_file}}}%
        \setCJKfamilyfont{{xbs}}[Path={{\GwaFontPath}}]{{{title_file}}}%
        \newfontfamily\enbt[Path={{\GwaFontPath}}]{{{title_file}}}%
        \setCJKfamilyfont{{kaiti}}[Path={{\GwaFontPath}}, AutoFakeBold=true]{{{kai_file}}}%
        \setCJKfamilyfont{{songti}}[Path={{\GwaFontPath}}]{{{song_file}}}%
        \newfontfamily\ennumber[Path={{\GwaFontPath}}]{{{song_file}}}%
        \newfontfamily\ensong[Path={{\GwaFontPath}}]{{{song_file}}}%
        \newfontfamily\enheiti[Path={{\GwaFontPath}}]{{{heiti_file}}}%
        \setCJKfamilyfont{{heiti}}[Path={{\GwaFontPath}}]{{{heiti_file}}}%
        \setCJKfamilyfont{{fangsong}}[Path={{\GwaFontPath}}, AutoFakeBold=true]{{{body_file}}}%
        \setmainfont[Path={{\GwaFontPath}}, ItalicFont={{{kai_file}}}]{{{body_file}}}%
        \setCJKmonofont[Path={{\GwaFontPath}}]{{{heiti_file}}}%
        \setmonofont[Path={{\GwaFontPath}}]{{{heiti_file}}}%
    \fi
}}
\makeatother
"
    ))
}

fn official_letter_tex(input: &DraftInput, markdown: &str, display: &UnitDisplay) -> String {
    let blocks = parse_markdown(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    let (mut body, attachments) =
        official_letter_sections_to_tex(&blocks, input.profile.style_mode == StyleMode::Compact);
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
    let (signature_year, signature_month, signature_day) =
        chinese_date_parts(&input.date).unwrap_or(("", "", ""));
    let document_year = input.document_year();
    let preview = input.profile.letter_version == LetterVersion::Preview;
    // 规格 §3.3：预览版所有占位区域统一 1em 宽。
    let preview_placeholder = "\\makebox[1em][c]{}";
    let document_number = if preview {
        preview_placeholder.to_string()
    } else {
        tex_escape(&input.profile.document_number)
    };
    let signature_day = if preview {
        preview_placeholder.to_string()
    } else {
        tex_escape(signature_day)
    };
    let duplex_option = if input.profile.duplex_printing {
        ",duplex"
    } else {
        ""
    };
    let phone_notice_option = if input.kind == TemplateKind::PhoneNotice {
        ",phonenotice"
    } else {
        ""
    };
    let joint_mode_one = input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1;
    let joint_option = if joint_mode_one { ",jointmodeone" } else { "" };
    let joint_commands = if joint_mode_one {
        joint_mode_one_commands(
            input,
            signature_year,
            signature_month,
            &signature_day,
            display,
        )
    } else {
        String::new()
    };
    let issuing_unit = if joint_mode_one {
        let units = split_units(&input.profile.joint_issuing_units);
        let main = input.profile.main_issuing_unit.trim();
        let chosen = if units.iter().any(|unit| unit == main) {
            main.to_string()
        } else {
            units.first().cloned().unwrap_or_default()
        };
        display.full_name_for(&chosen, input.uses_external_unit_names())
    } else {
        display.full_name_for(
            &input.profile.issuing_unit,
            input.uses_external_unit_names(),
        )
    };

    // 规格 §2.2/3.1：红头、主送、抄送用层级展开全称；版记承办单位用简称；
    // 落款：公函用全称、电话通知用简称（少于 5 字逐字加空格）。
    let recipient_display = display.join_hierarchical_for(
        &split_units(&input.profile.recipient),
        input.uses_external_unit_names(),
    );
    let copies_display = display.join_hierarchical_for(
        &split_units(&input.profile.copies_to),
        input.uses_external_unit_names(),
    );
    let responsible_display = if joint_mode_one {
        split_units(&input.profile.joint_responsible_units)
            .iter()
            .map(|unit| display.abbr(unit))
            .collect::<Vec<_>>()
            .join("、")
    } else {
        display.abbr(&input.profile.responsible_unit)
    };
    let signature_display = {
        let raw = if input.profile.signing_unit.trim().is_empty() {
            &input.profile.issuing_unit
        } else {
            &input.profile.signing_unit
        };
        if input.kind == TemplateKind::PhoneNotice {
            display.abbr_spaced(raw)
        } else {
            display.full_name_for(raw, input.uses_external_unit_names())
        }
    };

    format!(
        r#"%!TEX program = xelatex
\documentclass[noforcenewpage,autocalc{duplex_option}{phone_notice_option}{joint_option}]{{gonghan-gwa}}
\renewcommand{{\IssuingUnit}}{{{issuing}}}
\renewcommand{{\Year}}{{{document_year}}}
\renewcommand{{\DepartmentCode}}{{{department}}}
\renewcommand{{\DocumentNumber}}{{{number}}}
{security}\renewcommand{{\DocumentTitle}}{{{title}}}
\renewcommand{{\TitleContent}}{{{title_content}}}
\renewcommand{{\Recipient}}{{{recipient}}}
\renewcommand{{\MainContent}}{{
{body}
}}
{attachment_command}\renewcommand{{\SignatureUnit}}{{{signature_unit}}}
\renewcommand{{\SignatureSealOnBehalf}}{{{seal_on_behalf}}}
\renewcommand{{\SignatureYear}}{{{year}}}
\renewcommand{{\SignatureMonth}}{{{month}}}
\renewcommand{{\SignatureDay}}{{{day}}}
\renewcommand{{\CopiesTo}}{{{copies}}}
\renewcommand{{\ResponsibleUnit}}{{{responsible}}}
\renewcommand{{\ContactPerson}}{{{contact}}}
\renewcommand{{\ContactPhone}}{{{phone}}}
{joint_commands}
\begin{{document}}
\makeletter
\end{{document}}
"#,
        issuing = tex_escape(&issuing_unit),
        document_year = tex_escape(&document_year),
        year = tex_escape(signature_year),
        month = tex_escape(signature_month),
        department = tex_escape(&input.profile.department_code),
        number = document_number,
        security = security,
        title = tex_escape(title),
        title_content = title_content_tex(title),
        recipient = tex_escape(&recipient_display),
        body = body,
        attachment_command = attachment_command,
        signature_unit = if input.kind == TemplateKind::PhoneNotice {
            tex_spaced(&signature_display)
        } else {
            tex_escape(&signature_display)
        },
        seal_on_behalf = if crate::export::seals_on_behalf(input, display) {
            tex_escape("（代章）")
        } else {
            String::new()
        },
        day = signature_day,
        copies = tex_escape(&copies_display),
        responsible = tex_escape(&responsible_display),
        contact = latex_name(&input.profile.contact_person),
        phone = tex_escape(&input.profile.contact_phone),
        duplex_option = duplex_option,
        phone_notice_option = phone_notice_option,
        joint_option = joint_option,
        joint_commands = joint_commands,
    )
}

/// 普通公文只输出密级、标题、正文、附件和页码设置，不把其他模板元数据写入 TeX。
fn plain_document_tex(input: &DraftInput, markdown: &str) -> String {
    let blocks = parse_markdown(markdown);
    let title = blocks
        .iter()
        .find_map(|block| match block {
            MarkdownBlock::Title(title) => Some(title.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    let (mut body, attachments) =
        official_letter_sections_to_tex(&blocks, input.profile.style_mode == StyleMode::Compact);
    if let Some(summary) = attachment_summary_tex(&blocks) {
        body.push_str(&summary);
    }
    let attachment_command = if attachments.trim().is_empty() {
        String::new()
    } else {
        format!("\\SetAttachmentContent{{\n{attachments}\n}}\n")
    };
    let duplex_option = if input.profile.duplex_printing {
        ",duplex"
    } else {
        ""
    };

    format!(
        r#"%!TEX program = xelatex
\documentclass[noforcenewpage,autocalc{duplex_option},plaindocument]{{gonghan-gwa}}
{security}\renewcommand{{\DocumentTitle}}{{{title}}}
\renewcommand{{\TitleContent}}{{{title_content}}}
\renewcommand{{\MainContent}}{{
{body}
}}
{attachment_command}\begin{{document}}
\makeletter
\end{{document}}
"#,
        security = security_commands(input),
        title = tex_escape(title),
        title_content = title_content_tex(title),
    )
}

fn joint_mode_one_commands(
    input: &DraftInput,
    year: &str,
    month: &str,
    day: &str,
    display: &UnitDisplay,
) -> String {
    let units = split_units(&input.profile.joint_issuing_units);
    let row_count = units.len().div_ceil(2).max(1);
    // 规格 §2.5：超过两个单位且为奇数时，最后一个单位跨两列居中，而不是落在左列。
    let odd_last = units.len() > 2 && units.len() % 2 == 1;
    // 代章直接跟在主发文单位后面，不另起一行。
    let main_index = crate::export::joint_seal_index(input, display);
    let mut signature_rows = Vec::new();
    for (row_index, pair) in units.chunks(2).enumerate() {
        let base_index = row_index * 2;
        let mut left = display.full_name_for(
            pair.first().map_or("", String::as_str),
            input.uses_external_unit_names(),
        );
        let mut right = display.full_name_for(
            pair.get(1).map_or("", String::as_str),
            input.uses_external_unit_names(),
        );
        if Some(base_index) == main_index {
            left.push_str("（代章）");
        }
        if Some(base_index + 1) == main_index {
            right.push_str("（代章）");
        }
        let gap = if units.len() > 2 && row_index + 1 < row_count {
            "[45mm]"
        } else {
            ""
        };
        if odd_last && row_index + 1 == row_count {
            signature_rows.push(format!(
                "\\multicolumn{{2}}{{@{{}}c@{{}}}}{{{}}} \\\\{gap}",
                tex_escape(&left)
            ));
        } else {
            signature_rows.push(format!(
                "{} & {} \\\\{gap}",
                tex_escape(&left),
                tex_escape(&right)
            ));
        }
    }
    let responsible = split_units(&input.profile.joint_responsible_units);
    let contacts = &input.profile.joint_contacts;
    let record_rows = responsible.len().max(contacts.len()).max(1);
    let mut record_lines = Vec::new();
    for index in 0..record_rows {
        let contact = contacts.get(index);
        let responsible_name = display.abbr(responsible.get(index).map_or("", String::as_str));
        let contact_name = contact.map_or("", |value| value.name.as_str());
        // 规格 §3.2 版记对齐：第 2 行起承办单位名称前加 5em、联系人前加 4em 占位，
        // 使后续行的名称与第一行标签后的位置对齐。用 \hspace* 而非连续 \quad，
        // 规避个别 TeX 发行版对单元格起始处多个 \quad 的解析问题。
        let unit_pad = if index == 0 { "" } else { "\\hspace*{5em}" };
        let contact_pad = if index == 0 { "" } else { "\\hspace*{4em}" };
        record_lines.push(format!(
            "{}{}{} & {}{}{} & {}{} \\\\",
            if index == 0 { "承办单位：" } else { "" },
            unit_pad,
            tex_escape(&responsible_name),
            if index == 0 { "联系人：" } else { "" },
            contact_pad,
            latex_name(contact_name),
            if index == 0 { "联系电话：" } else { "" },
            tex_escape(contact.map_or("", |value| value.phone.as_str())),
        ));
    }
    // 落款需要多高由类在排版时装箱量出（见 cls 的 \gwa@placeclosing），
    // 这里不再按单位行数估算预留高度。
    // 日期压在主发文单位所在列下方，而不是整块居中；主单位跨列时整行居中。
    // 列内排法用第二个 72mm 双列表格；跨列时直接排一行（\multicolumn 里再放 \\ 会被
    // 外层 tabular 当作行结束符，故跨列情形不套表格）。
    let closing_content = format!("{year}年{month}月{day}日");
    let closing = match joint_main_column(input) {
        Some(0) => format!(
            r"\begin{{tabular}}{{@{{}}>{{\centering\arraybackslash}}p{{72mm}}>{{\centering\arraybackslash}}p{{72mm}}@{{}}}}
{closing_content} & \\
\end{{tabular}}"
        ),
        Some(1) => format!(
            r"\begin{{tabular}}{{@{{}}>{{\centering\arraybackslash}}p{{72mm}}>{{\centering\arraybackslash}}p{{72mm}}@{{}}}}
& {closing_content} \\
\end{{tabular}}"
        ),
        _ => closing_content,
    };
    format!(
        r#"\SetJointSignatureContent{{%
\begin{{center}}
\renewcommand{{\arraystretch}}{{1}}
\begin{{tabular}}{{@{{}}>{{\centering\arraybackslash}}p{{72mm}}>{{\centering\arraybackslash}}p{{72mm}}@{{}}}}
{signature_rows}
\end{{tabular}}
\par\vspace{{6mm}}
{closing}
\end{{center}}
}}
\SetJointFooterRecord{{%
\par
\vspace*{{\fill}}
\setlength{{\parindent}}{{0pt}}
\arrayrulewidth=1pt
\setlength{{\FooterRecordWidth}}{{\linewidth}}
\setlength{{\tabcolsep}}{{0pt}}
\renewcommand{{\arraystretch}}{{1.15}}
\zihao{{4}}
\noindent%
\begin{{tabularx}}{{\FooterRecordWidth}}{{@{{}}>{{\raggedright\arraybackslash}}X>{{\centering\arraybackslash}}X>{{\raggedleft\arraybackslash}}p{{11em}}@{{}}}}
\toprule[0.6mm]
\multicolumn{{3}}{{@{{}}p{{\FooterRecordWidth}}@{{}}}}{{\FooterCopiesLine{{}}}} \\ \midrule[0.3mm]
{record_lines}
\bottomrule[0.6mm]
\end{{tabularx}}\par
}}"#,
        signature_rows = signature_rows.join("\n"),
        closing = closing,
        record_lines = record_lines.join("\n"),
    )
}

fn official_letter_sections_to_tex(blocks: &[MarkdownBlock], compact: bool) -> (String, String) {
    let mut body = Vec::new();
    let mut attachments = Vec::new();
    let landscape_attachments = attachment_landscape_flags(blocks);
    let mut section = MarkdownSection::Body;
    let mut seen_document_title = false;
    let mut attachment_title_count = 0usize;
    let mut current_attachment_is_landscape = false;
    let mut counters = [0usize; 4];
    // 紧缩风格合并正文区 # 号最多的那一级标题；附件区标题不计入。
    let compact_heading_level = body_heading_max_level(blocks);

    let mut index = 0usize;
    while index < blocks.len() {
        let block = &blocks[index];
        match block {
            MarkdownBlock::Title(text) if !seen_document_title => {
                seen_document_title = true;
            }
            MarkdownBlock::Title(text) => {
                section = MarkdownSection::Attachment;
                counters = [0; 4];
                if attachment_title_count > 0 {
                    if current_attachment_is_landscape {
                        attachments.push("\\end{landscape}".to_string());
                    } else {
                        attachments.push("\\clearpage".to_string());
                    }
                }
                current_attachment_is_landscape = landscape_attachments
                    .get(attachment_title_count)
                    .copied()
                    .unwrap_or(false);
                attachment_title_count += 1;
                if current_attachment_is_landscape {
                    attachments.push("\\begin{landscape}".to_string());
                }
                attachments.push(format!(
                    "\\noindent{{\\xeCJKsetup{{CJKecglue={{\\hskip0pt}}}}\\heiti\\enheiti\\zihao{{3}} {}}}\\par",
                    tex_escape(&plain_text(text))
                ));
            }
            MarkdownBlock::Marker(next) => {
                section = *next;
                counters = [0; 4];
            }
            MarkdownBlock::Heading(level, text) => {
                // 紧缩风格（规格 §4.2）：正文区 # 号最多的那一级标题后紧跟正文段落时，
                // 合并为“一、任务目标。测试正文”，标题部分用该级标题字体、正文用正文字体。
                let next_is_paragraph = blocks.get(index + 1).is_some_and(|next| {
                    matches!(next, MarkdownBlock::Paragraph(p)
                        if !p.trim().is_empty()
                            && !p.contains("<div")
                            && !p.contains("</div"))
                });
                if compact
                    && section == MarkdownSection::Body
                    && *level == compact_heading_level
                    && next_is_paragraph
                    && let Some(number) = heading_number_prefix(*level, &mut counters)
                {
                    let MarkdownBlock::Paragraph(body_text) = &blocks[index + 1] else {
                        unreachable!()
                    };
                    let heading_escaped = tex_escape(&plain_text(text));
                    let body_escaped = body_text_to_tex(body_text);
                    // 标题段采用与独立标题一致的层级字体：2 级黑体、3 级楷体、4 级仿宋、5 级黑体加粗。
                    let title_tex = match *level {
                        2 => format!("\\heiti {number}{heading_escaped}。"),
                        3 => format!("\\kai {number}{heading_escaped}。"),
                        4 => format!("{number}{heading_escaped}。"),
                        5 => format!("\\textbf{{{number}{heading_escaped}。}}"),
                        _ => unreachable!(),
                    };
                    target_tex_section(section, &mut body, &mut attachments).push(format!(
                        "\\noindent\\hspace*{{2em}}{{{title_tex}}}{body_escaped}\\par"
                    ));
                    index += 1; // 跳过紧随的正文段落
                } else {
                    let rendered = match section {
                        MarkdownSection::Body => {
                            official_heading_to_tex(*level, text, &mut counters)
                        }
                        MarkdownSection::Attachment => {
                            attachment_heading_to_tex(*level, text, &mut counters)
                        }
                    };
                    if let Some(rendered) = rendered {
                        target_tex_section(section, &mut body, &mut attachments).push(rendered);
                    }
                }
            }
            MarkdownBlock::Paragraph(text) => {
                if !text.contains("<div") && !text.contains("</div") {
                    target_tex_section(section, &mut body, &mut attachments)
                        .push(format!("{}\\par", body_text_to_tex(text)));
                }
            }
            MarkdownBlock::ListItem(text) => {
                target_tex_section(section, &mut body, &mut attachments)
                    .push(format!("\\noindent {}\\par", body_text_to_tex(text)));
            }
            MarkdownBlock::Table(rows) => {
                let rendered = to_longtblr(rows);
                if !rendered.is_empty() {
                    target_tex_section(section, &mut body, &mut attachments).push(rendered);
                }
            }
            MarkdownBlock::Html(_) => {}
        }
        index += 1;
    }

    if current_attachment_is_landscape {
        attachments.push("\\end{landscape}".to_string());
    }

    (body.join("\n\n"), attachments.join("\n\n"))
}

/// 每个附件只要有一张表在竖页中横向过密，就将整个附件（而非仅表格）改为横页。
fn attachment_landscape_flags(blocks: &[MarkdownBlock]) -> Vec<bool> {
    let mut flags = Vec::new();
    let mut seen_document_title = false;
    let mut current_attachment = None;

    for block in blocks {
        match block {
            MarkdownBlock::Title(_) if !seen_document_title => seen_document_title = true,
            MarkdownBlock::Title(_) => {
                flags.push(false);
                current_attachment = Some(flags.len() - 1);
            }
            MarkdownBlock::Marker(MarkdownSection::Body) => current_attachment = None,
            MarkdownBlock::Table(rows) => {
                if let Some(index) = current_attachment
                    && requires_landscape(rows)
                {
                    flags[index] = true;
                }
            }
            _ => {}
        }
    }
    flags
}

fn attachment_heading_to_tex(level: u8, text: &str, counters: &mut [usize; 4]) -> Option<String> {
    if level == 2 {
        counters.fill(0);
        return Some(format!(
            // 附件标识位于第一行，正式标题置于第三行；用固定正文行距留出第二行，
            // 不使用 center 环境自带的可伸缩 topsep。
            "\\vspace{{\\BodyBaselineSkip}}\n{{\\centering\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {}\\par}}",
            tex_escape(&plain_text(text))
        ));
    }
    level
        .checked_sub(1)
        .and_then(|body_level| official_heading_to_tex(body_level, text, counters))
}

fn target_tex_section<'a>(
    section: MarkdownSection,
    body: &'a mut Vec<String>,
    attachments: &'a mut Vec<String>,
) -> &'a mut Vec<String> {
    match section {
        MarkdownSection::Body => body,
        MarkdownSection::Attachment => attachments,
    }
}

/// 推进标题计数器并生成该层级的编号前缀（如“一、”“（一）”“1.”“(1)”）。
/// 返回 None 表示该层级不支持编号，与 `official_heading_to_tex` 一致。
fn heading_number_prefix(level: u8, counters: &mut [usize; 4]) -> Option<String> {
    match level {
        2 => {
            counters[0] += 1;
            counters[1..].fill(0);
            Some(format!("{}、", number_to_chinese(counters[0])))
        }
        3 => {
            counters[1] += 1;
            counters[2..].fill(0);
            Some(format!("（{}）", number_to_chinese(counters[1])))
        }
        4 => {
            counters[2] += 1;
            counters[3] = 0;
            Some(format!("{}.", counters[2]))
        }
        5 => {
            counters[3] += 1;
            Some(format!("({})", counters[3]))
        }
        _ => None,
    }
}

fn official_heading_to_tex(level: u8, text: &str, counters: &mut [usize; 4]) -> Option<String> {
    let escaped = tex_escape(&plain_text(text));
    let number = heading_number_prefix(level, counters)?;
    let rendered = match level {
        2 => format!("\\noindent\\hspace*{{2em}}{{\\heiti {number}{escaped}}}\\par"),
        3 => format!("\\noindent\\hspace*{{2em}}{{\\kai {number}{escaped}}}\\par"),
        4 => format!("\\noindent\\hspace*{{2em}}{number}{escaped}\\par"),
        5 => format!("\\noindent\\hspace*{{2em}}\\textbf{{{number}{escaped}}}\\par"),
        _ => return None,
    };
    Some(rendered)
}

fn white_paper_tex(input: &DraftInput, markdown: &str, display: &UnitDisplay) -> String {
    let blocks = parse_markdown(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 正文与函稿一致（含标题编号与紧缩合并）；白头件无附件，附件区段不落版。
    let (body, _) =
        official_letter_sections_to_tex(&blocks, input.profile.style_mode == StyleMode::Compact);
    let security = security_commands(input);
    // 呈报领导（楷体顶格）按人员编码排序、相同职务合并后写入 \Recipient。
    let leaders = display
        .reporting_leaders(&input.profile.reporting_leaders)
        .trim()
        .trim_end_matches('：')
        .to_string();
    // 落款单位：优先落款单位，留空则同呈报单位。
    let signature_unit = {
        let raw = if input.profile.signing_unit.trim().is_empty() {
            &input.profile.issuing_unit
        } else {
            &input.profile.signing_unit
        };
        display.full_name(raw)
    };
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
\documentclass[noforcenewpage,whitepaper]{{gonghan-gwa}}
{security}\renewcommand{{\DocumentTitle}}{{{title}}}
\renewcommand{{\TitleContent}}{{{title_content}}}
\renewcommand{{\Recipient}}{{{leaders}}}
\renewcommand{{\MainContent}}{{
{body}
}}
\renewcommand{{\SignatureUnit}}{{{signature_unit}}}
{date_commands}\begin{{document}}
\makeletter
\end{{document}}
"#,
        title = tex_escape(title),
        title_content = title_content_tex(title),
        security = security,
        leaders = tex_escape(&leaders),
        body = body,
        signature_unit = tex_escape(&signature_unit),
        date_commands = date_commands,
    )
}

fn meeting_agenda_tex(input: &DraftInput, markdown: &str) -> String {
    let blocks = parse_markdown(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    // 正文排版与白头件/函稿一致（含标题编号与紧缩合并）；会议议程无附件、无落款。
    let (body, _) =
        official_letter_sections_to_tex(&blocks, input.profile.style_mode == StyleMode::Compact);
    let security = security_commands(input);

    format!(
        r#"%!TEX program = xelatex
\documentclass[noforcenewpage,meetingagenda]{{gonghan-gwa}}
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

/// 规格 §3.2/§6 姓名宽度处理：2 字姓名中间加 1em 空格，4 字姓名压缩到 3 字宽，
/// 保证版记联系人列与表格姓名列在视觉上整齐对齐。
fn latex_name(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    match chars.len() {
        2 => format!("{}\\hspace{{1em}}{}", chars[0], chars[1]),
        4 => format!("\\resizebox{{3em}}{{0.9em}}{{{}}}", tex_escape(value)),
        _ => tex_escape(value),
    }
}

fn tex_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '%' => out.push_str("\\%"),
            '&' => out.push_str("\\&"),
            '_' => out.push_str("\\_"),
            '^' => out.push_str("\\textasciicircum{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            _ => out.push(ch),
        }
    }
    out
}

/// 正文段转 TeX：Markdown 加粗转为 `\textbf`，由字体的 AutoFakeBold 实现；
/// 完整圆括号/方头括号及其中内容用四号楷体，其余保持正文三号仿宋。
/// 标题（文档标题、各级标题、附件标签）不经由此处，不受此规则影响。
/// 括号部分用花括号限定 `\kai\zihao{4}` 的作用域，闭合后自动回到正文三号仿宋。
fn body_text_to_tex(text: &str) -> String {
    let mut out = String::new();
    for segment in inline_segments(text) {
        let mut content = tex_escape(&segment.text);
        if segment.bold {
            content = format!("\\textbf{{{content}}}");
        }
        if segment.parenthesized {
            out.push_str(&format!("{{\\kai\\zihao{{4}} {content}}}"));
        } else {
            out.push_str(&content);
        }
    }
    out
}

/// 生成密级相关命令：密级、保密期限，以及“指人专办”标记（勾选后非空）。
/// 数字年限的保密期限把前导数字用 `\ttfamily` 排成等宽，如 `{\ttfamily 10}年`。
fn security_commands(input: &DraftInput) -> String {
    if input.profile.security_level.trim().is_empty() {
        return String::new();
    }
    let special = if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        "指人专办"
    } else {
        ""
    };
    let (digits, rest) = split_period_digits(&input.profile.security_period);
    let period = if digits.is_empty() {
        tex_escape(&input.profile.security_period)
    } else {
        format!("{{\\ttfamily {digits}}}{}", tex_escape(rest))
    };
    format!(
        "\\renewcommand{{\\SecurityLevel}}{{{}}}\n\\renewcommand{{\\SecurityPeriod}}{{{}}}\n\\renewcommand{{\\SpecialHandling}}{{{}}}\n",
        tex_escape(&input.profile.security_level),
        period,
        tex_escape(special)
    )
}

/// 附件概要：正文结束后、落款之前，与正文之间空两行、首行缩进两个汉字，
/// 按顺序列出附件名称。单个附件写“附件：名称”，多个附件写“附件1：名称”“附件2：名称”…。
fn attachment_summary_tex(blocks: &[MarkdownBlock]) -> Option<String> {
    let names = attachment_names(blocks);
    if names.is_empty() {
        return None;
    }
    let mut out = String::new();
    // 与正文之间空两行。
    out.push_str("\\vspace{2\\baselineskip}\n");
    for (index, name) in names.iter().enumerate() {
        let label = if names.len() == 1 {
            format!("附件：{name}")
        } else {
            format!("附件{}：{name}", index + 1)
        };
        out.push_str(&format!(
            "\\noindent\\hspace*{{2em}}{}\\par",
            body_text_to_tex(&label)
        ));
    }
    Some(out)
}

/// 标题内容 TeX：按标题字数与 jieba 排布。
/// 单行保持二号；超出一行不超过 2 字用 `\scalebox` 只缩横向、字高不变；
/// 超出更多在词边界均衡换行（`\\` 分段）。
fn title_content_tex(title: &str) -> String {
    let plain = plain_text(title);
    match title::title_plan(&plain, title::chars_per_line()) {
        TitlePlan::SingleLine => {
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {}}}",
                tex_escape(&plain)
            )
        }
        TitlePlan::Compressed => {
            // \scalebox{横向}[1] 只压缩字形宽度，纵向保持 1，即字高不变。
            let scale = title::compressed_scale_percent(&plain);
            let scale_f = scale as f64 / 100.0;
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}}\\scalebox{{{scale_f}}}[1]{{{}}}}}",
                tex_escape(&plain)
            )
        }
        TitlePlan::Wrapped(lines) => {
            let mut body = String::new();
            for (index, line) in lines.iter().enumerate() {
                if index > 0 {
                    body.push_str("\\\\");
                }
                body.push_str(&tex_escape(line));
            }
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {body}}}"
            )
        }
    }
}

/// LaTeX 中普通空格会被忽略或压缩，无法表达逐字间距；
/// 电话通知落款须把半角空格改写为受控空格命令 `\ `（每个空格独立有效）。
/// 必须在 `tex_escape` 之后替换，否则 `\` 会被转义为 `\textbackslash{}`。
fn tex_spaced(value: &str) -> String {
    tex_escape(value).replace(' ', "\\ ")
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::models::{JointContact, VocabularyCategory, VocabularyEntry};

    /// 测试大多使用无层级的扁平单位，空词库让 `UnitDisplay` 回落为规范名称。
    fn letter_tex(input: &DraftInput, markdown: &str) -> String {
        official_letter_tex(input, markdown, &UnitDisplay::new(&[]))
    }

    #[test]
    fn escapes_latex_reserved_chars() {
        assert_eq!(tex_escape("A&B_1%"), "A\\&B\\_1\\%");
    }

    #[test]
    fn meeting_agenda_tex_uses_letter_class_with_security_and_no_signature() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.profile.security_level = "机密".into();
        input.profile.security_period = "10年".into();
        input.title_hint = "重点任务联合研商会议议程".into();
        let tex = meeting_agenda_tex(
            &input,
            "# 重点任务联合研商会议议程\n\n一、时间地点：2026年8月5日（星期三）14:30，3C会议室。\n\n1. 张三同志汇报总体思路；",
        );
        // 与白头件/函稿共用 gonghan-gwa.cls，走 meetingagenda 选项。
        assert!(tex.contains("\\documentclass[noforcenewpage,meetingagenda]{gonghan-gwa}"));
        assert!(tex.contains("\\renewcommand{\\SecurityLevel}{机密}"));
        assert!(tex.contains("\\renewcommand{\\SecurityPeriod}{{\\ttfamily 10}年}"));
        assert!(tex.contains("\\renewcommand{\\DocumentTitle}{重点任务联合研商会议议程}"));
        // 无落款单位、无日期、无呈报领导命令。
        assert!(!tex.contains("\\SignatureUnit"));
        assert!(!tex.contains("\\SignatureYear"));
        assert!(
            !tex.contains("\\Recipient"),
            "会议议程不应有呈报领导/主送机关：{tex}"
        );
        // 正文沿用函稿/白头件渲染；完整括号及内容改用四号楷体。
        assert!(
            tex.contains(
                "一、时间地点：2026年8月5日{\\kai\\zihao{4} （星期三）}14:30，3C会议室。\\par"
            ),
            "{tex}"
        );
    }

    #[test]
    fn meeting_agenda_tex_honors_compact_style() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::MeetingAgenda;
        input.profile.style_mode = StyleMode::Compact;
        let tex = meeting_agenda_tex(&input, "# 标题\n\n## 任务目标\n测试正文。");
        assert!(tex.contains("{\\heiti 一、任务目标。}测试正文。\\par"));
        input.profile.style_mode = StyleMode::Normal;
        let tex = meeting_agenda_tex(&input, "# 标题\n\n## 任务目标\n测试正文。");
        assert!(tex.contains("{\\heiti 一、任务目标}\\par"));
    }

    #[test]
    fn gonghan_class_defines_whitepaper_and_meetingagenda_options() {
        assert!(GONGHAN_CLASS.contains("\\DeclareOption{whitepaper}"));
        assert!(GONGHAN_CLASS.contains("\\DeclareOption{meetingagenda}"));
        assert!(GONGHAN_CLASS.contains("\\WhitePaperHeader"));
        assert!(GONGHAN_CLASS.contains("\\WhitePaperSignature"));
        assert!(GONGHAN_CLASS.contains("\\MeetingAgendaHeader"));
        assert!(
            GONGHAN_CLASS.contains("\\vspace{10\\baselineskip}"),
            "白头件密级后应空 10 行"
        );
        assert!(
            GONGHAN_CLASS.contains("\\vspace{\\baselineskip}"),
            "会议议程密级后应空 1 行"
        );
    }

    #[test]
    fn white_paper_tex_uses_letter_class_with_security_leaders_and_left_signature() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "5年".into();
        input.profile.reporting_leaders = "张三、李四".into();
        input.profile.signing_unit = "办公室".into();
        input.date = "2026年8月5日".into();
        input.profile.style_mode = StyleMode::Compact;
        let tex = white_paper_tex(
            &input,
            "# 呈批件标题\n\n## 一、任务目标\n测试正文。",
            &UnitDisplay::new(&[]),
        );
        // 与函稿共用 gonghan-gwa.cls，走 whitepaper 选项。
        assert!(tex.contains("\\documentclass[noforcenewpage,whitepaper]{gonghan-gwa}"));
        // 顶格密级由类渲染；此处注入密级与保密期限。
        assert!(tex.contains("\\renewcommand{\\SecurityLevel}{秘密}"));
        assert!(tex.contains("\\renewcommand{\\SecurityPeriod}{{\\ttfamily 5}年}"));
        // 呈报领导写入 \Recipient（楷体顶格由类渲染），标题、落款、日期随之写入。
        assert!(tex.contains("\\renewcommand{\\Recipient}{张三、李四}"));
        assert!(tex.contains("\\renewcommand{\\DocumentTitle}{呈批件标题}"));
        assert!(tex.contains("\\renewcommand{\\SignatureUnit}{办公室}"));
        assert!(tex.contains("\\renewcommand{\\SignatureYear}{2026}"));
        assert!(tex.contains("\\renewcommand{\\SignatureMonth}{8}"));
        assert!(tex.contains("\\renewcommand{\\SignatureDay}{5}"));
        // 正文与函稿同格式：标题编号 + 紧缩合并。
        assert!(tex.contains("{\\heiti 一、任务目标。}测试正文。\\par"));
    }

    #[test]
    fn white_paper_tex_formats_leaders_by_person_order_and_position() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.reporting_leaders = "夏天、王庭、文强".into();
        let vocabulary = vec![
            VocabularyEntry {
                category: VocabularyCategory::Person,
                code: "01".into(),
                canonical: "王庭".into(),
                position: "主任".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Person,
                code: "02".into(),
                canonical: "文强".into(),
                position: "副主任".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Person,
                code: "03".into(),
                canonical: "夏天".into(),
                position: "副主任".into(),
                ..Default::default()
            },
        ];
        let tex = white_paper_tex(
            &input,
            "# 呈批件标题\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        );
        assert!(tex.contains("\\renewcommand{\\Recipient}{王主任，文、夏副主任}"));
    }

    #[test]
    fn white_paper_tex_supports_normal_and_compact_styles_and_missing_date() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.reporting_leaders = "张三".into();
        input.profile.signing_unit = "办公室".into();
        input.date = "2026年8月5日".into();
        // 紧缩：最深 2 级标题与正文合并。
        input.profile.style_mode = StyleMode::Compact;
        let tex = white_paper_tex(
            &input,
            "# 标题\n\n## 任务目标\n测试正文。",
            &UnitDisplay::new(&[]),
        );
        assert!(tex.contains("{\\heiti 一、任务目标。}测试正文。\\par"));
        // 正常：标题独立成段。
        input.profile.style_mode = StyleMode::Normal;
        let tex = white_paper_tex(
            &input,
            "# 标题\n\n## 任务目标\n测试正文。",
            &UnitDisplay::new(&[]),
        );
        assert!(tex.contains("{\\heiti 一、任务目标}\\par"));
        // 未填成文日期时不再注入日期命令，沿用类默认（年份取当前年、日期留空）。
        input.date = String::new();
        let tex = white_paper_tex(&input, "# 标题\n\n正文。", &UnitDisplay::new(&[]));
        assert!(
            !tex.contains("\\SignatureYear"),
            "无日期时不注入签名日期命令：{tex}"
        );
    }

    #[test]
    fn white_paper_preview_placeholders_the_signature_day_like_letters() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.reporting_leaders = "张三".into();
        input.profile.signing_unit = "办公室".into();
        input.date = "2026年8月5日".into();
        // 正式版：日期完整写入。
        input.profile.letter_version = LetterVersion::Formal;
        let tex = white_paper_tex(&input, "# 标题\n\n正文。", &UnitDisplay::new(&[]));
        assert!(tex.contains("\\renewcommand{\\SignatureDay}{5}"));
        // 预览版：成文日期“日”用 1em 占位，与公函一致。
        input.profile.letter_version = LetterVersion::Preview;
        let tex = white_paper_tex(&input, "# 标题\n\n正文。", &UnitDisplay::new(&[]));
        assert!(
            tex.contains("\\renewcommand{\\SignatureDay}{\\makebox[1em][c]{}}"),
            "预览版落款日期应占位：{tex}"
        );
        assert!(
            tex.contains("\\renewcommand{\\SignatureYear}{2026}"),
            "年份保持真实"
        );
    }

    #[test]
    fn white_paper_tex_falls_back_to_issuing_unit_for_signature_and_skips_attachments() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.issuing_unit = "某某处".into();
        input.profile.signing_unit = String::new(); // 留空则同呈报单位
        input.date = "2026年8月5日".into();
        let tex = white_paper_tex(
            &input,
            "# 标题\n\n正文。\n<!-- [附件] -->\n# 附件1\n附件内容。",
            &UnitDisplay::new(&[]),
        );
        assert!(tex.contains("\\renewcommand{\\SignatureUnit}{某某处}"));
        // 白头件一般只有正文：附件区段不落版。
        assert!(!tex.contains("附件内容"), "白头件不应渲染附件：{tex}");
    }

    #[test]
    fn white_paper_write_tex_outputs_class_file_alongside_tex() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.tex");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.reporting_leaders = "张三".into();
        input.profile.issuing_unit = "某单位".into();
        input.date = "2026年8月5日".into();
        write_tex(
            &path,
            &input,
            "# 标题\n\n正文。",
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let tex = std::fs::read_to_string(&path).unwrap();
        assert!(tex.contains("\\documentclass[noforcenewpage,whitepaper]{gonghan-gwa}"));
        let class_path = temp.path().join("gonghan-gwa.cls");
        assert!(
            class_path.exists(),
            "白头件应随 tex 一并输出 gonghan-gwa.cls"
        );
        let class = std::fs::read_to_string(class_path).unwrap();
        assert!(class.contains("\\setlength{\\BodyBaselineSkip}{28.98pt}"));
        assert!(!class.contains("\\setlength{\\BodyBaselineSkip}{28bp}"));
        assert!(!class.contains("\\setlength{\\BodyBaselineSkip}{29pt}"));
    }

    #[test]
    fn official_letter_class_detects_both_xiaobiaosong_font_names() {
        assert!(GONGHAN_CLASS.contains("\\IfFontExistsTF{FZXiaoBiaoSong-B05}"));
        assert!(GONGHAN_CLASS.contains("\\IfFontExistsTF{FZXiaoBiaoSong-B05S}"));
        assert!(GONGHAN_CLASS.contains("\\providecommand{\\GwaFontPath}{}"));
        assert!(GONGHAN_CLASS.contains("Path={\\GwaFontPath}"));
        assert!(GONGHAN_CLASS.contains("{XiaoBiaoSong.ttf}"));
        assert!(GONGHAN_CLASS.contains("\\RequirePackage[fontset=none]{ctex}"));
        assert!(GONGHAN_CLASS.contains("AutoFakeBold=true"));
        assert!(!GONGHAN_CLASS.contains("BoldFont={SimHei}"));
    }

    /// 类文件必须留着本机字体的逃生口，否则注入的钩子无人调用。
    #[test]
    fn class_defers_to_injected_font_hook() {
        assert!(GONGHAN_CLASS.contains("\\providecommand{\\GwaFontSetupHook}{}"));
        assert!(GONGHAN_CLASS.contains("\\ifx\\GwaFontSetupHook\\@empty"));
        assert!(GONGHAN_CLASS.contains("\\GwaFontSetupHook\n\\fi"));
    }

    fn system_font(family: &str, path: &str) -> crate::models::FontChoice {
        crate::models::FontChoice {
            family: family.into(),
            display: family.into(),
            path: path.into(),
        }
    }

    /// 没选本机字体时不注入任何东西：产出的 TeX 与从前逐字节一致。
    #[test]
    fn font_hook_is_absent_without_system_fonts() {
        assert!(font_setup_hook(&FontConfig::default()).is_none());
        // 填了但总开关关着，同样不生效。
        let fonts = FontConfig {
            use_system_fonts: false,
            body: system_font("FangSong", "C:/Windows/Fonts/simfang.ttf"),
            ..FontConfig::default()
        };
        assert!(font_setup_hook(&fonts).is_none());
    }

    /// 钩子的两条分支：按文件加载用拷进临时目录的固定文件名，按名字加载用家族名。
    /// 没配的位置继续用内置字体，两边都要保持原样。
    #[test]
    fn font_hook_covers_both_file_and_name_branches() {
        let fonts = FontConfig {
            use_system_fonts: true,
            body: system_font(
                "Source Han Serif SC",
                "C:/Windows/Fonts/SourceHanSerifSC.otf",
            ),
            page_number: system_font("NSimSun", "C:/Windows/Fonts/nsimsun.ttf"),
            ..FontConfig::default()
        };
        let hook = font_setup_hook(&fonts).expect("配了本机字体就该注入钩子");
        // `\@empty` 要在 @ 是字母的环境里读进来。
        assert!(hook.starts_with("\\makeatletter\n"));
        assert!(hook.ends_with("\\makeatother\n"));
        assert!(hook.contains("\\def\\GwaFontSetupHook{%"));

        // 按文件：正文与页码用重命名后的文件，其余仍是内置字体文件。
        assert!(hook.contains(
            "\\setCJKmainfont[Path={\\GwaFontPath}, ItalicFont={KaiTi.ttf}, AutoFakeBold=true]{gwa-body.otf}"
        ));
        assert!(hook.contains("\\newfontfamily\\ensong[Path={\\GwaFontPath}]{gwa-pagenumber.ttf}"));
        assert!(hook.contains("\\setCJKfamilyfont{xbs}[Path={\\GwaFontPath}]{XiaoBiaoSong.ttf}"));
        assert!(hook.contains("\\setCJKfamilyfont{heiti}[Path={\\GwaFontPath}]{SimHei.ttf}"));

        // 按名字：用家族名，未配置的位置沿用类文件原来的字体名。
        assert!(hook.contains(
            "\\setCJKmainfont[ItalicFont={KaiTi_GB2312}, AutoFakeBold=true]{Source Han Serif SC}"
        ));
        assert!(hook.contains("\\newfontfamily\\ensong{NSimSun}"));
        assert!(hook.contains("\\setCJKfamilyfont{heiti}{SimHei}"));
        // 未指定标题字体时保留方正小标宋的两段探测。
        assert!(hook.contains("\\IfFontExistsTF{FZXiaoBiaoSong-B05}"));
        assert!(hook.contains("\\IfFontExistsTF{FZXiaoBiaoSong-B05S}"));
    }

    /// 指定标题字体后，方正小标宋的探测让位给所选字体。
    #[test]
    fn font_hook_replaces_xiaobiaosong_probe_when_title_is_chosen() {
        let fonts = FontConfig {
            use_system_fonts: true,
            title: system_font("STZhongsong", "C:/Windows/Fonts/STZHONGS.TTF"),
            ..FontConfig::default()
        };
        let hook = font_setup_hook(&fonts).unwrap();
        assert!(!hook.contains("IfFontExistsTF"));
        assert!(hook.contains("\\setCJKfamilyfont{xbs}{STZhongsong}"));
        assert!(hook.contains("\\newfontfamily\\enbt{STZhongsong}"));
        // 扩展名统一转小写，与拷贝时的目标文件名一致。
        assert!(hook.contains("\\setCJKfamilyfont{xbs}[Path={\\GwaFontPath}]{gwa-title.ttf}"));
    }

    /// 字体名里混进 TeX 控制字符时要剔除，不能让它破坏后面的排版。
    #[test]
    fn font_hook_strips_tex_control_characters_from_names() {
        let fonts = FontConfig {
            use_system_fonts: true,
            body: system_font("Bad\\Name{}%", "C:/Windows/Fonts/bad.ttf"),
            ..FontConfig::default()
        };
        let hook = font_setup_hook(&fonts).unwrap();
        assert!(hook.contains("AutoFakeBold=true]{BadName}"));
    }

    /// 注入的钩子必须排在 `\documentclass` 之前：类文件加载时就要用上它。
    #[test]
    fn write_tex_puts_font_hook_before_documentclass() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.tex");
        let mut input = DraftInput::default();
        input.kind = TemplateKind::WhitePaper;
        input.profile.issuing_unit = "某单位".into();
        let fonts = FontConfig {
            use_system_fonts: true,
            body: system_font("FangSong", "C:/Windows/Fonts/simfang.ttf"),
            ..FontConfig::default()
        };
        write_tex(
            &path,
            &input,
            "# 标题\n\n正文。",
            &UnitDisplay::new(&[]),
            &fonts,
        )
        .unwrap();
        let tex = std::fs::read_to_string(&path).unwrap();
        let hook = tex.find("\\def\\GwaFontSetupHook").expect("应注入字体钩子");
        let class = tex.find("\\documentclass").expect("应保留 documentclass");
        assert!(hook < class, "字体钩子必须排在 documentclass 之前：{tex}");
    }

    #[test]
    fn official_letter_class_loads_automatic_closing_and_attachment_support() {
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\SetAttachmentContent}"));
        assert!(GONGHAN_CLASS.contains("（此页无正文）"));
        assert!(GONGHAN_CLASS.contains("\\noindent\\hspace*{2em}\\zihao{3}（此页无正文）"));
        // 落款是否另起一页按实际高度判断：先把“间距+落款”装箱量高，再 \unvbox 原样放回。
        // 固定估值会两头出错，见 cls 中 \gwa@placeclosing 的注释。
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\gwa@placeclosing}[1]"));
        assert!(
            GONGHAN_CLASS.contains("\\setbox\\gwa@closingbox=\\vbox{\\vspace{\\ClosingGap}#1}")
        );
        assert!(GONGHAN_CLASS.contains("\\unvbox\\gwa@closingbox"));
        assert!(
            GONGHAN_CLASS
                .contains("\\ifdim\\dimexpr\\pagegoal-\\pagetotal\\relax<\\gwa@closingneed")
        );
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\WhitePaperClosing}"));
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\LetterClosing}"));
        // 代章标注：类提供可选的 \SignatureSealOnBehalf，直接跟在落款单位后面不另起一行。
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\SignatureSealOnBehalf}"));
        assert!(
            GONGHAN_CLASS.contains("\\SignatureUnit{}\\SignatureSealOnBehalf{} \\\\"),
            "（代章）应紧跟落款单位同一行"
        );
        assert!(!GONGHAN_CLASS.contains("\\ifthenelse{\\equal{\\SignatureSealOnBehalf}{}}"));
        // 无附件的函稿要连版记一起量：版记与落款同页。
        assert!(GONGHAN_CLASS.contains("\\setbox\\gwa@recordbox=\\vbox{\\FooterRecord}"));
        assert!(GONGHAN_CLASS.contains("\\newlength{\\SignatureRecordGap}"));
        // 固定阈值已全部废弃，Rust 侧也不再按单位行数估算联合发文的预留高度。
        assert!(!GONGHAN_CLASS.contains("PrepareLetterClosing"));
        assert!(!GONGHAN_CLASS.contains("MinimumSpace"));
        assert!(GONGHAN_CLASS.contains("\\ifgwa@hasattachments"));
        assert!(GONGHAN_CLASS.contains("\\RequirePackage{tabularray}"));
        assert!(GONGHAN_CLASS.contains("\\SetTblrInner[longtblr]"));
        assert!(GONGHAN_CLASS.contains("\\newfontfamily\\enheiti{SimHei}"));
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\PrintCopiesAtLineEnd}"));
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\FooterCopiesLine}"));
        assert!(GONGHAN_CLASS.contains("\\hangindent=3em\\hangafter=1"));
        assert!(GONGHAN_CLASS.contains("\\makebox[3em][l]{抄送：}"));
        assert!(GONGHAN_CLASS.contains("\\begin{tabularx}{\\FooterRecordWidth}"));
        // 联系电话列固定 11em：标签 5em + 11 位半角数字 5.5em + 0.5em 余量。
        assert!(GONGHAN_CLASS.contains(">{\\raggedleft\\arraybackslash}p{11em}"));
        assert!(GONGHAN_CLASS.contains("\\vspace*{\\fill}"));
        assert!(GONGHAN_CLASS.contains("\\zihao{4}\n    \\noindent%"));
        assert!(!GONGHAN_CLASS.contains("\\zihao{-4}"));
    }

    #[test]
    fn official_letter_class_uses_fixed_vertical_spacing_without_page_stretching() {
        // 正文使用准确的 28.98 TeX pt；页底不足时留白，不能拉伸段间距。
        assert!(GONGHAN_CLASS.contains("\\setlength{\\BodyBaselineSkip}{28.98pt}"));
        assert!(!GONGHAN_CLASS.contains("\\setlength{\\BodyBaselineSkip}{28bp}"));
        assert!(GONGHAN_CLASS.contains("\\raggedbottom"));
        assert!(!GONGHAN_CLASS.contains("\\flushbottom"));
        assert!(GONGHAN_CLASS.contains("\\setlength{\\parskip}{0pt}"));
        // 表格及图像/浮动体周围同样只使用不可伸缩的固定尺寸。
        assert!(GONGHAN_CLASS.contains("\\fontsize{14bp}{21bp}"));
        assert!(GONGHAN_CLASS.contains("\\setlength{\\baselineskip}{21bp}"));
        assert!(GONGHAN_CLASS.contains("\\setlength{\\textfloatsep}{\\BodyBaselineSkip}"));
        assert!(GONGHAN_CLASS.contains("\\setlength{\\intextsep}{\\BodyBaselineSkip}"));
    }

    #[test]
    fn official_letter_duplex_option_uses_outer_page_numbers() {
        let mut input = DraftInput::default();
        let simplex = letter_tex(&input, "# 测试函\n\n正文。");
        assert!(simplex.contains("\\documentclass[noforcenewpage,autocalc]{gonghan-gwa}"));

        input.profile.duplex_printing = true;
        let duplex = letter_tex(&input, "# 测试函\n\n正文。");
        assert!(duplex.contains("\\documentclass[noforcenewpage,autocalc,duplex]{gonghan-gwa}"));
        assert!(GONGHAN_CLASS.contains("\\DeclareOption{duplex}"));
        assert!(GONGHAN_CLASS.contains("\\fancyfoot[RO]"));
        assert!(GONGHAN_CLASS.contains("\\fancyfoot[LE]"));
    }

    #[test]
    fn special_handling_emits_marker_and_parentheses_use_kai() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        let tex = letter_tex(
            &input,
            "# 关于测试的函\n\n他说：\"**重要事项**\"，现就（有关事项）及【特别说明】函告如下。",
        );
        assert!(tex.contains("\\renewcommand{\\SecurityLevel}{秘密}"));
        assert!(tex.contains("\\renewcommand{\\SpecialHandling}{指人专办}"));
        // 正文中完整括号及内容改用四号楷体，其余保持正文三号仿宋。
        assert!(tex.contains("他说：“\\textbf{重要事项}”"), "{tex}");
        assert!(
            tex.contains(
                "现就{\\kai\\zihao{4} （有关事项）}及{\\kai\\zihao{4} 【特别说明】}函告如下。\\par"
            ),
            "{tex}"
        );
        // 类文件提供“指人专办”命令与密级行宏，三个抬头共用。
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\SpecialHandling}{}"));
        assert!(GONGHAN_CLASS.contains("\\newcommand{\\SecurityLine}{"));
        assert!(GONGHAN_CLASS.contains("\\SecurityLine{}"));
        // 密级行顺序：密级★保密期限 在前，“指人专办” 在后。
        let line = &GONGHAN_CLASS[GONGHAN_CLASS.find("\\newcommand{\\SecurityLine}{").unwrap()..];
        assert!(line.contains("\\SecurityLevel{}★\\SecurityPeriod{}"));
        assert!(
            line.find("\\SecurityLevel{}★\\SecurityPeriod{}").unwrap()
                < line.find("\\SpecialHandling").unwrap(),
            "指人专办应排在保密期限之后"
        );
    }

    #[test]
    fn attachment_summary_is_rendered_before_signature() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.issuing_unit = "某单位".into();
        input.profile.recipient = "某部门".into();
        // 单附件用“附件：名称”。
        let single = letter_tex(
            &input,
            "# 测试函\n<!-- [附件] -->\n# 附件1\n## 统计表\n内容。",
        );
        assert!(
            single.contains("\\noindent\\hspace*{2em}附件：统计表\\par"),
            "{single}"
        );
        // 多附件逐行“附件N：名称”。
        let multi = letter_tex(
            &input,
            "# 测试函\n<!-- [附件] -->\n# 附件1\n## 统计表\n内容。\n# 附件2\n## 说明材料\n内容。",
        );
        assert!(
            multi.contains("\\noindent\\hspace*{2em}附件1：统计表\\par"),
            "{multi}"
        );
        assert!(
            multi.contains("\\noindent\\hspace*{2em}附件2：说明材料\\par"),
            "{multi}"
        );
        // 概要前空两行，且位于 MainContent（正文）内、附件区之前。
        assert!(multi.contains("\\vspace{2\\baselineskip}"));
        let main_at = multi.find("\\renewcommand{\\MainContent}{").unwrap();
        let summary_at = multi.find("附件1：统计表").unwrap();
        let attachment_at = multi.find("\\SetAttachmentContent").unwrap();
        assert!(main_at < summary_at && summary_at < attachment_at);
    }

    #[test]
    fn title_content_single_compressed_and_wrapped() {
        // 单行标题保持二号。
        let single = title_content_tex("关于开展测试工作的函");
        assert_eq!(
            single,
            "{\\bs\\enbt\\zihao{2}\\setlength{\\baselineskip}{\\BodyBaselineSkip} 关于开展测试工作的函}"
        );

        // 22 字（超出一行 2 字）→ 只横向缩放、字高保持二号。
        let compressed = title_content_tex("关于认真做好网络安全与信息化重点工作验收的函");
        assert!(compressed.contains("\\scalebox{0.91}[1]"), "{compressed}");
        assert!(
            compressed.contains(
                "\\zihao{2}\\setlength{\\baselineskip}{\\BodyBaselineSkip}\\scalebox{0.91}[1]{关于认真做好网络安全与信息化重点工作验收的函}"
            ),
            "{compressed}"
        );

        // 35 字 → jieba 换行，行间以 \\ 分隔，词不拆开、字符不增删。
        let title = "关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知";
        let wrapped = title_content_tex(title);
        let prefix = "{\\bs\\enbt\\zihao{2}\\setlength{\\baselineskip}{\\BodyBaselineSkip} ";
        assert!(wrapped.starts_with(prefix), "{wrapped}");
        assert!(wrapped.contains("\\\\"), "{wrapped}");
        assert!(wrapped.ends_with('}'));
        let content = wrapped
            .trim_start_matches(prefix)
            .trim_end_matches('}')
            .replace("\\\\", "");
        assert_eq!(content, title, "换行不得增删字符");
    }

    #[test]
    fn phone_notice_uses_letter_class_without_number_or_footer_record() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PhoneNotice;
        let tex = letter_tex(
            &input,
            "# 测试电话通知\n正文。\n<!-- [附件] -->\n# 附件1\n## 附件标题\n附件内容。",
        );
        assert!(tex.contains("\\documentclass[noforcenewpage,autocalc,phonenotice]{gonghan-gwa}"));
        assert!(tex.contains("\\SetAttachmentContent"));
        assert!(GONGHAN_CLASS.contains("\\DeclareOption{phonenotice}"));
        assert!(GONGHAN_CLASS.contains("\\ifgwa@phonenotice"));
    }

    #[test]
    fn plain_document_tex_contains_only_plain_metadata_and_reuses_attachments() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PlainDocument;
        input.profile.issuing_unit = "不应写入的发文单位".into();
        input.profile.recipient = "不应写入的主送单位".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "10年".into();
        input.profile.special_handling = true;
        input.profile.duplex_printing = true;
        let tex = plain_document_tex(
            &input,
            "# 普通公文测试\n正文。\n<!-- [附件] -->\n# 附件1\n## 附件标题\n附件内容。",
        );

        assert!(tex.contains(
            "\\documentclass[noforcenewpage,autocalc,duplex,plaindocument]{gonghan-gwa}"
        ));
        assert!(tex.contains("\\renewcommand{\\SecurityLevel}{秘密}"));
        assert!(tex.contains("\\renewcommand{\\SecurityPeriod}{{\\ttfamily 10}年}"));
        assert!(tex.contains("\\SetAttachmentContent"));
        assert!(tex.contains("附件：附件标题"));
        assert!(!tex.contains("不应写入的发文单位"));
        assert!(!tex.contains("不应写入的主送单位"));
        assert!(!tex.contains("指人专办"));
        assert!(GONGHAN_CLASS.contains("\\DeclareOption{plaindocument}"));
        assert!(GONGHAN_CLASS.contains("\\PlainDocumentHeader"));
    }

    #[test]
    fn phone_notice_signature_uses_spaced_abbreviation() {
        // 规格 §3.1：电话通知落款显示简称，少于 5 字时逐字加半角空格。
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
        let tex = official_letter_tex(&input, "# 电话通知\n\n正文。", &display);
        // LaTeX 中普通空格无效，必须输出受控空格命令 `\ `。
        assert!(tex.contains("\\renewcommand{\\SignatureUnit}{中\\ 宣\\ 部}"));
        assert!(!tex.contains("\\renewcommand{\\SignatureUnit}{中 宣 部}"));
    }

    #[test]
    fn official_letter_seal_on_behalf_emits_daizhang_mark() {
        // 默认不标注代章：命令注入为空串。
        let input = DraftInput::default();
        let tex = letter_tex(&input, "# 测试函\n\n正文。");
        assert!(tex.contains("\\renewcommand{\\SignatureSealOnBehalf}{}"));

        // 单位词库启用代章：自动注入“（代章）”。
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "星海省教育厅".into();
        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "星海省教育厅".into(),
            seal_on_behalf: true,
            ..Default::default()
        }];
        let tex = official_letter_tex(&input, "# 测试函\n\n正文。", &UnitDisplay::new(&vocabulary));
        assert!(
            tex.contains("\\renewcommand{\\SignatureSealOnBehalf}{（代章）}"),
            "应注入代章命令：{tex}"
        );
    }

    #[test]
    fn phone_notice_never_emits_daizhang_even_if_unit_configures_it() {
        // 代章只对公函生效：电话通知等其他文种不盖章，单位即使勾选代章也不注入。
        let mut input = DraftInput::default();
        input.kind = TemplateKind::PhoneNotice;
        input.profile.issuing_unit = "星海省教育厅".into();
        let vocabulary = vec![VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "星海省教育厅".into(),
            seal_on_behalf: true,
            ..Default::default()
        }];
        let tex = official_letter_tex(
            &input,
            "# 测试电话通知\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        );
        assert!(
            tex.contains("\\renewcommand{\\SignatureSealOnBehalf}{}"),
            "电话通知不盖章，代章命令应为空：{tex}"
        );
        assert!(!tex.contains("（代章）"), "电话通知不得出现代章：{tex}");
    }

    #[test]
    fn joint_mode_one_seal_on_behalf_follows_the_main_unit() {
        let mut input = DraftInput::default();
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位、乙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        input.date = "2026年8月5日".into();
        let mut vocabulary = vec![
            VocabularyEntry {
                category: VocabularyCategory::Unit,
                canonical: "甲单位".into(),
                seal_on_behalf: true,
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Unit,
                canonical: "乙单位".into(),
                ..Default::default()
            },
        ];
        let tex = official_letter_tex(
            &input,
            "# 联合发文测试函\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        );
        // 代章直接跟在主发文单位后面同一行，不另起一行；日期仍压在主单位列下方。
        assert!(tex.contains("甲单位（代章） & 乙单位"), "{tex}");
        assert!(!tex.contains("（代章）\\\\"), "代章不应另起一行：{tex}");

        // 主发文单位在右列时，代章跟在其后。
        input.profile.main_issuing_unit = "乙单位".into();
        vocabulary[0].seal_on_behalf = false;
        vocabulary[1].seal_on_behalf = true;
        let tex = official_letter_tex(
            &input,
            "# 联合发文测试函\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        );
        assert!(tex.contains("甲单位 & 乙单位（代章）"), "{tex}");

        // 主发文单位未配置代章时不出现。
        vocabulary[1].seal_on_behalf = false;
        let tex = official_letter_tex(
            &input,
            "# 联合发文测试函\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        );
        assert!(!tex.contains("（代章）"));
    }

    #[test]
    fn joint_mode_one_date_sits_under_the_main_unit_column() {
        let mut input = DraftInput::default();
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位、乙单位".into();
        input.date = "2026年8月5日".into();

        // 主单位在左列：日期进左单元格，右单元格留空。
        input.profile.main_issuing_unit = "甲单位".into();
        let tex = letter_tex(&input, "# 联合发文测试函\n\n正文。");
        assert!(tex.contains("2026年8月5日 & \\\\"), "{tex}");

        // 主单位在右列：日期进右单元格。
        input.profile.main_issuing_unit = "乙单位".into();
        let tex = letter_tex(&input, "# 联合发文测试函\n\n正文。");
        assert!(tex.contains("& 2026年8月5日 \\\\"), "{tex}");

        // 主单位是跨两列的最后一个单位时，日期整行居中（直接排一行，不套表格）。
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "丙单位".into();
        let tex = letter_tex(&input, "# 联合发文测试函\n\n正文。");
        assert!(
            tex.contains("\\par\\vspace{6mm}\n2026年8月5日\n\\end{center}"),
            "{tex}"
        );
    }

    #[test]
    fn external_letter_latex_uses_external_names_and_keeps_abbr() {
        let mut input = DraftInput::default();
        input.profile.correspondence_scope = crate::models::CorrespondenceScope::External;
        input.profile.issuing_unit = "内部单位名".into();
        input.profile.recipient = "内部收文名".into();
        input.profile.responsible_unit = "内部单位名".into();
        let vocabulary = vec![
            VocabularyEntry {
                canonical: "内部单位名".into(),
                external_name: "外部单位名".into(),
                abbr: "内单".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "内部收文名".into(),
                external_name: "外部收文名".into(),
                ..Default::default()
            },
        ];
        let tex = official_letter_tex(
            &input,
            "# 外部函测试\n\n正文。",
            &UnitDisplay::new(&vocabulary),
        );
        assert!(tex.contains("\\renewcommand{\\IssuingUnit}{外部单位名}"));
        assert!(tex.contains("\\renewcommand{\\Recipient}{外部收文名}"));
        assert!(tex.contains("\\renewcommand{\\ResponsibleUnit}{内单}"));
    }

    #[test]
    fn joint_mode_one_latex_uses_main_header_seal_gap_and_multiline_record() {
        let mut input = DraftInput::default();
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        input.profile.joint_responsible_units = "甲处室、乙处室".into();
        input.profile.joint_contacts = vec![
            JointContact {
                name: "张三".into(),
                phone: "010-11111111".into(),
            },
            JointContact {
                name: "李四".into(),
                phone: "010-22222222".into(),
            },
        ];
        input.date = "2026年8月5日".into();
        let tex = letter_tex(&input, "# 联合发文测试函\n\n正文。");
        assert!(tex.contains("\\documentclass[noforcenewpage,autocalc,jointmodeone]{gonghan-gwa}"));
        assert!(tex.contains("\\renewcommand{\\IssuingUnit}{乙单位}"));
        assert!(tex.contains("甲单位 & 乙单位 \\\\[45mm]"));
        // 规格 §2.5：三个单位时最后一个跨两列居中。
        assert!(tex.contains("\\multicolumn{2}{@{}c@{}}{丙单位} \\\\"));
        assert!(tex.contains("2026年8月5日"));
        // 主发文单位“乙单位”在右列，日期随之进右单元格、左单元格留空。
        assert!(tex.contains("& 2026年8月5日 \\\\"));
        // 规格 §3.2：2 字姓名中间加 1em；第 2 行起承办单位前 5 个 \quad、联系人前 4 em。
        assert!(
            tex.contains("承办单位：甲处室 & 联系人：张\\hspace{1em}三 & 联系电话：010-11111111")
        );
        assert!(
            tex.contains("\\hspace*{5em}乙处室 & \\hspace*{4em}李\\hspace{1em}四 & 010-22222222")
        );
        assert!(tex.contains("\\begin{tabularx}{\\FooterRecordWidth}"));
        assert!(tex.contains(">{\\raggedleft\\arraybackslash}p{11em}"));
        assert!(tex.contains("\\FooterCopiesLine{}"));
        assert!(tex.contains("\\vspace*{\\fill}"));
        assert!(tex.contains("\\zihao{4}"));
        assert!(!tex.contains("\\zihao{-4}"));
        assert!(GONGHAN_CLASS.contains("\\DeclareOption{jointmodeone}"));
        assert!(GONGHAN_CLASS.contains("\\SetJointSignatureContent"));
        assert!(GONGHAN_CLASS.contains("\\SetJointFooterRecord"));
    }

    #[test]
    fn official_letter_preview_masks_serial_and_day_in_latex() {
        let mut input = DraftInput::default();
        input.profile.department_code = "某政函".into();
        input.profile.document_number = "12".into();
        input.date = "2026年8月5日".into();

        let formal = letter_tex(&input, "# 测试函\n\n正文。");
        assert!(formal.contains("\\renewcommand{\\Year}{2026}"));
        assert!(formal.contains("\\renewcommand{\\DocumentNumber}{12}"));
        assert!(formal.contains("\\renewcommand{\\SignatureMonth}{8}"));
        assert!(formal.contains("\\renewcommand{\\SignatureDay}{5}"));

        input.profile.letter_version = LetterVersion::Preview;
        let preview = letter_tex(&input, "# 测试函\n\n正文。");
        // 规格 §3.3：预览版占位统一 1em。
        assert!(preview.contains("\\renewcommand{\\DocumentNumber}{\\makebox[1em][c]{}}"));
        assert!(preview.contains("\\renewcommand{\\SignatureDay}{\\makebox[1em][c]{}}"));
        assert!(!preview.contains("\\renewcommand{\\DocumentNumber}{12}"));
        assert!(!preview.contains("\\renewcommand{\\SignatureDay}{5}"));
    }

    #[test]
    fn official_letter_body_resets_line_spacing_locally() {
        let body = GONGHAN_CLASS
            .split("% 正文\n")
            .nth(1)
            .expect("正文区块存在");
        let spacing = body.find("\\gwa@bodyspacing").unwrap();
        let content = body.find("\\MainContent").unwrap();
        assert!(spacing < content);
    }

    #[test]
    fn official_letter_separates_body_and_attachments_and_numbers_headings() {
        let input = DraftInput::default();
        let tex = letter_tex(
            &input,
            "# 测试函\n<!-- [正文] -->\n## 一、总体要求\n### （一）具体事项\n正文。\n<!-- [附件] -->\n# 附件1\n## 统计表\n### 一、填报说明\n附件内容。",
        );
        let main_at = tex.find("\\renewcommand{\\MainContent}").unwrap();
        let attachment_at = tex.find("\\SetAttachmentContent").unwrap();
        assert!(main_at < attachment_at);
        assert!(tex.contains("\\heiti 一、总体要求"));
        assert!(tex.contains("\\kai （一）具体事项"));
        assert!(tex.contains("\\heiti\\enheiti\\zihao{3} 附件1"));
        assert!(tex.contains(
            "\\vspace{\\BodyBaselineSkip}\n{\\centering\\bs\\enbt\\zihao{2}\\setlength{\\baselineskip}{\\BodyBaselineSkip} 统计表\\par}"
        ));
        assert!(tex.contains("\\hspace*{2em}{\\heiti 一、总体要求}"));
        assert!(tex.contains("\\hspace*{2em}{\\kai （一）具体事项}"));
        assert!(!tex.contains("\\clearpage"));
        assert_eq!(
            tex.matches("\\heiti 一、").count(),
            2,
            "附件标题计数应重新开始"
        );
    }

    #[test]
    fn compact_style_merges_level_two_heading_with_following_paragraph() {
        let mut input = DraftInput::default();
        input.profile.style_mode = StyleMode::Compact;
        let tex = letter_tex(&input, "# 测试函\n\n## 任务目标\n测试正文。");
        // 标题部分用黑体并带顿号编号与句号，正文部分沿用正文字体，合并为一行。
        assert!(tex.contains("{\\heiti 一、任务目标。}测试正文。\\par"));
        // 正常风格不合并。
        input.profile.style_mode = StyleMode::Normal;
        let tex = letter_tex(&input, "# 测试函\n\n## 任务目标\n测试正文。");
        assert!(tex.contains("{\\heiti 一、任务目标}\\par"));
    }

    #[test]
    fn compact_style_leaves_heading_before_list_or_table_alone() {
        let mut input = DraftInput::default();
        input.profile.style_mode = StyleMode::Compact;
        // 标题后跟列表时不合并，仍输出独立标题段。
        let tex = letter_tex(&input, "# 测试函\n\n## 任务目标\n- 第一项\n- 第二项");
        assert!(tex.contains("{\\heiti 一、任务目标}\\par"));
        assert!(tex.contains("\\noindent • 第一项\\par"));
    }

    #[test]
    fn compact_style_merges_every_deepest_heading_paragraph_pair() {
        let mut input = DraftInput::default();
        input.profile.style_mode = StyleMode::Compact;
        // 正文区 # 号最多的是 3 级（###）：每个“3 级标题+段落”都合并，2 级标题保持独立。
        let tex = letter_tex(
            &input,
            "# 测试函\n\n## 一、总体要求\n开头段落。\n### （一）任务一\n任务一正文。\n### （二）任务二\n任务二正文。",
        );
        // 2 级标题不合并，仍输出独立标题段，其后的段落单独成段。
        assert!(tex.contains("{\\heiti 一、总体要求}\\par"));
        assert!(tex.contains("开头段落。\\par"));
        // 3 级标题每个都与紧随正文合并，用楷体与“（一）”“（二）”编号。
        assert_eq!(
            tex.matches("{\\kai （一）任务一。}任务一正文。\\par")
                .count(),
            1,
            "每个最深层标题都应合并：{tex}"
        );
        assert_eq!(
            tex.matches("{\\kai （二）任务二。}任务二正文。\\par")
                .count(),
            1
        );
        // 附件区标题不计入最深层级，正文区仍按 3 级合并。
        let tex = letter_tex(
            &input,
            "# 测试函\n\n### 正文事项\n正文内容。\n<!-- [附件] -->\n# 附件1\n## 表一\n内容一。",
        );
        assert!(tex.contains("正文事项。}正文内容。\\par"));
        assert!(tex.contains(
            "\\centering\\bs\\enbt\\zihao{2}\\setlength{\\baselineskip}{\\BodyBaselineSkip} 表一\\par"
        ));
    }

    #[test]
    fn each_additional_attachment_starts_on_a_new_page() {
        let blocks = parse_markdown(
            "# 测试函\n正文。\n<!-- [附件] -->\n# 附件1\n## 表一\n内容一。\n# 附件2\n## 表二\n内容二。",
        );
        let (_, attachments) = official_letter_sections_to_tex(&blocks, false);
        assert_eq!(attachments.matches("\\clearpage").count(), 1);
        assert!(attachments.find("附件1").unwrap() < attachments.find("附件2").unwrap());
    }

    #[test]
    fn attachment_table_uses_mdx_longtblr_environment() {
        let blocks = parse_markdown(
            "# 测试函\n<!-- [附件] -->\n# 附件1\n## 统计表\n| 序号 | 说明 |\n| --- | --- |\n| 1 | 较长的说明文字。 |",
        );
        let (_, attachments) = official_letter_sections_to_tex(&blocks, false);
        assert!(attachments.contains("\\begin{longtblr}"));
        assert!(attachments.contains("Q[c,wd=2em]"));
        assert!(attachments.contains("rowhead = 1"));
    }

    #[test]
    fn crowded_table_makes_only_its_attachment_landscape() {
        let blocks = parse_markdown(
            "# 测试函\n<!-- [附件] -->\n# 附件1\n## 宽表\n| 序号 | 事项类别 | 事项名称 | 存在问题 | 整改措施 | 责任部门 | 完成时限 | 当前状态 |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n| 1 | 线上办理 | 行政备案事项 | 移动端部分页面显示不完整，申请人无法正常上传附件。 | 优化移动端页面适配，增加格式和大小提示并开展测试。 | 技术保障部门 | 2026年8月12日 | 已完成 |\n# 附件2\n## 窄表\n| 序号 | 名称 |\n| --- | --- |\n| 1 | 短项 |",
        );
        let (_, attachments) = official_letter_sections_to_tex(&blocks, false);
        let first = attachments.find("附件1").unwrap();
        let landscape_end = attachments.find("\\end{landscape}").unwrap();
        let second = attachments.find("附件2").unwrap();
        assert!(attachments.starts_with("\\begin{landscape}"));
        assert!(first < landscape_end && landscape_end < second);
        assert_eq!(attachments.matches("\\begin{landscape}").count(), 1);
        assert_eq!(attachments.matches("\\end{landscape}").count(), 1);
    }

    #[test]
    fn official_letter_class_supports_per_attachment_landscape_pages() {
        assert!(GONGHAN_CLASS.contains("\\RequirePackage{pdflscape}"));
    }
}
