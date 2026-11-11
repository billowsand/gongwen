//! 应用总配置 `AppConfig` 及其模型解析、界面默认值。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::config`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{
    DEFAULT_TRANSMIT_TASKS, DiagramTheme, EditorFontScheme, ExportSelection, FontConfig, ImeConfig,
    NumberingConfig, PaperMode, PrintConfig, ProofreadConfig, ProxyConfig, RibbonTab,
    SecurityRules, TemplateKind, TemplateProfile, ThemeName, VocabularyEntry,
    VocabularySetupStatus,
};
#[cfg(feature = "ai-workbench")]
use crate::models::{
    LmStudioConfig, ModelRef, ModelRefError, PROVIDER_PRESETS, ProviderConfig, RagConfig,
    ReasoningEffort, ReasoningFormat, RerankMode, ReviseModelConfig, preset_for_url,
};
#[cfg(feature = "ai-workbench")]
use anyhow::Result;
use serde::{Deserialize, Serialize};
#[cfg(feature = "ai-workbench")]
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[cfg(feature = "ai-workbench")]
    pub lm_studio: LmStudioConfig,
    /// 模型服务提供商清单：地址与密钥的唯一存放处（「AI 管理 → 模型服务商管理」）。
    #[cfg(feature = "ai-workbench")]
    pub providers: Vec<ProviderConfig>,
    /// 文字起草用的模型（引用 providers 中的一项）。
    #[cfg(feature = "ai-workbench")]
    pub draft_model: ModelRef,
    /// 起草服务不可用且尚未输出时，本次调用使用的备用对话模型。
    #[cfg(feature = "ai-workbench")]
    pub draft_backup_model: ModelRef,
    /// 智能写作（Pi）独立选择的模型。旧配置没有该字段时为空；为空就明确提示，
    /// 不偷偷回落到起草模型。地址与密钥仍只存在 providers 里。
    #[serde(default)]
    #[cfg(feature = "ai-workbench")]
    pub pi_model: ModelRef,
    /// 智能写作（Pi）自己的生成参数（目前只有思考深度）。旧配置没有该字段时为空，
    /// 由 `migrate_pi_options` 一次性按原来实际用的深度补上，之后与文字起草的设置互不影响。
    /// 窗口与输出预算由程序自动管理，不放用户旋钮。
    #[serde(default)]
    #[cfg(feature = "ai-workbench")]
    pub pi_options: Option<PiOptions>,
    pub output_dir: String,
    pub vocabulary: Vec<VocabularyEntry>,
    /// 首次建库引导的完成状态；旧配置会根据是否已有词条在启动时自动迁移。
    pub vocabulary_setup: VocabularySetupStatus,
    pub profiles: Vec<TemplateProfile>,
    pub last_template: TemplateKind,
    pub export: ExportSelection,
    /// 传输任务类型（《传发任务单》的题名）字典。默认三项，用户可增删；
    /// 旧配置没有这一项时按默认补齐（见 [`AppConfig::ensure_transmit_tasks`]）。
    #[serde(default)]
    pub transmit_tasks: Vec<String>,
    /// 《传发任务单》上由用户预先定好的填写内容（设置页「加密传输单」一节）。
    #[serde(default)]
    pub transmit_defaults: TransmitDefaults,
    /// 打印对话框记住的选择（上次用的打印机）。
    pub print: PrintConfig,
    /// AI 起草或优化完成后是否立即按勾选格式导出一次。
    #[cfg(feature = "ai-workbench")]
    pub auto_export: bool,
    /// 定时与切标签时把改动静默写回稿件库。关掉就只认主快捷键+S。
    #[serde(default = "auto_save_default")]
    pub auto_save: bool,
    /// 定时自动保存的间隔（秒）。旧配置没有该字段时按 2 分钟处理。
    #[serde(default = "default_auto_save_interval_secs")]
    pub auto_save_interval_secs: u64,
    /// 应用锁空闲自动锁定的时限（分钟）：没有任何键盘 / 鼠标输入超过这个时长
    /// 就退回锁屏。0 = 不自动锁定。仅启用应用锁后生效；旧配置没有该字段时
    /// 按 15 分钟处理。
    #[serde(default = "default_auto_lock_minutes")]
    pub auto_lock_minutes: u32,
    pub security_rules: SecurityRules,
    /// 允许在词库之外手工填写单位、联系人等字段。
    pub allow_free_text: bool,
    /// 在 Markdown 源码编辑器左侧显示源码行号。
    pub show_editor_line_numbers: bool,
    /// Markdown 源码模式左侧显示可跳转的标题目录。
    #[serde(default = "source_navigation_default")]
    pub show_source_outline: bool,
    /// Markdown 源码模式右侧显示可点击、可拖动的全文缩略图。
    #[serde(default = "source_navigation_default")]
    pub show_source_minimap: bool,
    /// 在公文预览与对照模式的右缘显示导航刻度（悬停展开成标题列表）。
    /// 默认开：刻度只占右缘十几个点，且不吃点击。关掉后右缘完全干净。
    pub show_preview_navigator: bool,
    /// 在公文预览的左页边给正文逐行标号（`preview::gutter`）。默认关：
    /// 页边有没有号是投屏对稿才需要的取舍，旧配置缺该字段时按关处理。
    #[serde(default)]
    pub show_preview_line_numbers: bool,
    /// 标题聚焦编辑用大字排：投屏给领导看着改时只留放大的标题，不显示提示与说明。
    /// 记住上一次的选择，旧配置缺该字段时按关处理。
    #[serde(default)]
    pub heading_focus_large: bool,
    /// Markdown 源码编辑器的字号（px）。可用 Ctrl+滚轮或 Ctrl± 调整。
    pub editor_font_size: f32,
    /// 全局界面缩放。旧配置没有该字段时按 100% 处理。
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// 更新检查地址（GitHub `releases/latest` 接口）。内网环境可改成镜像地址。
    #[serde(default = "default_update_check_url")]
    pub update_check_url: String,
    /// 首次启动的示例稿是否已写入稿件库。旧配置没有该字段时视为尚未写入。
    #[serde(default)]
    pub samples_seeded: bool,
    /// 知识库检索增强（RAG）配置。
    #[cfg(feature = "ai-workbench")]
    pub rag: RagConfig,
    /// 校对词表的用户层：对内置条目的改动与自建条目。
    pub proofread: ProofreadConfig,
    /// 文字复核用的小模型接入。与起草模型相互独立。
    #[cfg(feature = "ai-workbench")]
    pub revise_model: ReviseModelConfig,
    /// 访问模型服务的网络代理。旧配置没有该字段时按「跟随系统」补齐。
    pub proxy: ProxyConfig,
    /// 编译公文时使用的字体。默认沿用随应用分发的内置字体。
    pub fonts: FontConfig,
    /// 各级标题与列表的编号样式。旧配置没有该字段时按默认值补齐。
    pub numbering: NumberingConfig,
    /// 界面主题。旧配置没有该字段时回退默认。
    pub theme: ThemeName,
    /// 界面明暗是否跟随系统：浅色系统用默认的 Claude 奶油，深色系统用
    /// 「复古暗」（`ThemeName::GruvboxDark`）。默认开启，旧配置没有该字段时
    /// 也按跟随系统处理；手动点选主题会把这一项关掉。
    #[serde(default = "follow_system_theme_default")]
    pub theme_follows_system: bool,
    /// 屏幕上公文纸面的明暗。导出结果不受影响。
    pub paper: PaperMode,
    /// 图表的配色与线型，预览与导出一致。旧配置缺该字段时按「按文种自动」。
    pub diagram_theme: DiagramTheme,
    /// 源码编辑器里各处 Markdown 元素分别用哪支字面。旧配置缺该字段时按默认值
    /// 补齐，即六处全用编辑器字体。
    pub editor_fonts: EditorFontScheme,
    /// 起草页功能区上次停在哪个分区卡。
    pub ribbon_tab: RibbonTab,
    /// 功能区第二行（当前分区的按钮）是否收起，只留分区卡那一条。
    pub ribbon_collapsed: bool,
    /// 应用内词表输入法。旧配置没有该字段时按默认值补齐（启用词表、全角标点）。
    #[serde(default)]
    pub ime: ImeConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            #[cfg(feature = "ai-workbench")]
            lm_studio: LmStudioConfig::default(),
            #[cfg(feature = "ai-workbench")]
            providers: Vec::new(),
            #[cfg(feature = "ai-workbench")]
            draft_model: ModelRef::default(),
            #[cfg(feature = "ai-workbench")]
            draft_backup_model: ModelRef::default(),
            #[cfg(feature = "ai-workbench")]
            pi_model: ModelRef::default(),
            #[cfg(feature = "ai-workbench")]
            pi_options: None,
            output_dir: String::new(),
            vocabulary: vec![],
            vocabulary_setup: VocabularySetupStatus::Pending,
            profiles: TemplateKind::ALL
                .into_iter()
                .map(TemplateProfile::for_kind)
                .collect(),
            last_template: TemplateKind::OfficialLetter,
            export: ExportSelection::default(),
            transmit_tasks: Vec::new(),
            transmit_defaults: TransmitDefaults::default(),
            print: PrintConfig::default(),
            #[cfg(feature = "ai-workbench")]
            auto_export: false,
            auto_save: true,
            auto_save_interval_secs: DEFAULT_AUTO_SAVE_INTERVAL_SECS,
            auto_lock_minutes: default_auto_lock_minutes(),
            security_rules: SecurityRules::default(),
            allow_free_text: true,
            show_editor_line_numbers: true,
            show_source_outline: true,
            show_source_minimap: true,
            show_preview_navigator: true,
            show_preview_line_numbers: false,
            heading_focus_large: false,
            editor_font_size: 14.0,
            ui_scale: 1.0,
            update_check_url: default_update_check_url(),
            samples_seeded: false,
            #[cfg(feature = "ai-workbench")]
            rag: RagConfig::default(),
            proofread: ProofreadConfig::default(),
            #[cfg(feature = "ai-workbench")]
            revise_model: ReviseModelConfig::default(),
            proxy: ProxyConfig::default(),
            fonts: FontConfig::default(),
            numbering: NumberingConfig::default(),
            theme: ThemeName::default(),
            theme_follows_system: true,
            paper: PaperMode::default(),
            diagram_theme: DiagramTheme::default(),
            editor_fonts: EditorFontScheme::default(),
            ribbon_tab: RibbonTab::default(),
            ribbon_collapsed: false,
            ime: ImeConfig::default(),
        }
    }
}

impl AppConfig {
    /// 应用到 egui 的缩放值：旧配置或手工改坏的值都收进可选范围。
    pub(crate) fn clamped_ui_scale(&self) -> f32 {
        if self.ui_scale.is_finite() {
            self.ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX)
        } else {
            1.0
        }
    }

    /// 自动保存实际使用的间隔：手工改坏或超出范围的配置都收进可选范围。
    pub(crate) fn clamped_auto_save_interval_secs(&self) -> u64 {
        self.auto_save_interval_secs
            .clamp(AUTO_SAVE_INTERVAL_MIN_SECS, AUTO_SAVE_INTERVAL_MAX_SECS)
    }

    #[cfg(feature = "ai-workbench")]
    pub(crate) fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    #[cfg(feature = "ai-workbench")]
    pub(crate) fn provider_mut(&mut self, id: &str) -> Option<&mut ProviderConfig> {
        self.providers.iter_mut().find(|p| p.id == id)
    }

    /// 下一个可用的提供商 id（`p1`、`p2`…，取数字后缀最大值加一）。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn next_provider_id(&self) -> String {
        let max = self
            .providers
            .iter()
            .filter_map(|p| p.id.strip_prefix('p'))
            .filter_map(|n| n.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("p{}", max + 1)
    }

    /// 校验并取出功能引用的提供商。错误消息直接给用户看。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn require_provider(
        &self,
        id: &str,
        func: &str,
    ) -> Result<&ProviderConfig, ModelRefError> {
        let provider = self.provider(id).ok_or_else(|| {
            ModelRefError(format!(
                "{func}引用的提供商已删除：请到「AI 管理 → 模型服务」重新选择模型。"
            ))
        })?;
        if !provider.enabled {
            return Err(ModelRefError(format!(
                "{func}用的提供商「{}」已停用：到「AI 管理 → 模型服务商管理」启用，或重新选择模型。",
                provider.name
            )));
        }
        if provider.base_url.trim().is_empty() {
            return Err(ModelRefError(format!(
                "提供商「{}」还没填接口地址：请到「AI 管理 → 模型服务商管理」补齐。",
                provider.name
            )));
        }
        Ok(provider)
    }

    /// 起草模型（对话）的完整接入配置：提供商的地址密钥 + 功能侧的生成参数。
    ///
    /// 旧配置的内联字段（迁移前 / 测试直填）优先于引用，保证两条路都能跑。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn draft_chat(&self) -> Result<LmStudioConfig, ModelRefError> {
        let mut config = self.lm_studio.clone();
        if self.draft_model.is_empty() {
            if !config.model.trim().is_empty() {
                return Ok(config);
            }
            return Err(ModelRefError(
                "还没有选择起草模型：请到「AI 管理 → 模型服务」选择。".into(),
            ));
        }
        let provider = self.require_provider(&self.draft_model.provider_id, "文字起草")?;
        config.base_url = provider.base_url.clone();
        config.api_key = provider.api_key.clone();
        config.model = self.draft_model.model.trim().to_string();
        Ok(config)
    }

    /// 一次性辅助调用（知识库问答、检索重排、数据接口 AI）统一用写作模型。
    ///
    /// 任务助手的材料整理与表单填写自 2026-11-10 起走 Pi 作业（`pi_agent::task_jobs`），
    /// 不再经过这里；它们的模型仍来自 Pi 自己的配置。
    ///
    /// 这些调用不需要多步工具循环，但需要理解与表达；2026-11-05 起不再单独维护「辅助模型」。
    /// 旧配置只选了文字起草模型时回落到它，避免升级后这些功能直接不可用。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn aux_chat(&self) -> Result<LmStudioConfig, ModelRefError> {
        // 选了 Pi 就以 Pi 为准：提供商被删 / 停用要如实报错，不能偷偷用旧起草模型。
        // 只有完全没选 Pi 的旧配置才回落 `draft_model`（兼容升级前的机器）。
        if !self.pi_model.is_empty() {
            return self.pi_chat();
        }
        self.draft_chat().map_err(|_| {
            ModelRefError(
                "还没有选择写作模型：请到「AI 管理 → 模型服务 → 智能写作（Pi）」选择。".into(),
            )
        })
    }

    /// Pi 只认独立模型；上下文自动按该服务与模型解析，不继承小模型的手动窗口和输出上限。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn pi_chat(&self) -> Result<LmStudioConfig, ModelRefError> {
        if self.pi_model.is_empty() {
            return Err(ModelRefError(
                "还没有选择智能写作（Pi）模型：请到「AI 管理 → 模型服务 → 智能写作（Pi）」选择。"
                    .into(),
            ));
        }
        let mut config = self.lm_studio.clone();
        let provider = self.require_provider(&self.pi_model.provider_id, "智能写作（Pi）")?;
        config.base_url = provider.base_url.clone();
        config.api_key = provider.api_key.clone();
        config.model = self.pi_model.model.trim().to_string();
        config.context_window = 0;
        config.max_tokens = 32_768;
        // 思考深度只认 Pi 自己的设置，不再隐式继承旧起草设置。
        // host 自行适配参数格式；保留这里的网关格式供知识库回退调用使用。
        config.reasoning_effort = self.pi_options().reasoning_effort;
        Ok(config)
    }

    /// Pi 的生成参数：旧配置还没迁移时按 Pi 默认值解析（不回写，迁移在启动时做）。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn pi_options(&self) -> PiOptions {
        self.pi_options.clone().unwrap_or_default()
    }

    /// 旧配置迁移：把 Pi 原来隐式继承的思考深度固化成 Pi 自己的设置，只做一次。
    ///
    /// 旧版 Pi 直接沿用文字起草的 `reasoning_effort`；这里把它复制成 `pi_options`，之后
    /// 改起草设置不再影响 Pi。旧配置没有 Pi 模型（本来也启动不了 Pi）时用 Pi 默认值。
    /// 与提供商迁移无关，单独由 `storage::load_config` 接入。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn migrate_pi_options(&mut self) -> bool {
        if self.pi_options.is_some() {
            return false;
        }
        let reasoning_effort = if self.pi_model.is_empty() {
            PiOptions::default().reasoning_effort
        } else {
            self.lm_studio.reasoning_effort
        };
        self.pi_options = Some(PiOptions { reasoning_effort });
        true
    }

    /// 文字复核与旧辅助步骤的接入配置：配了复核模型（且复核已启用）用它，
    /// 否则沿用写作模型（旧配置回落到旧起草模型）；温度固定 0，复核要的是可复现。
    ///
    /// 2026-11-05 起「文字复核」是唯一保留的专门小模型；`assist` 只为旧会话回放保留，
    /// 新流程的一次性辅助调用走 [`Self::aux_chat`]。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn revise_chat(&self, assist: bool) -> Result<LmStudioConfig, ModelRefError> {
        let revise = &self.revise_model;
        let mut config = if revise.enabled && !revise.model_ref.is_empty() {
            let provider = self.require_provider(&revise.model_ref.provider_id, "文字复核")?;
            LmStudioConfig {
                // 独立复核只依赖自己的提供商；写作配置失效不能连带禁用复核。
                session_id: self.lm_studio.session_id.clone(),
                base_url: provider.base_url.clone(),
                model: revise.model_ref.model.trim().to_string(),
                api_key: provider.api_key.clone(),
                temperature: 0.0,
                // 逐句复核的输出上限按句长现算，这里给个不起作用的兜底值。
                max_tokens: 512,
                timeout_seconds: revise.timeout_seconds.max(5),
                context_window: revise.context_window,
                reasoning_effort: ReasoningEffort::Default,
                reasoning_format: ReasoningFormat::default(),
            }
        } else if revise.enabled && !revise.model.trim().is_empty() {
            // 旧内联字段（未迁移 / 测试直填）。
            revise.resolve(&self.aux_chat()?)
        } else {
            let aux = self.aux_chat()?;
            LmStudioConfig {
                temperature: 0.0,
                max_tokens: 512,
                timeout_seconds: revise.timeout_seconds.max(5),
                // 复用同一个模型时，自动模式继承写作模型的窗口。
                context_window: if revise.enabled && revise.context_window > 0 {
                    revise.context_window
                } else {
                    aux.context_window
                },
                ..aux
            }
        };
        if assist {
            config.max_tokens = self.lm_studio.max_tokens;
        } else {
            // 逐句复核的关闭思考降级重试也不能重新带上写作模型的深度。
            config.reasoning_effort = ReasoningEffort::Default;
        }
        Ok(config)
    }

    /// 智能体辅助步骤用的接入配置（原 `agent::backend::assist_config`）。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn assist_chat(&self) -> Result<LmStudioConfig, ModelRefError> {
        self.revise_chat(true)
    }

    #[cfg(feature = "ai-workbench")]
    pub(crate) fn backup_chat(
        &self,
        model: &ModelRef,
        func: &str,
        mut config: LmStudioConfig,
    ) -> Result<Option<LmStudioConfig>, ModelRefError> {
        if model.is_empty() {
            return Ok(None);
        }
        let provider = self.require_provider(&model.provider_id, func)?;
        config.base_url = provider.base_url.clone();
        config.api_key = provider.api_key.clone();
        config.model = model.model.trim().into();
        // 备用模型可能有另一种窗口；按它的服务信息自动识别，不沿用主模型的手动值。
        config.context_window = 0;
        Ok(Some(config))
    }

    #[cfg(feature = "ai-workbench")]
    pub(crate) fn draft_backup_chat(&self) -> Result<Option<LmStudioConfig>, ModelRefError> {
        self.backup_chat(
            &self.draft_backup_model,
            "文字起草备用模型",
            self.lm_studio.clone(),
        )
    }

    /// 一次性写作模型的备用解析：选了 Pi 就没有旧起草备用语义；旧配置未选 Pi 时，
    /// 仍可沿用 `draft_backup_model` 兼容旧调用。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn aux_backup_chat(&self) -> Result<Option<LmStudioConfig>, ModelRefError> {
        if !self.pi_model.is_empty() {
            return Ok(None);
        }
        self.draft_backup_chat()
    }

    #[cfg(feature = "ai-workbench")]
    pub(crate) fn revise_backup_chat(
        &self,
        assist: bool,
    ) -> Result<Option<LmStudioConfig>, ModelRefError> {
        let config = LmStudioConfig {
            temperature: 0.0,
            // 独立复核备用的能力可能不同；辅助角色也不用起草模型的深度参数。
            reasoning_effort: ReasoningEffort::Default,
            max_tokens: if assist {
                self.lm_studio.max_tokens
            } else {
                512
            },
            timeout_seconds: self.revise_model.timeout_seconds.max(5),
            ..self.lm_studio.clone()
        };
        self.backup_chat(
            &self.revise_model.backup_model_ref,
            "文字复核备用模型",
            config,
        )
    }

    /// 智能体辅助角色的备用模型：配了复核备用就用它；没配、而辅助角色此刻用的就是起草
    /// 模型（复核没启用或没选模型，见 [`Self::revise_chat`]）时，沿用起草的备用（温度 0）——
    /// 否则起草模型一挂，起草调用换了备用，澄清、核对、自主步骤这些辅助调用却直接失败。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn assist_backup_chat(&self) -> Result<Option<LmStudioConfig>, ModelRefError> {
        if let Some(own) = self.revise_backup_chat(true)? {
            return Ok(Some(own));
        }
        let revise = &self.revise_model;
        let own_model =
            revise.enabled && !(revise.model_ref.is_empty() && revise.model.trim().is_empty());
        if own_model {
            return Ok(None);
        }
        Ok(self.draft_backup_chat()?.map(|config| LmStudioConfig {
            temperature: 0.0,
            ..config
        }))
    }

    /// 解析后的知识库配置：embedding / rerank 的地址密钥从提供商填好，
    /// 其余检索参数原样保留。提供商失效时不报错而是留空地址——检索路径
    /// 已有「embedding 不可用就退回关键词检索」的降级与告警，界面另有警示芯片。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn resolved_rag(&self) -> RagConfig {
        let mut rag = self.rag.clone();
        if !rag.embedding.model_ref.is_empty() {
            let model = rag.embedding.model_ref.model.trim().to_string();
            match self.provider(&rag.embedding.model_ref.provider_id) {
                Some(p) if p.enabled && !p.base_url.trim().is_empty() => {
                    rag.embedding.base_url = p.base_url.clone();
                    rag.embedding.api_key = p.api_key.clone();
                }
                _ => {
                    rag.embedding.base_url = String::new();
                    rag.embedding.api_key = String::new();
                }
            }
            rag.embedding.model = model;
        }
        if rag.rerank.mode == RerankMode::Api && !rag.rerank.model_ref.is_empty() {
            let model = rag.rerank.model_ref.model.trim().to_string();
            match self.provider(&rag.rerank.model_ref.provider_id) {
                Some(p) if p.enabled && !p.base_url.trim().is_empty() => {
                    rag.rerank.base_url = p.base_url.clone();
                    rag.rerank.api_key = p.api_key.clone();
                }
                _ => {
                    rag.rerank.base_url = String::new();
                    rag.rerank.api_key = String::new();
                }
            }
            rag.rerank.model = model;
        }
        rag
    }

    /// 当前 embedding 模型的名字（解析引用后）。建索引与「换模型提示重建」用它。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn embedding_model_name(&self) -> String {
        if !self.rag.embedding.model_ref.is_empty() {
            return self.rag.embedding.model_ref.model.trim().to_string();
        }
        self.rag.embedding.model.trim().to_string()
    }

    /// 状态栏上起草模型的显示名：模型（提供商）。
    /// 状态栏显示的写作模型名（带提供商）。没选 Pi 时为空，不回落到旧起草模型。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn pi_model_label(&self) -> String {
        let model = self.pi_model.model.trim();
        if model.is_empty() {
            return String::new();
        }
        match self.provider(&self.pi_model.provider_id) {
            Some(p) => format!("{model}（{}）", p.name),
            None => model.to_string(),
        }
    }

    /// 旧配置迁移：把散在四处的地址 / 密钥 / 模型名归并成提供商清单与模型引用。
    ///
    /// 内联字段只在还没有任何提供商时迁移；已有提供商只补旧 Go 模板遗漏的地址。
    /// 同一个「地址 + 密钥」只建一条。提供商按地址
    /// 匹配预设起名（127.0.0.1:1234 → LM Studio，dmxapi → DMXAPI……），认不出
    /// 的叫「自定义提供商」。迁移后清空旧内联字段，返回是否发生了改动。
    #[cfg(feature = "ai-workbench")]
    pub(crate) fn migrate_providers(&mut self) -> bool {
        if !self.providers.is_empty() {
            // 旧版 Go 模板漏了地址，只修复原名且地址仍为空的条目，保留自定义连接。
            // 预设表里没有这一条就不做迁移，不让升级路径把程序带崩。
            let Some(preset) = PROVIDER_PRESETS.iter().find(|p| p.id == "opencode-go") else {
                return false;
            };
            let mut changed = false;
            for provider in &mut self.providers {
                if provider.name == preset.name && provider.base_url.trim().is_empty() {
                    provider.base_url = preset.base_url.into();
                    changed = true;
                }
            }
            return changed;
        }
        // 先留住旧值：下面边迁边清。
        let draft_url = self.lm_studio.base_url.trim().to_string();
        let draft_key = self.lm_studio.api_key.trim().to_string();
        let draft_model = self.lm_studio.model.trim().to_string();
        let mut changed = false;

        // 起草：默认配置也带 127.0.0.1:1234，总能建出第一条（一般是 LM Studio）。
        if !draft_url.is_empty() || !draft_model.is_empty() {
            let pid = upsert_provider(&mut self.providers, &draft_url, &draft_key);
            if !draft_model.is_empty() {
                self.draft_model = ModelRef {
                    provider_id: pid,
                    model: draft_model,
                };
            }
            self.lm_studio.base_url.clear();
            self.lm_studio.api_key.clear();
            self.lm_studio.model.clear();
            changed = true;
        }

        // 复核：旧规则是地址 / 密钥留空沿用起草，迁移时归并到同一条提供商。
        let revise_model = self.revise_model.model.trim().to_string();
        if !revise_model.is_empty() {
            let url = {
                let own = self.revise_model.base_url.trim();
                if own.is_empty() {
                    draft_url.as_str()
                } else {
                    own
                }
                .to_string()
            };
            let key = {
                let own = self.revise_model.api_key.trim();
                if own.is_empty() {
                    draft_key.as_str()
                } else {
                    own
                }
                .to_string()
            };
            let pid = upsert_provider(&mut self.providers, &url, &key);
            self.revise_model.model_ref = ModelRef {
                provider_id: pid,
                model: revise_model,
            };
            changed = true;
        }
        self.revise_model.base_url.clear();
        self.revise_model.api_key.clear();
        self.revise_model.model.clear();

        // embedding / rerank：各自归并；模型名为空就只清字段、不建引用。
        let embed_model = self.rag.embedding.model.trim().to_string();
        let embed_url = self.rag.embedding.base_url.trim().to_string();
        let embed_key = self.rag.embedding.api_key.trim().to_string();
        if !embed_model.is_empty() || (!embed_url.is_empty() && embed_url != draft_url) {
            let pid = upsert_provider(&mut self.providers, &embed_url, &embed_key);
            if !embed_model.is_empty() {
                self.rag.embedding.model_ref = ModelRef {
                    provider_id: pid,
                    model: embed_model,
                };
            }
            changed = true;
        }
        self.rag.embedding.base_url.clear();
        self.rag.embedding.api_key.clear();
        self.rag.embedding.model.clear();

        let rerank_model = self.rag.rerank.model.trim().to_string();
        let rerank_url = self.rag.rerank.base_url.trim().to_string();
        let rerank_key = self.rag.rerank.api_key.trim().to_string();
        if !rerank_model.is_empty() || (!rerank_url.is_empty() && rerank_url != draft_url) {
            let pid = upsert_provider(&mut self.providers, &rerank_url, &rerank_key);
            if !rerank_model.is_empty() {
                self.rag.rerank.model_ref = ModelRef {
                    provider_id: pid,
                    model: rerank_model,
                };
            }
            changed = true;
        }
        self.rag.rerank.base_url.clear();
        self.rag.rerank.api_key.clear();
        self.rag.rerank.model.clear();

        changed
    }
}

/// 迁移用的归并助手：同一个「地址 + 密钥」只留一条提供商，返回其 id。
#[cfg(feature = "ai-workbench")]
pub(crate) fn upsert_provider(
    providers: &mut Vec<ProviderConfig>,
    base_url: &str,
    api_key: &str,
) -> String {
    let url = base_url.trim().trim_end_matches('/').to_string();
    let key = api_key.trim().to_string();
    if let Some(existing) = providers
        .iter()
        .find(|p| p.base_url == url && p.api_key == key)
    {
        return existing.id.clone();
    }
    let seq = providers.len() as u32 + 1;
    let preset = preset_for_url(&url);
    let name = preset.map(|p| p.name.to_string()).unwrap_or_else(|| {
        if url.contains("moonshot") {
            "Kimi 开放平台".into()
        } else if seq == 1 {
            "自定义提供商".into()
        } else {
            format!("自定义提供商 {seq}")
        }
    });
    let tags = preset
        .map(|p| p.tags.iter().map(|t| (*t).to_string()).collect())
        .unwrap_or_default();
    let id = format!("p{seq}");
    providers.push(ProviderConfig {
        id: id.clone(),
        name,
        base_url: url,
        api_key: key,
        enabled: true,
        models: Vec::new(),
        kinds: BTreeMap::new(),
        tags,
    });
    id
}

impl AppConfig {
    pub(crate) fn profile(&self, kind: TemplateKind) -> TemplateProfile {
        self.profiles
            .iter()
            .find(|p| p.kind == kind)
            .cloned()
            .unwrap_or_else(|| TemplateProfile::for_kind(kind))
    }

    pub(crate) fn upsert_profile(&mut self, profile: TemplateProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.kind == profile.kind) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    /// 密级从 v0.4 起是所有文稿类型的必要填报项目，默认机密、保密期限 20 年。
    /// 旧配置里各模板可能存了空密级（当时允许“不标注密级”），载入时统一回填默认值，
    /// 保证老用户新建稿件也带默认密级；用户显式保存过的非空密级一律不动。
    pub(crate) fn ensure_security_defaults(&mut self) {
        for profile in &mut self.profiles {
            if profile.security_level.trim().is_empty() {
                profile.security_level = "机密".into();
                profile.security_period = "20年".into();
            }
        }
    }

    /// 载入配置后补齐传输任务字典：旧配置没有这一项，用户也没自己加过，
    /// 就按默认三项补上。用户删空了就尊重用户的选择，不再补。
    pub(crate) fn ensure_transmit_tasks(&mut self) {
        if self.transmit_tasks.is_empty() {
            self.transmit_tasks = DEFAULT_TRANSMIT_TASKS
                .iter()
                .map(|task| (*task).to_string())
                .collect();
        }
        self.transmit_tasks.retain(|task| !task.trim().is_empty());
    }
}

/// 界面缩放的可选范围：再小看不清控件，再大普通笔记本一屏放不下两栏。
pub const UI_SCALE_MIN: f32 = 0.8;

pub const UI_SCALE_MAX: f32 = 1.75;

/// 界面缩放的预设档位（百分比）。设置页按这些档位画分段按钮，不再让人手填比例；
/// 90、100、120、150 是常用档，80 与 175 对应可选范围的两端。
pub const UI_SCALE_PRESETS: [u32; 7] = [80, 90, 100, 110, 120, 150, 175];

/// 自动保存间隔的可选范围（秒）：太密会和输入抢 SQLite 写锁，太疏又护不住现场。
pub const AUTO_SAVE_INTERVAL_MIN_SECS: u64 = 30;

pub const AUTO_SAVE_INTERVAL_MAX_SECS: u64 = 600;

/// 默认 2 分钟，与既有行为一致。
pub const DEFAULT_AUTO_SAVE_INTERVAL_SECS: u64 = 120;

/// 旧配置文件没有自动保存间隔；缺省 2 分钟。
pub(crate) fn default_auto_save_interval_secs() -> u64 {
    DEFAULT_AUTO_SAVE_INTERVAL_SECS
}

/// 旧配置文件没有自动锁定时限；缺省 15 分钟。
pub(crate) fn default_auto_lock_minutes() -> u32 {
    15
}

/// 旧配置文件没有界面缩放；缺省 100%。
pub(crate) fn default_ui_scale() -> f32 {
    1.0
}

/// 旧配置文件没有「跟随系统明暗」；缺省开启，升级后界面自动与桌面明暗一致。
pub(crate) fn follow_system_theme_default() -> bool {
    true
}

/// 旧配置文件没有更新检查地址；缺省用项目主页的 GitHub 发布接口。
pub(crate) fn default_update_check_url() -> String {
    crate::update_check::DEFAULT_UPDATE_CHECK_URL.to_string()
}

/// 旧配置文件里没有这个开关；缺省按开启处理。
pub(crate) fn auto_save_default() -> bool {
    true
}

/// 旧配置文件没有源码导航开关；升级后默认显示目录与缩略图。
pub(crate) fn source_navigation_default() -> bool {
    true
}

/// 智能写作（Pi）的生成参数。
///
/// 只放思考深度这一项：上下文窗口与输出上限由程序按服务与模型自动管理，不给用户旋钮。
/// 与 `LmStudioConfig` 里的起草参数彻底分开——改文字起草的思考深度不影响 Pi。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg(feature = "ai-workbench")]
pub struct PiOptions {
    /// 思考深度。默认「沿用服务端」：不发深度参数，兼容不认该参数的服务。
    pub reasoning_effort: ReasoningEffort,
}

#[cfg(feature = "ai-workbench")]
impl Default for PiOptions {
    fn default() -> Self {
        Self {
            reasoning_effort: ReasoningEffort::Default,
        }
    }
}

/// 《传发任务单》上的默认填写内容（设置页「加密传输单」一节）。
///
/// 这些是机要单据上「得由人确认」的部分：编号与签名程序不替人编造，但每次手填
/// 一遍也折腾，所以做成默认配置，导出时照填。**留空的仍留白手写**——单子上要
/// 留出位置，人签字、填号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransmitDefaults {
    /// 传输任务（单子题名）：从传输任务类型字典里选一项。留空则按密级自动带
    /// （有密级为涉密信息加密，无密级为公开信息明文，见
    /// [`crate::models::resolve_transmit_task`]）。
    pub task: String,
    /// 传字：印在右上角「传　　号」里，号码只留空白手写，不加方括号与横线。
    pub serial_prefix: String,
    /// 默认签批人姓名。配合 [`TransmitDefaults::signatory_blank`] 用。
    pub signatory: String,
    /// 签批人留空（手签），只留空白、不画横线。默认开：多数情况是纸质手签，姓名由人在单子上写。
    pub signatory_blank: bool,
    /// 时效要求。默认「急」；留空则这一栏留白手写。
    pub urgency: String,
    /// 发后处理：销毁或退还。
    pub disposal: TransmitDisposal,
    /// 备注：可粘多行，印在单子的备注栏；留空则那一栏留白手写。
    pub remark: String,
    /// 保密负责单位：印在末行，整体替掉模板上的「【保密单位】」占位，不保留方括号。
    pub security_unit: String,
}

impl Default for TransmitDefaults {
    fn default() -> Self {
        Self {
            task: String::new(),
            serial_prefix: "传".into(),
            signatory: String::new(),
            signatory_blank: true,
            urgency: "急".into(),
            disposal: TransmitDisposal::default(),
            remark: String::new(),
            security_unit: String::new(),
        }
    }
}

/// 发后处理：传输单上的「□退还 √销毁」二选一。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransmitDisposal {
    /// 销毁（默认，与模板原样一致）。
    #[default]
    Destroy,
    /// 退还。
    Return,
}

impl TransmitDisposal {
    /// 全部选项，按单子上出现的先后排列。
    pub const ALL: [Self; 2] = [Self::Destroy, Self::Return];

    /// 设置页里的显示名。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Destroy => "销毁",
            Self::Return => "退还",
        }
    }
}
