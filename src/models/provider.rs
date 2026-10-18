//! 模型服务商、模型引用与对话/修订/嵌入模型配置。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::provider`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// 模型服务提供商预设模板。预设只是模板：点「添加」才实例化成一条
/// [`ProviderConfig`] 写入配置；模板本身不落盘。地址都要求是 OpenAI 兼容接口
/// （含 /v1）。用途标签只用于模型选择器里的排序与提示，不做硬性过滤——服务端
/// 到底加载了什么模型，以「测试连接 / 刷新模型」读到的为准。
pub struct ProviderPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    /// 本地服务（LM Studio / Ollama）通常不需要密钥。
    pub needs_key: bool,
    /// 卡片上的一句话提示。
    pub note: &'static str,
    /// 用途标签：`chat` / `embedding` / `rerank`。
    pub tags: &'static [&'static str],
}

pub const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "lmstudio",
        name: "LM Studio",
        base_url: "http://127.0.0.1:1234/v1",
        needs_key: false,
        note: "本机 LM Studio 服务",
        tags: &["chat", "embedding"],
    },
    ProviderPreset {
        id: "ollama",
        name: "Ollama",
        base_url: "http://127.0.0.1:11434/v1",
        needs_key: false,
        note: "本机 Ollama 服务",
        tags: &["chat", "embedding"],
    },
    ProviderPreset {
        id: "vllm",
        name: "vLLM（内网）",
        base_url: "",
        needs_key: false,
        note: "内网 vLLM 部署，地址视部署填写；可同时承载对话、embedding 与 rerank 模型",
        tags: &["chat", "embedding", "rerank"],
    },
    ProviderPreset {
        id: "opencode-go",
        name: "OpenCode-Go",
        base_url: "https://opencode.ai/zen/go/v1",
        needs_key: true,
        note: "地址已预设；填写 Go API Key 后刷新模型清单",
        tags: &["chat"],
    },
    ProviderPreset {
        id: "minimax",
        name: "MiniMax",
        base_url: "https://api.minimaxi.com/v1",
        needs_key: true,
        note: "MiniMax 国内开放平台",
        tags: &["chat"],
    },
    ProviderPreset {
        id: "kimi-code",
        name: "Kimi Code",
        base_url: "https://api.kimi.com/coding/v1",
        needs_key: true,
        note: "Kimi Code 订阅的 OpenAI 兼容端点（与 Kimi 开放平台按量计费是两套账户）",
        tags: &["chat"],
    },
    ProviderPreset {
        id: "dmxapi",
        name: "DMXAPI",
        base_url: "https://www.dmxapi.cn/v1",
        needs_key: true,
        note: "多模型聚合网关",
        tags: &["chat", "embedding", "rerank"],
    },
    ProviderPreset {
        id: "siliconflow",
        name: "硅基流动",
        base_url: "https://api.siliconflow.cn/v1",
        needs_key: true,
        note: "对话、embedding 与 rerank 模型都有",
        tags: &["chat", "embedding", "rerank"],
    },
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        needs_key: true,
        note: "DeepSeek 官方开放平台",
        tags: &["chat"],
    },
];

/// 按接口地址猜预设（迁移旧配置时给提供商起名用）。归一化后按主机特征匹配。
pub(crate) fn preset_for_url(base_url: &str) -> Option<&'static ProviderPreset> {
    let url = base_url.trim().trim_end_matches('/').to_lowercase();
    if url.is_empty() {
        return None;
    }
    let host_like = url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    PROVIDER_PRESETS
        .iter()
        .find(|preset| {
            let preset_url = preset
                .base_url
                .trim_start_matches("http://")
                .trim_start_matches("https://");
            !preset_url.is_empty() && host_like == preset_url
        })
        .or_else(|| {
            // 端口 / 域名特征兜底：本机服务换主机名、聚合站换后缀都还能认出来。
            if host_like.starts_with("127.0.0.1:1234") || host_like.starts_with("localhost:1234") {
                return PROVIDER_PRESETS.iter().find(|p| p.id == "lmstudio");
            }
            if host_like.starts_with("127.0.0.1:11434") || host_like.starts_with("localhost:11434")
            {
                return PROVIDER_PRESETS.iter().find(|p| p.id == "ollama");
            }
            let contains = |needle: &str, id: &str| {
                host_like
                    .contains(needle)
                    .then(|| PROVIDER_PRESETS.iter().find(|p| p.id == id))
                    .flatten()
            };
            contains("dmxapi", "dmxapi")
                .or_else(|| contains("siliconflow", "siliconflow"))
                .or_else(|| contains("api.deepseek.com", "deepseek"))
                .or_else(|| contains("minimax", "minimax"))
                .or_else(|| contains("api.kimi.com", "kimi-code"))
        })
}

/// 模型用途类别：选择器按它过滤，选对话模型时不再看到向量模型。
///
/// OpenAI 兼容的 `/v1/models` 不返回类型，来源优先级：
/// 服务原生接口自报（LM Studio 的 `/api/v0/models`）> 模型名启发式 > Unknown。
/// Unknown 按对话模型对待（对话是多数，误藏比误列更糟）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    /// 对话生成（视觉模型 vlm 也归这里，起草 / 复核都用它）。
    Chat,
    /// 向量嵌入。
    Embedding,
    /// 重排序。
    Rerank,
    /// 认不出来，按对话处理。
    Unknown,
}

impl ModelKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Chat => "对话",
            Self::Embedding => "向量",
            Self::Rerank => "重排",
            Self::Unknown => "未识别",
        }
    }
}

/// 给一个模型定用途类别。`probed` 是服务原生接口自报的类型（没有就 None）。
///
/// rerank 关键词最具体，永远优先：服务把 reranker 报成普通 llm 也不耽误。
pub(crate) fn classify_model(name: &str, probed: Option<ModelKind>) -> ModelKind {
    let lower = name.to_lowercase();
    if lower.contains("rerank") {
        return ModelKind::Rerank;
    }
    if let Some(kind) = probed.filter(|kind| *kind != ModelKind::Unknown) {
        return kind;
    }
    // 名字启发式：覆盖 bge / gte / e5 / m3e / text-embedding / jina-embeddings /
    // nomic-embed-text / piccolo 等常见向量模型。认不出就 Unknown，宁漏勿错。
    const EMBEDDING_HINTS: &[&str] =
        &["embed", "bge-", "gte-", "e5-", "m3e", "text2vec", "piccolo"];
    if EMBEDDING_HINTS.iter().any(|hint| lower.contains(hint)) {
        return ModelKind::Embedding;
    }
    ModelKind::Unknown
}

/// 一个模型服务提供商：连接身份（地址、密钥）的唯一存放处。
///
/// 起草、复核、知识库各功能只存 [`ModelRef`] 引用，不再各自填地址密钥；
/// 换服务商时在「AI 管理 → 模型服务」重选一次即可。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// 内部 id（`p1`、`p2`…），改名不影响引用。
    pub id: String,
    /// 显示名，如「LM Studio」「内网 vLLM」。
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    /// 上次「测试连接 / 刷新模型」读到的模型清单，随配置缓存：
    /// 服务没开时选择器里仍能按这份清单选模型。
    pub models: Vec<String>,
    /// 各模型的用途类别（探测时由服务自报或按名字归类），选择器按它过滤。
    /// 键是模型名；表里没有的模型视为 Unknown。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, ModelKind>,
    /// 用途标签（chat / embedding / rerank），从预设复制，只用于选择器排序。
    pub tags: Vec<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            enabled: true,
            models: Vec::new(),
            kinds: BTreeMap::new(),
            tags: Vec::new(),
        }
    }
}

/// 功能侧的模型选择：哪家提供商的哪个模型。
///
/// `model` 为空表示「未选择」；复核与 rerank 里空还有「沿用起草 / 跳过重排」
/// 的语义，由各自的分区说明。提供商被删除时引用保留不清空，界面出警示，
/// 运行时报错指向「AI 管理 → 模型服务」。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelRef {
    pub provider_id: String,
    pub model: String,
}

impl ModelRef {
    pub(crate) fn is_empty(&self) -> bool {
        self.model.trim().is_empty()
    }
}

/// 模型引用解析失败：提供商被删、被停用或没填地址。消息直接给用户看。
#[derive(Debug, Clone)]
pub struct ModelRefError(pub String);

impl fmt::Display for ModelRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelRefError {}

/// 思考深度由模型服务解释；默认不发送参数，兼容旧配置和非思考模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    Default,
    Low,
    Medium,
    High,
    Max,
}

impl ReasoningEffort {
    pub const ALL: [Self; 5] = [
        Self::Default,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Max,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Default => "沿用服务端",
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Max => "最高",
        }
    }

    pub(crate) fn wire_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Max => Some("max"),
        }
    }
}

/// 同为 Chat Completions，网关对思考深度的字段写法可能不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningFormat {
    #[default]
    ReasoningEffort,
    ReasoningObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LmStudioConfig {
    /// 运行时会话标识：侧栏用已保存的会话 UUID，独立测试用临时 UUID；不写入配置。
    #[serde(skip)]
    pub session_id: String,
    /// 旧字段：新配置写的是 `AppConfig::providers` + `draft_model`，这里只在
    /// 迁移前（或测试直填）承载内联地址；迁移后清空并不再序列化。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub temperature: f32,
    pub reasoning_effort: ReasoningEffort,
    pub reasoning_format: ReasoningFormat,
    pub max_tokens: u32,
    pub timeout_seconds: u64,
    /// 上下文窗口（token）。0 表示自动：先问服务，问不到按模型名估，再不行按 32k
    /// （`lmstudio::context`）。
    pub context_window: u32,
}

impl Default for LmStudioConfig {
    fn default() -> Self {
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            model: String::new(),
            api_key: String::new(),
            temperature: 0.25,
            reasoning_effort: ReasoningEffort::Default,
            reasoning_format: ReasoningFormat::default(),
            // 思考型模型的推理也算在这里面：qwen3.5 9B 写两百来字的通知要先想近两万字，
            // 4096 会被思考吃光、一个正文字都出不来。
            max_tokens: 32000,
            timeout_seconds: 180,
            context_window: 0,
        }
    }
}

/// 访问模型服务时走哪条网络通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// 一律直连，连环境变量里的代理也不认。
    Direct,
    /// 跟随系统：先看 `HTTPS_PROXY` / `ALL_PROXY` 等环境变量，没设再读 Windows
    /// 「设置 → 网络 → 代理」（macOS 读系统偏好）。
    #[default]
    System,
    /// 用下面填的代理地址。
    Custom,
}

impl ProxyMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Direct => "直连",
            Self::System => "跟随系统",
            Self::Custom => "自定义代理",
        }
    }

    pub const ALL: [Self; 3] = [Self::Direct, Self::System, Self::Custom];
}

/// 网络代理。起草、复核、知识库的 embedding / rerank 请求都走这一条通道。
///
/// 本机地址（localhost、127.0.0.0/8、::1）不论哪种模式都直连：本地模型服务
/// 绕到代理上再折回来，轻则多一跳，重则被代理规则拦下。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    /// 代理地址，如 `http://127.0.0.1:7890`、`socks5h://127.0.0.1:1080`。
    /// 用户名密码写在地址里：`socks5://user:pass@host:port`。
    pub url: String,
    /// 自定义代理下不走代理的地址，逗号分隔；支持域名（含子域）、IP 与网段，
    /// 如 `intranet.gov.cn, 10.0.0.0/8`。
    pub bypass: String,
}

/// 文字复核用的小模型接入。
///
/// 与起草模型分开配是这一层的前提：起草要发挥，需要大模型和一点温度；逐句
/// 复核只要稳，Qwen3 4B/8B 一类的小模型温度 0 反而更好使，而且快得多——一篇
/// 稿子几十句，用大模型逐句问一遍要等到人失去耐心。
///
/// 地址和密钥留空时沿用起草模型的：多数人两者跑在同一个本地服务上，只是加载了
/// 不同的模型，没必要填两遍。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviseModelConfig {
    /// 默认关闭。模型没配好就把入口摆出来，只会让人点一次、失败一次。
    pub enabled: bool,
    /// 复核用的模型。留空 = 沿用起草模型（替代旧的「地址密钥分别留空沿用」规则）。
    pub model_ref: ModelRef,
    /// 复核与辅助角色共用的备用对话模型。
    pub backup_model_ref: ModelRef,
    /// 旧字段：内联地址 / 模型 / 密钥，迁移后清空，仅作向后兼容读取。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    pub timeout_seconds: u64,
    /// 上下文窗口（token），0 表示自动，同 [`LmStudioConfig::context_window`]。
    pub context_window: u32,
    /// 单句送检的字数上限。超过多半是整段没断句，交给小模型只会跑飞。
    pub max_sentence_chars: usize,
    /// 一轮最多送检多少句，免得一篇长稿把本地服务占死。
    pub max_sentences: usize,
    /// 停用的检查器 id。
    ///
    /// 用「停用清单」而不是「启用清单」：旧配置里没有这个字段时，serde 会填
    /// [`Default`] 的值，正好让还没量过的新检查器保持关闭；而如果用启用清单，
    /// 旧配置会解析成空集，把已经站住的语病检查也一并关掉。
    pub disabled_tasks: Vec<String>,
}

impl Default for ReviseModelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model_ref: ModelRef::default(),
            backup_model_ref: ModelRef::default(),
            base_url: String::new(),
            model: String::new(),
            api_key: String::new(),
            // 小模型单句推理很快，但本地服务冷启动会慢，留足一次的余量即可。
            timeout_seconds: 60,
            context_window: 0,
            max_sentence_chars: 120,
            max_sentences: 200,
            disabled_tasks: crate::revise_model::DEFAULT_DISABLED_TASKS
                .iter()
                .map(|id| (*id).to_string())
                .collect(),
        }
    }
}

impl ReviseModelConfig {
    pub(crate) fn task_enabled(&self, id: &str) -> bool {
        !self.disabled_tasks.iter().any(|item| item == id)
    }

    pub(crate) fn set_task_enabled(&mut self, id: &str, enabled: bool) {
        self.disabled_tasks.retain(|item| item != id);
        if !enabled {
            self.disabled_tasks.push(id.to_string());
        }
    }

    /// 当前启用的检查器。
    pub(crate) fn enabled_tasks(&self) -> Vec<&'static crate::revise_model::ReviseTask> {
        crate::revise_model::TASKS
            .iter()
            .filter(|task| self.task_enabled(task.id))
            .collect()
    }

    /// 解析成一次真正可用的接入配置。温度固定为 0：复核要的是可复现，不是发挥。
    pub(crate) fn resolve(&self, draft_model: &LmStudioConfig) -> LmStudioConfig {
        let pick = |own: &str, fallback: &str| {
            if own.trim().is_empty() {
                fallback.to_string()
            } else {
                own.trim().to_string()
            }
        };
        LmStudioConfig {
            session_id: draft_model.session_id.clone(),
            base_url: pick(&self.base_url, &draft_model.base_url),
            model: self.model.trim().to_string(),
            api_key: pick(&self.api_key, &draft_model.api_key),
            temperature: 0.0,
            // 逐句复核的输出上限按句长现算，这里给个不起作用的兜底值。
            max_tokens: 512,
            timeout_seconds: self.timeout_seconds.max(5),
            context_window: self.context_window,
            reasoning_effort: ReasoningEffort::Default,
            reasoning_format: ReasoningFormat::default(),
        }
    }
}

/// 知识库 embedding 模型接入（OpenAI 兼容 /v1/embeddings）。与 chat 模型相互独立。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    ///  embedding 用的模型，引用「模型服务商管理」里的提供商。
    pub model_ref: ModelRef,
    /// 旧字段：内联地址 / 模型 / 密钥，迁移后清空，仅作向后兼容读取。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// 一次请求批量嵌入的块数；本地模型 32 吞吐较好，也避免单批过大超时。
    pub batch_size: usize,
    pub timeout_seconds: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_ref: ModelRef::default(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            model: String::new(),
            api_key: String::new(),
            batch_size: 32,
            timeout_seconds: 60,
        }
    }
}
