//! 研究终端的 Word 白图适配：保留 mdx 的语义、分页、目录和公式，仅改视觉样式。
//! Word 的整页底色打印依赖阅读器设置，统一使用白图，暗场阅读由 PDF 承担。

use anyhow::{Context, Result};
use std::{
    io::{Cursor, Read, Write},
    path::Path,
};

pub(super) fn style_docx(path: &Path) -> Result<()> {
    let source = std::fs::read(path)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(source))?;
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if name.starts_with("word/") && name.ends_with(".xml") {
            let xml = String::from_utf8(bytes).context("Word XML 不是 UTF-8")?;
            bytes = style_xml(&name, &xml).into_bytes();
        }
        output.start_file(name, options)?;
        output.write_all(&bytes)?;
    }
    std::fs::write(path, output.finish()?.into_inner())?;
    Ok(())
}

fn style_xml(name: &str, xml: &str) -> String {
    // 只改字体声明，不替换正文中可能提及的字体名称或研究内容。
    let fonts = regex::Regex::new(r"<w:rFonts\b[^>]*/>").unwrap();
    let mut xml = fonts
        .replace_all(xml, |caps: &regex::Captures<'_>| {
            let formatted = caps[0]
                .replace("FZShuSong-Z01", "FZHei-B01")
                .replace("FZKai-Z03", "FZHei-B01")
                .replace("FZXiaoBiaoSong-B05", "FZHei-B01");
            formatted
                .replace("w:ascii=\"FZHei-B01\"", "w:ascii=\"JetBrains Mono\"")
                .replace("w:hAnsi=\"FZHei-B01\"", "w:hAnsi=\"JetBrains Mono\"")
                .replace("w:cs=\"FZHei-B01\"", "w:cs=\"JetBrains Mono\"")
        })
        .into_owned();
    let borders =
        regex::Regex::new(r"<w:(?:top|bottom|left|right|insideH|insideV)\b[^>]*/>").unwrap();
    xml = borders
        .replace_all(&xml, |caps: &regex::Captures<'_>| {
            caps[0].replace("000000", "A0ADB3")
        })
        .into_owned();
    if name == "word/styles.xml" {
        let headings =
            regex::Regex::new(r#"(?s)<w:style\b[^>]*w:styleId="Heading[123]".*?</w:style>"#)
                .unwrap();
        xml = headings
            .replace_all(&xml, |caps: &regex::Captures<'_>| {
                caps[0].replace("<w:rPr>", "<w:rPr><w:color w:val=\"B53D30\"/>")
            })
            .into_owned();
    }
    if name == "word/document.xml" {
        let frame = "<w:pgBorders w:offsetFrom=\"page\"><w:top w:val=\"single\" w:sz=\"4\" w:space=\"24\" w:color=\"A0ADB3\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"24\" w:color=\"A0ADB3\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"24\" w:color=\"A0ADB3\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"24\" w:color=\"A0ADB3\"/></w:pgBorders>";
        // pgBorders 位于页尺寸、页边距之后，遵守 WordprocessingML 的节属性顺序。
        let margins = regex::Regex::new(r"<w:pgMar\b[^>]*/>").unwrap();
        xml = margins
            .replace_all(&xml, |caps: &regex::Captures<'_>| {
                format!("{}{frame}", &caps[0])
            })
            .into_owned();
    }
    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_theme_changes_formatting_without_rewriting_report_text() {
        let xml = r#"<w:document><w:rFonts w:eastAsia="FZShuSong-Z01"/><w:t>FZShuSong-Z01 研究结论</w:t><w:pgMar w:top="100"/></w:document>"#;
        let themed = style_xml("word/document.xml", xml);
        assert!(themed.contains("w:eastAsia=\"FZHei-B01\""));
        assert!(themed.contains("<w:t>FZShuSong-Z01 研究结论</w:t>"));
        assert!(themed.contains("<w:pgBorders"));
    }

    #[test]
    fn terminal_word_export_keeps_editable_content_and_report_fields() {
        use crate::models::{DraftInput, NumberingConfig, ResearchTemplate, TemplateKind};
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().into()));
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..Default::default()
        };
        input.research.template = ResearchTemplate::Terminal;
        input.research.institution = "报告测试单位".into();
        let path = dir.path().join("terminal.docx");
        crate::export::research::write_docx(&path, &input,
            "# 模板研究报告\n\n<!-- [目录] -->\n\n<!-- [正文] -->\n\n## 研究背景\n\n结论保留，公式 $x^2$。\n", &NumberingConfig::default()).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut document = String::new();
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut document)
            .unwrap();
        assert!(document.contains("模板研究报告"));
        assert!(document.contains("结论保留"));
        assert!(document.contains("报告测试单位"));
        assert!(document.contains("TOC"));
        assert!(document.contains("<w:pgBorders"));
        assert!(document.contains("JetBrains Mono"));
        let mut styles = String::new();
        archive
            .by_name("word/styles.xml")
            .unwrap()
            .read_to_string(&mut styles)
            .unwrap();
        assert!(styles.contains("B53D30"));
        crate::storage::set_test_config_dir(None);
    }
}
