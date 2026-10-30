//! 文种、发文方向、联合行文与模板档案。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::template`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{DraftInput, StyleMode, split_units};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TemplateKind {
    #[default]
    OfficialLetter,
    PhoneNotice,
    PhoneRecord,
    PlainDocument,
    MeetingAgenda,
    MeetingMinutes,
    WhitePaper,
    RedHeadApproval,
    ResearchReport,
}

impl TemplateKind {
    pub const ALL: [Self; 9] = [
        Self::OfficialLetter,
        Self::PhoneNotice,
        Self::PhoneRecord,
        Self::PlainDocument,
        Self::MeetingAgenda,
        Self::MeetingMinutes,
        Self::WhitePaper,
        Self::RedHeadApproval,
        Self::ResearchReport,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OfficialLetter => "公函",
            Self::PhoneNotice => "电话通知",
            Self::PhoneRecord => "电话记录单",
            Self::PlainDocument => "普通公文",
            Self::MeetingAgenda => "会议议程",
            Self::MeetingMinutes => "会议纪要",
            Self::WhitePaper => "白头件（呈批件）",
            Self::RedHeadApproval => "红头呈批件",
            Self::ResearchReport => "研究报告",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::OfficialLetter => "适用于商洽、征求意见、告知、答复等平行行文",
            Self::PhoneNotice => "沿用函稿版式，但不编函号和底部版记",
            Self::PhoneRecord => "记录收到的电话通知，含来电信息、首长批示栏和拟办建议",
            Self::PlainDocument => "无红头、发文单位、主送单位、落款和版记的纯正文公文",
            Self::MeetingAgenda => "适用于会议时间地点、参会人员和议程安排",
            Self::MeetingMinutes => {
                "红头纪要标志与编号，正文后列出席名单，底部分送、抄送、印发版记"
            }
            Self::WhitePaper => "适用于内部情况报告、请示和领导呈批",
            Self::RedHeadApproval => "带发文单位红头、文号和首页批示栏的内部呈批件",
            Self::ResearchReport => {
                "按内置研究报告规范生成 PDF 与 Word，封面按文件类型分项目类、研究类"
            }
        }
    }

    pub(crate) fn uses_letter_layout(self) -> bool {
        matches!(
            self,
            Self::OfficialLetter | Self::PhoneNotice | Self::PlainDocument | Self::RedHeadApproval
        )
    }

    /// 使用“呈报领导—请示正文”业务结构的文种。
    pub(crate) fn is_approval(self) -> bool {
        matches!(self, Self::WhitePaper | Self::RedHeadApproval)
    }

    /// 研究报告使用独立的 Markdown 扩展、封面元数据和排版模板。
    pub(crate) fn is_research(self) -> bool {
        self == Self::ResearchReport
    }

    /// 走公文 Word 排版链（`export::docx`）的文种；研究报告的 Word 另由 mdx
    /// 的 research 转换器生成，封面与 PDF 同一张网格。
    pub(crate) fn uses_official_docx(self) -> bool {
        !self.is_research()
    }

    /// 使用机关代字、发文年份和发文序号的文种。
    pub(crate) fn has_document_number(self) -> bool {
        matches!(self, Self::OfficialLetter | Self::RedHeadApproval)
    }

    /// 只有年份与序号、没有机关代字的编号（会议纪要的「〔2026〕3号」）。
    /// 与 [`Self::has_document_number`] 分开，免得表单和校验去找代字。
    pub(crate) fn has_minutes_number(self) -> bool {
        self == Self::MeetingMinutes
    }

    /// 版头带份号位的文种。
    ///
    /// 公函的份号排在文号左端；会议纪要的份号顶格占版心第 1 行、密级在第 2 行。
    /// 红头呈批件版式里没有份号位；白头件、会议议程、普通公文的抬头只有密级。
    /// 给它们加份号位是改版式，风险另计，没有一并做。
    pub(crate) fn has_copy_numbering(self) -> bool {
        matches!(self, Self::OfficialLetter | Self::MeetingMinutes)
    }

    /// 可以整份改排横页的文种。只有普通公文：它没有红头、版记和落款，横排不牵动
    /// 这些按竖版心标定过的部件。
    pub(crate) fn supports_landscape(self) -> bool {
        matches!(self, Self::PlainDocument)
    }

    /// 可以排成领导阅件（大字版）的文种。只有普通公文：它没有红头、版记，
    /// 改行距与字号不牵动按竖版心标定过的抬头部件。
    pub(crate) fn supports_leader_reading(self) -> bool {
        matches!(self, Self::PlainDocument)
    }

    /// 承办条目数量上限。红头呈批件的承办区在首页底部、每条占一行，条目过多会
    /// 把批示框和正文区压没，因此限定 4 条；其他文种的版记不受此限。
    pub(crate) fn max_responsible_entries(self) -> Option<usize> {
        match self {
            Self::RedHeadApproval => Some(RED_APPROVAL_MAX_RESPONSIBLE),
            _ => None,
        }
    }
}

impl fmt::Display for TemplateKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 带落款稿件的输出状态。预览版保留正式文号和日期位置，但不写入流水号及具体日期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LetterVersion {
    Preview,
    #[default]
    Formal,
}

impl LetterVersion {
    pub const ALL: [Self; 2] = [Self::Preview, Self::Formal];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Preview => "预览版",
            Self::Formal => "正式版",
        }
    }
}

/// 公函发文组织方式。预留枚举以便后续继续增加联合发文模式2等规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum JointIssuanceMode {
    #[default]
    Single,
    Mode1,
}

impl JointIssuanceMode {
    pub const ALL: [Self; 2] = [Self::Single, Self::Mode1];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Single => "单独发文",
            Self::Mode1 => "联合发文模式1",
        }
    }
}

/// 联合发文模式 1 是否按“多家单位并列”排版落款：发文单位多于 1 个时才并列。
/// 只剩 1 个（或 0 个）发文单位时回落单独发文的右侧落款，避免孤零零的单位
/// 挤在联合落款的左列。红头、版记等其他联合逻辑不受此函数影响。
pub(crate) fn is_joint_signature(input: &DraftInput) -> bool {
    input.kind == TemplateKind::OfficialLetter
        && input.profile.joint_issuance_mode == JointIssuanceMode::Mode1
        && split_units(&input.profile.joint_issuing_units).len() > 1
}

/// 函稿的行文范围。内部函使用单位正常名称，外部函优先使用单位维护的外部名称。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CorrespondenceScope {
    #[default]
    Internal,
    External,
}

impl CorrespondenceScope {
    pub const ALL: [Self; 2] = [Self::Internal, Self::External];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Internal => "内部",
            Self::External => "外部",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct JointContact {
    /// 承办单位（成对录入时与联系人一一对应）。旧稿件没有该字段时为空串，
    /// 渲染端按索引回落 `joint_responsible_units` 的第 N 个。
    pub unit: String,
    pub name: String,
    pub phone: String,
}

/// 联合发文承办条目（成对视图）：承办单位 ↔ 联系人 ↔ 电话 一一对应。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JointResponsibleEntry {
    pub unit: String,
    pub name: String,
    pub phone: String,
}

/// 红头呈批件承办条目数量上限：首页底部承办区每条占一行，超过 4 条会把批示框
/// 和首页正文区挤没，界面按此上限封顶，旧稿件超限时由校验提示。
pub const RED_APPROVAL_MAX_RESPONSIBLE: usize = 4;

/// 把联合发文承办单位列表与联系人列表归一化成成对条目。
/// 旧稿件联系人未绑承办单位（`unit` 为空）时，按索引回落
/// `joint_responsible_units` 的第 N 个单位，保持版记配对关系。
/// 行数取两者较大值：旧稿件承办单位可能多于联系人，多出来的单位仍要进版记，
/// 不能因为没配联系人就被丢掉。
/// 完全空白（单位与联系人都没填）的占位行会被过滤，避免版记出现空行。
pub(crate) fn joint_responsible_entries(profile: &TemplateProfile) -> Vec<JointResponsibleEntry> {
    let units = split_units(&profile.joint_responsible_units);
    let count = units.len().max(profile.joint_contacts.len());
    (0..count)
        .map(|index| {
            let contact = profile.joint_contacts.get(index);
            let bound = contact.map_or("", |contact| contact.unit.trim());
            JointResponsibleEntry {
                unit: if bound.is_empty() {
                    units.get(index).cloned().unwrap_or_default()
                } else {
                    bound.to_string()
                },
                name: contact.map_or(String::new(), |contact| contact.name.clone()),
                phone: contact.map_or(String::new(), |contact| contact.phone.clone()),
            }
        })
        .filter(|entry| !entry.unit.trim().is_empty() || !entry.name.trim().is_empty())
        .collect()
}

/// 从成对承办条目同步 `joint_responsible_units` 与 `joint_contacts` 两个字段，
/// 保证两者数量与顺序一致（成对录入的唯一数据源是条目列表）。
pub(crate) fn sync_joint_responsible(
    profile: &mut TemplateProfile,
    entries: &[JointResponsibleEntry],
) {
    profile.joint_responsible_units = entries
        .iter()
        .map(|entry| entry.unit.trim().to_string())
        .filter(|unit| !unit.is_empty())
        .collect::<Vec<_>>()
        .join("、");
    profile.joint_contacts = entries
        .iter()
        .map(|entry| JointContact {
            unit: entry.unit.trim().to_string(),
            name: entry.name.clone(),
            phone: entry.phone.clone(),
        })
        .collect();
}

/// 份号位宽固定两位；超过 99 份才用三位，避免版头忽宽忽窄。
pub(crate) fn format_copy_number(value: u32) -> String {
    if value < 100 {
        format!("{value:02}")
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateProfile {
    pub kind: TemplateKind,
    pub profile_name: String,
    /// 身份引用使用的词条与祖先快照；空时保持旧版本序列化结果。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vocabulary_snapshot: Vec<crate::models::VocabularyEntry>,
    /// 仅函稿使用：内部行文使用正常名称，外部行文优先使用单位外部名称。
    pub correspondence_scope: CorrespondenceScope,
    pub joint_issuance_mode: JointIssuanceMode,
    pub issuing_unit: String,
    pub joint_issuing_units: String,
    pub main_issuing_unit: String,
    pub department_code: String,
    pub recipient: String,
    pub copies_to: String,
    pub responsible_unit: String,
    pub joint_responsible_units: String,
    pub contact_person: String,
    pub contact_phone: String,
    pub joint_contacts: Vec<JointContact>,
    pub reporting_leaders: String,
    pub signing_unit: String,
    /// 白头件落款是否使用简称。勾选后落款单位显示词库简称（未维护简称的
    /// 单位回落规范名称）；显示文本少于 5 字时分散对齐到 5 字宽。白头件和红头呈批件生效。
    pub use_short_name_for_signature: bool,
    pub security_level: String,
    pub security_period: String,
    /// 指人专办：勾选后在密级后空一个全角空格并以黑体标注“指人专办”。
    pub special_handling: bool,
    /// 发文字号中的年份。旧稿未保存该字段时由成文日期兼容推导。
    pub document_year: String,
    pub document_number: String,
    pub meeting_location: String,
    pub letter_version: LetterVersion,
    /// 正文排版风格：正常 / 全局紧缩 / 节内紧缩。
    /// 公函、电话通知、白头件、红头呈批件、会议议程均生效。
    pub style_mode: StyleMode,
    /// 函稿按双面方式排版页码：奇数页靠右、偶数页靠左。
    pub duplex_printing: bool,
    /// 逐份编制份号。默认关闭，关闭时版头固定排 01，与从前一致。
    ///
    /// 份数不单独设置——就是版记里那个「共印 N 份」（主送＋抄送＋承办），
    /// 本来就是这份文要印几份，再让人填一遍只会两处对不上。
    pub number_copies: bool,
    /// 整份横页排版（只对普通公文生效，见 [`TemplateKind::supports_landscape`]）：
    /// 横向 A4、左侧装订，正文与附件一律横排。关闭时不写入快照，旧稿校验值不变。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub landscape: bool,
    /// 领导阅件（只对普通公文生效，见 [`TemplateKind::supports_leader_reading`]）：
    /// 呈阅领导的大字版，正文与层级标题改小二、宋体，行距 31.85 磅；主标题与表格
    /// 不变。关闭时不写入快照，旧稿校验值不变。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub leader_reading: bool,
    /// 领导阅件的正文段落整体加粗（行内加粗随之不再另行区分）；不勾不加粗。
    /// 只在 `leader_reading` 打开时生效。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub leader_reading_bold: bool,
}

impl Default for TemplateProfile {
    fn default() -> Self {
        Self {
            kind: TemplateKind::OfficialLetter,
            profile_name: "默认配置".into(),
            vocabulary_snapshot: Vec::new(),
            correspondence_scope: CorrespondenceScope::Internal,
            joint_issuance_mode: JointIssuanceMode::Single,
            issuing_unit: String::new(),
            joint_issuing_units: String::new(),
            main_issuing_unit: String::new(),
            department_code: String::new(),
            recipient: String::new(),
            copies_to: String::new(),
            responsible_unit: String::new(),
            joint_responsible_units: String::new(),
            contact_person: String::new(),
            contact_phone: String::new(),
            joint_contacts: Vec::new(),
            reporting_leaders: String::new(),
            signing_unit: String::new(),
            use_short_name_for_signature: false,
            // 密级是所有文稿类型的必要填报项目：默认机密、保密期限 20 年。
            security_level: "机密".into(),
            security_period: "20年".into(),
            special_handling: false,
            document_year: String::new(),
            document_number: String::new(),
            meeting_location: String::new(),
            letter_version: LetterVersion::Formal,
            style_mode: StyleMode::Normal,
            duplex_printing: false,
            number_copies: false,
            landscape: false,
            leader_reading: false,
            leader_reading_bold: false,
        }
    }
}

impl TemplateProfile {
    pub(crate) fn for_kind(kind: TemplateKind) -> Self {
        Self {
            kind,
            ..Default::default()
        }
    }
}
