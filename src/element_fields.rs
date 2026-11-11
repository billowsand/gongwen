//! 公文要素字段登记表：表单与 AI 共用的一份「这个文种要哪些描述类要素、各自从哪
//! 取值、怎么读写」（历史方案 `docs/element-fill-design.md`）。
//!
//! 「要素抽取」生成链路已于 2026-11-05 退役：动笔前不再调模型核对原文、不再出表单
//! 要素题、不再发建议卡。这里只保留登记表、统一写值入口，以及旧会话回放要用的建议
//! 结构与挂起要素题的回答落地。
//!
//! 红线 2：授权类要素（密级与保密期限、文号年份与数字、份号、签发人、成文日期、
//! 版记印发机关与印发日期）不进这张表，本模块没有任何能写它们的代码路径；
//! 测试对每个字段逐一锁住这一点。
//!
//! 读写只有一条路径：[`apply_field`]。表单的下拉框和 AI 建议卡的采纳都调它，
//! 「人手工选」与「AI 建议采纳后」不会出现两种结果。主送 / 抄送 / 发文三者互斥
//! （规格 §2.4）、代字带出、承办条目同步都在这里处理。

#[cfg(feature = "ai-workbench")]
use crate::agent::clarify::{Action, Question, Reply, Target};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersonScope {
    /// 承办联系人：`responsible_people_of`（含可承办上级单位的人员）。
    Handlers,
    /// 呈报领导：`leaders_of`（落款单位及其上级的领导）。
    Leaders,
}

impl FieldId {
    #[cfg(any(test, feature = "ai-workbench"))]
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
    #[cfg(any(test, feature = "ai-workbench"))]
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
    #[cfg(any(test, feature = "ai-workbench"))]
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
    #[cfg(any(test, feature = "ai-workbench"))]
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
    #[cfg(any(test, feature = "ai-workbench"))]
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
    #[cfg(any(test, feature = "ai-workbench"))]
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
    let Some(person) =
        crate::units::identity::resolve(vocabulary, VocabularyCategory::Person, name)
    else {
        return String::new();
    };
    if !unit.trim().is_empty()
        && !UnitDisplay::new(vocabulary)
            .responsible_people_of(unit)
            .iter()
            .any(|(key, _)| key == &person.key())
    {
        return String::new();
    }
    person.phone.trim().to_string()
}

/// 统一的写值入口：表单下拉框与 AI 建议采纳都走这里。
///
/// - 主送 / 抄送 / 发文互斥（规格 §2.4）：写入其中一个字段时，把同一身份单位从另外
///   两个字段剔除，新写入的值优先（下拉框本来就禁选已被占用的单位，剔除只在
///   手填重复、先选主送再选同一发文单位这类边角情形生效）。联合发文模式的
///   `joint_issuing_units` 不参与互斥。
/// - 发文单位按文种带出代字：公函用发函代字、红头呈批件用呈批代字，沿单位层级
///   继承；公函代字随发文单位更新，呈批代字只在空值时填，电话通知不带代字。
/// - 联系人的电话从词库绑定带出；承办三要素在成对条目模式下写条目第一条并同步
///   `joint_responsible_units` / `joint_contacts`。
pub(crate) fn apply_field(
    draft: &mut DraftInput,
    field: FieldId,
    value: &str,
    vocabulary: &[VocabularyEntry],
) {
    apply_profile_field(&mut draft.profile, draft.kind, field, value, vocabulary);
    crate::units::identity::capture_input(draft, vocabulary)
}

/// 公函机关代字始终绑定当前发文单位；联合发文使用主发文单位。
/// 年份、序号不参与联动。单位未维护代字时清空，不能沿用上一单位的文号。
pub(crate) fn sync_letter_department_code(
    profile: &mut TemplateProfile,
    vocabulary: &[VocabularyEntry],
) {
    let issuing = if profile.joint_issuance_mode != crate::models::JointIssuanceMode::Mode1 {
        profile.issuing_unit.clone()
    } else {
        let units = split_units(&profile.joint_issuing_units);
        units
            .iter()
            .find(|unit| **unit == profile.main_issuing_unit)
            .or_else(|| units.first())
            .cloned()
            .unwrap_or_default()
    };
    profile.department_code = UnitDisplay::new(vocabulary).department_code_of(&issuing);
}

/// `apply_field` 的 profile 级版本：表单控件手里只有 `TemplateProfile` 时用它。
pub(crate) fn apply_profile_field(
    profile: &mut TemplateProfile,
    kind: TemplateKind,
    field: FieldId,
    value: &str,
    vocabulary: &[VocabularyEntry],
) {
    crate::units::identity::capture(profile, vocabulary);
    let value = split_units(value)
        .iter()
        .map(|key| {
            crate::units::identity::resolve(vocabulary, field.category(), key)
                .map_or_else(|| key.clone(), |e| e.key())
        })
        .collect::<Vec<_>>()
        .join("、");
    let value = value.trim();
    crate::units::identity::refresh_snapshot(profile, vocabulary, value);
    match field {
        FieldId::IssuingUnit => {
            profile.issuing_unit = value.to_string();
            let picked = [value.to_string()];
            exclude_from_multi(&mut profile.recipient, &picked);
            exclude_from_multi(&mut profile.copies_to, &picked);
            if kind == TemplateKind::OfficialLetter || profile.department_code.trim().is_empty() {
                let display = UnitDisplay::new(vocabulary);
                let code = match kind {
                    TemplateKind::OfficialLetter => display.department_code_of(value),
                    TemplateKind::RedHeadApproval => display.approval_department_code_of(value),
                    _ => String::new(),
                };
                if kind == TemplateKind::OfficialLetter || !code.is_empty() {
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
    if field == FieldId::ResponsibleUnit {
        let display = UnitDisplay::new(vocabulary);
        if uses_joint_responsible(profile, kind) {
            for row in profile.joint_contacts.iter_mut().take(1) {
                if !row.unit.is_empty()
                    && !display
                        .responsible_people_of(&row.unit)
                        .iter()
                        .any(|(key, _)| key == &row.name)
                {
                    row.name.clear();
                    row.phone.clear();
                }
            }
        } else if !profile.responsible_unit.is_empty()
            && !display
                .responsible_people_of(&profile.responsible_unit)
                .iter()
                .any(|(key, _)| key == &profile.contact_person)
        {
            profile.contact_person.clear();
            profile.contact_phone.clear();
        }
    }
    crate::units::identity::capture(profile, vocabulary);
    if kind == TemplateKind::OfficialLetter
        && matches!(
            field,
            FieldId::IssuingUnit | FieldId::Recipient | FieldId::CopiesTo
        )
    {
        sync_letter_department_code(profile, vocabulary);
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

// ================= 要素建议（旧会话兼容） =================
// 「要素抽取」的生成链路已于 2026-11-05 退役：不再调模型核对原文、不再按词库出
// 表单要素题，新任务只写正文、不生成要素建议。这里只保留旧会话回放要用的数据
// 结构、写值入口（仍在红线 2 的约束下）与挂起要素题的回答落地。

/// 一条要素建议。旧会话侧栏的建议卡仍要显示、采纳与撤销；新流程不再生成。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg(any(test, feature = "ai-workbench"))]
pub(crate) struct FieldSuggestion {
    pub(crate) field: FieldId,
    #[serde(default)]
    pub(crate) display_value: String,
    /// 建议值：带类别的永久身份引用（兼容旧名称）；多值字段以顿号分隔。
    pub(crate) value: String,
    /// 生成建议那一刻的表单值：建议卡做过期检测、「替换」行标冲突用。
    pub(crate) previous: String,
    /// 出处：「材料原文：……」或「起草人从标准词库选定」。
    pub(crate) source: String,
    /// 表单已填且与建议值不一致：建议卡显示「替换」并标冲突。
    pub(crate) conflict: bool,
}

impl FieldId {
    /// 候选的词库类别：单位字段只对单位，人员字段只对人员。
    fn category(self) -> VocabularyCategory {
        match self.source() {
            Source::Units { .. } => VocabularyCategory::Unit,
            _ => VocabularyCategory::Person,
        }
    }
}

/// 「已填要素：主送单位 市数据局；联系人 张三」——拼进六要素检查的原文，让「何人」
/// 算作已给（表单已填值 + 黑板上的旧建议值）。要素抽取退役后不再有新建议，旧会话
/// 恢复的黑板上可能还带着建议清单，这里照样算进已填。
#[cfg(any(test, feature = "ai-workbench"))]
pub(crate) fn filled_summary(draft: &DraftInput, suggestions: &[FieldSuggestion]) -> String {
    let mut parts = Vec::new();
    for field in FieldId::ALL
        .into_iter()
        .filter(|field| *field != FieldId::ContactPhone && field.applies(draft))
    {
        let current = field.read(draft);
        let value = if !current.trim().is_empty() {
            crate::units::identity::display_values(&draft.profile.vocabulary_snapshot, &current)
        } else {
            match suggestions.iter().find(|s| s.field == field && !s.conflict) {
                Some(suggestion) => {
                    if suggestion.display_value.is_empty() {
                        // 旧会话反序列化后可能没有 display_value：按快照现算名称。
                        crate::units::identity::display_values(
                            &draft.profile.vocabulary_snapshot,
                            &suggestion.value,
                        )
                    } else {
                        suggestion.display_value.clone()
                    }
                }
                None => continue,
            }
        };
        parts.push(format!("{} {value}", field.label()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("已填要素：{}", parts.join("；"))
    }
}

/// 多值字段的集合比较：顺序不同、分隔符不同都算一致。建议卡的提醒也用它。
#[cfg(feature = "ai-workbench")]
pub(crate) fn same_value(a: &str, b: &str) -> bool {
    let mut a = split_units(a);
    let mut b = split_units(b);
    a.sort();
    b.sort();
    a == b
}

/// 旧会话挂起的要素题的回答 → （要素建议，已确认信息）。只读表单快照算「生成时的
/// 表单值」，绝不回写（红线 2）；建议卡采纳才写表单。
///
/// - 选中（单选 `Choice` / 多选 `Many`）→ 建议清单，出处「起草人从标准词库选定」，
///   同时记一句已确认信息交给起草；
/// - 自己写 → 只记已确认信息（词库外的值不进表单）；
/// - 跳过（或多选一个都没勾）→ 记一句「之后在表单里填」。表单要素由版式从表单渲染，
///   起草提示词禁止写进正文（`prompt.rs`），所以不留「【待核实】」占位。
#[cfg(feature = "ai-workbench")]
pub(crate) fn resolve_answers(
    draft: &DraftInput,
    questions: &[Question],
    replies: &[(usize, Reply)],
) -> (Vec<FieldSuggestion>, Vec<String>) {
    let mut suggestions = Vec::new();
    let mut notes = Vec::new();
    for question in questions {
        let Target::Field(field) = question.target else {
            continue;
        };
        let reply = replies
            .iter()
            .find(|(id, _)| *id == question.id)
            .map_or(&Reply::Skip, |(_, reply)| reply);
        let fill = |index: usize| match question.choices.get(index).map(|c| &c.action) {
            Some(Action::Fill(value)) => Some(value.clone()),
            _ => None,
        };
        match reply {
            Reply::Custom(text) if !text.trim().is_empty() => {
                notes.push(format!(
                    "{}：{}（起草人确认，词库外的值不进表单）",
                    field.label(),
                    text.trim()
                ));
            }
            _ => {
                let picked: Vec<String> = match reply {
                    Reply::Choice(index) => fill(*index).into_iter().collect(),
                    Reply::Many(indices) => {
                        indices.iter().filter_map(|index| fill(*index)).collect()
                    }
                    _ => Vec::new(),
                };
                if picked.is_empty() {
                    notes.push(format!(
                        "{}暂未确定：由起草人之后在要素表单里选，正文不写这一项，也不要留占位或自己编",
                        field.label()
                    ));
                } else {
                    let value = join_units(&picked);
                    let previous = field.read(draft);
                    let conflict = !previous.trim().is_empty() && !same_value(&previous, &value);
                    suggestions.push(FieldSuggestion {
                        display_value: picked
                            .iter()
                            .filter_map(|value| {
                                question
                                    .choices
                                    .iter()
                                    .find(|c| c.action == Action::Fill(value.clone()))
                            })
                            .map(|c| c.label.clone())
                            .collect::<Vec<_>>()
                            .join("、"),
                        field,
                        value: value.clone(),
                        previous,
                        source: "起草人从标准词库选定".into(),
                        conflict,
                    });
                    let label = suggestions
                        .last()
                        .map(|s| s.display_value.as_str())
                        .unwrap_or("");
                    notes.push(format!(
                        "{}：{label}（起草人从标准词库选定）",
                        field.label()
                    ));
                }
            }
        }
    }
    (suggestions, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "ai-workbench")]
    use crate::agent::clarify::Choice;
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
    fn letter_code_tracks_switch_clear_and_joint_main_without_changing_number() {
        let mut vocabulary = vocabulary();
        vocabulary[1].department_code = "乙函".into();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.document_year = "2026".into();
        draft.profile.document_number = "12".into();
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        apply_field(&mut draft, FieldId::IssuingUnit, "乙局", &vocabulary);
        assert_eq!(draft.profile.department_code, "乙函");
        apply_field(&mut draft, FieldId::IssuingUnit, "", &vocabulary);
        assert!(draft.profile.department_code.is_empty());
        draft.profile.joint_issuance_mode = crate::models::JointIssuanceMode::Mode1;
        draft.profile.joint_issuing_units = "甲局、乙局".into();
        draft.profile.main_issuing_unit = "乙局".into();
        sync_letter_department_code(&mut draft.profile, &vocabulary);
        assert_eq!(draft.profile.department_code, "乙函");
        draft.profile.main_issuing_unit = "甲局".into();
        sync_letter_department_code(&mut draft.profile, &vocabulary);
        assert_eq!(draft.profile.department_code, "甲函");
        assert_eq!(draft.profile.document_year, "2026");
        assert_eq!(draft.profile.document_number, "12");
    }

    #[test]
    fn issuing_unit_replaces_letter_code() {
        let vocabulary = vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.issuing_unit, "甲局");
        assert_eq!(draft.profile.department_code, "甲函");

        // 公函代字已填时也必须随单位替换。
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.department_code = "旧代字".into();
        apply_field(&mut draft, FieldId::IssuingUnit, "甲局", &vocabulary);
        assert_eq!(draft.profile.department_code, "甲函");

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
        crate::units::normalize(&mut vocabulary);
        let selected = vocabulary.iter().find(|e| e.phone == "789").unwrap().key();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        apply_field(&mut draft, FieldId::ResponsibleUnit, "乙局", &vocabulary);
        apply_field(&mut draft, FieldId::ContactPerson, &selected, &vocabulary);
        assert_eq!(draft.profile.contact_phone, "789");

        let mut draft = draft_of(TemplateKind::RedHeadApproval);
        apply_field(&mut draft, FieldId::ResponsibleUnit, "乙局", &vocabulary);
        apply_field(&mut draft, FieldId::ContactPerson, &selected, &vocabulary);
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

    // ================= 回答落地（旧会话挂起的要素题） =================

    /// 造一道旧会话的要素题：选项都按「选中」处理。
    #[cfg(feature = "ai-workbench")]
    fn field_question(field: FieldId, labels: &[&str], multi: bool) -> Question {
        Question {
            id: 1,
            text: format!("{}是哪个？", field.label()),
            choices: labels
                .iter()
                .map(|label| Choice {
                    label: (*label).to_string(),
                    detail: String::new(),
                    recommended: false,
                    action: Action::Fill((*label).to_string()),
                })
                .collect(),
            custom_hint: None,
            prefill: String::new(),
            skippable: true,
            multi,
            target: Target::Field(field),
        }
    }

    /// 要素题跳过时写给起草的那句话（与 `resolve_answers` 的格式一致）。
    #[cfg(feature = "ai-workbench")]
    fn skip_note(field: FieldId) -> String {
        format!(
            "{}暂未确定：由起草人之后在要素表单里选，正文不写这一项，也不要留占位或自己编",
            field.label()
        )
    }

    /// 回答落地：选中进建议清单（出处「起草人从标准词库选定」）并记已确认信息。
    #[test]
    #[cfg(feature = "ai-workbench")]
    fn resolve_answers_turns_picks_into_suggestions_and_notes() {
        let draft = draft_of(TemplateKind::OfficialLetter);
        let question = field_question(FieldId::Recipient, &["市数据局", "省数据局"], true);
        let (suggestions, notes) = resolve_answers(
            &draft,
            std::slice::from_ref(&question),
            &[(1, Reply::Choice(1))],
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].field, FieldId::Recipient);
        assert_eq!(suggestions[0].value, "省数据局");
        assert_eq!(suggestions[0].source, "起草人从标准词库选定");
        assert!(!suggestions[0].conflict);
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位：省数据局（起草人从标准词库选定）"),
            "{notes:?}"
        );
    }

    /// 多选题：勾几个合几个；一个都没勾按跳过处理。
    #[test]
    #[cfg(feature = "ai-workbench")]
    fn resolve_answers_merges_multi_picks_and_treats_none_as_skip() {
        let draft = draft_of(TemplateKind::OfficialLetter);
        let question = field_question(FieldId::Recipient, &["市数据局", "省数据局"], true);
        let questions = std::slice::from_ref(&question);
        let (suggestions, notes) =
            resolve_answers(&draft, questions, &[(1, Reply::Many(vec![0, 1]))]);
        assert_eq!(suggestions[0].value, "市数据局、省数据局");
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位：市数据局、省数据局（起草人从标准词库选定）"),
            "{notes:?}"
        );
        // 一个都没勾：按跳过处理，记「之后在表单里填」。
        let (suggestions, notes) = resolve_answers(&draft, questions, &[(1, Reply::Many(vec![]))]);
        assert!(suggestions.is_empty());
        assert!(
            notes
                .iter()
                .any(|note| *note == skip_note(FieldId::Recipient)),
            "{notes:?}"
        );
    }

    /// 自己写：只记已确认信息，不进建议清单（词库外的值不进表单）；跳过记「之后在表单里填」。
    #[test]
    #[cfg(feature = "ai-workbench")]
    fn resolve_answers_custom_is_a_note_only_and_skip_leaves_a_placeholder() {
        let draft = draft_of(TemplateKind::OfficialLetter);
        let question = field_question(FieldId::Recipient, &["市数据局"], true);
        let questions = std::slice::from_ref(&question);
        let (suggestions, notes) =
            resolve_answers(&draft, questions, &[(1, Reply::Custom("市档案馆".into()))]);
        assert!(suggestions.is_empty());
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位：市档案馆（起草人确认，词库外的值不进表单）"),
            "{notes:?}"
        );
        let (suggestions, notes) = resolve_answers(&draft, questions, &[(1, Reply::Skip)]);
        assert!(suggestions.is_empty());
        assert!(
            notes
                .iter()
                .any(|note| *note == skip_note(FieldId::Recipient)),
            "{notes:?}"
        );
    }

    /// 答题那一刻表单已经被填上（等待期间用户在表单里改过）：建议标冲突，
    /// 生成时的表单值记进 previous。
    #[test]
    #[cfg(feature = "ai-workbench")]
    fn resolve_answers_marks_conflict_when_the_form_changed_while_answering() {
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        let question = field_question(FieldId::Recipient, &["市数据局"], true);
        // 出题后用户在表单里填了市应急局。
        draft.profile.recipient = "市应急局".into();
        let (suggestions, _) = resolve_answers(
            &draft,
            std::slice::from_ref(&question),
            &[(1, Reply::Choice(0))],
        );
        assert!(suggestions[0].conflict);
        assert_eq!(suggestions[0].previous, "市应急局");
    }

    /// 「已填要素」汇总：表单已填值 + 旧会话带回来的建议值（冲突建议不算），
    /// 拼进六要素检查原文。
    #[test]
    fn filled_summary_lists_form_values_and_suggestions() {
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.issuing_unit = "市政府".into();
        draft.profile.responsible_unit = "市数据局".into();
        let suggestions = [FieldSuggestion {
            field: FieldId::Recipient,
            display_value: "市数据局".into(),
            value: "市数据局".into(),
            previous: String::new(),
            source: "材料原文：「市数据局」".into(),
            conflict: false,
        }];
        let summary = filled_summary(&draft, &suggestions);
        assert_eq!(
            summary, "已填要素：发文单位 市政府；主送单位 市数据局；承办单位 市数据局",
            "{summary}"
        );
    }

    /// 旧检查点、旧会话：没有 `multi` 的题目、没有 `field_suggestions` 的黑板，读回不报错。
    #[test]
    #[cfg(feature = "ai-workbench")]
    fn old_sessions_without_multi_and_suggestions_still_load() {
        let question: Question = serde_json::from_str(
            r#"{"id":1,"text":"主送单位是哪些？","choices":[],"custom_hint":null,"prefill":"","skippable":true,"target":"PreDraft"}"#,
        )
        .unwrap();
        assert!(!question.multi, "旧会话读回按单选");
        let board: crate::agent::board::Board = serde_json::from_str("{}").unwrap();
        assert!(board.field_suggestions.is_empty());
    }
}
