//! 交给 Typst 模板的数据结构。字段名与 `assets/typst/gongwen.typ` 一一对应，
//! 改名要两边一起改；序列化时 `snake_case` 的字段转成模板习惯的 `kebab-case`。

use serde::Serialize;

use crate::typst_engine::FontFamilies;

/// 行内片段。模板按 `t` / `g` / `w` 区分三种：文字、中西文间隙、空占位。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Run {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<String>,
    /// 加粗。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub b: bool,
    /// 括号内容排楷体四号。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub k: bool,
    /// 花脸稿标注：`del` / `add`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub m: Option<&'static str>,
    /// 会议纪要段内「一是……。」：楷体，字号随正文（三号）。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub h: bool,
    /// 中西文间隙（pt）：对应 xeCJK 的 CJKecglue。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub g: Option<f64>,
    /// 空占位宽度（em）：预览版的文号、日期留空。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<f64>,
}

impl Run {
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self {
            t: Some(text.into()),
            ..Self::default()
        }
    }

    pub(crate) fn placeholder(em: f64) -> Self {
        Self {
            w: Some(em),
            ..Self::default()
        }
    }

    pub(crate) fn gap(pt: f64) -> Self {
        Self {
            g: Some(pt),
            ..Self::default()
        }
    }
}

pub(crate) type Runs = Vec<Run>;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Title {
    pub lines: Vec<Runs>,
    /// 横向压缩比（1 = 不压）。
    pub scale: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "k", rename_all = "kebab-case")]
pub(crate) enum Block {
    Par {
        runs: Runs,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
    },
    Heading {
        level: u8,
        runs: Runs,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
    },
    /// 紧缩风格：标题（含编号与句号）与紧随正文接成一段。
    Compact {
        level: u8,
        head: Runs,
        runs: Runs,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
    },
    Aligned {
        align: &'static str,
        runs: Runs,
    },
    Table(Table),
    Image {
        src: String,
        /// 图题（图 1 题名）：排在图下方；没有图题（alt 为空）时不编号。
        #[serde(skip_serializing_if = "Option::is_none")]
        caption: Option<Caption>,
    },
    /// 红头呈批件：从这里起不进首页窄栏（第一个表格 / 图片之前）。
    Barrier,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TableColumn {
    /// `fr`（按比例分内容宽）或 `em`（定宽，按表格四号字的 em）。
    pub kind: &'static str,
    pub v: f64,
    pub align: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TableCell {
    pub runs: Runs,
    pub align: &'static str,
    #[serde(skip_serializing_if = "is_one")]
    pub colspan: usize,
    #[serde(skip_serializing_if = "is_one")]
    pub rowspan: usize,
    /// 居中格按分词换行后的各行。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<Runs>>,
    /// 居中格横向压缩比。
    #[serde(skip_serializing_if = "is_unit_scale")]
    pub scale: f64,
    /// 姓名格：2 字加空、4 字压缩，由模板排。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 序号表的分组行（`（一）大标题`）：整格换楷体，字号不变。
    #[serde(skip_serializing_if = "is_false")]
    pub kai: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_one(value: &usize) -> bool {
    *value == 1
}

fn is_unit_scale(value: &f64) -> bool {
    (*value - 1.0).abs() < f64::EPSILON
}

/// 图表题注：标签「表 N」「图 N」由模板按 number 排黑体，题名 runs 排仿宋。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Caption {
    pub number: usize,
    pub runs: Runs,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Table {
    /// 文档内唯一，模板用它挂「表尾」标签判断表格是否跨页。
    pub id: String,
    /// 表题（表 1 题名）：排在表格上方，跨页不重复。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<Caption>,
    pub cols: Vec<TableColumn>,
    /// 表头行数（至少 1）：`rows` 的前几行进 `table.header`，续页整组重复。
    #[serde(rename = "header-rows")]
    pub header_rows: usize,
    /// 每行只列出要排的格（被合并掉的格不出现）。
    pub rows: Vec<Vec<TableCell>>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Attachment {
    pub label: String,
    pub landscape: bool,
    pub title: Title,
    pub blocks: Vec<Block>,
}

/// 领导阅件的版式参数（字号、行距在模板里按这一项切换）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Leader {
    /// 正文段落整体加粗。
    pub bold: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Header {
    /// 发文机关全称（红头）。
    pub issuing: String,
    /// 份号 / 文号行右侧的发文字号；电话通知没有这一行，为 `None`。
    pub number: Option<Runs>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JointRow {
    pub left: Runs,
    pub right: Option<Runs>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "k", rename_all = "kebab-case")]
pub(crate) enum Closing {
    /// 函稿 / 电话通知：11cm 盒子靠右，单位与日期在盒内居中。
    Letter { unit: Runs, date: Runs },
    /// 联合发文模式 1：两列并列。
    Joint {
        rows: Vec<JointRow>,
        gaps: bool,
        date: Runs,
        #[serde(rename = "date-column")]
        date_column: Option<u8>,
    },
    /// 白头件、红头呈批件：右侧留签字空间。
    Room {
        units: Vec<Runs>,
        #[serde(rename = "unit-width-mm")]
        unit_width_mm: f64,
        date: Runs,
    },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RecordRow {
    pub unit: String,
    pub contact: String,
    pub phone: String,
}

/// 版记。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Record {
    #[serde(rename = "copies-to")]
    pub copies_to: Option<Runs>,
    #[serde(rename = "print-copies")]
    pub print_copies: u32,
    pub rows: Vec<RecordRow>,
    /// 联合发文模式 1 的版记（多行、\arraystretch 1.15）。
    pub joint: bool,
}

/// 会议纪要名单的一行（出席 / 列席 / 请假）。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RosterRow {
    pub label: &'static str,
    pub names: Vec<String>,
    /// 每行 6 格对齐排；有名字放不进一格时为假，改连排。
    pub grid: bool,
}

/// 会议纪要版记：分送、抄送两行（可缺），细线，印发机关与印发日期。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MinutesRecord {
    pub distribute: Option<Runs>,
    #[serde(rename = "copies-to")]
    pub copies_to: Option<Runs>,
    #[serde(rename = "print-copies")]
    pub print_copies: u32,
    #[serde(rename = "printing-unit")]
    pub printing_unit: Runs,
    #[serde(rename = "printing-date")]
    pub printing_date: Runs,
}

/// 会议纪要的版头、名单与版记。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Minutes {
    /// 纪要标志（红头文字）。
    pub heading: String,
    /// 编号「〔2026〕3号」。
    pub number: Runs,
    pub roster: Vec<RosterRow>,
    pub record: MinutesRecord,
}

/// 红头呈批件首页承办区。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Red {
    pub rows: Vec<RecordRow>,
    #[serde(rename = "cols-mm")]
    pub cols_mm: [f64; 3],
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Doc {
    /// `letter` / `phone` / `plain` / `whitepaper` / `redapproval` / `agenda` / `minutes`。
    pub kind: &'static str,
    pub duplex: bool,
    /// 整份横排（普通公文的横页选项）：横向 A4、左侧装订，附件随之横排。
    pub landscape: bool,
    /// 领导阅件（普通公文的大字版）：正文小二宋体、行距 31.85 磅；`None` 为常规版式。
    pub leader: Option<Leader>,
    pub probe: bool,
    pub fonts: FontFamilies,
    pub security: Option<Runs>,
    pub header: Option<Header>,
    pub title: Title,
    pub recipient: Option<Runs>,
    pub body: Vec<Block>,
    pub summary: Vec<Runs>,
    pub attachments: Vec<Attachment>,
    pub closing: Option<Closing>,
    pub record: Option<Record>,
    /// 份号逐份编制时每份的份号；空表示只排一份（份号印 `01`）。
    pub copies: Vec<String>,
    pub red: Option<Red>,
    pub minutes: Option<Minutes>,
    pub phone_record: Option<crate::models::PhoneRecordMetadata>,
    pub phone_record_contact: Option<RecordRow>,
}
