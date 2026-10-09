//! 公文要素字段登记表：表单与 AI 共用的一份「这个文种要哪些描述类要素、各自从哪
//! 取值、怎么读写」（`docs/element-fill-design.md` 3.1）。
//!
//! 红线 2：授权类要素（密级与保密期限、文号年份与数字、份号、签发人、成文日期、
//! 版记印发机关与印发日期）不进这张表，本模块没有任何能写它们的代码路径；
//! 测试对每个字段逐一锁住这一点。
//!
//! 读写只有一条路径：[`apply_field`]。表单的下拉框和 AI 建议卡的采纳都调它，
//! 「人手工选」与「AI 建议采纳后」不会出现两种结果。主送 / 抄送 / 发文三者互斥
//! （规格 §2.4）、代字带出、承办条目同步都在这里处理。

use crate::models::{
    DraftInput, JointIssuanceMode, JointResponsibleEntry, TemplateKind, TemplateProfile,
    VocabularyCategory, VocabularyEntry, join_units, joint_responsible_entries, split_units,
    sync_joint_responsible,
};
use crate::units::UnitDisplay;

/// 描述类要素字段。授权类要素不在此列——AI 与程序建议都无权写它们。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum FieldId {
    /// 发文单位（公函、电话通知、红头呈批件；公函联合发文模式不参与）。
    IssuingUnit,
    /// 主送（公函、电话通知）。
    Recipient,
    /// 抄送（公函）。
    CopiesTo,
    /// 呈报领导（白头件、红头呈批件）。
    ReportingLeaders,
    /// 落款单位（白头件、红头呈批件）。
    SigningUnit,
    /// 承办单位（公函；联合发文模式与红头呈批件取承办条目第一条）。
    ResponsibleUnit,
    /// 联系人（同承办单位）。
    ContactPerson,
    /// 联系电话：不单独出题，跟联系人从词库带出。
    ContactPhone,
}

/// 候选项从哪来。第二期出题分流（`plan`）按它生成候选。
#[allow(dead_code)] // 第二期使用
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// 词库单位；主送、抄送、落款可多选。
    Units { multi: bool },
    /// 词库人员，按单位过滤（复用 `units::UnitDisplay` 的「人随事走」规则）。
    Persons { scope: PersonScope },
    /// 跟着某个人员字段从词库带出，不单独出题。
    PhoneOf(FieldId),
}

/// 人员候选的过滤范围。
#[allow(dead_code)] // 第二期按它过滤人员候选
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersonScope {
    /// 承办联系人：`responsible_people_of`（含可承办上级单位的人员）。
    Handlers,
    /// 呈报领导：`leaders_of`（落款单位及其上级的领导）。
    Leaders,
}

// 登记表的查询接口（label/kinds/applies/required/source/multi/read）：本期只有
// apply_field 与测试在用，第二期的出题分流与第三期的建议卡按它们查表。
#[allow(dead_code)]
impl FieldId {
    pub(crate) const ALL: [Self; 8] = [
        Self::IssuingUnit,
        Self::Recipient,
        Self::CopiesTo,
        Self::ReportingLeaders,
        Self::SigningUnit,
        Self::ResponsibleUnit,
        Self::ContactPerson,
        Self::ContactPhone,
    ];

    /// 中文名，与表单行标签一致。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::IssuingUnit => "发文单位",
            Self::Recipient => "主送单位",
            Self::CopiesTo => "抄送单位",
            Self::ReportingLeaders => "呈报领导",
            Self::SigningUnit => "落款单位",
            Self::ResponsibleUnit => "承办单位",
            Self::ContactPerson => "联系人",
            Self::ContactPhone => "联系电话",
        }
    }

    /// 适用文种。
    pub(crate) fn kinds(self) -> &'static [TemplateKind] {
        match self {
            Self::IssuingUnit => &[
                TemplateKind::OfficialLetter,
                TemplateKind::PhoneNotice,
                TemplateKind::RedHeadApproval,
            ],
            Self::Recipient => &[TemplateKind::OfficialLetter, TemplateKind::PhoneNotice],
            Self::CopiesTo => &[TemplateKind::OfficialLetter],
            Self::ReportingLeaders | Self::SigningUnit => {
                &[TemplateKind::WhitePaper, TemplateKind::RedHeadApproval]
            }
            Self::ResponsibleUnit | Self::ContactPerson | Self::ContactPhone => {
                &[TemplateKind::OfficialLetter, TemplateKind::RedHeadApproval]
            }
        }
    }

    /// 这个字段对当前稿件是否适用。联合发文模式下发文单位是
    /// `joint_issuing_units` 多选并要指定主发文单位，不在本框架内。
    pub(crate) fn applies(self, draft: &DraftInput) -> bool {
        if !self.kinds().contains(&draft.kind) {
            return false;
        }
        !(self == Self::IssuingUnit
            && draft.kind == TemplateKind::OfficialLetter
            && draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1)
    }

    /// 表单是否必填（与 `draft_page::form::check_form` 的必填项一致）。
    /// 红头呈批件的承办单位与联系人按「至少一条承办条目」整体必填，联系电话不单列。
    pub(crate) fn required(self, kind: TemplateKind) -> bool {
        match self {
            Self::IssuingUnit | Self::Recipient | Self::ReportingLeaders | Self::SigningUnit => {
                self.kinds().contains(&kind)
            }
            Self::ResponsibleUnit | Self::ContactPerson => kind == TemplateKind::RedHeadApproval,
            Self::CopiesTo | Self::ContactPhone => false,
        }
    }

    /// 候选项来源。
    pub(crate) fn source(self) -> Source {
        match self {
            Self::IssuingUnit
            | Self::Recipient
            | Self::CopiesTo
            | Self::SigningUnit
            | Self::ResponsibleUnit => Source::Units {
                multi: self.multi(),
            },
            Self::ReportingLeaders => Source::Persons {
                scope: PersonScope::Leaders,
            },
            Self::ContactPerson => Source::Persons {
                scope: PersonScope::Handlers,
            },
            Self::ContactPhone => Source::PhoneOf(Self::ContactPerson),
        }
    }

    /// 是否多值字段（表单上是多选，存储以顿号分隔）。
    pub(crate) fn multi(self) -> bool {
        matches!(
            self,
            Self::Recipient | Self::CopiesTo | Self::ReportingLeaders | Self::SigningUnit
        )
    }

    /// 读表单当前值。联合发文模式与红头呈批件的承办三要素取承办条目第一条。
    pub(crate) fn read(self, draft: &DraftInput) -> String {
        let profile = &draft.profile;
        let joint = uses_joint_responsible(profile, draft.kind);
        let first = || {
            joint_responsible_entries(profile)
                .into_iter()
                .next()
                .unwrap_or_default()
        };
        match self {
            Self::IssuingUnit => profile.issuing_unit.clone(),
            Self::Recipient => profile.recipient.clone(),
            Self::CopiesTo => profile.copies_to.clone(),
            Self::ReportingLeaders => profile.reporting_leaders.clone(),
            Self::SigningUnit => profile.signing_unit.clone(),
            Self::ResponsibleUnit => {
                if joint {
                    first().unit
                } else {
                    profile.responsible_unit.clone()
                }
            }
            Self::ContactPerson => {
                if joint {
                    first().name
                } else {
                    profile.contact_person.clone()
                }
            }
            Self::ContactPhone => {
                if joint {
                    first().phone
                } else {
                    profile.contact_phone.clone()
                }
            }
        }
    }
}

/// 承办三要素是否走成对条目（`joint_responsible_units` ↔ `joint_contacts`）：
/// 红头呈批件恒走条目；公函只在联合发文模式下走条目，否则用版记的单值字段。
fn uses_joint_responsible(profile: &TemplateProfile, kind: TemplateKind) -> bool {
    kind == TemplateKind::RedHeadApproval
        || (kind == TemplateKind::OfficialLetter
            && profile.joint_issuance_mode == JointIssuanceMode::Mode1)
}

/// 词库里某个人员绑定的电话；查不到（或词库里没维护电话）返回空串。
///
/// 先在承办单位可选的人里找（与表单联系人下拉的「人随事走」同一规则），词库里不同单位
/// 有同名的人时才不会带错电话；承办单位下没有这个人再按全词库第一个同名的取。
fn person_phone_of(vocabulary: &[VocabularyEntry], unit: &str, name: &str) -> String {
    if !unit.trim().is_empty()
        && let Some((_, phone)) = UnitDisplay::new(vocabulary)
            .responsible_people_of(unit)
            .into_iter()
            .find(|(person, _)| person == name)
    {
        return phone.trim().to_string();
    }
    vocabulary
        .iter()
        .find(|entry| {
            entry.category == VocabularyCategory::Person && entry.canonical.trim() == name
        })
        .map(|entry| entry.phone.trim().to_string())
        .unwrap_or_default()
}

/// 统一的写值入口：表单下拉框与 AI 建议采纳都走这里。
///
/// - 主送 / 抄送 / 发文互斥（规格 §2.4）：写入其中一个字段时，把同名单位从另外
///   两个字段剔除，新写入的值优先（下拉框本来就禁选已被占用的单位，剔除只在
///   手填重复、先选主送再选同一发文单位这类边角情形生效）。联合发文模式的
///   `joint_issuing_units` 不参与互斥。
/// - 发文单位按文种带出代字：公函用发函代字、红头呈批件用呈批代字，沿单位层级
///   继承；只在代字为空时填，电话通知不带代字。
/// - 联系人的电话从词库绑定带出；承办三要素在成对条目模式下写条目第一条并同步
///   `joint_responsible_units` / `joint_contacts`。
pub(crate) fn apply_field(
    draft: &mut DraftInput,
    field: FieldId,
    value: &str,
    vocabulary: &[VocabularyEntry],
) {
    apply_profile_field(&mut draft.profile, draft.kind, field, value, vocabulary)
}

/// `apply_field` 的 profile 级版本：表单控件手里只有 `TemplateProfile` 时用它。
pub(crate) fn apply_profile_field(
    profile: &mut TemplateProfile,
    kind: TemplateKind,
    field: FieldId,
    value: &str,
    vocabulary: &[VocabularyEntry],
) {
    let value = value.trim();
    match field {
        FieldId::IssuingUnit => {
            profile.issuing_unit = value.to_string();
            let picked = [value.to_string()];
            exclude_from_multi(&mut profile.recipient, &picked);
            exclude_from_multi(&mut profile.copies_to, &picked);
            if profile.department_code.trim().is_empty() {
                let display = UnitDisplay::new(vocabulary);
                let code = match kind {
                    TemplateKind::OfficialLetter => display.department_code_of(value),
                    TemplateKind::RedHeadApproval => display.approval_department_code_of(value),
                    _ => String::new(),
                };
                if !code.is_empty() {
                    profile.department_code = code;
                }
            }
        }
        FieldId::Recipient => {
            profile.recipient = value.to_string();
            let picked = split_units(value);
            exclude_from_multi(&mut profile.copies_to, &picked);
            exclude_issuing(profile, &picked);
        }
        FieldId::CopiesTo => {
            profile.copies_to = value.to_string();
            let picked = split_units(value);
            exclude_from_multi(&mut profile.recipient, &picked);
            exclude_issuing(profile, &picked);
        }
        FieldId::ReportingLeaders => profile.reporting_leaders = value.to_string(),
        FieldId::SigningUnit => profile.signing_unit = value.to_string(),
        FieldId::ResponsibleUnit => {
            if uses_joint_responsible(profile, kind) {
                let mut entries = joint_responsible_entries(profile);
                match entries.first_mut() {
                    Some(first) => first.unit = value.to_string(),
                    None if !value.is_empty() => entries.push(JointResponsibleEntry {
                        unit: value.to_string(),
                        ..Default::default()
                    }),
                    None => {}
                }
                sync_joint_responsible(profile, &entries);
            } else {
                profile.responsible_unit = value.to_string();
            }
        }
        FieldId::ContactPerson => {
            // 电话跟联系人从词库带出；词库外的人名不带电话（AI 侧的值必须先过词库校验）。
            if uses_joint_responsible(profile, kind) {
                let mut entries = joint_responsible_entries(profile);
                let unit = entries
                    .first()
                    .map(|first| first.unit.clone())
                    .unwrap_or_default();
                let phone = person_phone_of(vocabulary, &unit, value);
                match entries.first_mut() {
                    Some(first) => {
                        first.name = value.to_string();
                        first.phone = phone;
                    }
                    None if !value.is_empty() => entries.push(JointResponsibleEntry {
                        name: value.to_string(),
                        phone,
                        ..Default::default()
                    }),
                    None => {}
                }
                sync_joint_responsible(profile, &entries);
            } else {
                profile.contact_phone =
                    person_phone_of(vocabulary, &profile.responsible_unit, value);
                profile.contact_person = value.to_string();
            }
        }
        FieldId::ContactPhone => {
            if uses_joint_responsible(profile, kind) {
                let mut entries = joint_responsible_entries(profile);
                if let Some(first) = entries.first_mut() {
                    first.phone = value.to_string();
                }
                sync_joint_responsible(profile, &entries);
            } else {
                profile.contact_phone = value.to_string();
            }
        }
    }
}

/// 把 `picked` 里的单位从一个顿号分隔的多值字段中剔除。
fn exclude_from_multi(value: &mut String, picked: &[String]) {
    let kept = split_units(value)
        .into_iter()
        .filter(|unit| !picked.iter().any(|item| item == unit))
        .collect::<Vec<_>>();
    if kept.len() != split_units(value).len() {
        *value = join_units(&kept);
    }
}

/// 单值的发文单位与 `picked` 冲突时清空（代字保持现状，与表单里手动清掉发文单位一致）。
fn exclude_issuing(profile: &mut TemplateProfile, picked: &[String]) {
    let issuing = profile.issuing_unit.trim().to_string();
    if !issuing.is_empty() && picked.contains(&issuing) {
        profile.issuing_unit.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ResearchMetadata, VocabularyCategory};

    /// 两个单位（甲局带两种代字、乙局不带）加两个人员（张三挂在甲局、电话 123）。
    fn vocabulary() -> Vec<VocabularyEntry> {
        vec![
            VocabularyEntry {
                canonical: "甲局".into(),
                code: "00".into(),
                department_code: "甲函".into(),
                approval_department_code: "甲呈".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "乙局".into(),
                code: "01".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "张三".into(),
                category: VocabularyCategory::Person,
                unit: "00".into(),
                phone: "123".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "李四".into(),
                category: VocabularyCategory::Person,
                unit: "01".into(),
                position: "局长".into(),
                ..Default::default()
            },
        ]
    }

    fn draft_of(kind: TemplateKind) -> DraftInput {
        DraftInput {
            kind,
            profile: TemplateProfile::for_kind(kind),
            ..Default::default()
        }
    }

    /// 每个字段在它的每个适用文种下给一个像样的样例值。
    fn sample_value(field: FieldId) -> &'static str {
        match field {
            FieldId::IssuingUnit | FieldId::ResponsibleUnit => "甲局",
            FieldId::Recipient | FieldId::CopiesTo | FieldId::SigningUnit => "甲局、乙局",
            FieldId::ReportingLeaders => "李四",
            FieldId::ContactPerson => "张三",
            FieldId::ContactPhone => "456",
        }
    }

    /// 红线 2：对每个字段、每个适用文种跑一遍 `apply_field`，授权类要素与稿件
    /// 其余部分一个都不许变。
    #[test]
    fn apply_field_never_touches_authorized_fields() {
        let vocabulary = vocabulary();
        for field in FieldId::ALL {
            for kind in [
                TemplateKind::OfficialLetter,
                TemplateKind::PhoneNotice,
                TemplateKind::WhitePaper,
                TemplateKind::RedHeadApproval,
            ] {
                if !field.kinds().contains(&kind) {
                    continue;
                }
                let mut draft = draft_of(kind);
                draft.title_hint = "原标题".into();
                draft.date = "2026年10月9日".into();
                draft.profile.document_year = "2026".into();
                draft.profile.document_number = "5".into();
                draft.profile.number_copies = true;
                let before = draft.clone();
                apply_field(&mut draft, field, sample_value(field), &vocabulary);
                // 授权类：密级与保密期限、文号（年份与数字）、份号、成文日期。
                assert_eq!(
                    draft.profile.security_level, before.profile.security_level,
                    "{field:?}/{kind:?} 改了密级"
                );
                assert_eq!(
                    draft.profile.security_period, before.profile.security_period,
                    "{field:?}/{kind:?} 改了保密期限"
                );
                assert_eq!(
                    draft.profile.document_year, before.profile.document_year,
                    "{field:?}/{kind:?} 改了文号年份"
                );
                assert_eq!(
                    draft.profile.document_number, before.profile.document_number,
                    "{field:?}/{kind:?} 改了文号数字"
                );
                assert_eq!(
                    draft.profile.number_copies, before.profile.number_copies,
                    "{field:?}/{kind:?} 改了份号"
                );
                assert_eq!(draft.date, before.date, "{field:?}/{kind:?} 改了成文日期");
                assert_eq!(
                    draft.date_is_auto, before.date_is_auto,
                    "{field:?}/{kind:?} 改了成文日期开关"
                );
                // 表单其余部分与正文侧要素也不在这条写值路径上。
                assert_eq!(draft.kind, before.kind);
                assert_eq!(draft.title_hint, before.title_hint);
                assert_eq!(draft.meeting_time, before.meeting_time);
                assert_eq!(draft.attendees, before.attendees);
                assert_eq!(draft.research, before.research);
                assert_eq!(draft.phone_record, before.phone_record);
            }
        }
    }

    /// 登记表本身不含授权类要素：字段的中文名一个都不沾边。
    #[test]
    fn registry_has_no_authorized_field() {
        for field in FieldId::ALL {
            for word in ["密级", "文号", "份号", "签发", "成文日期", "印发"] {
                assert!(
                    !field.label().contains(word),
                    "{:?} 的中文名沾了授权类要素「{word}」",
                    field
                );
            }
        }
    }

    #[test]
    fn issuing_unit_brings_out_letter_code_only_when_empty() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.issuing_unit, "甲局");
        assert_eq!(draft.profile.department_code, "甲函");

        // 代字已填时不覆盖。
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.department_code = "旧代字".into();
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.department_code, "旧代字");

        // 词库单位没维护代字时不凭空造。
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        apply_field(&mut draft, FieldId::IssuingUnit, "乙局", &vocabulary);
        assert_eq!(draft.profile.department_code, "");
    }

    #[test]
    fn issuing_unit_code_follows_kind() {
        let vocabulary = vocabulary();
        // 红头呈批件用呈批代字。
        let mut draft = draft_of(TemplateKind::RedHeadApproval);
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.department_code, "甲呈");
        // 电话通知不带代字。
        let mut draft = draft_of(TemplateKind::PhoneNotice);
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.department_code, "");
    }

    #[test]
    fn recipient_copies_issuing_are_mutually_exclusive() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.recipient = "甲局、乙局".into();
        draft.profile.copies_to = "丙局".into();
        // 发文单位占了主送里的甲局：主送剔除，抄送不动。
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.recipient, "乙局");
        assert_eq!(draft.profile.copies_to, "丙局");
        // 主送选了抄送里的丙局：抄送剔除；主送选了发文单位：发文清空。
        apply_field(
            &mut draft,
            FieldId::Recipient,
            "乙局、丙局、甲局",
            &vocabulary,
        );
        assert_eq!(draft.profile.copies_to, "");
        assert_eq!(draft.profile.issuing_unit, "");
        // 抄送与主送同样互斥。
        apply_field(&mut draft, FieldId::CopiesTo, "乙局", &vocabulary);
        assert_eq!(draft.profile.recipient, "丙局、甲局");
    }

    #[test]
    fn contact_person_brings_out_bound_phone() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.contact_phone = "旧电话".into();
        apply_field(&mut draft, FieldId::ContactPerson, "张三", &vocabulary);
        assert_eq!(draft.profile.contact_person, "张三");
        assert_eq!(draft.profile.contact_phone, "123");
        // 词库外的人名不带电话，并清掉与新联系人对不上的旧电话。
        apply_field(
            &mut draft,
            FieldId::ContactPerson,
            "词库外的人",
            &vocabulary,
        );
        assert_eq!(draft.profile.contact_phone, "");
    }

    /// 不同单位有同名的人：电话按承办单位下的那位带，不按词库里第一个同名的。
    #[test]
    fn same_name_contacts_take_the_phone_of_the_responsible_unit() {
        let mut vocabulary = vocabulary();
        vocabulary.push(VocabularyEntry {
            canonical: "张三".into(),
            category: VocabularyCategory::Person,
            unit: "01".into(),
            phone: "789".into(),
            ..Default::default()
        });
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        apply_field(&mut draft, FieldId::ResponsibleUnit, "乙局", &vocabulary);
        apply_field(&mut draft, FieldId::ContactPerson, "张三", &vocabulary);
        assert_eq!(draft.profile.contact_phone, "789");

        let mut draft = draft_of(TemplateKind::RedHeadApproval);
        apply_field(&mut draft, FieldId::ResponsibleUnit, "乙局", &vocabulary);
        apply_field(&mut draft, FieldId::ContactPerson, "张三", &vocabulary);
        assert_eq!(FieldId::ContactPhone.read(&draft), "789");
    }

    #[test]
    fn joint_responsible_first_entry_stays_in_sync() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::RedHeadApproval);
        apply_field(&mut draft, FieldId::ResponsibleUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.joint_responsible_units, "甲局");
        assert_eq!(draft.profile.joint_contacts.len(), 1);
        assert_eq!(draft.profile.joint_contacts[0].unit, "甲局");

        apply_field(&mut draft, FieldId::ContactPerson, "张三", &vocabulary);
        let entries = joint_responsible_entries(&draft.profile);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (
                entries[0].unit.as_str(),
                entries[0].name.as_str(),
                entries[0].phone.as_str()
            ),
            ("甲局", "张三", "123")
        );
        // 读值函数与写值走的是同一条路径。
        assert_eq!(FieldId::ResponsibleUnit.read(&draft), "甲局");
        assert_eq!(FieldId::ContactPerson.read(&draft), "张三");
        assert_eq!(FieldId::ContactPhone.read(&draft), "123");

        // 已有第二条条目时只改第一条。
        apply_field(&mut draft, FieldId::ResponsibleUnit, "乙局", &vocabulary);
        let mut expected = joint_responsible_entries(&draft.profile);
        expected.push(JointResponsibleEntry {
            unit: "丙局".into(),
            name: "李四".into(),
            phone: "456".into(),
        });
        sync_joint_responsible(&mut draft.profile, &expected);
        apply_field(&mut draft, FieldId::ResponsibleUnit, "甲局", &vocabulary);
        let entries = joint_responsible_entries(&draft.profile);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].unit, "甲局");
        assert_eq!(entries[1].unit, "丙局");
        assert_eq!(entries[1].name, "李四");
    }

    /// 公函联合发文模式的承办三要素同样走承办条目，与红头呈批件一致。
    #[test]
    fn joint_letter_uses_joint_responsible_entries() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        apply_field(&mut draft, FieldId::ResponsibleUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.joint_responsible_units, "甲局");
        assert_eq!(draft.profile.responsible_unit, "");
        // 联合发文模式下发文单位不参与本框架。
        assert!(!FieldId::IssuingUnit.applies(&draft));
        assert!(FieldId::ResponsibleUnit.applies(&draft));
    }

    /// 非联合公函的承办三要素走版记单值字段。
    #[test]
    fn single_letter_uses_record_fields() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        apply_field(&mut draft, FieldId::ResponsibleUnit, "甲局", &vocabulary);
        apply_field(&mut draft, FieldId::ContactPerson, "张三", &vocabulary);
        apply_field(&mut draft, FieldId::ContactPhone, "456", &vocabulary);
        assert_eq!(draft.profile.responsible_unit, "甲局");
        assert_eq!(draft.profile.contact_person, "张三");
        assert_eq!(draft.profile.contact_phone, "456");
        assert!(draft.profile.joint_contacts.is_empty());
    }

    /// 必填标记与 check_form 对齐抽查：公函承办非必填，红头呈批件承办必填。
    #[test]
    fn required_matches_check_form() {
        assert!(!FieldId::ResponsibleUnit.required(TemplateKind::OfficialLetter));
        assert!(FieldId::ResponsibleUnit.required(TemplateKind::RedHeadApproval));
        assert!(FieldId::Recipient.required(TemplateKind::OfficialLetter));
        assert!(FieldId::Recipient.required(TemplateKind::PhoneNotice));
        assert!(!FieldId::CopiesTo.required(TemplateKind::OfficialLetter));
        assert!(FieldId::ReportingLeaders.required(TemplateKind::WhitePaper));
        assert!(FieldId::SigningUnit.required(TemplateKind::RedHeadApproval));
        // 研究报告、会议议程、接听电话记录不在本框架内。
        for field in FieldId::ALL {
            for kind in [
                TemplateKind::ResearchReport,
                TemplateKind::MeetingAgenda,
                TemplateKind::PhoneRecord,
                TemplateKind::PlainDocument,
            ] {
                assert!(
                    !field.kinds().contains(&kind),
                    "{field:?} 不该适用 {kind:?}"
                );
            }
        }
    }

    /// 登记表自身的完整性与一致性。
    #[test]
    fn registry_is_consistent() {
        assert_eq!(FieldId::ALL.len(), 8);
        for field in FieldId::ALL {
            // 多值标记与来源声明一致。
            if let Source::Units { multi } = field.source() {
                assert_eq!(multi, field.multi(), "{field:?} 的 multi 不一致");
            }
            // 电话不单独出题，永远跟着联系人。
            if field == FieldId::ContactPhone {
                assert_eq!(field.source(), Source::PhoneOf(FieldId::ContactPerson));
            }
            // 每个字段至少适用一个文种。
            assert!(!field.kinds().is_empty());
        }
    }

    /// 未被改动的稿件部分：研究报告元数据这类独立结构体必须原样保留
    /// （`ResearchMetadata` 参与 PartialEq，顺带验证比较可行）。
    #[test]
    fn research_metadata_untouched() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.research = ResearchMetadata::default();
        let before = draft.research.clone();
        apply_field(&mut draft, FieldId::Recipient, "甲局", &vocabulary);
        assert_eq!(draft.research, before);
    }
}
