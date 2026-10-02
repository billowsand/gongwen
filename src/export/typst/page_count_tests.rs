//! 页数口径与真实 PDF 对照，不以字数或屏幕预览估算。

use super::*;
use crate::models::TemplateKind;

#[test]
fn physical_page_count_matches_export_for_all_official_templates() {
    // 页数要与真实排版的 PDF 对得上，本机没有 runtime/fonts（CI）时跳过。
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let markdown = format!(
        "# 测试稿件\n\n{}\n<!-- [附件] -->\n# 附件\n\n附件正文。\n",
        "这是一段需要实际排版的正文，用来检查跨页和落款位置。\n\n".repeat(45)
    );
    for kind in [
        TemplateKind::OfficialLetter,
        TemplateKind::PhoneNotice,
        TemplateKind::WhitePaper,
        TemplateKind::RedHeadApproval,
        TemplateKind::PlainDocument,
        TemplateKind::MeetingAgenda,
    ] {
        let input = DraftInput {
            kind,
            ..Default::default()
        };
        let display = UnitDisplay::new(&[]);
        let fonts = FontConfig::default();
        let numbering = NumberingConfig::default();
        let count =
            page_count_with_base(&input, &markdown, &display, &fonts, &numbering, base.path())
                .unwrap();
        let outcome = write_pdf_with_base(
            &base.path().join("sample.pdf"),
            &input,
            &markdown,
            &display,
            &fonts,
            &numbering,
            &Default::default(),
            base.path(),
        )
        .unwrap();
        assert_eq!(
            count,
            crate::manuscript_io::send_package::page_count(&outcome.pdf).unwrap(),
            "{kind:?}"
        );
        assert!(count > 1, "{kind:?} 样稿必须跨页");
    }
}

#[test]
fn copy_numbered_manuscript_counts_one_copy_with_its_original_print_record() {
    // 同上：要真正排一遍 PDF 才能数份号稿的页数。
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let mut input = DraftInput::default();
    input.profile.number_copies = true;
    input.profile.recipient = "甲单位、乙单位、丙单位".into();
    let display = UnitDisplay::new(&[]);
    let fonts = FontConfig::default();
    let numbering = NumberingConfig::default();
    let markdown = "# 份号稿\n\n测试正文。\n";
    let count =
        page_count_with_base(&input, markdown, &display, &fonts, &numbering, base.path()).unwrap();
    let outcome = write_pdf_with_base(
        &base.path().join("copies.pdf"),
        &input,
        markdown,
        &display,
        &fonts,
        &numbering,
        &Default::default(),
        base.path(),
    )
    .unwrap();
    assert_eq!(
        crate::manuscript_io::send_package::page_count(&outcome.pdf).unwrap(),
        count * 3
    );
}
