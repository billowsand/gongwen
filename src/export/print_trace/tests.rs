//! 可解码性、叠加后的原文保护与真实样张。
use super::*;

#[test]
fn print_trace_barcode_is_rectangular_and_round_trips() {
    let code = "1234567890123456";
    let matrix = DataMatrix::encode(code.as_bytes(), SymbolSize::Rect8x32).unwrap();
    let bitmap = matrix.bitmap();
    assert_eq!((bitmap.width(), bitmap.height()), (32, 8));
    assert_eq!(
        DataMatrix::decode(bitmap.bits(), 32).unwrap(),
        code.as_bytes()
    );
    let svg = String::from_utf8(barcode_svg(code).unwrap()).unwrap();
    assert!(svg.contains("viewBox=\"0 0 34 10\""));
    assert!(barcode_svg("ABC").is_err());
}

#[test]
fn print_trace_settings_default_off_and_require_an_explicit_unit() {
    let config: crate::models::ExportSelection = serde_json::from_str("{}").unwrap();
    assert!(!config.print_trace.enabled);
    let mut trace = PrintTraceConfig {
        enabled: true,
        unit: String::new(),
    };
    assert!(trace.validate().is_err());
    trace.unit = "示例办公室".into();
    assert!(trace.validate().is_ok());
    trace.unit.push('\n');
    assert!(trace.validate().is_ok()); // 两端空白会统一去掉。
    trace.unit = "示例\n办公室".into();
    assert!(trace.validate().is_err());
}

#[test]
fn print_trace_overlay_preserves_pages_and_existing_streams() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let pdf = crate::typst_engine::compile_source_for_test(
        "#set page(paper: \"a4\")\nOriginal page one\n#pagebreak()\nOriginal page two",
    )
    .unwrap();
    let source = Document::load_mem(&pdf).unwrap();
    let config = PrintTraceConfig {
        enabled: true,
        unit: "示例办公室".into(),
    };
    let result = stamp_pdf(&pdf, &config, "1234567890123456").unwrap();
    let stamped = Document::load_mem(&result).unwrap();
    assert_eq!(source.get_pages().len(), stamped.get_pages().len());
    for (page, original_id) in source.get_pages() {
        let stamped_id = stamped.get_pages()[&page];
        assert_eq!(
            source
                .get_dictionary(original_id)
                .unwrap()
                .get(b"MediaBox")
                .unwrap(),
            stamped
                .get_dictionary(stamped_id)
                .unwrap()
                .get(b"MediaBox")
                .unwrap()
        );
        let before = source.get_page_content(original_id);
        let after = stamped.get_page_content(stamped_id);
        assert!(after.windows(before.len()).any(|part| part == before));
        assert!(
            stamped
                .get_page_resources(stamped_id)
                .unwrap()
                .0
                .unwrap()
                .get(b"XObject")
                .unwrap()
                .as_dict()
                .unwrap()
                .has(b"GWPrintTrace")
        );
    }
    assert!(stamp_pdf(&result, &config, "1234567890123456").is_err());
}

#[test]
#[ignore = "依赖随包字体，生成供人工检查的真实 PDF 样张"]
fn typst_samples_print_trace() {
    use crate::models::{FontConfig, NumberingConfig, TemplateKind, TemplateProfile};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("output/pdf/print-trace");
    std::fs::create_dir_all(&root).unwrap();
    let mut input = DraftInput {
        kind: TemplateKind::PlainDocument,
        profile: TemplateProfile::for_kind(TemplateKind::PlainDocument),
        date: "2026年11月6日".into(),
        ..Default::default()
    };
    input.profile.signing_unit = "示例办公室".into();
    input.profile.security_level.clear();
    input.profile.security_period.clear();
    let markdown = "# 关于开展年度工作总结的通知\n\n<!-- [正文] -->\n\n为全面梳理本年度工作情况，总结经验做法，明确下一步工作任务，现就开展年度工作总结有关事项通知如下。\n\n## 工作目的\n\n全面总结本年度工作成效，分析存在的不足，研究提出下一年度工作安排，为推动各项工作高质量开展奠定基础。\n\n## 主要内容\n\n重点围绕年度目标任务完成情况、重点工作推进情况、工作亮点和成效、存在的问题及下一年度工作思路等方面进行总结。\n\n## 工作要求\n\n各部门要高度重视，精心组织，统筹做好材料汇总。总结内容应实事求是、简明扼要，做到数据准确、重点突出、文字精炼。\n\n请各部门按照规定时间完成总结材料，认真审核后报送。\n";
    let outcome = crate::export::typst::compile_pdf_with_base(
        &input,
        markdown,
        &crate::units::UnitDisplay::new(&[]),
        &FontConfig::default(),
        &NumberingConfig::default(),
        &Default::default(),
        &root,
    )
    .unwrap();
    let config = PrintTraceConfig {
        enabled: true,
        unit: "示例办公室".into(),
    };
    let bytes = stamp_pdf(&outcome.pdf, &config, "1234567890123456").unwrap();
    std::fs::write(root.join("印制追溯码-真实样张.pdf"), &bytes).unwrap();
    std::fs::write(root.join("无追溯码-对照.pdf"), &outcome.pdf).unwrap();
    // 把实际排出的码模块也留作机器校验夹具。
    let matrix = DataMatrix::encode(b"1234567890123456", SymbolSize::Rect8x32).unwrap();
    let bitmap = matrix.bitmap();
    assert_eq!(
        DataMatrix::decode(bitmap.bits(), bitmap.width()).unwrap(),
        b"1234567890123456"
    );
    assert_eq!(
        Document::load_mem(&bytes).unwrap().get_pages().len(),
        outcome.pages
    );
    println!("真实样张：{}（{} 页）", root.display(), outcome.pages);
}

/// 从最终 PDF 的实际渲染像素取回模块，再解码；防止只验证编码器却漏掉缩放、裁切或旋转。
fn decode_rendered_trace(pdf: &[u8]) -> Vec<u8> {
    use hayro::hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf};
    use hayro::{RenderCache, RenderSettings};
    let document = Pdf::new(std::sync::Arc::new(pdf.to_vec())).unwrap();
    let page = &document.pages()[0];
    let scale = 300.0 / 72.0;
    let (width_pt, height_pt) = page.render_dimensions();
    let image = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
            ..Default::default()
        },
    );
    let mm = 72.0 / 25.4;
    let left = width_pt - 34.0 * mm;
    let top = height_pt - 18.0 * mm;
    let bytes = image.data_as_u8_slice();
    let mut bits = Vec::new();
    for row in 0..8 {
        for col in 0..32 {
            let x = ((left + (col as f32 + 1.5) * 0.65 * mm) * scale) as usize;
            let y = ((top + (row as f32 + 1.5) * 0.65 * mm) * scale) as usize;
            let offset = (y * usize::from(image.width()) + x) * 4;
            bits.push(bytes[offset] < 128);
        }
    }
    DataMatrix::decode(&bits, 32).expect("最终 PDF 渲染后的码必须可解码")
}

#[test]
fn print_trace_rendered_portrait_and_rotated_landscape_codes_are_readable() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let source =
        crate::typst_engine::compile_source_for_test("#set page(paper: \"a4\")\nTest").unwrap();
    let config = PrintTraceConfig {
        enabled: true,
        unit: "示例办公室".into(),
    };
    for rotation in [0, 90, 180, 270] {
        let mut document = Document::load_mem(&source).unwrap();
        let page_id = document.get_pages()[&1];
        document
            .get_dictionary_mut(page_id)
            .unwrap()
            .set("Rotate", rotation);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        let stamped = stamp_pdf(&bytes, &config, "1234567890123456").unwrap();
        assert_eq!(
            decode_rendered_trace(&stamped),
            b"1234567890123456",
            "旋转 {rotation} 度"
        );
    }
}
