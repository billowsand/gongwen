//! 各文种的版头、落款与版记 → 模板数据。逐项对应 TeX 路径
//! （`latex::official` / `latex::papers`）里写进类文件命令的那些取值：同一套
//! 显示变换（`element_display`）、同一套要素标注与预览版占位。

use chrono::Datelike;

use super::body::{attachment_summary, main_title, sections, title_data};
use super::data::{Closing, Doc, Header, JointRow, Record, RecordRow, Red, Run, Runs};
use super::runs::{marked_runs, plain_runs, with_cjk_latin_gaps};
use crate::export::element_display::{
    addressee_display, copies_to_display, date_display_parts, is_preview_version,
    number_display_parts, signing_unit_display,
};
use crate::export::title;
use crate::export::{MarkdownBlock, parse_markdown_with_lines_with_numbering};
use crate::models::{
    DraftInput, JointIssuanceMode, ListNumbering, NumberingConfig, TemplateKind, split_units,
};
use crate::typst_engine::FontFamilies;
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;
use crate::visual_diff::elements::{DateMarks, FieldMark};

/// 预览版留白（规格 §3.3：统一 1em 宽）。
fn placeholder() -> Runs {
    vec![Run::placeholder(1.0)]
}

/// 要素取值：变了整字段删旧插新，没变走原生构造。
fn element_runs(mark: &FieldMark, native: impl FnOnce() -> Runs) -> Runs {
    if mark.changed() {
        marked_runs(&mark.marked())
    } else {
        native()
    }
}

fn concat(parts: impl IntoIterator<Item = Runs>) -> Runs {
    with_cjk_latin_gaps(parts.into_iter().flatten().collect())
}

/// 密级行：「密级★保密期限」+ 可选的「　指人专办」（普通公文没有指人专办）。
fn security_runs(input: &DraftInput, mark: &FieldMark) -> Option<Runs> {
    let (level, period) = crate::export::element_display::security_parts(input);
    if level.is_empty() && !mark.changed() {
        return None;
    }
    let special = input.kind != TemplateKind::PlainDocument && input.profile.special_handling;
    let mut runs = if mark.changed() {
        marked_runs(&mark.marked())
    } else if period.is_empty() {
        plain_runs(level)
    } else {
        plain_runs(&format!("{level}★{period}"))
    };
    if special {
        runs.push(Run::placeholder(1.0));
        runs.push(Run::text("指人专办"));
    }
    Some(runs)
}

/// 发文字号「代字〔年份〕序号 号」。函稿序号与「号」之间是 TeX 的 `~`（不断行空格），
/// 红头呈批件紧挨。
fn number_runs(input: &DraftInput, elements: &ElementMarks, space_before_hao: bool) -> Runs {
    let (code, year, serial) = number_display_parts(input);
    let [code_mark, year_mark, serial_mark] = elements.number();
    let serial_native = if is_preview_version(input) {
        placeholder()
    } else {
        plain_runs(&serial)
    };
    concat([
        element_runs(code_mark, || plain_runs(&code)),
        vec![Run::text("〔")],
        element_runs(year_mark, || plain_runs(&year)),
        vec![Run::text("〕")],
        element_runs(serial_mark, || serial_native),
        vec![Run::text(if space_before_hao {
            "\u{a0}号"
        } else {
            "号"
        })],
    ])
}

/// 成文日期整行。`class_default`：日期切不开且没有标注时，白头件 / 红头呈批件
/// 沿用类文件默认（当年、当月、日留空），函稿则三段都留空（与 TeX 一致）。
fn date_runs(input: &DraftInput, elements: &ElementMarks, class_default: bool) -> Runs {
    let preview = is_preview_version(input);
    let parts = date_display_parts(input);
    let day_native = |day: &str| {
        if preview {
            placeholder()
        } else {
            plain_runs(day)
        }
    };
    let (year, month, day): (Runs, Runs, Runs) = match (elements.date(), parts.as_slice()) {
        (marks, [y, m, d]) if marks.changed() => match marks {
            DateMarks::Parts(marks) => (
                element_runs(&marks[0], || plain_runs(y)),
                element_runs(&marks[1], || plain_runs(m)),
                element_runs(&marks[2], || day_native(d)),
            ),
            DateMarks::Whole(whole) => (marked_runs(&whole.marked()), Vec::new(), Vec::new()),
        },
        (DateMarks::Whole(whole), _) if whole.changed() => {
            (marked_runs(&whole.marked()), Vec::new(), Vec::new())
        }
        (_, [y, m, d]) => (plain_runs(y), plain_runs(m), day_native(d)),
        _ if class_default => {
            let now = chrono::Local::now();
            (
                plain_runs(&now.year().to_string()),
                plain_runs(&now.month().to_string()),
                placeholder(),
            )
        }
        _ => (Vec::new(), Vec::new(), Vec::new()),
    };
    concat([
        year,
        vec![Run::text("年")],
        month,
        vec![Run::text("月")],
        day,
        vec![Run::text("日")],
    ])
}

/// 落款单位一行少于 5 字时逐字分散到 5 字宽（`tex_spread_signature`）。
fn spread_runs(unit: &str) -> Runs {
    match crate::units::spread_gap(unit) {
        Some(gap) => {
            let mut runs = Vec::new();
            for (index, ch) in unit.chars().enumerate() {
                if index > 0 {
                    runs.push(Run::placeholder(gap as f64));
                }
                runs.push(Run::text(ch.to_string()));
            }
            runs
        }
        None => plain_runs(unit),
    }
}

/// 带签字空间的落款（白头件、红头呈批件）。
fn room_closing(input: &DraftInput, display: &UnitDisplay, elements: &ElementMarks) -> Closing {
    let units = signing_unit_display(input, display)
        .into_iter()
        .filter(|unit| !unit.trim().is_empty())
        .collect::<Vec<_>>();
    let marks = elements.signing_units();
    let mut lines: Vec<Runs> = units
        .iter()
        .enumerate()
        .map(|(index, unit)| match marks.get(index) {
            Some(mark) if mark.changed() => marked_runs(&mark.marked()),
            _ => spread_runs(unit),
        })
        .collect();
    for mark in marks.iter().skip(units.len()) {
        if mark.changed() {
            lines.push(marked_runs(&mark.marked()));
        }
    }
    let width = if marks.iter().any(|mark| mark.changed()) {
        let marked = marks
            .iter()
            .map(|mark| crate::export::strip_redline(&mark.marked()))
            .collect::<Vec<_>>();
        crate::export::red_signature_unit_width_mm(&marked)
    } else {
        crate::export::red_signature_unit_width_mm(&units)
    };
    Closing::Room {
        units: lines,
        unit_width_mm: f64::from(width),
        date: date_runs(input, elements, true),
    }
}

/// TeX `\CountUnits`：按「，」「、」数单位个数，空串为 0。
fn count_units(text: &str) -> u32 {
    if text.trim().is_empty() {
        return 0;
    }
    1 + text.matches('，').count() as u32 + text.matches('、').count() as u32
}

/// 联合发文模式 1 的成文日期压在主发文单位那一列下。
fn joint_closing(input: &DraftInput, display: &UnitDisplay, date: Runs) -> Closing {
    let units = split_units(&input.profile.joint_issuing_units);
    let main_index = crate::export::joint_seal_index(input, display);
    let odd_last = units.len() > 2 && units.len() % 2 == 1;
    let row_count = units.len().div_ceil(2).max(1);
    let mut rows = Vec::new();
    for (row_index, pair) in units.chunks(2).enumerate() {
        let base = row_index * 2;
        let name = |offset: usize| {
            let mut name = display.full_name_for(
                pair.get(offset).map_or("", String::as_str),
                input.uses_external_unit_names(),
            );
            if Some(base + offset) == main_index {
                name.push_str("（代章）");
            }
            plain_runs(&name)
        };
        if odd_last && row_index + 1 == row_count {
            rows.push(JointRow {
                left: name(0),
                right: None,
            });
        } else {
            rows.push(JointRow {
                left: name(0),
                right: Some(name(1)),
            });
        }
    }
    Closing::Joint {
        rows,
        gaps: units.len() > 2,
        date,
        date_column: crate::export::joint_main_column(input).map(|c| c as u8),
    }
}

/// 函稿 / 电话通知：版头、落款与版记。
fn letter_frame(input: &DraftInput, display: &UnitDisplay, elements: &ElementMarks, doc: &mut Doc) {
    let phone = input.kind == TemplateKind::PhoneNotice;
    let joint_mode_one = input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1;
    let issuing = if joint_mode_one {
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
    doc.header = Some(Header {
        issuing,
        number: (!phone).then(|| number_runs(input, elements, true)),
    });

    let recipient_display = addressee_display(input, display);
    doc.recipient = Some(element_runs(elements.recipient(), || {
        plain_runs(&recipient_display)
    }));

    let date = date_runs(input, elements, false);
    doc.closing = Some(if crate::models::is_joint_signature(input) {
        joint_closing(input, display, date)
    } else {
        let signature_display = signing_unit_display(input, display)
            .into_iter()
            .next()
            .unwrap_or_default();
        let mut unit = match elements.signing_units().first() {
            Some(mark) if mark.changed() => marked_runs(&mark.marked()),
            _ => plain_runs(&signature_display),
        };
        if crate::export::seals_on_behalf(input, display) {
            unit.extend(plain_runs("（代章）"));
        }
        Closing::Letter { unit, date }
    });

    if phone {
        return;
    }
    // 版记。
    let copies_display = copies_to_display(input, display);
    let copies_to = if elements.copies_to().changed() {
        Some(marked_runs(&elements.copies_to().marked()))
    } else if copies_display.trim().is_empty() {
        None
    } else {
        Some(plain_runs(&copies_display))
    };
    let responsible_display = if joint_mode_one {
        split_units(&input.profile.joint_responsible_units)
            .iter()
            .map(|unit| display.abbr(unit))
            .collect::<Vec<_>>()
            .join("、")
    } else {
        display.abbr(&input.profile.responsible_unit)
    };
    let copy_numbering = input.profile.number_copies && input.kind.has_copy_numbering();
    let print_copies = if copy_numbering {
        crate::export::copy_count(input)
    } else {
        count_units(&recipient_display)
            + count_units(&copies_display)
            + count_units(&responsible_display)
    };
    let rows = if joint_mode_one {
        let entries = crate::models::joint_responsible_entries(&input.profile);
        let count = entries.len().max(1);
        (0..count)
            .map(|index| {
                let entry = entries.get(index);
                RecordRow {
                    unit: display.abbr(entry.map_or("", |value| value.unit.as_str())),
                    contact: entry.map_or(String::new(), |value| value.name.clone()),
                    phone: entry.map_or(String::new(), |value| value.phone.clone()),
                }
            })
            .collect()
    } else {
        vec![RecordRow {
            unit: responsible_display,
            contact: input.profile.contact_person.clone(),
            phone: input.profile.contact_phone.clone(),
        }]
    };
    doc.record = Some(Record {
        copies_to,
        print_copies,
        rows,
        joint: joint_mode_one,
    });
    if copy_numbering {
        doc.copies = (1..=crate::export::copy_count(input))
            .map(crate::models::format_copy_number)
            .collect();
    }
}

/// 红头呈批件的版头与首页承办区。
fn red_frame(input: &DraftInput, display: &UnitDisplay, elements: &ElementMarks, doc: &mut Doc) {
    doc.header = Some(Header {
        issuing: display.full_name(&input.profile.issuing_unit),
        number: Some(number_runs(input, elements, false)),
    });
    let leaders = addressee_display(input, display);
    doc.recipient = Some(element_runs(elements.recipient(), || plain_runs(&leaders)));
    doc.closing = Some(room_closing(input, display, elements));
    let entries = crate::models::joint_responsible_entries(&input.profile);
    let fallback = crate::models::JointResponsibleEntry::default();
    let entries = if entries.is_empty() {
        std::slice::from_ref(&fallback).to_vec()
    } else {
        entries
    };
    // 联系人姓名导出时留空给经办人手写签字（预览仍显示），栏宽同样按留空后的行算。
    let rows = entries
        .iter()
        .map(|entry| {
            [
                display.abbr(&entry.unit),
                String::new(),
                entry.phone.clone(),
            ]
        })
        .collect::<Vec<_>>();
    let columns = crate::export::red_record_signing_columns(&rows);
    doc.red = Some(Red {
        rows: rows
            .into_iter()
            .map(|[unit, contact, phone]| RecordRow {
                unit,
                contact,
                phone,
            })
            .collect(),
        cols_mm: [
            f64::from(crate::export::RedRecordColumns::mm(columns.unit)),
            f64::from(crate::export::RedRecordColumns::mm(columns.contact)),
            f64::from(crate::export::RedRecordColumns::mm(columns.phone)),
        ],
    });
}

/// 组装一份文档的模板数据。`markdown` 是已经物化过 Mermaid 的正文。
pub(crate) fn document(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
    fonts: FontFamilies,
) -> Doc {
    let kind = input.kind;
    // 会议议程事项固定「1. 2. 3.」，不随设置里的列表编号样式变化。
    let mut numbering = *numbering;
    if kind == TemplateKind::MeetingAgenda {
        numbering.list1 = ListNumbering::DecimalDot;
        numbering.list2 = ListNumbering::DecimalDot;
    }
    let (blocks, lines) = parse_markdown_with_lines_with_numbering(markdown, &numbering);
    let title_text = blocks
        .iter()
        .find_map(|block| match block {
            MarkdownBlock::Title(title) => Some(title.as_str()),
            _ => None,
        })
        .unwrap_or(input.title_hint.as_str())
        .to_string();
    let red = kind == TemplateKind::RedHeadApproval;
    let parts = sections(&blocks, &lines, input.profile.style_mode, red, &numbering);
    let title = if red {
        title_data(&title_text, title::red_approval_chars_per_line(), |plain| {
            title::compressed_scale_percent_for(
                plain,
                title::RED_APPROVAL_TITLE_WIDTH_PT,
                title::RED_APPROVAL_TITLE_SIZE_PT,
            )
        })
    } else {
        main_title(&title_text)
    };
    let agenda = kind == TemplateKind::MeetingAgenda;
    let mut doc = Doc {
        kind: match kind {
            TemplateKind::OfficialLetter => "letter",
            TemplateKind::PhoneNotice => "phone",
            TemplateKind::PlainDocument => "plain",
            TemplateKind::WhitePaper => "whitepaper",
            TemplateKind::RedHeadApproval => "redapproval",
            TemplateKind::MeetingAgenda => "agenda",
            TemplateKind::ResearchReport => "plain",
        },
        duplex: input.profile.duplex_printing,
        probe: true,
        fonts,
        security: security_runs(input, elements.security()),
        header: None,
        title,
        recipient: None,
        body: parts.body,
        // 会议议程没有附件与附件说明（TeX 路径同样丢弃附件区）。
        summary: if agenda {
            Vec::new()
        } else {
            attachment_summary(&blocks)
        },
        attachments: if agenda {
            Vec::new()
        } else {
            parts.attachments
        },
        closing: None,
        record: None,
        copies: Vec::new(),
        red: None,
    };
    match kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => {
            letter_frame(input, display, elements, &mut doc)
        }
        TemplateKind::WhitePaper => {
            let leaders = addressee_display(input, display);
            doc.recipient = Some(element_runs(elements.recipient(), || plain_runs(&leaders)));
            doc.closing = Some(room_closing(input, display, elements));
        }
        TemplateKind::RedHeadApproval => red_frame(input, display, elements, &mut doc),
        TemplateKind::PlainDocument
        | TemplateKind::MeetingAgenda
        | TemplateKind::ResearchReport => {}
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_units_like_the_class_file() {
        assert_eq!(count_units(""), 0);
        assert_eq!(count_units("甲单位"), 1);
        assert_eq!(count_units("甲单位、乙单位，丙单位"), 3);
    }

    #[test]
    fn short_signature_units_are_spread_to_five_chars() {
        let runs = spread_runs("民政局");
        let gaps: Vec<_> = runs.iter().filter_map(|r| r.w).collect();
        assert_eq!(gaps, [1.0, 1.0]);
        assert_eq!(spread_runs("某某市民政局").len(), 1);
    }
}
