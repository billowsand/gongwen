//! 稿件输入/输出结构与稿件状态。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::draft`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{CorrespondenceScope, TemplateKind, TemplateProfile};
use crate::print::{ColorMode, Duplex, Scaling};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 正文排版风格。全局紧缩在整个正文区选最深标题；节内紧缩则以
/// `##` 为边界，分别选出每节最深的标题。被选中的标题与紧随正文合并为一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StyleMode {
    #[default]
    Normal,
    /// 与旧版“紧缩”完全相同，只改为更明确的名称。
    Compact,
    SectionCompact,
}

impl StyleMode {
    pub const ALL: [Self; 3] = [Self::Normal, Self::Compact, Self::SectionCompact];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Normal => "正常",
            Self::Compact => "全局紧缩",
            Self::SectionCompact => "节内紧缩",
        }
    }
}

/// 稿件生命周期状态。归档是唯一终态：归档后标题、日期、正文等一律不可再改，
/// 供以后查阅；PDF 附件独立于稿件行，归档后仍可增删。
/// `New`（新建）是历史遗留状态：旧版首次保存落为“新建”，现已在创建与迁移时统一
/// 升为“草稿”，不再产生（枚举保留仅为解析旧库与旧 ZIP 包）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ManuscriptStatus {
    #[default]
    New,
    Draft,
    Published,
    Archived,
}

impl ManuscriptStatus {
    pub const ALL: [Self; 4] = [Self::New, Self::Draft, Self::Published, Self::Archived];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::New => "新建",
            Self::Draft => "草稿",
            Self::Published => "发布",
            Self::Archived => "归档",
        }
    }

    /// 状态迁移规则：草稿→发布/归档；发布→草稿（退回修改）/归档；归档无出边。
    /// `新建→草稿` 保留兼容旧数据，正常创建路径已在入库时统一升为草稿。
    pub(crate) fn can_transition(self, target: Self) -> bool {
        match self {
            Self::New => matches!(target, Self::Draft),
            Self::Draft => matches!(target, Self::Published | Self::Archived),
            Self::Published => matches!(target, Self::Draft | Self::Archived),
            Self::Archived => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DraftInput {
    pub kind: TemplateKind,
    pub title_hint: String,
    pub date: String,
    pub date_is_auto: bool,
    pub meeting_time: String,
    pub attendees: String,
    pub profile: TemplateProfile,
    pub research: ResearchMetadata,
    #[serde(skip_serializing_if = "PhoneRecordMetadata::is_empty")]
    pub phone_record: PhoneRecordMetadata,
    #[serde(skip_serializing_if = "MinutesMetadata::is_empty")]
    pub minutes: MinutesMetadata,
}

/// 接听电话的原始记录要素；空值不写入旧稿快照，保持版本校验值兼容。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhoneRecordMetadata {
    pub institution: String,
    pub caller_unit: String,
    pub caller_phone: String,
    pub caller_person: String,
    pub call_time: String,
    pub suggestion: String,
}

impl PhoneRecordMetadata {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// 会议纪要独有的要素；空值不写入旧稿快照，保持版本校验值兼容。
///
/// 会议时间、地点、出席、抄送、编号年份与序号、印发日期复用 `DraftInput` 与
/// `TemplateProfile` 里的已有字段，这里只放别的文种没有的东西。人名单位一律
/// 顿号分隔，与 `attendees`、`copies_to` 同一写法。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MinutesMetadata {
    /// 纪要标志（红头文字）。空时取「发文单位＋会议纪要」。
    pub heading: String,
    /// 列席人员。
    pub nonvoting: String,
    /// 请假人员。
    pub absent: String,
    /// 版记「分送」的人名。
    pub distribute: String,
    /// 版记印发机关。空时取发文单位。
    pub printing_unit: String,
}

impl MinutesMetadata {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// 研究报告封面元数据。模型只接收正文，不得回写这些字段。
///
/// 字段名称和默认值与 mdx research frontmatter 保持一致；导出时由程序生成
/// frontmatter，正文编辑器中不保存这段元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResearchMetadata {
    /// 版式主题与配色独立于文件类型，随稿件快照保存。
    #[serde(skip_serializing_if = "ResearchTemplate::is_default")]
    pub template: ResearchTemplate,
    #[serde(skip_serializing_if = "ResearchPalette::is_default")]
    pub palette: ResearchPalette,
    pub security: String,
    pub security_years: String,
    pub file_type: String,
    pub file_number: String,
    pub version: String,
    pub institution: String,
    pub date: String,
    /// 封面标识行，印在文种下方：项目编号、期号、课题编号或原文出处，按文件类型填写。
    pub ident: String,
    /// 封面署名行，印在落款上方：课题组、编译审校等。项目类封面不印（那里是阶段条）。
    pub byline: String,
    /// 外文翻译的原文题名，印在中文题名下方。
    pub original_title: String,
    /// 原始 BibTeX 文件名，仅供界面显示；正文引用的数据随稿件快照保存。
    pub bibliography_name: String,
    pub bibliography_content: String,
}

impl Default for ResearchMetadata {
    fn default() -> Self {
        Self {
            template: ResearchTemplate::default(),
            palette: ResearchPalette::default(),
            security: "公开".into(),
            security_years: String::new(),
            file_type: "研究报告".into(),
            file_number: String::new(),
            version: "V1.0".into(),
            institution: String::new(),
            date: Local::now().format("%Y年%-m月").to_string(),
            ident: String::new(),
            byline: String::new(),
            original_title: String::new(),
            bibliography_name: String::new(),
            bibliography_content: String::new(),
        }
    }
}

/// 研究报告的版式主题；旧稿件没有字段时沿用经典版式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResearchTemplate {
    #[default]
    Classic,
    Terminal,
}

impl ResearchTemplate {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::Classic
    }
    pub const ALL: [Self; 2] = [Self::Classic, Self::Terminal];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Classic => "经典研究报告",
            Self::Terminal => "研究终端",
        }
    }
}

/// 研究终端的阅读与打印配色，经典模板忽略此设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResearchPalette {
    #[default]
    Dark,
    Bright,
}

impl ResearchPalette {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::Dark
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Dark => "暗场阅读",
            Self::Bright => "白图打印",
        }
    }
}

impl Default for DraftInput {
    fn default() -> Self {
        Self {
            kind: TemplateKind::OfficialLetter,
            title_hint: String::new(),
            date: Local::now().format("%Y年%-m月%-d日").to_string(),
            date_is_auto: true,
            meeting_time: String::new(),
            attendees: String::new(),
            profile: TemplateProfile::default(),
            research: ResearchMetadata::default(),
            phone_record: PhoneRecordMetadata::default(),
            minutes: MinutesMetadata::default(),
        }
    }
}

impl DraftInput {
    /// 本篇是否整份横排：勾了横页且文种支持。换成别的文种时勾选保留但不生效。
    pub(crate) fn landscape(&self) -> bool {
        self.kind.supports_landscape() && self.profile.landscape
    }

    /// 本篇的密级与保密期限，不分文档类型。
    ///
    /// 公文的密级在版式要素里，研究报告的在封面元数据里。稿件库的列表、筛选和
    /// 详情都要显示密级，各自去猜取哪个字段，研究报告就会永远显示空白——所以
    /// 统一从这里取。
    pub(crate) fn security_marking(&self) -> (&str, &str) {
        if self.kind.is_research() {
            (
                self.research.security.trim(),
                self.research.security_years.trim(),
            )
        } else {
            (
                self.profile.security_level.trim(),
                self.profile.security_period.trim(),
            )
        }
    }

    /// 会议纪要版记的印发机关：表单填了就用，否则取发文单位。
    pub(crate) fn minutes_printing_unit(&self) -> &str {
        let explicit = self.minutes.printing_unit.trim();
        if explicit.is_empty() {
            self.profile.issuing_unit.trim()
        } else {
            explicit
        }
    }

    pub(crate) fn uses_external_unit_names(&self) -> bool {
        self.kind == TemplateKind::OfficialLetter
            && self.profile.correspondence_scope == CorrespondenceScope::External
    }

    /// 发文年份优先使用独立元数据；旧稿回退到成文日期开头的四位年份。
    pub(crate) fn document_year(&self) -> String {
        let explicit = self.profile.document_year.trim();
        if !explicit.is_empty() {
            return explicit.to_string();
        }
        let date = self.date.trim();
        let year = date.chars().take(4).collect::<String>();
        if year.len() == 4 && year.chars().all(|ch| ch.is_ascii_digit()) {
            year
        } else {
            String::new()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSelection {
    pub markdown: bool,
    pub docx: bool,
    /// PDF（Typst 排版）。旧配置里这一栏叫 `tex`（当年出的是 TeX 源文件与 PDF）。
    #[serde(alias = "tex")]
    pub pdf: bool,
    /// Typst 包：模板、数据与插图打成 zip，装有常见中文字体的机器可用 typst CLI 编译。
    /// 与 PDF 分开勾选——它只准备数据，不依赖本机字体。
    pub typst: bool,
    /// 反复导出同一篇稿件时覆盖同名文件，而不是不断生成 `-2`、`-3` 副本。
    pub overwrite: bool,
}

/// 打印对话框记住的选择。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintConfig {
    /// 上次用的打印机；清单里没有它（被删了、换了机器）就退回系统默认。
    pub last_printer: String,
    /// 按打印机名记住的双面、颜色、纸张等偏好。打印机被删或改名时自然失效，
    /// 不报错；旧配置没有这一项时按空表读。
    pub printers: BTreeMap<String, PrinterPrefs>,
}

/// 一台打印机记住的打印偏好。选中这台打印机时用来预填对话框（不算「用户动过」）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrinterPrefs {
    pub duplex: Option<Duplex>,
    pub color: Option<ColorMode>,
    /// 纸张 / 纸盒的驱动编号或 CUPS 关键字，逐台打印机自己的。
    pub paper: Option<String>,
    pub tray: Option<String>,
    pub scaling: Scaling,
    /// 手动双面第二轮（打反面）是否倒序出纸。出纸面朝下 / 朝上的机器不一样。
    pub manual_reverse: bool,
}

impl Default for ExportSelection {
    fn default() -> Self {
        Self {
            markdown: true,
            docx: true,
            pdf: true,
            typst: false,
            overwrite: false,
        }
    }
}

impl ExportSelection {
    pub(crate) fn any(&self) -> bool {
        self.markdown || self.docx || self.pdf || self.typst
    }
}

/// 一条审校提示。`span` 有值时，它指向正文里出问题的那一段——审校面板据此把
/// 这条做成可点的，点了就切回 Markdown 视图选中那段。孤行这类排版问题总能定位
/// 到具体段落；文号、密级之类的元数据提示没有对应正文位置，`span` 为 `None`。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewNote {
    pub message: String,
    pub span: Option<std::ops::Range<usize>>,
}

impl ReviewNote {
    pub(crate) fn located(message: String, span: std::ops::Range<usize>) -> Self {
        Self {
            message,
            span: Some(span),
        }
    }
}

impl From<String> for ReviewNote {
    fn from(message: String) -> Self {
        Self {
            message,
            span: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedDraft {
    pub markdown: String,
    pub title: String,
    pub warnings: Vec<ReviewNote>,
    /// 编译时由孤行探针实测出来的提示，与 `warnings` 分开存，正文一改即作废。
    pub proof_warnings: Vec<ReviewNote>,
    /// 这次有没有真的排完版量过。空的 `proof_warnings` 不足以区分“量过没问题”
    /// 和“没编译”，前者要压住粗估提示，后者不能压。
    pub proof_measured: bool,
    pub files: Vec<std::path::PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_print_config_without_printer_preferences_still_loads() {
        let config: PrintConfig =
            serde_json::from_str(r#"{"last_printer":"Office"}"#).expect("旧配置应能读");
        assert_eq!(config.last_printer, "Office");
        assert!(config.printers.is_empty());
    }

    #[test]
    fn printer_preferences_round_trip() {
        let config = PrintConfig {
            last_printer: "Office".to_string(),
            printers: BTreeMap::from([(
                "Office".to_string(),
                PrinterPrefs {
                    duplex: Some(Duplex::ShortEdge),
                    color: Some(ColorMode::Mono),
                    paper: Some("9".to_string()),
                    tray: Some("7".to_string()),
                    scaling: Scaling::Fit,
                    manual_reverse: true,
                },
            )]),
        };
        let json = serde_json::to_string(&config).expect("序列化");
        let back: PrintConfig = serde_json::from_str(&json).expect("反序列化");
        let prefs = &back.printers["Office"];
        assert_eq!(prefs.duplex, Some(Duplex::ShortEdge));
        assert_eq!(prefs.color, Some(ColorMode::Mono));
        assert_eq!(prefs.paper.as_deref(), Some("9"));
        assert!(prefs.manual_reverse);
        assert_eq!(prefs.scaling, Scaling::Fit);
    }
}
