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

use crate::agent::clarify::{Action, Choice, Question, Reply, Target};
use crate::agent::{elements, tools::vocab};
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

// ================= 第 2 期：动笔前的「要素抽取」、分类与出题 =================
// （`docs/element-fill-design.md` 3.2。分工与六要素相同：模型只判断原文写没写、
// 原样摘出名称；摘录核对、词库解析、分类、出题全是确定性的。）

/// 一条要素建议。不出题直接建议的（原文写明、词库唯一对上）在动笔前这一步就进黑板；
/// 答完题选定的也进这里。写表单只发生在用户点采纳时（第 3 期建议卡，红线 2）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FieldSuggestion {
    pub(crate) field: FieldId,
    /// 建议值：词库词条的规范名；多值字段以顿号分隔。
    pub(crate) value: String,
    /// 生成建议那一刻的表单值：建议卡做过期检测、「替换」行标冲突用。
    pub(crate) previous: String,
    /// 出处：「材料原文：……」或「起草人从标准词库选定」。
    pub(crate) source: String,
    /// 表单已填且与建议值不一致：建议卡显示「替换」并标冲突。
    pub(crate) conflict: bool,
}

/// 技能里没写「要素抽取」这段提示词时用这一份。
pub(crate) const PROMPT: &str =
    "下面是一份{kind}的写作要求。逐项核对表单要素在原文里写没写、写的是谁。

{fields}

每个字段输出一行，格式严格如下（全角竖线分隔）：
字段名｜已给｜从原文里原样摘出的名称（多值字段有几个摘几个，用顿号分隔）
字段名｜未提

只判断原文写没写，拿不准就写「未提」。名称必须原样摘抄，不补全、不改写、不编造。

【原文】
{source}";

/// 「要素抽取」要核对的字段：当前文种适用、且不是联系电话（电话跟联系人从词库带出，
/// 不单独抽取也不单独出题）。
pub(crate) fn prompt_fields(draft: &DraftInput) -> Vec<FieldId> {
    FieldId::ALL
        .into_iter()
        .filter(|field| *field != FieldId::ContactPhone && field.applies(draft))
        .collect()
}

/// 提示词里的字段清单几行：「- 主送单位（多值，必填）」。
pub(crate) fn prompt_fields_text(fields: &[FieldId], kind: TemplateKind) -> String {
    fields
        .iter()
        .map(|field| {
            let tags = [
                field.multi().then_some("多值"),
                field.required(kind).then_some("必填"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if tags.is_empty() {
                format!("- {}", field.label())
            } else {
                format!("- {}（{}）", field.label(), tags.join("，"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl FieldId {
    fn from_label(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|field| text.contains(field.label()))
    }

    /// 候选的词库类别：单位字段只对单位，人员字段只对人员。
    fn category(self) -> VocabularyCategory {
        match self.source() {
            Source::Units { .. } => VocabularyCategory::Unit,
            _ => VocabularyCategory::Person,
        }
    }
}

/// 「已填要素：主送单位 市数据局；联系人 张三」——拼进六要素检查的原文，
/// 让「何人」算作已给（表单已填值 + 本批建议值）。
pub(crate) fn filled_summary(draft: &DraftInput, suggestions: &[FieldSuggestion]) -> String {
    let mut parts = Vec::new();
    for field in prompt_fields(draft) {
        let current = field.read(draft);
        let value = if !current.trim().is_empty() {
            current.trim().to_string()
        } else {
            match suggestions.iter().find(|s| s.field == field && !s.conflict) {
                Some(suggestion) => suggestion.value.clone(),
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

/// 模型对一行的判断：`Some(摘录)` 已给（多值字段顿号分隔），`None` 未提。
/// 回复里没出现的字段按未提处理。
fn parse_extraction(reply: &str) -> Vec<(FieldId, Option<Vec<String>>)> {
    let mut out: Vec<(FieldId, Option<Vec<String>>)> = Vec::new();
    for line in reply.lines() {
        let line = crate::agent::clarify::strip_numbering(line.trim());
        let parts: Vec<&str> = line.split(['｜', '|']).map(str::trim).collect();
        let (Some(head), Some(status)) = (parts.first(), parts.get(1)) else {
            continue;
        };
        let Some(field) = FieldId::from_label(head) else {
            continue;
        };
        if out.iter().any(|(seen, _)| *seen == field) {
            continue;
        }
        let verdict = if status.contains("未提") {
            None
        } else if status.contains("已给") {
            let rest = parts
                .get(2..)
                .map(|rest| rest.join("｜"))
                .unwrap_or_default();
            Some(split_units(&rest))
        } else {
            continue;
        };
        out.push((field, verdict));
    }
    out
}

/// 一条摘要对上词库里的哪些词条（规范名）。名称对词库用 `vocab::matches`
/// 同一套规则：规范名、对外名、简称、别名。
fn resolve<'a>(
    vocabulary: &'a [VocabularyEntry],
    field: FieldId,
    excerpt: &str,
) -> Vec<&'a VocabularyEntry> {
    if excerpt.trim().is_empty() {
        return Vec::new();
    }
    vocabulary
        .iter()
        .filter(|entry| {
            entry.category == field.category()
                && !entry.canonical.trim().is_empty()
                && vocab::matches(entry, excerpt.trim())
        })
        .collect()
}

/// 多值字段的集合比较：顺序不同、分隔符不同都算一致。
fn same_value(a: &str, b: &str) -> bool {
    let mut a = split_units(a);
    let mut b = split_units(b);
    a.sort();
    b.sort();
    a == b
}

/// 分类的产出：不出题直接进建议卡的，和要出的题。
#[derive(Debug, Default)]
pub(crate) struct FieldPlan {
    pub(crate) suggestions: Vec<FieldSuggestion>,
    pub(crate) questions: Vec<Question>,
}

/// 「人随事走」的过滤单位：联系人看承办单位，呈报领导看落款单位。
/// 取值顺序：表单值 → 本批建议 → 本批题目里原文对上的推荐项。
fn filter_units_for(person_field: FieldId, draft: &DraftInput, planned: &FieldPlan) -> Vec<String> {
    let unit_field = match person_field {
        FieldId::ContactPerson => FieldId::ResponsibleUnit,
        FieldId::ReportingLeaders => FieldId::SigningUnit,
        _ => return Vec::new(),
    };
    let current = unit_field.read(draft);
    if !current.trim().is_empty() {
        return split_units(&current);
    }
    if let Some(suggestion) = planned
        .suggestions
        .iter()
        .find(|s| s.field == unit_field && !s.conflict)
    {
        return split_units(&suggestion.value);
    }
    planned
        .questions
        .iter()
        .find(|q| q.target == Target::Field(unit_field))
        .map(|q| {
            q.choices
                .iter()
                .filter(|choice| choice.recommended)
                .filter_map(|choice| match &choice.action {
                    Action::Fill(value) => Some(value.clone()),
                    _ => None,
                })
                .take(1)
                .collect()
        })
        .unwrap_or_default()
}

/// 人员候选：按单位过滤（人随事走，最多 3 个）；单位没定的按个人简介匹配。
/// 联系人严格按承办权限过滤，单位下没有候选也不回落（与表单一致）；呈报领导
/// 过滤不出时按简介匹配兜底（与表单 `filtered_contacts` 的兜底一致）。
fn person_choices(
    field: FieldId,
    units: &[String],
    vocabulary: &[VocabularyEntry],
    source: &str,
) -> Vec<Choice> {
    let display = UnitDisplay::new(vocabulary);
    let mut out: Vec<Choice> = Vec::new();
    for unit in units {
        let people = match field {
            FieldId::ContactPerson => display.responsible_people_of(unit),
            _ => display.leaders_of(unit),
        };
        let role = match field {
            FieldId::ContactPerson => "的承办联系人",
            _ => "及其上级的领导",
        };
        for (name, _) in people {
            if out.iter().any(|c| c.label == name) {
                continue;
            }
            out.push(Choice {
                label: name.clone(),
                detail: format!("标准词库 · {unit}{role}"),
                recommended: false,
                action: Action::Fill(name),
            });
        }
    }
    if out.is_empty() && field != FieldId::ContactPerson {
        out.extend(crate::agent::clarify::vocabulary_choices(
            vocabulary,
            VocabularyCategory::Person,
            source,
            false,
        ));
    }
    out.truncate(3);
    out
}

/// 一道要素题：选项全部由程序给——原文对上的放前面标推荐，然后是职能 / 简介匹配
/// （人员随单位过滤），最多再补 3 个。`unmatched` 是词库里对不上的摘录，写进题面。
fn field_question(
    field: FieldId,
    draft: &DraftInput,
    planned: &FieldPlan,
    vocabulary: &[VocabularyEntry],
    source: &str,
    matched: Vec<String>,
    unmatched: Vec<String>,
) -> Question {
    let mut choices: Vec<Choice> = matched
        .iter()
        .map(|name| Choice {
            label: name.clone(),
            detail: "材料原文里写的就是它（已对上标准词库）".into(),
            recommended: true,
            action: Action::Fill(name.clone()),
        })
        .collect();
    let mut extra: Vec<Choice> = match field.category() {
        VocabularyCategory::Unit => crate::agent::clarify::vocabulary_choices(
            vocabulary,
            VocabularyCategory::Unit,
            source,
            false,
        ),
        _ => {
            let units = filter_units_for(field, draft, planned);
            person_choices(field, &units, vocabulary, source)
        }
    };
    extra.retain(|choice| !choices.iter().any(|seen| seen.action == choice.action));
    choices.extend(extra.into_iter().take(3));
    let mut text = if field.multi() {
        format!("{}是哪些？（可多选）", field.label())
    } else if field.category() == VocabularyCategory::Person {
        format!("{}是哪位？", field.label())
    } else {
        format!("{}是哪个单位？", field.label())
    };
    if !unmatched.is_empty() {
        text.push_str(&format!(
            "（原文写的「{}」词库里没有）",
            unmatched.join("、")
        ));
    }
    Question {
        id: 0, // plan 收尾时统一编号
        text,
        choices,
        custom_hint: Some("词库里没有？自己写（只作起草备注，不进表单）".into()),
        prefill: String::new(),
        skippable: true,
        multi: field.multi(),
        target: Target::Field(field),
    }
}

/// 确定性分类（`docs/element-fill-design.md` 3.2 的分类表）：模型的抽取回复 + 表单
/// 当前值 + 词库 → 建议与题。纯函数，不碰模型。
///
/// - 摘录在原文里找不到（`elements::squash` 同一套去标点比较）的按「未提」处理；
/// - 空 + 原文给了 + 唯一对上 → 建议（不出题）；对上多个或一部分对不上 → 出题，
///   对上的当推荐项，对不上的在题面说明；
/// - 空 + 未提 → 必填字段出题（职能 / 简介匹配），非必填不问；
/// - 已填 + 给了且与表单一致 → 不动；已填 + 不一致且唯一对上 → 「替换」建议，标冲突；
///   已填的其余情况一律不动。
/// - 题数超过 `max_questions` 时必填字段优先，同必填按登记顺序。
pub(crate) fn plan(
    draft: &DraftInput,
    reply: &str,
    source: &str,
    vocabulary: &[VocabularyEntry],
    max_questions: usize,
) -> FieldPlan {
    // 先处理单位字段再处理人员字段：人员题的「人随事走」过滤要看本批的单位结论。
    const ORDER: [FieldId; 7] = [
        FieldId::IssuingUnit,
        FieldId::Recipient,
        FieldId::CopiesTo,
        FieldId::SigningUnit,
        FieldId::ResponsibleUnit,
        FieldId::ReportingLeaders,
        FieldId::ContactPerson,
    ];
    let parsed = parse_extraction(reply);
    let haystack = elements::squash(source);
    let mut out = FieldPlan::default();
    for field in ORDER.into_iter().filter(|field| field.applies(draft)) {
        let current = field.read(draft);
        let excerpts: Vec<String> = parsed
            .iter()
            .find(|(seen, _)| *seen == field)
            .and_then(|(_, verdict)| verdict.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|excerpt| {
                let needle = elements::squash(excerpt);
                !needle.is_empty() && haystack.contains(&needle)
            })
            .collect();
        let resolved: Vec<Vec<String>> = excerpts
            .iter()
            .map(|excerpt| {
                resolve(vocabulary, field, excerpt)
                    .into_iter()
                    .map(|entry| entry.canonical.trim().to_string())
                    .collect()
            })
            .collect();
        let all_unique = !excerpts.is_empty() && resolved.iter().all(|names| names.len() == 1);
        let matched: Vec<String> = resolved
            .iter()
            .flatten()
            .fold(Vec::new(), |mut names, name| {
                if !names.contains(name) {
                    names.push(name.clone());
                }
                names
            });
        if !current.trim().is_empty() {
            // 已填：一致不动；不一致且唯一对上才给「替换」建议，标冲突；其余不动。
            if all_unique {
                let value = join_units(&matched);
                if !same_value(&value, &current) {
                    out.suggestions.push(FieldSuggestion {
                        field,
                        value,
                        previous: current,
                        source: format!("材料原文：「{}」", excerpts.join("、")),
                        conflict: true,
                    });
                }
            }
            continue;
        }
        if excerpts.is_empty() {
            if field.required(draft.kind) {
                out.questions.push(field_question(
                    field,
                    draft,
                    &out,
                    vocabulary,
                    source,
                    Vec::new(),
                    Vec::new(),
                ));
            }
            continue;
        }
        if all_unique {
            out.suggestions.push(FieldSuggestion {
                field,
                value: join_units(&matched),
                previous: current,
                source: format!("材料原文：「{}」", excerpts.join("、")),
                conflict: false,
            });
            continue;
        }
        let unmatched: Vec<String> = excerpts
            .iter()
            .zip(&resolved)
            .filter(|(_, names)| names.is_empty())
            .map(|(excerpt, _)| excerpt.clone())
            .collect();
        out.questions.push(field_question(
            field, draft, &out, vocabulary, source, matched, unmatched,
        ));
    }
    // 名额：必填字段优先，同必填按登记顺序（sort_by_key 稳定）。
    out.questions.sort_by_key(|question| match question.target {
        Target::Field(field) => !field.required(draft.kind),
        _ => true,
    });
    out.questions.truncate(max_questions);
    // 题号从 1 起；与方向题、六要素题同批时 merge_predraft 还会统一重排。
    for (index, question) in out.questions.iter_mut().enumerate() {
        question.id = index + 1;
    }
    out
}

/// 要素题的回答 → （要素建议，已确认信息）。只读表单快照算「生成时的表单值」，
/// 绝不回写（红线 2）；建议卡采纳才写表单（第 3 期）。
///
/// - 选中（单选 `Choice` / 多选 `Many`）→ 建议清单，出处「起草人从标准词库选定」，
///   同时记一句已确认信息交给起草；
/// - 自己写 → 只记已确认信息（词库外的值不进表单）；
/// - 跳过（或多选一个都没勾）→ 正文留「【待核实：…】」占位，同六要素。
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
                        "{}暂未确定：正文写「{}」，不要自己编",
                        field.label(),
                        elements::pending_literal(field.label())
                    ));
                } else {
                    let value = join_units(&picked);
                    let previous = field.read(draft);
                    let conflict = !previous.trim().is_empty() && !same_value(&previous, &value);
                    suggestions.push(FieldSuggestion {
                        field,
                        value: value.clone(),
                        previous,
                        source: "起草人从标准词库选定".into(),
                        conflict,
                    });
                    notes.push(format!(
                        "{}：{value}（起草人从标准词库选定）",
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

    // ================= 第 2 期：要素抽取的解析、分类（plan）与回答落地 =================

    /// 层级词库：市政府（00）下辖市数据局（0001，带数据职能）、市应急局（0002，带应急职能）；
    /// 张三挂在市数据局、王五挂在市应急局、赵六挂在市政府（领导）。
    fn plan_vocabulary() -> Vec<VocabularyEntry> {
        let unit = |name: &str, code: &str, parent: &str, duties: &str| VocabularyEntry {
            canonical: name.into(),
            code: code.into(),
            parent: parent.into(),
            duties: duties.into(),
            ..Default::default()
        };
        let person = |name: &str, unit: &str, position: &str, profile: &str| VocabularyEntry {
            canonical: name.into(),
            category: VocabularyCategory::Person,
            unit: unit.into(),
            position: position.into(),
            profile: profile.into(),
            ..Default::default()
        };
        vec![
            unit("市政府", "00", "", "综合协调"),
            unit(
                "市数据局",
                "0001",
                "00",
                "负责公共数据归集、共享与开放，统筹数据资源平台建设",
            ),
            unit(
                "市应急局",
                "0002",
                "00",
                "负责安全生产综合监督管理和应急救援",
            ),
            person("张三", "0001", "科员", "数据平台项目建设"),
            person("王五", "0002", "科长", "应急救援演练"),
            person("赵六", "00", "副市长", "分管数据与应急"),
        ]
    }

    /// 原文里写明市数据局：主送唯一对上（建议）、承办单位对上多个（出题）。
    const PLAN_SOURCE: &str =
        "关于共建平台的函\n给市数据局发函，商请共建公共数据研究平台，请数据局牵头推进。";

    fn question_of(planned: &FieldPlan, field: FieldId) -> Option<&Question> {
        planned
            .questions
            .iter()
            .find(|q| q.target == Target::Field(field))
    }

    fn suggestion_of(planned: &FieldPlan, field: FieldId) -> Option<&FieldSuggestion> {
        planned.suggestions.iter().find(|s| s.field == field)
    }

    /// 分类表第 1 行：空 + 给了 + 唯一对上 → 建议（不出题），出处写材料原文。
    #[test]
    fn plan_suggests_when_excerpt_resolves_uniquely() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        let reply = "主送单位｜已给｜市数据局\n抄送单位｜未提";
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 4);
        let suggestion = suggestion_of(&planned, FieldId::Recipient).expect("主送要进建议");
        assert_eq!(suggestion.value, "市数据局");
        assert!(!suggestion.conflict);
        assert!(suggestion.previous.is_empty());
        assert!(
            suggestion.source.contains("材料原文"),
            "{}",
            suggestion.source
        );
        assert!(
            question_of(&planned, FieldId::Recipient).is_none(),
            "唯一对上不出题"
        );
    }

    /// 分类表第 2 行：空 + 给了 + 对上多个 → 出题，对上的几个当选项、标推荐。
    #[test]
    fn plan_asks_with_matched_candidates_when_excerpt_is_ambiguous() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        // 「数据局」同时上市数据局与（假设的）省数据局：加进词库让它对上两个。
        let mut vocabulary = vocabulary;
        vocabulary.push(VocabularyEntry {
            canonical: "省数据局".into(),
            code: "02".into(),
            ..Default::default()
        });
        let reply = "主送单位｜已给｜数据局";
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 4);
        let question = question_of(&planned, FieldId::Recipient).expect("对上多个要出题");
        assert!(question.multi, "主送是多选题");
        assert!(suggestion_of(&planned, FieldId::Recipient).is_none());
        let recommended: Vec<_> = question
            .choices
            .iter()
            .filter(|c| c.recommended)
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(recommended, ["市数据局", "省数据局"]);
    }

    /// 分类表第 3 行：空 + 给了 + 一个都对不上 → 出题，题面说明原文写的词库里没有。
    #[test]
    fn plan_asks_and_names_the_unknown_excerpt() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        let source = "给市数据局发函，抄送市档案馆。";
        let reply = "抄送单位｜已给｜市档案馆";
        let planned = plan(&draft, reply, source, &vocabulary, 4);
        let question = question_of(&planned, FieldId::CopiesTo).expect("对不上也要出题");
        assert!(question.text.contains("市档案馆"), "{}", question.text);
        assert!(question.text.contains("词库里没有"), "{}", question.text);
        assert!(
            question.choices.iter().all(|c| !c.recommended),
            "对不上的不能标推荐"
        );
    }

    /// 分类表第 4 行：空 + 未提 → 必填字段出题（选项按职能匹配），非必填不问。
    #[test]
    fn plan_asks_required_fields_with_duty_candidates_only() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        let reply = "主送单位｜未提\n抄送单位｜未提";
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 4);
        let question = question_of(&planned, FieldId::Recipient).expect("主送必填要问");
        assert!(
            question.choices.iter().any(|c| c.label == "市数据局"),
            "职能对得上的进选项：{:?}",
            question.choices
        );
        assert!(
            question_of(&planned, FieldId::CopiesTo).is_none(),
            "抄送非必填不问"
        );
        assert!(planned.suggestions.is_empty());
    }

    /// 分类表第 5、7 行：已填 + 给了且一致 → 不动；已填 + 未提 → 不动。
    #[test]
    fn plan_leaves_consistent_filled_fields_alone() {
        let vocabulary = plan_vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.recipient = "市数据局".into();
        // 顺序不同也算一致。
        draft.profile.copies_to = "市应急局、市数据局".into();
        let source = "给市数据局发函，抄送市数据局、市应急局。";
        let reply = "主送单位｜已给｜市数据局\n抄送单位｜已给｜市数据局、市应急局";
        let planned = plan(&draft, reply, source, &vocabulary, 4);
        assert!(planned.suggestions.is_empty(), "{:?}", planned.suggestions);
        assert!(question_of(&planned, FieldId::Recipient).is_none());
        assert!(question_of(&planned, FieldId::CopiesTo).is_none());
        // 已填 + 未提 → 不动。
        let reply = "主送单位｜未提";
        let planned = plan(&draft, reply, source, &vocabulary, 4);
        assert!(planned.suggestions.is_empty());
        assert!(question_of(&planned, FieldId::Recipient).is_none());
    }

    /// 分类表第 6 行：已填 + 给了，不一致且唯一对上 → 「替换」建议，标冲突，记下表单原值。
    #[test]
    fn plan_marks_conflicting_replacement_for_filled_field() {
        let vocabulary = plan_vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.recipient = "市应急局".into();
        let reply = "主送单位｜已给｜市数据局";
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 4);
        let suggestion = suggestion_of(&planned, FieldId::Recipient).expect("要出替换建议");
        assert!(suggestion.conflict);
        assert_eq!(suggestion.previous, "市应急局");
        assert_eq!(suggestion.value, "市数据局");
        assert!(
            question_of(&planned, FieldId::Recipient).is_none(),
            "已填的不出题"
        );
        // 不一致但对上多个：不动（不猜）。
        let mut vocabulary = plan_vocabulary();
        vocabulary.push(VocabularyEntry {
            canonical: "省数据局".into(),
            code: "02".into(),
            ..Default::default()
        });
        let reply = "主送单位｜已给｜数据局";
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 4);
        assert!(planned.suggestions.is_empty());
        assert!(
            question_of(&planned, FieldId::Recipient).is_none(),
            "已填的不出题"
        );
    }

    /// 摘录在原文里找不到（去标点比较）的按「未提」处理：必填字段照问，不给建议。
    #[test]
    fn plan_treats_unverifiable_excerpts_as_missing() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        // 原文里没有「市数据局」：模型说给了不算数。
        let reply = "主送单位｜已给｜市数据局";
        let planned = plan(
            &draft,
            reply,
            "给有关部门发函，商请共建平台。",
            &vocabulary,
            4,
        );
        assert!(suggestion_of(&planned, FieldId::Recipient).is_none());
        assert!(question_of(&planned, FieldId::Recipient).is_some());
    }

    /// 多值字段：每条摘录都唯一对上才出建议，值按顿号合并。
    #[test]
    fn plan_suggests_multi_value_when_every_excerpt_is_unique() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        let source = "给市数据局发函，会同市应急局共建平台。";
        let reply = "主送单位｜已给｜市数据局、市应急局";
        let planned = plan(&draft, reply, source, &vocabulary, 4);
        let suggestion = suggestion_of(&planned, FieldId::Recipient).expect("多值建议");
        assert_eq!(suggestion.value, "市数据局、市应急局");
        // 一条对上、一条对不上：出题，对上的标推荐、对不上的写进题面。
        let source = "给市数据局发函，会同市档案馆共建平台。";
        let reply = "主送单位｜已给｜市数据局、市档案馆";
        let planned = plan(&draft, reply, source, &vocabulary, 4);
        let question = question_of(&planned, FieldId::Recipient).expect("部分对不上要出题");
        assert_eq!(
            question
                .choices
                .iter()
                .filter(|c| c.recommended)
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["市数据局"]
        );
        assert!(question.text.contains("市档案馆"), "{}", question.text);
    }

    /// 人员随单位：联系人题只列承办单位的人（表单值），别单位的人不出现。
    #[test]
    fn contact_question_is_filtered_by_the_responsible_unit() {
        let vocabulary = plan_vocabulary();
        let mut draft = draft_of(TemplateKind::RedHeadApproval);
        // 红头呈批件：承办条目第一条的单位是市数据局。
        apply_field(
            &mut draft,
            FieldId::ResponsibleUnit,
            "市数据局",
            &vocabulary,
        );
        let reply = "联系人｜未提";
        let planned = plan(&draft, reply, "拟开展数据平台项目。", &vocabulary, 6);
        let question = question_of(&planned, FieldId::ContactPerson).expect("联系人必填要问");
        let labels: Vec<_> = question.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["张三"], "只列承办单位的人：{labels:?}");
        // 呈报领导题只列落款单位及其上级的人（落款单位没填 → 按简介匹配）。
        let leaders = question_of(&planned, FieldId::ReportingLeaders).expect("呈报领导必填要问");
        assert!(!leaders.choices.is_empty());
    }

    /// 呈报领导按落款单位过滤：落款填了市数据局，候选是市数据局与市政府的人。
    #[test]
    fn leader_question_is_filtered_by_the_signing_unit() {
        let vocabulary = plan_vocabulary();
        let mut draft = draft_of(TemplateKind::WhitePaper);
        draft.profile.signing_unit = "市数据局".into();
        let reply = "呈报领导｜未提";
        let planned = plan(&draft, reply, "拟开展数据平台项目。", &vocabulary, 6);
        let question = question_of(&planned, FieldId::ReportingLeaders).expect("呈报领导必填要问");
        let labels: Vec<_> = question.choices.iter().map(|c| c.label.as_str()).collect();
        assert!(labels.contains(&"张三"), "{labels:?}");
        assert!(labels.contains(&"赵六"), "上级单位的领导也要在：{labels:?}");
        assert!(!labels.contains(&"王五"), "别单位的不在：{labels:?}");
        assert!(question.multi, "呈报领导是多选题");
    }

    /// 名额分配：字段题超出上限时必填字段优先。
    #[test]
    fn plan_prioritizes_required_questions_within_the_cap() {
        let mut vocabulary = plan_vocabulary();
        vocabulary.push(VocabularyEntry {
            canonical: "省数据局".into(),
            code: "02".into(),
            ..Default::default()
        });
        let draft = draft_of(TemplateKind::OfficialLetter);
        // 发文单位已填（未提 → 不动，不占名额）；主送（必填）未提，承办单位（公函里
        // 非必填）给了但对上多个：只问 1 题时是主送。
        let mut draft = draft;
        draft.profile.issuing_unit = "市数据局".into();
        let reply = "主送单位｜未提\n承办单位｜已给｜数据局";
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 1);
        assert_eq!(planned.questions.len(), 1);
        assert_eq!(
            planned.questions[0].target,
            Target::Field(FieldId::Recipient)
        );
        assert_eq!(planned.questions[0].id, 1);
    }

    /// 红线 2：plan 产出的题与建议一个授权类要素都不沾（出题与建议由登记表驱动，
    /// 登记表本身不含授权类——这个测试锁的是题面、选项与建议值）。
    #[test]
    fn plan_never_produces_authorized_elements() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::RedHeadApproval);
        // 全空 + 全未提：能出多少题出多少。
        let reply = "发文单位｜未提\n呈报领导｜未提\n落款单位｜未提\n承办单位｜未提\n联系人｜未提";
        let planned = plan(&draft, reply, "拟开展数据平台项目。", &vocabulary, 9);
        assert!(!planned.questions.is_empty());
        for word in ["密级", "文号", "份号", "签发", "成文日期", "印发"] {
            for question in &planned.questions {
                assert!(
                    matches!(question.target, Target::Field(_)),
                    "要素题只能是 Target::Field"
                );
                assert!(!question.text.contains(word), "{}", question.text);
                for choice in &question.choices {
                    assert!(!choice.label.contains(word), "{}", choice.label);
                }
            }
            for suggestion in &planned.suggestions {
                assert!(!suggestion.value.contains(word));
            }
        }
    }

    /// 回答落地：选中进建议清单（出处「起草人从标准词库选定」）并记已确认信息。
    #[test]
    fn resolve_answers_turns_picks_into_suggestions_and_notes() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        let reply = "主送单位｜已给｜数据局";
        let mut vocabulary = vocabulary;
        vocabulary.push(VocabularyEntry {
            canonical: "省数据局".into(),
            code: "02".into(),
            ..Default::default()
        });
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 4);
        let question = question_of(&planned, FieldId::Recipient).unwrap();
        let picked = question
            .choices
            .iter()
            .position(|c| c.label == "省数据局")
            .unwrap();
        let (suggestions, notes) = resolve_answers(
            &draft,
            &planned.questions,
            &[(question.id, Reply::Choice(picked))],
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].field, FieldId::Recipient);
        assert_eq!(suggestions[0].value, "省数据局");
        assert_eq!(suggestions[0].source, "起草人从标准词库选定");
        assert!(!suggestions[0].conflict);
        // 同批没答的必填题（发文单位）按跳过记了待核实，这里只看主送这句。
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位：省数据局（起草人从标准词库选定）"),
            "{notes:?}"
        );
    }

    /// 多选题：勾几个合几个；一个都没勾按跳过处理。
    #[test]
    fn resolve_answers_merges_multi_picks_and_treats_none_as_skip() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        let mut vocabulary = vocabulary;
        vocabulary.push(VocabularyEntry {
            canonical: "省数据局".into(),
            code: "02".into(),
            ..Default::default()
        });
        let planned = plan(
            &draft,
            "主送单位｜已给｜数据局",
            PLAN_SOURCE,
            &vocabulary,
            4,
        );
        let question = question_of(&planned, FieldId::Recipient).unwrap();
        let (suggestions, notes) = resolve_answers(
            &draft,
            &planned.questions,
            &[(question.id, Reply::Many(vec![0, 1]))],
        );
        assert_eq!(suggestions[0].value, "市数据局、省数据局");
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位：市数据局、省数据局（起草人从标准词库选定）"),
            "{notes:?}"
        );
        // 一个都没勾：按跳过处理，留待核实占位（同批发文单位题没答，默认也是跳过）。
        let (suggestions, notes) = resolve_answers(
            &draft,
            &planned.questions,
            &[(question.id, Reply::Many(vec![]))],
        );
        assert!(suggestions.is_empty());
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位暂未确定：正文写「【待核实：主送单位】」，不要自己编"),
            "{notes:?}"
        );
    }

    /// 自己写：只记已确认信息，不进建议清单（词库外的值不进表单）；跳过留待核实占位。
    #[test]
    fn resolve_answers_custom_is_a_note_only_and_skip_leaves_a_placeholder() {
        let vocabulary = plan_vocabulary();
        let draft = draft_of(TemplateKind::OfficialLetter);
        let planned = plan(&draft, "主送单位｜未提", PLAN_SOURCE, &vocabulary, 4);
        let question = question_of(&planned, FieldId::Recipient).unwrap();
        let (suggestions, notes) = resolve_answers(
            &draft,
            &planned.questions,
            &[(question.id, Reply::Custom("市档案馆".into()))],
        );
        assert!(suggestions.is_empty());
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位：市档案馆（起草人确认，词库外的值不进表单）"),
            "{notes:?}"
        );
        let (suggestions, notes) =
            resolve_answers(&draft, &planned.questions, &[(question.id, Reply::Skip)]);
        assert!(suggestions.is_empty());
        assert!(
            notes
                .iter()
                .any(|note| note == "主送单位暂未确定：正文写「【待核实：主送单位】」，不要自己编"),
            "{notes:?}"
        );
    }

    /// 答题那一刻表单已经被填上（等待期间用户在表单里改过）：建议标冲突，
    /// 生成时的表单值记进 previous。
    #[test]
    fn resolve_answers_marks_conflict_when_the_form_changed_while_answering() {
        let vocabulary = plan_vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        let planned = plan(&draft, "主送单位｜未提", PLAN_SOURCE, &vocabulary, 4);
        let question = question_of(&planned, FieldId::Recipient).unwrap();
        let picked = question
            .choices
            .iter()
            .position(|c| c.label == "市数据局")
            .unwrap();
        // 出题后用户在表单里填了市应急局。
        draft.profile.recipient = "市应急局".into();
        let (suggestions, _) = resolve_answers(
            &draft,
            &planned.questions,
            &[(question.id, Reply::Choice(picked))],
        );
        assert!(suggestions[0].conflict);
        assert_eq!(suggestions[0].previous, "市应急局");
    }

    /// 「已填要素」汇总：表单已填值 + 本批建议值（冲突建议不算），拼进六要素检查原文。
    #[test]
    fn filled_summary_lists_form_values_and_suggestions() {
        let vocabulary = plan_vocabulary();
        let mut draft = draft_of(TemplateKind::OfficialLetter);
        draft.profile.issuing_unit = "市政府".into();
        draft.profile.responsible_unit = "市数据局".into();
        let reply = "主送单位｜已给｜市数据局";
        let planned = plan(&draft, reply, PLAN_SOURCE, &vocabulary, 4);
        let summary = filled_summary(&draft, &planned.suggestions);
        assert_eq!(
            summary, "已填要素：发文单位 市政府；主送单位 市数据局；承办单位 市数据局",
            "{summary}"
        );
    }

    /// 旧检查点、旧会话：没有 `multi` 的题目、没有 `field_suggestions` 的黑板，读回不报错。
    #[test]
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
