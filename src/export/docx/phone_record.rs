//! 电话记录单：按来电记录模板排出五列表格，批示区留给人工填写。
use super::*;

fn label(text: &str) -> Paragraph {
    let mut paragraph = Paragraph::new();
    for (index, line) in text.lines().enumerate() {
        if index > 0 {
            paragraph = paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
        }
        paragraph = paragraph.add_run(body_run(line));
    }
    paragraph
        .line_spacing(
            LineSpacing::new()
                .line(440)
                .line_rule(LineSpacingType::AtLeast),
        )
        .widow_control(false)
}

fn value(text: &str, span: usize) -> TableCell {
    TableCell::new()
        .grid_span(span)
        .vertical_align(VAlignType::Center)
        .add_paragraph(
            Paragraph::new()
                .add_run(body_run(text).size(24))
                .line_spacing(
                    LineSpacing::new()
                        .line(440)
                        .line_rule(LineSpacingType::AtLeast),
                ),
        )
}

fn cell(text: &str, span: usize) -> TableCell {
    TableCell::new()
        .grid_span(span)
        .add_paragraph(label(text))
        .vertical_align(VAlignType::Center)
}

pub(super) fn write(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
    elements: &crate::visual_diff::ElementMarks,
) -> Result<()> {
    let r = &input.phone_record;
    let (level, period) = crate::export::element_display::security_parts(input);
    let mut security = Paragraph::new();
    for run in security_runs(level, period, "", "黑体", true, elements.security()) {
        security = security.add_run(run.size(24));
    }
    let (blocks, _) = parse_markdown_with_lines_with_numbering(markdown, numbering);
    let mut content = Docx::new();
    let mut counters = [0; 4];
    let mut in_attachment = false;
    for block in &blocks {
        if matches!(block, MarkdownBlock::Marker(MarkdownSection::Attachment)) {
            in_attachment = true;
        }
        if !in_attachment {
            content = if let MarkdownBlock::Table {
                rows,
                aligns,
                spans,
                numbered,
            } = block
            {
                content::add_smart_table_with_width(
                    content,
                    rows,
                    aligns,
                    spans,
                    *numbered,
                    fonts.bold_family_docx(),
                    5970,
                )
            } else {
                add_official_content_block(
                    content,
                    block,
                    &mut counters,
                    numbering,
                    fonts.bold_family_docx(),
                )
            };
        }
    }
    let mut body = TableCell::new();
    for child in content.document.children {
        body = match child {
            DocumentChild::Paragraph(p) => body.add_paragraph(*p),
            DocumentChild::Table(t) => body.add_table(*t),
            _ => body,
        };
    }
    let left = Table::new(vec![
        TableRow::new(vec![body]).row_height(7100.0),
        TableRow::new(vec![
            cell(&format!("建议：{}", r.suggestion), 1).vertical_align(VAlignType::Bottom),
        ])
        .row_height(1500.0),
    ])
    .set_grid(vec![6186])
    .width(6186, WidthType::Dxa)
    .clear_all_border();
    let body = TableCell::new()
        .grid_span(4)
        .add_table(left)
        .add_paragraph(Paragraph::new());
    let rows = vec![
        TableRow::new(vec![
            cell("单位", 1),
            value(&r.caller_unit, 1),
            cell("电话", 1),
            value(&r.caller_phone, 1),
            TableCell::new()
                .add_paragraph(
                    Paragraph::new()
                        .add_run(title_run("首长批示", TITLE_SIZE))
                        .align(AlignmentType::Center),
                )
                .vertical_align(VAlignType::Center)
                .vertical_merge(VMergeType::Restart),
        ])
        .row_height(680.0),
        TableRow::new(vec![
            cell("谈话人", 1),
            value(&r.caller_person, 1),
            cell("密级", 1),
            TableCell::new()
                .add_paragraph(security)
                .vertical_align(VAlignType::Center),
            cell("", 1).vertical_merge(VMergeType::Continue),
        ])
        .row_height(590.0),
        TableRow::new(vec![
            cell("时间", 1),
            value(&r.call_time, 3),
            cell("", 1).vertical_merge(VMergeType::Restart),
        ])
        .row_height(590.0),
        // 最小行高保留手写空间；长来电内容可跨页，不裁剪。
        TableRow::new(vec![body, cell("", 1).vertical_merge(VMergeType::Continue)])
            .row_height(8_600.0),
    ];
    let table = Table::new(rows)
        .set_grid(vec![1382, 2294, 1161, 1575, 2432])
        .width(8844, WidthType::Dxa)
        .layout(TableLayoutType::Fixed)
        .set_borders(
            TableBorders::new()
                .clear_all()
                .set(TableBorder::new(TableBorderPosition::Top).size(4))
                .set(TableBorder::new(TableBorderPosition::Bottom).size(4))
                .set(TableBorder::new(TableBorderPosition::InsideH).size(4))
                .set(TableBorder::new(TableBorderPosition::InsideV).size(4)),
        );
    let mut doc = Docx::new()
        .page_size(11907, 16840)
        .page_margin(
            PageMargin::new()
                .top(2098)
                .bottom(1984)
                .left(1587)
                .right(1474),
        )
        .default_fonts(chinese_fonts("仿宋_GB2312"))
        .default_size(BODY_SIZE)
        .add_paragraph(
            Paragraph::new()
                .add_run(title_run(&r.institution, TITLE_SIZE).color("FF0000"))
                .align(AlignmentType::Center),
        )
        .add_paragraph(
            Paragraph::new()
                .add_run(title_run("电话记录单", TITLE_SIZE).color("FF0000"))
                .align(AlignmentType::Center)
                .line_spacing(LineSpacing::new().after(260)),
        )
        .add_table(table)
        .add_paragraph(
            Paragraph::new()
                .line_spacing(
                    LineSpacing::new()
                        .line(400)
                        .line_rule(LineSpacingType::Exact),
                )
                .add_run(
                    body_run(format!(
                        "承办单位：{}　联系人：{}　电话：{}",
                        input.profile.responsible_unit,
                        input.profile.contact_person,
                        input.profile.contact_phone
                    ))
                    .size(28),
                ),
        );
    in_attachment = false;
    for block in &blocks {
        if matches!(block, MarkdownBlock::Marker(MarkdownSection::Attachment)) {
            in_attachment = true;
            counters = [0; 4];
            doc = doc.add_paragraph(Paragraph::new().page_break_before(true));
        } else if in_attachment {
            if let MarkdownBlock::Title(t) = block {
                doc = doc.add_paragraph(document_title_paragraph(t, &title::title_plan(t, 14)));
            } else {
                doc = add_official_content_block(
                    doc,
                    block,
                    &mut counters,
                    numbering,
                    fonts.bold_family_docx(),
                );
            }
        }
    }
    doc.build()
        .pack(File::create(path)?)
        .context("写入电话记录单 Word 失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn phone_record_docx_keeps_incoming_fields_and_suggestion_in_left_cell() {
        let input = DraftInput {
            kind: TemplateKind::PhoneRecord,
            phone_record: crate::models::PhoneRecordMetadata {
                institution: "本单位".into(),
                caller_unit: "来电单位甲".into(),
                caller_phone: "010-87654321".into(),
                caller_person: "王某　主任".into(),
                call_time: "2026年10月8日14时30分".into(),
                suggestion: "请办公室办理。".into(),
            },
            profile: crate::models::TemplateProfile {
                issuing_unit: "旧发文机关".into(),
                recipient: "旧主送单位".into(),
                signing_unit: "旧落款单位".into(),
                responsible_unit: "办公室".into(),
                contact_person: "李某".into(),
                contact_phone: "010-12345678".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.docx");
        write_docx(&path, &input, "# 电话记录单\n<!-- [正文] -->\n来电请报送材料。\n\n| 事项 | 时间 |\n| --- | --- |\n| 报送 | 10月9日 |", &UnitDisplay::new(&[])).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        for text in [
            "电话记录单",
            "来电单位甲",
            "010-87654321",
            "王某",
            "2026年10月8日14时30分",
            "首长批示",
            "请办公室办理。",
            "来电请报送材料。",
            "办公室",
            "李某",
        ] {
            assert!(xml.contains(text), "记录字段丢失：{text}");
        }
        for text in ["旧发文机关", "旧主送单位", "旧落款单位"] {
            assert!(!xml.contains(text), "不得输出发文要素：{text}");
        }
        // 建议在正文单元格内，后面才是空的右侧批示格；窄栏表格不得用整页列宽。
        assert!(xml.contains("w:val=\"4\""));
        assert!(xml.contains("w:w=\"5970\""));
        assert_eq!(xml.matches("请办公室办理。").count(), 1);
    }
}
