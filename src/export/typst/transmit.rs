//! 公函电子加密传输单（《传发任务单》）：数据构造与 PDF 排版入口。
//!
//! 传输单是公函的随行单据，**每家接收单位一份**，只出 PDF、不用签字——签字由同批
//! 导出的纸质《公文签收记录单》承担。版式照用户提供的 Word 模板复刻：题名红色小标宋，
//! 主表五列（分组标签 / 小节标签 / 取值 / 副标签 / 副取值），分组标签竖排；表下是
//! 机要表两行与「保密单位 … 收到 / 发出」末行。
//!
//! 题名说的是「这次传的是什么任务」（涉密信息加密传发任务单、公开信息明文传发任务单
//! ……），取自设置页的传输任务字典；公文标题印在表内的「标题」一栏。
//!
//! 取值分两类：公函要素里现成的（发文单位、联系人、密级、标题、函号、正文页数）
//! 直接填；只由机要人员掌握或公文里没有的（传字号、签批人、时效要求、备注、收件人）
//! 留白手写——程序不替人编造。
//!
//! **一页是硬要求**：表内的公文标题特别长时那一行会长高，整张单子会挤到第二页。
//! 所以排版走一条收紧阶梯（[`Fit`]）：照模板原样排一版，超过一页就按阶梯收一档
//! 重排，直到排在一页里为止。阶梯是单调的（越靠后越紧），所以从上次成功的档位
//! 接着往下试即可，不必每份从头排。

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::export::element_display;
use crate::models::{DraftInput, FontConfig, TransmitDefaults, TransmitDisposal};
use crate::typst_engine::{self, FontFamilies, TypstJob, TypstOutcome};
use crate::units::UnitDisplay;

/// 主表列宽（毫米）。照用户给的 Word 模板量出来：分组标签 / 小节标签 / 取值 /
/// 副标签 / 副取值。表格比版心宽，模板原样。
pub(crate) const COLUMN_WIDTHS_MM: [f64; 5] = [15.0, 25.0, 51.7, 28.1, 42.7];

/// 机要表列宽（毫米）：发方机要机构 / 空 / 发送人 / 空 / 电话 / 空。
pub(crate) const MACHINE_COLUMN_WIDTHS_MM: [f64; 6] = [35.0, 40.0, 20.0, 25.0, 15.0, 27.4];

/// 主表各行的最小高度（毫米）：照模板 PDF 量出来。行高定得不小于它，
/// 手写栏才有地方落笔。
const ROW_HEIGHTS_MM: [f64; 11] = [
    12.2, 19.3, 11.2, 11.2, 11.2, 12.9, 11.2, 11.1, 11.2, 11.2, 36.2,
];

/// 题名字号的上限（磅）：模板用 36 磅。下限见 [`TITLE_SIZE_MIN_PT`]。
const TITLE_SIZE_MAX_PT: f64 = 36.0;

/// 题名字号的下限（磅）：再小就不是公文题名了。
const TITLE_SIZE_MIN_PT: f64 = 17.0;

/// 题名能排在一行里的版心宽（磅）：156mm。字宽按全角一字一算。
const TITLE_LINE_WIDTH_PT: f64 = 156.0 * 72.0 / 25.4;

/// 备注的字号（磅）与固定行距（磅）：照模板，仿宋小四（12pt）、行距固定 14pt。
const REMARK_SIZE_PT: f64 = 12.0;
const REMARK_LINE_PT: f64 = 14.0;

/// 备注格的净宽（毫米）：跨第 2 至第 5 列，去掉左右留白。
const REMARK_WIDTH_MM: f64 = 25.0 + 51.7 + 28.1 + 42.7 - 2.0;

/// 文字样式。模板里表格文字用黑体，勾选字形用楷体，备注用仿宋。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TransmitStyle {
    /// 黑体：标签与取值。
    Hei,
    /// 楷体：勾选字形（□ / ☑）。
    Kai,
    /// 仿宋：备注（模板的事例就是仿宋小四）。
    Fs,
}

/// 表格里的对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TransmitAlign {
    Left,
    Center,
}

/// 一段文字。勾选框（□ / ☑）写进文字里，排版时换成线画的框。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TransmitRun {
    pub text: String,
    pub style: TransmitStyle,
}

impl TransmitRun {
    fn hei(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: TransmitStyle::Hei,
        }
    }

    fn kai(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: TransmitStyle::Kai,
        }
    }

    /// 仿宋：备注。
    fn fs(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: TransmitStyle::Fs,
        }
    }
}

/// 竖直对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TransmitValign {
    /// 居中：模板里绝大多数格子。
    #[default]
    Center,
    /// 顶对齐：备注格——文字在上，底下留出手写 / 续写的空白。
    Top,
}

/// 格内的排版细节。默认按全局的表格字号与行距排。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub(crate) struct TransmitLayout {
    /// 竖直对齐方式。
    pub valign: TransmitValign,
    /// 本格的行距（磅）：0 表示跟全局的走。备注格用模板的固定行距。
    pub lead_pt: f64,
    /// 首行缩进（字）：模板里「通知收件人情况」等格子缩进一字。
    pub indent_em: f64,
    /// 顶对齐的格子里，内容下面补的空白（毫米）。
    pub tail_mm: f64,
}

/// 主表的一格。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TransmitCell {
    /// 横跨的列数。
    pub colspan: usize,
    /// 竖跨的行数（1 = 不与下一行合并）。
    pub rowspan: usize,
    /// 格里的文字。
    pub runs: Vec<TransmitRun>,
    /// 对齐方式。
    pub align: TransmitAlign,
    /// 取值为空时留出的手写横线长度（全角字数）；0 表示不留。
    pub blank_chars: usize,
    /// 本格撑起的最小高度（毫米）；0 表示按内容自然高度。
    pub strut_mm: f64,
    /// 竖排：每字一行（分组标签）。
    pub vertical: bool,
    /// 本格的字号（磅）；0 表示跟全局的表格字号走。
    pub size_pt: f64,
    /// 格内的排版细节（对齐、行距、缩进、底部留白）。
    pub layout: TransmitLayout,
}

/// 传输单的全部数据。字段名与 `assets/typst/transmit.typ` 一一对应。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TransmitDoc {
    pub fonts: FontFamilies,
    /// 题名：传输任务名（如「涉密信息加密传发任务单」）。公文标题不在这里，
    /// 它印在表内的「标题」一栏。
    pub title: String,
    /// 题名字号（磅）。
    pub title_size_pt: f64,
    /// 题名字距（磅）：负值把字压窄，用来把长题名塞进一行。
    pub title_track_pt: f64,
    /// 题名前后的空白（磅）。
    pub title_gap_pt: f64,
    /// 表格文字字号（磅）。
    pub label_size_pt: f64,
    /// 段落行距（磅）。
    pub lead_pt: f64,
    /// 单元格上下留白（毫米）。
    pub cell_inset_y_mm: f64,
    /// 上下页边距（毫米）。
    pub top_margin_mm: f64,
    pub bottom_margin_mm: f64,
    /// 传字：印在右上角「传　　号」里，号码留白手写，不加方括号与横线。
    pub serial_prefix: String,
    /// 保密负责单位：印在末行，直接替掉模板上的「【保密单位】」占位。空则保留占位。
    pub security_unit: String,
    /// 主表列宽（毫米）。
    pub column_widths_mm: Vec<f64>,
    /// 主表的格子，按行序排列。
    pub cells: Vec<TransmitCell>,
    /// 机要表列宽（毫米）。
    pub machine_column_widths_mm: Vec<f64>,
}

/// 一份传输单对应的一家接收单位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Receiver {
    /// 单位显示名（空 → 留白手写）。
    pub unit: String,
    /// 收件人显示名（空 → 留白手写）。
    pub person: String,
    /// 收件人电话（空 → 留白手写）。
    pub phone: String,
}

/// 接收单位清单：主送在前、抄送在后，每家单位一份传输单。
///
/// 单位按显示值去重（两个编码指向同一个外部名称时只出一份）。收件人优先取表单
/// 登记的收件人；没登记时，该单位在标准词库里只登记了一个人就用他；有多个或
/// 一个都没有就留白，由人手写。
pub(crate) fn receivers(input: &DraftInput, display: &UnitDisplay) -> Vec<Receiver> {
    let scoped = display.for_input(input);
    let display = &scoped;
    let external = input.uses_external_unit_names();
    let mut receivers: Vec<Receiver> = Vec::new();
    for unit in crate::models::receiving_units(&input.profile) {
        let name = display.full_name_for(&unit, external).trim().to_string();
        if name.is_empty() || receivers.iter().any(|seen| seen.unit == name) {
            continue;
        }
        let (person, phone) = match display.receiver_contact(&input.profile, &unit) {
            Some((person, phone)) => (display.person_name(&person), phone),
            None => (String::new(), String::new()),
        };
        receivers.push(Receiver {
            unit: name,
            person,
            phone,
        });
    }
    receivers
}

/// 版面档位：一页放不下就按这个顺序收紧。
///
/// 收紧顺序按「对版面的破坏从小到大」排：先动自己加的题名空白，再压题名字号与
/// 字距，再压表格字号（表格里的公文标题变长时，它既占行数又占行高，压字号最
/// 见效），再压行高与备注，最后才动页边距。每一档都比上一档更紧，所以阶梯单调，
/// 排出来的页数只会越来越少。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fit {
    /// 题名字号（磅）。
    title_pt: f64,
    /// 题名字距（磅）：负值把字压窄。
    title_track_pt: f64,
    /// 题名前后的空白（磅）。
    title_gap_pt: f64,
    /// 表格文字字号（磅）。
    label_pt: f64,
    /// 表格里「标题」一栏的字号（磅）：模板本来就是 12pt，比别处小一号。
    doc_title_pt: f64,
    /// 备注的字号（磅）：模板是仿宋小四（12pt）。
    remark_pt: f64,
    /// 段落行距（磅）。
    lead_pt: f64,
    /// 单元格上下留白（毫米）。
    inset_y_mm: f64,
    /// 各行最小行高的系数。
    row_scale: f64,
    /// 备注行高（毫米）。
    remark_mm: f64,
    /// 上下页边距（毫米）。
    top_mm: f64,
    bottom_mm: f64,
}

/// 收紧阶梯：从最宽松（照模板原样）到最紧。
const FITS: [Fit; 7] = [
    Fit {
        title_pt: 36.0,
        title_track_pt: 0.0,
        title_gap_pt: 11.0,
        label_pt: 14.0,
        doc_title_pt: 12.0,
        remark_pt: 12.0,
        lead_pt: 6.0,
        inset_y_mm: 3.0,
        row_scale: 1.0,
        remark_mm: 36.2,
        top_mm: 37.0,
        bottom_mm: 35.0,
    },
    Fit {
        title_pt: 36.0,
        title_track_pt: 0.0,
        title_gap_pt: 6.0,
        label_pt: 14.0,
        doc_title_pt: 12.0,
        remark_pt: 12.0,
        lead_pt: 6.0,
        inset_y_mm: 3.0,
        row_scale: 1.0,
        remark_mm: 36.2,
        top_mm: 37.0,
        bottom_mm: 35.0,
    },
    Fit {
        title_pt: 32.0,
        title_track_pt: -0.2,
        title_gap_pt: 4.0,
        label_pt: 14.0,
        doc_title_pt: 12.0,
        remark_pt: 12.0,
        lead_pt: 6.0,
        inset_y_mm: 3.0,
        row_scale: 1.0,
        remark_mm: 33.0,
        top_mm: 36.0,
        bottom_mm: 34.0,
    },
    Fit {
        title_pt: 28.0,
        title_track_pt: -0.3,
        title_gap_pt: 3.0,
        label_pt: 13.0,
        doc_title_pt: 11.5,
        remark_pt: 11.5,
        lead_pt: 5.0,
        inset_y_mm: 2.5,
        row_scale: 0.97,
        remark_mm: 28.0,
        top_mm: 34.0,
        bottom_mm: 31.0,
    },
    Fit {
        title_pt: 24.0,
        title_track_pt: -0.5,
        title_gap_pt: 2.0,
        label_pt: 12.5,
        doc_title_pt: 11.0,
        remark_pt: 11.0,
        lead_pt: 4.5,
        inset_y_mm: 2.5,
        row_scale: 0.94,
        remark_mm: 24.0,
        top_mm: 32.0,
        bottom_mm: 28.0,
    },
    Fit {
        title_pt: 20.0,
        title_track_pt: -0.7,
        title_gap_pt: 2.0,
        label_pt: 12.0,
        doc_title_pt: 10.5,
        remark_pt: 10.5,
        lead_pt: 4.0,
        inset_y_mm: 2.0,
        row_scale: 0.9,
        remark_mm: 20.0,
        top_mm: 29.0,
        bottom_mm: 25.0,
    },
    Fit {
        title_pt: 17.0,
        title_track_pt: -1.0,
        title_gap_pt: 2.0,
        label_pt: 11.0,
        doc_title_pt: 10.0,
        remark_pt: 10.0,
        lead_pt: 3.0,
        inset_y_mm: 1.5,
        row_scale: 0.85,
        remark_mm: 15.0,
        top_mm: 26.0,
        bottom_mm: 22.0,
    },
];

/// 按公函要素构造一份传输单的数据。
///
/// `defaults` 是设置页「加密传输单」里的默认填写内容：传字、签批人、时效要求、
/// 发后处理、备注与保密负责单位。留空的仍留白手写。
pub(crate) fn build(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    receiver: &Receiver,
    pages: usize,
    fit: &Fit,
    defaults: &TransmitDefaults,
) -> TransmitDoc {
    let scoped = display.for_input(input);
    let display = &scoped;
    let mut cells: Vec<TransmitCell> = Vec::new();

    // 题名：传输任务名。题名越长字号越小，尽量排在一行里；再长就压字距。
    let title = crate::models::resolve_transmit_task(&input.profile, defaults);
    let title_chars = title.chars().count().max(1) as f64;
    // 标题与字距各留一点余量，别让最后一个字顶到版心边上。
    let one_line_pt = (TITLE_LINE_WIDTH_PT * 0.98 / (title_chars + 0.25))
        .clamp(TITLE_SIZE_MIN_PT, TITLE_SIZE_MAX_PT);
    let title_size_pt = fit.title_pt.min(one_line_pt);
    // 行高系数作用在除备注行外的各行上；备注行有自己的高度（留给机要人员手写）。
    let row = |index: usize| ROW_HEIGHTS_MM[index] * fit.row_scale;

    // 收方：接收单位与收件人。收件人没登记就留白手写。
    cells.push(group("收\n方", 2));
    cells.push(label("单位", row(0)));
    cells.push(value(&receiver.unit, 3, 0.0));
    cells.push(label("收件人", row(1)));
    cells.push(text_or_blank(&receiver.person, 8, 1, 0.0));
    cells.push(label2("联系电话"));
    cells.push(text_or_blank(&receiver.phone, 6, 1, 0.0));

    // 发方：发文单位、签批人、联系人与通知情况。
    cells.push(group("发\n方", 4));
    cells.push(label("单位", row(2)));
    cells.push(value(
        &crate::export::typst::receipt::issuing_unit_text(input, display),
        3,
        0.0,
    ));
    cells.push(label("签批人", row(3)));
    cells.push(signatory_cell(defaults));
    cells.push(label("联系人", row(4)));
    cells.push(text_or_blank(&contact_name(input, display), 6, 1, 0.0));
    cells.push(label2("联系电话"));
    cells.push(text_or_blank(input.profile.contact_phone.trim(), 6, 1, 0.0));
    cells.push(label("通知收件人情况", row(5)));
    cells.push(TransmitCell {
        colspan: 3,
        rowspan: 1,
        runs: box_choice_runs("已通知收件人", false, "未通知收件人", true, &"　".repeat(6)),
        align: TransmitAlign::Left,
        blank_chars: 0,
        strut_mm: 0.0,
        vertical: false,
        size_pt: 0.0,
        layout: TransmitLayout {
            indent_em: 1.0,
            ..Default::default()
        },
    });

    // 涉密信息要素：密级、时效要求、发后处理、信息类型与标题。
    cells.push(group("涉密\n信息\n要素", 4));
    cells.push(label("密级", row(6)));
    cells.push(value(&security_text(input), 1, 0.0));
    cells.push(label2("时效要求"));
    cells.push(text_or_blank(&defaults.urgency, 4, 1, 0.0));
    cells.push(label("发后处理", row(7)));
    cells.push(TransmitCell {
        colspan: 3,
        rowspan: 1,
        // 模板里这一栏勾中的只印一个「√」，不画框。
        runs: match defaults.disposal {
            TransmitDisposal::Destroy => {
                plain_choice_runs("退还", false, "销毁", true, &"　".repeat(9))
            }
            TransmitDisposal::Return => {
                plain_choice_runs("退还", true, "销毁", false, &"　".repeat(9))
            }
        },
        align: TransmitAlign::Left,
        blank_chars: 0,
        strut_mm: 0.0,
        vertical: false,
        size_pt: 0.0,
        layout: TransmitLayout {
            indent_em: 1.0,
            ..Default::default()
        },
    });
    cells.push(label("信息类型", row(8)));
    cells.push(TransmitCell {
        colspan: 3,
        rowspan: 1,
        runs: vec![
            TransmitRun::kai("☑"),
            TransmitRun::hei(format!("纸质文件（{pages} 页）")),
            TransmitRun::hei("　　"),
            TransmitRun::kai("□"),
            TransmitRun::hei("电子文件（＿＿MB）"),
        ],
        align: TransmitAlign::Left,
        blank_chars: 0,
        strut_mm: 0.0,
        vertical: false,
        size_pt: 0.0,
        layout: TransmitLayout {
            indent_em: 1.0,
            ..Default::default()
        },
    });
    cells.push(label("标题", row(9)));
    // 公文标题印在这一栏：它可能很长，字号比别处小一号（模板原样），
    // 也最容易把整张单子挤到第二页——收紧时它先变小。
    cells.push(value(
        &title_and_number(input, markdown),
        3,
        fit.doc_title_pt,
    ));

    // 备注：照模板的事例排——仿宋小四、固定行距，每行一条、序号「1.」「2.」……
    // 没填内容就留出手写的大块空白。
    cells.push(group("备\n注", 1));
    cells.push(remark_cell(&defaults.remark, fit));

    TransmitDoc {
        fonts: empty_families(),
        title,
        title_size_pt,
        title_track_pt: fit.title_track_pt,
        title_gap_pt: fit.title_gap_pt,
        label_size_pt: fit.label_pt,
        lead_pt: fit.lead_pt,
        cell_inset_y_mm: fit.inset_y_mm,
        top_margin_mm: fit.top_mm,
        bottom_margin_mm: fit.bottom_mm,
        serial_prefix: defaults.serial_prefix.trim().to_string(),
        security_unit: defaults.security_unit.trim().to_string(),
        column_widths_mm: COLUMN_WIDTHS_MM.to_vec(),
        cells,
        machine_column_widths_mm: MACHINE_COLUMN_WIDTHS_MM.to_vec(),
    }
}

/// 签批人：默认留空手签（设置里勾「留空手签」），取消勾选才印默认姓名。
/// 留白是供手签的空白，不画横线；其他需要横线的栏位仍由 `blank_value` 处理。
fn signatory_cell(defaults: &TransmitDefaults) -> TransmitCell {
    let name = defaults.signatory.trim();
    if defaults.signatory_blank || name.is_empty() {
        blank_value(3, 0)
    } else {
        value(name, 3, 0.0)
    }
}

/// 二选一的勾选栏：勾中的印「☑」（排版时画成实心带勾的框），没勾的印「□」
/// （排版时画成空框），两项之间隔一段空白。模板里「通知收件人情况」与
/// 「信息类型」两栏都是这个样子。
fn box_choice_runs(
    first: &str,
    first_checked: bool,
    second: &str,
    second_checked: bool,
    gap: &str,
) -> Vec<TransmitRun> {
    let mut runs = Vec::new();
    for (index, (text, checked)) in [(first, first_checked), (second, second_checked)]
        .into_iter()
        .enumerate()
    {
        if index == 1 {
            runs.push(TransmitRun::hei(gap));
        }
        runs.push(TransmitRun::kai(if checked { "☑" } else { "□" }));
        runs.push(TransmitRun::hei(text));
    }
    runs
}

/// 二选一的勾选栏，但勾中的只印一个「√」、不画框——模板的「发后处理」一栏
/// 就是这么写的。
fn plain_choice_runs(
    first: &str,
    first_checked: bool,
    second: &str,
    second_checked: bool,
    gap: &str,
) -> Vec<TransmitRun> {
    let mut runs = Vec::new();
    for (index, (text, checked)) in [(first, first_checked), (second, second_checked)]
        .into_iter()
        .enumerate()
    {
        if index == 1 {
            runs.push(TransmitRun::hei(gap));
        }
        if checked {
            runs.push(TransmitRun::hei("√"));
        } else {
            runs.push(TransmitRun::hei("□"));
        }
        runs.push(TransmitRun::hei(text));
    }
    runs
}

/// 备注格：仿宋小四、固定行距，每行一条、序号「1.」「2.」……
///
/// 内容短时底下补一段空白，把这一行撑到模板给的高度；内容长了就让它长，
/// 不截断。
fn remark_cell(remark: &str, fit: &Fit) -> TransmitCell {
    let items = number_items(remark.trim());
    let text = items.join("\n");
    let size_pt = fit.remark_pt;
    // 模板的备注是仿宋小四、固定行距 14 磅；字号收紧时行距按比例跟着收。
    let lead_pt = (size_pt * (REMARK_LINE_PT / REMARK_SIZE_PT - 1.0)).max(0.5);
    let content_mm = estimate_text_height_mm(&text, size_pt, lead_pt, REMARK_WIDTH_MM);
    let tail_mm = (fit.remark_mm - 2.0 * fit.inset_y_mm - content_mm).max(0.0);
    TransmitCell {
        colspan: 4,
        rowspan: 1,
        runs: if text.is_empty() {
            Vec::new()
        } else {
            vec![TransmitRun::fs(text)]
        },
        align: TransmitAlign::Left,
        blank_chars: 0,
        strut_mm: 0.0,
        vertical: false,
        size_pt,
        layout: TransmitLayout {
            valign: TransmitValign::Top,
            lead_pt,
            indent_em: 0.0,
            tail_mm,
        },
    }
}

/// 备注按模板的事例排成编号条目：每行一条、序号「1.」「2.」…… 已经带序号的
/// 行原样保留，免得出现两个序号。
fn number_items(text: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if has_item_number(line) {
            items.push(line.to_string());
        } else {
            items.push(format!("{}.{}", items.len() + 1, line));
        }
    }
    items
}

/// 行首是不是序号（`1.` / `1、` / `1)` / `1．`）。
fn has_item_number(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_digit() && !matches!(first, '０'..='９') {
        return false;
    }
    matches!(chars.next(), Some('.' | '．' | '、' | ')' | '）'))
}

/// 估算一段文字排出来有多高（毫米）：按格子的净宽折行，行距按固定行距算。
///
/// 只用来给备注格补底部的空白，估得高一点也只是多留一点白，不影响内容。
fn estimate_text_height_mm(text: &str, size_pt: f64, lead_pt: f64, width_mm: f64) -> f64 {
    let width_pt = width_mm * 72.0 / 25.4;
    let mut lines = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut used = 0.0;
        for (index, ch) in line.chars().enumerate() {
            let advance = if ch.is_ascii() {
                size_pt * 0.5
            } else {
                size_pt
            };
            if index > 0 && used + advance > width_pt {
                lines += 1;
                used = 0.0;
            }
            used += advance;
        }
        lines += 1;
    }
    if lines == 0 {
        return 0.0;
    }
    let content_pt = size_pt + (lines - 1) as f64 * (size_pt + lead_pt);
    content_pt * 25.4 / 72.0
}

/// 第 1 列的竖排分组标签。
fn group(text: &str, rowspan: usize) -> TransmitCell {
    TransmitCell {
        colspan: 1,
        rowspan,
        runs: vec![TransmitRun::hei(text)],
        align: TransmitAlign::Center,
        blank_chars: 0,
        strut_mm: 0.0,
        vertical: true,
        size_pt: 0.0,
        layout: TransmitLayout::default(),
    }
}

/// 第 2 列的小节标签。标签撑起本行的高度。
fn label(text: &str, strut_mm: f64) -> TransmitCell {
    TransmitCell {
        colspan: 1,
        rowspan: 1,
        runs: vec![TransmitRun::hei(text)],
        align: TransmitAlign::Center,
        blank_chars: 0,
        strut_mm,
        vertical: false,
        size_pt: 0.0,
        layout: TransmitLayout::default(),
    }
}

/// 第 4 列的副标签（联系电话、时效要求）。
fn label2(text: &str) -> TransmitCell {
    TransmitCell {
        colspan: 1,
        rowspan: 1,
        runs: vec![TransmitRun::hei(text)],
        align: TransmitAlign::Center,
        blank_chars: 0,
        strut_mm: 0.0,
        vertical: false,
        size_pt: 0.0,
        layout: TransmitLayout::default(),
    }
}

/// 取值格。`size_pt` 为 0 时跟全局的表格字号走。
fn value(text: &str, colspan: usize, size_pt: f64) -> TransmitCell {
    TransmitCell {
        colspan,
        rowspan: 1,
        runs: vec![TransmitRun::hei(text)],
        align: TransmitAlign::Center,
        blank_chars: 0,
        strut_mm: 0.0,
        vertical: false,
        size_pt,
        layout: TransmitLayout::default(),
    }
}

/// 取值为空时留出手写横线。
fn text_or_blank(text: &str, blank_chars: usize, colspan: usize, size_pt: f64) -> TransmitCell {
    let text = text.trim();
    if text.is_empty() {
        blank_value(colspan, blank_chars)
    } else {
        value(text, colspan, size_pt)
    }
}

/// 留白待填的格。
fn blank_value(colspan: usize, blank_chars: usize) -> TransmitCell {
    TransmitCell {
        colspan,
        rowspan: 1,
        runs: Vec::new(),
        align: TransmitAlign::Center,
        blank_chars,
        strut_mm: 0.0,
        vertical: false,
        size_pt: 0.0,
        layout: TransmitLayout::default(),
    }
}

/// 版记里的联系人姓名。
fn contact_name(input: &DraftInput, display: &UnitDisplay) -> String {
    let person = input.profile.contact_person.trim();
    if person.is_empty() {
        return String::new();
    }
    display.person_name(person)
}

/// 密级：密级与保密期限都写上，便于收件单位核对。
fn security_text(input: &DraftInput) -> String {
    let (level, period) = element_display::security_parts(input);
    let level = level.trim();
    if level.is_empty() {
        return String::new();
    }
    let period = period.trim();
    if period.is_empty() {
        level.to_string()
    } else {
        format!("{level}（保密期限{period}）")
    }
}

/// 「标题」一栏的取值：公文标题加函号。
fn title_and_number(input: &DraftInput, markdown: &str) -> String {
    let title = crate::export::document_title(input, markdown);
    let number = crate::export::typst::receipt::number_text(input);
    if number.trim().is_empty() {
        title
    } else {
        format!("{title}（{number}）")
    }
}

/// 空的字体角色：排 PDF 时会覆盖。
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

/// 排一份传输单 PDF 写到 `path`，返回排版结果与用上的版面档位下标。
///
/// 档位从 `hint` 开始往下试（阶梯单调，往下只会更紧）；排在一页就停。传 `hint`
/// 是上一份单子用过的档位，同一次导出里后面的单位能直接省掉前面的试排。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_pdf(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    receiver: &Receiver,
    pages: usize,
    hint: usize,
    defaults: &TransmitDefaults,
) -> Result<(TypstOutcome, usize)> {
    let base = crate::storage::config_dir()?;
    write_pdf_with_base(
        path, input, markdown, display, fonts, receiver, pages, hint, defaults, &base,
    )
}

/// 同上，插图等相对路径按 `base_dir` 解析（传输单本身没有插图）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_pdf_with_base(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    fonts: &FontConfig,
    receiver: &Receiver,
    pages: usize,
    hint: usize,
    defaults: &TransmitDefaults,
    base_dir: &Path,
) -> Result<(TypstOutcome, usize)> {
    display
        .for_input(input)
        .ensure_resolved(input)
        .map_err(anyhow::Error::msg)?;
    let set = typst_engine::font_set(fonts)?;
    let start = hint.min(FITS.len() - 1);
    let mut last: Option<(TypstOutcome, usize)> = None;
    for (index, fit) in FITS.iter().enumerate().skip(start) {
        let mut doc = build(input, markdown, display, receiver, pages, fit, defaults);
        doc.fonts = set.families.clone();
        let data = serde_json::to_string(&doc).context("无法序列化传发任务单数据")?;
        let outcome = typst_engine::compile(&TypstJob::transmit(data, base_dir), &set)?;
        if outcome.pages <= 1 {
            std::fs::write(path, &outcome.pdf)
                .with_context(|| format!("无法写入传发任务单 PDF：{}", path.display()))?;
            return Ok((outcome, index));
        }
        last = Some((outcome, index));
    }
    // 收到最紧的档位还是两页：内容实在太多（公文标题极长之类）。
    // 照样把 PDF 写出来，但要在提示里说清楚。
    let (outcome, index) = last.expect("收紧阶梯至少有一档");
    std::fs::write(path, &outcome.pdf)
        .with_context(|| format!("无法写入传发任务单 PDF：{}", path.display()))?;
    Ok((outcome, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TemplateKind, TemplateProfile};

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
        input.profile.copies_to = "丙单位".into();
        input.profile.contact_person = "张三".into();
        input.profile.contact_phone = "0571-88888888".into();
        input.profile.security_level = "秘密".into();
        input.profile.security_period = "1年".into();
        input.date = "2026年9月30日".into();
        input
    }

    /// 取某一格里的文字，方便断言。
    fn texts(doc: &TransmitDoc) -> Vec<String> {
        doc.cells
            .iter()
            .map(|cell| {
                cell.runs
                    .iter()
                    .map(|run| run.text.clone())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect()
    }

    #[test]
    fn receivers_cover_recipients_and_copies_and_dedupe() {
        let input = letter();
        let receivers = receivers(&input, &UnitDisplay::new(&[]));
        let units: Vec<&str> = receivers.iter().map(|r| r.unit.as_str()).collect();
        assert_eq!(units, ["甲单位", "乙单位", "丙单位"]);
    }

    /// 词库里的一个单位与它名下的一个人。
    fn vocabulary_with_one_person() -> Vec<crate::models::VocabularyEntry> {
        vec![
            crate::models::VocabularyEntry {
                code: "U01".into(),
                canonical: "甲单位".into(),
                category: crate::models::VocabularyCategory::Unit,
                ..Default::default()
            },
            crate::models::VocabularyEntry {
                code: "P01".into(),
                canonical: "李四".into(),
                category: crate::models::VocabularyCategory::Person,
                unit: "U01".into(),
                phone: "13800000000".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn receiver_contact_falls_back_to_the_only_person_of_the_unit() {
        // 词库里该单位只登记了一个人：自动带出他与绑定的电话。
        let vocabulary = vocabulary_with_one_person();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = letter();
        input.profile.recipient = "U01".into();
        input.profile.copies_to = String::new();
        let receivers = receivers(&input, &display);
        assert_eq!(receivers.len(), 1);
        assert_eq!(receivers[0].person, "李四");
        assert_eq!(receivers[0].phone, "13800000000");
    }

    #[test]
    fn receiver_contact_is_blank_when_the_unit_has_several_people() {
        // 有两个人时不替人挑：留白手写。
        let mut vocabulary = vocabulary_with_one_person();
        vocabulary.push(crate::models::VocabularyEntry {
            code: "P02".into(),
            canonical: "王五".into(),
            category: crate::models::VocabularyCategory::Person,
            unit: "U01".into(),
            ..Default::default()
        });
        let display = UnitDisplay::new(&vocabulary);
        let mut input = letter();
        input.profile.recipient = "U01".into();
        input.profile.copies_to = String::new();
        let receivers = receivers(&input, &display);
        assert_eq!(receivers.len(), 1);
        assert!(receivers[0].person.is_empty());
        assert!(receivers[0].phone.is_empty());
    }

    #[test]
    fn form_contact_wins_over_the_vocabulary_fallback() {
        let vocabulary = vocabulary_with_one_person();
        let display = UnitDisplay::new(&vocabulary);
        let mut input = letter();
        input.profile.recipient = "U01".into();
        input.profile.copies_to = String::new();
        // 表单里登记的收件人（存的是人的身份引用）与电话优先于词库回落。
        input.profile.receiving_contacts = vec![crate::models::ReceivingContact {
            unit: "U01".into(),
            name: "李四".into(),
            phone: "13900000000".into(),
        }];
        let receivers = receivers(&input, &display);
        assert_eq!(receivers[0].person, "李四");
        assert_eq!(receivers[0].phone, "13900000000");
    }

    #[test]
    fn the_sheet_carries_the_letter_elements_and_leaves_the_rest_blank() {
        let input = letter();
        let receiver = Receiver {
            unit: "甲单位".into(),
            person: "赵六".into(),
            phone: "0571-66666666".into(),
        };
        let doc = build(
            &input,
            "# 关于商洽的函\n\n正文。",
            &UnitDisplay::new(&[]),
            &receiver,
            3,
            &FITS[0],
            &TransmitDefaults::default(),
        );
        // 题名是传输任务名，不是公文标题；公文标题在表内的「标题」一栏。
        assert_eq!(doc.title, "涉密信息加密传发任务单");
        let texts = texts(&doc);
        // 收方：单位与收件人。
        assert!(texts.iter().any(|t| t == "甲单位"));
        assert!(texts.iter().any(|t| t == "赵六"));
        assert!(texts.iter().any(|t| t == "0571-66666666"));
        // 发方：发文单位与联系人。
        assert!(texts.iter().any(|t| t == "某某市民政局"));
        assert!(texts.iter().any(|t| t == "张三"));
        // 涉密信息要素：密级、页数、标题与函号。
        assert!(texts.iter().any(|t| t == "秘密（保密期限1年）"));
        assert!(texts.iter().any(|t| t.contains("3 页")));
        assert!(
            texts
                .iter()
                .any(|t| t == "关于商洽的函（某民函〔2026〕18号）")
        );
        // 默认设置：传字「传」、时效要求「急」、发后处理「销毁」；签批人默认留空手签。
        assert_eq!(doc.serial_prefix, "传");
        assert!(texts.iter().any(|t| t == "急"));
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("□退还") && t.ends_with("√销毁"))
        );
        assert!(doc.security_unit.is_empty());
        let sign_index = doc
            .cells
            .iter()
            .position(|cell| cell.runs.iter().any(|run| run.text == "签批人"))
            .unwrap();
        let sign = &doc.cells[sign_index];
        let after = &doc.cells[sign_index + 1];
        assert_eq!(sign.blank_chars, 0);
        assert_eq!(after.blank_chars, 0);
        assert!(after.runs.is_empty());
    }

    /// 设置页「加密传输单」里填的每一项都要落到单子上。
    #[test]
    fn the_configured_defaults_reach_the_sheet() {
        let input = letter();
        let receiver = Receiver {
            unit: "甲单位".into(),
            person: "赵六".into(),
            phone: "0571-66666666".into(),
        };
        let defaults = TransmitDefaults {
            task: String::new(),
            serial_prefix: "机要".into(),
            signatory: "李四".into(),
            signatory_blank: false,
            urgency: "特急".into(),
            disposal: TransmitDisposal::Return,
            remark: "第一行：加急办理\n第二行：当天回执".into(),
            security_unit: "某某市保密局".into(),
        };
        let doc = build(
            &input,
            "# 关于商洽的函\n\n正文。",
            &UnitDisplay::new(&[]),
            &receiver,
            3,
            &FITS[0],
            &defaults,
        );
        let texts = texts(&doc);
        assert_eq!(doc.serial_prefix, "机要");
        assert_eq!(doc.security_unit, "某某市保密局");
        // 签批人：取消「留空手签」后印姓名。
        let sign_index = doc
            .cells
            .iter()
            .position(|cell| cell.runs.iter().any(|run| run.text == "签批人"))
            .unwrap();
        assert!(texts[sign_index + 1] == "李四");
        // 时效要求、发后处理、备注。
        assert!(texts.iter().any(|t| t == "特急"));
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with("√退还") && t.ends_with("□销毁"))
        );
        assert!(texts.iter().any(|t| t.contains("第一行：加急办理")));
        // 填了备注就不必再留手写高度。
        let remark_index = doc
            .cells
            .iter()
            .position(|cell| cell.runs.iter().any(|run| run.text.contains("第一行")))
            .unwrap();
        assert_eq!(doc.cells[remark_index].strut_mm, 0.0);
    }

    /// 备注按模板的事例排：仿宋小四、每行一条、序号「1.」「2.」……
    #[test]
    fn the_remark_follows_the_examples_in_the_template() {
        let input = letter();
        let receiver = Receiver {
            unit: "甲单位".into(),
            person: String::new(),
            phone: String::new(),
        };
        let defaults = TransmitDefaults {
            remark: "当面交接，不得邮寄。\n限当日回执。".into(),
            ..TransmitDefaults::default()
        };
        let doc = build(
            &input,
            "# 关于商洽的函\n\n正文。",
            &UnitDisplay::new(&[]),
            &receiver,
            3,
            &FITS[0],
            &defaults,
        );
        let remark = doc
            .cells
            .iter()
            .find(|cell| cell.runs.iter().any(|run| run.text.contains("当面交接")))
            .expect("备注格里应有内容");
        // 每行一条，前面带序号。
        assert_eq!(
            remark.runs[0].text,
            "1.当面交接，不得邮寄。\n2.限当日回执。"
        );
        // 仿宋、模板的字号（12pt）与固定行距（14pt）。
        assert_eq!(remark.runs[0].style, TransmitStyle::Fs);
        assert_eq!(remark.size_pt, REMARK_SIZE_PT);
        assert!((remark.layout.lead_pt - 2.0).abs() < 0.01);
        // 顶对齐，底下补空白把行高撑到模板给的高度。
        assert_eq!(remark.layout.valign, TransmitValign::Top);
        assert!(remark.layout.tail_mm > 0.0);
    }

    /// 备注里已经带序号的行原样保留，免得出现两个序号。
    #[test]
    fn the_remark_keeps_the_numbers_it_already_has() {
        assert_eq!(number_items("1.第一条\n2.第二条"), ["1.第一条", "2.第二条"]);
        assert_eq!(number_items("第一条\n2、第二条"), ["1.第一条", "2、第二条"]);
        // 空行不算一条。
        assert_eq!(number_items("第一条\n\n第二条"), ["1.第一条", "2.第二条"]);
        // 数字开头但不是序号的行照旧编号。
        assert_eq!(number_items("2026 年的任务"), ["1.2026 年的任务"]);
    }

    /// 勾了「留空手签」时，签批人那一栏留白，哪怕填了姓名也不印。
    #[test]
    fn the_signatory_stays_blank_when_the_sheet_is_signed_by_hand() {
        let input = letter();
        let receiver = Receiver {
            unit: "甲单位".into(),
            person: String::new(),
            phone: String::new(),
        };
        let defaults = TransmitDefaults {
            signatory: "李四".into(),
            signatory_blank: true,
            ..TransmitDefaults::default()
        };
        let doc = build(
            &input,
            "# 关于商洽的函\n\n正文。",
            &UnitDisplay::new(&[]),
            &receiver,
            3,
            &FITS[0],
            &defaults,
        );
        let sign_index = doc
            .cells
            .iter()
            .position(|cell| cell.runs.iter().any(|run| run.text == "签批人"))
            .unwrap();
        assert_eq!(doc.cells[sign_index + 1].blank_chars, 0);
        assert!(doc.cells[sign_index + 1].runs.is_empty());
    }

    /// 没填默认姓名、但关闭了「留空手签」时，也只留空白，不画横线。
    #[test]
    fn an_empty_signatory_name_does_not_draw_a_line() {
        let input = letter();
        let receiver = Receiver {
            unit: "甲单位".into(),
            person: String::new(),
            phone: String::new(),
        };
        let defaults = TransmitDefaults {
            signatory: "  ".into(),
            signatory_blank: false,
            ..TransmitDefaults::default()
        };
        let doc = build(
            &input,
            "# 关于商洽的函\n\n正文。",
            &UnitDisplay::new(&[]),
            &receiver,
            3,
            &FITS[0],
            &defaults,
        );
        let sign_index = doc
            .cells
            .iter()
            .position(|cell| cell.runs.iter().any(|run| run.text == "签批人"))
            .unwrap();
        assert_eq!(doc.cells[sign_index + 1].blank_chars, 0);
        assert!(doc.cells[sign_index + 1].runs.is_empty());
    }

    /// 题名取自传输任务字典：稿件里选过用稿件的，否则用设置页默认，都没选按密级自动带。
    #[test]
    fn the_title_comes_from_the_transmit_task_dictionary() {
        let mut input = letter();
        let empty = TransmitDefaults::default();
        assert_eq!(
            crate::models::resolve_transmit_task(&input.profile, &empty),
            "涉密信息加密传发任务单"
        );
        input.profile.transmit_task = "公开信息密文传发任务单".into();
        assert_eq!(
            crate::models::resolve_transmit_task(&input.profile, &empty),
            "公开信息密文传发任务单"
        );
        input.profile.transmit_task.clear();
        input.profile.security_level.clear();
        assert_eq!(
            crate::models::resolve_transmit_task(&input.profile, &empty),
            "公开信息明文传发任务单"
        );
        // 设置页选了默认题名，稿件没选时用它。
        let configured = TransmitDefaults {
            task: "公开信息密文传发任务单".into(),
            ..TransmitDefaults::default()
        };
        assert_eq!(
            crate::models::resolve_transmit_task(&input.profile, &configured),
            "公开信息密文传发任务单"
        );
    }

    /// 收紧阶梯必须单调：越往后越紧，页数只会越来越少。
    #[test]
    fn the_fit_ladder_only_gets_tighter() {
        for pair in FITS.windows(2) {
            let (loose, tight) = (&pair[0], &pair[1]);
            assert!(tight.title_pt <= loose.title_pt);
            assert!(tight.label_pt <= loose.label_pt);
            assert!(tight.lead_pt <= loose.lead_pt);
            assert!(tight.inset_y_mm <= loose.inset_y_mm);
            assert!(tight.row_scale <= loose.row_scale);
            assert!(tight.remark_mm <= loose.remark_mm);
            assert!(tight.top_mm <= loose.top_mm);
            assert!(tight.bottom_mm <= loose.bottom_mm);
        }
    }

    /// 样张用的默认填写内容：把设置页能填的都填上，好一次看全单子的样子。
    fn filled_defaults() -> TransmitDefaults {
        TransmitDefaults {
            task: String::new(),
            serial_prefix: "传".into(),
            signatory: "王主任".into(),
            signatory_blank: false,
            urgency: "急".into(),
            disposal: TransmitDisposal::Destroy,
            remark: "本件为涉密材料，请按保密规定办理；\n当面交接，不得邮寄。".into(),
            security_unit: "某某市保密局".into(),
        }
    }

    /// 样张：PDF、模板数据与逐页 PNG 写到 `tmp/typst-samples/transmit/`，
    /// 改模板后目视检查用。依赖本机随包字体，默认忽略：
    ///
    /// ```text
    /// cargo test --locked typst_samples_transmit -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "依赖本机 runtime 字体，手动运行"]
    fn typst_samples_transmit() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/typst-samples/transmit");
        std::fs::create_dir_all(&dir).unwrap();
        let fonts = FontConfig::default();
        let display = UnitDisplay::new(&[]);
        let input = letter();
        let markdown =
            "# 关于商洽公共服务的函\n\n甲单位、乙单位：\n\n现就有关事项函告如下。\n\n特此函告。";
        let receiver = Receiver {
            unit: "某某市公共政务服务与数据管理局".into(),
            person: "李四".into(),
            phone: "0571-88888888".into(),
        };

        let set = typst_engine::font_set(&fonts).unwrap();
        let mut doc = build(
            &input,
            markdown,
            &display,
            &receiver,
            3,
            &FITS[0],
            &filled_defaults(),
        );
        doc.fonts = set.families.clone();
        let data = serde_json::to_string_pretty(&doc).unwrap();
        std::fs::write(dir.join("doc.json"), &data).unwrap();

        let outcome = typst_engine::compile(&TypstJob::transmit(data, &dir), &set).unwrap();
        std::fs::write(dir.join("typst.pdf"), &outcome.pdf).unwrap();
        assert_eq!(outcome.pages, 1, "样张应排在一页里");
        save_page_images(&outcome.pdf, &dir);
    }

    /// 超长公文标题与保密单位也必须排在一页里：标题走收紧阶梯，末行只横向压缩。
    ///
    /// ```text
    /// cargo test --locked typst_samples_transmit_long -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "依赖本机 runtime 字体，手动运行"]
    fn typst_samples_transmit_long() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/typst-samples/transmit-long");
        std::fs::create_dir_all(&dir).unwrap();
        let input = letter();
        let markdown = format!(
            "# 关于开展{}重点工作的通知\n\n甲单位、乙单位：\n\n现就有关事项函告如下。\n\n特此函告。",
            "基层治理与营商环境优化".repeat(4)
        );
        let receiver = Receiver {
            unit: "某某市公共政务服务与数据管理局".into(),
            person: "李四".into(),
            phone: "0571-88888888".into(),
        };
        let mut defaults = filled_defaults();
        defaults.signatory_blank = true;
        defaults.security_unit = "某某市国家保密行政管理部门机要密码传输安全监督管理中心".into();
        let (outcome, index) = write_pdf_with_base(
            &dir.join("typst.pdf"),
            &input,
            &markdown,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &receiver,
            3,
            0,
            &defaults,
            &dir,
        )
        .unwrap();
        println!("超长标题与保密单位用的版面档位：{index}");
        assert_eq!(
            outcome.pages, 1,
            "收紧到底仍应排在一页里（用的档位 {index}）"
        );
        save_page_images(&outcome.pdf, &dir);
    }

    /// 把 PDF 每页渲染成 `page-N.png`（宽 1240 像素，约 150dpi）。
    fn save_page_images(pdf: &[u8], dir: &Path) {
        use hayro::hayro_interpret::{InterpreterSettings, hayro_syntax::Pdf};
        use hayro::{RenderCache, RenderSettings};
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
}
