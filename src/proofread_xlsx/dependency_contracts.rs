//! 锁住 Excel 解析库升级时容易改变的业务边界。

use super::parse;
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;

/// 手工造包，避免写入库自动添加 xml:space 后掩盖导入差异。
fn fixture(path: &Path, shared: bool, preserve: bool, strict: bool, extra_attributes: usize) {
    let main = if strict {
        "http://purl.oclc.org/ooxml/spreadsheetml/main"
    } else {
        "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
    };
    let rel = if strict {
        "http://purl.oclc.org/ooxml/officeDocument/relationships"
    } else {
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    };
    let space = if preserve {
        " xml:space=\"preserve\""
    } else {
        ""
    };
    let cells = [
        " \tUSR-000017\t ",
        "\t　我办　\t",
        " 本办 ",
        " 疑似 ",
        " 总是 ",
        " 自定义 ",
        "  保留 中间　空格  ",
        " 是 ",
    ];
    let row: String = cells
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let column = (b'A' + index as u8) as char;
            if shared {
                format!("<c r=\"{column}2\" t=\"s\"><v>{index}</v></c>")
            } else {
                format!("<c r=\"{column}2\" t=\"inlineStr\"><is><t{space}>{text}</t></is></c>")
            }
        })
        .collect();
    let attrs: String = (0..extra_attributes)
        .map(|index| format!(" attr{index}=\"x\""))
        .collect();
    let worksheet = format!(
        "<worksheet xmlns=\"{main}\"{attrs}><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>条目编号</t></is></c></row><row r=\"2\">{row}</row><row r=\"3\"><c r=\"A3\" t=\"inlineStr\"><is><t>　 </t></is></c></row></sheetData></worksheet>"
    );
    let workbook = format!(
        "<workbook xmlns=\"{main}\" xmlns:r=\"{rel}\"><sheets><sheet name=\"校对词表\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>"
    );
    let mut relationships = format!(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{rel}/worksheet\" Target=\"worksheets/sheet1.xml\"/>"
    );
    if shared {
        relationships.push_str(&format!(
            "<Relationship Id=\"rId2\" Type=\"{rel}/sharedStrings\" Target=\"sharedStrings.xml\"/>"
        ));
    }
    relationships.push_str("</Relationships>");
    let mut files = vec![
        ("[Content_Types].xml", "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/></Types>".to_string()),
        ("_rels/.rels", format!("<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{rel}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>")),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", relationships),
        ("xl/worksheets/sheet1.xml", worksheet),
    ];
    if shared {
        let strings: String = cells
            .iter()
            .map(|text| format!("<si><t{space}>{text}</t></si>"))
            .collect();
        files.push((
            "xl/sharedStrings.xml",
            format!("<sst xmlns=\"{main}\">{strings}</sst>"),
        ));
    }
    let file = std::fs::File::create(path).expect("创建测试工作簿");
    let mut archive = zip::ZipWriter::new(file);
    for (name, content) in files {
        archive
            .start_file(name, SimpleFileOptions::default())
            .expect("写入条目");
        archive.write_all(content.as_bytes()).expect("写入 XML");
    }
    archive.finish().expect("结束工作簿");
}

#[test]
fn whitespace_and_strict_ooxml_preserve_imported_rule_semantics() {
    let dir = tempfile::tempdir().expect("临时目录");
    for shared in [false, true] {
        for preserve in [false, true] {
            for strict in [false, true] {
                let path = dir.path().join("词表.xlsx");
                fixture(&path, shared, preserve, strict, 0);
                let (rules, warnings) = parse(&path).unwrap_or_else(|error| {
                    panic!("shared={shared}, preserve={preserve}, strict={strict}: {error:#}")
                });
                assert!(warnings.is_empty(), "{warnings:?}");
                assert_eq!(rules.len(), 1, "空白行不得产生规则");
                let rule = &rules[0];
                assert_eq!(rule.id, "USR-000017");
                assert_eq!(rule.wrong, "我办");
                assert_eq!(rule.suggestion, "本办");
                assert_eq!(rule.note, "保留 中间　空格");
                assert!(rule.enabled);
            }
        }
    }
}

#[test]
fn bounded_attribute_heavy_workbook_keeps_rule_contents() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("大量属性.xlsx");
    // 固定 5000 个属性，只验证业务结果，不给 CI 加脆弱的计时断言。
    fixture(&path, false, true, false, 5000);
    let (rules, warnings) = parse(&path).expect("读取有上限的属性样本");
    assert!(warnings.is_empty());
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, "USR-000017");
}

#[test]
fn truncated_workbook_is_rejected_without_rules() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("损坏.xlsx");
    std::fs::write(&path, b"PK\x03\x04truncated").expect("写入损坏包");
    assert!(parse(&path).is_err());
}
