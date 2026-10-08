//! 审核发现的跨文类误报回归；测试实际检查结果及原文字节位置。

use super::*;
use crate::models::{LetterVersion, ProofreadConfig, SecurityRules, TemplateProfile};

fn input(kind: TemplateKind) -> DraftInput {
    DraftInput {
        kind,
        profile: TemplateProfile::for_kind(kind),
        ..Default::default()
    }
}

fn has(input: &DraftInput, text: &str, id: &str) -> bool {
    check(input, text).iter().any(|note| note.entry_id == id)
}

#[test]
fn approved_closings_work_in_both_approval_templates_but_quotes_do_not_supply_them() {
    for kind in [TemplateKind::WhitePaper, TemplateKind::RedHeadApproval] {
        let input = input(kind);
        for closing in [
            "妥否，请批示。",
            "当否，请指示。",
            "特此请示。",
            "请予批准。",
        ] {
            let text = format!("# 关于某事的请示\n\n{closing}");
            let warnings =
                crate::validator::validate(&input, &text, &[], &SecurityRules::default());
            assert!(
                !warnings.iter().any(|note| note.contains("缺少请求性结语")),
                "{warnings:?}"
            );
        }
        for text in [
            "# 关于某事的请示\n\n> 妥否，请指示。",
            "# 关于某事的请示\n\n原函结语为“妥否，请指示。”。",
            "# 关于某事的请示\n\n<!-- [附件] -->\n\n# 附件报告\n\n妥否，请指示。",
        ] {
            let warnings = crate::validator::validate(&input, text, &[], &SecurityRules::default());
            assert!(
                warnings.iter().any(|note| note.contains("缺少请求性结语")),
                "{warnings:?}"
            );
        }
    }
}

#[test]
fn request_closing_is_not_a_request_when_recorded_or_quoted() {
    let text = "# 关于办理情况的报告\n\n> 妥否，请指示。\n\n原函结语为‘请予批复’。\n\n来函以妥否，请批示收束。";
    for kind in TemplateKind::ALL {
        assert!(!has(&input(kind), text, "RULE-KIND-REQUEST"), "{kind:?}");
    }
    let actual = "# 关于办理情况的报告\n\n妥否，请批示。";
    assert!(has(
        &input(TemplateKind::PlainDocument),
        actual,
        "RULE-KIND-REQUEST"
    ));
    assert!(!has(
        &input(TemplateKind::PhoneRecord),
        actual,
        "RULE-KIND-REQUEST"
    ));
    assert!(!has(
        &input(TemplateKind::ResearchReport),
        actual,
        "RULE-KIND-REQUEST"
    ));
}

#[test]
fn resumed_body_is_checked_and_positions_still_point_to_the_source() {
    let text = "# 关于某事的报告\n\n<!-- [附件] -->\n\n# 附件请示\n\n妥否，请指示。\n\n<!-- [正文] -->\n\n妥否，请批示。";
    let notes = check(&input(TemplateKind::PlainDocument), text);
    let hits: Vec<_> = notes
        .iter()
        .filter(|note| note.entry_id == "RULE-KIND-REQUEST")
        .collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(&text[hits[0].span.clone()], "妥否，请批示");
}

#[test]
fn bare_attachment_description_does_not_declare_zero_items() {
    for description in ["附件：情况表", "附件：3份", "附件：", "附件：3 份"] {
        let text = format!("# 关于情况的函\n\n{description}\n\n<!-- [附件] -->\n\n# 情况表");
        assert!(
            !has(
                &input(TemplateKind::OfficialLetter),
                &text,
                "RULE-ATTACH-COUNT"
            ),
            "{description}"
        );
    }
    assert_eq!(declared_list_count("附件：1. 表一\n3. 表三"), None);
    // 正文说明的是项目数，仍然核对，不把未知的另行随附认作必错。
    let text = "# 关于情况的函\n\n附件：共3项\n\n<!-- [附件] -->\n\n# 情况表";
    let notes = check(&input(TemplateKind::OfficialLetter), text);
    assert!(
        notes
            .iter()
            .any(|note| note.entry_id == "RULE-ATTACH-COUNT" && note.level == Level::Suspect)
    );
}

#[test]
fn attachment_quotes_are_not_inconsistent_with_this_drafts_attachments() {
    let text =
        "# 关于情况的函\n\n原函称“详见附件8”。\n\n> 附件共5项。\n\n<!-- [附件] -->\n\n# 情况表";
    for id in ["RULE-ATTACH-COUNT", "RULE-ATTACH-REF"] {
        assert!(!has(&input(TemplateKind::OfficialLetter), text, id));
    }
}

#[test]
fn gui_place_names_and_reported_requirements_do_not_trigger_honorific_or_tone_rules() {
    for name in ["贵阳市财政局", "贵港市教育局", "贵溪市公安局"] {
        let text = format!("# 关于协助事项的函\n\n请联系{name}办理。");
        assert!(!has(
            &input(TemplateKind::OfficialLetter),
            &text,
            "RULE-HONOR-ATTACHED"
        ));
    }
    for (kind, title, text, id) in [
        (
            TemplateKind::OfficialLetter,
            "函",
            "上级要求你局按期完成。",
            "RULE-TONE-PARALLEL",
        ),
        (
            TemplateKind::WhitePaper,
            "报告",
            "督导组要求市政府落实整改。",
            "RULE-TONE-UPWARD",
        ),
    ] {
        assert!(!has(
            &input(kind),
            &format!("# 关于某事的{title}\n\n{text}"),
            id
        ));
    }
    assert!(has(
        &input(TemplateKind::PlainDocument),
        "# 关于某事的请示\n\n要求上级立即批复。",
        "RULE-TONE-UPWARD"
    ));
}

#[test]
fn style_preferences_are_opt_in_and_do_not_spread_to_other_templates() {
    let text = "# 测试报告\n\n我局高度重视——必须、必须、必须、必须、必须、必须进一步进一步进一步进一步。甲——乙。";
    let input = input(TemplateKind::ResearchReport);
    assert!(!check(&input, text).iter().any(|note| {
        RULES
            .iter()
            .any(|rule| rule.optional && rule.id == note.entry_id)
    }));
    let mut config = ProofreadConfig::default();
    set_style(&mut config, input.kind, "RULE-FORCE-QUOTA", true);
    let notes = check_with_config(&input, text, &config);
    assert!(
        notes
            .iter()
            .any(|note| note.entry_id == "RULE-FORCE-QUOTA" && note.level == Level::Hint)
    );
    assert!(
        !check_with_config(&self::input(TemplateKind::PhoneRecord), text, &config)
            .iter()
            .any(|note| note.entry_id == "RULE-FORCE-QUOTA")
    );
}

#[test]
fn historical_or_hidden_dates_do_not_raise_an_export_warning() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    for kind in TemplateKind::ALL {
        let mut input = input(kind);
        input.date = "2026年10月1日".into();
        input.date_is_auto = false;
        assert!(check_doc_date(&input, today).is_none());
        input.date_is_auto = true;
        input.profile.letter_version = LetterVersion::Preview;
        assert!(check_doc_date(&input, today).is_none());
        input.profile.letter_version = LetterVersion::Formal;
        assert_eq!(
            check_doc_date(&input, today).is_some(),
            matches!(
                kind,
                TemplateKind::OfficialLetter
                    | TemplateKind::WhitePaper
                    | TemplateKind::RedHeadApproval
            )
        );
    }
}

#[test]
fn relative_dates_in_attachments_and_direct_quotes_are_not_rebased() {
    let text =
        "# 关于某事的函\n\n对方称“明天报送”。\n\n<!-- [附件] -->\n\n# 来文记录\n\n下周一召开会议。";
    let warnings = crate::validator::validate(
        &input(TemplateKind::OfficialLetter),
        text,
        &[],
        &SecurityRules::default(),
    );
    assert!(!warnings.iter().any(|note| note.contains("相对日期")));
}

#[test]
fn ambiguous_names_terms_and_style_preferences_are_not_auto_replacements() {
    let lexicon = crate::proofread::Lexicon::resolved(&ProofreadConfig::default());
    let name = lexicon.check("曾强同志负责协调工作。");
    assert!(!name.iter().any(|note| note.entry_id == "TYP-010"));
    let text = "电机起动后按其它要求运行；惟一的问题是物品免费赠送。";
    for note in lexicon.check(text) {
        if matches!(
            note.entry_id.as_str(),
            "TYP-056" | "TYP-066" | "TYP-070" | "GRM-011"
        ) {
            assert_ne!(note.level, Level::MustFix);
            assert!(note.replacement.is_none());
        }
    }
    assert!(
        lexicon
            .check("工作布署完成。")
            .iter()
            .any(|note| note.entry_id == "TYP-001" && note.replacement.as_deref() == Some("部署"))
    );
}
