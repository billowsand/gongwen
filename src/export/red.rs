//! 红头版记度量：三线表列宽、呈批件分页与落款宽度。
//!
//! 由 src/export/mod.rs 拆分而来：本文件是模块 `export::red`，与其它子模块共享
//! `export` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use super::docx;
use super::title;
use crate::export::{MarkdownBlock, MarkdownSection, plain_text};

/// 承办区「电话」栏宽：`电话：` 3 em + 半角号码约 6.5 em。
pub(crate) const RED_RECORD_PHONE_TWIPS: usize = 3_040;

/// 承办区「联系人」栏宽：`联系人：` 4 em + 姓名 3 em（姓名一律归一到 3 字宽）。
pub(crate) const RED_RECORD_CONTACT_TWIPS: usize = 2_240;

/// 承办区「承办单位」栏宽：版心减去另两栏，`承办单位：` 5 em 之后还余约 6 em。
/// 三栏里只有这一栏宽度可变，所以把全部余量都给它，超出时再横向压缩。
pub(crate) const RED_RECORD_UNIT_TWIPS: usize =
    RED_RECORD_TOTAL_TWIPS - RED_RECORD_PHONE_TWIPS - RED_RECORD_CONTACT_TWIPS;

/// 承办区总宽 = 版心 156 mm。
pub(crate) const RED_RECORD_TOTAL_TWIPS: usize = docx::TABLE_CONTENT_WIDTH_TWIPS;

/// 三号字一个全角字宽 = 16 pt = 320 缇。
pub(crate) const RED_RECORD_EM_TWIPS: usize = 320;

/// 栏间留白：内容压缩后正好铺满整栏时会与下一栏的红色标签贴在一起，
/// 因此靠左的两栏可用宽度比栏宽少一个字，保证栏与栏之间始终看得出间隔。
pub(crate) const RED_RECORD_GUTTER_TWIPS: usize = RED_RECORD_EM_TWIPS;

/// 承办单位栏可用宽度（扣掉栏间留白）。
pub(crate) const RED_RECORD_UNIT_USABLE_TWIPS: usize =
    RED_RECORD_UNIT_TWIPS - RED_RECORD_GUTTER_TWIPS;

/// 联系人栏可用宽度（扣掉栏间留白）。
pub(crate) const RED_RECORD_CONTACT_USABLE_TWIPS: usize =
    RED_RECORD_CONTACT_TWIPS - RED_RECORD_GUTTER_TWIPS;

/// 电话栏靠右贴版心右缘，与左边的间隔由联系人栏的留白提供，自身不再让宽，
/// 这样 `010-12345678` 这样的常见号码正好排得下、不必压缩。
pub(crate) const RED_RECORD_PHONE_USABLE_TWIPS: usize = RED_RECORD_PHONE_TWIPS;

/// 首页正文/标题栏宽 100 mm，三号字每行 17 个全角字（100 / 5.644 = 17.7）。
pub(crate) const RED_APPROVAL_NARROW_CHARS: usize = 17;

/// 承办区某一栏的横向压缩比（百分数，100 = 原宽）。
///
/// 承办单位一律不许换行：先按 `usable_twips` 量出自然宽度，放得下就原样排，
/// 放不下才按比例压窄字形——字高不变，只收窄字宽，与标题压缩同一套做法。
pub(crate) fn red_record_scale_percent(text: &str, usable_twips: usize) -> usize {
    // display_units 以半角为单位，半角字占半个全角字宽。
    let natural = title::display_units(text) * RED_RECORD_EM_TWIPS / 2;
    if natural <= usable_twips || natural == 0 || usable_twips == 0 {
        return 100;
    }
    ((usable_twips as f64 / natural as f64) * 100.0)
        .floor()
        .clamp(30.0, 100.0) as usize
}

/// DOCX 红头呈批件首页正文栏能排下的估算行数。
///
/// 首页版心 225 mm，扣掉红头预留的 55 mm、标题、呈报领导各占的行，以及底部
/// 承办区（每条一行），余下就是正文可用行数。TeX 端已改为真实盒高判定，
/// 并按剩余基线连续流排；编辑预览则简化为白头件。这里仅保留给 DOCX 的估算。
pub(crate) fn red_approval_wrap_lines(title_lines: usize, responsible_rows: usize) -> usize {
    14usize
        .saturating_sub(title_lines)
        .saturating_sub(responsible_rows)
        .max(4)
}

/// 落款右侧留给签字的空间：签名写在落款单位名的右边。白头件与红头呈批件同宽，
/// 对应类文件里的 `\WhitePaperSignatureRoom` / `\RedApprovalSignatureRoom`。
pub(crate) const SIGNATURE_ROOM_MM: f32 = 40.0;

/// 同上，换算成缇（Word 用）：40 mm = 2268 缇。两者一致由单测把关。
pub(crate) const SIGNATURE_ROOM_TWIPS: usize = 2_268;

/// 落款单位块的宽度，以三号字的全角字宽（em）计。
///
/// 成文日期要落在「落款单位 + 右侧签字空间」这一整段的正中，所以得先知道
/// 单位块有多宽。少于 5 字的单位会被分散对齐到 5 字宽（见 `units::spread_gap`），
/// 按分散后的宽度算；多个单位取最宽的一个。
pub(crate) fn red_signature_unit_width_em(units: &[String]) -> f32 {
    units
        .iter()
        .map(|unit| {
            if crate::units::spread_gap(unit).is_some() {
                5.0
            } else {
                title::display_units(unit) as f32 / 2.0
            }
        })
        .fold(0.0f32, f32::max)
}

/// 同上，换算成缇（Word 用）。1 em = 三号字 16 pt = 320 缇。
pub(crate) fn red_signature_unit_width_twips(units: &[String]) -> usize {
    (red_signature_unit_width_em(units) * RED_RECORD_EM_TWIPS as f32).round() as usize
}

/// 同上，换算成毫米（LaTeX 用）。
///
/// 必须写成绝对长度：这条 `\setlength` 在导言区执行，那里的字号不是三号，
/// 直接写 `em` 会按导言区的字号折算，成文日期就会偏出居中位置。
pub(crate) fn red_signature_unit_width_mm(units: &[String]) -> f32 {
    red_signature_unit_width_em(units) * RED_RECORD_EM_TWIPS as f32 / 1440.0 * 25.4
}

/// DOCX 红头呈批件正文的版面估算。TeX 端按动态内容区连续流排，不依赖这里。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RedApprovalBodyMetrics {
    /// 正文（不含附件）按首页 100 mm 窄栏估算占多少行；表格与图片不计入。
    pub lines: usize,
    /// 正文区第一个表格/图片之前累计的行数；正文没有表格图片时为 `None`。
    pub lines_before_float: Option<usize>,
}

impl RedApprovalBodyMetrics {
    /// 正文是否已经延续到第二页。决定落款页要不要标「（此页无正文）」——正文
    /// 只有首页那点内容时落款另起一页并标注，正文本来就跨页时落款紧随正文。
    pub(crate) fn reaches_second_page(&self, wrap_lines: usize) -> bool {
        self.lines_before_float.is_some() || self.lines > wrap_lines
    }

    /// 第一个表格/图片是否需要强制换页。红头呈批件首页右侧是批示栏，
    /// `\parshape` 只能收窄段落文本、管不到表格与图片，它们会按整幅版心排版
    /// 并压过批示栏，因此落在首页的就推到第二页。
    pub(crate) fn float_needs_page_break(&self, wrap_lines: usize) -> bool {
        self.lines_before_float
            .is_some_and(|before| before <= wrap_lines)
    }
}

/// 扫描一遍正文块，按首页窄栏宽度估算行数并记录第一个表格/图片的位置。
pub(crate) fn red_approval_body_metrics(blocks: &[MarkdownBlock]) -> RedApprovalBodyMetrics {
    let per_line = RED_APPROVAL_NARROW_CHARS * 2;
    let mut section = MarkdownSection::Body;
    let mut metrics = RedApprovalBodyMetrics::default();
    let mut seen_document_title = false;
    for block in blocks {
        if let MarkdownBlock::Marker(next) = block {
            section = *next;
            continue;
        }
        if section != MarkdownSection::Body {
            continue;
        }
        match block {
            MarkdownBlock::Title(_) if !seen_document_title => seen_document_title = true,
            MarkdownBlock::Paragraph(text) => {
                // 段落首行缩进 2 字，折算成 4 个半角单位。
                let units = title::display_units(&plain_text(text)) + 4;
                metrics.lines += units.div_ceil(per_line).max(1);
            }
            MarkdownBlock::Heading(_, text) | MarkdownBlock::ListItem(text) => {
                let units = title::display_units(&plain_text(text));
                metrics.lines += units.div_ceil(per_line).max(1);
            }
            MarkdownBlock::Table { .. } | MarkdownBlock::Image { .. }
                if metrics.lines_before_float.is_none() =>
            {
                metrics.lines_before_float = Some(metrics.lines);
            }
            _ => {}
        }
    }
    metrics
}
