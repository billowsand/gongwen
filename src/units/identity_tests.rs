//! 身份引用回归：覆盖真实选择、联动、快照、显示及交换链，不能只测名称拼接。
use super::{UnitDisplay, identity, normalize};
use crate::element_fields::{FieldId, apply_field};
use crate::models::{DraftInput, TemplateKind, VocabularyCategory as C, VocabularyEntry as E};

pub(crate) fn fixture() -> Vec<E> {
    let unit = |code: &str, parent: &str, name: &str| E {
        code: code.into(),
        parent: parent.into(),
        canonical: name.into(),
        ..Default::default()
    };
    let person = |unit: &str, code: &str, phone: &str| E {
        category: C::Person,
        unit: unit.into(),
        code: code.into(),
        canonical: "张宁".into(),
        position: "主任".into(),
        phone: phone.into(),
        ..Default::default()
    };
    let mut entries = vec![
        unit("A", "", "甲市"),
        unit("A01", "A", "教育局"),
        unit("A0101", "A01", "办公室"),
        unit("B", "", "乙市"),
        unit("B01", "B", "教育局"),
        unit("B0101", "B01", "办公室"),
        person("A0101", "07", "010-1111"),
        person("A0101", "19", "010-2222"),
        person("B0101", "07", "020-3333"),
    ];
    normalize(&mut entries);
    entries
}
fn key(v: &[E], code: &str) -> String {
    v.iter()
        .find(|e| e.category == C::Unit && e.code == code)
        .unwrap()
        .key()
}
fn person(v: &[E], phone: &str) -> String {
    v.iter().find(|e| e.phone == phone).unwrap().key()
}

#[test]
fn same_parent_rejects_duplicates_but_different_branches_and_people_are_independent() {
    let mut entries = fixture();
    identity::validate(&entries).unwrap();
    assert_ne!(person(&entries, "010-1111"), person(&entries, "010-2222"));
    let mut duplicate = entries.iter().find(|e| e.code == "A0101").unwrap().clone();
    duplicate.id = 999;
    duplicate.identity = uuid::Uuid::new_v4().to_string();
    duplicate.code = "A0102".into();
    entries.push(duplicate);
    assert!(
        identity::validate(&entries)
            .unwrap_err()
            .contains("同一父节点")
    );
}

#[test]
fn ambiguous_legacy_names_and_name_equal_to_other_code_never_pick_first() {
    let mut entries = fixture();
    assert!(identity::resolve(&entries, C::Unit, "办公室").is_none());
    assert!(identity::resolve(&entries, C::Person, "张宁").is_none());
    entries
        .iter_mut()
        .find(|e| e.code == "B")
        .unwrap()
        .canonical = "A".into();
    assert!(identity::resolve(&entries, C::Unit, "A").is_none());
    let mut draft = DraftInput::default();
    draft.profile.recipient = "办公室".into();
    identity::capture_input(&mut draft, &entries);
    assert_eq!(draft.profile.recipient, "@unresolved:办公室");
    assert!(
        UnitDisplay::new(&entries)
            .for_input(&draft)
            .ensure_resolved(&draft)
            .is_err()
    );
}

#[test]
fn selected_person_controls_phone_and_unit_filter_even_when_all_names_are_equal() {
    let entries = fixture();
    let mut draft = DraftInput::default();
    apply_field(
        &mut draft,
        FieldId::ResponsibleUnit,
        &key(&entries, "A0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::ContactPerson,
        &person(&entries, "010-2222"),
        &entries,
    );
    assert_eq!(draft.profile.contact_phone, "010-2222");
    assert_eq!(
        UnitDisplay::new(&entries)
            .responsible_people_of(&key(&entries, "A0101"))
            .len(),
        2
    );
    apply_field(
        &mut draft,
        FieldId::ResponsibleUnit,
        &key(&entries, "B0101"),
        &entries,
    );
    assert!(draft.profile.contact_person.is_empty());
    assert!(draft.profile.contact_phone.is_empty());
    apply_field(
        &mut draft,
        FieldId::ContactPerson,
        &person(&entries, "020-3333"),
        &entries,
    );
    assert_eq!(draft.profile.contact_phone, "020-3333");
    apply_field(&mut draft, FieldId::ContactPerson, "张宁", &entries);
    assert!(draft.profile.contact_phone.is_empty());
}

#[test]
fn same_name_different_units_do_not_exclude_each_other() {
    let entries = fixture();
    let mut draft = DraftInput::default();
    apply_field(
        &mut draft,
        FieldId::IssuingUnit,
        &key(&entries, "A0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::CopiesTo,
        &key(&entries, "B0101"),
        &entries,
    );
    assert_eq!(draft.profile.issuing_unit, key(&entries, "A0101"));
    assert_eq!(draft.profile.copies_to, key(&entries, "B0101"));
}

#[test]
fn whole_document_qualifies_same_names_across_header_copies_and_footer() {
    let entries = fixture();
    let mut draft = DraftInput::default();
    apply_field(
        &mut draft,
        FieldId::IssuingUnit,
        &key(&entries, "A0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::CopiesTo,
        &key(&entries, "B0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::ResponsibleUnit,
        &key(&entries, "B01"),
        &entries,
    );
    let display = UnitDisplay::new(&entries).for_input(&draft);
    display.ensure_resolved(&draft).unwrap();
    assert_eq!(
        display.full_name(&draft.profile.issuing_unit),
        "甲市教育局办公室"
    );
    assert_eq!(
        crate::export::element_display::copies_to_display(&draft, &display),
        "乙市教育局办公室"
    );
    assert_eq!(
        crate::preview::header_unit(&draft, &display),
        "甲市教育局办公室"
    );
    let receipt = crate::export::typst::receipt::build(&draft, "# 同名测试", &display);
    let json = serde_json::to_string(&receipt).unwrap();
    assert!(json.contains("乙市教育局办公室"));
    assert!(!json.contains("@unit:"));
}

#[test]
fn snapshots_survive_move_rename_delete_and_code_reuse_without_changing_history() {
    let mut entries = fixture();
    let mut draft = DraftInput::default();
    apply_field(
        &mut draft,
        FieldId::IssuingUnit,
        &key(&entries, "A0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::CopiesTo,
        &key(&entries, "B0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::ResponsibleUnit,
        &key(&entries, "A0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::ContactPerson,
        &person(&entries, "010-1111"),
        &entries,
    );
    let serialized = serde_json::to_string(&draft).unwrap();
    let old_identity = key(&entries, "A0101");
    let id = entries.iter().find(|e| e.key() == old_identity).unwrap().id;
    // 改名后移到另一个上级，业务编码随之变化，永久身份不变。
    entries.iter_mut().find(|e| e.id == id).unwrap().canonical = "综合处".into();
    super::relocate_entry(&mut entries, id, "B01", super::SiblingPosition::Last).unwrap();
    assert_eq!(
        entries.iter().find(|e| e.id == id).unwrap().key(),
        old_identity
    );
    assert_ne!(entries.iter().find(|e| e.id == id).unwrap().code, "A0101");
    entries.retain(|e| e.key() != old_identity && e.phone != "010-1111");
    let display = UnitDisplay::new(&entries).for_input(&draft);
    display.ensure_resolved(&draft).unwrap();
    assert_eq!(display.full_name(&old_identity), "甲市教育局办公室");
    assert_eq!(draft.profile.contact_phone, "010-1111");
    assert_eq!(serde_json::to_string(&draft).unwrap(), serialized);
    let restored: DraftInput = serde_json::from_str(&serialized).unwrap();
    assert_eq!(
        UnitDisplay::new(&[])
            .for_input(&restored)
            .full_name(&old_identity),
        "甲市教育局办公室"
    );
}

#[test]
fn person_codes_are_preserved_through_sort_and_xlsx_identity_round_trip() {
    let mut entries = fixture();
    let before = person(&entries, "010-2222");
    entries.reverse();
    normalize(&mut entries);
    let e = entries.iter().find(|e| e.key() == before).unwrap();
    assert_eq!(e.code, "19");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("同名词库.xlsx");
    let codes: Vec<_> = entries
        .iter()
        .filter(|e| e.category == C::Unit)
        .map(|e| e.code.clone())
        .collect();
    crate::vocabulary_xlsx::to_xlsx(&entries, &path, &codes).unwrap();
    let restored = crate::vocabulary_xlsx::parse(&path, &codes)
        .unwrap()
        .entries;
    assert_eq!(
        restored.iter().find(|e| e.key() == before).unwrap().code,
        "19"
    );
    let mut merged = entries.clone();
    crate::vocabulary_xlsx::merge(&mut merged, restored).unwrap();
    assert_eq!(merged.len(), entries.len());
}

#[test]
fn identity_merge_renames_one_person_and_does_not_merge_same_name_people() {
    let mut entries = fixture();
    let mut incoming = entries
        .iter()
        .find(|e| e.phone == "010-2222")
        .unwrap()
        .clone();
    let identity = incoming.identity.clone();
    incoming.canonical = "张宁新名".into();
    crate::vocabulary_xlsx::merge(&mut entries, vec![incoming]).unwrap();
    assert_eq!(
        entries.iter().filter(|e| e.category == C::Person).count(),
        3
    );
    assert_eq!(
        entries
            .iter()
            .find(|e| e.identity == identity)
            .unwrap()
            .canonical,
        "张宁新名"
    );
    let before = entries.clone();
    let mut conflict = entries.iter().find(|e| e.code == "B01").unwrap().clone();
    conflict.identity = uuid::Uuid::new_v4().to_string();
    assert!(crate::vocabulary_xlsx::merge(&mut entries, vec![conflict]).is_err());
    assert_eq!(entries, before);
}

#[test]
fn ai_same_name_requires_identity_choice_and_card_keeps_distinct_people() {
    let entries = fixture();
    let draft = DraftInput {
        kind: TemplateKind::OfficialLetter,
        ..Default::default()
    };
    let plan = crate::element_fields::plan(
        &draft,
        "主送单位｜已给｜办公室",
        "请办公室反馈意见",
        &entries,
        8,
    );
    assert!(
        !plan
            .suggestions
            .iter()
            .any(|s| s.field == FieldId::Recipient)
    );
    let question = plan
        .questions
        .iter()
        .find(|q| q.target == crate::agent::clarify::Target::Field(FieldId::Recipient))
        .unwrap();
    let actions: Vec<_> = question
        .choices
        .iter()
        .filter_map(|c| {
            if let crate::agent::clarify::Action::Fill(key) = &c.action {
                Some(key)
            } else {
                None
            }
        })
        .collect();
    assert!(actions.contains(&&key(&entries, "A0101")));
    assert!(actions.contains(&&key(&entries, "B0101")));
    assert!(
        question
            .choices
            .iter()
            .any(|c| c.label == "甲市教育局办公室")
    );
}

#[test]
fn same_surname_leaders_are_not_collapsed_to_indistinguishable_titles() {
    let mut entries = fixture();
    let first = person(&entries, "010-1111");
    let second = person(&entries, "020-3333");
    entries
        .iter_mut()
        .find(|e| e.key() == second)
        .unwrap()
        .canonical = "张明".into();
    let result = UnitDisplay::new(&entries).reporting_leaders(&format!("{first}、{second}"));
    assert!(result.contains("张宁"));
    assert!(result.contains("张明"));
}

#[test]
fn failed_same_parent_move_leaves_the_wordbook_unchanged() {
    let mut entries = fixture();
    let before = entries.clone();
    let id = entries.iter().find(|e| e.code == "A0101").unwrap().id;
    assert!(super::relocate_entry(&mut entries, id, "B01", super::SiblingPosition::Last).is_err());
    assert_eq!(entries, before);
}

#[test]
fn legacy_hidden_code_column_is_not_a_permanent_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("旧模板.xlsx");
    let mut book = rust_xlsxwriter::Workbook::new();
    let unit = book.add_worksheet();
    unit.set_name("单位").unwrap();
    unit.write_string(0, 0, "层级编码").unwrap();
    unit.write_string(0, 1, "单位名称").unwrap();
    unit.write_string(1, 0, "A").unwrap();
    unit.write_string(1, 1, "甲单位").unwrap();
    unit.write_string(1, 10, "A").unwrap();
    book.add_worksheet().set_name("人员").unwrap();
    book.save(&path).unwrap();
    let imported = crate::vocabulary_xlsx::parse(&path, &[]).unwrap();
    assert!(imported.entries[0].identity.is_empty());
}

#[test]
#[ignore = "生成本次试用样张、示例词库和页面 PNG，需本机字体"]
fn typst_samples_identity() {
    let entries = fixture();
    let mut draft = DraftInput {
        title_hint: "关于反馈有关工作情况的函".into(),
        ..Default::default()
    };
    for (field, value) in [
        (FieldId::IssuingUnit, key(&entries, "A0101")),
        (FieldId::CopiesTo, key(&entries, "B0101")),
        (FieldId::Recipient, key(&entries, "B01")),
        (FieldId::ResponsibleUnit, key(&entries, "A0101")),
        (FieldId::ContactPerson, person(&entries, "010-2222")),
    ] {
        apply_field(&mut draft, field, &value, &entries);
    }
    let dir = std::path::Path::new("output/vocabulary-identity-trial");
    std::fs::create_dir_all(dir).unwrap();
    let codes = entries
        .iter()
        .filter(|e| e.category == C::Unit)
        .map(|e| e.code.clone())
        .collect::<Vec<_>>();
    crate::vocabulary_xlsx::to_xlsx(&entries, &dir.join("同名单位与人员示例词库.xlsx"), &codes)
        .unwrap();
    std::fs::write(
        dir.join("sample.json"),
        serde_json::to_vec_pretty(&draft).unwrap(),
    )
    .unwrap();
    let display = UnitDisplay::new(&entries);
    let markdown = "# 关于反馈有关工作情况的函\n\n请结合工作职责，反馈有关工作进展及下一步安排。\n\n一、核对单位归属。请各承办单位逐项核实有关信息。\n\n二、落实联系人。有关事宜请与指定联系人联系。";
    crate::export::write_docx(
        &dir.join("同名单位消歧样张.docx"),
        &draft,
        markdown,
        &display,
    )
    .unwrap();
    let outcome = crate::export::write_pdf(
        &dir.join("同名单位消歧样张.pdf"),
        &draft,
        markdown,
        &display,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    let pdf =
        hayro::hayro_interpret::hayro_syntax::Pdf::new(std::sync::Arc::new(outcome.pdf)).unwrap();
    for (index, page) in pdf.pages().iter().enumerate() {
        let pixmap = hayro::render(
            page,
            &hayro::RenderCache::new(),
            &hayro::hayro_interpret::InterpreterSettings::default(),
            &hayro::RenderSettings {
                x_scale: 2.0,
                y_scale: 2.0,
                bg_color: hayro::vello_cpu::color::palette::css::WHITE,
                ..Default::default()
            },
        );
        image::RgbaImage::from_raw(
            u32::from(pixmap.width()),
            u32::from(pixmap.height()),
            pixmap.data_as_u8_slice().to_vec(),
        )
        .unwrap()
        .save(dir.join(format!("page-{}.png", index + 1)))
        .unwrap();
    }
}

#[test]
fn arbitrary_child_codes_move_using_parent_identity_and_empty_codes_are_allocated_safely() {
    let mut entries = fixture();
    let parent_id = entries
        .iter()
        .find(|e| e.code == "A01")
        .unwrap()
        .identity
        .clone();
    let mut child = E {
        canonical: "综合组".into(),
        code: "CUSTOM-X".into(),
        parent: "A01".into(),
        owner_identity: parent_id,
        ..Default::default()
    };
    child.identity = uuid::Uuid::new_v4().to_string();
    let identity = child.identity.clone();
    entries.push(child);
    normalize(&mut entries);
    let id = entries.iter().find(|e| e.code == "A01").unwrap().id;
    super::relocate_entry(&mut entries, id, "B0101", super::SiblingPosition::Last).unwrap();
    let child = entries.iter().find(|e| e.identity == identity).unwrap();
    assert_ne!(child.code, "CUSTOM-X");
    assert_eq!(identity::parent(&entries, child).unwrap().id, id);
    identity::validate(&entries).unwrap();

    let mut empty_codes = vec![
        E {
            canonical: "既有单位".into(),
            code: "00".into(),
            ..Default::default()
        },
        E {
            canonical: "待补编码".into(),
            ..Default::default()
        },
    ];
    normalize(&mut empty_codes);
    identity::validate(&empty_codes).unwrap();
    assert_eq!(empty_codes[0].code, "00");
    assert_eq!(empty_codes[1].code, "01");
}

#[test]
fn gwx_round_trip_without_global_vocabulary_keeps_identities_and_paper_names() {
    use crate::manuscript::{ManuscriptStore, NewManuscript};
    use crate::manuscript_io::{self, sync};
    let entries = fixture();
    let mut draft = DraftInput::default();
    apply_field(
        &mut draft,
        FieldId::IssuingUnit,
        &key(&entries, "A0101"),
        &entries,
    );
    apply_field(
        &mut draft,
        FieldId::CopiesTo,
        &key(&entries, "B0101"),
        &entries,
    );
    let hash_before = crate::manuscript::sync::payload_hash(&draft, "正文", "").unwrap();
    let mut a = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let mut b = ManuscriptStore::open(std::path::Path::new(":memory:")).unwrap();
    let id = a
        .create(
            &NewManuscript {
                snapshot: draft.clone(),
                content_markdown: "正文".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.gwx");
    let password = "Identity!RoundTrip8";
    manuscript_io::export_zip_selected(&mut a, &[id], &[], &path, password, false).unwrap();
    assert!(
        manuscript_io::read_vocabulary(&path, password)
            .unwrap()
            .is_none()
    );
    let manifest = manuscript_io::read_manifest(&path, password).unwrap();
    assert_eq!(manifest.schema, 3);
    let previews = sync::inspect(&mut b, &manifest).unwrap();
    let hash = crate::manuscript::sync::bytes_hash(&serde_json::to_vec(&manifest).unwrap());
    sync::import_with_actions(
        &mut b,
        &path,
        password,
        &hash,
        &[sync::ImportAction::New],
        &previews,
    )
    .unwrap();
    let row = b.list(&Default::default()).unwrap().remove(0);
    let imported = b.get(row.id).unwrap().unwrap().snapshot;
    assert_eq!(
        crate::manuscript::sync::payload_hash(&imported, "正文", "").unwrap(),
        hash_before
    );
    assert_eq!(imported, draft);
    let display = UnitDisplay::new(&[]).for_input(&imported);
    display.ensure_resolved(&imported).unwrap();
    assert_eq!(
        display.full_name(&imported.profile.issuing_unit),
        "甲市教育局办公室"
    );
    assert_eq!(
        crate::export::element_display::copies_to_display(&imported, &display),
        "乙市教育局办公室"
    );
}

#[test]
fn explicit_reselection_refreshes_frozen_person_and_announces_dictionary_changes() {
    let mut entries = fixture();
    let mut draft = DraftInput::default();
    let unit = key(&entries, "A0101");
    let person = person(&entries, "010-1111");
    apply_field(&mut draft, FieldId::ResponsibleUnit, &unit, &entries);
    apply_field(&mut draft, FieldId::ContactPerson, &person, &entries);
    assert!(identity::snapshot_notice(&draft, &entries).is_none());
    let entry = entries.iter_mut().find(|e| e.key() == person).unwrap();
    entry.canonical = "张宁新名".into();
    entry.phone = "010-9999".into();
    identity::capture_input(&mut draft, &entries);
    assert!(identity::snapshot_notice(&draft, &entries).is_some());
    assert_eq!(draft.profile.contact_phone, "010-1111");
    apply_field(&mut draft, FieldId::ContactPerson, &person, &entries);
    assert_eq!(draft.profile.contact_phone, "010-9999");
    assert_eq!(
        UnitDisplay::new(&[]).for_input(&draft).person_name(&person),
        "张宁新名"
    );
    assert!(identity::snapshot_notice(&draft, &entries).is_none());
}

#[test]
fn literal_brackets_and_uuid_letter_case_cannot_confuse_identity_checks() {
    let mut entries = fixture();
    let e = entries.iter_mut().find(|e| e.code == "A").unwrap();
    e.canonical = "甲市〔测试〕".into();
    let selected = e.key();
    let mut draft = DraftInput::default();
    apply_field(&mut draft, FieldId::IssuingUnit, &selected, &entries);
    UnitDisplay::new(&entries)
        .for_input(&draft)
        .ensure_resolved(&draft)
        .unwrap();
    let mut duplicate = entries[0].clone();
    duplicate.identity = duplicate.identity.to_uppercase();
    duplicate.code = "OTHER".into();
    duplicate.canonical = "另一单位".into();
    entries.push(duplicate);
    assert!(identity::validate(&entries).is_err());
    let fake = format!("@unit:{}", "wrong-id");
    assert!(identity::resolve(&entries, C::Unit, &fake).is_none());
}

#[test]
fn reused_business_codes_in_saved_snapshots_do_not_merge_units_or_their_people() {
    let entries = fixture();
    let mut old = DraftInput::default();
    let a = key(&entries, "A0101");
    let b = key(&entries, "B0101");
    let p_a = person(&entries, "010-1111");
    let p_b = person(&entries, "020-3333");
    apply_field(&mut old, FieldId::Recipient, &format!("{a}、{b}"), &entries);
    apply_field(&mut old, FieldId::ResponsibleUnit, &a, &entries);
    apply_field(&mut old, FieldId::ContactPerson, &p_a, &entries);
    // 用联合承办第二行把另一个人员也存进同一个文档快照。
    old.profile
        .joint_contacts
        .push(crate::models::JointContact {
            unit: b.clone(),
            name: p_b.clone(),
            phone: "020-3333".into(),
        });
    identity::capture_input(&mut old, &entries);
    for e in &mut old.profile.vocabulary_snapshot {
        if e.key() == b {
            e.code = "A0101".into();
        }
        if e.key() == p_b {
            e.unit = "A0101".into();
        }
    }
    let display = UnitDisplay::new(&[]).for_input(&old);
    assert_eq!(
        display
            .people_of(&a)
            .iter()
            .map(|p| p.0.clone())
            .collect::<Vec<_>>(),
        [p_a]
    );
    assert_eq!(
        display
            .people_of(&b)
            .iter()
            .map(|p| p.0.clone())
            .collect::<Vec<_>>(),
        [p_b]
    );
    assert!(!display.same_unit(&a, &b));
    assert_eq!(
        crate::export::element_display::addressee_display(&old, &display),
        "甲市教育局办公室、乙市教育局办公室"
    );
    display.ensure_resolved(&old).unwrap();
}

#[test]
fn ambiguous_legacy_phone_record_unit_requires_reselection() {
    let entries = fixture();
    let mut input = DraftInput {
        kind: TemplateKind::PhoneRecord,
        ..Default::default()
    };
    input.phone_record.caller_unit = "办公室".into();
    identity::capture_input(&mut input, &entries);
    assert_eq!(input.phone_record.caller_unit, "@unresolved:办公室");
    assert!(
        UnitDisplay::new(&entries)
            .for_input(&input)
            .ensure_resolved(&input)
            .is_err()
    );
    assert!(UnitDisplay::new(&entries).people_of("办公室").is_empty());
}
