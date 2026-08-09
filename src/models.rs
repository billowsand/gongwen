use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TemplateKind {
    #[default]
    OfficialLetter,
    PhoneNotice,
    PlainDocument,
    MeetingAgenda,
    WhitePaper,
}

impl TemplateKind {
    pub const ALL: [Self; 5] = [
        Self::OfficialLetter,
        Self::PhoneNotice,
        Self::PlainDocument,
        Self::MeetingAgenda,
        Self::WhitePaper,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::OfficialLetter => "公函",
            Self::PhoneNotice => "电话通知",
            Self::PlainDocument => "普通公文",
            Self::MeetingAgenda => "会议议程",
            Self::WhitePaper => "白头件（呈批件）",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::OfficialLetter => "适用于商洽、征求意见、告知、答复等平行行文",
            Self::PhoneNotice => "沿用函稿版式，但不编函号和底部版记",
            Self::PlainDocument => "无红头、发文单位、主送单位、落款和版记的纯正文公文",
            Self::MeetingAgenda => "适用于会议时间地点、参会人员和议程安排",
            Self::WhitePaper => "适用于内部情况报告、请示和领导呈批",
        }
    }

    pub fn uses_letter_layout(self) -> bool {
        matches!(
            self,
            Self::OfficialLetter | Self::PhoneNotice | Self::PlainDocument
        )
    }
}

impl fmt::Display for TemplateKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 函稿输出状态。预览版保留正式文号和日期位置，但不写入流水号及具体日期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LetterVersion {
    Preview,
    #[default]
    Formal,
}

impl LetterVersion {
    pub const ALL: [Self; 2] = [Self::Preview, Self::Formal];

    pub fn label(self) -> &'static str {
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

    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "单独发文",
            Self::Mode1 => "联合发文模式1",
        }
    }
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

    pub fn label(self) -> &'static str {
        match self {
            Self::Internal => "内部",
            Self::External => "外部",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct JointContact {
    pub name: String,
    pub phone: String,
}

/// 正文排版风格。紧缩风格把正文区 # 号最多（层级最深）的那一级标题与紧随其后的正文段落合并为一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StyleMode {
    #[default]
    Normal,
    Compact,
}

impl StyleMode {
    pub const ALL: [Self; 2] = [Self::Normal, Self::Compact];

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "正常",
            Self::Compact => "紧缩",
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

    pub fn label(self) -> &'static str {
        match self {
            Self::New => "新建",
            Self::Draft => "草稿",
            Self::Published => "发布",
            Self::Archived => "归档",
        }
    }

    /// 状态迁移规则：草稿→发布/归档；发布→草稿（退回修改）/归档；归档无出边。
    /// `新建→草稿` 保留兼容旧数据，正常创建路径已在入库时统一升为草稿。
    pub fn can_transition(self, target: Self) -> bool {
        match self {
            Self::New => matches!(target, Self::Draft),
            Self::Draft => matches!(target, Self::Published | Self::Archived),
            Self::Published => matches!(target, Self::Draft | Self::Archived),
            Self::Archived => false,
        }
    }
}

/// 一条可复用的 AI 优化提示词。它只描述“这次要模型做什么”，输出形态一律由
/// `prompt::output_contract` 内置约束，用户既不能改也不能关掉，
/// 因此自定义提示词再怎么写都不会破坏导出所依赖的 Markdown 结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AiPrompt {
    pub id: u32,
    pub name: String,
    /// 交给模型的任务描述。留空表示只按内置标准做格式规整。
    pub instruction: String,
    /// 适用文种；留空表示所有文种通用。
    pub kinds: Vec<TemplateKind>,
    /// 预置项的稳定标识。非空表示这是内置项：可改、可恢复默认，但不可删除。
    pub builtin_key: String,
}

impl AiPrompt {
    /// 该提示词是否适用于指定文种。未限定文种的条目对所有文种可见。
    pub fn applies_to(&self, kind: TemplateKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    pub fn is_builtin(&self) -> bool {
        !self.builtin_key.is_empty()
    }

    /// 适用文种的中文摘要，用于列表和选择面板。
    pub fn kinds_label(&self) -> String {
        if self.kinds.is_empty() {
            "全部文种".to_string()
        } else {
            self.kinds
                .iter()
                .map(|kind| kind.label())
                .collect::<Vec<_>>()
                .join("、")
        }
    }
}

/// 预置提示词。`格式规整` 是保底项：指令留空，等价于旧版“只规整格式”的行为。
/// 其余三条允许改写措辞和篇幅，但都不得凭空造事实——这条底线同时写在
/// 各自指令和内置输出标准里。
pub fn builtin_ai_prompts() -> Vec<AiPrompt> {
    vec![
        AiPrompt {
            id: 0,
            name: "格式规整".into(),
            instruction: String::new(),
            kinds: vec![],
            builtin_key: "format".into(),
        },
        AiPrompt {
            id: 0,
            name: "公文化润色".into(),
            instruction: "在不改变任何事实、数据、结论和责任主体的前提下，把口语化、\
网络化、汇报体的表述改写为规范的公文语体：多用短句和动宾结构，去掉“我觉得”“大概”\
“差不多”“进一步加强”一类含糊或空转的说法，保持庄重、平实、准确。不得新增事实、\
数据、结论或政策依据。"
                .into(),
            kinds: vec![],
            builtin_key: "polish".into(),
        },
        AiPrompt {
            id: 0,
            name: "精简篇幅".into(),
            instruction: "在完整保留全部关键事实、数据、时限、责任主体和办理要求的前提下压缩篇幅：\
删去重复表述、套话铺垫和与主旨无关的枝节，合并同类内容，长句拆短。\
宁可少说，也不得为了通顺而改动或编造事实。"
                .into(),
            kinds: vec![],
            builtin_key: "condense".into(),
        },
        AiPrompt {
            id: 0,
            name: "充实论述".into(),
            instruction: "对论述单薄的段落做展开：补充逻辑衔接、层次划分和必要的说明性表述，\
使各部分详略均衡。只能在现有稿件已有的事实基础上展开；\
缺依据的地方原位写“【待核实：缺什么】”，绝不虚构数据、案例、文件名称、\
政策条款或领导讲话。"
                .into(),
            kinds: vec![
                TemplateKind::OfficialLetter,
                TemplateKind::PlainDocument,
                TemplateKind::WhitePaper,
            ],
            builtin_key: "expand".into(),
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LmStudioConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout_seconds: u64,
}

impl Default for LmStudioConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:1234/v1".into(),
            model: String::new(),
            api_key: String::new(),
            temperature: 0.25,
            max_tokens: 4096,
            timeout_seconds: 180,
        }
    }
}

/// 知识库 embedding 模型接入（OpenAI 兼容 /v1/embeddings）。与 chat 模型相互独立。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// 一次请求批量嵌入的块数；本地模型 32 吞吐较好，也避免单批过大超时。
    pub batch_size: usize,
    pub timeout_seconds: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:1234/v1".into(),
            model: String::new(),
            api_key: String::new(),
            batch_size: 32,
            timeout_seconds: 60,
        }
    }
}

/// 重排方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RerankMode {
    /// 不重排，直接按 RRF 融合分取前 N 条。
    None,
    /// 专用 rerank 端点（Jina / Cohere / TEI / Infinity / llama.cpp 等）。
    #[default]
    Api,
    /// 用对话大模型逐条打分重排。**LM Studio / Ollama 不提供 rerank 端点**，
    /// 这是不另起服务的替代方案：复用已配好的对话模型，代价是每次检索
    /// 多一次 LLM 调用（低温、短输出，通常几秒）。
    Llm,
}

impl RerankMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "不重排",
            Self::Api => "专用 rerank 端点",
            Self::Llm => "用对话大模型重排",
        }
    }

    pub const ALL: [Self; 3] = [Self::None, Self::Api, Self::Llm];
}

/// rerank 模型接入。rerank 不是 OpenAI 标准接口，各家（Jina / Cohere / TEI）的
/// 路径与响应字段不一，故路径与响应取值键做成可配，默认取 Jina/Cohere 风格。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RerankConfig {
    /// 重排方式。默认 `api`，保持既有配置的行为不变。
    pub mode: RerankMode,
    pub base_url: String,
    /// 端点路径，拼在 base_url 后（默认 "rerank"）。
    pub path: String,
    pub model: String,
    pub api_key: String,
    /// 请求里 top_n 字段名；空串表示不传 top_n。
    pub top_n_field: String,
    /// 响应里结果数组 / 原下标 / 相关分的取值键。
    pub response_results_key: String,
    pub response_index_key: String,
    pub response_score_key: String,
    pub timeout_seconds: u64,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            mode: RerankMode::default(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            path: "rerank".into(),
            model: String::new(),
            api_key: String::new(),
            top_n_field: "top_n".into(),
            response_results_key: "results".into(),
            response_index_key: "index".into(),
            response_score_key: "relevance_score".into(),
            timeout_seconds: 60,
        }
    }
}

/// 知识库检索增强（RAG）总配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RagConfig {
    /// 全局开关；起草页开关在此基础上临时覆盖。
    pub enabled: bool,
    pub embedding: EmbeddingConfig,
    pub rerank: RerankConfig,
    /// 向量路召回条数。
    pub recall_top_k: usize,
    /// 关键词（FTS5）路召回条数。
    pub fts_top_k: usize,
    /// 两路 RRF 融合后送 rerank 的条数。
    pub fused_top_m: usize,
    /// 最终注入提示词的参考块数。
    pub rerank_top_n: usize,
    /// 切块目标字数 / 上限 / 相邻块重叠。
    pub chunk_target_chars: usize,
    pub chunk_max_chars: usize,
    pub chunk_overlap_chars: usize,
    /// RRF 融合常数 k。
    pub rrf_k: f32,
    /// 相关性下限，按与首条结果的比例算（0 = 不过滤，0.5 = 砍掉不足首条一半分的）。
    /// 用相对比例而非绝对分数，是因为不同 rerank/embedding 模型的分值范围差异很大。
    pub min_score_ratio: f32,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding: EmbeddingConfig::default(),
            rerank: RerankConfig::default(),
            recall_top_k: 20,
            fts_top_k: 20,
            fused_top_m: 12,
            rerank_top_n: 5,
            chunk_target_chars: 400,
            chunk_max_chars: 600,
            chunk_overlap_chars: 80,
            rrf_k: 60.0,
            min_score_ratio: 0.5,
        }
    }
}

impl RagConfig {
    /// 实际生效的重排方式：选了「专用端点」却没填模型名时降级为不重排。
    pub fn effective_rerank_mode(&self) -> RerankMode {
        match self.rerank.mode {
            RerankMode::Api if self.rerank.model.trim().is_empty() => RerankMode::None,
            mode => mode,
        }
    }
}

/// 词库只有两种节点：单位是树干，人员是挂在单位下面的末端叶子。
/// 机关代字、简称、是否代章都是单位自身的属性，不再单列分类；
/// 会议地点与规范用语已从词库中移除（前者改为直接填写，后者不再影响成稿）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VocabularyCategory {
    #[default]
    Unit,
    Person,
}

impl VocabularyCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unit => "单位",
            Self::Person => "人员",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VocabularyEntry {
    /// 词条稳定标识。树形界面用它记录展开与选中状态，重排序、改名都不受影响。
    /// `0` 表示尚未分配，载入词库时统一补号。
    pub id: u64,
    pub category: VocabularyCategory,
    /// 单位专属：层级编码，每级两位，下级以上级编码为前缀（`00` → `0001`）。
    /// 由树形结构自动生成，也是单位在词库和各下拉框中的排列顺序。
    pub code: String,
    pub canonical: String,
    /// 单位专属：对外行文时使用的名称；为空时回退正常名称并由审核给出提醒。
    pub external_name: String,
    pub aliases: Vec<String>,
    pub note: String,
    /// 与联系人绑定的电话；只对 `VocabularyCategory::Person` 有意义。
    pub phone: String,
    /// 人员专属：职务。姓名与职务分开维护，便于白头件按职务合并呈报称谓。
    pub position: String,
    /// 单位专属：上级单位的层级编码，空串表示顶层单位。
    /// 旧配置中可能仍是规范名称，载入后由 `units::normalize` 自动迁移。
    pub parent: String,
    /// 单位专属：本单位简称（如“新舆处”），空串时回落规范名称。
    pub abbr: String,
    /// 人员专属：所属单位的层级编码，用于“人随事走”过滤。
    /// 旧配置中可能仍是规范名称，载入后由 `units::normalize` 自动迁移。
    pub unit: String,
    /// 人员专属：允许该人员在公函版记中担任所属单位各级上级单位的承办联系人。
    pub can_handle_parent_unit: bool,
    /// 单位专属：绑定的机关代字，选中该单位作发文单位时自动带出。
    pub department_code: String,
    /// 单位专属：以该单位作为公函落款（联合发文时为主发文单位）时是否自动标注“（代章）”。
    /// 仅公函盖章；电话通知等其他文种不适用，一律不标注。
    pub seal_on_behalf: bool,
    /// 仅供 Markdown 导入合并判断该列是否真实存在；不写入本机配置。
    #[serde(skip)]
    pub(crate) seal_on_behalf_imported: bool,
}

/// 公文中可以选择的密级。“不标注”用于无需标密的普通公文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityLevel {
    #[default]
    Unmarked,
    Internal,
    Secret,
    Confidential,
    TopSecret,
}

impl SecurityLevel {
    pub const ALL: [Self; 5] = [
        Self::Unmarked,
        Self::Internal,
        Self::Secret,
        Self::Confidential,
        Self::TopSecret,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Unmarked => "不标注密级",
            Self::Internal => "内部",
            Self::Secret => "秘密",
            Self::Confidential => "机密",
            Self::TopSecret => "绝密",
        }
    }

    /// 写入版心的密级文字；“不标注”为空串。
    pub fn marking(self) -> &'static str {
        match self {
            Self::Unmarked => "",
            Self::Internal => "内部",
            Self::Secret => "秘密",
            Self::Confidential => "机密",
            Self::TopSecret => "绝密",
        }
    }

    pub fn from_marking(value: &str) -> Self {
        match value.trim().trim_end_matches('级') {
            "内部" | "内部资料" | "工作秘密" => Self::Internal,
            "秘密" => Self::Secret,
            "机密" => Self::Confidential,
            "绝密" => Self::TopSecret,
            _ => Self::Unmarked,
        }
    }
}

/// 密级与保密期限的约束。默认值取自《中华人民共和国保守国家秘密法》第十五条：
/// 绝密级不超过三十年、机密级不超过二十年、秘密级不超过十年。
/// 本单位另有更严格或不同口径时，可在“设置”页直接改这三个上限。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityRules {
    pub secret_max_years: u32,
    pub confidential_max_years: u32,
    pub top_secret_max_years: u32,
    /// 期限不能确定时是否允许写“长期”。
    pub allow_long_term: bool,
}

impl Default for SecurityRules {
    fn default() -> Self {
        Self {
            secret_max_years: 10,
            confidential_max_years: 20,
            top_secret_max_years: 30,
            allow_long_term: true,
        }
    }
}

/// 可供选择的保密期限阶梯，按年数升序；实际下拉项会按密级上限裁剪。
const PERIOD_LADDER_YEARS: [u32; 7] = [1, 3, 5, 10, 20, 30, 50];
pub const LONG_TERM: &str = "长期";

impl SecurityRules {
    pub fn max_years(&self, level: SecurityLevel) -> Option<u32> {
        match level {
            SecurityLevel::Secret => Some(self.secret_max_years),
            SecurityLevel::Confidential => Some(self.confidential_max_years),
            SecurityLevel::TopSecret => Some(self.top_secret_max_years),
            _ => None,
        }
    }

    pub fn period_options(&self, level: SecurityLevel) -> Vec<String> {
        match level {
            SecurityLevel::Unmarked => Vec::new(),
            SecurityLevel::Internal => vec![LONG_TERM.to_string()],
            _ => {
                let max = self.max_years(level).unwrap_or(0);
                let mut options = vec!["6个月".to_string()];
                options.extend(
                    PERIOD_LADDER_YEARS
                        .iter()
                        .filter(|years| **years <= max)
                        .map(|years| format!("{years}年")),
                );
                if !options.iter().any(|value| value == &format!("{max}年")) && max > 0 {
                    options.push(format!("{max}年"));
                }
                if self.allow_long_term {
                    options.push(LONG_TERM.to_string());
                }
                options
            }
        }
    }

    /// 切换密级时给出的默认期限：取该密级的上限。
    pub fn default_period(&self, level: SecurityLevel) -> String {
        match level {
            SecurityLevel::Unmarked => String::new(),
            SecurityLevel::Internal => LONG_TERM.to_string(),
            _ => self
                .max_years(level)
                .map(|years| format!("{years}年"))
                .unwrap_or_default(),
        }
    }

    /// 返回 `Some(提示)` 表示密级与保密期限不匹配。
    pub fn check(&self, level: SecurityLevel, period: &str) -> Option<String> {
        let period = period.trim();
        if level == SecurityLevel::Unmarked {
            return (!period.is_empty()).then(|| "未选择密级时不应填写保密期限".to_string());
        }
        if period.is_empty() {
            return Some(format!("{}级公文必须标注保密期限", level.label()));
        }
        if period == LONG_TERM {
            return if self.allow_long_term {
                None
            } else {
                Some("当前设置不允许使用“长期”作为保密期限".to_string())
            };
        }
        let Some(months) = parse_period_months(period) else {
            return Some(format!(
                "保密期限“{period}”无法识别，应写为“6个月”“10年”或“长期”"
            ));
        };
        let max = self.max_years(level)?;
        if months > max * 12 {
            return Some(format!(
                "{}级公文的保密期限不得超过{}年，当前为“{}”",
                level.label(),
                max,
                period
            ));
        }
        None
    }
}

/// 把“6个月”“10年”解析为月数；“长期”和无法识别的写法返回 `None`。
pub fn parse_period_months(value: &str) -> Option<u32> {
    let value = value.trim();
    if let Some(years) = value.strip_suffix('年') {
        return years.trim().parse::<u32>().ok().map(|years| years * 12);
    }
    if let Some(months) = value.strip_suffix("个月") {
        return months.trim().parse::<u32>().ok();
    }
    None
}

/// 把保密期限拆成“前导数字 + 其余部分”：`“10年” → (“10”, “年”)`、`“6个月” → (“6”, “个月”)`。
/// 非数字期限（如“长期”）返回 `("", 原串)`。数字部分用于按等宽字体（LaTeX `\ttfamily`、
/// Word 等宽西文字体）排版，其余按行内基准字体。
pub fn split_period_digits(value: &str) -> (&str, &str) {
    let value = value.trim();
    let digits = value.chars().take_while(|ch| ch.is_ascii_digit()).count();
    // 前导 ASCII 数字每个占 1 字节，切点正好是字符边界。
    (&value[..digits], &value[digits..])
}

/// 主送、抄送等多值字段在配置中以顿号分隔存储，界面上按列表编辑。
pub fn split_units(value: &str) -> Vec<String> {
    value
        .split(['、', '，', ',', '；', ';', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn join_units(units: &[String]) -> String {
    units.join("、")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateProfile {
    pub kind: TemplateKind,
    pub profile_name: String,
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
    pub security_level: String,
    pub security_period: String,
    /// 指人专办：勾选后在密级后空一个全角空格并以黑体标注“指人专办”。
    pub special_handling: bool,
    /// 函号中的发文年份。旧稿未保存该字段时由成文日期兼容推导。
    pub document_year: String,
    pub document_number: String,
    pub meeting_location: String,
    pub letter_version: LetterVersion,
    /// 正文排版风格：正常 / 紧缩。公函、电话通知、白头件、会议议程均生效。
    pub style_mode: StyleMode,
    /// 函稿按双面方式排版页码：奇数页靠右、偶数页靠左。
    pub duplex_printing: bool,
}

impl Default for TemplateProfile {
    fn default() -> Self {
        Self {
            kind: TemplateKind::OfficialLetter,
            profile_name: "默认配置".into(),
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
            security_level: String::new(),
            security_period: String::new(),
            special_handling: false,
            document_year: String::new(),
            document_number: String::new(),
            meeting_location: String::new(),
            letter_version: LetterVersion::Formal,
            style_mode: StyleMode::Normal,
            duplex_printing: false,
        }
    }
}

impl TemplateProfile {
    pub fn for_kind(kind: TemplateKind) -> Self {
        Self {
            kind,
            ..Default::default()
        }
    }
}

/// 界面明色主题。与 `ThemeName::ALL` 的预设一一对应，缺省为默认的 Claude 奶油。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeName {
    #[default]
    Claude,
    Sky,
    Lilac,
    Green,
}

impl ThemeName {
    /// 全部可选主题，顺序与设置页展示一致。
    pub const ALL: [ThemeName; 4] = [Self::Claude, Self::Sky, Self::Lilac, Self::Green];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub lm_studio: LmStudioConfig,
    pub output_dir: String,
    pub vocabulary: Vec<VocabularyEntry>,
    pub profiles: Vec<TemplateProfile>,
    pub last_template: TemplateKind,
    pub export: ExportSelection,
    /// AI 起草或优化完成后是否立即按勾选格式导出一次。
    pub auto_export: bool,
    /// 定时与切标签时把改动静默写回稿件库。关掉就只认 Ctrl+S。
    #[serde(default = "auto_save_default")]
    pub auto_save: bool,
    pub security_rules: SecurityRules,
    /// 允许在词库之外手工填写单位、联系人等字段。
    pub allow_free_text: bool,
    /// 在 Markdown 源码与实时排版编辑器左侧显示源码行号。
    pub show_editor_line_numbers: bool,
    /// AI 优化提示词库。首次载入为空时补齐预置项，见 `ensure_ai_prompts`。
    pub ai_prompts: Vec<AiPrompt>,
    /// 上次使用的提示词 id，供选择面板默认高亮。
    pub last_ai_prompt: u32,
    /// 知识库检索增强（RAG）配置。
    pub rag: RagConfig,
    /// 编译公文时使用的字体。默认沿用随应用分发的内置字体。
    pub fonts: FontConfig,
    /// 界面明色主题。旧配置没有该字段时回退默认。
    pub theme: ThemeName,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            lm_studio: LmStudioConfig::default(),
            output_dir: String::new(),
            vocabulary: vec![],
            profiles: TemplateKind::ALL
                .into_iter()
                .map(TemplateProfile::for_kind)
                .collect(),
            last_template: TemplateKind::OfficialLetter,
            export: ExportSelection::default(),
            auto_export: false,
            auto_save: true,
            security_rules: SecurityRules::default(),
            allow_free_text: true,
            show_editor_line_numbers: true,
            ai_prompts: vec![],
            last_ai_prompt: 0,
            rag: RagConfig::default(),
            fonts: FontConfig::default(),
            theme: ThemeName::default(),
        }
    }
}

/// 公文排版里可以单独换字体的五个位置。
///
/// 与 `gonghan-gwa.cls` 的字体族一一对应，且互不复用：换其中一项不会牵动另一项，
/// 唯一的例外是二级标题字体同时充当正文的 `ItalicFont`（类文件历来如此）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// 公文大标题与附件标题，内置为方正小标宋。
    Title,
    /// `##` 一级标题「一、」，内置为黑体。
    Heading1,
    /// `###` 二级标题「（一）」，内置为楷体。
    Heading2,
    /// 正文、四级标题与表格，内置为仿宋。
    Body,
    /// 页脚页码，内置为宋体。
    PageNumber,
}

impl FontRole {
    pub const ALL: [FontRole; 5] = [
        Self::Title,
        Self::Heading1,
        Self::Heading2,
        Self::Body,
        Self::PageNumber,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "标题字体",
            Self::Heading1 => "一级标题字体",
            Self::Heading2 => "二级标题字体",
            Self::Body => "正文字体",
            Self::PageNumber => "页码字体",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Title => "公文大标题与附件标题，内置为方正小标宋",
            Self::Heading1 => "一级标题「一、」、表头与附件名，内置为黑体",
            Self::Heading2 => "二级标题「（一）」，内置为楷体；同时用作正文的斜体字面",
            Self::Body => "正文、四级标题与表格，内置为仿宋",
            Self::PageNumber => "页脚页码，内置为宋体",
        }
    }

    /// 内置字体文件名，未配置本机字体时使用。必须是
    /// `portable_runtime::FONT_FILES` 里的一项，由单元测试守住。
    pub fn bundled_file(self) -> &'static str {
        match self {
            Self::Title => "XiaoBiaoSong.ttf",
            Self::Heading1 => "SimHei.ttf",
            Self::Heading2 => "KaiTi.ttf",
            Self::Body => "FangSong.ttf",
            Self::PageNumber => "SimSun.ttf",
        }
    }

    /// 未配置本机字体时按名字加载所用的字体名。方正小标宋没有稳定的字体名，
    /// 由类文件另行探测，因此这里返回空串。
    pub fn bundled_family(self) -> &'static str {
        match self {
            Self::Title => "",
            Self::Heading1 => "SimHei",
            Self::Heading2 => "KaiTi_GB2312",
            Self::Body => "FangSong_GB2312",
            Self::PageNumber => "SimSun",
        }
    }

    /// 内置字体的中文名，用于界面上说明“不选就用什么”。
    pub fn bundled_label(self) -> &'static str {
        match self {
            Self::Title => "方正小标宋",
            Self::Heading1 => "黑体",
            Self::Heading2 => "楷体",
            Self::Body => "仿宋",
            Self::PageNumber => "宋体",
        }
    }

    /// 配置键与临时字体文件名里用的短标识。
    pub fn key(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Heading1 => "heading1",
            Self::Heading2 => "heading2",
            Self::Body => "body",
            Self::PageNumber => "pagenumber",
        }
    }
}

/// 某一个位置选定的本机字体。家族名与文件路径都要留：前者给导出的 `.tex`
/// 拿到别的机器上按名字编译，后者给内置 Tectonic 按文件加载。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontChoice {
    /// 英文家族名，写进 `.tex` 的按名字加载分支。
    pub family: String,
    /// 本地化字体名，只用于界面显示。
    pub display: String,
    /// 字体文件绝对路径。
    pub path: String,
}

impl FontChoice {
    pub fn is_set(&self) -> bool {
        !self.family.trim().is_empty() && !self.path.trim().is_empty()
    }

    /// 界面上显示的名字：优先本地化名。
    pub fn label(&self) -> &str {
        let display = self.display.trim();
        if display.is_empty() {
            self.family.trim()
        } else {
            display
        }
    }

    /// 拷进临时字体目录后的文件名。扩展名沿用原文件，fontspec 靠它判断格式；
    /// 重命名后目录里不存在同名的粗体、斜体文件，字面选择因此是确定的。
    pub fn compiled_file_name(&self, role: FontRole) -> String {
        let extension = std::path::Path::new(self.path.trim())
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_lowercase())
            .filter(|ext| ext == "ttf" || ext == "otf")
            .unwrap_or_else(|| "ttf".to_string());
        format!("gwa-{}.{extension}", role.key())
    }
}

/// 编译公文时使用的字体。字段全为空即表示沿用内置字体。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// 总开关。关掉时下面五项即使填了也不生效，方便临时对照内置版式。
    pub use_system_fonts: bool,
    /// 应用界面（窗口、菜单、列表）字体。留空用内置霞鹜文楷；不受
    /// `use_system_fonts` 开关控制，随时可以换回内置。
    pub ui_font: FontChoice,
    pub title: FontChoice,
    pub heading1: FontChoice,
    pub heading2: FontChoice,
    pub body: FontChoice,
    pub page_number: FontChoice,
}

impl FontConfig {
    pub fn choice(&self, role: FontRole) -> &FontChoice {
        match role {
            FontRole::Title => &self.title,
            FontRole::Heading1 => &self.heading1,
            FontRole::Heading2 => &self.heading2,
            FontRole::Body => &self.body,
            FontRole::PageNumber => &self.page_number,
        }
    }

    pub fn choice_mut(&mut self, role: FontRole) -> &mut FontChoice {
        match role {
            FontRole::Title => &mut self.title,
            FontRole::Heading1 => &mut self.heading1,
            FontRole::Heading2 => &mut self.heading2,
            FontRole::Body => &mut self.body,
            FontRole::PageNumber => &mut self.page_number,
        }
    }

    /// 真正生效的选择：总开关关掉或者这一项没配就返回 `None`，调用方据此
    /// 回退到内置字体。
    pub fn active(&self, role: FontRole) -> Option<&FontChoice> {
        if !self.use_system_fonts {
            return None;
        }
        let choice = self.choice(role);
        choice.is_set().then_some(choice)
    }

    /// 界面字体：不受 `use_system_fonts` 编译开关影响，配了就生效。
    pub fn active_ui_font(&self) -> Option<&FontChoice> {
        self.ui_font.is_set().then_some(&self.ui_font)
    }

    /// 有没有任何一项本机字体生效。全都没有时导出的 `.tex` 与从前完全一致。
    pub fn any_active(&self) -> bool {
        FontRole::ALL
            .iter()
            .any(|role| self.active(*role).is_some())
    }
}

impl AppConfig {
    pub fn profile(&self, kind: TemplateKind) -> TemplateProfile {
        self.profiles
            .iter()
            .find(|p| p.kind == kind)
            .cloned()
            .unwrap_or_else(|| TemplateProfile::for_kind(kind))
    }

    pub fn upsert_profile(&mut self, profile: TemplateProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.kind == profile.kind) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    /// 载入配置后整理提示词库：补齐缺失的预置项、给未编号的条目分配 id。
    /// 旧配置里没有 `ai_prompts` 字段，这里一次性补全；用户删过的内置项不会
    /// 复活——内置项本就不可删，缺失只可能来自旧版本或手工编辑。
    pub fn ensure_ai_prompts(&mut self) {
        for builtin in builtin_ai_prompts() {
            if !self
                .ai_prompts
                .iter()
                .any(|entry| entry.builtin_key == builtin.builtin_key)
            {
                self.ai_prompts.push(builtin);
            }
        }
        let mut next = self
            .ai_prompts
            .iter()
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0);
        for entry in &mut self.ai_prompts {
            if entry.id == 0 {
                next += 1;
                entry.id = next;
            }
        }
    }

    pub fn ai_prompt(&self, id: u32) -> Option<&AiPrompt> {
        self.ai_prompts.iter().find(|entry| entry.id == id)
    }

    /// 分配一个未占用的提示词 id。
    pub fn next_ai_prompt_id(&self) -> u32 {
        self.ai_prompts
            .iter()
            .map(|entry| entry.id)
            .max()
            .unwrap_or(0)
            + 1
    }
}

/// 旧配置文件里没有这个开关；缺省按开启处理。
fn auto_save_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DraftInput {
    pub kind: TemplateKind,
    pub title_hint: String,
    pub date: String,
    pub date_is_auto: bool,
    pub meeting_time: String,
    pub attendees: String,
    pub profile: TemplateProfile,
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
        }
    }
}

impl DraftInput {
    pub fn uses_external_unit_names(&self) -> bool {
        self.kind == TemplateKind::OfficialLetter
            && self.profile.correspondence_scope == CorrespondenceScope::External
    }

    /// 函号年份优先使用独立元数据；旧稿回退到成文日期开头的四位年份。
    pub fn document_year(&self) -> String {
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
    pub tex: bool,
    /// 反复导出同一篇稿件时覆盖同名文件，而不是不断生成 `-2`、`-3` 副本。
    pub overwrite: bool,
}

impl Default for ExportSelection {
    fn default() -> Self {
        Self {
            markdown: true,
            docx: true,
            tex: true,
            overwrite: false,
        }
    }
}

impl ExportSelection {
    pub fn any(&self) -> bool {
        self.markdown || self.docx || self.tex
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedDraft {
    pub markdown: String,
    pub title: String,
    pub warnings: Vec<String>,
    pub files: Vec<std::path::PathBuf>,
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    /// 旧配置没有 `ai_prompts` 字段，载入后必须补齐全部预置项并编好 id。
    #[test]
    fn missing_ai_prompts_are_backfilled_with_unique_ids() {
        let mut config = AppConfig::default();
        assert!(config.ai_prompts.is_empty());
        config.ensure_ai_prompts();

        assert_eq!(config.ai_prompts.len(), builtin_ai_prompts().len());
        assert!(config.ai_prompts.iter().all(|entry| entry.id != 0));
        let ids = config
            .ai_prompts
            .iter()
            .map(|entry| entry.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), config.ai_prompts.len(), "id 不得重复");
        // 保底项永远在，指令为空即“只规整格式”。
        let format = config
            .ai_prompts
            .iter()
            .find(|entry| entry.builtin_key == "format")
            .expect("应有格式规整预置项");
        assert!(format.instruction.is_empty());
        assert!(format.kinds.is_empty());
    }

    /// 旧配置的 `fonts` 没有 `ui_font` 字段：载入后应为空选择（回退内置字体），
    /// 且编译字体不受影响。
    #[test]
    fn old_font_config_without_ui_font_stays_empty() {
        let json = r#"{
            "use_system_fonts": true,
            "title": {"family": "SimSun", "display": "宋体", "path": "C:\\Windows\\Fonts\\simsun.ttc"},
            "heading1": {"family": "SimHei", "display": "黑体", "path": "C:\\Windows\\Fonts\\simhei.ttf"},
            "heading2": {"family": "KaiTi", "display": "楷体", "path": "C:\\Windows\\Fonts\\simkai.ttf"},
            "body": {"family": "FangSong", "display": "仿宋", "path": "C:\\Windows\\Fonts\\simfang.ttf"},
            "pagenumber": {"family": "SimSun", "display": "宋体", "path": "C:\\Windows\\Fonts\\simsun.ttc"}
        }"#;
        let config: FontConfig = serde_json::from_str(json).expect("旧配置应能反序列化");
        assert!(!config.ui_font.is_set());
        assert!(config.active_ui_font().is_none());
        assert_eq!(config.active(FontRole::Body).unwrap().family, "FangSong");
    }

    /// 带 `ui_font` 的新配置序列化后能原样读回。
    #[test]
    fn ui_font_round_trips_through_json() {
        let mut config = FontConfig::default();
        config.ui_font = FontChoice {
            family: "Microsoft YaHei".into(),
            display: "微软雅黑".into(),
            path: "C:\\Windows\\Fonts\\msyh.ttc".into(),
        };
        let json = serde_json::to_string(&config).expect("应能序列化");
        let back: FontConfig = serde_json::from_str(&json).expect("应能反序列化");
        assert_eq!(config, back);
        assert_eq!(back.active_ui_font().unwrap().family, "Microsoft YaHei");
    }

    /// 主题字段序列化后能原样读回；旧配置没有 `theme` 字段时回退默认 Claude。
    #[test]
    fn theme_name_round_trips_and_old_config_defaults_to_claude() {
        let mut config = AppConfig::default();
        config.theme = ThemeName::Sky;
        let json = serde_json::to_string(&config).expect("应能序列化");
        let back: AppConfig = serde_json::from_str(&json).expect("应能反序列化");
        assert_eq!(back.theme, ThemeName::Sky, "round-trip 后主题不变");

        // 去掉 theme 键模拟旧配置：载入后回退默认。
        let mut value: serde_json::Value =
            serde_json::from_str(&json).expect("应能解析为 JSON 值");
        value
            .as_object_mut()
            .expect("配置应为对象")
            .remove("theme");
        let old: AppConfig = serde_json::from_value(value).expect("旧配置应能反序列化");
        assert_eq!(old.theme, ThemeName::default(), "旧配置回退默认主题");
    }

    /// 补齐是幂等的：反复载入不会把预置项复制成好几份，也不会重排用户改过的内容。
    #[test]
    fn backfilling_ai_prompts_is_idempotent_and_keeps_user_edits() {
        let mut config = AppConfig::default();
        config.ensure_ai_prompts();
        let polish_id = config
            .ai_prompts
            .iter()
            .find(|entry| entry.builtin_key == "polish")
            .expect("应有润色预置项")
            .id;
        config
            .ai_prompts
            .iter_mut()
            .find(|entry| entry.id == polish_id)
            .unwrap()
            .instruction = "我改过的内容".into();

        let before = config.ai_prompts.clone();
        config.ensure_ai_prompts();
        assert_eq!(config.ai_prompts, before);
    }

    /// 自定义条目和预置项混在一起时，补齐只补缺的那条，并给新条目排不冲突的 id。
    #[test]
    fn backfilling_only_adds_the_missing_builtins() {
        let mut config = AppConfig::default();
        config.ai_prompts = vec![AiPrompt {
            id: 7,
            name: "我的提示词".into(),
            instruction: "精简".into(),
            kinds: vec![TemplateKind::WhitePaper],
            builtin_key: String::new(),
        }];
        config.ensure_ai_prompts();

        assert_eq!(config.ai_prompts.len(), builtin_ai_prompts().len() + 1);
        assert_eq!(config.ai_prompts[0].id, 7, "已有条目的 id 不变");
        assert!(
            config.ai_prompts.iter().skip(1).all(|entry| entry.id > 7),
            "新补的 id 必须避开已用的 7"
        );
        assert_eq!(config.next_ai_prompt_id(), 12);
    }

    #[test]
    fn prompts_without_kinds_apply_to_every_template() {
        let general = AiPrompt {
            kinds: vec![],
            ..Default::default()
        };
        assert!(
            TemplateKind::ALL
                .into_iter()
                .all(|kind| general.applies_to(kind))
        );
        assert_eq!(general.kinds_label(), "全部文种");
        assert!(!general.is_builtin());

        let scoped = AiPrompt {
            kinds: vec![TemplateKind::WhitePaper, TemplateKind::PlainDocument],
            builtin_key: "expand".into(),
            ..Default::default()
        };
        assert!(scoped.applies_to(TemplateKind::WhitePaper));
        assert!(!scoped.applies_to(TemplateKind::MeetingAgenda));
        assert_eq!(scoped.kinds_label(), "白头件（呈批件）、普通公文");
        assert!(scoped.is_builtin());
    }

    /// 扩写类提示词不该出现在会议议程和电话通知的选择面板里——那两种文种没有
    /// 可展开的论述段落。
    #[test]
    fn expand_preset_is_scoped_away_from_agenda_and_phone_notice() {
        let expand = builtin_ai_prompts()
            .into_iter()
            .find(|entry| entry.builtin_key == "expand")
            .expect("应有充实论述预置项");
        assert!(!expand.applies_to(TemplateKind::MeetingAgenda));
        assert!(!expand.applies_to(TemplateKind::PhoneNotice));
        assert!(expand.applies_to(TemplateKind::WhitePaper));
    }

    /// 预置项的 key 是恢复默认时的锚点，必须唯一。
    #[test]
    fn builtin_keys_are_unique_and_non_empty() {
        let keys = builtin_ai_prompts()
            .into_iter()
            .map(|entry| entry.builtin_key)
            .collect::<Vec<_>>();
        assert!(keys.iter().all(|key| !key.is_empty()));
        let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), keys.len());
    }

    #[test]
    fn period_options_are_capped_by_level() {
        let rules = SecurityRules::default();
        let secret = rules.period_options(SecurityLevel::Secret);
        assert!(secret.contains(&"10年".to_string()));
        assert!(!secret.contains(&"20年".to_string()));
        assert!(secret.contains(&LONG_TERM.to_string()));

        let top = rules.period_options(SecurityLevel::TopSecret);
        assert!(top.contains(&"30年".to_string()));
        assert!(!top.contains(&"50年".to_string()));
    }

    #[test]
    fn check_rejects_period_beyond_level_limit() {
        let rules = SecurityRules::default();
        assert!(rules.check(SecurityLevel::Secret, "10年").is_none());
        assert!(
            rules
                .check(SecurityLevel::Secret, "20年")
                .is_some_and(|message| message.contains("不得超过10年"))
        );
        assert!(rules.check(SecurityLevel::TopSecret, "长期").is_none());
        assert!(rules.check(SecurityLevel::Confidential, "").is_some());
        assert!(rules.check(SecurityLevel::Unmarked, "10年").is_some());
        assert!(rules.check(SecurityLevel::Unmarked, "").is_none());
    }

    #[test]
    fn custom_limits_are_respected() {
        let rules = SecurityRules {
            confidential_max_years: 30,
            ..SecurityRules::default()
        };
        assert!(rules.check(SecurityLevel::Confidential, "30年").is_none());
        assert!(
            rules
                .period_options(SecurityLevel::Confidential)
                .contains(&"30年".to_string())
        );
    }

    #[test]
    fn units_round_trip_through_separators() {
        let units = split_units("甲单位、乙单位，丙单位;丁单位");
        assert_eq!(units, ["甲单位", "乙单位", "丙单位", "丁单位"]);
        assert_eq!(join_units(&units), "甲单位、乙单位、丙单位、丁单位");
    }

    #[test]
    fn split_period_digits_keeps_leading_numbers() {
        assert_eq!(split_period_digits("10年"), ("10", "年"));
        assert_eq!(split_period_digits("6个月"), ("6", "个月"));
        assert_eq!(split_period_digits("长期"), ("", "长期"));
        assert_eq!(split_period_digits(""), ("", ""));
        assert_eq!(split_period_digits("  5年"), ("5", "年"));
    }

    #[test]
    fn manuscript_status_transitions() {
        use ManuscriptStatus::{Archived, Draft, New, Published};
        assert!(New.can_transition(Draft));
        assert!(!New.can_transition(Published));
        assert!(!New.can_transition(Archived));
        assert!(Draft.can_transition(Published));
        assert!(Draft.can_transition(Archived));
        assert!(!Draft.can_transition(New));
        assert!(Published.can_transition(Draft));
        assert!(Published.can_transition(Archived));
        assert!(!Published.can_transition(New));
        assert!(!Archived.can_transition(Draft));
        assert!(!Archived.can_transition(Archived));
        assert_eq!(New.label(), "新建");
        assert_eq!(Draft.label(), "草稿");
        assert_eq!(Published.label(), "发布");
        assert_eq!(Archived.label(), "归档");
    }

    #[test]
    fn draft_input_serde_round_trip() {
        let draft = DraftInput {
            kind: TemplateKind::MeetingAgenda,
            title_hint: "标题提示".into(),
            date: "2026年8月6日".into(),
            date_is_auto: false,
            meeting_time: "8月10日上午9时".into(),
            attendees: "张三、李四".into(),
            profile: TemplateProfile {
                document_number: "某教函〔2026〕12号".into(),
                ..TemplateProfile::for_kind(TemplateKind::MeetingAgenda)
            },
        };
        let json = serde_json::to_string(&draft).expect("快照应能序列化");
        let back: DraftInput = serde_json::from_str(&json).expect("快照应能反序列化");
        assert_eq!(back.kind, TemplateKind::MeetingAgenda);
        assert_eq!(back.date, "2026年8月6日");
        assert_eq!(back.profile.document_number, "某教函〔2026〕12号");
        assert!(!back.date_is_auto);
        assert_eq!(back.attendees, "张三、李四");
    }
}
