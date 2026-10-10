//! 美国国防部词库的主送、抄送前缀省略回归测试。

use super::*;

/// 从用户提供的美国国防部 XLSX 单位表提取，保留 88 个单位的编码、全称与简称。
fn us_defense_vocab() -> Vec<VocabularyEntry> {
    let mut entries: Vec<VocabularyEntry> =
        serde_json::from_str(include_str!("test_data/us-defense-units.json")).unwrap();
    rebuild_parents_from_codes(&mut entries);
    entries
}

#[test]
fn us_defense_addressees_and_copies_use_scoped_prefixes() {
    check_us_defense_prefixes(&us_defense_vocab(), None);
}

fn check_us_defense_prefixes(vocabulary: &[VocabularyEntry], output: Option<&std::path::Path>) {
    let display = UnitDisplay::new(vocabulary);
    let cases: &[(&str, &[&str], &str)] = &[
        (
            "三军部",
            &["90-03", "90-04", "90-05"],
            "美国陆军部、海军部、空军部",
        ),
        (
            "海军与陆战队",
            &["90-04-02", "90-04-03"],
            "美国海军、海军陆战队",
        ),
        (
            "空军与太空军",
            &["90-05-02", "90-05-03"],
            "美国空军、太空军",
        ),
        (
            "政策次长下属",
            &["90-01-01-01", "90-01-01-02", "90-01-01-03"],
            "美国国防部印度太平洋安全事务助理部长办公室、国际安全事务助理部长办公室、战略规划与能力助理部长办公室",
        ),
        (
            "后勤局下属",
            &[
                "90-01-02-02-01-01",
                "90-01-02-02-01-02",
                "90-01-02-02-01-03",
                "90-01-02-02-01-04",
            ],
            "美国国防后勤局物资处置服务局、配送局、能源局、部队支援局",
        ),
        (
            "联合秘书处",
            &["90-02-01-01-01-01", "90-02-01-01-01-02"],
            "美国联合秘书处信息管理处、事务办理处",
        ),
        (
            "参谋部三局",
            &["90-02-01-02", "90-02-01-03", "90-02-01-04"],
            "美国联合参谋部人事与人力局、情报局、作战局",
        ),
        (
            "两组机关",
            &[
                "90-01-02-02-01-02",
                "90-01-02-02-01-03",
                "90-01-06-01-01",
                "90-01-06-01-02",
            ],
            "美国国防后勤局配送局、能源局，美国国防情报局国家医学情报中心、导弹与航天情报中心",
        ),
        (
            "跨机关保留机关名",
            &["90-01-02-02-01-02", "90-01-06-01-01", "90-01-02-02-01-03"],
            "美国国防后勤局配送局、国防情报局国家医学情报中心、国防后勤局能源局",
        ),
        (
            "祖先也是收文对象",
            &["90-01-02-02-01", "90-01-02-02-01-02", "90-01-02-02-01-03"],
            "美国国防后勤局，美国国防后勤局配送局、能源局",
        ),
        (
            "军种和作战司令部",
            &["90-03", "90-11", "90-12"],
            "美国陆军部、北方司令部、南方司令部",
        ),
        (
            "十一作战司令部",
            &[
                "90-11", "90-12", "90-13", "90-14", "90-15", "90-16", "90-17", "90-18", "90-19",
                "90-20", "90-21",
            ],
            "美国北方司令部、南方司令部、中央司令部、欧洲司令部、印度太平洋司令部、非洲司令部、战略司令部、特种作战司令部、运输司令部、网络司令部、太空司令部",
        ),
        ("单个单位仍用全称", &["90-05-03"], "美国太空军"),
        (
            "部长不能截成部加长",
            &["90-31", "90-01"],
            "美国国防部监察长办公室、国防部长办公室",
        ),
        (
            "词库名称与编码混选去重",
            &["90-03", "美国陆军部", "90-04"],
            "美国陆军部、海军部",
        ),
    ];
    let mut report = "# 美国国防部词库：主送与抄送前缀测试\n\n使用项目实际 XLSX 导入器，保留原单位名称及层级编码。以下各例均核对主送、抄送以及对内、对外两种显示入口。\n".to_string();
    for (name, codes, expected) in cases {
        for external in [false, true] {
            // 两个正式显示入口均验证，不能只测内部 join 函数。
            let mut draft = crate::models::DraftInput::default();
            if external {
                draft.profile.correspondence_scope = crate::models::CorrespondenceScope::External;
            }
            let selected = codes.join("、");
            draft.profile.recipient = selected.clone();
            draft.profile.copies_to = selected;
            assert_eq!(
                crate::export::element_display::addressee_display(&draft, &display),
                *expected,
                "主送：{name}"
            );
            assert_eq!(
                crate::export::element_display::copies_to_display(&draft, &display),
                *expected,
                "抄送：{name}"
            );
        }
        let full = codes
            .iter()
            .map(|code| display.full_name(code))
            .collect::<Vec<_>>()
            .join("、");
        report.push_str(&format!(
            "\n## {name}\n\n选择：{full}\n\n显示：{expected}\n\n结果：通过。\n"
        ));
    }
    if let Some(path) = output {
        std::fs::write(path, report).unwrap();
    }
}

#[test]
#[ignore = "需用户提供的本机美国国防部 XLSX，验证真实导入结果"]
fn us_defense_actual_xlsx_matches_prefix_fixture() {
    let imported = crate::vocabulary_xlsx::parse(
        std::path::Path::new("output/us-defense-vocabulary-test/美国国防部标准词库-中文版.xlsx"),
        &[],
    )
    .unwrap();
    let fixture = us_defense_vocab();
    let units: Vec<_> = imported
        .entries
        .iter()
        .filter(|entry| entry.category == VocabularyCategory::Unit)
        .collect();
    assert_eq!(units.len(), 88);
    for expected in fixture {
        let actual = units
            .iter()
            .find(|entry| entry.code == expected.code)
            .unwrap();
        assert_eq!(
            (
                &actual.canonical,
                &actual.external_name,
                &actual.abbr,
                &actual.parent
            ),
            (
                &expected.canonical,
                &expected.external_name,
                &expected.abbr,
                &expected.parent
            )
        );
    }
    check_us_defense_prefixes(
        &imported.entries,
        Some(std::path::Path::new(
            "output/us-defense-vocabulary-test/prefix-checks.md",
        )),
    );
}
