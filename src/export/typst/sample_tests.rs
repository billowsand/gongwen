//! 公文 Typst 样张与回归测试。
//!
//! 样张：各文种（含附件、横页、联合发文、份号、花脸稿）排一遍，PDF 与模板数据落到
//! `tmp/typst-samples/<用例>/`，改模板后目视检查用。依赖本机随包字体，默认忽略：
//!
//! ```text
//! cargo test --locked typst_samples -- --ignored --nocapture
//! ```
//!
//! 其余几条（每个文种都排得出、插图、生僻字、专用粗体）不依赖外部文件，常规运行。

use std::path::{Path, PathBuf};

use crate::models::{
    DraftInput, FontConfig, JointContact, JointIssuanceMode, LetterVersion, NumberingConfig,
    StyleMode, TemplateKind, TemplateProfile,
};
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;

struct Case {
    name: &'static str,
    kind: TemplateKind,
    markdown: String,
    tweak: fn(&mut DraftInput),
}

fn base_input(kind: TemplateKind) -> DraftInput {
    let mut input = DraftInput {
        kind,
        profile: TemplateProfile::for_kind(kind),
        ..Default::default()
    };
    let p = &mut input.profile;
    p.issuing_unit = "某某市民政局".into();
    p.department_code = "某民函".into();
    p.document_year = "2026".into();
    p.document_number = "18".into();
    p.recipient = "各区民政局".into();
    p.copies_to = "市财政局".into();
    p.responsible_unit = "养老服务处".into();
    p.contact_person = "张三".into();
    p.contact_phone = "010-12345678".into();
    p.reporting_leaders = "王局长".into();
    p.security_level = "秘密".into();
    p.security_period = "1年".into();
    input.date = "2026年9月30日".into();
    input
}

const BODY: &str = "# 关于加快推进全市社区养老服务设施提质改造工作的通知

<!-- [正文] -->

## 工作目标

今年以来，我局会同各区民政部门对全市社区养老服务设施开展了全面摸排，**共排查社区养老服务站点312个**，其中设施老化、功能不全、专业服务人员不足的站点占比较高，难以满足老年人日益增长的多样化服务需求（摸排数据截至2026年8月31日）。

### 总体要求

到2026年底，全市社区养老服务设施全部完成提质改造，服务能力明显提升。

## 工作措施

经测算，完成全部站点提质改造共需专项经费860万元，其中设施适老化改造520万元，护理人员培训140万元，智慧养老服务平台建设200万元。

1. 各区于3月底前完成辖区站点摸底；
2. 4月至9月集中实施改造；
3. 10月起组织第三方验收评估。
";

const SHORT: &str = "# 关于申请拨付社区养老服务专项经费的请示

<!-- [正文] -->

妥否，请批示。
";

const WITH_ATTACHMENT: &str = "# 关于印发社区养老服务设施改造任务分解表的通知

<!-- [正文] -->

现将任务分解表印发给你们，请认真贯彻落实。

<!-- [附件] -->

# 社区养老服务设施改造任务分解表

| 序号 | 区域 | 改造内容 | 完成时限 |
| --- | --- | --- | --- |
| 1 | 东城区 | 无障碍通道、扶手、呼叫系统 | 2026年6月 |
| 2 | 西城区 | 助浴间、适老化卫生间 | 2026年8月 |
";

const LANDSCAPE: &str = "# 关于印发提质改造任务分解表的通知

<!-- [正文] -->

<!-- [居中] -->
**附表说明**

现将任务分解表印发给你们。

<!-- [附件] -->

# 全市社区养老服务设施提质改造任务分解表及各区责任单位联系人名单

| 序号 | 区域 | 站点 | 改造内容 | 设施适老化 | 人员配备 | 智慧平台 | 资金来源 | 责任单位 | 联系人 | 完成时限 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 东城区 | 东华门站 | 无障碍通道、扶手、呼叫系统 | 是 | 4人 | 接入 | 市级补助 | 东城区民政局 | 张三 | 2026年6月 |
| 2 | 西城区 | 月坛站 | 助浴间、适老化卫生间 | 是 | 3人 | 接入 | 区级配套 | 西城区民政局 | 李四 | 2026年8月 |
";

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "letter",
            kind: TemplateKind::OfficialLetter,
            markdown: BODY.into(),
            tweak: |_| {},
        },
        Case {
            name: "letter-attachment",
            kind: TemplateKind::OfficialLetter,
            markdown: WITH_ATTACHMENT.into(),
            tweak: |_| {},
        },
        Case {
            name: "letter-preview-duplex",
            kind: TemplateKind::OfficialLetter,
            markdown: format!("{BODY}\n{}", BODY.lines().skip(4).collect::<Vec<_>>().join("\n")),
            tweak: |input| {
                input.profile.letter_version = LetterVersion::Preview;
                input.profile.duplex_printing = true;
                input.profile.special_handling = true;
            },
        },
        Case {
            name: "letter-joint",
            kind: TemplateKind::OfficialLetter,
            markdown: SHORT.replace("请示", "通知").replace("妥否，请批示。", "请遵照执行。"),
            tweak: |input| {
                let p = &mut input.profile;
                p.joint_issuance_mode = JointIssuanceMode::Mode1;
                p.joint_issuing_units = "某某市民政局、某某市财政局、某某市卫生健康委员会".into();
                p.main_issuing_unit = "某某市民政局".into();
                p.joint_responsible_units = "养老服务处、社会保障处".into();
                p.joint_contacts = vec![
                    JointContact {
                        unit: "养老服务处".into(),
                        name: "张三".into(),
                        phone: "010-12345678".into(),
                    },
                    JointContact {
                        unit: "社会保障处".into(),
                        name: "欧阳明远".into(),
                        phone: "010-87654321".into(),
                    },
                ];
            },
        },
        Case {
            name: "letter-copies",
            kind: TemplateKind::OfficialLetter,
            markdown: SHORT.replace("请示", "通知").replace("妥否，请批示。", "请遵照执行。"),
            tweak: |input| input.profile.number_copies = true,
        },
        Case {
            name: "phone",
            kind: TemplateKind::PhoneNotice,
            markdown: BODY.into(),
            tweak: |_| {},
        },
        Case {
            name: "plain",
            kind: TemplateKind::PlainDocument,
            markdown: WITH_ATTACHMENT.into(),
            tweak: |input| input.profile.style_mode = StyleMode::Compact,
        },
        Case {
            name: "plain-landscape",
            kind: TemplateKind::PlainDocument,
            markdown: LANDSCAPE.into(),
            tweak: |input| input.profile.duplex_printing = true,
        },
        Case {
            name: "whitepaper",
            kind: TemplateKind::WhitePaper,
            markdown: BODY.into(),
            tweak: |_| {},
        },
        Case {
            name: "redapproval",
            kind: TemplateKind::RedHeadApproval,
            markdown: BODY.into(),
            tweak: |_| {},
        },
        Case {
            name: "redapproval-short",
            kind: TemplateKind::RedHeadApproval,
            markdown: SHORT.into(),
            tweak: |_| {},
        },
        Case {
            name: "agenda",
            kind: TemplateKind::MeetingAgenda,
            markdown: "# 社区养老服务工作推进会议程\n\n<!-- [正文] -->\n\n## 时间地点\n\n2026年8月5日（星期三）14:30，3C会议室。\n\n## 会议议程\n\n1. 张三同志通报工作进展；\n2. 各区汇报；\n3. 王局长讲话。\n".into(),
            tweak: |_| {},
        },
    ]
}

fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/typst-samples")
}

/// 每个文种都能用 Typst 排出 PDF、没有警告、带回探针报告。
/// 本机没有 runtime/fonts 时跳过（CI）。
#[test]
fn every_kind_compiles_with_typst() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let fonts = FontConfig::default();
    let display = UnitDisplay::new(&[]);
    let dir = tempfile::tempdir().unwrap();
    for case in cases() {
        let mut input = base_input(case.kind);
        (case.tweak)(&mut input);
        let path = dir.path().join(format!("{}.pdf", case.name));
        let outcome = super::write_pdf_with_base(
            &path,
            &input,
            &case.markdown,
            &display,
            &fonts,
            &NumberingConfig::default(),
            &ElementMarks::default(),
            dir.path(),
        )
        .unwrap_or_else(|error| panic!("{} 排版失败：{error:#}", case.name));
        assert!(outcome.pdf.starts_with(b"%PDF"), "{}", case.name);
        assert!(
            outcome.warnings.is_empty(),
            "{} 有警告：{:?}",
            case.name,
            outcome.warnings
        );
        assert!(outcome.proof.is_some(), "{} 没有探针报告", case.name);
    }
}

/// 各文种样张：PDF 与模板数据写到 `tmp/typst-samples/<用例>/`。
#[test]
#[ignore = "依赖本机 runtime 字体，手动运行"]
fn typst_samples() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let fonts = FontConfig::default();
    let display = UnitDisplay::new(&[]);
    let numbering = NumberingConfig::default();
    for case in cases() {
        let dir = sample_dir().join(case.name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut input = base_input(case.kind);
        (case.tweak)(&mut input);
        let data = super::document_json(
            &input,
            &case.markdown,
            &display,
            &numbering,
            &ElementMarks::default(),
            crate::typst_engine::font_set(&fonts).unwrap().families,
        )
        .unwrap();
        std::fs::write(dir.join("doc.json"), data).unwrap();
        let outcome = super::write_pdf_with_base(
            &dir.join("typst.pdf"),
            &input,
            &case.markdown,
            &display,
            &fonts,
            &NumberingConfig::default(),
            &ElementMarks::default(),
            &dir,
        );
        if let Err(error) = outcome {
            eprintln!("{}: {error:#}", case.name);
        }
    }
}

/// 花脸稿样张：正文增删、标题新增、表格改数、要素（主送、成文日期）变更。
#[test]
#[ignore = "依赖本机 runtime 字体，手动运行"]
fn typst_samples_redline() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let old_md = "# 关于开展社区养老服务设施专项检查的函\n\n<!-- [正文] -->\n\n## 检查范围\n\n全市社区养老服务站点，重点检查消防设施和用电安全。\n\n## 时间安排\n\n9月底前完成自查。\n\n| 区域 | 站点数 |\n| --- | ---: |\n| 东城区 | 120 |\n| 西城区 | 80 |\n";
    let new_md = "# 关于开展社区养老服务设施专项检查的函\n\n<!-- [正文] -->\n\n## 检查范围\n\n全市社区养老服务站点和日间照料中心，重点检查消防设施、食品安全和用电安全。\n\n## 时间安排\n\n10月中旬前完成自查，10月底前完成抽查。\n\n## 工作要求\n\n各区要高度重视。\n\n| 区域 | 站点数 |\n| --- | ---: |\n| 东城区 | 126 |\n| 西城区 | 80 |\n";
    let old_input = base_input(TemplateKind::OfficialLetter);
    let mut new_input = base_input(TemplateKind::OfficialLetter);
    new_input.profile.recipient = "各区民政局、各街道办事处".into();
    new_input.date = "2026年10月8日".into();
    let display = UnitDisplay::new(&[]);
    let doc = crate::redline::build_with_inputs(old_md, new_md, &old_input, &new_input, &display);
    let fonts = FontConfig::default();
    let numbering = NumberingConfig::default();
    let dir = sample_dir().join("redline");
    std::fs::create_dir_all(&dir).unwrap();
    super::write_pdf_with_base(
        &dir.join("typst.pdf"),
        &new_input,
        &doc.markdown,
        &display,
        &fonts,
        &numbering,
        &doc.elements,
        &dir,
    )
    .unwrap();
}

fn compile_plain(markdown: &str, fonts: &FontConfig, base: &Path) -> super::TypstOutcome {
    let input = base_input(TemplateKind::PlainDocument);
    super::write_pdf_with_base(
        &base.join("out.pdf"),
        &input,
        markdown,
        &UnitDisplay::new(&[]),
        fonts,
        &NumberingConfig::default(),
        &ElementMarks::default(),
        base,
    )
    .unwrap()
}

/// PDF 里嵌入的全部字体名（BaseFont，去掉子集前缀）。
fn embedded_fonts(pdf: &[u8]) -> Vec<String> {
    let doc = lopdf::Document::load_mem(pdf).unwrap();
    doc.objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .filter(|dict| dict.get(b"Type").and_then(|t| t.as_name()).ok() == Some(b"Font"))
        .filter_map(|dict| dict.get(b"BaseFont").and_then(|n| n.as_name()).ok())
        .map(|name| {
            let name = String::from_utf8_lossy(name).to_string();
            name.split_once('+')
                .map_or(name.clone(), |(_, rest)| rest.to_string())
        })
        .collect()
}

/// 插图：按基准目录解析、原样嵌入；文件缺失时略过并给出提示，不让整份排版失败。
#[test]
fn images_are_embedded_and_missing_ones_are_skipped() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("images")).unwrap();
    let png = image::RgbImage::from_pixel(40, 20, image::Rgb([31, 78, 158]));
    png.save(dir.path().join("images/chart.png")).unwrap();
    let markdown = "# 标题\n\n<!-- [正文] -->\n\n见下图。\n\n![图](images/chart.png)\n\n![缺](images/missing.png)\n";
    let outcome = compile_plain(markdown, &FontConfig::default(), dir.path());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("images/missing.png")),
        "{:?}",
        outcome.warnings
    );
    assert!(
        String::from_utf8_lossy(&outcome.pdf).contains("/Image"),
        "PDF 里应有插图"
    );
}

/// 仿宋_GB2312 没有的 GBK 字（人名里的「喆」）落到兜底宋体，不丢字。
#[test]
fn rare_characters_fall_back_to_simsun() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let uses_simsun = |name: &str| {
        let outcome = compile_plain(
            &format!("# 标题\n\n<!-- [正文] -->\n\n联系人{name}。\n"),
            &FontConfig::default(),
            dir.path(),
        );
        embedded_fonts(&outcome.pdf)
            .iter()
            .any(|f| f.contains("SimSun"))
    };
    // 对照：同一份稿子换成仿宋里有的字，宋体就不该出现。
    assert!(!uses_simsun("王哲"), "仿宋有的字不应落到宋体");
    assert!(uses_simsun("王喆"), "「喆」应落到兜底宋体");
}

/// 选了「专用粗体字体」：加粗字换成粗体字面（内置回落黑体），不再描边。
#[test]
fn dedicated_bold_font_replaces_fake_bold() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let fonts = FontConfig {
        bold_style: crate::models::BoldStyle::DedicatedFont,
        ..FontConfig::default()
    };
    let set = crate::typst_engine::font_set(&fonts).unwrap();
    assert_eq!(set.families.bold.as_deref(), Some("SimHei"));
    let outcome = compile_plain(
        "# 标题\n\n<!-- [正文] -->\n\n**加粗**正文。\n",
        &fonts,
        dir.path(),
    );
    let fonts = embedded_fonts(&outcome.pdf);
    assert!(fonts.iter().any(|f| f.contains("SimHei")), "{fonts:?}");
}

/// 标题压缩 / 换行、全局紧缩 / 节内紧缩的样张。
#[test]
#[ignore = "依赖本机 runtime，手动运行"]
fn typst_samples_title_compact() {
    if crate::portable_runtime::find_font_dir().is_none() {
        return;
    }
    let cases = [
        (
            "title-compressed",
            "# 关于加强全市社区养老服务设施安全管理的通知\n\n<!-- [正文] -->\n\n## 总体要求\n\n### 压实责任\n各区要落实属地责任。\n\n### 排查隐患\n\n全面排查消防、用电等隐患。\n\n## 工作措施\n\n### 建立台账\n逐站建立台账，限期整改到位。\n",
            StyleMode::SectionCompact,
        ),
        (
            "title-wrapped",
            "# 关于进一步加强全市基层社区养老服务设施安全管理工作的通知\n\n<!-- [正文] -->\n\n## 工作目标\n\n#### 摸底排查\n各区于3月底前完成摸底。\n\n#### 集中改造\n\n4月至9月集中实施改造。\n",
            StyleMode::Compact,
        ),
    ];
    let fonts = FontConfig::default();
    let display = UnitDisplay::new(&[]);
    let numbering = NumberingConfig::default();
    for (name, markdown, style) in cases {
        let dir = sample_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut input = base_input(TemplateKind::OfficialLetter);
        input.profile.style_mode = style;
        super::write_pdf_with_base(
            &dir.join("typst.pdf"),
            &input,
            markdown,
            &display,
            &fonts,
            &numbering,
            &ElementMarks::default(),
            &dir,
        )
        .unwrap();
    }
}

/// 用真实稿件出样张：`GW_SAMPLE_DOCS` 指向一个目录，里面每份稿件一对 `<名>.md` +
/// `<名>.json`（稿件库的 content_markdown 与 snapshot_json）；可选 `GW_SAMPLE_KIND`
/// 覆盖文种（如 RedHeadApproval）。结果写到 tmp/typst-samples/doc-<名>/。
#[test]
#[ignore = "依赖本机 runtime 与外部稿件，手动运行"]
fn typst_samples_real_docs() {
    let Ok(src) = std::env::var("GW_SAMPLE_DOCS") else {
        return;
    };
    let fonts = FontConfig::default();
    let display = UnitDisplay::new(&[]);
    let numbering = NumberingConfig::default();
    for entry in std::fs::read_dir(&src).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let markdown = std::fs::read_to_string(&path).unwrap();
        let mut input: DraftInput =
            serde_json::from_str(&std::fs::read_to_string(path.with_extension("json")).unwrap())
                .unwrap();
        if let Ok(kind) = std::env::var("GW_SAMPLE_KIND") {
            input.kind = serde_json::from_str(&format!("\"{kind}\"")).unwrap();
            input.profile.kind = input.kind;
        }
        let dir = sample_dir().join(format!("doc-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        crate::export::write_pdf(
            &dir.join("typst.pdf"),
            &input,
            &markdown,
            &display,
            &fonts,
            &numbering,
            &ElementMarks::default(),
        )
        .unwrap();
        eprintln!(
            "{name}: {:?} style {:?}",
            input.kind, input.profile.style_mode
        );
    }
}

/// 发布前冒烟：用随包 runtime（`GONGWEN_RUNTIME_DIR`）把每个文种连同研究报告都排一遍。
/// 与上面那条不同，找不到字体就算失败——发布流程拿到不完整的 runtime 要在这里停下。
#[test]
#[ignore = "发布流程用：需要完整的随包 runtime"]
fn shipped_runtime_typesets_every_kind() {
    let fonts_dir = crate::portable_runtime::find_font_dir().expect("找不到随包字体 runtime/fonts");
    crate::portable_runtime::validate_research_fonts(&fonts_dir).expect("研究报告字体不全");
    let fonts = FontConfig::default();
    let display = UnitDisplay::new(&[]);
    let dir = tempfile::tempdir().unwrap();
    for case in cases() {
        let mut input = base_input(case.kind);
        (case.tweak)(&mut input);
        crate::export::write_pdf(
            &dir.path().join(format!("{}.pdf", case.name)),
            &input,
            &case.markdown,
            &display,
            &fonts,
            &NumberingConfig::default(),
            &ElementMarks::default(),
        )
        .unwrap_or_else(|error| panic!("{} 排版失败：{error:#}", case.name));
    }
    let mut research = DraftInput {
        kind: TemplateKind::ResearchReport,
        title_hint: "冒烟测试报告".into(),
        ..Default::default()
    };
    research.research.institution = "测试单位".into();
    let outcome = crate::export::write_pdf(
        &dir.path().join("research.pdf"),
        &research,
        "<!-- [摘要] -->\n\n摘要。\n\n<!-- [目录] -->\n\n<!-- [正文] -->\n\n## 研究背景\n\n正文，$\\sum_{i=1}^{n} x_i$。\n\n$$\\begin{cases} 1 & x > 0 \\\\ 0 & x \\le 0 \\end{cases}$$\n",
        &display,
        &fonts,
        &NumberingConfig::default(),
        &ElementMarks::default(),
    )
    .expect("研究报告排版失败");
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}
