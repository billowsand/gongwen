//! 公函骨架：正式公函/普通文档的 LaTeX 骨架、联合发文与分节转换。
//!
//! 由 src/export/latex.rs 拆分而来：本文件是模块 `export::latex::official`，与其它子模块共享
//! `export::latex` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::latex::{
    attachment_document_title_to_tex, attachment_landscape_flags, attachment_summary_tex,
    body_text_to_tex, heading_number_prefix, latex_name, official_heading_to_tex,
    security_commands, target_tex_section, tex_escape, tex_spaced, title_content_tex,
};
use crate::export::table::to_longtblr;
use crate::export::{
    MarkdownBlock, MarkdownSection, body_heading_max_level, chinese_date_parts, joint_main_column,
    parse_markdown_with_lines, plain_text,
};
use crate::models::{
    DraftInput, JointIssuanceMode, LetterVersion, StyleMode, TemplateKind, split_units,
};
use crate::units::UnitDisplay;

pub(crate) fn official_letter_tex(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
) -> String {
    let (blocks, block_lines) = parse_markdown_with_lines(markdown);
    let title = blocks
        .iter()
        .find_map(|b| match b {
            MarkdownBlock::Title(t) => Some(t.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
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
    // 落款按“多家单位并列”排版只发生在发文单位多于 1 个时；只剩 1 个时回落
    // 单独发文的右侧落款。版记不受此影响：只要是联合发文模式 1，承办单位、
    // 联系人、电话就按行逐条列出，因此 jointrecord 与 jointmodeone 分开给。
    let joint_signature = crate::models::is_joint_signature(input);
    let joint_option = if joint_signature {
        ",jointmodeone"
    } else if joint_mode_one {
        ",jointrecord"
    } else {
        ""
    };
    let joint_commands = if joint_mode_one {
        joint_mode_one_commands(
            input,
            signature_year,
            signature_month,
            &signature_day,
            display,
            joint_signature,
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
        let raw = if !input.profile.signing_unit.trim().is_empty() {
            input.profile.signing_unit.trim().to_string()
        } else if joint_mode_one {
            // 联合发文模式 1 只剩 1 个发文单位：落款回落右侧单列，单位取该唯一发文
            // 单位（联合发文的单位存在 joint_issuing_units，issuing_unit 是空的）。
            split_units(&input.profile.joint_issuing_units)
                .into_iter()
                .next()
                .unwrap_or_else(|| input.profile.issuing_unit.trim().to_string())
        } else {
            input.profile.issuing_unit.trim().to_string()
        };
        if input.kind == TemplateKind::PhoneNotice {
            display.abbr_spaced(&raw)
        } else {
            display.full_name_for(&raw, input.uses_external_unit_names())
        }
    };

    format!(
        r#"%!TEX program = xelatex
\documentclass[proof,noforcenewpage,autocalc{duplex_option}{phone_notice_option}{joint_option}]{{gonghan-gwa}}
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
pub(crate) fn plain_document_tex(input: &DraftInput, markdown: &str) -> String {
    let (blocks, block_lines) = parse_markdown_with_lines(markdown);
    let title = blocks
        .iter()
        .find_map(|block| match block {
            MarkdownBlock::Title(title) => Some(title.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str());
    let (mut body, attachments) = official_letter_sections_to_tex(
        &blocks,
        &block_lines,
        input.profile.style_mode == StyleMode::Compact,
    );
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
\documentclass[proof,noforcenewpage,autocalc{duplex_option},plaindocument]{{gonghan-gwa}}
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

/// 联合发文模式 1 的落款与版记内容。`with_signature` 为假时只输出版记
/// （发文单位只剩 1 个，落款交给类默认的右侧单列，不需要并列落款内容）。
pub(crate) fn joint_mode_one_commands(
    input: &DraftInput,
    year: &str,
    month: &str,
    day: &str,
    display: &UnitDisplay,
    with_signature: bool,
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
    let entries = crate::models::joint_responsible_entries(&input.profile);
    let record_rows = entries.len().max(1);
    let mut record_lines = Vec::new();
    for index in 0..record_rows {
        let entry = entries.get(index);
        let responsible_name = display.abbr(entry.map_or("", |value| value.unit.as_str()));
        let contact_name = entry.map_or("", |value| value.name.as_str());
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
            tex_escape(entry.map_or("", |value| value.phone.as_str())),
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
    let signature_command = if with_signature {
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
"#,
            signature_rows = signature_rows.join("\n"),
            closing = closing,
        )
    } else {
        String::new()
    };
    format!(
        r#"{signature_command}\SetJointFooterRecord{{%
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
        signature_command = signature_command,
        record_lines = record_lines.join("\n"),
    )
}

/// 段末探针：`lines` 里没有对应行号时退回普通 `\par`，保证任何调用方都能用。
pub(crate) fn gwa_tail(lines: &[usize], index: usize) -> String {
    match lines.get(index) {
        Some(line) => format!("\\GwaTail{{{line}}}"),
        None => "\\par".to_string(),
    }
}

pub(crate) fn official_letter_sections_to_tex(
    blocks: &[MarkdownBlock],
    lines: &[usize],
    compact: bool,
) -> (String, String) {
    official_letter_sections_to_tex_with_barrier(blocks, lines, compact, None)
}

/// 同上，另可在第一个表格/图片之前插入屏障。红头呈批件的普通文字保持连续
/// 流排；表格和图片不能进入首页批示栏，因此仍由屏障直接送到第二页。
/// `lines` 与 `blocks` 一一对应，是各块在 Markdown 源码里的 1-based 起始行号。
/// 每个正文段落末尾据此发 `\GwaTail{行号}` 取代 `\par`：类文件里它默认就是
/// `\par`，开 proof 选项后额外把末行坐标与行数写进 `.gwaproof`，孤行提示就能
/// 直接点回 Markdown 的那一行。
pub(crate) fn official_letter_sections_to_tex_with_barrier(
    blocks: &[MarkdownBlock],
    lines: &[usize],
    compact: bool,
    barrier: Option<&str>,
) -> (String, String) {
    let mut body_float_barrier = barrier;
    let mut body = Vec::new();
    let mut attachments = Vec::new();
    let landscape_attachments = attachment_landscape_flags(blocks);
    let attachment_count = blocks
        .iter()
        .filter(|block| matches!(block, MarkdownBlock::Marker(MarkdownSection::Attachment)))
        .count();
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
            MarkdownBlock::Title(_) if !seen_document_title && section == MarkdownSection::Body => {
                seen_document_title = true;
            }
            MarkdownBlock::Title(text) if section == MarkdownSection::Attachment => {
                counters = [0; 4];
                attachments.push(attachment_document_title_to_tex(text));
            }
            MarkdownBlock::Title(_) => {}
            MarkdownBlock::Marker(MarkdownSection::Attachment) => {
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
                let label = if attachment_count == 1 {
                    "附件".to_string()
                } else {
                    format!("附件{}", attachment_title_count)
                };
                attachments.push(format!(
                    "\\noindent{{\\xeCJKsetup{{CJKecglue={{\\hskip0pt}}}}\\heiti\\enheiti\\zihao{{3}} {}}}\\par",
                    tex_escape(&label)
                ));
            }
            MarkdownBlock::Marker(MarkdownSection::Body) => {
                section = MarkdownSection::Body;
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
                        2 => format!("\\heiti\\enheiti {number}{heading_escaped}。"),
                        3 => format!("\\kai\\enkai {number}{heading_escaped}。"),
                        4 => format!("{number}{heading_escaped}。"),
                        5 => format!("\\textbf{{{number}{heading_escaped}。}}"),
                        _ => unreachable!(),
                    };
                    target_tex_section(section, &mut body, &mut attachments).push(format!(
                        "\\noindent\\hspace*{{2em}}{{{title_tex}}}{body_escaped}{tail}",
                        tail = gwa_tail(lines, index + 1)
                    ));
                    index += 1; // 跳过紧随的正文段落
                } else {
                    let rendered = match section {
                        MarkdownSection::Body => {
                            official_heading_to_tex(*level, text, &mut counters)
                        }
                        MarkdownSection::Attachment => {
                            official_heading_to_tex(*level, text, &mut counters)
                        }
                    };
                    if let Some(rendered) = rendered {
                        // 标题同样可能折行后末行挂字，末尾的 \par 一并换成探针。
                        let rendered = match rendered.strip_suffix("\\par") {
                            Some(head) => format!("{head}{}", gwa_tail(lines, index)),
                            None => rendered,
                        };
                        target_tex_section(section, &mut body, &mut attachments).push(rendered);
                    }
                }
            }
            MarkdownBlock::Paragraph(text) => {
                if !text.contains("<div") && !text.contains("</div") {
                    target_tex_section(section, &mut body, &mut attachments).push(format!(
                        "{}{}",
                        body_text_to_tex(text),
                        gwa_tail(lines, index)
                    ));
                }
            }
            MarkdownBlock::ListItem(text) => {
                target_tex_section(section, &mut body, &mut attachments).push(format!(
                    "\\noindent {}{}",
                    body_text_to_tex(text),
                    gwa_tail(lines, index)
                ));
            }
            MarkdownBlock::OrderedListItem { number, text } => {
                target_tex_section(section, &mut body, &mut attachments).push(format!(
                    "\\noindent\\hspace*{{2em}}{number}.{}{}",
                    body_text_to_tex(text),
                    gwa_tail(lines, index)
                ));
            }
            MarkdownBlock::Table { rows, aligns } => {
                let rendered = to_longtblr(rows, aligns);
                if !rendered.is_empty() {
                    push_body_float_barrier(
                        section,
                        &mut body_float_barrier,
                        &mut body,
                        &mut attachments,
                    );
                    target_tex_section(section, &mut body, &mut attachments).push(rendered);
                }
            }
            MarkdownBlock::Image { alt: _, src } => {
                push_body_float_barrier(
                    section,
                    &mut body_float_barrier,
                    &mut body,
                    &mut attachments,
                );
                target_tex_section(section, &mut body, &mut attachments).push(format!(
                    "\\begin{{center}}\\includegraphics[width=\\textwidth]{{{}}}\\end{{center}}",
                    tex_escape(src)
                ));
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

/// 在正文区第一个表格/图片之前插入一次屏障命令，之后把屏障取走不再重复插入。
/// 附件区不受影响——附件本来就另起一页，没有批示栏要避让。
pub(crate) fn push_body_float_barrier(
    section: MarkdownSection,
    barrier: &mut Option<&str>,
    body: &mut Vec<String>,
    attachments: &mut Vec<String>,
) {
    if section != MarkdownSection::Body {
        return;
    }
    if let Some(command) = barrier.take() {
        target_tex_section(section, body, attachments).push(command.to_string());
    }
}
