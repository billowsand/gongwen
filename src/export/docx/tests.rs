//! `export::docx` 的导出测试：段落对齐、字体、页眉、落款、版记、红头呈批件、
//! 议程与横向页。
//!
//! 由 `src/export/docx.rs` 拆分而来，作为 `export::docx` 的子模块编译（仅测试构建）。

use super::*;
use crate::models::{
    JointContact, JointIssuanceMode, LetterVersion, VocabularyCategory, VocabularyEntry,
};
use regex::Regex;
use std::io::Read;

/// 测试大多使用无层级的扁平单位，空词库让 `UnitDisplay` 回落为规范名称。
fn write_docx_ok(path: &Path, input: &DraftInput, markdown: &str) -> Result<()> {
    write_docx(path, input, markdown, &UnitDisplay::new(&[]))
}

/// 动态生成一张 1x1 红色 PNG 的字节。
fn tiny_png() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
    cursor.into_inner()
}

/// 正文流里的每一段都两端对齐（Word 的 `w:jc=both`），与 TeX 的默认对齐
/// 和预览一致；不是靠 Word 的默认左对齐凑合。
#[test]
fn every_body_paragraph_is_justified() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("justified.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "某单位".into();
    input.profile.recipient = "某部门".into();
    write_docx_ok(
        &path,
        &input,
        "# 测试函\n<!-- [正文] -->\n## 工作要求\n正文段落。\n- 无序列表项\n\n1. 有序列表项\n<!-- [附件] -->\n# 附件1\n## 统计表\n附件正文。",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    for needle in [
        "正文段落。",
        "一、工作要求",
        "无序列表项",
        "1.有序列表项",
        "附件：统计表",
        "附件正文。",
    ] {
        let paragraph = paragraph_containing(&xml, needle);
        assert!(
            paragraph.contains(r#"<w:jc w:val="both" />"#),
            "「{needle}」所在段应两端对齐：{paragraph}"
        );
    }
    // 无序列表项顶格，不再额外左缩进（与 TeX 的 \noindent 一致）。
    let list = paragraph_containing(&xml, "无序列表项");
    assert!(!list.contains(r#"w:left="420""#), "{list}");
}

/// 加粗排法三端同源：默认让 Word 合成粗体，选了专用粗体字体就换字面。
/// 序号表导出到 Word：分组行整行合并（`w:gridSpan`）且靠左（`w:jc="left"`），
/// 首列是程序编出来的组内序号。
#[test]
fn numbered_table_exports_merged_left_aligned_group_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("numbered-table.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;
    write_docx_ok(
        &path,
        &input,
        "# 序号表测试\n\
         <!-- [序号表] -->\n\
         | 序号 | 标题 | 内容 | 备注 |\n\
         | --- | --- | --- | --- |\n\
         | 大标题一 |  |  |  |\n\
         |  | 标题甲 | 内容甲 | 备注甲 |\n\
         |  | 标题乙 | 内容乙 | 备注乙 |\n\
         | 大标题二 |  |  |  |\n\
         |  | 标题丙 | 内容丙 | 备注丙 |",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 分组行：整行四列合并，内容按设置生成 `（一）`，靠左。括号里的字面走
    // 楷体那一套，与标题分成两个 run，所以按标题文字定位。
    let group = table_cell_containing(&xml, "大标题一");
    assert!(group.contains(r#"<w:gridSpan w:val="4" />"#), "{group}");
    assert!(
        paragraph_containing(group, "大标题一").contains(r#"<w:jc w:val="left" />"#),
        "{group}"
    );
    let second = table_cell_containing(&xml, "大标题二");
    assert!(second.contains(r#"<w:gridSpan w:val="4" />"#), "{second}");
    // 分组行整格排楷体（序号表分组行的规矩），字号仍是表格的四号。
    assert!(
        group.contains(r#"w:eastAsia="楷体_GB2312""#),
        "分组行应当是楷体：{group}"
    );
    // 数据格与表头不受这条规矩影响。
    let data = table_cell_containing(&xml, "标题甲");
    assert!(
        data.contains(r#"w:eastAsia="仿宋_GB2312""#),
        "数据格应当是仿宋：{data}"
    );
    // 数据行不合并，序号列是解析器填进去的组内位次。
    for needle in ["标题甲", "标题乙"] {
        let cell = table_cell_containing(&xml, needle);
        assert!(!cell.contains("<w:gridSpan"), "数据行不该合并：{cell}");
    }
    assert_eq!(xml.matches(">（一）</w:t>").count(), 1, "{xml}");
    assert_eq!(xml.matches(">（二）</w:t>").count(), 1);
    for number in ["1", "2"] {
        assert!(
            xml.contains(&format!(">{number}</w:t>")),
            "缺少自动编号 {number}"
        );
    }
    assert!(xml.contains(">序号</w:t>"), "表头首格应补「序号」");
}

/// 居中 / 居右区：两条导出路径（普通文档与函稿版式）都按标记排成居中、
/// 右对齐段落，不带首行缩进；空行之后的正文恢复两端对齐。
#[test]
fn aligned_lines_export_as_centered_and_right_paragraphs() {
    let temp = tempfile::tempdir().unwrap();
    for kind in [TemplateKind::PlainDocument, TemplateKind::WhitePaper] {
        let path = temp.path().join(format!("aligned-{kind:?}.docx"));
        let mut input = DraftInput::default();
        input.kind = kind;
        write_docx_ok(
            &path,
            &input,
            "# 对齐测试\n\
             <!-- [居中] -->\n\
             居中一行\n\
             <!-- [居右] -->\n\
             居右一行\n\
             \n\
             恢复正文。",
        )
        .unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let center = paragraph_containing(&xml, "居中一行");
        assert!(center.contains(r#"<w:jc w:val="center" />"#), "{center}");
        assert!(!center.contains("w:firstLine"), "{center}");
        let right = paragraph_containing(&xml, "居右一行");
        assert!(right.contains(r#"<w:jc w:val="right" />"#), "{right}");
        let body = paragraph_containing(&xml, "恢复正文");
        assert!(body.contains(r#"<w:jc w:val="both" />"#), "{body}");
        assert!(!xml.contains("居中]"), "标记行不该印出来");
    }
}

#[test]
fn bold_text_follows_the_configured_bold_style() {
    let temp = tempfile::tempdir().unwrap();
    let markdown = "# 测试函\n\n请**务必**按时报送。";
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;

    // 默认：仍是仿宋 + w:b，由 Word 合成粗体。
    let synthetic = temp.path().join("synthetic.docx");
    write_docx_ok(&synthetic, &input, markdown).unwrap();
    let xml = zip_text(&synthetic, "word/document.xml");
    let run = bold_run(&xml, "务必");
    assert!(run.contains("<w:b "), "{run}");
    assert!(run.contains("w:eastAsia=\"仿宋_GB2312\""), "{run}");

    // 选专用粗体字体：换字面，不再叠加 w:b（真粗体再合成一层会糊）。
    let dedicated = temp.path().join("dedicated.docx");
    let mut fonts = crate::models::FontConfig::default();
    fonts.bold_style = crate::models::BoldStyle::DedicatedFont;
    write_docx_with_numbering(
        &dedicated,
        &input,
        markdown,
        &UnitDisplay::new(&[]),
        &fonts,
        &NumberingConfig::default(),
        &crate::visual_diff::ElementMarks::default(),
    )
    .unwrap();
    let xml = zip_text(&dedicated, "word/document.xml");
    let run = bold_run(&xml, "务必");
    assert!(run.contains("w:eastAsia=\"黑体\""), "{run}");
    assert!(!run.contains("<w:b "), "{run}");
}

#[test]
fn deep_headings_keep_synthetic_fangsong_with_dedicated_body_bold() {
    let temp = tempfile::tempdir().unwrap();
    let markdown = "# 测试函\n\n#### 三级标题\n\n##### 四级标题\n\n请**务必**报送。";
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;
    let mut fonts = crate::models::FontConfig::default();
    fonts.bold_style = crate::models::BoldStyle::DedicatedFont;
    let path = temp.path().join("headings.docx");
    write_docx_with_numbering(
        &path,
        &input,
        markdown,
        &UnitDisplay::new(&[]),
        &fonts,
        &NumberingConfig::default(),
        &crate::visual_diff::ElementMarks::default(),
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    for title in ["三级标题", "四级标题"] {
        let run = bold_run(&xml, title);
        assert!(run.contains("w:eastAsia=\"仿宋_GB2312\""), "{run}");
        assert!(run.contains("<w:b "), "{run}");
    }
    let body = bold_run(&xml, "务必");
    assert!(body.contains("w:eastAsia=\"黑体\""), "{body}");

    input.profile.style_mode = StyleMode::Compact;
    let compact = temp.path().join("compact-headings.docx");
    write_docx_with_numbering(
        &compact,
        &input,
        "# 测试函\n\n#### 紧缩标题\n紧跟正文。",
        &UnitDisplay::new(&[]),
        &fonts,
        &NumberingConfig::default(),
        &crate::visual_diff::ElementMarks::default(),
    )
    .unwrap();
    let xml = zip_text(&compact, "word/document.xml");
    let run = bold_run(&xml, "紧缩标题");
    assert!(run.contains("w:eastAsia=\"仿宋_GB2312\""), "{run}");
    assert!(run.contains("<w:b "), "{run}");
}

/// 取含 `needle` 的那个 run 的 XML。
fn bold_run<'a>(xml: &'a str, needle: &str) -> &'a str {
    let at = xml.find(needle).unwrap();
    let start = xml[..at].rfind("<w:r>").unwrap();
    let end = xml[at..].find("</w:r>").unwrap() + at + "</w:r>".len();
    &xml[start..end]
}

#[test]
fn image_paragraph_embeds_bitmap_and_docx_contains_media() {
    let temp = tempfile::tempdir().unwrap();
    let paragraph = image_paragraph_from_bytes(
        "示意图",
        "示意图.png",
        false,
        &tiny_png(),
        TABLE_CONTENT_WIDTH_TWIPS,
    )
    .expect("有效 PNG 应生成图片段落");
    let mut doc = Docx::new();
    doc = doc.add_paragraph(paragraph);
    let docx_path = temp.path().join("with-image.docx");
    doc.build().pack(File::create(&docx_path).unwrap()).unwrap();
    let file = File::open(&docx_path).unwrap();
    let archive = zip::ZipArchive::new(file).unwrap();
    let media: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with("word/media/"))
        .map(str::to_string)
        .collect();
    assert!(!media.is_empty(), "docx 应含 word/media/ 图片文件");
    let xml = zip_text(&docx_path, "word/document.xml");
    assert!(xml.contains("<w:drawing>"), "图片段落应含 drawing 元素");
}

#[test]
fn image_paragraph_renders_pdf_as_attachment_note() {
    let temp = tempfile::tempdir().unwrap();
    let paragraph = image_paragraph_from_bytes(
        "",
        "扫描件.pdf",
        true,
        b"%PDF-1.4",
        TABLE_CONTENT_WIDTH_TWIPS,
    )
    .expect("PDF 应生成附件说明段落");
    let mut doc = Docx::new();
    doc = doc.add_paragraph(paragraph);
    let docx_path = temp.path().join("with-pdf.docx");
    doc.build().pack(File::create(&docx_path).unwrap()).unwrap();
    let xml = zip_text(&docx_path, "word/document.xml");
    assert!(xml.contains("【附件】扫描件.pdf"));
    // PDF 不产生媒体文件。
    let file = File::open(&docx_path).unwrap();
    let archive = zip::ZipArchive::new(file).unwrap();
    assert!(
        archive
            .file_names()
            .all(|name| !name.starts_with("word/media/")),
        "PDF 附件说明不应产生媒体文件"
    );
}

#[test]
fn image_paragraph_ignores_undecodable_bytes() {
    assert!(
        image_paragraph_from_bytes(
            "",
            "坏图.png",
            false,
            b"not an image",
            TABLE_CONTENT_WIDTH_TWIPS
        )
        .is_none(),
        "无法解码的字节应跳过而不是 panic"
    );
}

#[test]
fn image_paragraph_skips_unresolvable_src() {
    // 非法路径（穿越）解析失败时跳过，不 panic。
    assert!(image_paragraph("", "../etc/passwd", TABLE_CONTENT_WIDTH_TWIPS).is_none());
}

#[test]
fn short_issuing_unit_keeps_its_size_and_stays_centered() {
    let layout = issuing_unit_header("国务院");
    assert_eq!(layout.size, HEADER_SIZE, "字数少时不得改变字号");
    // 三个字加两个一字宽的字距，居中摆放。
    let block = 5 * HEADER_SIZE as i32 * 10;
    assert_eq!(layout.side_indent, (HEADER_WIDTH_TWIPS - block) / 2);
    assert!(layout.side_indent > 0, "短名称应留出左右缩进而不是铺满版心");
}

#[test]
fn medium_issuing_unit_fills_the_line_without_shrinking() {
    // 10 个字：一字宽的字距已经超出版心，因此正好撑满，字号不变。
    let layout = issuing_unit_header("中华人民共和国教育部");
    assert_eq!(layout.size, HEADER_SIZE);
    assert_eq!(layout.side_indent, 0);
}

#[test]
fn long_issuing_unit_scales_width_like_tex() {
    let unit = "某某省人民政府政务服务和数字化建设管理局办公室综合处";
    let layout = issuing_unit_header(unit);
    assert_eq!(layout.size, HEADER_SIZE, "与 TeX 一样保持红头字高");
    assert!(layout.scale < 100);
    assert_eq!(layout.side_indent, 0);
    let count = unit.chars().count() as i32;
    assert!(
        count * layout.size as i32 * 10 * layout.scale / 100 <= HEADER_WIDTH_TWIPS,
        "缩小后必须能排进版心"
    );
}

#[test]
fn very_long_issuing_unit_still_fits_the_line() {
    let layout = issuing_unit_header(&"某".repeat(60));
    assert_eq!(layout.size, HEADER_SIZE);
    assert!(60 * HEADER_SIZE as i32 * 10 * layout.scale / 100 <= HEADER_WIDTH_TWIPS);
}

#[test]
fn single_character_issuing_unit_is_centered() {
    let layout = issuing_unit_header("函");
    assert_eq!(layout.size, HEADER_SIZE);
    assert_eq!(
        layout.side_indent,
        (HEADER_WIDTH_TWIPS - HEADER_SIZE as i32 * 10) / 2
    );
}

#[test]
fn hierarchical_units_expand_to_full_names_in_header_and_addressee() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("hierarchy.docx");
    let mut input = DraftInput::default();
    input.profile.issuing_unit = "中央网信办新闻舆论处".into();
    input.profile.recipient = "中央网信办新闻舆论处、中央网信办信访接待处".into();
    input.profile.responsible_unit = "中央网信办新闻舆论处".into();
    input.profile.contact_person = "张三".into();
    input.profile.contact_phone = "010-1".into();
    let vocabulary = vec![
        VocabularyEntry {
            canonical: "中央网信办".into(),
            category: VocabularyCategory::Unit,
            ..Default::default()
        },
        VocabularyEntry {
            canonical: "中央网信办新闻舆论处".into(),
            category: VocabularyCategory::Unit,
            parent: "中央网信办".into(),
            ..Default::default()
        },
        VocabularyEntry {
            canonical: "中央网信办信访接待处".into(),
            category: VocabularyCategory::Unit,
            parent: "中央网信办".into(),
            ..Default::default()
        },
    ];
    let display = UnitDisplay::new(&vocabulary);
    write_docx(&path, &input, "# 测试函\n\n正文。", &display).unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 规格 §2.2：发文单位红头使用词库全称。
    assert!(xml.contains("中央网信办新闻舆论处"), "红头应使用词库全称");
    // 同属一个上级的主送单位：顿号连接且不重复上级。
    assert!(xml.contains("中央网信办新闻舆论处、信访接待处"));
    // 规格 §3.1：版记承办单位显示简称（无简称时回落规范名称）。
    assert!(xml.contains("承办单位：中央网信办新闻舆论处"));
}

#[test]
fn external_letter_uses_external_names_but_keeps_responsible_abbr() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external.docx");
    let mut input = DraftInput::default();
    input.profile.correspondence_scope = crate::models::CorrespondenceScope::External;
    input.profile.issuing_unit = "中央网信办新闻舆论处".into();
    input.profile.recipient = "中央网信办信访接待处".into();
    input.profile.responsible_unit = "中央网信办新闻舆论处".into();
    let vocabulary = vec![
        VocabularyEntry {
            canonical: "中央网信办".into(),
            external_name: "国家互联网信息办公室".into(),
            ..Default::default()
        },
        VocabularyEntry {
            canonical: "中央网信办新闻舆论处".into(),
            external_name: "国家互联网信息办公室新闻传播管理处".into(),
            parent: "中央网信办".into(),
            abbr: "新舆处".into(),
            ..Default::default()
        },
        VocabularyEntry {
            canonical: "中央网信办信访接待处".into(),
            external_name: "国家互联网信息办公室公众服务处".into(),
            parent: "中央网信办".into(),
            ..Default::default()
        },
    ];
    write_docx(
        &path,
        &input,
        "# 外部函测试\n\n正文。",
        &UnitDisplay::new(&vocabulary),
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(xml.contains("国家互联网信息办公室新闻传播管理处"));
    assert!(xml.contains("国家互联网信息办公室公众服务处"));
    assert!(xml.contains("承办单位：新舆处"));
}

#[test]
fn red_header_is_written_as_distributed_text_not_stretched_glyphs() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("letter.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "国务院".into();
    input.profile.recipient = "某某市教育局".into();
    write_docx_ok(&path, &input, "# 关于测试红头的函\n\n正文。\n").unwrap();

    // 与 TeX 的 \DocumentHeader 一样，红头与红色反线排在首页页眉里。
    let xml = header_text(&path);
    assert!(
        xml.contains(r#"w:val="distribute""#),
        "红头应使用分散对齐，由 Word 均匀撑开字距"
    );
    let indent = (HEADER_WIDTH_TWIPS - 5 * HEADER_SIZE as i32 * 10) / 2;
    assert!(
        xml.contains(&format!(r#"w:left="{indent}""#)),
        "短名称应靠左右缩进居中，实际 XML：{xml}"
    );
    assert!(
        xml.contains(&format!(r#"w:val="{HEADER_SIZE}""#)),
        "字数少时不得缩小字号"
    );
    assert!(xml.contains(r#"w:color="FF0000""#), "红头应为红色：{xml}");
}

#[test]
fn official_letter_docx_numbers_headings_and_starts_attachments_on_a_new_page() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("letter-with-attachment.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "某单位".into();
    input.profile.recipient = "某部门".into();
    write_docx_ok(
        &path,
        &input,
        "# 测试函\n<!-- [正文] -->\n## 一、总体要求\n### （一）具体事项\n正文。\n<!-- [附件] -->\n# 附件1\n## 统计表\n### 一、填报说明\n附件内容。\n# 附件2\n## 说明材料\n附件二内容。",
    )
    .unwrap();

    let xml = zip_text(&path, "word/document.xml");
    assert!(xml.contains("一、总体要求"));
    assert!(xml.contains("（一）具体事项"));
    assert!(xml.contains("附件1"));
    assert!(xml.contains("统计表"));
    assert!(xml.contains("附件2"));
    assert_eq!(xml.matches("一、填报说明").count(), 1);
    assert_eq!(xml.matches(r#"w:type="page""#).count(), 2, "{xml}");
    // 附件概要：正文结束后、落款前按顺序列出附件名称，与正文空两行、首行缩进两个汉字。
    assert!(xml.contains("附件1：统计表"), "{xml}");
    assert!(xml.contains("　　2：说明材料"), "{xml}");
    let summary = paragraph_containing(&xml, "附件1：统计表");
    assert!(summary.contains("w:eastAsia=\"仿宋_GB2312\""));
    assert!(!summary.contains("w:eastAsia=\"黑体\""));
    assert!(summary.contains(r#"w:firstLine="640""#), "{summary}");
    let gap = gap_between(&xml, "正文。", "附件1：统计表");
    assert_eq!(gap.matches("<w:p ").count(), 2, "概要前应空两行：{gap}");
    assert!(!gap.contains("<w:t"));
    let after_summary = &xml[xml.find("附件1：统计表").unwrap()..];
    let signature = paragraph_containing(after_summary, "某单位");
    assert!(
        signature.contains(&format!(r#"w:before="{CLOSING_GAP_TWIPS}""#)),
        "附件概要与落款之间通常应空 3 行：{signature}"
    );
    let body_heading = paragraph_containing(&xml, "一、总体要求");
    assert!(body_heading.contains(r#"w:firstLine="640""#));
    // 附件区真正的“附件1”黑体标签与“统计表”小标宋标题均位于概要之后。
    let after_summary = &xml[xml.find("附件1：统计表").unwrap() + "附件1：统计表".len()..];
    let attachment_label = paragraph_containing(after_summary, "附件1");
    assert!(attachment_label.contains("w:eastAsia=\"黑体\""));
    assert!(attachment_label.contains("w:ascii=\"黑体\""));
    assert!(attachment_label.contains("w:hAnsi=\"黑体\""));
    assert!(attachment_label.contains("w:val=\"left\""));
    assert!(!attachment_label.contains("w:firstLine"));
    let attachment_title = paragraph_containing(after_summary, "统计表");
    assert!(attachment_title.contains("w:eastAsia=\"方正小标宋简体\""));
    assert!(attachment_title.contains("w:val=\"center\""));
    let attachment_heading = paragraph_containing(&xml, "一、填报说明");
    assert!(attachment_heading.contains(r#"w:firstLine="640""#));
    assert!(!xml.contains("一、一、"));
}

#[test]
fn official_letter_preview_masks_serial_and_day_but_formal_keeps_them() {
    let temp = tempfile::tempdir().unwrap();
    let formal_path = temp.path().join("formal.docx");
    let preview_path = temp.path().join("preview.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "某单位".into();
    input.profile.department_code = "某政函".into();
    input.profile.document_number = "12".into();
    input.date = "2026年8月5日".into();

    write_docx_ok(&formal_path, &input, "# 测试函\n\n正文。").unwrap();
    let formal = zip_text(&formal_path, "word/document.xml");
    // TeX \DocumentHeader 写的是 \DocumentNumber{}~号：序号与“号”之间留一个空格。
    assert!(formal.contains("某政函〔2026〕12 号"));
    assert!(formal.contains("2026年8月5日"));

    input.profile.letter_version = LetterVersion::Preview;
    write_docx_ok(&preview_path, &input, "# 测试函\n\n正文。").unwrap();
    let preview = zip_text(&preview_path, "word/document.xml");
    // 规格 §3.3：预览版占位统一 1em。
    assert!(preview.contains("某政函〔2026〕\u{2003} 号"));
    assert!(preview.contains("2026年8月\u{2003}日"));
    assert!(!preview.contains("某政函〔2026〕12 号"));
    assert!(!preview.contains("2026年8月5日"));
}

#[test]
fn phone_notice_keeps_letter_layout_but_omits_number_and_record() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("phone-notice.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PhoneNotice;
    input.profile.issuing_unit = "某某省教育厅".into();
    input.profile.department_code = "某教函".into();
    input.profile.document_number = "12".into();
    input.profile.recipient = "某某市教育局".into();
    input.profile.copies_to = "某某市财政局".into();
    input.profile.responsible_unit = "办公室".into();
    input.profile.contact_person = "张三".into();
    input.profile.contact_phone = "010-12345678".into();
    write_docx_ok(
        &path,
        &input,
        "# 关于测试事项的电话通知\n<!-- [正文] -->\n## 工作要求\n正文。\n<!-- [附件] -->\n# 附件标题\n附件内容。",
    )
    .unwrap();

    let xml = zip_text(&path, "word/document.xml");
    assert!(xml.contains("某某省教育厅"));
    assert!(xml.contains("某某市教育局"));
    assert!(xml.contains("一、工作要求"));
    assert!(xml.contains("附件"));
    assert!(!xml.contains("某教函"));
    assert!(!xml.contains("〔"));
    assert!(!xml.contains("抄送："));
    assert!(!xml.contains("承办单位："));
    assert!(!xml.contains("张三"));
    assert!(!xml.contains("010-12345678"));
}

#[test]
fn phone_notice_signature_uses_spaced_abbreviation() {
    // 规格 §3.1：电话通知落款显示简称，少于 5 字时逐字加半角空格。
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("phone-notice-abbr.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PhoneNotice;
    input.profile.issuing_unit = "中央宣传部".into();
    let vocabulary = vec![VocabularyEntry {
        canonical: "中央宣传部".into(),
        category: VocabularyCategory::Unit,
        abbr: "中宣部".into(),
        ..Default::default()
    }];
    let display = UnitDisplay::new(&vocabulary);
    write_docx(&path, &input, "# 电话通知\n\n正文。", &display).unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 红头保留全称（排在首页页眉里），落款输出“中 宣 部”（简称逐字加半角空格）。
    assert!(header_text(&path).contains("中央宣传部"), "红头应保留全称");
    let signature = paragraph_containing(&xml, "中 宣 部");
    // TeX \SignatureContent：右对齐的 11cm minipage 内居中，即左缩进 2608 后居中。
    assert!(signature.contains("w:val=\"center\""), "{signature}");
    assert!(signature.contains(r#"w:left="2608""#), "{signature}");
    assert!(!xml.contains("中宣部"), "不得输出未加空格的简称");
}

#[test]
fn white_paper_signature_stacks_multiple_units_right_aligned_with_spread() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("white-paper-multi.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::WhitePaper;
    input.profile.signing_unit = "星海省教育厅、教师工作处".into();
    input.profile.use_short_name_for_signature = true;
    input.date = "2026年8月7日".into();
    let vocabulary = vec![
        VocabularyEntry {
            canonical: "星海省教育厅".into(),
            category: VocabularyCategory::Unit,
            abbr: "省教育厅".into(),
            ..Default::default()
        },
        VocabularyEntry {
            canonical: "教师工作处".into(),
            category: VocabularyCategory::Unit,
            abbr: "教师处".into(),
            ..Default::default()
        },
    ];
    let display = UnitDisplay::new(&vocabulary);
    write_docx(&path, &input, "# 标题\n\n正文。", &display).unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 两个单位分行列出；简称逐字成 run，字间距 320 缇（1em，三号 16pt 下分散到 5 字宽）。
    for ch in ["省", "教", "厅", "教", "师", "处"] {
        assert!(
            xml.contains(&format!("<w:t xml:space=\"preserve\">{ch}</w:t>")),
            "应含“{ch}”：{xml}"
        );
    }
    assert!(
        xml.contains("w:spacing w:val=\"320\""),
        "3 字简称应有 1em 字符间距：{xml}"
    );
    // 单位段落右对齐、右侧留签字空间。
    assert!(xml.contains("w:val=\"right\""), "落款应右对齐：{xml}");
    // 成文日期居中于“落款单位 + 签字空间”：左缩进到单位块（分散后 5 字宽）
    // 的左沿，段落右缘是版心右缘。
    let date = paragraph_containing(&xml, "2026年8月7日");
    let expected_left = TABLE_CONTENT_WIDTH_TWIPS
        - crate::export::SIGNATURE_ROOM_TWIPS
        - crate::export::red_signature_unit_width_twips(&["省教育厅".to_string()]);
    assert!(
        date.contains(r#"w:val="center""#)
            && date.contains(&format!(r#"w:left="{expected_left}""#)),
        "白头件成文日期应居中于单位与签字空间之间：{date}"
    );
    // 不出现未分散的整串简称。
    assert!(!xml.contains("省教育厅"), "简称不应整串出现：{xml}");
}

#[test]
fn white_paper_signature_single_unit_keeps_full_name() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("white-paper-single.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::WhitePaper;
    input.profile.signing_unit = "星海省教育厅".into();
    input.date = "2026年8月7日".into();
    write_docx_ok(&path, &input, "# 标题\n\n正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(
        xml.contains("星海省教育厅"),
        "单单位未选简称应输出全称：{xml}"
    );
}

#[test]
fn official_letter_seal_on_behalf_follows_the_unit_on_the_same_line() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("seal-on-behalf.docx");
    let mut input = DraftInput::default();
    input.profile.issuing_unit = "星海省教育厅".into();
    input.date = "2026年8月7日".into();
    write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(!xml.contains("（代章）"), "单位未配置时不得标注代章");

    let vocabulary = vec![VocabularyEntry {
        category: VocabularyCategory::Unit,
        canonical: "星海省教育厅".into(),
        seal_on_behalf: true,
        ..Default::default()
    }];
    write_docx(
        &path,
        &input,
        "# 测试函\n\n正文。",
        &UnitDisplay::new(&vocabulary),
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 落款单位出现在红头与落款两处，最后一次出现是落款；“（代章）”直接跟在
    // 落款单位后面同一行（“星海省教育厅（代章）”），成文日期在其后另起一行。
    let unit_pos = xml.rfind("星海省教育厅").expect("落款单位");
    let seal_pos = xml.find("（代章）").expect("代章标注");
    let date_pos = xml.find("2026年8月7日").expect("成文日期");
    assert!(unit_pos < seal_pos && seal_pos < date_pos);
    assert!(
        xml.contains("星海省教育厅（代章）"),
        "代章应与落款单位同一行：{xml}"
    );
    let seal = paragraph_containing(&xml, "（代章）");
    // 落款整体在 TeX 的 11cm 右侧 minipage 内居中，代章与落款单位同段同侧。
    assert!(seal.contains("w:val=\"center\""), "{seal}");
    assert!(seal.contains(r#"w:left="2608""#), "{seal}");
}

#[test]
fn landscape_plain_document_turns_the_page_and_widens_tables_and_title() {
    let temp = tempfile::tempdir().unwrap();
    let markdown = "# 关于印发全市社区养老服务设施提质改造任务分解表的通知
<!-- [正文] -->
        | 序号 | 区域 | 改造内容 |
| --- | --- | --- |
| 1 | 东城区 | 无障碍通道 |
";
    let grid_total = |xml: &str| -> usize {
        Regex::new(r#"<w:gridCol w:w="(\d+)""#)
            .unwrap()
            .captures_iter(xml)
            .map(|c| c[1].parse::<usize>().unwrap())
            .sum()
    };
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;
    input.profile.landscape = true;
    let path = temp.path().join("landscape.docx");
    write_docx_ok(&path, &input, markdown).unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(xml.contains(r#"w:orient="landscape""#), "{xml}");
    assert!(xml.contains(r#"w:w="16838""#) && xml.contains(r#"w:h="11906""#));
    assert!(xml.contains(&format!(r#"w:left="{LANDSCAPE_MARGIN_LEFT_TWIPS}""#)));
    assert_eq!(grid_total(&xml), LANDSCAPE_CONTENT_WIDTH_TWIPS);
    // 26 个字的标题在竖页要换行，横页一行放得下（28 字）。
    assert!(!paragraph_containing(&xml, "关于印发").contains("w:br"));

    // 横页只对普通公文生效：别的文种勾着也照旧竖排。
    input.kind = TemplateKind::WhitePaper;
    let path = temp.path().join("portrait.docx");
    write_docx_ok(&path, &input, markdown).unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(!xml.contains("landscape"));
    assert_eq!(grid_total(&xml), TABLE_CONTENT_WIDTH_TWIPS);
}

#[test]
fn plain_document_keeps_body_and_attachments_but_omits_all_decorative_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plain-document.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;
    input.profile.issuing_unit = "不应出现的发文单位".into();
    input.profile.recipient = "不应出现的主送单位".into();
    input.profile.signing_unit = "不应出现的落款单位".into();
    input.profile.security_level = "秘密".into();
    input.profile.security_period = "10年".into();
    input.profile.special_handling = true;
    input.date = "2026年8月7日".into();
    write_docx_ok(
        &path,
        &input,
        "# 普通公文测试\n<!-- [正文] -->\n## 工作要求\n正文内容。\n<!-- [附件] -->\n# 附件1\n## 附件标题\n附件内容。",
    )
    .unwrap();

    let xml = zip_text(&path, "word/document.xml");
    assert!(runs_text(&xml).contains("秘密★10年"));
    assert!(xml.contains("普通公文测试"));
    assert!(xml.contains("一、工作要求"));
    assert!(xml.contains("正文内容。"));
    assert!(xml.contains("附件：附件标题"));
    assert!(xml.contains("附件内容。"));
    assert!(!xml.contains("不应出现的发文单位"));
    assert!(!xml.contains("不应出现的主送单位"));
    assert!(!xml.contains("不应出现的落款单位"));
    assert!(!xml.contains("2026年8月7日"));
    assert!(!xml.contains("指人专办"));
    assert!(!xml.contains("C00000"), "普通公文不得出现红头颜色");
}

#[test]
fn joint_mode_one_uses_main_header_two_column_signatures_and_multiline_record() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("joint-letter.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
    input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
    input.profile.main_issuing_unit = "乙单位".into();
    input.profile.department_code = "乙函".into();
    input.profile.document_number = "8".into();
    input.profile.recipient = "收文单位".into();
    input.profile.copies_to = "丁单位、戊单位".into();
    input.profile.joint_responsible_units = "甲处室、乙处室".into();
    input.profile.joint_contacts = vec![
        JointContact {
            unit: "甲处室".into(),
            name: "张三".into(),
            phone: "010-11111111".into(),
        },
        JointContact {
            unit: "乙处室".into(),
            name: "李四".into(),
            phone: "010-22222222".into(),
        },
    ];
    input.date = "2026年8月5日".into();
    write_docx_ok(&path, &input, "# 联合发文测试函\n\n正文。").unwrap();

    let xml = zip_text(&path, "word/document.xml");
    let header_xml = header_text(&path);
    let header = paragraph_containing(&header_xml, "乙单位");
    assert!(header.contains("FF0000"), "主发文单位应写入红头：{header}");
    assert!(!header.contains("甲单位、乙单位、丙单位"));
    assert!(xml.contains("甲单位"));
    assert!(xml.contains("丙单位"));
    assert!(xml.contains(r#"w:trHeight w:val="2551""#));
    // 规格 §2.5：三个单位时最后一个单位跨两列居中。
    assert!(xml.contains(r#"w:gridSpan w:val="2""#));
    assert!(xml.contains("2026年8月5日"));
    // 日期压在主发文单位“乙单位”（右列）所在单元格，而不是整行居中。
    let date_at = xml.find("2026年8月5日").unwrap();
    let cell_start = xml[..date_at].rfind("<w:tc>").unwrap();
    let cell_end = xml[date_at..].find("</w:tc>").unwrap() + date_at + "</w:tc>".len();
    let row_start = xml[..cell_start].rfind("<w:tr>").unwrap();
    let row_end = xml[cell_end..].find("</w:tr>").unwrap() + cell_end + "</w:tr>".len();
    let closing_row = &xml[row_start..row_end];
    assert_eq!(closing_row.matches("<w:tc>").count(), 2, "收尾行应有两列");
    let first_cell_end = closing_row.find("</w:tc>").unwrap() + "</w:tc>".len();
    assert!(
        closing_row[first_cell_end..].contains("2026年8月5日"),
        "日期应在主发文单位（右列）单元格内"
    );
    assert_eq!(xml.matches("承办单位：").count(), 1);
    assert_eq!(xml.matches("联系人：").count(), 1);
    assert_eq!(xml.matches("联系电话：").count(), 1);
    // 联系电话列预留 11em（标签 5em + 11 位半角数字 5.5em + 0.5em 余量），
    // 其余两列均分剩余宽度，11 位电话号码不再换行。
    assert_eq!(xml.matches(r#"w:gridCol w:w="2882""#).count(), 2);
    assert_eq!(xml.matches(r#"w:gridCol w:w="3080""#).count(), 1);
    assert!(
        xml.contains(r#"w:tcW w:w="3080""#),
        "联系电话单元格宽度应能容纳 11 位数字"
    );
    assert!(xml.contains(r#"w:tab w:val="right" w:pos="8844""#));
    assert!(xml.contains("（共印5份）"));
    // 版记首行抄送跨三列，单元格带 gridSpan=3；回行时用悬挂缩进对齐单位名称。
    assert!(xml.contains(r#"w:gridSpan w:val="3""#));
    assert!(paragraph_containing(&xml, "抄送：").contains(r#"w:hanging="840""#));
    // 规格 §3.2：第 2 行起承办单位、联系人用左缩进对齐第一行标签后的位置。
    assert!(xml.contains(r#"w:left="1400""#));
    assert!(xml.contains(r#"w:left="1120""#));
    assert!(paragraph_containing(&xml, "承办单位：").contains(r#"w:sz w:val="28""#));
    assert!(paragraph_containing(&xml, "（共印5份）").contains(r#"w:sz w:val="28""#));
    assert!(xml.contains("甲处室"));
    assert!(xml.contains("乙处室"));
    // 规格 §3.2：2 字姓名中间加全角空格，占 3 字宽。
    assert!(xml.contains("张\u{2003}三"));
    assert!(xml.contains("李\u{2003}四"));
    assert!(xml.contains("010-11111111"));
    assert!(xml.contains("010-22222222"));
    // 版记行高必须交给 Word 自动撑开：承办单位没维护简称时会回落较长的
    // 规范全称，固定行高会把折行后的第二行直接裁掉。
    let record_at = xml.find("承办单位：").unwrap();
    let record_row_start = xml[..record_at].rfind("<w:tr>").unwrap();
    assert!(
        !xml[record_row_start..record_at].contains("hRule=\"exact\""),
        "联合发文版记行不得使用固定行高：{}",
        &xml[record_row_start..record_at]
    );
}

#[test]
fn joint_mode_one_single_unit_falls_back_to_right_aligned_signature() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("joint-single.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
    input.profile.joint_issuing_units = "甲单位".into();
    input.profile.main_issuing_unit = "甲单位".into();
    input.profile.department_code = "甲函".into();
    input.profile.document_number = "9".into();
    input.profile.recipient = "收文单位".into();
    input.profile.joint_responsible_units = "甲处室".into();
    input.profile.joint_contacts = vec![JointContact {
        unit: "甲处室".into(),
        name: "张三".into(),
        phone: "010-11111111".into(),
    }];
    input.date = "2026年8月5日".into();
    write_docx_ok(&path, &input, "# 单单位联合函\n\n正文。").unwrap();

    let xml = zip_text(&path, "word/document.xml");
    // 只剩一个发文单位时落款回落单独发文那套：TeX \SignatureContent 的
    // 11cm 右侧 minipage 内居中，而不是联合发文的两列表格。
    let mut search_from = 0;
    let mut last_sig = "";
    while let Some(at) = xml[search_from..].find("甲单位") {
        let abs = search_from + at;
        let start = xml[..abs].rfind("<w:p ").unwrap();
        let end = xml[abs..].find("</w:p>").unwrap() + abs + "</w:p>".len();
        last_sig = &xml[start..end];
        search_from = end;
    }
    assert!(!last_sig.is_empty(), "落款应出现“甲单位”");
    assert!(
        last_sig.contains(r#"w:jc w:val="center""#) && last_sig.contains(r#"w:left="2608""#),
        "单单位联合发文落款应回落单独发文的居中落款：{last_sig}"
    );
    assert!(
        !last_sig.contains("<w:tbl>"),
        "不应再走联合落款两列表格：{last_sig}"
    );
    // 联合落款的两列表格（72mm 列）不再出现。
    assert!(!xml.contains(r#"w:gridCol w:w="4422""#));
}

#[test]
fn official_letter_docx_supports_simplex_and_duplex_page_numbers() {
    let temp = tempfile::tempdir().unwrap();
    let simplex_path = temp.path().join("simplex.docx");
    let duplex_path = temp.path().join("duplex.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;

    write_docx_ok(&simplex_path, &input, "# 测试函\n\n正文。").unwrap();
    let simplex_document = zip_text(&simplex_path, "word/document.xml");
    let simplex_settings = zip_text(&simplex_path, "word/settings.xml");
    let simplex_footer = zip_text(&simplex_path, "word/footer1.xml");
    let simplex_first_footer = zip_text(&simplex_path, "word/footer2.xml");
    assert!(simplex_document.contains(r#"w:type="default""#));
    assert!(simplex_document.contains(r#"w:type="first""#));
    assert!(!simplex_document.contains(r#"w:type="even""#));
    assert!(!simplex_settings.contains("w:evenAndOddHeaders"));
    assert!(simplex_footer.contains(r#"w:val="center""#));
    assert!(simplex_footer.contains("<w:instrText>PAGE</w:instrText>"));
    assert!(!simplex_first_footer.contains("<w:instrText>PAGE</w:instrText>"));

    input.profile.duplex_printing = true;
    write_docx_ok(&duplex_path, &input, "# 测试函\n\n正文。").unwrap();
    let duplex_document = zip_text(&duplex_path, "word/document.xml");
    let duplex_settings = zip_text(&duplex_path, "word/settings.xml");
    let odd_footer = zip_text(&duplex_path, "word/footer1.xml");
    let even_footer = zip_text(&duplex_path, "word/footer2.xml");
    let first_footer = zip_text(&duplex_path, "word/footer3.xml");
    assert!(duplex_document.contains(r#"w:type="default""#));
    assert!(duplex_document.contains(r#"w:type="even""#));
    assert!(duplex_document.contains(r#"w:type="first""#));
    assert!(duplex_settings.contains("w:evenAndOddHeaders"));
    assert!(odd_footer.contains(r#"w:val="right""#));
    assert!(even_footer.contains(r#"w:val="left""#));
    assert!(odd_footer.contains("<w:instrText>PAGE</w:instrText>"));
    assert!(even_footer.contains("<w:instrText>PAGE</w:instrText>"));
    assert!(!first_footer.contains("<w:instrText>PAGE</w:instrText>"));
}

/// 页码高度：GB/T 9704—2012 要求一字线上距版心下边缘 7 mm。
/// Word 的 `w:footer` 量到页脚块**下沿**，所以要再扣掉页码那一行的行高；
/// 行高和页脚距边界必须一起改，这里把两者绑在同一个断言里。
#[test]
fn page_number_sits_seven_millimetres_below_the_text_area() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("footer-height.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();

    let document = zip_text(&path, "word/document.xml");
    assert!(
        document.contains(&format!(r#"w:bottom="{BODY_BOTTOM_MARGIN_TWIPS}""#)),
        "版心下边距应为 35mm：{document}"
    );
    assert!(
        document.contains(&format!(r#"w:footer="{FOOTER_DISTANCE_TWIPS}""#)),
        "页脚距边界应为 35mm − 7mm − 页码行高 = {FOOTER_DISTANCE_TWIPS} twip：{document}"
    );
    let footer = zip_text(&path, "word/footer1.xml");
    assert!(
        footer.contains(&format!(
            r#"<w:spacing w:line="{PAGE_NUMBER_LINE_TWIPS}" w:lineRule="exact" />"#
        )),
        "页码行高变了，页脚距边界的反算就不成立：{footer}"
    );
    // 页码行上沿 = 纸底 − 页脚距边界 − 行高，应正好落在版心下边缘往下 7mm。
    assert_eq!(
        BODY_BOTTOM_MARGIN_TWIPS - FOOTER_DISTANCE_TWIPS - PAGE_NUMBER_LINE_TWIPS,
        PAGE_NUMBER_GAP_TWIPS
    );
}

#[test]
fn attachment_table_uses_smart_fixed_grid_and_official_fonts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("letter-table.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    write_docx_ok(
        &path,
        &input,
        "# 测试函\n<!-- [附件] -->\n# 附件1\n## 统计表\n| 序号 | 名称 | 详细说明 |\n| --- | --- | --- |\n| 1 | 甲 | 这是一段**重要说明**。 |\n| 2 | 乙 | 另一段较长的说明文字。 |",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(xml.contains(r#"w:tblLayout w:type="fixed""#));
    assert!(xml.contains(r#"w:gridCol w:w="560""#));
    assert!(xml.contains("w:eastAsia=\"黑体\""));
    assert!(xml.contains("w:eastAsia=\"仿宋_GB2312\""));
    assert!(xml.contains("w:insideH"));
    assert!(xml.contains("w:insideV"));
    let bold_cell = paragraph_containing(&xml, "重要说明");
    assert!(
        bold_cell.contains("<w:b"),
        "表格 Markdown 加粗应保留：{bold_cell}"
    );
    assert!(!bold_cell.contains("**"));
}

#[test]
fn body_table_writes_grid_span_and_vertical_merge() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("merged-table.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;
    write_docx_ok(
        &path,
        &input,
        "# 合并表格
| 类别 | 项目 | 说明 |
| --- | --- | --- |
| 横向合并 || 备注 |
| 纵向合并 | 事项一 | 甲 |
| ^^ | 事项二 | 乙 |",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(xml.contains(r#"w:gridSpan w:val="2""#), "{xml}");
    assert!(xml.contains(r#"w:vMerge w:val="restart""#), "{xml}");
    assert!(xml.contains(r#"w:vMerge w:val="continue""#), "{xml}");
    assert_eq!(xml.matches("横向合并").count(), 1);
    assert_eq!(xml.matches("纵向合并").count(), 1);
}

#[test]
fn table_cell_breaks_become_soft_returns_in_one_paragraph() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cell-breaks.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;
    write_docx_ok(
        &path,
        &input,
        "# 单元格换行
| 单位 | 联系方式 |
| --- | --- |
| 办公室 | 张某<br>电话：**12345678** |",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    let cell = table_cell_containing(&xml, "张某");
    assert_eq!(
        cell.matches("<w:p ").count() + cell.matches("<w:p>").count(),
        1
    );
    assert_eq!(
        cell.matches(r#"<w:br w:type="textWrapping""#).count(),
        1,
        "{cell}"
    );
    assert!(!cell.contains("&lt;br&gt;"), "{cell}");
    assert!(cell.contains("<w:b"), "换行后的加粗仍要保留：{cell}");
}

#[test]
fn compact_style_merges_heading_and_paragraph_into_one_paragraph() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("compact.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.style_mode = StyleMode::Compact;
    input.profile.responsible_unit = "办公室".into();
    input.profile.contact_person = "张三".into();
    input.profile.contact_phone = "010-1".into();
    write_docx_ok(&path, &input, "# 测试函\n\n## 任务目标\n测试正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 标题与正文并入同一段：黑体标题 run + 仿宋正文 run。
    let merged = paragraph_containing(&xml, "任务目标");
    assert!(
        merged.contains("一、任务目标。"),
        "标题应带编号与句号：{merged}"
    );
    assert!(
        merged.contains("测试正文"),
        "紧缩风格应把正文并入标题段：{merged}"
    );
    assert!(merged.contains("黑体"), "标题部分应使用标题字体：{merged}");
}

#[test]
fn compact_style_merges_every_deepest_heading_paragraph_pair() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("compact-deep.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.style_mode = StyleMode::Compact;
    input.profile.responsible_unit = "办公室".into();
    input.profile.contact_person = "张三".into();
    input.profile.contact_phone = "010-1".into();
    // 正文区 # 号最多的是 3 级（###）：每个“3 级标题+段落”都合并，2 级标题保持独立。
    write_docx_ok(
        &path,
        &input,
        "# 测试函\n\n## 一、总体要求\n开头段落。\n### （一）任务一\n任务一正文。\n### （二）任务二\n任务二正文。",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 2 级标题保持独立：正文段落单独成段，不并入标题段。
    let level_two = paragraph_containing(&xml, "总体要求");
    assert!(level_two.contains("一、总体要求"));
    assert!(
        !level_two.contains("开头段落"),
        "2 级标题不应合并正文：{level_two}"
    );
    // 每个 3 级标题都与紧随正文合并，用楷体标题字体。
    for (title, body) in [
        ("（一）任务一", "任务一正文"),
        ("（二）任务二", "任务二正文"),
    ] {
        let merged = paragraph_containing(&xml, title);
        assert!(
            merged.contains(&format!("{title}。")),
            "标题应带编号与句号：{merged}"
        );
        assert!(
            merged.contains(body),
            "每个最深层标题都应合并正文：{merged}"
        );
        assert!(
            merged.contains("楷体_GB2312"),
            "3 级标题应使用楷体标题字体：{merged}"
        );
    }
}

#[test]
fn section_compact_merges_each_sections_own_deepest_heading() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("section-compact.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.style_mode = StyleMode::SectionCompact;
    input.profile.responsible_unit = "办公室".into();
    input.profile.contact_person = "张三".into();
    input.profile.contact_phone = "010-1".into();
    write_docx_ok(
        &path,
        &input,
        "# 测试函\n\n## 总体要求\n节首正文。\n### 具体任务\n任务正文。\n## 工作安排\n安排正文。",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(paragraph_containing(&xml, "具体任务").contains("任务正文"));
    assert!(!paragraph_containing(&xml, "总体要求").contains("节首正文"));
    assert!(paragraph_containing(&xml, "工作安排").contains("安排正文"));
}

#[test]
fn empty_copies_to_leaves_the_record_line_without_a_dangling_label() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("no-copies.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.recipient = "某部门".into();
    input.profile.responsible_unit = "办公室".into();
    input.profile.contact_person = "张三".into();
    input.profile.contact_phone = "010-1".into();
    write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // 与 gonghan-gwa.cls 的 \FooterCopiesLine 一致：没有抄送单位时整行不写“抄送：”。
    assert!(!xml.contains("抄送："), "无抄送单位时不应留下空标签");
    assert!(xml.contains("（共印2份）"));
    assert!(xml.contains("承办单位：办公室"));
}

#[test]
fn letter_record_floats_at_the_bottom_of_the_last_page() {
    let temp = tempfile::tempdir().unwrap();
    for (name, markdown) in [
        ("plain", "# 测试函\n\n正文。"),
        (
            "attachment",
            "# 测试函\n\n<!-- [正文] -->\n\n正文。\n\n<!-- [附件] -->\n\n# 附件标题\n\n附件正文。",
        ),
    ] {
        let path = temp.path().join(format!("{name}.docx"));
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.recipient = "某部门".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-1".into();
        write_docx_ok(&path, &input, markdown).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        // 版记是正文最后一张表：浮动、底边贴版心底，与上方内容至少隔 1cm。
        let table = &xml[xml.rfind("<w:tbl>").unwrap()..];
        assert!(table.contains("共印"), "{name}：最后一张表应是版记");
        let position = &table[table.find("<w:tblpPr").unwrap()..];
        let position = &position[..position.find("/>").unwrap()];
        for attr in [
            r#"w:vertAnchor="margin""#,
            r#"w:tblpYSpec="bottom""#,
            r#"w:horzAnchor="margin""#,
            r#"w:topFromText="567""#,
        ] {
            assert!(
                position.contains(attr),
                "{name}：版记定位缺 {attr}：{position}"
            );
        }
        // 浮动表锚在其后的段落上：表后必须还有一个段落，再接节属性。
        let after = &table[table.find("</w:tbl>").unwrap()..];
        let anchor = after.find("<w:p").expect("版记后应有锚段");
        assert!(
            anchor < after.find("<w:sectPr").unwrap(),
            "{name}：锚段应在节属性之前"
        );
    }
}

#[test]
fn red_head_approval_docx_has_floating_frame_records_small_title_and_second_page_signature() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("red-approval.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::RedHeadApproval;
    input.profile.kind = TemplateKind::RedHeadApproval;
    input.profile.issuing_unit = "某某委员会办公室".into();
    input.profile.department_code = "某办呈".into();
    input.profile.document_year = "2026".into();
    input.profile.document_number = "12".into();
    input.profile.reporting_leaders = "张三、李四".into();
    input.profile.signing_unit = "某某委员会".into();
    input.profile.joint_responsible_units = "综合处、业务处".into();
    input.profile.joint_contacts = vec![
        JointContact {
            unit: "综合处".into(),
            name: "王五".into(),
            phone: "010-12345678".into(),
        },
        JointContact {
            unit: "业务处".into(),
            name: "赵六".into(),
            phone: "010-87654321".into(),
        },
    ];
    input.date = "2026年8月12日".into();
    write_docx_ok(
        &path,
        &input,
        "# 关于认真做好网络安全与信息化重点工作的请示\n\n现将有关情况呈报如下。妥否，请指示。",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    // TeX RedApprovalPageOverlay 写 \DocumentNumber{}号，中间不留空格。
    assert!(xml.contains("某办呈〔2026〕12号"));
    // 批示框、贯穿红线与承办区排在正文流最前面（对应 TeX 的绝对定位 picture）。
    // 不能放页眉：WPS 会丢弃页眉内的高浮动表且不绕排。
    let body_start = xml.find("<w:body>").unwrap() + "<w:body>".len();
    assert!(
        xml[body_start..].starts_with("<w:tbl>"),
        "三张浮动表应排在正文流最前面：{}",
        &xml[body_start..body_start + 200]
    );
    assert!(xml.contains("批　示"));
    assert!(xml.contains("w:tblpXSpec=\"right\""));
    assert!(xml.contains("w:tblpY=\"2720\""));
    assert!(xml.contains("w:tblpYSpec=\"bottom\""));
    assert!(xml.contains("承办单位："));
    assert!(xml.contains("综合处"));
    assert!(xml.contains("业务处"));
    let title = paragraph_containing(&xml, "关于认真做好网络安全与");
    assert!(title.contains("w:sz w:val=\"36\""));
    assert!(title.contains("w:right=\"3175\""));
    let notice = paragraph_containing(&xml, "（此页无正文）");
    assert!(notice.contains("w:pageBreakBefore"));
    assert!(xml.find("（此页无正文）").unwrap() < xml.rfind("某某委员会").unwrap());
    // 落款单位右对齐，但右缩进 4cm 让出签字空间。从「此页无正文」之后找起，
    // 免得匹配到红头里的「某某委员会办公室」。
    let closing = &xml[xml.find("（此页无正文）").unwrap()..];
    let signature = paragraph_containing(closing, "某某委员会");
    assert!(
        signature.contains(r#"w:val="right""#)
            && signature.contains(&format!(
                r#"w:right="{}""#,
                crate::export::SIGNATURE_ROOM_TWIPS
            )),
        "落款单位右侧应留出签字空间：{signature}"
    );
    // 成文日期居中于“落款单位 + 签字空间”：左缩进到最宽单位的左沿，
    // 段落右缘仍是版心右缘，居中位置正好是这一整段的中点。
    let date = paragraph_containing(&xml, "2026年8月12日");
    let expected_left = TABLE_CONTENT_WIDTH_TWIPS
        - crate::export::SIGNATURE_ROOM_TWIPS
        - crate::export::red_signature_unit_width_twips(&["某某委员会".to_string()]);
    assert!(
        date.contains(r#"w:val="center""#)
            && date.contains(&format!(r#"w:left="{expected_left}""#)),
        "成文日期应居中于单位与签字空间之间：{date}"
    );
    // 承办区栏宽按内容一次算定：联系人栏固定 9 em（标签 4 + 手写签字空位 4 + 留白 1），
    // 电话栏按最长号码定宽，承办单位栏吃版心余量。本例两条号码都是 12 位 → 3084/2880/2880。
    assert!(
        xml.contains(&format!(
            "<w:gridCol w:w=\"{}\" w:type=\"dxa\" /><w:gridCol w:w=\"{}\" w:type=\"dxa\" /><w:gridCol w:w=\"{}\" w:type=\"dxa\" />",
            3_084, 2_880, 2_880
        )),
        "承办区栏宽应与 LaTeX/预览同源：{xml}"
    );
    // 多条承办条目：标签只在首行出现一次，续行只排取值并保持对齐。
    assert_eq!(xml.matches("承办单位：").count(), 1, "{xml}");
    assert_eq!(xml.matches("联系人：").count(), 1, "{xml}");
    assert_eq!(xml.matches("电话：").count(), 1, "{xml}");
}

/// 承办单位一律不换行：栏内放不下时整格按同一比例横向压窄（`w:w`），
/// 行高固定一行，与 LaTeX 的 \RedFit 和首页承办区高度对齐。
#[test]
fn red_head_approval_record_compresses_long_units_instead_of_wrapping() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("red-approval-long-unit.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::RedHeadApproval;
    input.profile.kind = TemplateKind::RedHeadApproval;
    input.profile.issuing_unit = "某某委员会办公室".into();
    input.profile.signing_unit = "某某委员会".into();
    input.profile.joint_responsible_units = "教师工作与师资管理处、综合处".into();
    input.profile.joint_contacts = vec![
        JointContact {
            unit: "教师工作与师资管理处".into(),
            name: "王五".into(),
            phone: "010-12345678".into(),
        },
        JointContact {
            unit: "综合处".into(),
            name: "赵六".into(),
            phone: "010-87654321".into(),
        },
    ];
    input.date = "2026年8月12日".into();
    write_docx_ok(&path, &input, "# 标题\n\n正文。妥否，请指示。").unwrap();
    // 承办区排在正文流最前面（随三张浮动表一起）。
    let xml = zip_text(&path, "word/document.xml");
    let columns = crate::export::red_record_signing_columns(&[
        [
            "教师工作与师资管理处".to_string(),
            "王五".to_string(),
            "010-12345678".to_string(),
        ],
        [
            "综合处".to_string(),
            "赵六".to_string(),
            "010-87654321".to_string(),
        ],
    ]);
    // 长单位名在首行：标签与取值一起按同一比例压窄。
    let scale = crate::export::red_record_scale_units(
        crate::export::red_record_unit_twips("教师工作与师资管理处"),
        columns.unit_usable(),
    );
    assert!(scale < 100, "长单位名应触发压缩");
    assert!(
        xml.contains(&format!("<w:w w:val=\"{scale}\"")),
        "长单位名应按 {scale}% 横向压窄：{xml}"
    );
    // 短单位名不压缩，也不得整行被裁：行高固定为一行 28pt。
    assert!(
        xml.contains("<w:trHeight w:val=\"560\" w:hRule=\"exact\" />"),
        "{xml}"
    );
}

/// 红头呈批件正文里的表格要被赶出首页，否则会压过右侧批示栏。
#[test]
fn red_head_approval_body_table_starts_on_a_new_page() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("red-approval-table.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::RedHeadApproval;
    input.profile.kind = TemplateKind::RedHeadApproval;
    input.profile.issuing_unit = "某某委员会办公室".into();
    input.profile.signing_unit = "某某委员会".into();
    input.profile.joint_responsible_units = "综合处".into();
    input.date = "2026年8月12日".into();
    write_docx_ok(
        &path,
        &input,
        "# 标题\n\n短正文。\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n\n妥否，请指示。",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    let table = xml.find("甲").expect("正文应含表格");
    let break_before = xml[..table]
        .rfind("w:pageBreakBefore")
        .expect("表格前应有换页");
    assert!(
        xml[break_before..table].find("短正文").is_none(),
        "换页要排在表格之前、短正文之后：{xml}"
    );
    // 正文因表格延续到第二页，落款接在正文之后，不再制造“此页无正文”空白页。
    assert!(!xml.contains("（此页无正文）"), "{xml}");
}

fn zip_text(path: &Path, entry: &str) -> String {
    let file = File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut text = String::new();
    archive
        .by_name(entry)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

/// 红头与红色反线排在首页页眉部件的浮动表里（与 TeX 的绝对定位对应），
/// 断言这些元素时要连页眉一起看。红头呈批件的框线/承办区不在此列：
/// 它们排在正文流最前面（见 write_docx_with_numbering 的注释）。
fn header_text(path: &Path) -> String {
    let file = File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut names = archive
        .file_names()
        .filter(|name| name.starts_with("word/header"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    names.sort();
    let mut text = String::new();
    for name in names {
        archive
            .by_name(&name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
    }
    text
}

fn paragraph_containing<'a>(xml: &'a str, needle: &str) -> &'a str {
    let needle_at = xml.find(needle).unwrap();
    let start = xml[..needle_at].rfind("<w:p ").unwrap();
    let end = xml[needle_at..].find("</w:p>").unwrap() + needle_at + "</w:p>".len();
    &xml[start..end]
}

/// 取含 `needle` 的那个表格单元格的 XML：合并跨度 `w:gridSpan` 写在
/// `<w:tcPr>` 里，看段落看不到。
fn table_cell_containing<'a>(xml: &'a str, needle: &str) -> &'a str {
    let needle_at = xml.find(needle).unwrap();
    let start = xml[..needle_at]
        .rfind("<w:tc>")
        .or_else(|| xml[..needle_at].rfind("<w:tc "))
        .unwrap();
    let end = xml[needle_at..].find("</w:tc>").unwrap() + needle_at + "</w:tc>".len();
    &xml[start..end]
}

#[test]
fn ordered_lists_use_circles_inline_and_two_char_first_line_indent_when_independent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ordered-lists.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::PlainDocument;
    write_docx_ok(
        &path,
        &input,
        "# 测试\n\n正文：\n1. 第一项；\n1. 第二项，\n\n1. 独立甲，\n1. 独立乙；",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(xml.contains("正文：①第一项；②第二项。"), "{xml}");
    let first = paragraph_containing(&xml, "1.独立甲。");
    assert!(first.contains(r#"w:firstLine="640""#), "{first}");
    let second = paragraph_containing(&xml, "2.独立乙。");
    assert!(second.contains(r#"w:firstLine="640""#), "{second}");
    assert!(!xml.contains("1. 独立甲"), "编号与正文之间不得留空格");
}

/// 取一段 XML 里所有 `<w:t>` 文本拼接后的纯文本。密级行的数字年限拆成了
/// “黑体 + 等宽数字 + 黑体”多个 run，整行断言用拼接后的文本更稳妥。
fn runs_text(xml: &str) -> String {
    let re = Regex::new(r#"<w:t[^>]*>([^<]*)</w:t>"#).unwrap();
    re.captures_iter(xml)
        .map(|capture| capture[1].to_string())
        .collect()
}

fn gap_between<'a>(xml: &'a str, before: &str, after: &str) -> &'a str {
    let before_at = xml.find(before).unwrap();
    let before_end = xml[before_at..].find("</w:p>").unwrap() + before_at + "</w:p>".len();
    let after_at = xml.find(after).unwrap();
    let after_start = xml[..after_at].rfind("<w:p ").unwrap();
    &xml[before_end..after_start]
}

#[test]
fn meeting_agenda_docx_has_required_fonts_indents_and_numbering() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("agenda.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::MeetingAgenda;
    input.title_hint = "专题研商会议议程".into();
    input.profile.security_level = "机密".into();
    input.profile.security_period = "10年".into();
    let markdown = "# 专题研商会议议程\n\n一、时间地点：2026年8月5日（星期三）14:30，3C会议室。\n\n二、参加人员：张三同志、项目组成员。\n\n三、研讨内容：\n\n1. 汇报总体思路；\n2. 研究下一步工作。";

    write_docx_ok(&path, &input, markdown).unwrap();
    let document = zip_text(&path, "word/document.xml");
    let numberings = zip_text(&path, "word/numbering.xml");

    assert!(document.find("机密★").unwrap() < document.find("专题研商会议议程").unwrap());
    let security = paragraph_containing(&document, "机密★");
    assert!(security.contains("w:eastAsia=\"黑体\""));
    assert!(!security.contains("w:firstLine=\"640\""));
    let security_title_gap = gap_between(&document, "机密★", "专题研商会议议程");
    assert_eq!(
        security_title_gap.matches("<w:p ").count(),
        1,
        "{security_title_gap}"
    );
    assert!(!security_title_gap.contains("<w:t"));

    let title_body_gap = gap_between(&document, "专题研商会议议程", "一、时间地点：");
    assert_eq!(
        title_body_gap.matches("<w:p ").count(),
        1,
        "{title_body_gap}"
    );
    assert!(!title_body_gap.contains("<w:t"));

    // “一、时间地点”等三节在 TeX/预览里都是普通正文段：仿宋三号、首行缩进两字，
    // 标签不另设黑体。
    for label in ["一、时间地点：", "二、参加人员：", "三、研讨内容："] {
        let section = paragraph_containing(&document, label);
        assert!(section.contains("w:eastAsia=\"仿宋_GB2312\""), "{section}");
        assert!(!section.contains("w:eastAsia=\"黑体\""), "{section}");
        assert!(section.contains("w:firstLine=\"640\""), "{section}");
    }

    let first_item = paragraph_containing(&document, "汇报总体思路；");
    let second_item = paragraph_containing(&document, "研究下一步工作。");
    assert!(first_item.contains("<w:numPr>"));
    assert!(second_item.contains("<w:numPr>"));
    assert!(first_item.contains("w:firstLine=\"640\""));
    assert!(second_item.contains("w:firstLine=\"640\""));
    assert!(numberings.contains("w:val=\"%1.\""));
}

#[test]
fn parenthesized_body_uses_kaiti_and_smaller_size() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("parens.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "某单位".into();
    input.profile.recipient = "某部门".into();
    write_docx_ok(
        &path,
        &input,
        "# 测试函\n\n他说：\"**重要事项**\"。现就（有关事项）及【特别说明】函告如下，参见附表（三）。",
    )
    .unwrap();
    let xml = zip_text(&path, "word/document.xml");
    let paragraph = paragraph_containing(&xml, "现就");
    // 完整括号段用楷体_GB2312，字号为四号（28 半磅）；其余正文保持仿宋三号。
    assert!(
        paragraph.contains("w:eastAsia=\"楷体_GB2312\""),
        "{paragraph}"
    );
    assert!(
        paragraph.contains(&format!(r#"w:val="{PAREN_SIZE}""#)),
        "{paragraph}"
    );
    assert!(paragraph.contains("w:eastAsia=\"仿宋_GB2312\""));
    assert!(paragraph.contains(&format!(r#"w:val="{BODY_SIZE}""#)));
    assert!(paragraph.contains("“"));
    assert!(paragraph.contains("”"));
    assert!(!paragraph.contains("**"));
    assert!(
        paragraph.contains("<w:b"),
        "Markdown 加粗应生成加粗属性：{paragraph}"
    );
    assert!(paragraph.contains("【特别说明】"));
}

#[test]
fn special_handling_marks_after_security_level_in_black_font() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("special.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "某单位".into();
    input.profile.recipient = "某部门".into();
    input.profile.security_level = "秘密".into();
    input.profile.security_period = "10年".into();
    input.profile.special_handling = true;
    write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    let security = paragraph_containing(&xml, "秘密");
    let text = runs_text(security);
    assert!(security.contains("指人专办"));
    assert!(text.contains("★10年"));
    // “指人专办”位于“密级★保密期限”之后，且以黑体单独成 run。
    assert!(
        text.find("★10年").unwrap() < text.find("指人专办").unwrap(),
        "指人专办应排在保密期限之后：{text}"
    );
    assert!(security.contains("w:eastAsia=\"黑体\""));
    assert!(!security.contains("w:eastAsia=\"仿宋_GB2312\""));
}

#[test]
fn templates_with_security_line_use_heiti() {
    let temp = tempfile::tempdir().unwrap();
    for kind in TemplateKind::ALL {
        // 白头件目前未导出密级行；此处检查已有密级行的字体。
        if kind == TemplateKind::WhitePaper || !kind.uses_official_docx() {
            continue;
        }
        for period in ["10年", "长期"] {
            let path = temp.path().join("security-font.docx");
            let mut input = DraftInput::default();
            input.kind = kind;
            input.profile.security_level = "秘密".into();
            input.profile.security_period = period.into();
            input.profile.special_handling = false;
            write_docx_ok(&path, &input, "# 测试标题\n\n正文。").unwrap();
            let xml = zip_text(&path, "word/document.xml");
            let security = paragraph_containing(&xml, "秘密");
            assert!(runs_text(security).contains(&format!("秘密★{period}")));
            for run in security.split("<w:r>").skip(1) {
                let run = run.split("</w:r>").next().unwrap();
                assert!(run.contains("w:eastAsia=\"黑体\""), "{kind:?}: {run}");
                assert!(run.contains("w:ascii=\"黑体\""), "{kind:?}: {run}");
                assert!(run.contains("w:hAnsi=\"黑体\""), "{kind:?}: {run}");
                assert!(run.contains("<w:b "), "{kind:?}: {run}");
                assert!(
                    run.contains(&format!(
                        "w:val=\"{}\"",
                        if kind == TemplateKind::PhoneRecord {
                            24
                        } else {
                            BODY_SIZE
                        }
                    )),
                    "{kind:?}: {run}"
                );
            }
            assert!(!security.contains("仿宋_GB2312"), "{kind:?}: {security}");
        }
    }
}
#[test]
fn western_fonts_follow_chinese_fonts_in_exported_docx() {
    let temp = tempfile::tempdir().unwrap();
    let fonts_pattern = regex::Regex::new(r"<w:rFonts\b[^>]*/>").unwrap();
    let east_asia_pattern = regex::Regex::new(r#"w:eastAsia="([^"]+)""#).unwrap();
    for kind in TemplateKind::ALL {
        if !kind.uses_official_docx() {
            continue;
        }
        let path = temp.path().join("mixed-fonts.docx");
        let mut input = DraftInput::default();
        input.kind = kind;
        input.profile.issuing_unit = "某单位ABC".into();
        write_docx_ok(
            &path,
            &input,
            "# 测试标题ABC123\n\n## 测试小标题DEF456\n\n正文English123，café（说明Note456）。\n\n| 项目Item | 数量Count |\n| --- | --- |\n| 数据Data | 789 |",
        ).unwrap();
        let xml = zip_text(&path, "word/document.xml");
        let body = paragraph_containing(&xml, "正文English123");
        assert!(body.contains("w:ascii=\"仿宋_GB2312\""), "{kind:?}: {body}");
        assert!(body.contains("w:hAnsi=\"仿宋_GB2312\""), "{kind:?}: {body}");
        let mut archive = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut checked = 0;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            if !entry.name().ends_with(".xml") {
                continue;
            }
            let mut xml = String::new();
            entry.read_to_string(&mut xml).unwrap();
            for fonts in fonts_pattern.find_iter(&xml) {
                let fonts = fonts.as_str();
                let Some(captures) = east_asia_pattern.captures(fonts) else {
                    continue;
                };
                let font = &captures[1];
                assert!(
                    fonts.contains(&format!("w:ascii=\"{font}\"")),
                    "{kind:?}: {fonts}"
                );
                assert!(
                    fonts.contains(&format!("w:hAnsi=\"{font}\"")),
                    "{kind:?}: {fonts}"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "{kind:?}");
    }
}

#[test]
fn numeric_security_period_digits_use_heiti_font() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("security-mono.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "某单位".into();
    input.profile.recipient = "某部门".into();
    input.profile.security_level = "秘密".into();
    input.profile.security_period = "10年".into();
    write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    let security = paragraph_containing(&xml, "秘密");
    assert!(runs_text(security).contains("秘密★10年"));
    // 数字“10”单独成 run，中西文字体均为黑体。
    let digit_run = security
        .split("<w:r>")
        .find(|run| run.contains(">10<"))
        .unwrap();
    assert!(
        digit_run.contains("w:ascii=\"黑体\"") && digit_run.contains("w:hAnsi=\"黑体\""),
        "数字应使用黑体：{digit_run}"
    );
    assert!(
        security.contains("w:eastAsia=\"黑体\""),
        "“年”等其余部分应与 TeX 一致使用黑体"
    );

    // 非数字期限（“长期”）不使用等宽字体。
    input.profile.security_period = "长期".into();
    write_docx_ok(&path, &input, "# 测试函\n\n正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    assert!(!xml.contains("Courier New"), "“长期”不应使用等宽字体");
}

#[test]
fn meeting_agenda_special_handling_appends_to_heiti_security_line() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("agenda-special.docx");
    let mut input = DraftInput::default();
    input.kind = TemplateKind::MeetingAgenda;
    input.profile.security_level = "机密".into();
    input.profile.security_period = "10年".into();
    input.profile.special_handling = true;
    write_docx_ok(&path, &input, "# 专题会议议程\n\n正文。").unwrap();
    let xml = zip_text(&path, "word/document.xml");
    let security = paragraph_containing(&xml, "机密");
    let text = runs_text(security);
    // 会议议程密级行整行为黑体，“指人专办”跟在“密级★保密期限”之后。
    assert!(text.contains("机密★10年"));
    assert!(text.contains("指人专办"));
    assert!(
        text.find("★10年").unwrap() < text.find("指人专办").unwrap(),
        "指人专办应排在保密期限之后：{text}"
    );
    assert!(security.contains("w:eastAsia=\"黑体\""));
}

#[test]
fn document_title_compresses_small_overflow_and_wraps_large() {
    let temp = tempfile::tempdir().unwrap();
    let mut input = DraftInput::default();
    input.kind = TemplateKind::OfficialLetter;
    input.profile.issuing_unit = "某单位".into();
    input.profile.recipient = "某部门".into();

    // 22 字标题（超出一行 2 字）→ 字高不变（字号仍为二号），仅横向缩放字形。
    let compress_path = temp.path().join("compress.docx");
    let title_text = "关于认真做好网络安全与信息化重点工作验收的函";
    write_docx_ok(&compress_path, &input, &format!("# {title_text}\n\n正文。")).unwrap();
    let xml = zip_text(&compress_path, "word/document.xml");
    let title = paragraph_containing(&xml, title_text);
    assert!(
        title.contains(&format!(r#"w:sz w:val="{TITLE_SIZE}""#)),
        "字高应保持二号：{title}"
    );
    let scale = title::document_compressed_scale_percent(title_text, false);
    assert!(
        title.contains(&format!(r#"<w:w w:val="{scale}""#)),
        "仅横向缩放 {scale}%：{title}"
    );

    // 35 字标题 → jieba 换行，行间用文本换行符（不拆词）。
    let wrap_path = temp.path().join("wrap.docx");
    write_docx_ok(
        &wrap_path,
        &input,
        "# 关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知\n\n正文。",
    )
    .unwrap();
    let xml = zip_text(&wrap_path, "word/document.xml");
    let first_line = paragraph_containing(&xml, "关于转发国家互联网信息办公室");
    assert!(
        first_line.contains(r#"w:type="textWrapping""#),
        "长标题应换行：{first_line}"
    );
}
