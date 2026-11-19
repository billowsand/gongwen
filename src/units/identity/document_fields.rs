//! 实际参与成文的身份字段，表单保存所需的隐藏值由 identity::fields 单独遍历。
use crate::models::{DraftInput, JointIssuanceMode, TemplateKind, VocabularyCategory as Category};

pub(super) fn active_fields(
    input: &DraftInput,
    with_transmit: bool,
    mut visit: impl FnMut(&str, Category, &'static str),
) {
    let p = &input.profile;
    let values: &[(&str, Category, &str)] = match input.kind {
        TemplateKind::OfficialLetter => {
            if p.joint_issuance_mode == JointIssuanceMode::Mode1 {
                visit(&p.joint_issuing_units, Category::Unit, "联合发文单位");
                visit(&p.main_issuing_unit, Category::Unit, "主发文单位");
            } else {
                visit(&p.issuing_unit, Category::Unit, "发文单位");
            }
            if with_transmit {
                let receiving = crate::models::receiving_units(p);
                for row in &p.receiving_contacts {
                    if receiving.contains(&row.unit) {
                        visit(&row.name, Category::Person, "传发任务单收件人");
                    }
                }
            }
            &[
                (&p.recipient, Category::Unit, "主送单位"),
                (&p.copies_to, Category::Unit, "抄送单位"),
                (&p.signing_unit, Category::Unit, "落款单位"),
            ]
        }
        TemplateKind::RedHeadApproval => &[
            (&p.issuing_unit, Category::Unit, "发文单位"),
            (&p.reporting_leaders, Category::Person, "呈报领导"),
            (&p.signing_unit, Category::Unit, "落款单位"),
        ],
        TemplateKind::WhitePaper => &[
            (&p.reporting_leaders, Category::Person, "呈报领导"),
            (&p.signing_unit, Category::Unit, "落款单位"),
        ],
        TemplateKind::PhoneNotice => &[
            (&p.issuing_unit, Category::Unit, "发文单位"),
            (&p.recipient, Category::Unit, "主送单位"),
            (&p.signing_unit, Category::Unit, "落款单位"),
        ],
        TemplateKind::PhoneRecord => &[
            (&input.phone_record.institution, Category::Unit, "本单位"),
            (&input.phone_record.caller_unit, Category::Unit, "来电单位"),
            (
                &input.phone_record.caller_person,
                Category::Person,
                "谈话人",
            ),
            (&p.responsible_unit, Category::Unit, "承办单位"),
            (&p.contact_person, Category::Person, "联系人"),
        ],
        TemplateKind::MeetingMinutes => &[
            (&p.issuing_unit, Category::Unit, "发文单位"),
            (&p.copies_to, Category::Unit, "抄送单位"),
            (&input.minutes.printing_unit, Category::Unit, "印发单位"),
        ],
        TemplateKind::MeetingAgenda
        | TemplateKind::PlainDocument
        | TemplateKind::ResearchReport => &[],
    };
    for &(value, category, field) in values {
        visit(value, category, field);
    }
    let rows = match input.kind {
        TemplateKind::OfficialLetter => p.letter_responsible_entries(),
        TemplateKind::RedHeadApproval => crate::models::joint_responsible_entries(p),
        _ => Vec::new(),
    };
    for row in rows {
        visit(&row.unit, Category::Unit, "承办条目中的单位");
        visit(&row.name, Category::Person, "承办条目中的联系人");
    }
}

pub(crate) fn active_keys(
    input: &DraftInput,
    with_transmit: bool,
) -> Vec<(String, Category, &'static str)> {
    let mut keys = Vec::new();
    active_fields(input, with_transmit, |value, category, field| {
        keys.extend(
            crate::models::split_units(value)
                .into_iter()
                .map(|key| (key, category, field)),
        );
    });
    keys
}
