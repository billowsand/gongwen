//! 通用文稿数据：单位三套代字、人员两个号码、编号方案与文号。
//!
//! F0 只落数据和纯算法；F1 起列表、导出命名接 `DocNumber`，F4 起台账接
//! `NumberingScheme`。这里不读配置、不碰数据库，方便单测和两版构建。

use crate::models::{DraftInput, ManuscriptStatus, TemplateKind, TemplateProfile};

/// 一个单位的三套代字。发函、呈批、纪要各自独立；纪要代字可以没有。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UnitCodes {
    pub letter: String,
    pub approval: String,
    /// `None` 表示这个单位的纪要本来就不带代字。
    pub minutes: Option<String>,
}

/// 一个人的两个号码。通讯录、承办联系人、收件人、传输单/签收单都从这里取。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PersonContact {
    pub office: String,
    pub mobile: String,
}

impl PersonContact {
    /// 手机只认 11 位、`1[3-9]` 开头；允许空格和半角横线分隔。
    /// 其余（带区号、短号、8 位固话、带括号、带 `+86`）一律按办公电话处理，
    /// 与“认不出来先放办公”的口径一致。
    pub(crate) fn is_mobile(number: &str) -> bool {
        let mut digits = String::with_capacity(11);
        for ch in number.trim().chars() {
            if ch.is_ascii_digit() {
                digits.push(ch);
            } else if ch.is_whitespace() || ch == '-' {
                continue;
            } else {
                return false;
            }
        }
        let bytes = digits.as_bytes();
        bytes.len() == 11 && bytes[0] == b'1' && (b'3'..=b'9').contains(&bytes[1])
    }

    /// 旧 `phone` / 旧 Excel“联系电话”的迁移：手机号进 mobile，其余进 office。
    pub(crate) fn migrate_legacy(phone: &str) -> Self {
        let phone = phone.trim();
        if phone.is_empty() {
            return Self::default();
        }
        if Self::is_mobile(phone) {
            Self {
                office: String::new(),
                mobile: phone.to_string(),
            }
        } else {
            Self {
                office: phone.to_string(),
                mobile: String::new(),
            }
        }
    }

    /// 承办联系人取号：办公优先，没有办公再用手机。
    pub(crate) fn preferred_office_first(&self) -> &str {
        let office = self.office.trim();
        if office.is_empty() {
            self.mobile.trim()
        } else {
            office
        }
    }
}

/// 编号方案：公函、红头呈批件、会议纪要各一个池。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumberingScheme {
    Letter,
    Approval,
    Minutes,
}

impl NumberingScheme {
    pub(crate) fn for_kind(kind: TemplateKind) -> Option<Self> {
        match kind {
            TemplateKind::OfficialLetter => Some(Self::Letter),
            TemplateKind::RedHeadApproval => Some(Self::Approval),
            TemplateKind::MeetingMinutes => Some(Self::Minutes),
            _ => None,
        }
    }

    /// 纪要代字可以没有；公函、呈批件必须有。
    pub(crate) fn code_is_optional(self) -> bool {
        matches!(self, Self::Minutes)
    }

    /// 从单位词条取这一方案对应的代字；空串按“没有”处理。
    pub(crate) fn code(self, codes: &UnitCodes) -> Option<&str> {
        let value = match self {
            Self::Letter => codes.letter.trim(),
            Self::Approval => codes.approval.trim(),
            Self::Minutes => codes.minutes.as_deref().map_or("", str::trim),
        };
        (!value.is_empty()).then_some(value)
    }
}

/// 一份公文的编号：方案 + 代字 + 年度 + 序号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocNumber {
    pub scheme: NumberingScheme,
    /// `None` 或空串都表示不带代字（纪要允许，公函/呈批件由校验提示）。
    pub code: Option<String>,
    pub year: String,
    pub serial: String,
}

impl DocNumber {
    /// 按文种选方案；代字优先用表单已填的，空时从单位带出。
    /// 纪要以单位为准：单位有纪要代字就带，没有就保持空。
    pub(crate) fn from_draft(draft: &DraftInput, codes: &UnitCodes) -> Option<Self> {
        let scheme = NumberingScheme::for_kind(draft.kind)?;
        let profile_code = draft.profile.department_code.trim();
        let code = if profile_code.is_empty() {
            scheme.code(codes).map(str::to_string)
        } else {
            Some(profile_code.to_string())
        };
        let year = draft.document_year();
        let serial = draft.profile.document_number.trim().to_string();
        if year.is_empty() && serial.is_empty() {
            return None;
        }
        Some(Self {
            scheme,
            code,
            year,
            serial,
        })
    }

    /// 有代字 `某纪〔2026〕3号`；没有代字 `〔2026〕3号`。
    pub(crate) fn display(&self) -> String {
        if self.year.trim().is_empty() && self.serial.trim().is_empty() {
            return String::new();
        }
        let mut out = String::new();
        if let Some(code) = self
            .code
            .as_deref()
            .map(str::trim)
            .filter(|code| !code.is_empty())
        {
            out.push_str(code);
        }
        let year = self.year.trim();
        if !year.is_empty() {
            out.push('〔');
            out.push_str(year);
            out.push('〕');
        }
        out.push_str(self.serial.trim());
        out.push('号');
        out
    }
}

/// 密级与保密期限。研究报告的密级在封面元数据里，`DraftInput::security_marking`
/// 已经替两套来源归一，这里只存结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SecurityMark {
    pub level: String,
    pub period: String,
}

/// 一份文稿的事实。列表、搜索、导出、台账、汇编以后都从这里取，
/// 不再各自读 `DraftInput` 的字段或各自拼文号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocumentMeta {
    pub title: String,
    pub kind: TemplateKind,
    pub number: Option<DocNumber>,
    pub date_text: String,
    /// `YYYY-MM-DD`；无法解析时为空串，与稿件库 `doc_date_iso` 同口径。
    pub date_iso: String,
    pub security: SecurityMark,
    pub status: Option<ManuscriptStatus>,
}

impl DocumentMeta {
    /// 从表单快照取事实。标题由调用方给（正文标题/文件名称的取法在导出层），
    /// 这样平台不依赖 Markdown 解析器。
    pub(crate) fn from_draft(draft: &DraftInput, title: impl Into<String>) -> Self {
        let (level, period) = draft.security_marking();
        Self {
            title: title.into(),
            kind: draft.kind,
            // 表单里的 `profile.department_code` 已经是生效代字；这里不从词库猜。
            number: DocNumber::from_draft(draft, &UnitCodes::default()),
            date_text: draft.date.trim().to_string(),
            date_iso: parse_doc_date_iso(&draft.date),
            security: SecurityMark {
                level: level.trim().to_string(),
                period: period.trim().to_string(),
            },
            status: None,
        }
    }
}

/// 成文日期显示串 → `YYYY-MM-DD`，用于范围过滤。
/// 支持 `2026年8月6日`、`2026-08-06`、`2026/8/6` 等；缺月/日时补 `01`；
/// 无法解析返回空串。与稿件库 `doc_date_iso` 同一算法。
pub(crate) fn parse_doc_date_iso(value: &str) -> String {
    let parts: Vec<u32> = value
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse::<u32>().ok())
        .collect();
    let Some(&year) = parts.first() else {
        return String::new();
    };
    if !(1900..=2200).contains(&year) {
        return String::new();
    }
    let month = parts.get(1).copied().unwrap_or(1);
    let day = parts.get(2).copied().unwrap_or(1);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return String::new();
    }
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes() -> UnitCodes {
        UnitCodes {
            letter: "某函".into(),
            approval: "某呈".into(),
            minutes: Some("某纪".into()),
        }
    }

    fn draft(kind: TemplateKind, code: &str, year: &str, serial: &str) -> DraftInput {
        DraftInput {
            kind,
            date: String::new(),
            profile: TemplateProfile {
                department_code: code.into(),
                document_year: year.into(),
                document_number: serial.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn mobile_recognition_accepts_eleven_digits_with_common_separators() {
        for number in ["13812345678", "138-1234-5678", "138 1234 5678"] {
            assert!(PersonContact::is_mobile(number), "{number}");
        }
        for number in [
            "",
            "12345",
            "12345678",
            "010-12345678",
            "12812345678",
            "1381234567",
            "(010)12345678",
            "+8613812345678",
        ] {
            assert!(!PersonContact::is_mobile(number), "{number}");
        }
    }

    #[test]
    fn legacy_phone_falls_back_to_office_when_not_a_mobile() {
        let mobile = PersonContact::migrate_legacy("13812345678");
        assert_eq!(mobile.office, "");
        assert_eq!(mobile.mobile, "13812345678");

        for value in ["010-12345678", "12345", "12345678"] {
            let contact = PersonContact::migrate_legacy(value);
            assert_eq!(contact.office, value);
            assert_eq!(contact.mobile, "");
        }

        assert_eq!(PersonContact::migrate_legacy(""), PersonContact::default());
    }

    #[test]
    fn contact_prefers_office_then_mobile() {
        let both = PersonContact {
            office: "010-12345678".into(),
            mobile: "13812345678".into(),
        };
        assert_eq!(both.preferred_office_first(), "010-12345678");

        let mobile_only = PersonContact {
            office: "  ".into(),
            mobile: "13812345678".into(),
        };
        assert_eq!(mobile_only.preferred_office_first(), "13812345678");

        assert_eq!(PersonContact::default().preferred_office_first(), "");
    }

    #[test]
    fn numbering_schemes_cover_the_three_numbered_kinds() {
        assert_eq!(
            NumberingScheme::for_kind(TemplateKind::OfficialLetter),
            Some(NumberingScheme::Letter)
        );
        assert_eq!(
            NumberingScheme::for_kind(TemplateKind::RedHeadApproval),
            Some(NumberingScheme::Approval)
        );
        assert_eq!(
            NumberingScheme::for_kind(TemplateKind::MeetingMinutes),
            Some(NumberingScheme::Minutes)
        );
        assert_eq!(NumberingScheme::for_kind(TemplateKind::PlainDocument), None);
        assert!(NumberingScheme::Minutes.code_is_optional());
        assert!(!NumberingScheme::Letter.code_is_optional());
    }

    #[test]
    fn minutes_number_supports_both_unit_forms() {
        let with_code = DocNumber::from_draft(
            &draft(TemplateKind::MeetingMinutes, "", "2026", "3"),
            &codes(),
        )
        .expect("纪要应有编号");
        assert_eq!(with_code.display(), "某纪〔2026〕3号");

        let without_codes = UnitCodes {
            minutes: None,
            ..codes()
        };
        let without_code = DocNumber::from_draft(
            &draft(TemplateKind::MeetingMinutes, "", "2026", "3"),
            &without_codes,
        )
        .expect("纪要应有编号");
        assert_eq!(without_code.display(), "〔2026〕3号");
    }

    #[test]
    fn letter_number_uses_profile_code_then_unit_code() {
        let explicit = DocNumber::from_draft(
            &draft(TemplateKind::OfficialLetter, "手工函", "2026", "12"),
            &codes(),
        )
        .expect("公函应有编号");
        assert_eq!(explicit.display(), "手工函〔2026〕12号");

        let from_unit = DocNumber::from_draft(
            &draft(TemplateKind::OfficialLetter, "", "2026", "12"),
            &codes(),
        )
        .expect("公函应有编号");
        assert_eq!(from_unit.display(), "某函〔2026〕12号");
    }

    #[test]
    fn minutes_without_code_matches_the_current_paper_text() {
        let draft = draft(TemplateKind::MeetingMinutes, "", "2026", "3");
        let number = DocNumber::from_draft(
            &draft,
            &UnitCodes {
                minutes: None,
                ..codes()
            },
        )
        .expect("纪要应有编号");
        assert_eq!(
            number.display(),
            crate::export::minutes::minutes_number_text(&draft)
        );
    }

    #[test]
    fn document_meta_keeps_research_security_and_iso_date() {
        let mut draft = DraftInput {
            kind: TemplateKind::ResearchReport,
            date: "2026年8月6日".into(),
            ..Default::default()
        };
        draft.research.security = "内部".into();
        draft.research.security_years = "5年".into();
        let meta = DocumentMeta::from_draft(&draft, "研究报告");
        assert_eq!(meta.title, "研究报告");
        assert_eq!(meta.kind, TemplateKind::ResearchReport);
        assert_eq!(meta.number, None);
        assert_eq!(meta.date_text, "2026年8月6日");
        assert_eq!(meta.date_iso, "2026-08-06");
        assert_eq!(meta.security.level, "内部");
        assert_eq!(meta.security.period, "5年");
        assert_eq!(meta.status, None);
    }

    #[test]
    fn document_meta_uses_the_same_number_and_status_as_the_form() {
        let draft = draft(TemplateKind::OfficialLetter, "某函", "2026", "12");
        let meta = DocumentMeta::from_draft(&draft, "关于某事的通知");
        assert_eq!(meta.number.expect("应有文号").display(), "某函〔2026〕12号");
        assert_eq!(meta.status, None);
    }

    #[test]
    fn date_iso_parser_matches_manuscript_variants() {
        assert_eq!(parse_doc_date_iso("2026年8月6日"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026年08月06日"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026-08-06"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026/8/6"), "2026-08-06");
        assert_eq!(parse_doc_date_iso("2026年8月"), "2026-08-01");
        assert_eq!(parse_doc_date_iso("2026年"), "2026-01-01");
        assert_eq!(parse_doc_date_iso(""), "");
        assert_eq!(parse_doc_date_iso("另行通知"), "");
        assert_eq!(parse_doc_date_iso("9999年8月6日"), "");
    }

    #[test]
    fn a_draft_without_year_and_serial_has_no_number() {
        let draft = draft(TemplateKind::MeetingMinutes, "某纪", "", "");
        assert_eq!(DocNumber::from_draft(&draft, &codes()), None);
        let empty = DocNumber {
            scheme: NumberingScheme::Minutes,
            code: Some("某纪".into()),
            year: String::new(),
            serial: String::new(),
        };
        assert_eq!(empty.display(), "");
    }
}
