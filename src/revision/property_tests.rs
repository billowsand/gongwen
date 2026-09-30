//! 用随机中文、组合字符和表情验证偏移、重定位及撤销，失败案例由 proptest 缩减。

use super::*;
use proptest::prelude::*;

fn surrounding_text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            '甲', '乙', '中', 'a', '1', ' ', '\n', 'é', '\u{301}', '👩', '\u{200d}', '💻',
        ]),
        0..80,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn revision_set(text: &str, span: Range<usize>) -> RevisionSet {
    let mut set = RevisionSet::default();
    set.begin_rule_pass(text);
    set.push_notes(
        vec![ProofNote {
            entry_id: "属性测试".into(),
            group: "错别字".into(),
            level: Level::MustFix,
            message: "改为部署".into(),
            span,
            replacement: Some("部署".into()),
        }],
        false,
        text,
        &[],
    );
    set.end_rule_pass();
    set
}

proptest! {
    #[test]
    fn inserting_a_prefix_relocates_to_valid_utf8_boundaries(
        prefix in surrounding_text(), suffix in surrounding_text(), inserted in surrounding_text(),
    ) {
        let original = format!("{prefix}布署{suffix}");
        let span = prefix.len()..prefix.len() + "布署".len();
        let anchor = Anchor::new(&original, &span);
        let edited = format!("{inserted}{original}");
        let found = anchor.relocate(&edited, &span).unwrap();
        prop_assert_eq!(&found, &(inserted.len() + span.start..inserted.len() + span.end));
        prop_assert!(edited.is_char_boundary(found.start));
        prop_assert!(edited.is_char_boundary(found.end));
        prop_assert_eq!(&edited[found], "布署");
    }

    #[test]
    fn applying_and_undoing_preserves_every_surrounding_character(
        prefix in surrounding_text(), suffix in surrounding_text(),
    ) {
        let original = format!("{prefix}布署{suffix}");
        let mut text = original.clone();
        let mut set = revision_set(&text, prefix.len()..prefix.len() + "布署".len());
        let id = set.items()[0].id;
        prop_assert!(set.apply(id, &mut text, |_, _| Ok(())).is_ok());
        prop_assert_eq!(&text, &format!("{prefix}部署{suffix}"));
        prop_assert!(set.undo_last(&mut text).is_ok());
        prop_assert_eq!(text, original);
    }

    #[test]
    fn failing_the_gate_never_changes_text_or_consumes_the_suggestion(
        prefix in surrounding_text(), suffix in surrounding_text(),
    ) {
        let original = format!("{prefix}布署{suffix}");
        let mut text = original.clone();
        let mut set = revision_set(&text, prefix.len()..prefix.len() + "布署".len());
        let id = set.items()[0].id;
        prop_assert!(set.apply(id, &mut text, |_, _| Err("复扫未通过".into())).is_err());
        prop_assert_eq!(text, original);
        prop_assert_eq!(set.items()[0].id, id);
        prop_assert_eq!(set.applied_count(), 0);
    }

    #[test]
    fn removing_the_original_prevents_applying_a_stale_suggestion(
        prefix in surrounding_text(), suffix in surrounding_text(),
    ) {
        let original = format!("{prefix}布署{suffix}");
        let mut set = revision_set(&original, prefix.len()..prefix.len() + "布署".len());
        let id = set.items()[0].id;
        let mut text = format!("{prefix}用户已改{suffix}");
        let before = text.clone();
        prop_assert!(set.apply(id, &mut text, |_, _| Ok(())).is_err());
        prop_assert_eq!(text, before);
    }
}
