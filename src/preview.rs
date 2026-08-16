//! 公文版式预览：把审校区的 Markdown 连同表单锁定的行文要素，按导出后的样子画出来。
//!
//! 各版式部件已拆分到 `preview/` 子模块（排版基础、红头、文尾、红头呈批件、
//! 正文渲染），根文件保留版式常量、`Metrics` / `PreviewScale` 与测试。

use crate::theme;
use eframe::egui::FontId;

mod header;
mod layout;
mod red;
mod render;
mod tail;

pub(crate) use header::{document_number, header_block, header_unit, is_joint_mode_one};
pub(crate) use layout::{
    append_inline, body_block, clickable, draw, heading_family, indent, is_renderable_paragraph,
    job, layout, line_block, line_galley, place, sheet, single_line, stacked, table_block,
    text_format,
};
pub(crate) use red::{BodyRun, red_approval_print_preview};
pub(crate) use render::{content_block, official_preview};
pub(crate) use tail::{addressee_block, footer_record, signature_block, signature_date};
// test-only names（根文件的测试模块使用）
#[cfg(test)]
pub(crate) use header::security_text;
#[cfg(test)]
pub(crate) use red::{fitting_closing_gap_lines, red_build_print_layout};
#[cfg(test)]
pub(crate) use tail::{signature_seal_mark, signature_unit};

/// Word 的“磅”换算成 egui 的逻辑像素（96dpi）。
const PT: f32 = 96.0 / 72.0;

// 与 export::docx 的常量对应（那边是半磅，这里是磅）。
const BODY_PT: f32 = 16.0; // 三号
const TITLE_PT: f32 = 22.0; // 二号
const TABLE_PT: f32 = 14.0; // 四号
const PAREN_PT: f32 = 14.0; // 括号内容，楷体四号
const LINE_PT: f32 = 28.0; // 固定行距
const TABLE_LINE_PT: f32 = 21.0; // 表格内行距（420 缇）
const INDENT_CHARS: f32 = 2.0; // 首行缩进 2 字
const LIST_INDENT_PT: f32 = 21.0; // 列表项左缩进 420 缇

// 抬头与版记的参数取自 gonghan-gwa.cls。
/// 毫米换算成磅。
const MM: f32 = 72.0 / 25.4;
const HEADER_PT: f32 = 29.0; // 红头字号 \HeaderFontSize
const HEADER_RULE_MM: f32 = 0.53; // 红色反线粗细
const HEADER_RULE_GAP_MM: f32 = 4.0; // 红头与反线之间
const HEADER_MAX_GAP_EM: f32 = 1.0; // 红头最大字距 \HeaderMaxGap
const RECORD_PT: f32 = 14.0; // 版记四号
/// 联系电话列宽：标签 5em + 11 位半角数字 5.5em + 0.5em 余量。
const RECORD_PHONE_COLUMN_EM: f32 = 11.0;
const CLOSING_GAP_LINES: usize = 3; // 正文、附件概要或“此页无正文”与落款之间通常空 3 行
const RECORD_GAP_MM: f32 = 10.0; // 落款与版记之间 \SignatureRecordGap
const SIGNATURE_WIDTH_MM: f32 = 110.0; // 落款块宽 11cm
const JOINT_COLUMN_MM: f32 = 72.0; // 联合发文落款每列宽
const JOINT_ROW_GAP_MM: f32 = 45.0; // 联合发文落款行间公章空档
const JOINT_DATE_GAP_MM: f32 = 6.0; // 联合发文落款与成文日期之间
const WHITE_PAPER_BLANK_LINES: f32 = 10.0; // 白头件密级后空 10 行
/// 预览版留白占位：规格 §3.3 统一 1em 宽。
const PREVIEW_PLACEHOLDER: &str = "\u{2003}";

// A4 版心：页宽 210mm，左边距 1587 缇、右边距 1474 缇。
const PAGE_PT: f32 = 595.28;
const PAGE_HEIGHT_PT: f32 = 841.89;
const MARGIN_LEFT_PT: f32 = 79.35;
const MARGIN_RIGHT_PT: f32 = 73.70;
const MARGIN_TOP_PT: f32 = 52.0;

/// 缩放后的版式尺寸，单位都是 egui 逻辑像素。
pub(crate) struct Metrics {
    scale: f32,
    /// 窗格里真正看得见的宽度，用来把纸张居中。
    viewport: f32,
    page: f32,
    page_height: f32,
    content: f32,
    margin_left: f32,
    margin_top: f32,
    line: f32,
}

/// 本帧应该显示的缩放倍率：`zoom` 为 None 时按可用宽度自适应，否则按给定倍率。
/// `available` 必须是调用方在进入滚动区之前量好的宽度：滚动方向上的
/// `available_width()` 是无穷大，拿进去算会把整页放大到上限。
///
/// 单独抽出来是因为调用方要先算出"该显示多大"，才能决定这一帧是重排版面
/// 还是沿用上一次的版面再做一次层变换（见 `draft_page::markdown_render`）。
pub fn fit_scale(available: f32, zoom: Option<f32>) -> f32 {
    match zoom {
        Some(zoom) => zoom,
        // 留出滚动条与外边距，再夹到一个仍然读得清的区间。
        None => ((available - 24.0) / (PAGE_PT * PT)).clamp(0.3, 1.6),
    }
}

/// 本帧的缩放决定。
#[derive(Clone, Copy, Debug, Default)]
pub struct PreviewScale {
    /// 排版倍率；None 表示按可视宽度自适应。
    pub zoom: Option<f32>,
    /// 用来把纸张横向居中的可视宽度。None 表示自己去量 `ui.clip_rect()`。
    ///
    /// 拖动分隔条时调用方会把裁剪矩形按层变换的逆变换预补偿一遍，量出来的宽度
    /// 不是真实窗格宽度；而纸张的左边沿是从布局游标铺开的，游标并不受裁剪矩形
    /// 影响。两者口径不一致，纸张就会偏离中线，所以那种情况下必须由调用方把真
    /// 实宽度递进来。
    pub viewport: Option<f32>,
}

impl PreviewScale {
    /// 常规情形：自己量宽度，按给定倍率（或自适应）排版。
    pub fn zoom(zoom: Option<f32>) -> Self {
        Self {
            zoom,
            viewport: None,
        }
    }
}

impl Metrics {
    fn new(available: f32, zoom: Option<f32>) -> Self {
        let scale = fit_scale(available, zoom);
        Self {
            scale,
            viewport: available,
            page: PAGE_PT * PT * scale,
            page_height: PAGE_HEIGHT_PT * PT * scale,
            content: (PAGE_PT - MARGIN_LEFT_PT - MARGIN_RIGHT_PT) * PT * scale,
            margin_left: MARGIN_LEFT_PT * PT * scale,
            margin_top: MARGIN_TOP_PT * PT * scale,
            line: LINE_PT * PT * scale,
        }
    }

    fn pt(&self, size: f32) -> f32 {
        size * PT * self.scale
    }

    fn mm(&self, size: f32) -> f32 {
        self.pt(size * MM)
    }

    fn font(&self, family: &str, size: f32) -> FontId {
        FontId::new(self.pt(size), theme::official_family(family))
    }
}

// ── 行文要素：抬头、主送、落款、版记 ────────────────────────────────────────
// 版式对齐 gonghan-gwa.cls：函稿/电话通知走 \DocumentHeader，白头件走
// \WhitePaperHeader，会议议程走 \MeetingAgendaHeader，普通公文走 \PlainDocumentHeader。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export;
    use crate::export::MarkdownBlock;
    use crate::models::{
        DraftInput, JointIssuanceMode, LetterVersion, TemplateKind, TemplateProfile,
        VocabularyEntry,
    };
    use crate::units::UnitDisplay;
    use eframe::egui;
    use eframe::egui::Align;

    /// 一份两级单位的词库：厅下面挂一个处，处有简称。
    fn vocabulary() -> Vec<VocabularyEntry> {
        vec![
            VocabularyEntry {
                code: "00".into(),
                canonical: "星海省教育厅".into(),
                abbr: "省教育厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "0001".into(),
                canonical: "教师工作处".into(),
                parent: "00".into(),
                abbr: "教师处".into(),
                ..Default::default()
            },
        ]
    }

    fn draft(kind: TemplateKind) -> DraftInput {
        DraftInput {
            kind,
            date: "2026年8月7日".into(),
            profile: TemplateProfile {
                kind,
                issuing_unit: "星海省教育厅".into(),
                department_code: "星教函".into(),
                document_number: "12".into(),
                security_level: "秘密".into(),
                security_period: "10年".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn security_line_carries_level_period_and_special_handling() {
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(security_text(&input).as_deref(), Some("秘密★10年"));

        input.profile.special_handling = true;
        assert_eq!(
            security_text(&input).as_deref(),
            Some("秘密★10年\u{2003}指人专办")
        );

        // 普通公文不标“指人专办”，与 export::latex::security_commands 一致。
        input.kind = TemplateKind::PlainDocument;
        assert_eq!(security_text(&input).as_deref(), Some("秘密★10年"));

        input.profile.security_level.clear();
        assert_eq!(security_text(&input), None);
    }

    #[test]
    fn document_number_blanks_the_serial_in_preview_version() {
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(document_number(&input), "星教函〔2026〕12 号");

        input.profile.letter_version = LetterVersion::Preview;
        assert_eq!(document_number(&input), "星教函〔2026〕\u{2003} 号");
    }

    #[test]
    fn signature_date_blanks_the_day_in_preview_version() {
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(signature_date(&input), "2026年8月7日");

        input.profile.letter_version = LetterVersion::Preview;
        assert_eq!(signature_date(&input), "2026年8月\u{2003}日");
    }

    #[test]
    fn signature_unit_follows_the_rule_of_each_template() {
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);

        // 公函落款用全称；落款单位留空时回落发文单位。
        let mut input = draft(TemplateKind::OfficialLetter);
        assert_eq!(signature_unit(&input, &display), "星海省教育厅");
        input.profile.signing_unit = "教师工作处".into();
        assert_eq!(signature_unit(&input, &display), "星海省教育厅教师工作处");

        // 电话通知落款用简称，少于 5 字逐字加空格。
        let mut notice = draft(TemplateKind::PhoneNotice);
        notice.profile.signing_unit = "星海省教育厅".into();
        assert_eq!(signature_unit(&notice, &display), "省 教 育 厅");
    }

    #[test]
    fn signature_seal_mark_follows_the_daizhang_option() {
        let input = draft(TemplateKind::OfficialLetter);
        let mut vocabulary = vocabulary();
        assert_eq!(
            signature_seal_mark(&input, &UnitDisplay::new(&vocabulary)),
            None
        );
        vocabulary[0].seal_on_behalf = true;
        assert_eq!(
            signature_seal_mark(&input, &UnitDisplay::new(&vocabulary)),
            Some("（代章）")
        );
    }

    #[test]
    fn closing_gap_prefers_three_lines_and_compresses_before_paginating() {
        let line = 28.0;
        let signature = 84.0;
        let bottom = 500.0;
        assert_eq!(
            fitting_closing_gap_lines(300.0, signature, bottom, line),
            Some(3)
        );
        assert_eq!(
            fitting_closing_gap_lines(345.0, signature, bottom, line),
            Some(2)
        );
        assert_eq!(
            fitting_closing_gap_lines(380.0, signature, bottom, line),
            Some(1)
        );
        assert_eq!(
            fitting_closing_gap_lines(395.0, signature, bottom, line),
            None
        );
    }

    /// 把一帧里的文本按行分组，返回每行的包围盒；空行与占位不产生可见文本。
    fn text_rows(output: &egui::FullOutput) -> Vec<egui::Rect> {
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            if let egui::epaint::Shape::Text(shape) = &clipped.shape {
                let rect = shape.visual_bounding_rect();
                if rect.is_positive() {
                    rects.push(rect);
                }
            }
        }
        rects.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
        let mut rows: Vec<egui::Rect> = Vec::new();
        for rect in rects {
            if let Some(last) = rows.last_mut()
                && (rect.min.y - last.min.y).abs() < 2.0
            {
                last.min.x = last.min.x.min(rect.min.x);
                last.min.y = last.min.y.min(rect.min.y);
                last.max.x = last.max.x.max(rect.max.x);
                last.max.y = last.max.y.max(rect.max.y);
                continue;
            }
            rows.push(rect);
        }
        rows
    }

    #[test]
    fn white_paper_signature_stacks_units_right_aligned_and_spreads_short_abbr() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = draft(TemplateKind::WhitePaper);
        // 多单位 + 使用简称：省教育厅（3 字）、教师处（3 字），均应分散到 5 字宽。
        input.profile.signing_unit = "星海省教育厅、教师工作处".into();
        input.profile.use_short_name_for_signature = true;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(available, 1200.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            signature_block(ui, &metrics, &input, &display);
        });
        let rows = text_rows(&output);
        // 单位两行 + 日期一行（单位间与日期前各空一行不产生文本）。
        assert_eq!(rows.len(), 3, "落款应有两行单位与一行日期：{rows:?}");
        let right = rows.iter().map(|row| row.max.x).collect::<Vec<_>>();
        let max_right = right.iter().cloned().fold(f32::MIN, f32::max);
        for (index, row) in rows.iter().enumerate() {
            assert!(
                (row.max.x - max_right).abs() <= 1.0,
                "第 {index} 行应右对齐：{row:?}，最大右缘 {max_right}"
            );
        }
        // 简称 3 字分散到 5 字宽：行宽应明显大于 3 字自然宽（≈62px）、
        // 而等于 5 字宽（≈103px，按实际字体度量）。
        for row in &rows[..2] {
            let width = row.width();
            assert!(
                width > 80.0 && width < 115.0,
                "3 字简称应分散到 5 字宽：实际 {width:.1}px（{row:?}）"
            );
        }
    }

    #[test]
    fn white_paper_signature_keeps_full_name_single_unit_layout() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = draft(TemplateKind::WhitePaper);
        // 单单位全称 8 字，不分散；仍是 单位、空行、日期 三行。
        input.profile.signing_unit = "星海省教育厅".into();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(available, 1200.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            signature_block(ui, &metrics, &input, &display);
        });
        let rows = text_rows(&output);
        assert_eq!(rows.len(), 2, "单单位应为单位与日期两行：{rows:?}");
        let width = rows[0].width();
        assert!(
            width > 100.0,
            "8 字全称应按自然宽度排：实际 {width:.1}px（{rows:?}）"
        );
    }

    /// 防御性落款辅助布局仍可独立使用；主预览走上面的红头打印分页器。
    #[test]
    fn red_head_approval_signature_helper_stays_right_aligned() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = draft(TemplateKind::RedHeadApproval);
        input.profile.signing_unit = "星海省教育厅、教师工作处".into();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(available, 1200.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            signature_block(ui, &metrics, &input, &display);
        });
        let rows = text_rows(&output);
        // 两个落款单位各一行 + 日期一行；单位间与日期前的空行不产生文本。
        assert_eq!(rows.len(), 3, "两个落款单位都要列出：{rows:?}");
        let red_right = rows.iter().map(|row| row.max.x).fold(f32::MIN, f32::max);
        for row in &rows {
            assert!(
                (row.max.x - red_right).abs() <= 1.0,
                "红头呈批件的辅助落款应保持全部右对齐：{rows:?}"
            );
        }

        // 与同内容的白头件逐行位置一致。
        let mut white = input.clone();
        white.kind = TemplateKind::WhitePaper;
        white.profile.kind = TemplateKind::WhitePaper;
        let white_rows = text_rows(&ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(available, 1200.0),
                )),
                ..Default::default()
            },
            |ui| signature_block(ui, &metrics, &white, &display),
        ));
        assert_eq!(white_rows.len(), rows.len());
        for (red, white) in rows.iter().zip(&white_rows) {
            assert!(
                (red.min.x - white.min.x).abs() <= 1.0 && (red.max.x - white.max.x).abs() <= 1.0,
                "两个文种的简化预览落款应一致：red={rows:?}, white={white_rows:?}"
            );
        }
    }

    #[test]
    fn red_print_preview_keeps_heading_and_following_paragraph_narrow_on_first_page() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = draft(TemplateKind::RedHeadApproval);
        input.profile.reporting_leaders = "张三、李四".into();
        input.profile.signing_unit = "星海省教育厅".into();
        input.profile.joint_responsible_units = "教师工作处".into();
        input.profile.joint_contacts = vec![crate::models::JointContact {
            unit: "教师工作处".into(),
            name: "王五".into(),
            phone: "010-12345678".into(),
        }];
        let first = "为进一步推进服务事项标准化、规范化、便利化，全面掌握各单位年度工作进展，请结合实际报送年度标准化建设情况。报送数据应与服务事项管理系统保持一致。";
        let second = "各单位应当围绕事项清单维护、服务指南规范、线上线下融合和服务效能提升等方面，全面梳理年度工作完成情况，客观反映工作成效、存在问题及下一步安排。".repeat(3);
        let markdown =
            format!("# 关于报送标准化建设情况的函\n\n{first}\n\n## 报送内容\n\n{second}");
        let located = export::parse_markdown_located(&markdown);
        let body = located.iter().collect::<Vec<_>>();
        let title = located
            .iter()
            .find_map(|block| match &block.block {
                MarkdownBlock::Title(text) => Some((export::plain_text(text), block.range.clone())),
                _ => None,
            })
            .unwrap();
        let heading_range = located
            .iter()
            .find_map(|block| match block.block {
                MarkdownBlock::Heading(_, _) => Some(block.range.clone()),
                _ => None,
            })
            .unwrap();
        let second_range = located
            .iter()
            .rev()
            .find_map(|block| match block.block {
                MarkdownBlock::Paragraph(_) => Some(block.range.clone()),
                _ => None,
            })
            .unwrap();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(available, 1200.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| {
            let (layout, _) =
                red_build_print_layout(ui, &metrics, &input, &display, &body, &title, &[]);
            let heading = layout.pages[0]
                .fragments
                .iter()
                .find(|fragment| fragment.range.as_ref() == Some(&heading_range))
                .expect("一级标题应继续排在首页");
            assert!((heading.width - metrics.mm(100.0)).abs() < 0.5);
            let first_part = layout.pages[0]
                .fragments
                .iter()
                .find(|fragment| fragment.range.as_ref() == Some(&second_range))
                .expect("标题后的正文应利用首页剩余空间");
            assert!((first_part.width - metrics.mm(100.0)).abs() < 0.5);
            let continuation = layout.pages[1]
                .fragments
                .iter()
                .find(|fragment| fragment.range.as_ref() == Some(&second_range))
                .expect("同一段的剩余文字应续排到第二页");
            assert!((continuation.width - metrics.mm(156.0)).abs() < 0.5);
        });
    }

    #[test]
    fn red_print_preview_reflows_one_long_paragraph_at_second_page_width() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let metrics = Metrics::new(1000.0, Some(1.0));
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = draft(TemplateKind::RedHeadApproval);
        input.profile.reporting_leaders = "张三、李四".into();
        input.profile.signing_unit = "星海省教育厅".into();
        let paragraph = "为进一步推进服务事项标准化、规范化、便利化，全面掌握各单位年度工作进展，请结合实际报送年度标准化建设情况。报送数据应与服务事项管理系统保持一致，其中涉及系统接口调用、电子凭证共享和跨部门联办的内容，应当一并核实。".repeat(5);
        let markdown = format!("# 关于报送标准化建设情况的函\n\n{paragraph}");
        let located = export::parse_markdown_located(&markdown);
        let body = located.iter().collect::<Vec<_>>();
        let title = located
            .iter()
            .find_map(|block| match &block.block {
                MarkdownBlock::Title(text) => Some((export::plain_text(text), block.range.clone())),
                _ => None,
            })
            .unwrap();
        let paragraph_range = located
            .iter()
            .find_map(|block| match block.block {
                MarkdownBlock::Paragraph(_) => Some(block.range.clone()),
                _ => None,
            })
            .unwrap();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 1200.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| {
            let (layout, _) =
                red_build_print_layout(ui, &metrics, &input, &display, &body, &title, &[]);
            let first = layout.pages[0]
                .fragments
                .iter()
                .find(|fragment| fragment.range.as_ref() == Some(&paragraph_range))
                .expect("长段落应从首页开始");
            let second = layout.pages[1]
                .fragments
                .iter()
                .find(|fragment| fragment.range.as_ref() == Some(&paragraph_range))
                .expect("长段落应续排到第二页");
            assert!((first.width - metrics.mm(100.0)).abs() < 0.5);
            assert!((second.width - metrics.mm(156.0)).abs() < 0.5);
            assert!(second.galley.rows[0].size.x > first.galley.rows[0].size.x * 1.25);
        });
    }

    #[test]
    fn official_red_preview_draws_multiple_fixed_a4_pages() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let mut input = draft(TemplateKind::RedHeadApproval);
        input.profile.reporting_leaders = "张三、李四".into();
        input.profile.signing_unit = "星海省教育厅".into();
        input.profile.joint_responsible_units = "教师工作处".into();
        input.profile.joint_contacts = vec![crate::models::JointContact {
            unit: "教师工作处".into(),
            name: "王五".into(),
            phone: "010-12345678".into(),
        }];
        let paragraph = "为进一步推进服务事项标准化、规范化、便利化，全面掌握各单位年度工作进展，请结合实际报送年度标准化建设情况。".repeat(8);
        let markdown = format!("# 关于报送标准化建设情况的函\n\n{paragraph}");
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 2600.0),
                )),
                ..Default::default()
            },
            |ui| {
                let _ = official_preview(
                    ui,
                    &input,
                    &UnitDisplay::new(&vocabulary()),
                    &markdown,
                    PreviewScale::zoom(Some(0.8)),
                    None,
                    false,
                );
            },
        );
        let metrics = Metrics::new(1000.0, Some(0.8));
        let pages = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.fill == theme::paper::bg() => Some(rect.rect),
                _ => None,
            })
            .filter(|rect| {
                (rect.width() - metrics.page).abs() < 1.0
                    && (rect.height() - metrics.page_height).abs() < 1.0
            })
            .count();
        assert!(
            pages >= 2,
            "长正文应绘制为多张固定 A4 页面，实际 {pages} 张"
        );
    }

    #[test]
    fn joint_issuance_puts_the_main_unit_on_the_red_header() {
        let vocabulary = vocabulary();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = draft(TemplateKind::OfficialLetter);
        input.profile.joint_issuance_mode = JointIssuanceMode::Mode1;
        input.profile.joint_issuing_units = "教师工作处，星海省教育厅".into();

        // 未指定主办单位时取第一个。
        assert_eq!(header_unit(&input, &display), "星海省教育厅教师工作处");

        input.profile.main_issuing_unit = "星海省教育厅".into();
        assert_eq!(header_unit(&input, &display), "星海省教育厅");

        // 单独发文时红头永远是发文单位，不看联合发文字段。
        input.profile.joint_issuance_mode = JointIssuanceMode::Single;
        assert_eq!(header_unit(&input, &display), "星海省教育厅");
    }

    /// 把一帧里画出的文字包络出来，供对齐断言用。
    fn text_bounds(output: &egui::FullOutput) -> egui::Rect {
        let mut bounds = egui::Rect::NOTHING;
        for clipped in &output.shapes {
            if let egui::epaint::Shape::Text(shape) = &clipped.shape {
                bounds = bounds.union(shape.visual_bounding_rect());
            }
        }
        bounds
    }

    #[test]
    fn centered_heading_is_centered_within_the_content_width() {
        // 单行与多行的小标宋标题都要以版心宽居中，单行不能贴左缘。
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        for title in ["重点工作通知", "关于做好二〇二六年重点工作有关事项的通知"]
        {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(available, 1200.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(raw, |ui| {
                line_block(
                    ui,
                    &metrics,
                    title,
                    theme::FONT_BIAOSONG,
                    TITLE_PT,
                    Align::Center,
                );
            });
            let bounds = text_bounds(&output);
            assert!(
                bounds.is_positive(),
                "标题“{title}”应有可见文字：{bounds:?}"
            );
            let center = bounds.min.x + bounds.width() / 2.0;
            let expected = metrics.content / 2.0;
            assert!(
                (center - expected).abs() <= 1.0,
                "标题“{title}”应相对版心居中：实际中心 {center:.1}，期望 {expected:.1}（{bounds:?}）"
            );
        }
    }

    #[test]
    fn official_font_families_do_not_contain_a_separate_latin_font() {
        // 英文、数字随对应的中文字体排：字体族里不再把 Times New Roman 放在最前。
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        // 字体集要跑过一帧才就绪。
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let definitions = ctx.fonts(|fonts| fonts.definitions().clone());
        for family in [
            theme::FONT_FANGSONG,
            theme::FONT_HEITI,
            theme::FONT_KAITI,
            theme::FONT_BIAOSONG,
        ] {
            let list = definitions
                .families
                .get(&egui::FontFamily::Name(family.into()))
                .expect("官方字体族应已注册");
            assert!(
                !list.iter().any(|key| key == "gw-latin"),
                "{family} 字体族不应含单独的西文字体 gw-latin：{list:?}"
            );
        }
    }

    #[test]
    fn image_block_falls_back_to_placeholder_without_panicking() {
        // 文件缺失、路径非法、PDF 三种情况都走占位卡片，且不 panic。
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let available = 1000.0;
        let metrics = Metrics::new(available, Some(1.0));
        for (alt, src) in [
            ("缺失图", "images/20260809_120000_不存在的.png"),
            ("穿越", "../etc/passwd"),
            ("PDF 附件", "images/20260809_120000_扫描件.pdf"),
        ] {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(available, 1200.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(raw, |ui| {
                let mut counters = [0usize; 4];
                content_block(
                    ui,
                    &metrics,
                    &MarkdownBlock::Image {
                        alt: alt.to_string(),
                        src: src.to_string(),
                    },
                    &mut counters,
                    false,
                );
            });
        }
    }

    #[test]
    fn image_block_loads_texture_after_background_decode() {
        // content_block 的 Image 块：后台线程解码完成后，纹理应可加载（Ready）。
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_diag_{}.png", std::process::id());
        let img_path = dir.join(&name);
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            100,
            50,
            image::Rgba([255, 0, 0, 255]),
        ))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
        std::fs::write(&img_path, cursor.into_inner()).unwrap();
        let src = format!("images/{name}");
        let metrics = Metrics::new(1000.0, Some(1.0));
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 1200.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw.clone(), |ui| {
            let mut counters = [0usize; 4];
            content_block(
                ui,
                &metrics,
                &MarkdownBlock::Image {
                    alt: "图".into(),
                    src: src.clone(),
                },
                &mut counters,
                false,
            );
        });
        // 后台线程解码需要时间与后续帧推进；等解码完成后重查。
        std::thread::sleep(std::time::Duration::from_millis(500));
        let uri = format!("bytes://{src}");
        let mut result = ctx.try_load_texture(
            &uri,
            egui::TextureOptions::default(),
            egui::load::SizeHint::default(),
        );
        if matches!(result, Ok(egui::load::TexturePoll::Pending { .. })) {
            let _ = ctx.run_ui(raw, |_| {});
            result = ctx.try_load_texture(
                &uri,
                egui::TextureOptions::default(),
                egui::load::SizeHint::default(),
            );
        }
        let _ = std::fs::remove_file(&img_path);
        match result {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                assert!(texture.size.x > 0.0 && texture.size.y > 0.0);
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => {
                panic!("图片纹理应在多帧后加载完成，但仍 Pending");
            }
            Err(error) => {
                panic!("图片纹理加载失败：{error:?}");
            }
        }
    }

    #[test]
    fn official_preview_loads_image_texture() {
        // 完整走 official_preview：构造含图片的公文，多帧渲染后纹理应可加载。
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_diag2_{}.png", std::process::id());
        let img_path = dir.join(&name);
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            200,
            100,
            image::Rgba([0, 0, 255, 255]),
        ))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
        std::fs::write(&img_path, cursor.into_inner()).unwrap();
        let src = format!("images/{name}");
        let markdown = format!("# 关于测试的函\n\n正文。\n\n![示意图]({src})\n\n特此函告。");
        let input = draft(TemplateKind::OfficialLetter);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 2000.0),
            )),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run_ui(raw.clone(), |ui| {
                let _ = official_preview(
                    ui,
                    &input,
                    &UnitDisplay::new(&[]),
                    &markdown,
                    PreviewScale::default(),
                    None,
                    false,
                );
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let uri = format!("bytes://{src}");
        let result = ctx.try_load_texture(
            &uri,
            egui::TextureOptions::default(),
            egui::load::SizeHint::default(),
        );
        let _ = std::fs::remove_file(&img_path);
        match result {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                assert!(texture.size.x > 0.0 && texture.size.y > 0.0);
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => {
                panic!("official_preview 中图片纹理应加载完成，但仍 Pending");
            }
            Err(error) => {
                panic!("official_preview 中图片纹理加载失败：{error:?}");
            }
        }
    }

    #[test]
    fn official_preview_draws_image_with_finite_size() {
        // 回归测试：含图片的公文在滚动区内渲染时，图片必须绘制为尺寸有限的
        // 带纹理矩形（旧实现默认 ImageFit::Fraction 在滚动区把高算成无穷大，
        // tessellate 后画面损坏，正是用户看不到图片的原因）。
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let dir = crate::images::image_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("gw_diag3_{}.png", std::process::id());
        let img_path = dir.join(&name);
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            200,
            100,
            image::Rgba([0, 255, 0, 255]),
        ))
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
        std::fs::write(&img_path, cursor.into_inner()).unwrap();
        let src = format!("images/{name}");
        let markdown = format!("# 关于测试的函\n\n正文。\n\n![示意图]({src})\n\n特此函告。");
        let input = draft(TemplateKind::OfficialLetter);
        let uri = format!("bytes://{src}");
        let mut states = Vec::new();
        for frame in 0..4 {
            let full = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 2000.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    // 与真实 markdown_render 一致：official_preview 在滚动区内渲染，
                    // 滚动方向的 available_size 是无穷大，正是旧实现的崩溃点。
                    egui::ScrollArea::both().show(ui, |ui| {
                        let _ = official_preview(
                            ui,
                            &input,
                            &UnitDisplay::new(&[]),
                            &markdown,
                            PreviewScale::default(),
                            None,
                            false,
                        );
                    });
                },
            );
            let state = ctx
                .try_load_texture(
                    &uri,
                    egui::TextureOptions::default(),
                    egui::load::SizeHint::default(),
                )
                .map(|poll| match poll {
                    egui::load::TexturePoll::Ready { .. } => "ready".to_string(),
                    egui::load::TexturePoll::Pending { .. } => "pending".to_string(),
                })
                .unwrap_or_else(|e| format!("err({e:?})"));
            let textured_rects = full
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Rect(rect)
                            if rect.fill_texture_id() != egui::TextureId::default()
                    )
                })
                .count();
            // 图片矩形必须尺寸有限（旧实现把高算成无穷大，tessellate 后画面损坏）。
            let finite_tex_rects = full
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(
                        &clipped.shape,
                        egui::epaint::Shape::Rect(rect)
                            if rect.fill_texture_id() != egui::TextureId::default()
                                && rect.rect.width().is_finite()
                                && rect.rect.height().is_finite()
                                && rect.rect.height() > 0.0
                                && rect.rect.height() < 10000.0
                    )
                })
                .count();
            states.push(format!(
                "frame{frame}:{state}:tex_rects={textured_rects}:finite_tex_rects={finite_tex_rects}"
            ));
            if frame == 0 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }
        let _ = std::fs::remove_file(&img_path);
        // 文件存在期间（循环内），图片应真正绘制：出现尺寸有限的带纹理矩形。
        // （旧实现把图片高度算成无穷大，tessellate 后画面损坏，正是用户看不到图的原因。）
        assert!(
            states
                .iter()
                .any(|s| s.contains(":finite_tex_rects=") && !s.ends_with("finite_tex_rects=0")),
            "图片应绘制尺寸有限的带纹理矩形：{states:?}"
        );
    }
}
