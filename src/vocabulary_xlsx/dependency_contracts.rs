//! 表格写入库升级时核对数据和用户可见的工作表结构。

use super::*;
use std::io::Read;

fn xml_entry(archive: &mut zip::ZipArchive<File>, name: &str) -> String {
    let mut xml = String::new();
    archive
        .by_name(name)
        .expect("工作簿部件")
        .read_to_string(&mut xml)
        .expect("XML 文本");
    xml
}

fn attributes(xml: &str, element: &str) -> Vec<BTreeMap<String, String>> {
    let tags = regex::Regex::new(&format!(r"<{element}\b([^>]*)>")).expect("标签表达式");
    let attrs = regex::Regex::new(r#"(\w+)="([^"]*)""#).expect("属性表达式");
    tags.captures_iter(xml)
        .map(|tag| {
            attrs
                .captures_iter(&tag[1])
                .map(|attr| (attr[1].to_owned(), attr[2].to_owned()))
                .collect()
        })
        .collect()
}

#[test]
fn exported_vocabulary_keeps_codes_layout_and_validation() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("标准词库.xlsx");
    let entries = vec![
        VocabularyEntry {
            category: VocabularyCategory::Unit,
            code: "001".into(),
            canonical: "市公文办".into(),
            department_code: "公办".into(),
            aliases: vec!["公文办".into()],
            ..Default::default()
        },
        VocabularyEntry {
            category: VocabularyCategory::Person,
            unit: "001".into(),
            code: "USR-000017".into(),
            canonical: "张三".into(),
            position: "处长".into(),
            phone: "010-0017".into(),
            can_handle_parent_unit: true,
            note: "中文备注\n保留内部 空格".into(),
            ..Default::default()
        },
    ];
    let codes = vec!["001".to_owned()];
    to_xlsx(&entries, &path, &codes).expect("导出标准词库");
    let report = parse(&path, &codes).expect("回读标准词库");
    assert!(report.conflicts.is_empty());
    assert_eq!(report.entries.len(), 2);
    let unit = report
        .entries
        .iter()
        .find(|entry| entry.category == VocabularyCategory::Unit)
        .expect("单位");
    assert_eq!(unit.code, "001");
    assert_eq!(unit.canonical, "市公文办");
    assert_eq!(unit.department_code, "公办");
    assert_eq!(unit.aliases, ["公文办"]);
    let person = report
        .entries
        .iter()
        .find(|entry| entry.category == VocabularyCategory::Person)
        .expect("人员");
    assert_eq!(person.unit, "001");
    assert_eq!(person.code, "USR-000017");
    assert_eq!(person.phone, "010-0017");
    assert_eq!(person.note, "中文备注\n保留内部 空格");
    assert!(person.can_handle_parent_unit);

    let mut workbook: Xlsx<_> = calamine::open_workbook(&path).expect("打开工作簿");
    assert_eq!(workbook.sheet_names(), [UNIT_SHEET, PERSON_SHEET]);
    for (name, headers) in [(UNIT_SHEET, UNIT_HEADERS), (PERSON_SHEET, PERSON_HEADERS)] {
        let range = workbook.worksheet_range(name).expect("读取工作表");
        let actual: Vec<_> = range
            .rows()
            .next()
            .expect("表头")
            .iter()
            .take(headers.len())
            .map(ToString::to_string)
            .collect();
        assert_eq!(actual, headers);
    }

    let mut archive = zip::ZipArchive::new(File::open(&path).expect("读取包")).expect("XLSX 包");
    for (name, expected_ranges) in [
        ("xl/worksheets/sheet1.xml", vec!["F2:F1001"]),
        ("xl/worksheets/sheet2.xml", vec!["A2:A5001", "F2:F5001"]),
    ] {
        let xml = xml_entry(&mut archive, name);
        let panes = attributes(&xml, "pane");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].get("state").map(String::as_str), Some("frozen"));
        assert_eq!(panes[0].get("ySplit").map(String::as_str), Some("1"));
        assert_eq!(panes[0].get("topLeftCell").map(String::as_str), Some("A2"));
        let protection = attributes(&xml, "sheetProtection");
        assert_eq!(protection.len(), 1);
        assert_eq!(protection[0].get("sheet").map(String::as_str), Some("1"));
        let ranges: Vec<_> = attributes(&xml, "dataValidation")
            .iter()
            .map(|attrs| attrs["sqref"].clone())
            .collect();
        assert_eq!(ranges, expected_ranges);
        assert!(xml.contains("<formula1>\"-,否,是\"</formula1>"));
    }
    let unit_xml = xml_entry(&mut archive, "xl/worksheets/sheet1.xml");
    assert!(
        attributes(&unit_xml, "col")
            .iter()
            .any(|col| col.get("min").map(String::as_str) == Some("11")
                && col.get("hidden").map(String::as_str) == Some("1"))
    );

    // 可把同一份合成样本留给 Excel/WPS 抽样打开及升级前后的 XML 结构比较。
    if let Ok(output) = std::env::var("GONGWEN_XLSX_SAMPLE_DIR") {
        let output = Path::new(&output);
        std::fs::create_dir_all(output).expect("样本目录");
        std::fs::copy(&path, output.join("标准词库.xlsx")).expect("保存词库样本");
        crate::proofread_xlsx::to_xlsx(
            &crate::models::ProofreadConfig::default(),
            &output.join("校对词表.xlsx"),
        )
        .expect("保存校对词表示例");
    }
}

#[test]
#[ignore = "表格依赖升级时单独运行大词表性能对照"]
fn five_thousand_rule_workbook_probe() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("大词表.xlsx");
    let config = crate::models::ProofreadConfig {
        custom: (0..5000)
            .map(|index| crate::models::ProofreadRule {
                id: format!("USR-BENCH-{index:06}"),
                wrong: format!("测试词{index:06}"),
                suggestion: format!("修订词{index:06}"),
                level: "疑似".into(),
                condition: "总是".into(),
                group: "自定义".into(),
                note: "保留内部 空格与中文备注".into(),
                enabled: true,
            })
            .collect(),
        ..Default::default()
    };
    let mut export_us = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        crate::proofread_xlsx::to_xlsx(&config, &path).expect("导出大词表");
        export_us.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }
    export_us.sort_by(f64::total_cmp);
    let (rules, warnings) = crate::proofread_xlsx::parse(&path).expect("回读大词表");
    assert!(warnings.is_empty());
    let custom: Vec<_> = rules
        .into_iter()
        .filter(|rule| rule.id.starts_with("USR-BENCH-"))
        .collect();
    assert_eq!(custom, config.custom);
    let report = serde_json::json!({
        "custom_rules": custom.len(),
        "samples": export_us.len(),
        "median_us": export_us[2],
        "min_us": export_us[0],
        "max_us": export_us[4],
        "file_bytes": std::fs::metadata(&path).expect("文件信息").len(),
        "profile": "dev，依赖 opt-level=3，两边相同",
    });
    println!("XLSX_BENCH={report}");
    if let Ok(output) = std::env::var("GONGWEN_XLSX_BENCH_OUTPUT") {
        std::fs::write(
            output,
            serde_json::to_vec_pretty(&report).expect("性能记录"),
        )
        .expect("保存性能记录");
    }
}
