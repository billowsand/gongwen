//! 导出：把公文导出为 docx / latex / md 三种成品，并提供 Markdown 解析、
//! 标题计数、红头版记度量与文本工具。
//!
//! 各功能域已拆分到 `export/` 子模块（Markdown 解析、红头版记、标题计数、
//! 文本工具），根文件保留导出入口与文件名工具。

mod docx;
mod latex;
pub(crate) mod table;
pub(crate) mod title;
pub(crate) use latex::write_tex;

use crate::models::{DraftInput, ExportSelection, FontConfig, TemplateKind, split_units};
use crate::units::UnitDisplay;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

mod headings;
mod parse;
mod red;
mod text;

pub(crate) use headings::{HeadingCounters, clean_heading_number, official_heading_text};
pub(crate) use parse::{
    ColumnAlign, LocatedBlock, MarkdownBlock, MarkdownSection, block_span_for_line,
    body_heading_max_level, circled_number, is_image_line, normalize_ordered_list_punctuation,
    parse_markdown, parse_markdown_located, parse_markdown_with_lines, parse_ordered_item,
    parse_section_marker,
};
pub(crate) use red::{
    RED_RECORD_CONTACT_TWIPS, RED_RECORD_CONTACT_USABLE_TWIPS, RED_RECORD_PHONE_TWIPS,
    RED_RECORD_PHONE_USABLE_TWIPS, RED_RECORD_UNIT_TWIPS, RED_RECORD_UNIT_USABLE_TWIPS,
    SIGNATURE_ROOM_MM, SIGNATURE_ROOM_TWIPS, red_approval_body_metrics, red_approval_wrap_lines,
    red_record_scale_percent, red_signature_unit_width_mm, red_signature_unit_width_twips,
};
#[cfg(test)]
pub(crate) use red::{RED_RECORD_EM_TWIPS, RED_RECORD_TOTAL_TWIPS, red_signature_unit_width_em};
#[cfg(test)]
pub(crate) use text::parenthesized_ranges;
pub(crate) use text::{
    InlineSegment, attachment_names, attachment_title_name, chinese_date_parts, inline_segments,
    legacy_attachment_label, normalize_chinese_quotes, number_to_chinese, plain_text,
    table_columns,
};

pub(crate) fn joint_main_index(input: &DraftInput) -> Option<usize> {
    let units = split_units(&input.profile.joint_issuing_units);
    let main = input.profile.main_issuing_unit.trim();
    if units.is_empty() || main.is_empty() {
        return None;
    }
    units.iter().position(|unit| unit == main)
}

/// 当前稿件实际承载代章属性的单位：联合发文取主发文单位，其他公函取落款单位，
/// 落款单位为空时回落发文单位。是否标注由标准词库中的单位属性唯一决定。
/// 代章只对公函生效——其他文种不盖章，无需标注。
pub(crate) fn seals_on_behalf(input: &DraftInput, display: &UnitDisplay) -> bool {
    if input.kind != TemplateKind::OfficialLetter {
        return false;
    }
    let unit = if input.profile.joint_issuance_mode == crate::models::JointIssuanceMode::Mode1 {
        let units = split_units(&input.profile.joint_issuing_units);
        let main = input.profile.main_issuing_unit.trim();
        if units.iter().any(|unit| unit == main) {
            main.to_string()
        } else {
            units.first().cloned().unwrap_or_default()
        }
    } else if !input.profile.signing_unit.trim().is_empty() {
        input.profile.signing_unit.trim().to_string()
    } else {
        input.profile.issuing_unit.trim().to_string()
    };
    display.seals_on_behalf(&unit)
}

/// 联合发文中实际需要追加“（代章）”的单位位置（联合发文仅公函）。旧稿若未保存主发文单位，
/// 与红头规则一致回落到第一个联合发文单位。
pub(crate) fn joint_seal_index(input: &DraftInput, display: &UnitDisplay) -> Option<usize> {
    if !seals_on_behalf(input, display) {
        return None;
    }
    joint_main_index(input)
        .or_else(|| (!split_units(&input.profile.joint_issuing_units).is_empty()).then_some(0))
}

/// 联合发文落款：主发文单位所在列（0=左列，1=右列）。预览/LaTeX/DOCX 三处共用，
/// 把成文日期（含代章）压在主发文单位下方而不是整块居中。
/// 返回 `None` 表示主单位不在联合单位列表中、未指定，或是跨两列的最后一个单位，
/// 此时日期仍整体居中。
pub(crate) fn joint_main_column(input: &DraftInput) -> Option<usize> {
    let units = split_units(&input.profile.joint_issuing_units);
    let index = joint_main_index(input)?;
    // 超过两个且为奇数时最后一个单位跨两列居中（规格 §2.5），日期随之居中。
    let odd_last = units.len() > 2 && units.len() % 2 == 1;
    if odd_last && index + 1 == units.len() {
        return None;
    }
    Some(index % 2)
}

pub fn extract_title(markdown: &str, fallback: &str) -> String {
    markdown
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if fallback.trim().is_empty() {
                "未命名公文"
            } else {
                fallback.trim()
            }
        })
        .to_string()
}

/// Markdown 保留“标题 + 正文 + 可选附件”。密级、文号、主送、落款、成文日期、抄送和版记
/// 都由 DOCX/LaTeX 导出器按锁定元数据渲染，这里不再重复写入。
pub fn finalize_markdown(input: &DraftInput, generated: &str) -> String {
    let mut text = normalize_ordered_list_punctuation(generated.trim());
    if !text.lines().any(|line| line.starts_with("# ")) {
        let title = if input.title_hint.trim().is_empty() {
            "【待核实：标题】"
        } else {
            input.title_hint.trim()
        };
        text = format!("# {title}\n\n{text}");
    }
    format!("{}\n", text.trim())
}

pub fn export_all(
    output_dir: &Path,
    input: &DraftInput,
    markdown: &str,
    selection: &ExportSelection,
    display: &UnitDisplay,
    fonts: &FontConfig,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("无法创建输出目录：{}", output_dir.display()))?;
    let title = extract_title(markdown, &input.title_hint);
    // 按文稿类型生成统一主干名，三格式及编译出的 PDF 共用，方便归档对应：
    // 会议议程“名称+会议时间”，白头件“白头+名称+时间戳”，公函“函号+名称+时间戳”，
    // 电话通知“电话通知+时间戳”，普通公文“普通公文+名称+时间戳”。
    let stem = document_stem(input, &title);
    // 每次导出以文件生成名为单元归档：同一批的 md/docx/tex/pdf
    // 以及 TeX 类文件都放进同名子目录。不覆盖时在目录名上统一编号，
    // 避免各扩展名分别寻找可用名称后落到不同版本。
    let export_stem = if selection.overwrite {
        stem
    } else {
        unique_directory_stem(output_dir, &stem)
    };
    let document_dir = output_dir.join(&export_stem);
    fs::create_dir_all(&document_dir)
        .with_context(|| format!("无法创建文稿导出目录：{}", document_dir.display()))?;
    let mut files = Vec::new();

    if selection.markdown {
        let path = document_dir.join(format!("{export_stem}.md"));
        fs::write(&path, markdown)?;
        // 图片复制到导出目录（保持 images/ 相对结构），导出的 md 目录自包含。
        crate::images::copy_refs(markdown, &document_dir)?;
        files.push(path);
    }
    if selection.docx {
        let path = document_dir.join(format!("{export_stem}.docx"));
        docx::write_docx(&path, input, markdown, display)?;
        files.push(path);
    }
    if selection.tex {
        let path = document_dir.join(format!("{export_stem}.tex"));
        latex::write_tex(&path, input, markdown, display, fonts)?;
        files.push(path);
    }
    Ok(files)
}

/// 导出文件名的固定前缀（不含分钟级时间戳）。导出目录里属于同一文稿的
/// 文件夹都以它为前缀，工具栏“打开最近导出”靠它识别当前文稿的文件夹：
/// - 会议议程：名称（或 名称 + 会议时间，两者都不带时间戳，前缀即完整主干）；
/// - 白头件：`白头` + 名称；
/// - 公函：函号 + 名称（草稿期未编序号时回落机关代字，均缺失时用“公函”）；
/// - 电话通知：`电话通知`；
/// - 普通公文：`普通公文` + 名称。
pub(crate) fn document_stem_prefix(input: &DraftInput, title: &str) -> String {
    let title = safe_filename(title);
    match input.kind {
        TemplateKind::MeetingAgenda => {
            if input.meeting_time.trim().is_empty() {
                title
            } else {
                format!("{title}-{}", safe_filename(&input.meeting_time))
            }
        }
        TemplateKind::WhitePaper => format!("白头-{title}"),
        TemplateKind::RedHeadApproval => format!("红头呈批-{}-{title}", letter_prefix(input)),
        TemplateKind::OfficialLetter => format!("{}-{title}", letter_prefix(input)),
        TemplateKind::PhoneNotice => "电话通知".to_string(),
        TemplateKind::PlainDocument => format!("普通公文-{title}"),
    }
}

/// 导出文件名的主干按文稿类型区分，`title` 已由 `extract_title` 取好：
/// 会议议程没有时间戳，其余类型在固定前缀后追加分钟级时间戳。
pub(crate) fn document_stem(input: &DraftInput, title: &str) -> String {
    // 时间戳为分钟级，同一分钟内反复导出保持同名，由 overwrite/同名编号决定是否覆盖。
    let timestamp = chrono::Local::now().format("%Y%m%d%H%M").to_string();
    let prefix = document_stem_prefix(input, title);
    match input.kind {
        TemplateKind::MeetingAgenda => prefix,
        _ => format!("{prefix}-{timestamp}"),
    }
}

/// 公函文件名的文号前缀：机关代字〔年〕序号号；草稿期未编序号时回落机关代字。
fn letter_prefix(input: &DraftInput) -> String {
    let code = input.profile.department_code.trim();
    let serial = input.profile.document_number.trim();
    if code.is_empty() && serial.is_empty() {
        return "公函".to_string();
    }
    if serial.is_empty() {
        return safe_filename(code);
    }
    let year = input.document_year();
    if code.is_empty() {
        return format!("{}号", safe_filename(serial));
    }
    let code = safe_filename(code);
    let serial = safe_filename(serial);
    if year.is_empty() {
        format!("{code}{serial}号")
    } else {
        format!("{code}〔{year}〕{serial}号")
    }
}

pub fn safe_filename(value: &str) -> String {
    let mut name = value
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    name = name.trim().trim_end_matches('.').to_string();
    if name.chars().count() > 80 {
        name = name.chars().take(80).collect();
    }
    if name.is_empty() {
        "未命名公文".into()
    } else {
        name
    }
}

fn unique_directory_stem(dir: &Path, stem: &str) -> String {
    let candidate = dir.join(stem);
    if !candidate.exists() {
        return stem.to_string();
    }
    for index in 2..1000 {
        let versioned_stem = format!("{stem}-{index}");
        let candidate = dir.join(&versioned_stem);
        if !candidate.exists() {
            return versioned_stem;
        }
    }
    format!("{stem}-{}", chrono::Local::now().timestamp())
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn filename_is_windows_safe() {
        assert_eq!(safe_filename("关于A/B:测试?的函"), "关于A_B_测试_的函");
    }

    /// 承办区三栏宽度必须正好铺满版心，否则三端会各自错位。
    #[test]
    fn red_record_columns_fill_the_content_width() {
        assert_eq!(
            RED_RECORD_UNIT_TWIPS + RED_RECORD_CONTACT_TWIPS + RED_RECORD_PHONE_TWIPS,
            RED_RECORD_TOTAL_TWIPS
        );
        // 承办单位栏最宽：`承办单位：`5 字之后还要放得下单位简称。
        const { assert!(RED_RECORD_UNIT_TWIPS > RED_RECORD_PHONE_TWIPS) };
    }

    /// 承办单位一律不换行：放得下不动、放不下按比例压窄，且始终留出栏间空白。
    #[test]
    fn red_record_scale_compresses_only_when_too_wide() {
        // “承办单位：综合处”8 字 = 2560 缇，可用 3564-320 = 3244 缇，放得下。
        assert_eq!(
            red_record_scale_percent("承办单位：综合处", RED_RECORD_UNIT_USABLE_TWIPS),
            100
        );
        // 15 字 = 4800 缇，超出可用宽度，应压到 67%（3244/4800）。
        let long = red_record_scale_percent(
            "承办单位：教师工作与师资管理处",
            RED_RECORD_UNIT_USABLE_TWIPS,
        );
        assert!((65..=70).contains(&long), "应按比例压窄：{long}");
        // 压缩后的宽度不得超过“栏宽减一个字”，保证与下一栏标签之间有间隔。
        let natural =
            title::display_units("承办单位：教师工作与师资管理处") * RED_RECORD_EM_TWIPS / 2;
        assert!(natural * long / 100 <= RED_RECORD_UNIT_USABLE_TWIPS);
        // 电话栏不让宽：常见 12 位号码原样排下，不做压缩。
        assert_eq!(
            red_record_scale_percent("电话：010-12345678", RED_RECORD_PHONE_USABLE_TWIPS),
            100
        );
    }

    /// 签字空间在三端必须同宽：mm（预览/LaTeX）与缇（Word）不能各说各话。
    #[test]
    fn signature_room_is_the_same_width_in_every_unit() {
        assert_eq!(
            (SIGNATURE_ROOM_MM / 25.4 * 1440.0).round() as usize,
            SIGNATURE_ROOM_TWIPS
        );
    }

    /// 落款单位块宽度：少于 5 字的按分散后的 5 字宽算，多单位取最宽的一个。
    #[test]
    fn red_signature_unit_width_takes_the_widest_spread_unit() {
        // 3 字会被分散到 5 字宽，6 字按自然宽度，取较大者 6。
        let units = vec!["网信办".to_string(), "星海省教育厅".to_string()];
        assert_eq!(red_signature_unit_width_em(&units), 6.0);
        assert_eq!(
            red_signature_unit_width_twips(&units),
            6 * RED_RECORD_EM_TWIPS
        );
        // 毫米与缇必须指向同一个宽度（LaTeX 用毫米，Word 用缇）。
        let mm = red_signature_unit_width_mm(&units);
        assert!((mm - 33.867).abs() < 0.01, "{mm}");
        assert_eq!(
            (mm / 25.4 * 1440.0).round() as usize,
            red_signature_unit_width_twips(&units)
        );
        // 全是短单位时统一按分散后的 5 字宽。
        assert_eq!(red_signature_unit_width_em(&["办公室".to_string()]), 5.0);
        assert_eq!(red_signature_unit_width_em(&[]), 0.0);
    }

    /// 首页正文行数随标题行数与承办条目数递减，并保留下限。
    #[test]
    fn red_approval_wrap_lines_shrink_with_title_and_records() {
        assert_eq!(red_approval_wrap_lines(1, 1), 12);
        assert_eq!(red_approval_wrap_lines(1, 2), 11);
        assert_eq!(red_approval_wrap_lines(2, 4), 8);
        // 极端情况下仍留出最少行数，不会归零。
        assert_eq!(red_approval_wrap_lines(9, 9), 4);
    }

    /// 三端共用的正文估算：行数、首个表格图片的位置、以及由此得出的两个判断。
    #[test]
    fn red_approval_body_metrics_drive_page_break_and_notice() {
        // 短正文：不到首页额度，落款另起一页并标“此页无正文”。
        let short = parse_markdown("# 标题\n\n情况如下。妥否，请指示。");
        let metrics = red_approval_body_metrics(&short);
        assert_eq!(metrics.lines_before_float, None);
        assert!(!metrics.reaches_second_page(11));
        assert!(!metrics.float_needs_page_break(11));

        // 长正文：超出首页额度，落款紧随正文，不再制造空白页。
        let long = parse_markdown(&format!("# 标题\n\n{}", "填充版面的正文。".repeat(60)));
        let metrics = red_approval_body_metrics(&long);
        assert!(metrics.lines > 11);
        assert!(metrics.reaches_second_page(11));

        // 正文含表格：无论长短都算作跨页，且表格要被赶出首页。
        let table = parse_markdown("# 标题\n\n短正文。\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n");
        let metrics = red_approval_body_metrics(&table);
        assert_eq!(metrics.lines_before_float, Some(1));
        assert!(metrics.reaches_second_page(11));
        assert!(metrics.float_needs_page_break(11));

        // 附件区的表格不受影响：附件本来就另起一页，没有批示栏要避让。
        let attachment = parse_markdown(
            "# 标题\n\n短正文。\n<!-- [附件] -->\n# 附件\n\n| 甲 | 乙 |\n| --- | --- |\n| 1 | 2 |\n",
        );
        assert_eq!(
            red_approval_body_metrics(&attachment).lines_before_float,
            None
        );
    }

    #[test]
    fn parses_chinese_document_date_parts() {
        assert_eq!(chinese_date_parts("2026年8月5日"), Some(("2026", "8", "5")));
        assert_eq!(chinese_date_parts("待定"), None);
    }

    /// 实时排版编辑器逐行叠加的标题编号：规则与导出器/预览一致——
    /// 区段标记与每个附件标题处都重置计数器，附件区整体上移一级编号。
    #[test]
    fn heading_prefixes_restart_after_section_markers() {
        let mut counters = HeadingCounters::default();
        let lines = [
            "# 测试函",
            "<!-- [正文] -->",
            "## 一、总体要求",
            "### （一）具体事项",
            "正文段落。",
            "<!-- [附件] -->",
            "# 统计表",
            "## 一、填报说明",
            "### 子项",
            "附件内容。",
            "<!-- [附件] -->",
            "# 说明材料",
            "## 二、其他说明",
        ];
        let prefixes = lines
            .iter()
            .map(|line| counters.next(line))
            .collect::<Vec<_>>();
        assert_eq!(prefixes[0], None, "文档标题不编号");
        assert_eq!(prefixes[1], None, "区段标记不编号");
        assert_eq!(prefixes[2].as_deref(), Some("一、"));
        assert_eq!(prefixes[3].as_deref(), Some("（一）"));
        assert_eq!(prefixes[4], None, "正文段落不编号");
        assert_eq!(prefixes[5], None, "附件区段标记不编号");
        assert_eq!(prefixes[6], None, "附件正式标题（#）不编号");
        assert_eq!(
            prefixes[7].as_deref(),
            Some("一、"),
            "附件内容标题与正文同级并从一、起"
        );
        assert_eq!(prefixes[8].as_deref(), Some("（一）"));
        assert_eq!(prefixes[9], None);
        assert_eq!(prefixes[10], None, "第二个附件标记");
        assert_eq!(prefixes[11], None, "第二个附件正式标题");
        assert_eq!(
            prefixes[12].as_deref(),
            Some("一、"),
            "每个附件之后的内容都重新编号"
        );

        // 旧格式仍可用于实时排版，附件内容层级会在显示期上移一级。
        let mut counters = HeadingCounters::default();
        counters.next("# 测试函");
        assert_eq!(counters.next("## 一"), Some("一、".to_string()));
        counters.next("<!-- [附件] -->");
        assert_eq!(counters.next("# 附件1"), None);
        assert_eq!(counters.next("## 统计表"), None);
        assert_eq!(counters.next("### 填报说明").as_deref(), Some("一、"));

        // 正文标记回到正文区，同样重新起算。
        let mut counters = HeadingCounters::default();
        counters.next("# 测试函");
        counters.next("## 一");
        counters.next("## 二");
        assert_eq!(counters.next("<!-- [正文] -->"), None);
        assert_eq!(counters.next("## 重新起算").as_deref(), Some("一、"));
    }

    /// 文档标题与附件正式标题需要按方正小标宋二号居中渲染（centered_title 标志），
    /// 附件标识与内容标题不居中。
    #[test]
    fn formal_titles_are_marked_for_centered_rendering() {
        let mut counters = HeadingCounters::default();
        assert!(!counters.centered_title());
        counters.next("# 测试函");
        assert!(counters.centered_title(), "文档标题需居中");
        counters.next("正文段落。");
        assert!(!counters.centered_title());
        counters.next("<!-- [附件] -->");
        assert!(!counters.centered_title());
        counters.next("# 附件1");
        assert!(!counters.centered_title(), "附件标识不居中");
        counters.next("## 统计表");
        assert!(counters.centered_title(), "附件正式标题需居中");
        counters.next("### 一、填报说明");
        assert!(!counters.centered_title(), "附件内容标题不居中");
    }

    /// 联合发文日期压在主发文单位所在列下方；主单位跨列时整体居中。
    #[test]
    fn joint_main_column_follows_the_main_unit() {
        let mut input = DraftInput::default();
        input.profile.joint_issuing_units = "甲单位、乙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        assert_eq!(joint_main_column(&input), Some(0));
        input.profile.main_issuing_unit = "乙单位".into();
        assert_eq!(joint_main_column(&input), Some(1));

        // 超过两个且为奇数时，最后一个单位跨两列，日期随之整体居中。
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "甲单位".into();
        assert_eq!(joint_main_column(&input), Some(0));
        input.profile.main_issuing_unit = "丙单位".into();
        assert_eq!(joint_main_column(&input), None);

        // 主单位未指定或不在列表中时整体居中。
        input.profile.main_issuing_unit = "丁单位".into();
        assert_eq!(joint_main_column(&input), None);
        input.profile.main_issuing_unit.clear();
        assert_eq!(joint_main_column(&input), None);
    }

    #[test]
    fn joint_main_index_locates_the_main_unit() {
        let mut input = DraftInput::default();
        input.profile.joint_issuing_units = "甲单位、乙单位、丙单位".into();
        input.profile.main_issuing_unit = "乙单位".into();
        assert_eq!(joint_main_index(&input), Some(1));
        input.profile.main_issuing_unit = "丁单位".into();
        assert_eq!(joint_main_index(&input), None);
        input.profile.main_issuing_unit.clear();
        assert_eq!(joint_main_index(&input), None);
    }

    /// 每个块记下的字节范围必须正好切出它自己的源码：界面预览点击回跳全靠它。
    #[test]
    fn block_lines_and_spans_agree_with_each_other() {
        let markdown = "# 标题\n\n第一段首行\n第一段次行\n\n- 列表项\n\n第三段。\n";
        let (blocks, lines) = parse_markdown_with_lines(markdown);
        assert_eq!(blocks.len(), lines.len());
        // 行号就是 \GwaTail 写进 TeX 的那个值，必须指向该块自己的第一行。
        assert_eq!(lines, vec![1, 3, 6, 8]);
        for line in lines {
            let span = block_span_for_line(markdown, line).expect("每个块都该能反查回范围");
            let first_line = markdown[span.clone()].lines().next().unwrap();
            assert_eq!(
                markdown.lines().nth(line - 1).unwrap(),
                first_line,
                "第 {line} 行反查到的范围应从这一行开始"
            );
        }
    }

    #[test]
    fn block_span_covers_the_whole_soft_wrapped_paragraph() {
        // 孤行提示指向整段：点击后要把这段全选中，用户才知道改哪里。
        let markdown = "# 标题\n\n第一段首行\n第一段次行\n\n第二段。\n";
        let span = block_span_for_line(markdown, 3).unwrap();
        assert_eq!(&markdown[span], "第一段首行\n第一段次行");
        assert!(
            block_span_for_line(markdown, 4).is_none(),
            "段内后续行不单独成块"
        );
    }

    #[test]
    fn located_blocks_point_back_at_their_own_source() {
        let markdown = "# 标题\n\n<!-- [正文] -->\n\n第一段首行\n第一段次行\n\n- 列表项\n\n| 甲 | 乙 |\n|---|---|\n| 1 | 2 |\n";
        let located = parse_markdown_located(markdown);
        let slices = located
            .iter()
            .map(|block| &markdown[block.range.clone()])
            .collect::<Vec<_>>();
        assert_eq!(
            slices,
            [
                "# 标题",
                "<!-- [正文] -->",
                // 段落跨行时范围覆盖合并进来的每一行。
                "第一段首行\n第一段次行",
                "- 列表项",
                // 表格从表头一直到最后一行数据。
                "| 甲 | 乙 |\n|---|---|\n| 1 | 2 |",
            ]
        );
    }

    /// 带位置的解析必须与原来的解析给出完全相同的块序列。
    #[test]
    fn located_parsing_matches_plain_parsing() {
        let markdown = "# 标题\n\n正文（含括号）。\n\n## 小标题\n\n<div class=\"x\">\n内容\n</div>\n\n<!-- [附件] -->\n\n# 附件1\n";
        let located = parse_markdown_located(markdown)
            .into_iter()
            .map(|block| block.block)
            .collect::<Vec<_>>();
        assert_eq!(located, parse_markdown(markdown));
    }

    #[test]
    fn parses_basic_blocks() {
        let blocks = parse_markdown("# 标题\n\n## 一、事项\n正文\n\n- 要点");
        assert!(matches!(&blocks[0], MarkdownBlock::Title(t) if t == "标题"));
        assert!(matches!(&blocks[1], MarkdownBlock::Heading(2, t) if t == "事项"));
        assert!(matches!(&blocks[2], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn parses_body_and_attachment_markers_and_all_heading_levels() {
        let blocks = parse_markdown(
            "# 函件标题\n<!-- [正文] -->\n## 一、正文事项\n### （一）具体要求\n<!-- [附件] -->\n# 表格\n## 一、说明",
        );
        assert!(matches!(
            blocks[1],
            MarkdownBlock::Marker(MarkdownSection::Body)
        ));
        assert!(matches!(&blocks[2], MarkdownBlock::Heading(2, text) if text == "正文事项"));
        assert!(matches!(&blocks[3], MarkdownBlock::Heading(3, text) if text == "具体要求"));
        assert!(matches!(
            blocks[4],
            MarkdownBlock::Marker(MarkdownSection::Attachment)
        ));
        assert!(matches!(&blocks[5], MarkdownBlock::Title(text) if text == "表格"));
        assert!(matches!(&blocks[6], MarkdownBlock::Heading(2, text) if text == "说明"));

        // 历史稿读取时规范化为相同结构，不要求用户手工迁移。
        let legacy = parse_markdown("# 函件标题\n<!-- [附件] -->\n# 附件1：表格\n### 一、说明");
        assert!(matches!(&legacy[2], MarkdownBlock::Title(text) if text == "表格"));
        assert!(matches!(&legacy[3], MarkdownBlock::Heading(2, text) if text == "说明"));
    }

    #[test]
    fn body_heading_max_level_ignores_title_and_attachment_headings() {
        // 正文区最深 4 级；附件区虽含 5 级标题，但不计入正文最大层级。
        let blocks = parse_markdown(
            "# 函件标题\n## 一、正文事项\n### （一）具体要求\n#### 1. 更细\n<!-- [附件] -->\n# 附件1：表格\n##### 1. 说明",
        );
        assert_eq!(body_heading_max_level(&blocks), 4);
        // 无正文标题时返回 0，紧缩合并自然不触发。
        let blocks = parse_markdown("# 函件标题\n正文。\n<!-- [附件] -->\n## 表一");
        assert_eq!(body_heading_max_level(&blocks), 0);
    }

    #[test]
    fn parses_gfm_table_as_a_single_block() {
        let blocks = parse_markdown(
            "# 附件测试\n| 序号 | 名称 | 说明 |\n| --- | --- | --- |\n| 1 | 甲 | 较长说明 |\n| 2 | 乙 | 另一说明 |",
        );
        assert!(
            matches!(&blocks[1], MarkdownBlock::Table { rows, .. } if rows.len() == 3 && rows[0].len() == 3)
        );
    }

    /// 分隔行里的冒号是列对齐，随表格一起带给导出器与预览。
    #[test]
    fn table_keeps_column_alignment_from_the_separator_row() {
        let blocks = parse_markdown(
            "| 序号 | 名称 | 金额 |
| :--- | :---: | ---: |
| 1 | 甲 | 12 |",
        );
        let MarkdownBlock::Table { rows, aligns } = &blocks[0] else {
            panic!("应当解析为表格：{blocks:?}");
        };
        assert_eq!(rows.len(), 2, "分隔行不进正文");
        assert_eq!(
            aligns,
            &[ColumnAlign::Left, ColumnAlign::Center, ColumnAlign::Right]
        );

        // 不写冒号就是 Auto，交给智能列宽判定。
        let blocks = parse_markdown(
            "| 甲 | 乙 |
| --- | --- |
| 1 | 2 |",
        );
        let MarkdownBlock::Table { aligns, .. } = &blocks[0] else {
            panic!("应当解析为表格");
        };
        assert_eq!(aligns, &[ColumnAlign::Auto, ColumnAlign::Auto]);
    }

    #[test]
    fn parses_image_reference_as_own_block() {
        let blocks = parse_markdown(
            "# 标题\n正文段落。\n![扫描件](images/20260809_120000_扫描件.png)\n结尾段落。",
        );
        assert!(matches!(
            &blocks[2],
            MarkdownBlock::Image { alt, src }
                if alt.as_str() == "扫描件" && src.as_str() == "images/20260809_120000_扫描件.png"
        ));
        // 图片行独立成块，不并入相邻段落。
        assert!(matches!(&blocks[1], MarkdownBlock::Paragraph(_)));
        assert!(matches!(&blocks[3], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn image_parsing_ignores_inline_and_malformed() {
        // 段内出现的 `![..]` 不被当作图片块。
        let blocks = parse_markdown("文中出现 ![图标](x.png) 引用");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
        // 残缺语法不匹配。
        let blocks = parse_markdown("![缺括号](images/a.png");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
        // src 含空白的引用不匹配（与 images::image_refs 约束一致）。
        let blocks = parse_markdown("![图](images/my file.png)");
        assert!(matches!(&blocks[0], MarkdownBlock::Paragraph(_)));
    }

    #[test]
    fn exports_all_three_formats() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = DraftInput::default();
        input.profile.issuing_unit = "某某单位".into();
        input.profile.recipient = "某某部门".into();
        input.profile.responsible_unit = "办公室".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "010-12345678".into();
        let markdown =
            "# 关于开展测试工作的函\n\n某某部门：\n\n现就有关事项函告如下。\n\n特此函告。";
        let files = export_all(
            temp.path(),
            &input,
            markdown,
            &ExportSelection::default(),
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|path| path.metadata().unwrap().len() > 0));
        let document_dir = files[0].parent().unwrap();
        assert!(files.iter().all(|path| path.parent() == Some(document_dir)));
        assert!(
            files
                .iter()
                .all(|path| { path.file_stem() == document_dir.file_name() })
        );
        assert!(document_dir.join("gonghan-gwa.cls").exists());

        let docx_path = files
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "docx"))
            .unwrap();
        let file = std::fs::File::open(docx_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut xml = String::new();
        use std::io::Read;
        archive
            .by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("关于开展测试工作的函"));
    }

    #[test]
    fn markdown_export_survives_missing_image_refs() {
        // 图片引用指向不存在的文件时，md 导出照常成功（copy_refs 跳过缺失文件）。
        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let selection = ExportSelection {
            markdown: true,
            docx: false,
            tex: false,
            overwrite: true,
        };
        let files = export_all(
            temp.path(),
            &input,
            "# 标题\n\n正文。\n\n![图](images/不存在的.png)",
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].extension().is_some_and(|ext| ext == "md"));
        assert_eq!(
            std::fs::read_to_string(&files[0]).unwrap(),
            "# 标题\n\n正文。\n\n![图](images/不存在的.png)"
        );
    }

    #[test]
    fn overwrite_reuses_the_same_paths() {
        let temp = tempfile::tempdir().unwrap();
        let input = DraftInput::default();
        let markdown = "# 关于开展测试工作的函\n\n正文。\n";
        let selection = ExportSelection {
            markdown: true,
            docx: false,
            tex: false,
            overwrite: true,
        };
        let first = export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        let second = export_all(
            temp.path(),
            &input,
            markdown,
            &selection,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_eq!(first, second);

        let versioned = ExportSelection {
            overwrite: false,
            ..selection
        };
        let third = export_all(
            temp.path(),
            &input,
            markdown,
            &versioned,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .unwrap();
        assert_ne!(first, third);
        let versioned_dir = third[0].parent().unwrap();
        assert_eq!(third[0].file_stem(), versioned_dir.file_name());
        assert!(
            versioned_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("-2")
        );
    }

    #[test]
    fn document_stem_is_per_kind() {
        let mut input = DraftInput::default();

        // 会议议程：名称 + 会议时间；时间里的半角冒号被替换为下划线，保证 Windows 安全。
        input.kind = TemplateKind::MeetingAgenda;
        input.meeting_time = "2026年8月5日（星期三）14:30".into();
        assert_eq!(
            document_stem(&input, "开展专题研讨会议"),
            "开展专题研讨会议-2026年8月5日（星期三）14_30"
        );
        // 会议时间留空时回落名称。
        input.meeting_time = "  ".into();
        assert_eq!(
            document_stem(&input, "开展专题研讨会议"),
            "开展专题研讨会议"
        );

        // 白头件：白头 + 名称 + 时间戳，时间戳为 12 位数字。
        input.kind = TemplateKind::WhitePaper;
        let stem = document_stem(&input, "关于解决XXX问题的请示");
        let (prefix, rest) = stem.split_once('-').unwrap();
        assert_eq!(prefix, "白头");
        assert!(rest.starts_with("关于解决XXX问题的请示-"));
        assert_timestamp(rest.rsplit('-').next().unwrap());

        // 电话通知：只有“电话通知” + 时间戳，不带名称。
        input.kind = TemplateKind::PhoneNotice;
        let stem = document_stem(&input, "关于召开会议的通知");
        assert!(stem.starts_with("电话通知-"));
        assert_timestamp(stem.strip_prefix("电话通知-").unwrap());

        // 普通公文：普通公文 + 名称 + 时间戳。
        input.kind = TemplateKind::PlainDocument;
        let stem = document_stem(&input, "工作安排");
        assert!(stem.starts_with("普通公文-工作安排-"));
        assert_timestamp(stem.rsplit('-').next().unwrap());
    }

    #[test]
    fn official_letter_stem_uses_full_document_number() {
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.department_code = "某政函".into();
        input.profile.document_number = "12".into();
        input.date = "2026年8月6日".into();
        let stem = document_stem(&input, "关于开展测试工作的函");
        let timestamp = stem.rsplit('-').next().unwrap();
        assert_eq!(
            stem.trim_end_matches(&format!("-{timestamp}")),
            "某政函〔2026〕12号-关于开展测试工作的函"
        );
        assert_timestamp(timestamp);
    }

    #[test]
    fn official_letter_without_serial_falls_back_to_code() {
        // 草稿期尚未编发文序号：只带机关代字，仍保留名称与时间戳。
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        input.profile.department_code = "某政函".into();
        let stem = document_stem(&input, "关于开展测试工作的函");
        assert!(stem.starts_with("某政函-关于开展测试工作的函-"));

        // 代字与序号都缺失：回落“公函”。
        let mut input = DraftInput::default();
        input.kind = TemplateKind::OfficialLetter;
        let stem = document_stem(&input, "关于开展测试工作的函");
        assert!(stem.starts_with("公函-关于开展测试工作的函-"));
    }

    fn assert_timestamp(value: &str) {
        assert_eq!(value.len(), 12, "时间戳应为 12 位：{value}");
        assert!(value.chars().all(|character| character.is_ascii_digit()));
    }

    /// 把正文按"完整括号"切段：全角 `（…）`、半角 `(…)` 或方头括号 `【…】`
    /// 配对出现的整段归为"括号内"，其余归为"括号外"；括号不配对（缺左或缺右）时按普通文本处理。
    /// 仅供测试内对照：生产代码走的是 `inline_segments`（保留 Markdown `**…**` 加粗语义）。
    fn split_parenthesized(text: &str) -> Vec<(&str, bool)> {
        let mut segments = Vec::new();
        let mut normal_start = 0usize;
        for (start, end) in parenthesized_ranges(text) {
            if normal_start < start {
                segments.push((&text[normal_start..start], false));
            }
            segments.push((&text[start..end], true));
            normal_start = end;
        }
        if normal_start < text.len() {
            segments.push((&text[normal_start..], false));
        }
        segments
    }

    #[test]
    fn splits_complete_parentheses_only() {
        assert_eq!(
            split_parenthesized("现就（有关事项）函告如下。"),
            [
                ("现就", false),
                ("（有关事项）", true),
                ("函告如下。", false)
            ]
        );
        assert_eq!(
            split_parenthesized("使用(a)半角与（b）全角。"),
            [
                ("使用", false),
                ("(a)", true),
                ("半角与", false),
                ("（b）", true),
                ("全角。", false)
            ]
        );
        // 不配对的括号按普通文本处理。
        assert_eq!(
            split_parenthesized("缺少（右括号"),
            [("缺少（右括号", false)]
        );
        // 嵌套括号整体算一个括号段。
        assert_eq!(
            split_parenthesized("测试（含（内层）内容）完毕"),
            [
                ("测试", false),
                ("（含（内层）内容）", true),
                ("完毕", false)
            ]
        );
        assert_eq!(
            split_parenthesized("正文【特别说明】继续（办理要求）。"),
            [
                ("正文", false),
                ("【特别说明】", true),
                ("继续", false),
                ("（办理要求）", true),
                ("。", false)
            ]
        );
        assert_eq!(
            split_parenthesized("混合（外层【内层】内容）结束"),
            [
                ("混合", false),
                ("（外层【内层】内容）", true),
                ("结束", false)
            ]
        );
    }

    #[test]
    fn normalizes_quote_styles_and_directions() {
        assert_eq!(
            normalize_chinese_quotes(r#"他说："第一句"，又说：“第二句“。"#),
            "他说：“第一句”，又说：“第二句”。"
        );
        assert_eq!(
            normalize_chinese_quotes("外层「文字『内层』文字」"),
            "外层“文字‘内层’文字”"
        );
        // 英文单词内部的撇号使用中文右单引号，但不参与成对引号方向切换。
        assert_eq!(normalize_chinese_quotes("don't 'quote'"), "don’t ‘quote’");
    }

    #[test]
    fn inline_bold_and_parentheses_can_cross_each_other() {
        assert_eq!(
            inline_segments("正文（**重点**内容）和**【加粗说明】**。"),
            [
                InlineSegment {
                    text: "正文".into(),
                    bold: false,
                    parenthesized: false,
                },
                InlineSegment {
                    text: "（".into(),
                    bold: false,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "重点".into(),
                    bold: true,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "内容）".into(),
                    bold: false,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "和".into(),
                    bold: false,
                    parenthesized: false,
                },
                InlineSegment {
                    text: "【加粗说明】".into(),
                    bold: true,
                    parenthesized: true,
                },
                InlineSegment {
                    text: "。".into(),
                    bold: false,
                    parenthesized: false,
                },
            ]
        );
    }

    #[test]
    fn collects_attachment_names_from_heading_and_inline() {
        // 规范写法：每个附件标记后的 `#` 是正式标题。
        let blocks = parse_markdown(
            "# 测试函\n<!-- [正文] -->\n正文。\n<!-- [附件] -->\n# 统计表\n内容。\n<!-- [附件] -->\n# 说明材料\n内容。",
        );
        assert_eq!(attachment_names(&blocks), ["统计表", "说明材料"]);
        // 历史格式在读取期规范化，附件清单结果保持不变。
        let blocks = parse_markdown("# 测试函\n<!-- [附件] -->\n# 附件1：汇总表\n内容。");
        assert_eq!(attachment_names(&blocks), ["汇总表"]);
        // 没有名称的附件不入列。
        let blocks = parse_markdown(
            "# 测试函\n<!-- [附件] -->\n内容。\n<!-- [附件] -->\n# 说明材料\n内容。",
        );
        assert_eq!(attachment_names(&blocks), ["说明材料"]);
    }
}
