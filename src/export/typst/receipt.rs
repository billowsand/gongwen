//! 公函签收记录单：两端共用的数据构造与 PDF 排版入口。
//!
//! 签收记录单是公函的附属件，与原公函**分开成文件**（`<主干名>_签收记录单.pdf` 与
//! `<主干名>_签收记录单.docx`），绝不拼进公函正文。版面按已确认的设计：A4 纵向、
//! 黑白、156mm 版心；题名小标宋二号居中；表格五列（序号 / 签收单位 / 签收方式 /
//! 签收人 / 签收日期），表头黑体四号、内容仿宋四号，外框稍粗、内部细线；
//! 跨页重复表头、行不拆分、有页码。密级只在表格上方的要素行印一次，不逐行重复。
//!
//! 数据构造放在本模块，Word（`export::docx::receipt`）与 PDF（这里）读同一份：
//! 要素显示值、签收单位清单的顺序与去重规则、表格列宽都不会各说各话。

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::export::element_display;
use crate::models::{DraftInput, FontConfig, JointIssuanceMode, split_units};
use crate::typst_engine::{self, FontFamilies, TypstJob, TypstOutcome};
use crate::units::UnitDisplay;

/// 签收记录单的题名（固定）。
pub(crate) const RECEIPT_TITLE: &str = "公文签收记录单";

/// 签收表列宽（毫米）。合计正好一个版心（156mm），Word 与 PDF 共用同一组宽度。
/// 签收人与签收日期是手写栏，给得最宽：签名约需 35mm，「2026年10月29日」约需 28mm；
/// 签收方式的两个勾选项上下叠放，只要放得下表头「签收方式」四个字。
pub(crate) const COLUMN_WIDTHS_MM: [f64; 5] = [13.0, 46.0, 24.0, 41.0, 32.0];

/// 签收表列名，与 [`COLUMN_WIDTHS_MM`] 一一对应。
pub(crate) const COLUMN_NAMES: [&str; 5] = ["序号", "签收单位", "签收方式", "签收人", "签收日期"];

/// 签收内容行的最小行高（毫米）：手写签名与日期要有足够的上下余量。
pub(crate) const ROW_MIN_HEIGHT_MM: f64 = 18.0;

/// 主送、抄送都没填时，签收表留出的空行数，供手工填写签收单位。
const BLANK_ROWS: usize = 5;

/// 签收表的版心宽（缇）：156mm。Word 各列宽之和要正好等于它。
pub(crate) const CONTENT_WIDTH_TWIPS: usize = 8_844;

/// 签收方式的可勾选项：电子、纸质。留空由收件人手工勾选。
pub(crate) const SIGN_METHODS: [&str; 2] = ["电子", "纸质"];

/// 签收方式格里的印法：两个可勾选项上下各占一行，PDF 与 Word 共用同一份文字。
pub(crate) fn sign_method_lines() -> Vec<String> {
    SIGN_METHODS
        .iter()
        .map(|method| format!("□{method}"))
        .collect()
}

/// 签收单上的一处「标签 + 取值」。
///
/// 取值为空时留出手写空间：手写栏按全角字数给出（`blank_chars`），Word 与 PDF
/// 都按同一组字数留白，两端画出来的空当一样长。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReceiptField {
    pub label: String,
    pub value: String,
    /// 取值为空时留白处有几个全角字。
    pub blank_chars: usize,
}

impl ReceiptField {
    /// 有取值的字段。
    pub(crate) fn new(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.to_string(),
            value: value.into(),
            blank_chars: 0,
        }
    }

    /// 留白待填的字段：`blank_chars` 个全角字的手写空间。
    pub(crate) fn blank(label: &str, blank_chars: usize) -> Self {
        Self {
            label: label.to_string(),
            value: String::new(),
            blank_chars,
        }
    }

    /// 有值就印值；取值为空（草稿期还没编号、没定日期）就留 `blank_chars` 个全角字的横线。
    pub(crate) fn value_or_blank(
        label: &str,
        value: impl Into<String>,
        blank_chars: usize,
    ) -> Self {
        let value = value.into().trim().to_string();
        if value.is_empty() {
            Self::blank(label, blank_chars)
        } else {
            Self::new(label, value)
        }
    }
}

/// 签收表的一行：序号与签收单位（单位为空时由人手填）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ReceiptRow {
    pub index: String,
    pub unit: String,
}

/// 签收单的全部数据。字段名与 `assets/typst/receipt.typ` 一一对应。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ReceiptDoc {
    pub fonts: FontFamilies,
    /// 题名（固定「公文签收记录单」）。
    pub title: String,
    /// 表格上方的要素行：一行若干「标签 + 取值」。
    pub fields: Vec<Vec<ReceiptField>>,
    /// 签收表列名。
    pub columns: Vec<String>,
    /// 签收方式的可勾选项（电子、纸质），格子里上下叠印成「□电子」「□纸质」。
    pub sign_methods: Vec<String>,
    /// 签收表列宽（毫米）。
    pub column_widths_mm: Vec<f64>,
    /// 签收内容行的最小行高（毫米）。
    pub row_min_height_mm: f64,
    /// 签收单位清单：主送在前、抄送在后，按显示值去重；都没填时是若干空行。
    pub rows: Vec<ReceiptRow>,
    /// 表格下方的送文信息：送文人、联系电话、送文日期、备注。
    pub footer: Vec<Vec<ReceiptField>>,
}

/// 按公函的要素构造签收单数据。
///
/// 字体角色只有 Typst 那边用得到，Word 侧不看这一项，所以这里先留空，
/// 排 PDF 时再把 [`typst_engine::FontFamilies`] 填进去。
pub(crate) fn build(input: &DraftInput, markdown: &str, display: &UnitDisplay) -> ReceiptDoc {
    let mut units = signing_units(input, display);
    if units.is_empty() {
        units = vec![String::new(); BLANK_ROWS];
    }
    ReceiptDoc {
        fonts: empty_families(),
        title: RECEIPT_TITLE.to_string(),
        fields: element_rows(input, markdown, display),
        columns: COLUMN_NAMES.iter().map(|name| name.to_string()).collect(),
        sign_methods: SIGN_METHODS.iter().map(|name| name.to_string()).collect(),
        column_widths_mm: COLUMN_WIDTHS_MM.to_vec(),
        row_min_height_mm: ROW_MIN_HEIGHT_MM,
        rows: units
            .into_iter()
            .enumerate()
            .map(|(index, unit)| ReceiptRow {
                index: (index + 1).to_string(),
                unit,
            })
            .collect(),
        footer: footer_rows(),
    }
}

/// 空的字体角色：Word 侧不用，Typst 侧排版前会覆盖。
fn empty_families() -> FontFamilies {
    FontFamilies {
        title: String::new(),
        heading1: String::new(),
        heading2: String::new(),
        body: String::new(),
        page_number: String::new(),
        bold: None,
        page_number_latin: None,
        fallback: Vec::new(),
    }
}

/// 表格上方的要素行。分三行排：文件名称独占一行，函号与发函单位一行，
/// 成文日期与密级一行（没有密级时这一行只有成文日期）。草稿期还没有的要素
/// （函号、成文日期等）留出手写横线，不让标签后面空着。
fn element_rows(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
) -> Vec<Vec<ReceiptField>> {
    let file_name = crate::export::document_title(input, markdown);
    let mut rows = vec![
        vec![ReceiptField::value_or_blank("文件名称：", file_name, 20)],
        vec![
            ReceiptField::value_or_blank("函号：", number_text(input), 8),
            ReceiptField::value_or_blank("发函单位：", issuing_unit_text(input, display), 10),
        ],
    ];
    let mut last = vec![ReceiptField::value_or_blank(
        "成文日期：",
        element_display::date_display_line(input),
        8,
    )];
    if let Some(security) = element_display::security_display(input) {
        last.push(ReceiptField::new("密级：", security));
    }
    rows.push(last);
    rows
}

/// 表格下方的送文信息。送文人、联系电话、送文日期与备注都留白待填——
/// 程序不替人编造这些内容。横线长度按四号字凑满一个版心（约 31 个全角字）：
/// 第一行 4+4 + 1 + 5+6 + 1 + 5+5，第二行 3+28；电话留 6 个全角字，够写 11 位手机号。
fn footer_rows() -> Vec<Vec<ReceiptField>> {
    vec![
        vec![
            ReceiptField::blank("送文人：", 4),
            ReceiptField::blank("联系电话：", 6),
            ReceiptField::blank("送文日期：", 5),
        ],
        vec![ReceiptField::blank("备注：", 28)],
    ]
}

/// 函号：机关代字〔年份〕序号号。草稿期还没编序号时只印机关代字。
pub(crate) fn number_text(input: &DraftInput) -> String {
    let (code, year, serial) = element_display::number_display_parts(input);
    if serial.is_empty() {
        return code;
    }
    if code.is_empty() {
        return format!("{serial}号");
    }
    if year.is_empty() {
        return format!("{code}{serial}号");
    }
    format!("{code}〔{year}〕{serial}号")
}

/// 发函单位：联合发文取联合单位，否则取发文单位；显示名称与公文一致
/// （外部行文优先用单位外部名称）。
pub(crate) fn issuing_unit_text(input: &DraftInput, display: &UnitDisplay) -> String {
    let external = input.uses_external_unit_names();
    let joint = split_units(&input.profile.joint_issuing_units);
    let units =
        if input.profile.joint_issuance_mode == JointIssuanceMode::Mode1 && !joint.is_empty() {
            joint
        } else {
            split_units(&input.profile.issuing_unit)
        };
    units
        .iter()
        .map(|unit| display.full_name_for(unit, external).trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join("、")
}

/// 签收单位清单：主送在前、抄送在后；按最终显示值去重。
pub(crate) fn signing_units(input: &DraftInput, display: &UnitDisplay) -> Vec<String> {
    let external = input.uses_external_unit_names();
    let mut units: Vec<String> = Vec::new();
    for unit in split_units(&input.profile.recipient)
        .into_iter()
        .chain(split_units(&input.profile.copies_to))
    {
        let name = display.full_name_for(&unit, external).trim().to_string();
        if name.is_empty() || units.iter().any(|seen| seen == &name) {
            continue;
        }
        units.push(name);
    }
    units
}

/// 排一份签收记录单 PDF 写到 `path`。
pub(crate) fn write_pdf(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
) -> Result<TypstOutcome> {
    let base = crate::storage::config_dir()?;
    write_pdf_with_base(path, input, markdown, display, fonts, &base)
}

/// 同上，插图等相对路径按 `base_dir` 解析（签收单本身没有插图）。
pub(crate) fn write_pdf_with_base(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    base_dir: &Path,
) -> Result<TypstOutcome> {
    let set = typst_engine::font_set(fonts)?;
    let mut doc = build(input, markdown, display);
    doc.fonts = set.families.clone();
    let data = serde_json::to_string(&doc).context("无法序列化签收记录单数据")?;
    let outcome = typst_engine::compile(&TypstJob::receipt(data, base_dir), &set)?;
    std::fs::write(path, &outcome.pdf)
        .with_context(|| format!("无法写入签收记录单 PDF：{}", path.display()))?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CorrespondenceScope, TemplateKind, TemplateProfile};

    fn letter() -> DraftInput {
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            profile: TemplateProfile::for_kind(TemplateKind::OfficialLetter),
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.department_code = "某民函".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "18".into();
        input.profile.recipient = "甲单位、乙单位".into();
        input.profile.copies_to = "丙单位、甲单位".into();
        // 默认密级是「机密」，这里清掉，密级另行单测。
        input.profile.security_level = String::new();
        input.date = "2026年9月30日".into();
        input
    }

    #[test]
    fn signing_units_put_recipients_first_and_deduplicate() {
        let display = UnitDisplay::new(&[]);
        let units = signing_units(&letter(), &display);
        // 主送在前、抄送在后；抄送里的甲单位与主送重复，去掉。
        assert_eq!(units, ["甲单位", "乙单位", "丙单位"]);
    }

    #[test]
    fn signing_units_dedupe_by_displayed_name() {
        // 词库里两个不同编码指向同一个外部名称时，按显示值去重只留一行。
        let vocabulary = vec![crate::models::VocabularyEntry {
            code: "01".into(),
            canonical: "某某市教育局".into(),
            ..Default::default()
        }];
        let display = UnitDisplay::new(&vocabulary);
        let mut input = letter();
        input.profile.recipient = "01、某某市教育局".into();
        input.profile.copies_to = String::new();
        assert_eq!(signing_units(&input, &display), ["某某市教育局"]);
    }

    #[test]
    fn number_text_follows_the_letter_number_rules() {
        let input = letter();
        assert_eq!(number_text(&input), "某民函〔2026〕18号");
        // 草稿期未编序号：只印机关代字。
        let mut input = input.clone();
        input.profile.document_number = String::new();
        assert_eq!(number_text(&input), "某民函");
        // 没有年代字：机关代字与序号直接相连（年代字取成文日期里的年份，
        // 所以日期也要清掉）。
        input.profile.document_number = "5".into();
        input.profile.document_year = String::new();
        input.date = String::new();
        assert_eq!(number_text(&input), "某民函5号");
    }

    #[test]
    fn issuing_unit_uses_joint_units_when_issued_jointly() {
        let display = UnitDisplay::new(&[]);
        let mut input = letter();
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "甲机关、乙机关".into();
        assert_eq!(issuing_unit_text(&input, &display), "甲机关、乙机关");
        // 非联合发文取发文单位。
        let mut input = letter();
        input.profile.joint_issuance_mode = JointIssuanceMode::Single;
        assert_eq!(issuing_unit_text(&input, &display), "某某市民政局");
    }

    #[test]
    fn elements_are_read_from_the_letter() {
        let display = UnitDisplay::new(&[]);
        let input = letter();
        let doc = build(&input, "# 关于商洽的函\n\n正文。", &display);
        assert_eq!(doc.title, "公文签收记录单");
        assert_eq!(doc.fields.len(), 3);
        assert_eq!(doc.fields[0][0].value, "关于商洽的函");
        assert_eq!(doc.fields[1][0].value, "某民函〔2026〕18号");
        assert_eq!(doc.fields[1][1].value, "某某市民政局");
        assert_eq!(doc.fields[2][0].value, "2026年9月30日");
        // 没有密级时不排密级这一栏。
        assert_eq!(doc.fields[2].len(), 1);
        assert_eq!(doc.rows.len(), 3);
        assert_eq!(doc.rows[0].index, "1");
        assert_eq!(doc.row_min_height_mm, ROW_MIN_HEIGHT_MM);
        assert_eq!(doc.rows[0].unit, "甲单位");
        assert_eq!(doc.columns, COLUMN_NAMES);
        assert_eq!(doc.column_widths_mm, COLUMN_WIDTHS_MM.to_vec());
        // 送文信息一律留白，不替人编造。
        assert!(
            doc.footer
                .iter()
                .all(|row| row.iter().all(|f| f.value.is_empty()))
        );
    }

    #[test]
    fn security_is_shown_once_above_the_table() {
        let display = UnitDisplay::new(&[]);
        let mut input = letter();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "1年".into();
        let doc = build(&input, "# 关于商洽的函", &display);
        assert_eq!(doc.fields[2][1].label, "密级：");
        assert_eq!(doc.fields[2][1].value, "秘密★1年");
        // 密级只在要素行印一次，签收表不再逐行重复。
        assert!(!doc.columns.iter().any(|name| name == "密级"));
    }

    #[test]
    fn missing_elements_and_units_leave_room_to_write_by_hand() {
        let display = UnitDisplay::new(&[]);
        let mut input = letter();
        input.profile.department_code.clear();
        input.profile.document_number.clear();
        input.profile.recipient.clear();
        input.profile.copies_to.clear();
        input.date.clear();
        let doc = build(&input, "# 关于商洽的函", &display);
        let number = &doc.fields[1][0];
        assert!(number.value.is_empty() && number.blank_chars > 0);
        let date = &doc.fields[2][0];
        assert!(date.value.is_empty() && date.blank_chars > 0);
        assert_eq!(doc.rows.len(), BLANK_ROWS);
        assert!(doc.rows.iter().all(|row| row.unit.is_empty()));
        assert_eq!(doc.rows[BLANK_ROWS - 1].index, BLANK_ROWS.to_string());
    }

    #[test]
    fn column_widths_fill_the_text_block_and_footer_fits_one_line() {
        assert_eq!(COLUMN_WIDTHS_MM.iter().sum::<f64>(), 156.0);
        // 四号字 14pt，一个版心约 31.3 个全角字；送文信息每行都不能折行。
        let per_line = 156.0 / (14.0 * 25.4 / 72.0);
        for row in footer_rows() {
            let chars = row
                .iter()
                .map(|f| f.label.chars().count() + f.blank_chars)
                .sum::<usize>()
                + row.len()
                - 1;
            assert!(
                (chars as f64) <= per_line,
                "送文信息一行 {chars} 字，会折行"
            );
        }
    }

    /// 外部行文时签收单位用外部名称。
    #[test]
    fn external_scope_uses_the_external_unit_name() {
        let vocabulary = vec![crate::models::VocabularyEntry {
            code: "01".into(),
            canonical: "某某市教育局".into(),
            external_name: "某某市教育局（对外）".into(),
            ..Default::default()
        }];
        let display = UnitDisplay::new(&vocabulary);
        let mut input = letter();
        input.profile.correspondence_scope = CorrespondenceScope::External;
        input.profile.recipient = "01".into();
        input.profile.copies_to = String::new();
        assert_eq!(signing_units(&input, &display), ["某某市教育局（对外）"]);
    }

    /// 把 PDF 每页渲染成 `page-N.png`（宽 1240 像素，约 150dpi）。
    fn save_page_images(pdf: &[u8], dir: &Path) {
        use hayro::hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf};
        use hayro::{RenderCache, RenderSettings};
        // 先清掉上一次留下的页图，页数变少时不留过期的尾页。
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("page-") && name.ends_with(".png") {
                std::fs::remove_file(entry.path()).unwrap();
            }
        }
        let pdf = Pdf::new(std::sync::Arc::new(pdf.to_vec())).unwrap();
        for (index, page) in pdf.pages().iter().enumerate() {
            let scale = 1240.0 / page.render_dimensions().0;
            let pixmap = hayro::render(
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
            image::RgbaImage::from_raw(
                u32::from(pixmap.width()),
                u32::from(pixmap.height()),
                pixmap.data_as_u8_slice().to_vec(),
            )
            .unwrap()
            .save(dir.join(format!("page-{}.png", index + 1)))
            .unwrap();
        }
    }

    /// 样张：PDF、Word、模板数据与逐页 PNG 写到 `tmp/typst-samples/receipt/`，
    /// 改模板后目视检查用。依赖本机随包字体，默认忽略：
    ///
    /// ```text
    /// cargo test --locked typst_samples_receipt -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "依赖本机 runtime 字体，手动运行"]
    fn typst_samples_receipt() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/typst-samples/receipt");
        std::fs::create_dir_all(&dir).unwrap();
        let fonts = FontConfig::default();
        let display = UnitDisplay::new(&[]);
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            profile: TemplateProfile::for_kind(TemplateKind::OfficialLetter),
            ..Default::default()
        };
        input.profile.issuing_unit = "某某市民政局".into();
        input.profile.department_code = "某民函".into();
        input.profile.document_year = "2026".into();
        input.profile.document_number = "18".into();
        input.profile.recipient = "甲单位、乙单位".into();
        input.profile.copies_to = "丙单位、丁单位".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "1年".into();
        input.date = "2026年9月30日".into();
        let markdown =
            "# 关于商洽公共服务的函\n\n甲单位、乙单位：\n\n现就有关事项函告如下。\n\n特此函告。";

        let set = typst_engine::font_set(&fonts).unwrap();
        let mut doc = build(&input, markdown, &display);
        doc.fonts = set.families.clone();
        let data = serde_json::to_string_pretty(&doc).unwrap();
        std::fs::write(dir.join("doc.json"), &data).unwrap();

        let outcome = typst_engine::compile(&TypstJob::receipt(data, &dir), &set).unwrap();
        std::fs::write(dir.join("typst.pdf"), &outcome.pdf).unwrap();
        crate::export::docx::write_receipt_docx(&dir.join("word.docx"), &input, markdown, &display)
            .unwrap();
        save_page_images(&outcome.pdf, &dir);
    }

    /// 多页样张：验证续页重复表头、长单位名与整行不拆分。
    #[test]
    #[ignore = "依赖本机 runtime 字体，手动运行"]
    fn typst_samples_receipt_multipage() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/typst-samples/receipt-multipage");
        std::fs::create_dir_all(&dir).unwrap();
        let mut input = letter();
        input.profile.recipient = (1..=24)
            .map(|index| {
                if index % 7 == 0 {
                    format!("某某市第{index}区公共政务服务与数据管理联合办公室")
                } else {
                    format!("签收单位{index:02}")
                }
            })
            .collect::<Vec<_>>()
            .join("、");
        input.profile.copies_to.clear();
        let outcome = write_pdf_with_base(
            &dir.join("typst.pdf"),
            &input,
            "# 关于商洽公共服务事项的函",
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &dir,
        )
        .unwrap();
        let pages = lopdf::Document::load_mem(&outcome.pdf)
            .unwrap()
            .get_pages()
            .len();
        assert!(pages > 1, "多页样张实际只有一页");
        save_page_images(&outcome.pdf, &dir);
    }
}
