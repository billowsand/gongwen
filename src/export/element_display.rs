//! 公文要素的纸面显示值。
//!
//! 纸上印的不是 `DraftInput` 字段的原始值：主送 / 抄送要经 `UnitDisplay` 层级展开，
//! 落款按文种取全称或简称，成文日期要切成年月日，密级要拼保密期限，发文字号要拼
//! 机关代字〔年份〕序号。这些变换原先散在 TeX（`export::latex`）、Word
//! （`export::docx`）与预览（`preview::header` / `preview::tail`）三处渲染里，
//! 本模块把每个字段的显示值抽成一个函数：三处渲染与要素标注
//! （`visual_diff::elements`）都调用它，保证三端看的是同一张纸。
//!
//! 函数只做**显示变换**，不做转义、不做样式：TeX 转义（`tex_escape`）、预览版
//! 留白的占位写法（全角空格 / `\makebox`）、落款少于 5 字的分散对齐都是各载体
//! 自己的事，留在渲染处。

use crate::models::{DraftInput, LetterVersion, TemplateKind, split_units};
use crate::units::UnitDisplay;

/// 预览版留白占位：文号序号与成文日期「日」位留空待填。预览与 Word 用一个全角
/// 空格，TeX 写 `\makebox[1em][c]{}`（规格 §3.3 统一 1em 宽），由导出器转换。
pub(crate) const PREVIEW_PLACEHOLDER: &str = "\u{2003}";

/// 预览版（送审稿）：序号与「日」留白。
pub(crate) fn is_preview_version(input: &DraftInput) -> bool {
    input.profile.letter_version == LetterVersion::Preview
}

/// 主送机关位的纸面显示值：函稿 / 电话通知是主送机关，白头件 / 红头呈批件是
/// 呈报领导；其余文种没有这一位。返回值不含结尾的「：」（版式自己加）。
pub(crate) fn addressee_display(input: &DraftInput, display: &UnitDisplay) -> String {
    let text = match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => display.join_hierarchical_for(
            &split_units(&input.profile.recipient),
            input.uses_external_unit_names(),
        ),
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            display.reporting_leaders(&input.profile.reporting_leaders)
        }
        TemplateKind::PlainDocument
        | TemplateKind::MeetingAgenda
        | TemplateKind::ResearchReport => String::new(),
    };
    text.trim().trim_end_matches('：').to_string()
}

/// 抄送机关的纸面显示值（不含「抄送：」标签，只有单位串）。
pub(crate) fn copies_to_display(input: &DraftInput, display: &UnitDisplay) -> String {
    display.join_hierarchical_for(
        &split_units(&input.profile.copies_to),
        input.uses_external_unit_names(),
    )
}

/// 成文日期的纸面显示部件：能按「年月日」切开时是（年，月，日）三个部件，
/// 自由文本日期切不开时整串一个部件。部件不含「年月日」三个字——那是版式
/// 写死的（类文件 / 预览 / Word 各自拼）。
pub(crate) fn date_display_parts(input: &DraftInput) -> Vec<String> {
    match crate::export::chinese_date_parts(&input.date) {
        Some((year, month, day)) => {
            vec![year.to_string(), month.to_string(), day.to_string()]
        }
        None => vec![input.date.trim().to_string()],
    }
}

/// 成文日期整行的纸面显示值（含「年月日」），切不开时就是整串原文。
/// 预览版的「日」位换 [`PREVIEW_PLACEHOLDER`]。
pub(crate) fn date_display_line(input: &DraftInput) -> String {
    let parts = date_display_parts(input);
    if parts.len() == 3 {
        let day = if is_preview_version(input) {
            PREVIEW_PLACEHOLDER
        } else {
            parts[2].as_str()
        };
        format!("{}年{}月{day}日", parts[0], parts[1])
    } else {
        parts[0].clone()
    }
}

/// 发文字号的纸面显示部件：机关代字、年份、序号。版式把它们拼成
/// 「代字〔年份〕序号 号」——函稿序号与「号」之间留一个西文空格（TeX 是 `~`），
/// 红头呈批件紧挨着不空。预览版的序号留白由调用方按各自载体换占位。
pub(crate) fn number_display_parts(input: &DraftInput) -> (String, String, String) {
    (
        input.profile.department_code.trim().to_string(),
        input.document_year(),
        input.profile.document_number.trim().to_string(),
    )
}

/// 落款单位的纸面显示值，每行一个单位：白头件 / 红头呈批件多单位自上而下分行，
/// 其余文种一行。行的显示按文种取全称或简称（规格 §2.5 / §3.1）；联合发文模式 1
/// 只剩 1 个发文单位时回落右侧单列，单位取该唯一发文单位。
pub(crate) fn signing_unit_display(input: &DraftInput, display: &UnitDisplay) -> Vec<String> {
    match input.kind {
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            display.white_paper_signature_units(input)
        }
        TemplateKind::PhoneNotice => vec![display.abbr_spaced(&signing_unit_raw(input))],
        _ => {
            vec![display.full_name_for(&signing_unit_raw(input), input.uses_external_unit_names())]
        }
    }
}

/// 落款单位的原始取值：留空时回落发文单位（联合发文模式 1 回落唯一发文单位）。
fn signing_unit_raw(input: &DraftInput) -> String {
    let signing = input.profile.signing_unit.trim();
    if !signing.is_empty() {
        return signing.to_string();
    }
    if input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == crate::models::JointIssuanceMode::Mode1
    {
        // 联合发文的单位存在 joint_issuing_units，issuing_unit 是空的。
        return split_units(&input.profile.joint_issuing_units)
            .into_iter()
            .next()
            .unwrap_or_else(|| input.profile.issuing_unit.trim().to_string());
    }
    input.profile.issuing_unit.trim().to_string()
}

/// 密级（含保密期限）的纸面显示值：「密级★保密期限」；「内部」件没有保密期限，
/// 只印密级二字、不出「★」。未标密级返回 None，整行不排。指人专办不在其中
/// （那是另一个要素），由调用方按自己的版式追加。
pub(crate) fn security_display(input: &DraftInput) -> Option<String> {
    let (level, period) = security_parts(input);
    if level.is_empty() {
        return None;
    }
    Some(if period.is_empty() {
        level.to_string()
    } else {
        format!("{level}★{period}")
    })
}

/// 密级的原始两段：（密级，保密期限），各自去首尾空白。按样式分段渲染的路径
/// （保密期限数字用等宽西文）从这里取值，拼好的整串见 [`security_display`]。
pub(crate) fn security_parts(input: &DraftInput) -> (&str, &str) {
    (
        input.profile.security_level.trim(),
        input.profile.security_period.trim(),
    )
}
