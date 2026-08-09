use crate::{
    diff,
    diff_view::{self, DiffViewConfig, DiffViewState},
    draft_page::{
        DocKey, DraftPage, DraftSession, ExportLinks, LoadedVersion, TOOLBAR_CONTROL_HEIGHT,
        editor_id, toolbar_separator,
    },
    export, knowledge,
    knowledge::KnowledgeStore,
    lmstudio, manuscript,
    manuscript::{
        ManuscriptFilter, ManuscriptRecord, ManuscriptRow, ManuscriptStore, ManuscriptUpdate,
        NewManuscript, VersionRecord,
    },
    manuscript_io,
    models::{
        AiPrompt, AppConfig, DraftInput, ExportSelection, FontConfig, FontRole, GeneratedDraft,
        ManuscriptStatus, RerankMode, SecurityLevel, TemplateKind, VocabularyCategory,
        VocabularyEntry, builtin_ai_prompts, join_units, split_units,
    },
    preview, prompt, rag, storage, system_fonts, texcompile, theme, units,
    units::UnitDisplay,
    validator, vocabulary_xlsx,
};
use anyhow::Context;
use eframe::egui;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

pub(crate) const ACCENT: egui::Color32 = theme::ACCENT;
pub(crate) const WARN: egui::Color32 = theme::WARN;
/// 四个汉字约 58 px，另留 18 px 呼吸空间；超长标签显式换行而不扩大此列。
pub(crate) const LABEL_WIDTH: f32 = 76.0;
/// 单选/多选下拉右侧“手填/选择”图标文字切换按钮占用的宽度。
pub(crate) const TOGGLE_WIDTH: f32 = 72.0;
/// 左侧表单的输入框、下拉框和切换按钮使用同一最小高度。
pub(crate) const FORM_CONTROL_HEIGHT: f32 = 30.0;
/// 最窄状态下输入控件仍需容纳约 8 个汉字及下拉箭头。
pub(crate) const FORM_FIELD_MIN_WIDTH: f32 = 150.0;
/// 标签列、字段、切换按钮之间的网格/控件间距预留。
pub(crate) const FORM_LAYOUT_GUTTER: f32 = 24.0;
/// 76 标签 + 150 字段 + 72 切换 + 24 间距 = 322 px。
pub(crate) const FORM_CONTENT_MIN_WIDTH: f32 =
    LABEL_WIDTH + FORM_FIELD_MIN_WIDTH + TOGGLE_WIDTH + FORM_LAYOUT_GUTTER;
/// 内容 322 + 面板左右内边距 24 + 非浮动滚动条及留白 18 = 364 px。
pub(crate) const FORM_PANEL_MIN_WIDTH: f32 = FORM_CONTENT_MIN_WIDTH + 42.0;
pub(crate) const FORM_PANEL_DEFAULT_WIDTH: f32 = 420.0;
/// 文稿标签的宽度区间：最窄仍能显示约 5 个汉字加关闭按钮。
const DOC_TAB_MIN_WIDTH: f32 = 132.0;
const DOC_TAB_MAX_WIDTH: f32 = 250.0;
/// 自动保存的间隔。太密会和输入抢 SQLite 写锁，太疏又护不住现场。
const AUTOSAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);
/// 标签标题超过这个字数就中部省略。
const DOC_TAB_TITLE_CHARS: usize = 14;
/// 标签里除标题以外的固定占位：内边距 + 状态标记 + 关闭按钮。
const DOC_TAB_CHROME_WIDTH: f32 = 74.0;
/// 标签条统一动效时长（秒）。悬停/按下短促，指示条滑动稍长，三者构成一套节奏。
const TAB_HOVER_ANIM: f32 = 0.12;
const TAB_PRESS_ANIM: f32 = 0.15;
const TAB_SELECT_ANIM: f32 = 0.16;
const TAB_CLOSE_ANIM: f32 = 0.15;
const TAB_INDICATOR_ANIM: f32 = 0.16;
pub(crate) const FORM_PANEL_MAX_WIDTH: f32 = 620.0;
/// 表单内容的宽度区间。可调整面板必须拿到确定的宽度：控件一旦请求
/// `f32::INFINITY`，面板会被内容顶到远超 `size_range` 的宽度。
pub(crate) const CONTENT_WIDTH: std::ops::RangeInclusive<f32> = FORM_CONTENT_MIN_WIDTH..=700.0;

/// 导航行上的常驻页面。起草不在其中——它由打开的稿件派生成标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavPage {
    Manuscript,
    Vocabulary,
    AiPrompts,
    Knowledge,
    Settings,
}

/// 标签栏上的一格：一篇打开的稿件，或一个导航页。稿件和导航页共用同一条
/// 标签栏——看设置不必丢掉稿件上下文，关掉设置就回到稿子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabRef {
    Doc(DocKey),
    Page(NavPage),
}

impl NavPage {
    /// 写进稿件库的稳定标识，用于会话恢复。改名会让旧记录失配，别改。
    fn key(self) -> &'static str {
        match self {
            Self::Manuscript => "manuscript",
            Self::Vocabulary => "vocabulary",
            Self::AiPrompts => "ai_prompts",
            Self::Knowledge => "knowledge",
            Self::Settings => "settings",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "manuscript" => Some(Self::Manuscript),
            "vocabulary" => Some(Self::Vocabulary),
            "ai_prompts" => Some(Self::AiPrompts),
            "knowledge" => Some(Self::Knowledge),
            "settings" => Some(Self::Settings),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Manuscript => "稿件管理",
            Self::Vocabulary => "标准词库",
            Self::AiPrompts => "AI 管理",
            Self::Knowledge => "知识库",
            Self::Settings => "设置",
        }
    }

    fn icon(self) -> theme::Icon {
        match self {
            Self::Manuscript => theme::Icon::Library,
            Self::Vocabulary => theme::Icon::Book,
            Self::AiPrompts => theme::Icon::WandSparkles,
            Self::Knowledge => theme::Icon::PackageOpen,
            Self::Settings => theme::Icon::Settings,
        }
    }
}

pub(crate) enum WorkerResult {
    /// 全局任务：探测 LM Studio 已加载的模型。
    Models(Result<Vec<String>, String>),
    /// 探测知识库 embedding 端点已加载的模型。
    EmbeddingModels(Result<Vec<String>, String>),
    /// 探测知识库 rerank 端点已加载的模型。
    RerankModels(Result<Vec<String>, String>),
    /// 真跑一次 rerank 的验证结果（端点路径 + 响应字段是否对得上）。
    RerankVerify(Result<String, String>),
    /// 某一篇稿件的任务结果。`key` 认稿件、`seq` 认这一次任务：稿件关了或者
    /// 同一篇又发起了新任务，回来的结果就已作废。
    Doc { key: DocKey, seq: u64, job: DocJob },
    /// 知识库任务：与具体稿件无关的全局任务（索引构建 / 检索测试）。
    Knowledge(KnowledgeJob),
    /// 扫描本机字体目录的结果。中文字体文件很大，扫描放在后台线程。
    SystemFonts(Vec<system_fonts::SystemFont>),
}

/// 知识库后台任务的结果。
pub(crate) enum KnowledgeJob {
    /// 索引进度（每篇回报一次）。
    IndexProgress {
        done: usize,
        total: usize,
        current_title: String,
    },
    /// 索引整批完成。`failed` 是 (标题, 错误)。
    IndexFinished {
        ok: usize,
        failed: Vec<(String, String)>,
    },
    /// 检索测试的结果。
    SearchDone(Result<rag::RetrievalOutcome, String>),
}

/// 起草页发起的后台任务的结果。
pub(crate) enum DocJob {
    /// 从零起草的结果。
    Drafted(Result<GeneratedDraft, String>),
    Optimized(Result<GeneratedDraft, String>),
    ExportProgress(String),
    Exported(Result<(Vec<PathBuf>, Option<String>), String>),
}

/// 同理，词库树上的增删和排序也要等本帧渲染完再改动 `Vec`。
enum VocabAction {
    /// 新增单位。`parent` 为空表示顶层单位。
    AddUnit {
        parent: String,
    },
    /// 在指定单位下新增人员。
    AddPerson {
        unit: String,
    },
    /// 删除词条；删除单位时连同其下级单位与人员一并删除。
    Delete(u64),
    /// 在同级之间上移/下移，随后重排层级编码。
    MoveUp(u64),
    MoveDown(u64),
    /// 清空当前标准词库。
    Clear,
}

/// 词库树上的一行：单位按层级缩进，人员是所属单位下面的末端子节点。
struct TreeRow {
    index: usize,
    id: u64,
    depth: usize,
    is_unit: bool,
    has_children: bool,
}

/// 稿件列表上的操作在遍历表格时不能直接改 `self`，先记下来循环结束后再执行。
enum ManuscriptAction {
    /// 打开只读详情（归档行或“查看”）。
    Detail(i64),
    /// 载入起草页继续编辑。
    Edit(i64),
    /// 复制该稿件的行文要素和正文，作为一篇尚未入库的新稿载入起草页。
    CreateFromExisting(i64),
    Publish(i64),
    RevertToDraft(i64),
    /// 归档。PDF 附件来自 `manuscript_archive_pending` 中已选的文件。
    Archive(i64),
    Delete(i64),
    DeleteSelected(Vec<i64>),
    /// 进入删除二次确认。
    DeletePending(i64),
    /// 进入归档确认（先选扫描盖章 PDF）。
    ArchivePending(i64),
    /// 打开版本对照窗，对照该版本与其上一版（旧在左、新在右）。
    DiffVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 打开版本对照窗，默认对照最新版与上一版。
    OpenVersionDiff {
        manuscript_id: i64,
    },
    /// 把某版本载入起草页继续编辑（不写活稿行）。
    LoadVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 进入"回退到该版本"的二次确认。
    RevertPending {
        manuscript_id: i64,
        version_number: i64,
    },
}

/// 详情窗内 PDF 附件的操作，同样延迟到帧末执行。
enum PdfAction {
    Open(i64),
    SaveAs(i64),
    Delete(i64),
}

/// 等待二次确认的归档操作：记录要归档的稿件与已选择的扫描盖章 PDF。
struct ArchivePending {
    manuscript_id: i64,
    pdf_paths: Vec<PathBuf>,
}

/// 导入 ZIP 的预览状态：清单、勾选、关键词过滤与是否跳过同源记录。
struct ImportPreview {
    manifest: manuscript_io::Manifest,
    zip_path: PathBuf,
    selected: Vec<bool>,
    keyword: String,
    skip_existing: bool,
}

/// 提交版本对话框打开的版本链：某篇稿件，或全局配置。
#[derive(Debug, Clone)]
pub(crate) enum VersionScope {
    Manuscript(i64),
    Config,
}

/// 提交版本对话框的输入：版本名称、注释、最近一次提交尝试的错误。
struct VersionCommitDraft {
    scope: VersionScope,
    name: String,
    comment: String,
    error: Option<String>,
}

/// 版本对照窗的选版状态。方向由字段名固定：`from` 恒为旧版、`to` 恒为新版，
/// 所以"变更前/变更后"不可能再被选反。
struct VersionDiffState {
    scope: VersionScope,
    /// 旧侧版本号；稿件的 v1 没有上一版，此时为 None，整篇算新增。
    from: Option<i64>,
    /// 新侧版本号。
    to: Option<i64>,
    /// 仅配置版用：新侧取"当前配置"而不是某个已提交版本。稿件版没有这个选项
    /// ——详情页看的稿件未必是起草页正在编辑的那篇，拿起草页内容当新侧会串稿。
    to_is_current_config: bool,
    view: DiffViewState,
}

/// 版本切换的目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionTarget {
    /// 回到稿件库活稿行（"当前未提交内容"）。
    Working,
    /// 某个已提交版本。
    Version(i64),
}

/// 切换版本前的三选确认：当前内容相对最新版有未提交修改，先问怎么处置。
pub(crate) struct VersionSwitchPrompt {
    pub(crate) manuscript_id: i64,
    pub(crate) target: VersionTarget,
    /// 当前内容所基于的版本号，用于文案。
    pub(crate) base_label: String,
}

/// 点“AI 优化”后弹出的提示词选择面板。`custom` 是一次性指令，用完即弃，
/// 不进提示词库。
#[derive(Default)]
struct AiPromptPicker {
    keyword: String,
    custom: String,
    /// 只列出适用于该文种的提示词；面板打开那一刻的文种，中途切换不影响。
    kind: TemplateKind,
}

/// AI 管理页右侧的编辑区。改动先落在这里，点“保存更改”才写回配置。
struct AiPromptDraft {
    /// 新建时为 None，保存时才分配 id。
    id: Option<u32>,
    name: String,
    instruction: String,
    kinds: Vec<TemplateKind>,
    builtin_key: String,
    error: Option<String>,
}

impl AiPromptDraft {
    fn from_entry(entry: &AiPrompt) -> Self {
        Self {
            id: Some(entry.id),
            name: entry.name.clone(),
            instruction: entry.instruction.clone(),
            kinds: entry.kinds.clone(),
            builtin_key: entry.builtin_key.clone(),
            error: None,
        }
    }

    fn blank() -> Self {
        Self {
            id: None,
            name: String::new(),
            instruction: String::new(),
            kinds: vec![],
            builtin_key: String::new(),
            error: None,
        }
    }
}

pub struct GongwenApp {
    config: AppConfig,
    /// 已打开的稿件，每篇一个起草页标签。空表示当前只在导航页里。
    docs: Vec<DraftSession>,
    /// `docs` 中当前显示的那一篇；`view` 不是 `View::Doc` 时无意义。
    active_doc: usize,
    /// 下一篇打开的稿件用的 key，只增不减。
    next_doc_key: DocKey,
    /// 起草页回传给外壳执行的动作，帧末统一处理。
    draft_actions: Vec<DraftAction>,
    /// 导出目录里最近的 tex/pdf/docx 索引，工具栏三枚成品入口共用一份。
    export_links: ExportLinks,
    models: Vec<String>,
    vocabulary_import_conflicts: Option<Vec<vocabulary_xlsx::Conflict>>,
    /// "关于公文助手"弹窗显隐。
    about_window_open: bool,
    vocabulary_filter: String,
    /// 词库树上当前选中的词条 id，右侧编辑区显示它的详情。
    vocabulary_selected: Option<u64>,
    /// 已折叠的单位 id；默认全部展开。
    vocabulary_collapsed: BTreeSet<u64>,
    /// 等待二次确认的删除目标。
    vocabulary_delete_confirm: Option<u64>,
    /// 清空整库需要单独二次确认。
    vocabulary_clear_confirm: bool,
    /// “AI 优化”按钮弹出的提示词选择面板；None 表示未打开。
    ai_prompt_picker: Option<AiPromptPicker>,
    /// AI 管理页当前编辑的提示词。
    ai_prompt_editor: Option<AiPromptDraft>,
    /// AI 管理页列表里选中的提示词 id。
    ai_prompt_selected: Option<u32>,
    /// 等待二次确认的提示词删除目标。
    ai_prompt_delete_confirm: Option<u32>,
    /// 内置输出标准预览区选的文种。独立于 `draft.kind`——在 AI 管理页翻看
    /// 别的文种，不该把起草页正在写的稿件换掉。
    ai_contract_preview_kind: TemplateKind,
    /// 稿件库（SQLite）。初始化失败时为 None，稿件管理页显示错误但不影响其他功能。
    manuscript_store: Option<ManuscriptStore>,
    manuscript_error: Option<String>,
    manuscript_filter: ManuscriptFilter,
    /// 上次已执行的过滤条件，用于判断过滤是否变化而需要重查。
    manuscript_applied: Option<ManuscriptFilter>,
    /// 任何增删改后置 true，强制下次重查列表。
    manuscript_dirty: bool,
    manuscript_rows: Vec<ManuscriptRow>,
    /// 稿件列表当前勾选项；筛选刷新时只保留仍在列表中的记录。
    manuscript_selected: BTreeSet<i64>,
    /// 各状态稿件数 [新建, 草稿, 发布, 归档]；新建恒为 0（已取消，见 manuscript.rs）。
    manuscript_count: [i64; 4],
    manuscript_delete_confirm: Option<i64>,
    manuscript_batch_delete_confirm: bool,
    manuscript_archive_pending: Option<ArchivePending>,
    manuscript_detail: Option<ManuscriptRecord>,
    manuscript_detail_delete_pdf: Option<i64>,
    manuscript_import_preview: Option<ImportPreview>,
    /// 提交版本对话框（稿件版或配置版）。
    version_commit: Option<VersionCommitDraft>,
    /// 版本对照窗。
    version_diff: Option<VersionDiffState>,
    /// 起草页版本切换的三选确认框。
    version_switch: Option<VersionSwitchPrompt>,
    /// "提交后再切换"：提交版本对话框成功后要跳到的目标。
    switch_after_commit: Option<VersionTarget>,
    /// 等待二次确认的"回退到该版本"。
    revert_confirm: Option<(i64, i64)>,
    /// 详情页当前稿件的版本历史，随 `refresh_detail` 一起载入。
    manuscript_versions: Vec<manuscript::VersionRow>,
    /// 配置版本历史窗是否打开。
    config_versions_open: bool,
    /// 等待二次确认“应用”的配置版本号。
    config_apply_confirm: Option<i64>,
    /// 标签栏的内容与顺序。稿件标签按 key 指向 `docs`。
    tabs: Vec<TabRef>,
    active_tab: usize,
    /// 关闭标签的二次确认：`docs` 里这一篇有未保存改动。
    close_confirm: Option<usize>,
    /// 退出前的汇总确认；None 表示没在退出流程里。
    exit_prompt: Option<ExitPrompt>,
    /// 汇总框已放行，下一帧真正关窗。
    exit_confirmed: bool,
    /// 上次定时自动保存的时刻。
    last_autosave: std::time::Instant,
    status: String,
    /// 全局任务（当前只有模型探测）。稿件自己的生成/导出记在各自的会话上。
    busy: bool,
    sender: Sender<WorkerResult>,
    receiver: Receiver<WorkerResult>,
    /// 知识库存储（与稿件库同一文件，独立连接）。打开失败不阻塞启动。
    pub(crate) knowledge_store: Option<KnowledgeStore>,
    pub(crate) knowledge_error: Option<String>,
    /// 知识库文档列表，按文种过滤。
    pub(crate) knowledge_docs: Vec<knowledge::KnowledgeDocRow>,
    pub(crate) knowledge_chunk_count: i64,
    pub(crate) knowledge_filter_kind: Option<TemplateKind>,
    pub(crate) knowledge_dirty: bool,
    pub(crate) knowledge_delete_confirm: Option<i64>,
    /// 索引进度：(done, total, current_title)。
    pub(crate) knowledge_index_progress: Option<(usize, usize, String)>,
    /// 上次索引的结果摘要。
    pub(crate) knowledge_index_result: Option<String>,
    /// 检索测试框。
    pub(crate) knowledge_test_query: String,
    pub(crate) knowledge_test_results: Vec<rag::RetrievedChunk>,
    /// 上次检索的降级告警（embedding / rerank 不可用等），显示在检索区。
    pub(crate) knowledge_search_warnings: Vec<String>,
    /// 已入库的稿件 id，稿件管理列表据此打「已入库」标记。
    pub(crate) knowledge_indexed_manuscripts: std::collections::HashSet<i64>,
    /// 库内出现过的 embedding 模型名，与当前配置不符时提示重建索引。
    pub(crate) knowledge_embed_models: Vec<String>,
    /// 知识库后台任务在跑（索引/检索测试）。
    pub(crate) knowledge_busy: bool,
    /// 外部 md 导入对话框：待导入的文件路径与所选文种。
    pub(crate) knowledge_import: Option<KnowledgeImportDraft>,
    /// 探测到的 embedding / rerank 端点模型列表；空表示尚未探测，退回手填。
    pub(crate) embedding_models: Vec<String>,
    pub(crate) rerank_models: Vec<String>,
    /// embedding / rerank 探测是否正在进行。
    pub(crate) embedding_probe_busy: bool,
    pub(crate) rerank_probe_busy: bool,
    /// rerank 端点验证结果：(是否通过, 说明)。
    pub(crate) rerank_verify_result: Option<(bool, String)>,
    /// 知识库文档预览弹窗：标题、文种、原文。
    pub(crate) knowledge_preview: Option<KnowledgePreviewState>,
    /// 扫描到的本机字体；空表示还没扫过，设置页会在需要时触发一次。
    pub(crate) system_fonts: Vec<system_fonts::SystemFont>,
    pub(crate) system_fonts_busy: bool,
    /// 是否已经扫过一次本机字体。扫描结果为空（比如没有任何字体目录）时也
    /// 只扫一次，避免设置页每次重进都重新扫盘。
    pub(crate) system_fonts_scanned: bool,
    /// 每个字体下拉框的筛选词，按 `FontRole::key()` 存。本机字体动辄几百个，
    /// 没有筛选框的下拉列表没法用。
    pub(crate) font_filter: BTreeMap<&'static str, String>,
}

/// 知识库文档预览弹窗的状态。
pub(crate) struct KnowledgePreviewState {
    pub(crate) title: String,
    pub(crate) kind: TemplateKind,
    pub(crate) markdown: String,
    /// 预览缩放；None 表示按窗口宽度自适应。
    pub(crate) zoom: Option<f32>,
    pub(crate) fit_scale: f32,
}

/// 外部 markdown 导入对话框的草稿：选了哪些文件、归到哪个文种。
pub(crate) struct KnowledgeImportDraft {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) kind: TemplateKind,
}

/// 退出前的汇总确认。上区必须处理，下区只是提醒。
struct ExitPrompt {
    /// 有改动没写库的稿件：`(docs 下标, 是否勾选保存)`。默认全勾。
    unsaved: Vec<(usize, bool)>,
    /// 已存库但没提交版本的稿件：`(docs 下标, 是否勾选提交)`。默认不勾。
    uncommitted: Vec<(usize, bool)>,
}

/// 起草页需要外壳代办的事：这些动作要么改的是外壳自己的状态，要么会
/// 重排 `docs`，在借出会话的那一帧里做不了，统一延到帧末。
pub(crate) enum DraftAction {
    /// 保存当前稿件到稿件库（新建或更新）。
    SaveToLibrary,
    /// 打开提交版本对话框。
    OpenVersionCommit(VersionScope),
    /// 打开“AI 优化”的提示词选择面板。
    OpenAiPromptPicker,
    /// 打开版本对照窗：`to` 与它的上一版比。
    OpenVersionDiff { manuscript_id: i64, to: i64 },
    /// 把已发布的稿件退回草稿，好继续编辑。
    RevertToDraft(i64),
    /// 把某个已提交版本载入当前起草页。
    LoadManuscriptVersion {
        manuscript_id: i64,
        version_number: i64,
    },
    /// 把当前版式记进配置并落盘。
    Persist,
}

impl GongwenApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut config = storage::load().unwrap_or_default();
        // 预览字体要按配置装，所以先读配置再装字体。
        theme::configure_fonts(&cc.egui_ctx, &config.fonts);
        theme::configure_icons(&cc.egui_ctx);
        theme::configure_style(&cc.egui_ctx);

        // 载入即整理：补齐词条 id，按层级重排单位并重新生成层级编码。
        units::normalize(&mut config.vocabulary);
        // 旧配置没有提示词库，这里补齐预置项并给未编号的条目分配 id。
        config.ensure_ai_prompts();
        if config.output_dir.trim().is_empty() {
            config.output_dir = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("output")
                .display()
                .to_string();
        }
        let kind = config.last_template;
        let (sender, receiver) = mpsc::channel();
        // 先摆一篇空白稿兜底；下面会话恢复成功就把它换掉。
        let docs = vec![DraftSession::blank(0, &config)];
        // 稿件库打开失败不阻塞启动：其余标签页照常可用，稿件管理页显示错误。
        let (manuscript_store, manuscript_error) = match storage::manuscript_db_path() {
            Ok(path) => match manuscript::ManuscriptStore::open(&path) {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(format!("稿件库打开失败：{error:#}"))),
            },
            Err(error) => (None, Some(format!("稿件库路径获取失败：{error:#}"))),
        };
        // 知识库与稿件库同一文件、独立连接；打开失败同样不阻塞启动。
        let (knowledge_store, knowledge_error) = match storage::manuscript_db_path() {
            Ok(path) => match knowledge::KnowledgeStore::open(&path) {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(format!("知识库打开失败：{error:#}"))),
            },
            Err(error) => (None, Some(format!("知识库路径获取失败：{error:#}"))),
        };
        let mut app = Self {
            config,
            docs,
            active_doc: 0,
            next_doc_key: 1,
            draft_actions: Vec::new(),
            export_links: ExportLinks::default(),
            models: Vec::new(),
            vocabulary_import_conflicts: None,
            about_window_open: false,
            vocabulary_filter: String::new(),
            vocabulary_selected: None,
            vocabulary_collapsed: BTreeSet::new(),
            vocabulary_delete_confirm: None,
            vocabulary_clear_confirm: false,
            ai_prompt_picker: None,
            ai_prompt_editor: None,
            ai_prompt_selected: None,
            ai_prompt_delete_confirm: None,
            ai_contract_preview_kind: kind,
            manuscript_store,
            manuscript_error,
            manuscript_filter: ManuscriptFilter::default(),
            manuscript_applied: None,
            manuscript_dirty: true,
            manuscript_rows: Vec::new(),
            manuscript_selected: BTreeSet::new(),
            manuscript_count: [0; 4],
            manuscript_delete_confirm: None,
            manuscript_batch_delete_confirm: false,
            manuscript_archive_pending: None,
            manuscript_detail: None,
            manuscript_detail_delete_pdf: None,
            manuscript_import_preview: None,
            version_commit: None,
            version_diff: None,
            version_switch: None,
            switch_after_commit: None,
            revert_confirm: None,
            manuscript_versions: Vec::new(),
            config_versions_open: false,
            config_apply_confirm: None,
            tabs: vec![TabRef::Doc(0)],
            active_tab: 0,
            close_confirm: None,
            exit_prompt: None,
            exit_confirmed: false,
            last_autosave: std::time::Instant::now(),
            status: "就绪。先在“设置”中连接 LM Studio。".into(),
            busy: false,
            sender,
            receiver,
            knowledge_store,
            knowledge_error,
            knowledge_docs: Vec::new(),
            knowledge_chunk_count: 0,
            knowledge_filter_kind: None,
            knowledge_dirty: true,
            knowledge_delete_confirm: None,
            knowledge_index_progress: None,
            knowledge_index_result: None,
            knowledge_test_query: String::new(),
            knowledge_test_results: Vec::new(),
            knowledge_search_warnings: Vec::new(),
            knowledge_indexed_manuscripts: std::collections::HashSet::new(),
            knowledge_embed_models: Vec::new(),
            knowledge_busy: false,
            knowledge_import: None,
            embedding_models: Vec::new(),
            rerank_models: Vec::new(),
            system_fonts: Vec::new(),
            system_fonts_busy: false,
            system_fonts_scanned: false,
            font_filter: BTreeMap::new(),
            embedding_probe_busy: false,
            rerank_probe_busy: false,
            rerank_verify_result: None,
            knowledge_preview: None,
        };
        app.restore_session();
        app
    }

    /// 当前标签是不是稿件。
    fn showing_doc(&self) -> bool {
        matches!(self.tabs.get(self.active_tab), Some(TabRef::Doc(_)))
    }

    fn doc_index_of_key(&self, key: DocKey) -> Option<usize> {
        self.docs.iter().position(|doc| doc.key == key)
    }

    /// 切到某一格标签。稿件标签同时把 `active_doc` 指过去。
    fn activate_tab(&mut self, tab: usize) {
        let Some(&item) = self.tabs.get(tab) else {
            return;
        };
        // 切走之前先把手上这篇写回去，免得改了半天停在别的标签上没存。
        if tab != self.active_tab {
            self.autosave_all();
        }
        self.active_tab = tab;
        if let TabRef::Doc(key) = item
            && let Some(index) = self.doc_index_of_key(key)
        {
            self.active_doc = index;
        }
    }

    /// 切到某一篇稿件（按 `docs` 下标）。
    fn activate_doc(&mut self, index: usize) {
        let Some(key) = self.docs.get(index).map(|doc| doc.key) else {
            return;
        };
        if let Some(tab) = self.tabs.iter().position(|item| *item == TabRef::Doc(key)) {
            self.activate_tab(tab);
        }
    }

    /// 新开一个稿件标签并切过去。
    fn open_doc(&mut self, mut session: DraftSession) {
        session.key = self.next_doc_key;
        self.next_doc_key += 1;
        let key = session.key;
        self.docs.push(session);
        self.tabs.push(TabRef::Doc(key));
        self.activate_tab(self.tabs.len() - 1);
        self.draft_page().revalidate();
        self.remember_session();
    }

    /// 打开导航页。同一个页面不重复开，已经在就切过去。
    fn open_page(&mut self, page: NavPage) {
        match self
            .tabs
            .iter()
            .position(|item| *item == TabRef::Page(page))
        {
            Some(tab) => self.activate_tab(tab),
            None => {
                self.tabs.push(TabRef::Page(page));
                self.activate_tab(self.tabs.len() - 1);
                self.remember_session();
            }
        }
    }

    /// 同一篇稿件不重复打开：已经开着就切过去。
    fn focus_manuscript(&mut self, id: i64) -> bool {
        let found = self
            .docs
            .iter()
            .position(|doc| doc.manuscript_id == Some(id));
        match found {
            Some(index) => {
                self.activate_doc(index);
                true
            }
            None => false,
        }
    }

    /// 关闭标签。稿件有未保存改动时先弹确认，别把人写了一半的稿子直接扔掉。
    fn request_close_tab(&mut self, tab: usize) {
        match self.tabs.get(tab) {
            Some(TabRef::Doc(key)) => {
                let Some(index) = self.doc_index_of_key(*key) else {
                    self.close_tab(tab);
                    return;
                };
                if self.docs[index].is_dirty() {
                    self.close_confirm = Some(index);
                } else {
                    self.close_tab(tab);
                }
            }
            Some(TabRef::Page(_)) => self.close_tab(tab),
            None => {}
        }
    }

    fn close_tab(&mut self, tab: usize) {
        if tab >= self.tabs.len() {
            return;
        }
        if let TabRef::Doc(key) = self.tabs[tab]
            && let Some(index) = self.doc_index_of_key(key)
        {
            self.docs.remove(index);
            if index < self.active_doc {
                self.active_doc -= 1;
            }
            self.active_doc = self.active_doc.min(self.docs.len().saturating_sub(1));
        }
        self.tabs.remove(tab);
        if self.tabs.is_empty() {
            // 一格不剩就回到稿件管理，而不是停在空白页。
            self.open_page(NavPage::Manuscript);
            return;
        }
        // 关掉当前这格时顺位接管后一格；关掉前面的则保持指着同一格。
        let next = if tab < self.active_tab {
            self.active_tab - 1
        } else {
            self.active_tab
        };
        self.activate_tab(next.min(self.tabs.len() - 1));
        self.remember_session();
    }

    /// 关闭未保存稿件的二次确认。
    fn close_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.close_confirm else {
            return;
        };
        let Some(doc) = self.docs.get(index) else {
            self.close_confirm = None;
            return;
        };
        let title = doc.title();
        let key = doc.key;
        let mut decision: Option<bool> = None;
        let mut cancel = false;
        egui::Window::new("关闭稿件")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("《{title}》有未保存的改动。"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::Save, "保存并关闭").clicked()
                    {
                        decision = Some(true);
                    }
                    if ui
                        .add(theme::secondary_icon_button(theme::Icon::Trash, "不保存"))
                        .clicked()
                    {
                        decision = Some(false);
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.close_confirm = None;
            return;
        }
        let Some(save) = decision else {
            return;
        };
        self.close_confirm = None;
        if save {
            let previous = self.active_doc;
            self.active_doc = index;
            self.save_to_manuscript_library();
            let saved = !self.docs[index].is_dirty();
            self.active_doc = previous;
            if !saved {
                // 保存失败（归档稿、库不可用等），保留标签，状态栏已说明原因。
                return;
            }
        }
        if let Some(tab) = self.tabs.iter().position(|item| *item == TabRef::Doc(key)) {
            self.close_tab(tab);
        }
    }

    /// 关窗请求的入口。有未保存或未提交的稿件就先拦下来问一次。
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        if self.exit_confirmed {
            return;
        }
        // 开着自动保存时先静默存一轮，能不打扰就不打扰。
        self.autosave_all();
        let unsaved: Vec<(usize, bool)> = self
            .docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.is_dirty())
            .map(|(index, _)| (index, true))
            .collect();
        let uncommitted: Vec<(usize, bool)> = self
            .docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.has_uncommitted())
            .map(|(index, _)| (index, false))
            .collect();
        if unsaved.is_empty() && uncommitted.is_empty() {
            self.remember_session();
            self.exit_confirmed = true;
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.exit_prompt = Some(ExitPrompt {
            unsaved,
            uncommitted,
        });
    }

    /// 退出汇总框：上区未保存必须处理，下区未提交版本只是提醒。
    fn exit_prompt_window(&mut self, ctx: &egui::Context) {
        let Some(mut prompt) = self.exit_prompt.take() else {
            return;
        };
        let mut decision: Option<bool> = None;
        let mut cancel = false;
        egui::Window::new("退出公文助手")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                if !prompt.unsaved.is_empty() {
                    ui.label(egui::RichText::new("以下稿件有未保存的改动").strong());
                    for (index, keep) in prompt.unsaved.iter_mut() {
                        let title = self.docs[*index].title();
                        ui.checkbox(keep, format!("保存《{title}》"));
                    }
                    ui.add_space(6.0);
                }
                if !prompt.uncommitted.is_empty() {
                    ui.label(
                        egui::RichText::new("以下稿件已存库，但相对最新版本还有改动").strong(),
                    );
                    ui.weak("不处理也可以，下次打开继续改；勾选则退出前顺手固化一个版本。");
                    for (index, commit) in prompt.uncommitted.iter_mut() {
                        let title = self.docs[*index].title();
                        ui.checkbox(commit, format!("提交《{title}》的新版本"));
                    }
                    ui.add_space(6.0);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::Save, "处理所选并退出").clicked()
                    {
                        decision = Some(true);
                    }
                    if ui
                        .add(theme::secondary_icon_button(theme::Icon::X, "直接退出"))
                        .on_hover_text("放弃所有未保存的改动")
                        .clicked()
                    {
                        decision = Some(false);
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            return;
        }
        let Some(apply) = decision else {
            self.exit_prompt = Some(prompt);
            return;
        };
        if apply {
            self.apply_exit_actions(&prompt);
        }
        self.remember_session();
        self.exit_confirmed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// 按汇总框的勾选逐篇保存、逐篇提交版本。
    fn apply_exit_actions(&mut self, prompt: &ExitPrompt) {
        let previous = self.active_doc;
        for (index, keep) in &prompt.unsaved {
            if *keep && *index < self.docs.len() {
                self.active_doc = *index;
                self.save_to_manuscript_library();
            }
        }
        for (index, commit) in &prompt.uncommitted {
            if !*commit || *index >= self.docs.len() {
                continue;
            }
            let Some(id) = self.docs[*index].manuscript_id else {
                continue;
            };
            self.active_doc = *index;
            let existing = self
                .manuscript_store
                .as_mut()
                .and_then(|store| store.list_manuscript_versions(id).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|row| row.name)
                .collect::<Vec<_>>();
            let draft = VersionCommitDraft {
                scope: VersionScope::Manuscript(id),
                name: unique_version_name(&existing, &default_version_name()),
                comment: "退出前自动提交".into(),
                error: None,
            };
            if let Err(error) = self.run_version_commit(&draft) {
                self.status = format!("退出前提交版本失败：{error:#}");
            }
        }
        self.active_doc = previous.min(self.docs.len().saturating_sub(1));
    }

    /// 刷新某篇的“最新已提交版本”基线。打开稿件、保存、提交版本、载入版本
    /// 之后都要刷一次，否则标签上的空心圈会停在旧结论上。
    fn refresh_committed_baseline(&mut self, index: usize) {
        let Some(id) = self.docs.get(index).and_then(|doc| doc.manuscript_id) else {
            if let Some(doc) = self.docs.get_mut(index) {
                doc.set_committed_baseline(None);
            }
            return;
        };
        let latest = self
            .manuscript_store
            .as_ref()
            .and_then(|store| store.latest_manuscript_content(id).ok())
            .flatten()
            .map(|(json, content, _)| (json, content));
        if let Some(doc) = self.docs.get_mut(index) {
            doc.set_committed_baseline(latest);
        }
    }

    /// 稿件的生命周期状态在别处改过之后，把已打开的标签同步过来：
    /// 发布/归档会让标签转只读，退回草稿会让它重新可编辑。
    fn sync_record_status(&mut self, id: i64) {
        let status = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.get(id).ok())
            .flatten()
            .map(|record| record.status);
        let Some(status) = status else {
            return;
        };
        for doc in self.docs.iter_mut().filter(|d| d.manuscript_id == Some(id)) {
            doc.record_status = status;
        }
    }

    /// 稿件在管理页被删除或归档后，已打开的标签就不再对应库里那条记录了。
    /// 断开关联而不是关掉标签：内容还在，用户可以另存为新稿。
    fn detach_docs_of(&mut self, id: i64) {
        for doc in self.docs.iter_mut().filter(|d| d.manuscript_id == Some(id)) {
            doc.manuscript_id = None;
            doc.loaded_version = None;
            doc.saved_baseline = None;
            doc.committed_baseline = None;
        }
    }

    /// 全局任务或任意一篇稿件的任务在跑。
    fn any_busy(&self) -> bool {
        self.busy || self.knowledge_busy || self.docs.iter().any(|doc| doc.busy)
    }

    /// 当前标签是稿件时返回它；停在导航页时为 None。
    fn active_doc_ref(&self) -> Option<&DraftSession> {
        self.docs.get(self.active_doc)
    }

    fn doc(&self) -> &DraftSession {
        &self.docs[self.active_doc]
    }

    fn doc_mut(&mut self) -> &mut DraftSession {
        &mut self.docs[self.active_doc]
    }

    /// 借出当前会话与它需要的应用级资源，组成起草页这一帧的执行上下文。
    /// 各字段是**互不相交**的可变借用，所以能同时借出而不冲突。
    fn draft_page(&mut self) -> DraftPage<'_> {
        self.draft_page_at(self.active_doc)
    }

    fn draft_page_at(&mut self, index: usize) -> DraftPage<'_> {
        DraftPage {
            doc: &mut self.docs[index],
            config: &mut self.config,
            store: self.manuscript_store.as_mut(),
            sender: &self.sender,
            status: &mut self.status,
            version_switch: &mut self.version_switch,
            revert_confirm: &mut self.revert_confirm,
            actions: &mut self.draft_actions,
            export_links: &mut self.export_links,
        }
    }

    /// 帧末执行起草页回传的动作。
    fn apply_draft_actions(&mut self) {
        for action in std::mem::take(&mut self.draft_actions) {
            match action {
                DraftAction::SaveToLibrary => self.save_to_manuscript_library(),
                DraftAction::OpenVersionCommit(scope) => self.open_version_commit(scope),
                DraftAction::OpenAiPromptPicker => self.open_ai_prompt_picker(),
                DraftAction::OpenVersionDiff { manuscript_id, to } => {
                    self.version_diff = Some(VersionDiffState {
                        scope: VersionScope::Manuscript(manuscript_id),
                        from: (to > 1).then_some(to - 1),
                        to: Some(to),
                        to_is_current_config: false,
                        view: DiffViewState::default(),
                    });
                }
                DraftAction::RevertToDraft(id) => {
                    self.transition_status(id, ManuscriptStatus::Draft);
                    self.sync_record_status(id);
                }
                DraftAction::LoadManuscriptVersion {
                    manuscript_id,
                    version_number,
                } => self.load_manuscript_version(manuscript_id, version_number),
                DraftAction::Persist => self.persist(),
            }
        }
    }

    /// 启动时按上次退出的标签重建现场。稿件已被删掉的行静默跳过；
    /// 一格都恢复不出来就保留启动时那篇空白稿。
    fn restore_session(&mut self) {
        let Some(store) = self.manuscript_store.as_mut() else {
            return;
        };
        let Ok((saved, active)) = store.load_open_tabs() else {
            return;
        };
        if saved.is_empty() {
            return;
        }
        let mut tabs: Vec<TabRef> = Vec::new();
        let mut docs: Vec<DraftSession> = Vec::new();
        let mut next_key = 0;
        let mut restored_active = 0;
        for (ord, tab) in saved.iter().enumerate() {
            let item = match tab {
                manuscript::OpenTab::Manuscript(id) => {
                    let Some(store) = self.manuscript_store.as_mut() else {
                        continue;
                    };
                    let Ok(Some(record)) = store.get(*id) else {
                        continue;
                    };
                    let mut session = DraftSession::from_parts(
                        next_key,
                        Some(record.id),
                        record.snapshot,
                        record.content_markdown,
                    );
                    session.record_status = record.status;
                    session.mark_saved();
                    next_key += 1;
                    let key = session.key;
                    docs.push(session);
                    TabRef::Doc(key)
                }
                manuscript::OpenTab::Page(name) => match NavPage::from_key(name) {
                    Some(page) => TabRef::Page(page),
                    None => continue,
                },
            };
            if ord == active {
                restored_active = tabs.len();
            }
            tabs.push(item);
        }
        if tabs.is_empty() {
            return;
        }
        self.docs = docs;
        self.tabs = tabs;
        self.next_doc_key = next_key;
        self.activate_tab(restored_active.min(self.tabs.len() - 1));
        for index in 0..self.docs.len() {
            self.refresh_committed_baseline(index);
            self.draft_page_at(index).revalidate();
        }
        self.status = format!("已恢复上次打开的 {} 个标签。", self.tabs.len());
    }

    /// 把当前标签写进稿件库，供下次启动恢复。未入库的新稿没有身份，跳过。
    fn remember_session(&mut self) {
        let tabs: Vec<manuscript::OpenTab> = self
            .tabs
            .iter()
            .filter_map(|item| match item {
                TabRef::Doc(key) => {
                    let index = self.doc_index_of_key(*key)?;
                    let id = self.docs[index].manuscript_id?;
                    Some(manuscript::OpenTab::Manuscript(id))
                }
                TabRef::Page(page) => Some(manuscript::OpenTab::Page(page.key().to_string())),
            })
            .collect();
        // 过滤掉未入库的稿件后下标会错位，按身份重新定位当前这一格。
        let active = self
            .tabs
            .get(self.active_tab)
            .and_then(|item| match item {
                TabRef::Doc(key) => {
                    let index = self.doc_index_of_key(*key)?;
                    let id = self.docs[index].manuscript_id?;
                    tabs.iter()
                        .position(|t| *t == manuscript::OpenTab::Manuscript(id))
                }
                TabRef::Page(page) => tabs
                    .iter()
                    .position(|t| *t == manuscript::OpenTab::Page(page.key().to_string())),
            })
            .unwrap_or(0);
        if let Some(store) = self.manuscript_store.as_mut() {
            let _ = store.save_open_tabs(&tabs, active);
        }
    }

    /// 尚未入库的新稿，一旦真的动过就静默建一条草稿记录。此后它就有了身份，
    /// 自动保存、会话恢复、版本链才有落脚点。
    fn auto_create_touched_doc(&mut self) {
        if !self.showing_doc() {
            return;
        }
        let Some(doc) = self.docs.get(self.active_doc) else {
            return;
        };
        if doc.manuscript_id.is_some() || doc.busy || !doc.touched() {
            return;
        }
        self.save_to_manuscript_library();
        self.remember_session();
    }

    /// 定时自动保存：把有改动且已入库的稿件静默写回。
    fn autosave_tick(&mut self) {
        if !self.config.auto_save {
            return;
        }
        if self.last_autosave.elapsed() < AUTOSAVE_INTERVAL {
            return;
        }
        self.last_autosave = std::time::Instant::now();
        self.autosave_all();
    }

    fn autosave_all(&mut self) {
        if !self.config.auto_save {
            return;
        }
        let targets: Vec<usize> = (0..self.docs.len())
            .filter(|&index| {
                let doc = &self.docs[index];
                doc.manuscript_id.is_some() && doc.is_dirty() && !doc.busy
            })
            .collect();
        let previous = self.active_doc;
        for index in targets {
            self.active_doc = index;
            self.save_to_manuscript_library();
        }
        self.active_doc = previous.min(self.docs.len().saturating_sub(1));
    }

    fn persist(&mut self) {
        // 设置页可在没有任何稿件标签时打开（关掉全部 Doc 标签后 docs 为空），
        // 此时不能索引 docs[active_doc]，否则越界 panic 直接退出进程。
        // 先取出所需的不变数据结束对 docs 的借用，再写回 config。
        let current = self
            .active_doc_ref()
            .map(|doc| (doc.draft.kind, doc.draft.profile.clone()));
        if let Some((kind, profile)) = current {
            self.config.last_template = kind;
            self.config.upsert_profile(profile);
        }
        match storage::save(&self.config) {
            Ok(()) => self.status = "配置已保存到本机。".into(),
            Err(error) => self.status = format!("保存配置失败：{error:#}"),
        }
    }

    /// 应用级快捷键要在各个文本框处理输入前消费，避免 `Ctrl+S` / `Ctrl+F`
    /// 被当前聚焦的编辑控件吞掉。
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let save = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::S);
        if ctx.input_mut(|input| input.consume_shortcut(&save)) && self.showing_doc() {
            self.save_to_manuscript_library();
        }

        let new_doc = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::N);
        if ctx.input_mut(|input| input.consume_shortcut(&new_doc)) {
            self.new_blank_manuscript();
        }

        if self.showing_doc() && !self.docs.is_empty() {
            let close = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::W);
            if ctx.input_mut(|input| input.consume_shortcut(&close)) {
                self.request_close_tab(self.active_tab);
            }
            let next = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Tab);
            if ctx.input_mut(|input| input.consume_shortcut(&next)) {
                self.activate_doc((self.active_doc + 1) % self.docs.len());
            }
            let prev = egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::Tab,
            );
            if ctx.input_mut(|input| input.consume_shortcut(&prev)) {
                self.activate_doc((self.active_doc + self.docs.len() - 1) % self.docs.len());
            }
        }
        // Ctrl+1..9 直达第 N 个标签，第 9 个固定指最后一篇。
        for (offset, key) in [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ]
        .into_iter()
        .enumerate()
        {
            let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, key);
            if ctx.input_mut(|input| input.consume_shortcut(&shortcut)) && !self.docs.is_empty() {
                let index = if offset == 8 {
                    self.docs.len() - 1
                } else {
                    offset.min(self.docs.len() - 1)
                };
                self.activate_doc(index);
            }
        }

        let find = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::F);
        if ctx.input_mut(|input| input.consume_shortcut(&find)) && self.showing_doc() {
            if !self.doc().markdown_find.open
                && let Some(state) = egui::TextEdit::load_state(ctx, editor_id())
                && let Some(range) = state.cursor.char_range()
                && !range.is_empty()
            {
                let selected = range.slice_str(&self.doc().generated_markdown);
                if !selected.contains('\n') && selected.chars().count() <= 200 {
                    self.doc_mut().markdown_find.query = selected.to_owned();
                    self.doc_mut().markdown_find.current = 0;
                }
            }
            self.doc_mut().markdown_find.open = true;
            self.doc_mut().markdown_find.focus_query = true;
        }

        // 查找条与结果抽屉同时打开时，Esc 先交给查找条；否则收起结果抽屉。
        if self.showing_doc()
            && self.doc().result_drawer_open
            && !self.doc().markdown_find.open
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.doc_mut().result_drawer_open = false;
        }
    }

    fn start_model_probe(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "正在连接 LM Studio 并读取已加载模型…".into();
        let config = self.config.lm_studio.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = lmstudio::list_models(&config).map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::Models(result));
        });
    }

    /// 探测知识库 embedding 端点已加载的模型。
    /// 扫描本机字体。中文字体文件动辄十几兆，整轮扫描要一两秒，放后台线程做。
    fn start_system_font_scan(&mut self) {
        if self.system_fonts_busy {
            return;
        }
        self.system_fonts_busy = true;
        self.status = "正在扫描本机字体…".into();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let _ = tx.send(WorkerResult::SystemFonts(system_fonts::scan()));
        });
    }

    fn start_embedding_probe(&mut self) {
        if self.embedding_probe_busy {
            return;
        }
        self.embedding_probe_busy = true;
        self.status = "正在连接 embedding 服务并读取模型…".into();
        let base_url = self.config.rag.embedding.base_url.clone();
        let api_key = self.config.rag.embedding.api_key.clone();
        let timeout = self.config.rag.embedding.timeout_seconds;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = lmstudio::list_models_at(&base_url, &api_key, timeout)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::EmbeddingModels(result));
        });
    }

    /// 探测知识库 rerank 端点已加载的模型。
    fn start_rerank_probe(&mut self) {
        if self.rerank_probe_busy {
            return;
        }
        self.rerank_probe_busy = true;
        self.status = "正在连接 rerank 服务并读取模型…".into();
        let base_url = self.config.rag.rerank.base_url.clone();
        let api_key = self.config.rag.rerank.api_key.clone();
        let timeout = self.config.rag.rerank.timeout_seconds;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = lmstudio::list_models_at(&base_url, &api_key, timeout)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::RerankModels(result));
        });
    }

    /// 真跑一次 rerank，验证端点路径与响应字段是否对得上。
    ///
    /// 只查 `/v1/models` 是不够的：LM Studio 对不认识的路径会记
    /// `Unexpected endpoint or method` 却仍返回 200，于是"连接成功"，
    /// 而每次检索的 rerank 都在静默失败。
    fn start_rerank_verify(&mut self) {
        if self.rerank_probe_busy {
            return;
        }
        let mode = self.config.rag.rerank.mode;
        match mode {
            RerankMode::None => {
                self.status = "当前未启用重排，无需验证。".into();
                return;
            }
            RerankMode::Api if self.config.rag.rerank.model.trim().is_empty() => {
                self.status = "请先填写或选择 rerank 模型，再验证。".into();
                return;
            }
            RerankMode::Llm if self.config.lm_studio.model.trim().is_empty() => {
                self.status = "请先在上面的「对话模型」里选好模型，再验证。".into();
                return;
            }
            _ => {}
        }
        self.rerank_probe_busy = true;
        self.status = "正在验证重排…".into();
        let cfg = self.config.rag.rerank.clone();
        let chat = self.config.lm_studio.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = match mode {
                RerankMode::Llm => crate::rag_client::probe_rerank_llm(&chat),
                _ => crate::rag_client::probe_rerank(&cfg),
            }
            .map(|n| format!("重排验证通过，返回 {n} 条排序结果。"))
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerResult::RerankVerify(result));
        });
    }

    /// 打开提示词选择面板。面板记下打开那一刻的文种，只列适用条目。
    fn open_ai_prompt_picker(&mut self) {
        if self.doc().busy {
            return;
        }
        if !self.draft_page().can_optimize() {
            self.status = "还没有可优化的内容：请先在右侧粘贴稿件，或填写写作素材。".into();
            return;
        }
        self.ai_prompt_picker = Some(AiPromptPicker {
            kind: self.doc().draft.kind,
            ..Default::default()
        });
    }

    fn import_vocabulary_xlsx(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel 词库", &["xlsx"])
            .pick_file()
        else {
            return;
        };
        self.vocabulary_import_conflicts = None;
        let result = (|| -> Result<(usize, vocabulary_xlsx::MergeReport), String> {
            let existing_codes: Vec<String> = self
                .config
                .vocabulary
                .iter()
                .filter(|e| e.category == VocabularyCategory::Unit)
                .map(|e| e.code.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            let report =
                vocabulary_xlsx::parse(&path, &existing_codes).map_err(|error| error.summary)?;
            let parsed_count = report.entries.len();
            let merge = vocabulary_xlsx::merge(&mut self.config.vocabulary, report.entries);
            units::normalize(&mut self.config.vocabulary);
            storage::save(&self.config).map_err(|error| format!("保存词库失败：{error:#}"))?;
            Ok((parsed_count, merge))
        })();
        match result {
            Ok((parsed, merge)) => {
                self.status = format!(
                    "已从“{}”解析 {} 条：新增 {} 条、更新 {} 条、未变化 {} 条。",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Excel 词库"),
                    parsed,
                    merge.added,
                    merge.updated,
                    merge.unchanged
                );
            }
            Err(summary) => {
                // 重新解析 conflict 列表用于渲染。如要优化可把 conflicts 放在错误里。
                let existing_codes: Vec<String> = self
                    .config
                    .vocabulary
                    .iter()
                    .filter(|e| e.category == VocabularyCategory::Unit)
                    .map(|e| e.code.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect();
                let conflicts = vocabulary_xlsx::parse(&path, &existing_codes)
                    .err()
                    .map(|e| e.conflicts)
                    .unwrap_or_default();
                self.vocabulary_import_conflicts = if conflicts.is_empty() {
                    None
                } else {
                    Some(conflicts)
                };
                self.status = format!("词库导入失败：{summary}");
            }
        }
    }

    fn export_vocabulary_xlsx(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel", &["xlsx"])
            .set_file_name("公文助手标准词库.xlsx")
            .save_file()
        else {
            return;
        };
        let existing_codes: Vec<String> = self
            .config
            .vocabulary
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| e.code.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        let result = vocabulary_xlsx::to_xlsx(&self.config.vocabulary, &path, &existing_codes);
        match result {
            Ok(()) => self.status = format!("词库已导出到 {}。", path.display()),
            Err(error) => self.status = format!("词库导出失败：{error}"),
        }
    }

    fn export_blank_vocabulary_template(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel", &["xlsx"])
            .set_file_name("公文助手标准词库模板.xlsx")
            .save_file()
        else {
            return;
        };
        let codes: Vec<String> = self
            .config
            .vocabulary
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| e.code.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        let result = vocabulary_xlsx::to_xlsx(&[], &path, &codes);
        match result {
            Ok(()) => self.status = format!("空白词库模板已导出到 {}。", path.display()),
            Err(error) => self.status = format!("模板导出失败：{error}"),
        }
    }

    fn renormalize_vocabulary(&mut self) {
        units::rebuild_parents_from_codes(&mut self.config.vocabulary);
        units::normalize(&mut self.config.vocabulary);
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.receiver.try_recv() {
            match result {
                WorkerResult::Models(Ok(models)) => {
                    self.busy = false;
                    self.models = models;
                    if self.config.lm_studio.model.trim().is_empty() && self.models.len() == 1 {
                        self.config.lm_studio.model = self.models[0].clone();
                    }
                    self.status = if self.models.is_empty() {
                        "LM Studio 已连接，但没有已加载模型。".into()
                    } else {
                        format!("LM Studio 连接成功，发现 {} 个模型。", self.models.len())
                    };
                }
                WorkerResult::Models(Err(error)) => {
                    self.busy = false;
                    self.status = format!("连接失败：{error}");
                }
                WorkerResult::Doc { key, seq, job } => self.apply_doc_job(key, seq, job),
                WorkerResult::Knowledge(job) => self.apply_knowledge_job(job),
                WorkerResult::SystemFonts(fonts) => {
                    self.system_fonts_busy = false;
                    self.system_fonts_scanned = true;
                    self.status = if fonts.is_empty() {
                        "没有在本机字体目录里找到可用的 ttf/otf 字体。".into()
                    } else {
                        format!("已找到 {} 个本机字体。", fonts.len())
                    };
                    self.system_fonts = fonts;
                }
                WorkerResult::EmbeddingModels(result) => {
                    self.embedding_probe_busy = false;
                    match result {
                        Ok(models) => {
                            self.status = if models.is_empty() {
                                "embedding 服务已连接，但没有已加载模型。".into()
                            } else {
                                format!("embedding 服务连接成功，发现 {} 个模型。", models.len())
                            };
                            // 只有一个模型且未配置时自动选上，省去手选。
                            if self.config.rag.embedding.model.trim().is_empty()
                                && models.len() == 1
                            {
                                self.config.rag.embedding.model = models[0].clone();
                            }
                            self.embedding_models = models;
                        }
                        Err(error) => self.status = format!("embedding 连接失败：{error}"),
                    }
                }
                WorkerResult::RerankModels(result) => {
                    self.rerank_probe_busy = false;
                    match result {
                        Ok(models) => {
                            self.status = if models.is_empty() {
                                "rerank 服务已连接，但没有已加载模型。".into()
                            } else {
                                format!("rerank 服务连接成功，发现 {} 个模型。", models.len())
                            };
                            if self.config.rag.rerank.model.trim().is_empty() && models.len() == 1 {
                                self.config.rag.rerank.model = models[0].clone();
                            }
                            self.rerank_models = models;
                        }
                        Err(error) => self.status = format!("rerank 连接失败：{error}"),
                    }
                }
                WorkerResult::RerankVerify(result) => {
                    self.rerank_probe_busy = false;
                    self.rerank_verify_result = Some(match result {
                        Ok(message) => (true, message),
                        Err(error) => (
                            false,
                            match self.config.rag.rerank.mode {
                                RerankMode::Llm => format!(
                                    "用对话大模型重排失败：{error}　可换一个指令跟随更好的对话模型，或把重排方式改为「不重排」。",
                                ),
                                _ => format!(
                                    "rerank 端点验证失败：{error}　请核对「端点路径」——服务对不认识的路径可能照样返回 200，看着像连上了，实际每次重排都会静默失败。LM Studio 目前不提供该接口，可改用「用对话大模型重排」。",
                                ),
                            },
                        ),
                    });
                    if let Some((_, message)) = &self.rerank_verify_result {
                        self.status = message.clone();
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    /// 知识库后台任务的收尾。
    fn apply_knowledge_job(&mut self, job: KnowledgeJob) {
        match job {
            KnowledgeJob::IndexProgress {
                done,
                total,
                current_title,
            } => {
                self.knowledge_index_progress = Some((done, total, current_title));
            }
            KnowledgeJob::IndexFinished { ok, failed } => {
                self.knowledge_busy = false;
                self.knowledge_index_progress = None;
                self.knowledge_dirty = true;
                self.knowledge_index_result = Some(if failed.is_empty() {
                    format!("索引完成：{ok} 篇全部成功。")
                } else {
                    let detail = failed
                        .iter()
                        .take(3)
                        .map(|(title, err)| format!("《{title}》：{err}"))
                        .collect::<Vec<_>>()
                        .join("；");
                    format!(
                        "索引完成：{ok} 篇成功，{} 篇失败（{detail}）。",
                        failed.len()
                    )
                });
            }
            KnowledgeJob::SearchDone(Ok(outcome)) => {
                self.knowledge_busy = false;
                self.knowledge_test_results = outcome.chunks;
                // 降级说明直接摆在检索区，不再只写进服务端日志。
                self.knowledge_search_warnings = outcome.warnings;
            }
            KnowledgeJob::SearchDone(Err(error)) => {
                self.knowledge_busy = false;
                self.knowledge_test_results = Vec::new();
                self.knowledge_search_warnings = Vec::new();
                self.status = format!("知识库检索失败：{error}");
            }
        }
    }

    /// 刷新知识库文档列表、块计数、已入库稿件集合与库内嵌入模型。
    pub(crate) fn refresh_knowledge(&mut self) {
        self.knowledge_dirty = false;
        let Some(store) = self.knowledge_store.as_mut() else {
            return;
        };
        match store.list_docs(self.knowledge_filter_kind) {
            Ok(docs) => self.knowledge_docs = docs,
            Err(error) => self.knowledge_error = Some(format!("知识库列表读取失败：{error:#}")),
        }
        self.knowledge_chunk_count = store.count_chunks().unwrap_or(0);
        self.knowledge_indexed_manuscripts = store.indexed_manuscript_ids().unwrap_or_default();
        self.knowledge_embed_models = store.distinct_embed_models().unwrap_or_default();
    }

    /// 库内是否存在与当前配置不同的 embedding 模型。换模型后维度多半不同，
    /// 旧块的余弦恒为 0，会静默退出向量召回——必须提示用户重建索引。
    pub(crate) fn knowledge_embed_model_mismatch(&self) -> Option<String> {
        let current = self.config.rag.embedding.model.trim();
        if current.is_empty() {
            return None;
        }
        let stale: Vec<&str> = self
            .knowledge_embed_models
            .iter()
            .map(String::as_str)
            .filter(|model| *model != current)
            .collect();
        if stale.is_empty() {
            return None;
        }
        Some(format!(
            "库内有文档是用「{}」嵌入的，与当前的「{current}」不一致；向量维度不同会让这些文档静默退出向量检索，请点「重建索引」。",
            stale.join("、")
        ))
    }

    /// 打开外部 markdown 文件选择框，进入导入确认（选文种）。
    pub(crate) fn knowledge_pick_markdown(&mut self) {
        let paths = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown"])
            .pick_files();
        let Some(paths) = paths else { return };
        if paths.is_empty() {
            return;
        }
        self.knowledge_import = Some(KnowledgeImportDraft {
            paths,
            kind: self.config.last_template,
        });
    }

    /// 确认导入外部 markdown：读出文件内容，归一化成导入项后建索引。
    pub(crate) fn knowledge_confirm_import(&mut self) {
        let Some(draft) = self.knowledge_import.take() else {
            return;
        };
        let mut items = Vec::new();
        // 逐个收集读取失败，别让后一条把前一条的错误覆盖掉——选十个文件失败九个
        // 时，只看得到最后一条错误是没法排查的（GBK 编码的 .md 就会走到这里）。
        let mut failures: Vec<String> = Vec::new();
        for path in &draft.paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let title = export::extract_title(&content, "");
                    items.push(knowledge::KnowledgeImportItem {
                        source: knowledge::KnowledgeSource::Markdown,
                        source_manuscript_id: None,
                        source_path: path.display().to_string(),
                        kind: draft.kind,
                        title,
                        content_markdown: content,
                    });
                }
                Err(error) => failures.push(format!("{name}（{error}）")),
            }
        }
        if !failures.is_empty() {
            // 放进 knowledge_error 而不是 status：status 紧接着会被索引进度覆盖。
            self.knowledge_error = Some(format!(
                "{} 个文件读取失败，未加入知识库：{}。若是 GBK 编码，请先另存为 UTF-8。",
                failures.len(),
                failures.join("；")
            ));
        } else {
            self.knowledge_error = None;
        }
        self.start_knowledge_index(items, false);
    }

    /// 把稿件库当前勾选的稿件加入知识库。
    pub(crate) fn knowledge_import_selected_manuscripts(&mut self) {
        let ids: Vec<i64> = self.manuscript_selected.iter().copied().collect();
        if ids.is_empty() {
            self.status = "请先在稿件管理里勾选要加入知识库的稿件。".into();
            return;
        }
        let Some(store) = self.manuscript_store.as_mut() else {
            self.status = "稿件库不可用。".into();
            return;
        };
        let mut items = Vec::new();
        for id in ids {
            match store.get(id) {
                Ok(Some(record)) => items.push(knowledge::KnowledgeImportItem {
                    source: knowledge::KnowledgeSource::Manuscript,
                    source_manuscript_id: Some(record.id),
                    source_path: String::new(),
                    kind: record.kind,
                    title: record.title,
                    content_markdown: record.content_markdown,
                }),
                Ok(None) => {}
                Err(error) => self.status = format!("读取稿件 #{id} 失败：{error:#}"),
            }
        }
        self.start_knowledge_index(items, false);
    }

    /// 重建全部索引：清空后把库内所有文档重新嵌入。这里实现为“对现有文档逐一
    /// 重新切块+嵌入”，用于更换 embedding 模型后。
    pub(crate) fn knowledge_rebuild_all(&mut self) {
        let Some(store) = self.knowledge_store.as_mut() else {
            self.status = "知识库不可用。".into();
            return;
        };
        let docs = match store.list_docs(None) {
            Ok(docs) => docs,
            Err(error) => {
                self.status = format!("读取知识库失败：{error:#}");
                return;
            }
        };
        let mut items = Vec::new();
        for doc in docs {
            if let Ok(Some((title, content))) = store.get_doc_content(doc.id) {
                let source = if doc.source == "manuscript" {
                    knowledge::KnowledgeSource::Manuscript
                } else {
                    knowledge::KnowledgeSource::Markdown
                };
                items.push(knowledge::KnowledgeImportItem {
                    source,
                    source_manuscript_id: doc.source_manuscript_id,
                    source_path: doc.source_path,
                    kind: doc.kind,
                    title,
                    content_markdown: content,
                });
            }
        }
        self.start_knowledge_index(items, true);
    }

    /// 在后台线程跑索引流水线：切块 → 分词 → 批量嵌入 → 入库。
    /// `rebuild` 为 true 时先清空（保留元数据重新嵌入）。
    fn start_knowledge_index(&mut self, items: Vec<knowledge::KnowledgeImportItem>, rebuild: bool) {
        if self.knowledge_busy {
            self.status = "知识库任务正在进行中…".into();
            return;
        }
        if items.is_empty() {
            if !rebuild {
                self.status = "没有可索引的内容。".into();
            }
            return;
        }
        if self.config.rag.embedding.model.trim().is_empty() {
            self.status = "请先在“设置”中配置知识库 embedding 模型。".into();
            return;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            self.status = "知识库路径不可用。".into();
            return;
        };
        self.knowledge_busy = true;
        self.knowledge_index_result = None;
        self.knowledge_index_progress = Some((0, items.len(), String::new()));
        let cfg = self.config.rag.clone();
        let tx = self.sender.clone();
        let _ = rebuild; // replace_document 已按来源去重，重建等价于逐篇重嵌。
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let (ok, failed) = knowledge::run_index_pipeline_with(
                db_path,
                cfg,
                items,
                move |done, total, title| {
                    let _ =
                        progress_tx.send(WorkerResult::Knowledge(KnowledgeJob::IndexProgress {
                            done,
                            total,
                            current_title: title,
                        }));
                },
            );
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::IndexFinished {
                ok,
                failed,
            }));
        });
    }

    /// 在后台线程跑一次检索测试。
    pub(crate) fn knowledge_test_search(&mut self) {
        if self.knowledge_busy {
            return;
        }
        let query = self.knowledge_test_query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            return;
        };
        self.knowledge_busy = true;
        self.knowledge_search_warnings.clear();
        let cfg = self.config.rag.clone();
        // 重排走「对话大模型」模式时要用到聊天模型的配置。
        let chat = self.config.lm_studio.clone();
        let kind = self.knowledge_filter_kind;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = rag::retrieve(&cfg, &chat, &db_path, &query, kind)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::SearchDone(result)));
        });
    }

    /// 删除一篇知识库文档。
    pub(crate) fn knowledge_delete(&mut self, id: i64) {
        if let Some(store) = self.knowledge_store.as_mut() {
            match store.delete_document(id) {
                Ok(()) => {
                    self.status = "已从知识库删除。".into();
                    self.knowledge_dirty = true;
                }
                Err(error) => self.status = format!("删除失败：{error:#}"),
            }
        }
        self.knowledge_delete_confirm = None;
    }

    /// 打开知识库文档预览弹窗：从库里读出标题、文种、原文。
    pub(crate) fn knowledge_open_preview(&mut self, doc_id: i64) {
        let Some(store) = self.knowledge_store.as_mut() else {
            self.status = "知识库不可用。".into();
            return;
        };
        match store.get_doc_for_preview(doc_id) {
            Ok(Some((title, kind, markdown))) => {
                self.knowledge_preview = Some(KnowledgePreviewState {
                    title,
                    kind,
                    markdown,
                    zoom: None,
                    fit_scale: 1.0,
                });
            }
            Ok(None) => self.status = "该文档已不在知识库中。".into(),
            Err(error) => self.status = format!("读取知识库文档失败：{error:#}"),
        }
    }

    /// 知识库文档预览浮窗：复用起草页的公文版式渲染，按窗口宽度自适应缩放。
    fn knowledge_preview_window(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.knowledge_preview.as_mut() else {
            return;
        };
        let mut open = true;
        let mut close_clicked = false;
        egui::Window::new(format!("预览 · {}", preview.title))
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 720.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                // 顶部一行：缩放控件 + 关闭按钮。
                ui.horizontal(|ui| {
                    let current = preview.zoom.unwrap_or(preview.fit_scale);
                    if theme::icon_button(ui, theme::Icon::ZoomIn, "放大").clicked() {
                        preview.zoom = Some((current + 0.1).min(2.0));
                    }
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", current * 100.0))
                            .color(theme::TEXT_MUTED),
                    );
                    if theme::icon_button(ui, theme::Icon::ZoomOut, "缩小").clicked() {
                        preview.zoom = Some((current - 0.1).max(0.4));
                    }
                    if theme::icon_button_enabled(
                        ui,
                        preview.zoom.is_some(),
                        theme::Icon::FitWidth,
                        "适应宽度",
                    )
                    .clicked()
                    {
                        preview.zoom = None;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("关闭").clicked() {
                            close_clicked = true;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::both()
                    .id_salt("knowledge_preview_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let display = UnitDisplay::new(&self.config.vocabulary);
                        // 知识库文档只存了文种与原文；版式要素用该文种的默认配置补全。
                        let mut input = DraftInput {
                            kind: preview.kind,
                            ..Default::default()
                        };
                        input.profile = self.config.profile(preview.kind);
                        let output = preview::official_preview(
                            ui,
                            &input,
                            &display,
                            &preview.markdown,
                            preview.zoom,
                            None,
                            false,
                        );
                        preview.fit_scale = output.scale;
                    });
            });
        if !open || close_clicked {
            self.knowledge_preview = None;
        }
    }

    /// 生成与优化的收尾一致：换正文、换审校结果，有提示就弹审校抽屉。
    fn take_generated(doc: &mut DraftSession, result: GeneratedDraft) {
        doc.generated_markdown = result.markdown;
        doc.warnings = result.warnings;
        doc.output_files = result.files;
        doc.export_error = None;
        if !doc.warnings.is_empty() {
            doc.result_drawer_open = true;
        }
    }

    /// 后台任务回投。稿件可能已经关闭，或者同一篇又发起了新任务——
    /// 两种情况下这份结果都已作废，直接丢掉，绝不能落到别的稿件上。
    fn apply_doc_job(&mut self, key: DocKey, seq: u64, job: DocJob) {
        let Some(index) = self.docs.iter().position(|doc| doc.key == key) else {
            return;
        };
        if self.docs[index].job_seq != seq {
            return;
        }
        if !matches!(job, DocJob::ExportProgress(_)) {
            self.docs[index].busy = false;
        }
        // 后台跑完的未必是当前显示的那篇，状态栏要点名是谁。
        let prefix = if index == self.active_doc {
            String::new()
        } else {
            format!("《{}》", self.docs[index].title())
        };
        match job {
            DocJob::Drafted(Ok(result)) => {
                let title = result.title.clone();
                Self::take_generated(&mut self.docs[index], result);
                self.status = if self.docs[index].output_files.is_empty() {
                    format!(
                        "{prefix}“{title}”草稿已生成。可直接在右侧修改，然后点“导出当前审校稿”。"
                    )
                } else {
                    format!(
                        "{prefix}“{title}”已生成并导出 {} 个文件。",
                        self.docs[index].output_files.len()
                    )
                };
            }
            DocJob::Optimized(Ok(result)) => {
                let title = result.title.clone();
                Self::take_generated(&mut self.docs[index], result);
                self.status = if self.docs[index].output_files.is_empty() {
                    format!(
                        "{prefix}“{title}”已按“{}”优化，输出格式已按内置标准校正。",
                        self.docs[index].ai_prompt_last_label
                    )
                } else {
                    format!(
                        "{prefix}“{title}”已按“{}”优化并导出 {} 个文件。",
                        self.docs[index].ai_prompt_last_label,
                        self.docs[index].output_files.len()
                    )
                };
            }
            DocJob::Drafted(Err(error)) => self.status = format!("{prefix}起草失败：{error}"),
            DocJob::Optimized(Err(error)) => self.status = format!("{prefix}优化失败：{error}"),
            DocJob::ExportProgress(message) => self.status = format!("{prefix}{message}"),
            DocJob::Exported(Ok((files, warning))) => {
                self.docs[index].output_files = files;
                self.docs[index].export_error = None;
                self.draft_page_at(index).revalidate();
                if let Some(warning) = warning {
                    self.docs[index].warnings.push(warning);
                }
                // 成品不再进抽屉：让工具栏那三枚 TEX/PDF/WORD 入口重新扫盘点亮即可。
                self.export_links.invalidate();
                self.status = format!(
                    "{prefix}当前审校稿已导出 {} 个文件。",
                    self.docs[index].output_files.len()
                );
            }
            DocJob::Exported(Err(error)) => {
                self.status = format!("{prefix}导出失败：{error}");
                let doc = &mut self.docs[index];
                doc.export_error = Some(error);
                // 失败原因全文挂在审校抽屉顶上，弹出来让人看见。
                doc.result_drawer_open = true;
            }
        }
    }

    /// 顶格一整行：左边是菜单按钮，右边依次排开所有标签。稿件和导航页共用
    /// 这一条，界面纵向只让出一行。
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            self.app_menu_button(ui);
            toolbar_separator(ui);
            self.tab_strip(ui);
        });
    }

    /// 左上角的菜单：应用图标就是入口，点开是四个常驻页面。
    fn app_menu_button(&mut self, ui: &mut egui::Ui) {
        let mut open: Option<NavPage> = None;
        egui::containers::menu::MenuButton::from_button(
            egui::Button::image(
                theme::Icon::Menu
                    .image()
                    .fit_to_exact_size(egui::vec2(22.0, 22.0)),
            )
            .image_tint_follows_text_color(true),
        )
        .ui(ui, |ui| {
            ui.set_min_width(148.0);
            for page in [
                NavPage::Manuscript,
                NavPage::Vocabulary,
                NavPage::AiPrompts,
                NavPage::Knowledge,
                NavPage::Settings,
            ] {
                let opened = self.tabs.contains(&TabRef::Page(page));
                if ui
                    .add(
                        egui::Button::image_and_text(page.icon().image(), page.label())
                            .image_tint_follows_text_color(true)
                            .selected(opened)
                            .frame(false),
                    )
                    .clicked()
                {
                    open = Some(page);
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .add(
                    egui::Button::image_and_text(theme::Icon::FilePlus.image(), "新建空白公文")
                        .image_tint_follows_text_color(true)
                        .frame(false),
                )
                .clicked()
            {
                open = None;
                self.new_blank_manuscript();
                ui.close();
            }
            ui.separator();
            if ui
                .add(
                    egui::Button::image_and_text(theme::Icon::Book.image(), "关于公文助手")
                        .image_tint_follows_text_color(true)
                        .frame(false),
                )
                .clicked()
            {
                open = None;
                self.about_window_open = true;
                ui.close();
            }
        })
        .0
        .on_hover_text("稿件管理、标准词库、AI 管理与设置");
        if let Some(page) = open {
            self.open_page(page);
        }
    }

    /// 应用图标菜单下的"关于"弹窗：版本号、构建信息、依赖致谢。
    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_window_open {
            return;
        }
        let mut close_clicked = false;
        egui::Window::new("关于公文助手")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(360.0)
            .open(&mut self.about_window_open)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.heading("公文助手");
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.weak("版本");
                    ui.label(env!("CARGO_PKG_VERSION"));
                });
                ui.horizontal(|ui| {
                    ui.weak("构建目标");
                    ui.label(format!(
                        "{} {}",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    ));
                });
                ui.horizontal(|ui| {
                    ui.weak("Rust 工具链");
                    ui.label(format!(
                        "rustc {}（运行时由 env! 决定）",
                        env!("CARGO_PKG_RUST_VERSION", "未知")
                    ));
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    "基于 egui 的离线公文写作助手：规范排版、AI 起草与优化、知识库检索、稿件管理。",
                );
                ui.add_space(4.0);
                ui.weak("本软件为内部使用工具，所有数据仅保存在本机。");
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    if ui.button("关闭").clicked() {
                        close_clicked = true;
                    }
                });
            });
        if close_clicked {
            self.about_window_open = false;
        }
    }

    /// 标签栏：宽度不够时收进右端的溢出下拉，当前这格始终可见。
    fn tab_strip(&mut self, ui: &mut egui::Ui) {
        // 右端固定占位：新建按钮，多格时还要留出溢出下拉。
        let reserved = if self.tabs.len() > 1 { 104.0 } else { 48.0 };
        let usable = (ui.available_width() - reserved).max(DOC_TAB_MIN_WIDTH);
        let desired: Vec<f32> = (0..self.tabs.len())
            .map(|tab| self.tab_width(ui, tab))
            .collect();

        // 从左往右塞，塞不下的收进溢出下拉；当前这格必须留在可见区。
        let mut shown: Vec<usize> = Vec::new();
        let mut used = 0.0;
        for (tab, width) in desired.iter().enumerate() {
            if used + width > usable && !shown.is_empty() {
                break;
            }
            used += width;
            shown.push(tab);
        }
        if !shown.contains(&self.active_tab) && self.active_tab < self.tabs.len() {
            while used + desired[self.active_tab] > usable && shown.len() > 1 {
                used -= desired[shown.pop().expect("shown 非空")];
            }
            shown.push(self.active_tab);
            shown.sort_unstable();
        }
        // 仍然超宽（比如只剩一格特别长的标题）就按比例压回去。
        let total: f32 = shown.iter().map(|&tab| desired[tab]).sum();
        let scale = if total > usable { usable / total } else { 1.0 };

        let mut select: Option<usize> = None;
        let mut close: Option<usize> = None;
        let mut active_rect: Option<egui::Rect> = None;
        ui.spacing_mut().item_spacing.x = 4.0;
        for &tab in &shown {
            let width = (desired[tab] * scale).max(DOC_TAB_MIN_WIDTH);
            let (clicked, closed, rect) = self.tab_button(ui, tab, width);
            if clicked {
                select = Some(tab);
            }
            if closed {
                close = Some(tab);
            }
            if tab == self.active_tab {
                active_rect = Some(rect);
            }
        }
        // 活动标签底部的主题橙下划线：左右两边各自插值，切换标签时平滑滑过去。
        if let Some(rect) = active_rect {
            let ctx = ui.ctx();
            let left = ctx.animate_value_with_time(
                egui::Id::new("tab_indicator_left"),
                rect.left() + 1.0,
                TAB_INDICATOR_ANIM,
            );
            let right = ctx.animate_value_with_time(
                egui::Id::new("tab_indicator_right"),
                rect.right() - 1.0,
                TAB_INDICATOR_ANIM,
            );
            if right > left {
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(left, rect.bottom() - 2.0),
                        egui::pos2(right, rect.bottom()),
                    ),
                    egui::CornerRadius::same(1),
                    theme::ACCENT,
                );
            }
        }
        let hidden: Vec<usize> = (0..self.tabs.len())
            .filter(|tab| !shown.contains(tab))
            .collect();
        if !hidden.is_empty() {
            egui::ComboBox::from_id_salt("tab_overflow")
                .selected_text(format!("»{}", hidden.len()))
                .width(46.0)
                .show_ui(ui, |ui| {
                    for tab in hidden {
                        let (mark, title) = self.tab_label(tab);
                        if ui
                            .selectable_label(false, format!("{mark}{title}"))
                            .clicked()
                        {
                            select = Some(tab);
                        }
                    }
                })
                .response
                .on_hover_text("切换到其余已打开的标签");
        }
        if theme::icon_button(ui, theme::Icon::FilePlus, "新建空白公文（Ctrl+N）").clicked()
        {
            self.new_blank_manuscript();
        }

        if let Some(tab) = select {
            self.activate_tab(tab);
        }
        if let Some(tab) = close {
            self.request_close_tab(tab);
        }
    }

    /// 标签上显示的（状态标记, 标题）。
    fn tab_label(&self, tab: usize) -> (&'static str, String) {
        match self.tabs.get(tab) {
            Some(TabRef::Doc(key)) => match self.doc_index_of_key(*key) {
                Some(index) => (self.docs[index].dirty_mark(), self.docs[index].title()),
                None => ("", "已关闭".to_string()),
            },
            Some(TabRef::Page(page)) => ("", page.label().to_string()),
            None => ("", String::new()),
        }
    }

    /// 一格标签想要多宽：标题实际排版宽度加上标记与关闭按钮的固定占位。
    fn tab_width(&self, ui: &egui::Ui, tab: usize) -> f32 {
        let (_, title) = self.tab_label(tab);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text = ui
            .painter()
            .layout_no_wrap(
                truncate_middle(&title, DOC_TAB_TITLE_CHARS),
                font,
                theme::TEXT,
            )
            .size()
            .x;
        (text + DOC_TAB_CHROME_WIDTH).clamp(DOC_TAB_MIN_WIDTH, DOC_TAB_MAX_WIDTH)
    }

    /// 画一格标签，返回（是否点了标签体, 是否点了关闭, 标签占位矩形）。
    ///
    /// 动效统一在这里：背景/边框/文字色由 `animate_bool_with_time` 插值
    /// （悬停 120ms、按下 150ms、选中 160ms），关闭按钮悬停标签才渐显、
    /// 悬停它本身时渐变到危险红。下划线指示条由调用方 `tab_strip` 负责，
    /// 因为要跨标签共享动画状态。交互状态来自先占位拿到的 response，
    /// 颜色才能在画背景之前算好。
    fn tab_button(&mut self, ui: &mut egui::Ui, tab: usize, width: f32) -> (bool, bool, egui::Rect) {
        let selected = tab == self.active_tab;
        let (mark, title) = self.tab_label(tab);
        let (icon, hover, busy) = match self.tabs[tab] {
            TabRef::Doc(key) => match self.doc_index_of_key(key) {
                Some(index) => {
                    let doc = &self.docs[index];
                    // 只读稿件用生命周期图标代替脏标记，一眼看出这篇动不了。
                    let icon = match doc.record_status {
                        _ if !doc.read_only() => None,
                        ManuscriptStatus::Archived => Some(theme::Icon::Archive),
                        _ => Some(theme::Icon::Publish),
                    };
                    (icon, doc.tab_hover(), doc.busy)
                }
                None => (None, title.clone(), false),
            },
            TabRef::Page(page) => (Some(page.icon()), page.label().to_string(), false),
        };

        // 先占位拿到交互状态，才能在同一帧内驱动颜色动画。
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(width, TOOLBAR_CONTROL_HEIGHT),
            egui::Sense::click(),
        );

        // Context 是 Arc，克隆断开与 ui 的借用，new_child 才能拿到可变借用。
        let ctx = ui.ctx().clone();
        let hover_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_hover").with(tab),
            response.hovered(),
            TAB_HOVER_ANIM,
        );
        let press_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_press").with(tab),
            response.is_pointer_button_down_on(),
            TAB_PRESS_ANIM,
        );
        let sel_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_select").with(tab),
            selected,
            TAB_SELECT_ANIM,
        );

        // 背景：选中淡入白底；未选中从沉色经悬停色到按下的更深色。
        let mut bg = theme::SURFACE_SUNK.lerp_to_gamma(theme::SURFACE, sel_t);
        bg = bg.lerp_to_gamma(theme::SURFACE_HOVER, hover_t * (1.0 - sel_t));
        bg = bg.lerp_to_gamma(theme::SURFACE_ACTIVE, press_t * (1.0 - sel_t));
        // 边框：悬停加深，选中渐变到主题橙。
        let border = theme::BORDER
            .lerp_to_gamma(theme::BORDER_STRONG, hover_t * (1.0 - sel_t))
            .lerp_to_gamma(theme::ACCENT, sel_t);
        let text_color = theme::TEXT_SOFT.lerp_to_gamma(theme::TEXT, sel_t.max(hover_t));
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(6),
            bg,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );

        let mut clicked = false;
        let mut closed = false;

        // 内容区：图标/脏标记在左，关闭按钮钉死在右端，标题吃掉中间。
        let inner = rect.shrink2(egui::vec2(8.0, 4.0));
        let mut content = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        content.spacing_mut().item_spacing.x = 4.0;
        if busy {
            content.spinner();
        } else if !mark.is_empty() {
            content.colored_label(theme::ACCENT, mark);
        } else if let Some(icon) = icon {
            // Lucide 图标用 currentColor 描边，这里跟随文字色渐变。
            content.add(
                icon.image()
                    .tint(text_color)
                    .fit_to_exact_size(egui::vec2(14.0, 14.0)),
            );
        }
        let close_width = 18.0;
        let label_width = (content.available_width() - close_width).max(24.0);
        let label_response = content.add_sized(
            [label_width, TOOLBAR_CONTROL_HEIGHT - 8.0],
            egui::Label::new(
                egui::RichText::new(truncate_middle(&title, DOC_TAB_TITLE_CHARS))
                    .color(text_color),
            )
            .truncate()
            .sense(egui::Sense::click()),
        );
        // 点击标签空白处也选中；中键关闭是浏览器/编辑器的习惯，任意位置都认。
        if label_response.clicked() || response.clicked() {
            clicked = true;
        }
        if label_response.middle_clicked() || response.middle_clicked() {
            closed = true;
        }
        label_response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(hover);
        // 关闭按钮：悬停标签才渐显，悬停它本身时渐变到危险红。它钉死在
        // 内容右端、位置可精确预估，hover 检测才能在画按钮之前拿到。
        let close_vis_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_close_vis").with(tab),
            response.hovered(),
            TAB_CLOSE_ANIM,
        );
        let close_rect = egui::Rect::from_min_size(
            egui::pos2(inner.right() - close_width, inner.center().y - 8.0),
            egui::vec2(close_width, 16.0),
        );
        let close_hover_t = ctx.animate_bool_with_time(
            egui::Id::new("tab_close_hover").with(tab),
            content.rect_contains_pointer(close_rect),
            TAB_HOVER_ANIM,
        );
        let mut close_color = theme::TEXT_MUTED.lerp_to_gamma(theme::DANGER, close_hover_t);
        close_color = close_color.gamma_multiply(close_vis_t);
        if content
            .add(
                egui::Button::new(egui::RichText::new("×").color(close_color))
                    .frame(false)
                    .min_size(egui::vec2(close_width, 16.0)),
            )
            .on_hover_text("关闭这个标签")
            .clicked()
        {
            closed = true;
        }

        (clicked, closed, rect)
    }

    /// 底部状态栏。整条只有一行：左边状态文案，中间模型名，右边仿 Zed 的抽屉入口。
    ///
    /// 这里的布局有一处必须守住的约束：横向布局的纵向对齐是 `Align::Center`，
    /// egui 会让行内控件填满**当前可用高度**（见 `Layout::next_frame_ignore_wrap`）。
    /// 底部面板的可用高度又来自上一帧量到的内容高度，所以状态栏里一旦出现第二行，
    /// 第一行就会吃掉整条面板，第二行再往上叠一截，面板每帧长高一次，状态栏便会
    /// 一路往上爬。因此这里只允许一个 `horizontal`，并把行高钉死。
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        /// 状态栏行高，参照 Windows 状态栏取紧凑值。
        const ROW_HEIGHT: f32 = 22.0;
        /// 模型标签高度，必须小于等于行高，否则又会把面板顶高。
        const CHIP_HEIGHT: f32 = 20.0;

        ui.scope(|ui| {
            // 字号按 Windows 状态栏的习惯收小一档。
            for style in [
                egui::TextStyle::Body,
                egui::TextStyle::Button,
                egui::TextStyle::Small,
            ] {
                ui.style_mut().text_styles.insert(
                    style,
                    egui::FontId::new(12.0, egui::FontFamily::Proportional),
                );
            }
            ui.style_mut().spacing.button_padding = egui::vec2(4.0, 2.0);
            ui.style_mut().spacing.item_spacing = egui::vec2(4.0, 2.0);
            ui.style_mut().spacing.interact_size.y = ROW_HEIGHT;

            let status = self.status.clone();
            let model = self.config.lm_studio.model.trim().to_owned();
            let show_doc_controls = self.showing_doc();
            let active_doc = self.active_doc;
            let (versions_open, result_open, warnings_count, saved) = if show_doc_controls {
                self.docs
                    .get(active_doc)
                    .map(|doc| {
                        (
                            doc.versions_open,
                            doc.result_drawer_open,
                            doc.warnings.len(),
                            doc.manuscript_id.is_some(),
                        )
                    })
                    .unwrap_or((false, false, 0, false))
            } else {
                (false, false, 0, false)
            };

            let status_limit = (ui.available_width() * 0.38).clamp(160.0, 420.0);
            ui.horizontal(|ui| {
                // 行高写死：`set_height` 同时钉住上下限，行内控件才不会去填满面板高度。
                ui.set_height(ROW_HEIGHT);

                // 左：忙碌指示灯与状态文案。文案过长直接截断，不许把中间的模型名挤走。
                if self.any_busy() {
                    ui.spinner();
                } else {
                    theme::dot(ui, theme::SUCCESS);
                }
                ui.add_space(2.0);
                ui.add_sized(
                    [status_limit, ROW_HEIGHT],
                    egui::Label::new(egui::RichText::new(status).color(theme::TEXT_SOFT))
                        .truncate(),
                );
                let left_bound = ui.min_rect().right();

                // 右：导出、审校、版本三个抽屉入口。只留图标，说明放在悬停里。
                let right_bound = ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !show_doc_controls {
                            return ui.max_rect().right();
                        }
                        let review_tip = if warnings_count > 0 {
                            format!("审校提示 {warnings_count} 条")
                        } else {
                            "审校通过".to_string()
                        };
                        let review_tint = if warnings_count > 0 {
                            theme::WARN
                        } else {
                            theme::SUCCESS
                        };
                        if status_icon_button(
                            ui,
                            result_open,
                            theme::Icon::SquareCheck,
                            &review_tip,
                            Some(review_tint),
                        )
                        .clicked()
                        {
                            let doc = &mut self.docs[active_doc];
                            doc.result_drawer_open = !doc.result_drawer_open;
                        }
                        let version_tip = if saved {
                            "版本历史".to_string()
                        } else {
                            "版本历史（先保存）".to_string()
                        };
                        if status_icon_button(
                            ui,
                            versions_open,
                            theme::Icon::History,
                            &version_tip,
                            None,
                        )
                        .clicked()
                        {
                            self.docs[active_doc].versions_open =
                                !self.docs[active_doc].versions_open;
                        }
                        ui.min_rect().left()
                    })
                    .inner;

                // 中：模型名。画在左右两组之间的空档里，仍属于这一行，不额外占纵向空间。
                let gap_left = left_bound + 10.0;
                let gap_right = right_bound - 10.0;
                if gap_right - gap_left > 80.0 {
                    let model_text = if model.is_empty() {
                        "模型：未选择".to_string()
                    } else {
                        format!("模型：{}", truncate_middle(&model, 36))
                    };
                    let (fg, bg) = if model.is_empty() {
                        (theme::WARN, theme::WARN_SOFT)
                    } else {
                        (theme::SUCCESS, theme::SURFACE_SUNK)
                    };
                    // 对齐整条状态栏的中线；只有窗口太窄、居中会压到两侧时才让位。
                    let chip_width = (gap_right - gap_left).min(420.0);
                    let center_x = ui
                        .max_rect()
                        .center()
                        .x
                        .clamp(gap_left + chip_width * 0.5, gap_right - chip_width * 0.5);
                    let chip_rect = egui::Rect::from_center_size(
                        egui::pos2(center_x, ui.max_rect().center().y),
                        egui::vec2(chip_width, CHIP_HEIGHT),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(chip_rect)
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                        |ui| theme::chip(ui, &model_text, fg, bg),
                    );
                }
            });
        });
    }

    fn vocabulary_ui(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let mut structure_changed = false;
        let unit_count = self
            .config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .count();
        let person_count = self.config.vocabulary.len() - unit_count;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("标准词库");
                ui.weak(format!(
                    "{unit_count} 个单位 · {person_count} 名人员 · 数据仅保存在本机"
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存更改").clicked() {
                    self.persist();
                }
                if theme::icon_button(ui, theme::Icon::History, "版本历史")
                    .on_hover_text("查看全局配置版本（词库、版式、设置），可应用回滚或对照")
                    .clicked()
                {
                    self.config_versions_open = true;
                }
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::GitCommit,
                        "提交配置版本",
                    ))
                    .on_hover_text(
                        "把当前词库、版式与设置固化为一个配置版本（需相对上一版本有变更）",
                    )
                    .clicked()
                {
                    self.open_version_commit(VersionScope::Config);
                }
                ui.menu_image_text_button(
                    theme::Icon::ArrowUpDown.image().tint(theme::TEXT_SOFT),
                    "导入 / 导出",
                    |ui| {
                        if ui
                            .add(theme::icon_text_button(theme::Icon::FileUp, "导入 Excel"))
                            .on_hover_text("选择本机 .xlsx 词库，按编码优先合并")
                            .clicked()
                        {
                            ui.close();
                            self.import_vocabulary_xlsx();
                        }
                        if ui
                            .add(theme::icon_text_button(theme::Icon::FileDown, "导出 Excel"))
                            .on_hover_text("把当前词库导出为 Excel 模板，含下拉与冻结")
                            .clicked()
                        {
                            ui.close();
                            self.export_vocabulary_xlsx();
                        }
                        ui.separator();
                        if ui
                            .add(theme::icon_text_button(
                                theme::Icon::FileDown,
                                "下载空白模板",
                            ))
                            .on_hover_text("导出空白模板（仅表头 + 数据验证）")
                            .clicked()
                        {
                            ui.close();
                            self.export_blank_vocabulary_template();
                        }
                    },
                );
            });
        });
        ui.add_space(8.0);

        ui.horizontal_wrapped(|ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::Building, "顶级单位"))
                .on_hover_text("新增一个没有上级的单位；选中单位后可在右侧继续加下级和人员")
                .clicked()
            {
                action = Some(VocabAction::AddUnit {
                    parent: String::new(),
                });
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.vocabulary_filter)
                    .hint_text("搜索单位、简称、机关代字、姓名或电话")
                    .desired_width(280.0),
            );
            if !self.vocabulary_filter.is_empty()
                && theme::icon_button(ui, theme::Icon::SearchClear, "清除搜索").clicked()
            {
                self.vocabulary_filter.clear();
            }
            ui.separator();
            if theme::icon_button(ui, theme::Icon::Expand, "展开全部").clicked() {
                self.vocabulary_collapsed.clear();
            }
            if theme::icon_button(ui, theme::Icon::Collapse, "折叠全部").clicked() {
                self.vocabulary_collapsed = self
                    .config
                    .vocabulary
                    .iter()
                    .filter(|entry| entry.category == VocabularyCategory::Unit)
                    .map(|entry| entry.id)
                    .collect();
            }
            ui.separator();
            if ui
                .add(theme::warning_icon_button(theme::Icon::Trash, "清空词库"))
                .clicked()
            {
                self.vocabulary_clear_confirm = true;
            }
        });

        if self.vocabulary_clear_confirm {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.colored_label(
                    WARN,
                    "将清空当前标准词库中的全部单位和人员。此操作尚未保存到磁盘。",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("确认清空").color(WARN),
                        ))
                        .clicked()
                    {
                        action = Some(VocabAction::Clear);
                    }
                    if ui.button("取消").clicked() {
                        self.vocabulary_clear_confirm = false;
                    }
                });
            });
        }

        let conflicts_snapshot = self.vocabulary_import_conflicts.clone();
        if let Some(conflicts) = &conflicts_snapshot
            && !conflicts.is_empty()
        {
            ui.add_space(6.0);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        WARN,
                        format!("导入失败（{} 项冲突）：修正后重新上传", conflicts.len()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, theme::Icon::X, "关闭提示").clicked() {
                            self.vocabulary_import_conflicts = None;
                        }
                    });
                });
                let mut by_sheet: BTreeMap<&'static str, Vec<&vocabulary_xlsx::Conflict>> =
                    BTreeMap::new();
                for c in conflicts {
                    by_sheet.entry(c.sheet).or_default().push(c);
                }
                for (sheet, list) in by_sheet {
                    ui.strong(sheet);
                    for c in list {
                        ui.label(format!(
                            "  • 第 {} 行 「{}」=「{}」：{}",
                            c.row, c.field, c.current_value, c.message
                        ));
                    }
                }
            });
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        let filter = self.vocabulary_filter.trim().to_lowercase();
        let rows = self.vocabulary_rows(&filter);
        // 窄窗口放不下左右两栏，改成树在上、编辑区在下。
        if ui.available_width() < 880.0 {
            egui::ScrollArea::vertical()
                .id_salt("vocabulary_single_column")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.vocabulary_tree_ui(ui, &rows, &filter, &mut action);
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    structure_changed |= self.vocabulary_editor_ui(ui, &mut action);
                });
        } else {
            let tree_width = (ui.available_width() * 0.52).clamp(340.0, 620.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(tree_width);
                    egui::ScrollArea::vertical()
                        .id_salt("vocabulary_tree")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.vocabulary_tree_ui(ui, &rows, &filter, &mut action);
                        });
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("vocabulary_editor")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            structure_changed |= self.vocabulary_editor_ui(ui, &mut action);
                        });
                });
            });
        }

        if let Some(action) = action {
            self.apply_vocab_action(action);
        } else if structure_changed {
            units::normalize(&mut self.config.vocabulary);
        }
    }

    /// 把词库摊平成树形界面要画的行：单位按层级缩进，人员挂在所属单位下面。
    /// 词库本身已由 `units::normalize` 排成深度优先顺序，这里只补层级和折叠。
    fn vocabulary_rows(&self, filter: &str) -> Vec<TreeRow> {
        let vocab = &self.config.vocabulary;
        let depths = vocabulary_depths(vocab);
        let mut rows: Vec<TreeRow> = Vec::with_capacity(vocab.len());
        for (index, entry) in vocab.iter().enumerate() {
            rows.push(TreeRow {
                index,
                id: entry.id,
                depth: depths[index],
                is_unit: entry.category == VocabularyCategory::Unit,
                has_children: false,
            });
        }
        // 深度优先顺序下，下一行更深就说明本行有子节点。
        for position in 0..rows.len() {
            rows[position].has_children = rows
                .get(position + 1)
                .is_some_and(|next| next.depth > rows[position].depth);
        }

        if !filter.is_empty() {
            let mut visible = rows
                .iter()
                .map(|row| vocabulary_matches(&vocab[row.index], filter))
                .collect::<Vec<_>>();
            // 命中的节点要连同它的各级上级一起显示，否则看不出它挂在哪儿。
            for position in (0..rows.len()).rev() {
                if !visible[position] {
                    continue;
                }
                let mut depth = rows[position].depth;
                let mut ancestor = position;
                while depth > 0 && ancestor > 0 {
                    ancestor -= 1;
                    if rows[ancestor].depth < depth {
                        visible[ancestor] = true;
                        depth = rows[ancestor].depth;
                    }
                }
            }
            // 搜索期间忽略折叠状态，免得命中项藏在折叠的分支里。
            return rows
                .into_iter()
                .zip(visible)
                .filter(|(_, visible)| *visible)
                .map(|(row, _)| row)
                .collect();
        }

        let mut result = Vec::with_capacity(rows.len());
        let mut skip_below: Option<usize> = None;
        for row in rows {
            if let Some(depth) = skip_below {
                if row.depth > depth {
                    continue;
                }
                skip_below = None;
            }
            let collapse_here =
                row.is_unit && row.has_children && self.vocabulary_collapsed.contains(&row.id);
            let depth = row.depth;
            result.push(row);
            if collapse_here {
                skip_below = Some(depth);
            }
        }
        result
    }

    fn vocabulary_tree_ui(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[TreeRow],
        filter: &str,
        action: &mut Option<VocabAction>,
    ) {
        ui.horizontal(|ui| {
            ui.strong("单位层级");
            ui.weak(if filter.is_empty() {
                "点击节点在右侧编辑；行尾按钮调整同级顺序".to_string()
            } else {
                format!("匹配 {} 行", rows.len())
            });
        });
        ui.add_space(4.0);

        if rows.is_empty() {
            ui.group(|ui| {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.strong(if filter.is_empty() {
                        "词库还是空的"
                    } else {
                        "没有找到匹配的单位或人员"
                    });
                    ui.weak(if filter.is_empty() {
                        "点击上方“顶级单位”开始建库，或从 Markdown 批量导入。"
                    } else {
                        "试试缩短关键词，或清除搜索条件。"
                    });
                });
                ui.add_space(12.0);
            });
            return;
        }

        for row in rows {
            let entry = &self.config.vocabulary[row.index];
            let id = row.id;
            let name = if entry.canonical.trim().is_empty() {
                "（未命名）".to_string()
            } else {
                entry.canonical.trim().to_string()
            };
            let code = entry.code.trim().to_string();
            let selected = self.vocabulary_selected == Some(id);
            let collapsed = self.vocabulary_collapsed.contains(&id);
            // 单位层级只显示机关代字；人员显示职务、电话及承办上级单位权限。
            let detail = if row.is_unit {
                let mut parts = Vec::new();
                if !entry.department_code.trim().is_empty() {
                    parts.push(format!("代字 {}", entry.department_code.trim()));
                }
                if entry.seal_on_behalf {
                    parts.push("代章".to_string());
                }
                parts.join(" · ")
            } else {
                let mut parts = Vec::new();
                if !entry.position.trim().is_empty() {
                    parts.push(entry.position.trim().to_string());
                }
                if !entry.phone.trim().is_empty() {
                    parts.push(entry.phone.trim().to_string());
                }
                if entry.can_handle_parent_unit {
                    parts.push("可承办上级单位".to_string());
                }
                if parts.is_empty() {
                    "未维护职务和电话".to_string()
                } else {
                    parts.join(" · ")
                }
            };
            let orphan = !row.is_unit && entry.unit.trim().is_empty();

            ui.horizontal(|ui| {
                ui.add_space(row.depth as f32 * 16.0);
                if row.is_unit && row.has_children {
                    if theme::icon_button(
                        ui,
                        if collapsed {
                            theme::Icon::ChevronRight
                        } else {
                            theme::Icon::ChevronDown
                        },
                        if collapsed { "展开" } else { "折叠" },
                    )
                    .clicked()
                    {
                        if collapsed {
                            self.vocabulary_collapsed.remove(&id);
                        } else {
                            self.vocabulary_collapsed.insert(id);
                        }
                    }
                } else {
                    ui.add_space(28.0);
                }

                ui.monospace(if code.is_empty() { "--" } else { code.as_str() });
                let label = if orphan {
                    egui::RichText::new(&name).color(WARN)
                } else {
                    egui::RichText::new(&name)
                };
                let response = ui.selectable_label(selected, label);
                let response = if orphan {
                    response.on_hover_text("该人员没有所属单位，请在右侧指定")
                } else {
                    response
                };
                if response.clicked() {
                    self.vocabulary_selected = Some(id);
                    self.vocabulary_delete_confirm = None;
                }
                if !detail.is_empty() {
                    ui.weak(detail);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, theme::Icon::ArrowDown, "下移")
                        .on_hover_text("与后一个同级交换")
                        .clicked()
                    {
                        *action = Some(VocabAction::MoveDown(id));
                    }
                    if theme::icon_button(ui, theme::Icon::ArrowUp, "上移")
                        .on_hover_text("与前一个同级交换")
                        .clicked()
                    {
                        *action = Some(VocabAction::MoveUp(id));
                    }
                });
            });
        }
    }

    /// 右侧编辑区。返回 `true` 表示改动了层级（改名或换上级），需要重排词库。
    fn vocabulary_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<VocabAction>,
    ) -> bool {
        let Some(id) = self.vocabulary_selected else {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.strong("未选中词条");
                ui.weak("在左侧点选一个单位或人员即可编辑。");
            });
            return false;
        };
        let Some(index) = self
            .config
            .vocabulary
            .iter()
            .position(|entry| entry.id == id)
        else {
            self.vocabulary_selected = None;
            return false;
        };

        let mut structure_changed = false;
        let width = (ui.available_width() - 96.0).clamp(180.0, 420.0);
        let is_unit = self.config.vocabulary[index].category == VocabularyCategory::Unit;
        let display = UnitDisplay::new(&self.config.vocabulary);
        let heading = if is_unit {
            display.full_name(&self.config.vocabulary[index].code)
        } else {
            let unit = self.config.vocabulary[index].unit.trim().to_string();
            if unit.is_empty() {
                self.config.vocabulary[index].canonical.trim().to_string()
            } else {
                format!(
                    "{} · {}",
                    self.config.vocabulary[index].canonical.trim(),
                    display.full_name(&unit)
                )
            }
        };

        ui.horizontal(|ui| {
            ui.strong(if is_unit { "单位" } else { "人员" });
            if is_unit {
                ui.label("层级编码");
                let prev_code = self.config.vocabulary[index].code.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].code)
                        .hint_text("留空由系统按位置自动补；前缀即上级编码")
                        .desired_width(160.0),
                );
                if resp.changed() {
                    self.config.vocabulary[index].code =
                        self.config.vocabulary[index].code.trim().to_string();
                    structure_changed = true;
                }
                if resp.lost_focus() && self.config.vocabulary[index].code != prev_code {
                    self.renormalize_vocabulary();
                }
                if ui
                    .button("整理")
                    .on_hover_text("按编码重新建上下级并排序")
                    .clicked()
                {
                    self.renormalize_vocabulary();
                    structure_changed = true;
                }
                // 编码重复红字提示。
                let code = self.config.vocabulary[index].code.trim();
                if !code.is_empty() {
                    let dup = self.config.vocabulary.iter().enumerate().any(|(i, e)| {
                        i != index
                            && e.category == VocabularyCategory::Unit
                            && e.code.trim() == code
                    });
                    if dup {
                        ui.colored_label(WARN, "编码重复");
                    }
                }
            } else {
                ui.weak(format!("单位内编码 {}", self.config.vocabulary[index].code));
            }
        });
        ui.add(egui::Label::new(egui::RichText::new(&heading).heading()).wrap());
        if is_unit {
            ui.weak("以上是本单位在公文中展开后的全称。");
        }
        ui.add_space(8.0);

        if is_unit {
            structure_changed |= self.vocabulary_unit_editor(ui, index, width);
        } else {
            structure_changed |= self.vocabulary_person_editor(ui, index, width);
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        let canonical = self.config.vocabulary[index].canonical.trim().to_string();
        let unit_code = self.config.vocabulary[index].code.trim().to_string();
        if is_unit {
            let children = units::child_units(&self.config.vocabulary, &unit_code).len();
            let people = units::unit_people(&self.config.vocabulary, &unit_code).len();
            ui.weak(format!("下属 {children} 个单位 · {people} 名人员"));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(theme::icon_text_button(theme::Icon::FolderPlus, "下级单位"))
                    .on_hover_text("新单位的上级自动设为本单位")
                    .clicked()
                {
                    *action = Some(VocabAction::AddUnit {
                        parent: unit_code.clone(),
                    });
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::UserPlus, "人员"))
                    .on_hover_text("新人员自动归属本单位")
                    .clicked()
                {
                    *action = Some(VocabAction::AddPerson {
                        unit: unit_code.clone(),
                    });
                }
            });
            ui.add_space(6.0);
        }

        let doomed = self.vocabulary_delete_confirm == Some(id);
        if doomed {
            let (units_count, people_count) = if is_unit {
                let indices = units::subtree_indices(&self.config.vocabulary, index);
                let units_count = indices
                    .iter()
                    .filter(|i| self.config.vocabulary[**i].category == VocabularyCategory::Unit)
                    .count();
                (units_count, indices.len() - units_count)
            } else {
                (0, 1)
            };
            ui.group(|ui| {
                ui.colored_label(
                    WARN,
                    if is_unit {
                        format!(
                            "将删除本单位及其下级：共 {units_count} 个单位、{people_count} 名人员"
                        )
                    } else {
                        format!("将删除人员“{canonical}”")
                    },
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("确认删除").color(WARN),
                        ))
                        .clicked()
                    {
                        *action = Some(VocabAction::Delete(id));
                    }
                    if ui.button("取消").clicked() {
                        self.vocabulary_delete_confirm = None;
                    }
                });
            });
        } else if ui
            .add(theme::warning_icon_button(
                theme::Icon::Trash,
                if is_unit {
                    "删除单位"
                } else {
                    "删除人员"
                },
            ))
            .clicked()
        {
            self.vocabulary_delete_confirm = Some(id);
        }
        ui.add_space(4.0);
        ui.weak("改动会立即反映在起草页；点击右上角“保存更改”写入本机配置。");
        structure_changed
    }

    fn vocabulary_unit_editor(&mut self, ui: &mut egui::Ui, index: usize, width: f32) -> bool {
        let mut structure_changed = false;
        // 上级不能选自己或自己的下级，否则会形成环。
        let blocked = units::subtree_indices(&self.config.vocabulary, index)
            .into_iter()
            .map(|i| self.config.vocabulary[i].code.trim().to_string())
            .collect::<Vec<_>>();
        let display = UnitDisplay::new(&self.config.vocabulary);
        let parent_options = self
            .config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .map(|entry| {
                (
                    entry.code.trim().to_string(),
                    format!("{} · {}", entry.code.trim(), display.full_name(&entry.code)),
                )
            })
            .filter(|(code, _)| !code.is_empty() && !blocked.contains(code))
            .collect::<Vec<_>>();
        let current_parent_label = self.config.vocabulary[index]
            .parent
            .trim()
            .is_empty()
            .then(|| "（顶层单位）".to_string())
            .or_else(|| {
                parent_options
                    .iter()
                    .find(|(code, _)| code == self.config.vocabulary[index].parent.trim())
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| self.config.vocabulary[index].parent.clone());

        egui::Grid::new(("unit_editor", index))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("单位名称");
                let renamed = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.config.vocabulary[index].canonical)
                            .hint_text("本级名称，不含上级；如“新闻舆论处”")
                            .desired_width(width),
                    )
                    .changed();
                ui.end_row();
                structure_changed |= renamed;

                ui.label("上级单位");
                let parent = &mut self.config.vocabulary[index].parent;
                let previous_parent = parent.clone();
                egui::ComboBox::from_id_salt(("unit_parent", index))
                    .selected_text(&current_parent_label)
                    .width(width)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(parent.trim().is_empty(), "（顶层单位）")
                            .clicked()
                        {
                            parent.clear();
                        }
                        for (code, label) in &parent_options {
                            if ui
                                .selectable_label(parent.as_str() == code, label)
                                .clicked()
                            {
                                *parent = code.clone();
                            }
                        }
                    });
                structure_changed |= *parent != previous_parent;
                ui.end_row();

                ui.label("简称");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].abbr)
                        .hint_text("如“新舆处”；留空时回落单位名称")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("外部名称");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].external_name)
                        .hint_text("对外函件使用；留空时回退单位名称并在审核中提醒")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("机关代字");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].department_code)
                        .hint_text("如“某教函”；选中本单位发文时自动带出")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("是否代章");
                ui.checkbox(
                    &mut self.config.vocabulary[index].seal_on_behalf,
                    "该单位落款时自动标注“（代章）”",
                )
                .on_hover_text(
                    "仅公函选择该单位作为落款单位时自动标注；联合发文按主发文单位判断。电话通知等其他文种不盖章，不适用。",
                );
                ui.end_row();

                ui.label("别名 / 常见错写");
                let mut aliases = self.config.vocabulary[index].aliases.join("、");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut aliases)
                            .hint_text("多个名称用顿号分隔，用于成稿后查错")
                            .desired_width(width),
                    )
                    .changed()
                {
                    self.config.vocabulary[index].aliases = split_units(&aliases);
                }
                ui.end_row();

                ui.label("备注");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].note)
                        .hint_text("可填写适用范围或使用说明")
                        .desired_width(width),
                );
                ui.end_row();
            });

        ui.add_space(4.0);
        let name = self.config.vocabulary[index].canonical.trim().to_string();
        if name.is_empty() {
            ui.colored_label(WARN, "单位名称为空：下级单位和人员无法挂靠。");
        }
        let abbr = UnitDisplay::new(&self.config.vocabulary)
            .abbr_spaced(&self.config.vocabulary[index].code);
        ui.weak(format!("版记承办单位用简称；电话通知落款显示为“{abbr}”。"));
        structure_changed
    }

    fn vocabulary_person_editor(&mut self, ui: &mut egui::Ui, index: usize, width: f32) -> bool {
        let mut structure_changed = false;
        let display = UnitDisplay::new(&self.config.vocabulary);
        let unit_options = self
            .config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .map(|entry| {
                (
                    entry.code.trim().to_string(),
                    format!("{} · {}", entry.code.trim(), display.full_name(&entry.code)),
                )
            })
            .filter(|(code, _)| !code.is_empty())
            .collect::<Vec<_>>();
        let current_unit_label = self.config.vocabulary[index]
            .unit
            .trim()
            .is_empty()
            .then(|| "（未归属）".to_string())
            .or_else(|| {
                unit_options
                    .iter()
                    .find(|(code, _)| code == self.config.vocabulary[index].unit.trim())
                    .map(|(_, label)| label.clone())
            })
            .unwrap_or_else(|| self.config.vocabulary[index].unit.clone());

        egui::Grid::new(("person_editor", index))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("姓名");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].canonical)
                        .hint_text("只填姓名，如“王庭”")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("职务");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].position)
                        .hint_text("如“主任”“副主任”")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("联系电话");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].phone)
                        .hint_text("与姓名一一绑定，起草页自动带出")
                        .desired_width(width),
                );
                ui.end_row();

                ui.label("所属单位");
                let unit = &mut self.config.vocabulary[index].unit;
                let previous_unit = unit.clone();
                egui::ComboBox::from_id_salt(("person_unit", index))
                    .selected_text(&current_unit_label)
                    .width(width)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(unit.trim().is_empty(), "（未归属）")
                            .clicked()
                        {
                            unit.clear();
                        }
                        for (code, label) in &unit_options {
                            if ui.selectable_label(unit.as_str() == code, label).clicked() {
                                *unit = code.clone();
                            }
                        }
                    });
                structure_changed |= *unit != previous_unit;
                ui.end_row();

                ui.label("承办上级单位");
                ui.checkbox(
                    &mut self.config.vocabulary[index].can_handle_parent_unit,
                    "可在上级单位的公函版记中作为联系人",
                );
                ui.end_row();

                ui.label("别名 / 常见错写");
                let mut aliases = self.config.vocabulary[index].aliases.join("、");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut aliases)
                            .hint_text("多个写法用顿号分隔")
                            .desired_width(width),
                    )
                    .changed()
                {
                    self.config.vocabulary[index].aliases = split_units(&aliases);
                }
                ui.end_row();

                ui.label("备注");
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.vocabulary[index].note)
                        .hint_text("可填写适用场合或使用说明")
                        .desired_width(width),
                );
                ui.end_row();
            });

        ui.add_space(4.0);
        wrapped_hint(
            ui,
            "公函联系人按承办单位过滤；勾选“承办上级单位”后，也可在所属单位的上级单位版记中作为联系人。白头件呈报领导仍只列出落款单位的直属人员。",
            width + 96.0,
        );
        structure_changed
    }

    // ---------- AI 管理（优化提示词库） ----------

    /// 提示词管理页：左侧列表，右侧编辑区，底部常驻内置输出标准的只读预览。
    fn ai_prompts_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("AI 管理");
                let builtin = self
                    .config
                    .ai_prompts
                    .iter()
                    .filter(|entry| entry.is_builtin())
                    .count();
                ui.weak(format!(
                    "{} 条优化提示词（内置 {builtin} 条）· 输出格式标准内置生效，不可关闭 · 仅保存在本机",
                    self.config.ai_prompts.len()
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存更改").clicked() {
                    self.persist();
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::FilePlus, "新建提示词"))
                    .on_hover_text("新增一条自定义优化提示词")
                    .clicked()
                {
                    self.ai_prompt_selected = None;
                    self.ai_prompt_editor = Some(AiPromptDraft::blank());
                }
            });
        });
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .id_salt("ai_prompts")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let available = ui.available_width();
                // 列表和编辑区各占一半；窄窗口下让列表优先，编辑区自然收窄。
                let list_width = (available * 0.42).clamp(240.0, 460.0);
                ui.horizontal_top(|ui| {
                    // 高度给 0 让它按内容撑开：这里已经在滚动区里，若按
                    // available_height 预留，列表会独占整屏，把下面的标准预览挤出可视区。
                    ui.allocate_ui_with_layout(
                        egui::vec2(list_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| self.ai_prompt_list_ui(ui),
                    );
                    // 这里不用 ui.separator()：横向布局里的分隔线会撑满可视高度，
                    // 把下面的标准预览顶出滚动区。
                    ui.add_space(12.0);
                    ui.vertical(|ui| self.ai_prompt_editor_ui(ui));
                });
                ui.add_space(12.0);
                self.output_contract_preview_ui(ui);
            });
    }

    fn ai_prompt_list_ui(&mut self, ui: &mut egui::Ui) {
        let mut edit: Option<u32> = None;
        let mut duplicate: Option<u32> = None;
        let mut restore: Option<u32> = None;
        let mut move_up: Option<usize> = None;
        let mut move_down: Option<usize> = None;
        let mut delete: Option<u32> = None;
        let last = self.config.ai_prompts.len().saturating_sub(1);

        for (index, entry) in self.config.ai_prompts.iter().enumerate() {
            let selected = self.ai_prompt_selected == Some(entry.id);
            let frame = if selected {
                theme::card().fill(theme::ACCENT_SOFT)
            } else {
                theme::card()
            };
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&entry.name).strong());
                    if entry.is_builtin() {
                        theme::chip(ui, "内置", theme::INFO, theme::SURFACE_SUNK);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if entry.is_builtin() {
                            if theme::icon_button(ui, theme::Icon::RotateCcw, "恢复默认")
                                .on_hover_text("把这条内置提示词还原为出厂内容")
                                .clicked()
                            {
                                restore = Some(entry.id);
                            }
                        } else if theme::icon_button(ui, theme::Icon::Trash, "删除").clicked() {
                            delete = Some(entry.id);
                        }
                        if theme::icon_button(ui, theme::Icon::Copy, "复制一份")
                            .on_hover_text("以这条为底稿新建一条可自由修改的提示词")
                            .clicked()
                        {
                            duplicate = Some(entry.id);
                        }
                        if theme::icon_button_enabled(
                            ui,
                            index < last,
                            theme::Icon::ArrowDown,
                            "下移",
                        )
                        .clicked()
                        {
                            move_down = Some(index);
                        }
                        if theme::icon_button_enabled(ui, index > 0, theme::Icon::ArrowUp, "上移")
                            .on_hover_text("列表顺序就是选择面板里的顺序")
                            .clicked()
                        {
                            move_up = Some(index);
                        }
                    });
                });
                ui.weak(entry.kinds_label());
                let preview = if entry.instruction.trim().is_empty() {
                    "（无附加指令：只按内置标准做格式规整）".to_string()
                } else {
                    summarize(&entry.instruction, 60)
                };
                ui.label(egui::RichText::new(preview).color(theme::TEXT_SOFT));
                if ui
                    .add(theme::icon_text_button(theme::Icon::Edit, "编辑"))
                    .clicked()
                {
                    edit = Some(entry.id);
                }
            });
            ui.add_space(6.0);
        }

        if self.config.ai_prompts.is_empty() {
            ui.weak("提示词库为空。点右上角“新建提示词”添加一条。");
        }

        if let Some(index) = move_up {
            self.config.ai_prompts.swap(index, index - 1);
        }
        if let Some(index) = move_down {
            self.config.ai_prompts.swap(index, index + 1);
        }
        if let Some(id) = edit
            && let Some(entry) = self.config.ai_prompt(id)
        {
            self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
            self.ai_prompt_selected = Some(id);
        }
        if let Some(id) = duplicate {
            self.duplicate_ai_prompt(id);
        }
        if let Some(id) = restore {
            self.restore_builtin_ai_prompt(id);
        }
        if let Some(id) = delete {
            self.ai_prompt_delete_confirm = Some(id);
        }
        self.ai_prompt_delete_confirm_ui(ui);
    }

    /// 删除确认就地展开，不再弹窗；确认后同时清掉可能正在编辑它的编辑区。
    fn ai_prompt_delete_confirm_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.ai_prompt_delete_confirm else {
            return;
        };
        let Some(name) = self.config.ai_prompt(id).map(|entry| entry.name.clone()) else {
            self.ai_prompt_delete_confirm = None;
            return;
        };
        theme::card().fill(theme::DANGER_SOFT).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(theme::DANGER, format!("删除提示词“{name}”？"));
            ui.horizontal(|ui| {
                if ui
                    .add(theme::warning_icon_button(theme::Icon::Trash, "确认删除"))
                    .clicked()
                {
                    self.config.ai_prompts.retain(|entry| entry.id != id);
                    if self.ai_prompt_selected == Some(id) {
                        self.ai_prompt_selected = None;
                        self.ai_prompt_editor = None;
                    }
                    self.ai_prompt_delete_confirm = None;
                    self.status = format!("已删除提示词“{name}”。记得点“保存更改”。");
                }
                if ui.button("取消").clicked() {
                    self.ai_prompt_delete_confirm = None;
                }
            });
        });
    }

    fn ai_prompt_editor_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut draft) = self.ai_prompt_editor.take() else {
            ui.add_space(16.0);
            ui.weak("在左侧选一条提示词编辑，或新建一条。");
            return;
        };
        let mut close = false;
        let mut submit = false;

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(if draft.id.is_some() {
                        "编辑提示词"
                    } else {
                        "新建提示词"
                    })
                    .strong(),
                );
                if !draft.builtin_key.is_empty() {
                    theme::chip(ui, "内置", theme::INFO, theme::SURFACE_SUNK);
                }
            });
            ui.add_space(6.0);

            ui.label("名称");
            ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text("例如：精简篇幅")
                    .desired_width(ui.available_width()),
            );
            ui.add_space(8.0);

            ui.label("适用文种");
            ui.weak("一个都不勾表示所有文种通用；勾选后只在对应文种的选择面板里出现。");
            ui.horizontal_wrapped(|ui| {
                for kind in TemplateKind::ALL {
                    let mut checked = draft.kinds.contains(&kind);
                    if ui.checkbox(&mut checked, kind.label()).changed() {
                        if checked {
                            draft.kinds.push(kind);
                        } else {
                            draft.kinds.retain(|item| *item != kind);
                        }
                    }
                }
            });
            ui.add_space(8.0);

            ui.label("优化指令");
            ui.weak(
                "只写“这次要模型做什么”。输出的 Markdown 结构、表格写法、\
不得输出版记落款等要求由内置标准强制，无需也无法在这里改。",
            );
            ui.add(
                egui::TextEdit::multiline(&mut draft.instruction)
                    .hint_text("留空表示只按内置标准做格式规整")
                    .desired_width(ui.available_width())
                    .desired_rows(10),
            );

            if let Some(error) = &draft.error {
                ui.add_space(4.0);
                ui.colored_label(WARN, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "应用到提示词库")
                    .on_hover_text("写回提示词库；仍需点右上角“保存更改”落盘")
                    .clicked()
                {
                    submit = true;
                }
                if ui.button("取消").clicked() {
                    close = true;
                }
            });
        });

        if submit {
            match self.apply_ai_prompt_draft(&draft) {
                Ok(id) => {
                    self.ai_prompt_selected = Some(id);
                    self.status = "提示词已更新。点右上角“保存更改”写入本机配置。".into();
                    return;
                }
                Err(message) => draft.error = Some(message),
            }
        }
        if !close {
            self.ai_prompt_editor = Some(draft);
        }
    }

    /// 把编辑区内容写回提示词库。名称必填且同名会挡下——选择面板只显示名称，
    /// 重名了没法区分。
    fn apply_ai_prompt_draft(&mut self, draft: &AiPromptDraft) -> Result<u32, String> {
        let name = draft.name.trim();
        if name.is_empty() {
            return Err("请填写提示词名称。".into());
        }
        if self
            .config
            .ai_prompts
            .iter()
            .any(|entry| entry.name.trim() == name && Some(entry.id) != draft.id)
        {
            return Err(format!("已有同名提示词“{name}”，请换一个名称。"));
        }
        // 勾选顺序不确定，按文种固有顺序归一，列表文案才稳定。
        let kinds = TemplateKind::ALL
            .into_iter()
            .filter(|kind| draft.kinds.contains(kind))
            .collect::<Vec<_>>();

        match draft.id {
            Some(id) => {
                let entry = self
                    .config
                    .ai_prompts
                    .iter_mut()
                    .find(|entry| entry.id == id)
                    .ok_or_else(|| "提示词已不存在，请重新选择。".to_string())?;
                entry.name = name.to_string();
                entry.instruction = draft.instruction.trim().to_string();
                entry.kinds = kinds;
                Ok(id)
            }
            None => {
                let id = self.config.next_ai_prompt_id();
                self.config.ai_prompts.push(AiPrompt {
                    id,
                    name: name.to_string(),
                    instruction: draft.instruction.trim().to_string(),
                    kinds,
                    builtin_key: String::new(),
                });
                Ok(id)
            }
        }
    }

    /// 复制出来的副本一律是自定义条目（不带 builtin_key），可以随便改和删。
    fn duplicate_ai_prompt(&mut self, id: u32) {
        let Some(source) = self.config.ai_prompt(id).cloned() else {
            return;
        };
        let mut name = format!("{} 副本", source.name);
        let mut suffix = 2;
        while self
            .config
            .ai_prompts
            .iter()
            .any(|entry| entry.name == name)
        {
            name = format!("{} 副本{suffix}", source.name);
            suffix += 1;
        }
        let new_id = self.config.next_ai_prompt_id();
        self.config.ai_prompts.push(AiPrompt {
            id: new_id,
            name,
            instruction: source.instruction,
            kinds: source.kinds,
            builtin_key: String::new(),
        });
        self.ai_prompt_selected = Some(new_id);
        if let Some(entry) = self.config.ai_prompt(new_id) {
            self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
        }
        self.status = "已复制一份可自由修改的提示词。".into();
    }

    /// 内置项改坏了可以还原：按 builtin_key 找出厂内容覆盖回去，id 和排序不变。
    fn restore_builtin_ai_prompt(&mut self, id: u32) {
        let Some(key) = self
            .config
            .ai_prompt(id)
            .map(|entry| entry.builtin_key.clone())
        else {
            return;
        };
        let Some(defaults) = builtin_ai_prompts()
            .into_iter()
            .find(|entry| entry.builtin_key == key)
        else {
            return;
        };
        let Some(entry) = self
            .config
            .ai_prompts
            .iter_mut()
            .find(|entry| entry.id == id)
        else {
            return;
        };
        entry.name = defaults.name;
        entry.instruction = defaults.instruction;
        entry.kinds = defaults.kinds;
        if self.ai_prompt_selected == Some(id)
            && let Some(entry) = self.config.ai_prompt(id)
        {
            self.ai_prompt_editor = Some(AiPromptDraft::from_entry(entry));
        }
        self.status = "已恢复该内置提示词的出厂内容。".into();
    }

    /// 把内置输出标准原样摊开给用户看。它是不可编辑的，但藏着不说会让人
    /// 怀疑自定义提示词到底还受不受约束。
    fn output_contract_preview_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("查看内置输出格式标准（只读）")
            .id_salt("output_contract_preview")
            .show(ui, |ui| {
                ui.weak(
                    "下面这段会自动拼在每条提示词之后，并声明优先级更高：\
自定义指令与它冲突时，一律以它为准。",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("文种");
                    egui::ComboBox::from_id_salt("contract_kind")
                        .selected_text(self.ai_contract_preview_kind.label())
                        .show_ui(ui, |ui| {
                            for kind in TemplateKind::ALL {
                                ui.selectable_value(
                                    &mut self.ai_contract_preview_kind,
                                    kind,
                                    kind.label(),
                                );
                            }
                        });
                });
                ui.add_space(6.0);
                let mut contract = prompt::output_contract(self.ai_contract_preview_kind);
                ui.add(
                    egui::TextEdit::multiline(&mut contract)
                        .desired_width(ui.available_width())
                        .desired_rows(16)
                        .interactive(false),
                );
            });
    }

    /// “AI 优化”的提示词选择面板。列出适用当前文种的条目，单击即执行；
    /// 底部可以写一条只用一次的临时指令。
    fn ai_prompt_picker_window(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.ai_prompt_picker.take() else {
            return;
        };
        // 审校稿为空就是从零起草，非空就是改现有稿件：同一个面板，两种口径。
        let drafting = self
            .active_doc_ref()
            .is_none_or(|doc| doc.generated_markdown.trim().is_empty());
        let mut chosen: Option<(String, String)> = None;
        let mut close = false;

        egui::Window::new(if drafting {
            "AI 起草"
        } else {
            "AI 优化"
        })
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.weak(format!(
                    "当前文种：{}　输出格式标准始终生效，不受下面的指令影响。",
                    picker.kind.label()
                ));
                if drafting {
                    ui.colored_label(
                        ACCENT,
                        "当前审校稿为空：下面写明要起草什么，将结合左侧公文要素从零生成。",
                    );
                }
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut picker.keyword)
                        .hint_text("按名称筛选")
                        .desired_width(ui.available_width()),
                );
                ui.add_space(6.0);

                let keyword = picker.keyword.trim().to_lowercase();
                let matches = self
                    .config
                    .ai_prompts
                    .iter()
                    .filter(|entry| entry.applies_to(picker.kind))
                    .filter(|entry| {
                        keyword.is_empty() || entry.name.to_lowercase().contains(&keyword)
                    })
                    .cloned()
                    .collect::<Vec<_>>();

                egui::ScrollArea::vertical()
                    .id_salt("ai_prompt_picker")
                    .max_height(280.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if matches.is_empty() {
                            ui.weak("没有适用于当前文种的提示词。可以在下面临时写一条，或到“AI 管理”页新增。");
                        }
                        for entry in &matches {
                            let last_used = self.config.last_ai_prompt == entry.id;
                            let frame = if last_used {
                                theme::card().fill(theme::ACCENT_SOFT)
                            } else {
                                theme::card()
                            };
                            let response = frame
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(&entry.name).strong());
                                        if last_used {
                                            theme::chip(
                                                ui,
                                                "上次使用",
                                                theme::ACCENT,
                                                theme::SURFACE_SUNK,
                                            );
                                        }
                                    });
                                    let preview = if entry.instruction.trim().is_empty() {
                                        "只按内置标准做格式规整，不改措辞。".to_string()
                                    } else {
                                        summarize(&entry.instruction, 70)
                                    };
                                    ui.label(
                                        egui::RichText::new(preview).color(theme::TEXT_SOFT),
                                    );
                                })
                                .response
                                .interact(egui::Sense::click());
                            let tip = if drafting {
                                "按这条提示词起草（素材写在下面的临时提示词里）"
                            } else {
                                "按这条提示词优化当前稿件"
                            };
                            if response.on_hover_text(tip).clicked() {
                                self.config.last_ai_prompt = entry.id;
                                chosen =
                                    Some((entry.instruction.clone(), entry.name.clone()));
                            }
                            ui.add_space(4.0);
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.label(
                    egui::RichText::new(if drafting {
                        "写作素材与要求（用完即弃，不进提示词库）"
                    } else {
                        "临时提示词（用完即弃，不进提示词库）"
                    })
                    .strong(),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut picker.custom)
                        .hint_text(if drafting {
                            "例如：就 2026 年度教师培训经费事项向省财政厅去函，背景是……，请求是……"
                        } else {
                            "例如：把第三部分改写成三条并列举措，保留全部数据"
                        })
                        .desired_width(ui.available_width())
                        .desired_rows(if drafting { 8 } else { 4 }),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let has_custom = !picker.custom.trim().is_empty();
                    if theme::primary_icon_button_enabled(
                        ui,
                        has_custom,
                        theme::Icon::Sparkles,
                        if drafting { "按此素材起草" } else { "按此提示词优化" },
                    )
                    .clicked()
                    {
                        let label = if drafting { "临时素材" } else { "临时提示词" };
                        chosen = Some((picker.custom.trim().to_string(), label.into()));
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });

        if let Some((instruction, label)) = chosen {
            self.draft_page().start_optimize(instruction, label);
            return;
        }
        if !close {
            self.ai_prompt_picker = Some(picker);
        }
    }

    // ---------- 稿件管理（SQLite 稿件库） ----------

    fn manuscript_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.manuscript_error.clone() {
            ui.colored_label(WARN, error);
            return;
        }
        if self.manuscript_store.is_none() {
            ui.colored_label(WARN, "稿件库不可用。");
            return;
        }

        let mut action: Option<ManuscriptAction> = None;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("稿件管理");
                let total: i64 = self.manuscript_count.iter().sum();
                ui.weak(format!(
                    "共 {total} 篇 · 草稿 {} · 发布 {} · 归档 {} · 仅保存在本机",
                    self.manuscript_count[1], self.manuscript_count[2], self.manuscript_count[3],
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(theme::Icon::FilePlus, "新建稿件"))
                    .on_hover_text("清空起草页，开始一份全新的稿件")
                    .clicked()
                {
                    self.new_blank_manuscript();
                }
            });
        });
        ui.add_space(8.0);

        self.manuscript_filter_bar(ui);
        self.refresh_manuscript_rows();
        self.manuscript_confirm_groups(ui, &mut action);

        // 列表是工作台的主视图；单击一行只在右侧更新资料卡，不再把整页替换成
        // “详情页”。真正查看正文统一进入公文编辑标签，发布/归档稿会自动只读。
        let pdf_action = egui::Panel::right("manuscript_metadata_panel")
            .resizable(true)
            .default_size(320.0)
            .size_range(280.0..=440.0)
            .frame(theme::panel(theme::SURFACE, 10))
            .show(ui, |ui| self.manuscript_detail_ui(ui, &mut action))
            .inner;
        self.manuscript_list_ui(ui, &mut action);

        if let Some(act) = action {
            self.apply_manuscript_action(act);
        }
        if let Some(act) = pdf_action {
            self.apply_pdf_action(act);
        }
    }

    fn manuscript_filter_bar(&mut self, ui: &mut egui::Ui) {
        // 文本框、下拉框和按钮的默认内容边距不同。统一交互高度后，各筛选组的
        // 外框高度一致，组内标签也会落在同一条垂直中线上。
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = FORM_CONTROL_HEIGHT;
            ui.horizontal_wrapped(|ui| {
                ui.horizontal(|ui| {
                    ui.label("搜索");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.manuscript_filter.keyword)
                                .hint_text("标题、文号或备注")
                                .desired_width(220.0),
                        )
                        .changed()
                    {
                        self.manuscript_dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("状态");
                    egui::ComboBox::from_id_salt("manuscript_status")
                        .selected_text(
                            self.manuscript_filter
                                .status
                                .map(|s| s.label())
                                .unwrap_or("全部状态"),
                        )
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut self.manuscript_filter.status,
                                    None,
                                    "全部状态",
                                )
                                .changed()
                            {
                                self.manuscript_dirty = true;
                            }
                            for status in ManuscriptStatus::ALL {
                                if status == ManuscriptStatus::New {
                                    continue; // “新建”已取消，历史记录创建时统一升为草稿。
                                }
                                if ui
                                    .selectable_value(
                                        &mut self.manuscript_filter.status,
                                        Some(status),
                                        status.label(),
                                    )
                                    .changed()
                                {
                                    self.manuscript_dirty = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("文种");
                    egui::ComboBox::from_id_salt("manuscript_kind")
                        .selected_text(
                            self.manuscript_filter
                                .kind
                                .map(|k| k.label())
                                .unwrap_or("全部文种"),
                        )
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_value(
                                    &mut self.manuscript_filter.kind,
                                    None,
                                    "全部文种",
                                )
                                .changed()
                            {
                                self.manuscript_dirty = true;
                            }
                            for kind in TemplateKind::ALL {
                                if ui
                                    .selectable_value(
                                        &mut self.manuscript_filter.kind,
                                        Some(kind),
                                        kind.label(),
                                    )
                                    .changed()
                                {
                                    self.manuscript_dirty = true;
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("成文日期");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.manuscript_filter.date_from)
                                .hint_text("起 YYYY-MM-DD")
                                .desired_width(110.0),
                        )
                        .changed()
                    {
                        self.manuscript_dirty = true;
                    }
                    ui.label("至");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.manuscript_filter.date_to)
                                .hint_text("止 YYYY-MM-DD")
                                .desired_width(110.0),
                        )
                        .changed()
                    {
                        self.manuscript_dirty = true;
                    }
                });
                let filter_active = !self.manuscript_filter.keyword.trim().is_empty()
                    || self.manuscript_filter.status.is_some()
                    || self.manuscript_filter.kind.is_some()
                    || !self.manuscript_filter.date_from.trim().is_empty()
                    || !self.manuscript_filter.date_to.trim().is_empty();
                if filter_active
                    && theme::icon_button(ui, theme::Icon::SearchClear, "清除筛选").clicked()
                {
                    self.manuscript_filter = ManuscriptFilter::default();
                    self.manuscript_dirty = true;
                }
                ui.separator();
                let selected = self.manuscript_selected.len();
                if selected > 0 {
                    ui.label(format!("已选 {selected} 篇"));
                    if ui
                        .add(theme::icon_text_button(theme::Icon::Package, "导出所选"))
                        .on_hover_text("仅导出列表中已勾选的稿件及其 PDF 附件")
                        .clicked()
                    {
                        self.export_selected_manuscripts_zip();
                    }
                    if ui
                        .add(theme::icon_text_button(
                            theme::Icon::PackageOpen,
                            "导入到知识库",
                        ))
                        .on_hover_text("把勾选的稿件切块、向量化后加入知识库，供起草时检索参考")
                        .clicked()
                    {
                        self.knowledge_import_selected_manuscripts();
                    }
                    let deletable = self.manuscript_rows.iter().any(|row| {
                        self.manuscript_selected.contains(&row.id)
                            && row.status != ManuscriptStatus::Archived
                    });
                    if ui
                        .add_enabled(
                            deletable,
                            theme::warning_icon_button(theme::Icon::Trash, "批量删除"),
                        )
                        .on_hover_text("归档稿件不会被删除")
                        .clicked()
                    {
                        self.manuscript_batch_delete_confirm = true;
                    }
                    if theme::icon_button(ui, theme::Icon::X, "清空选择").clicked() {
                        self.manuscript_selected.clear();
                    }
                    ui.separator();
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::Package, "按筛选导出"))
                    .on_hover_text("按当前过滤条件导出稿件（含 PDF 附件）")
                    .clicked()
                {
                    self.export_manuscripts_zip();
                }
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::PackageOpen,
                        "导入 ZIP",
                    ))
                    .on_hover_text("从 ZIP 稿件包导入，先预览后确认")
                    .clicked()
                {
                    self.start_import_manuscript();
                }
            });
        });
        ui.add_space(6.0);
    }

    fn refresh_manuscript_rows(&mut self) {
        // 「已入库」标记要在稿件管理页上也是准的，哪怕用户从没打开过知识库页。
        if self.knowledge_dirty
            && let Some(knowledge) = self.knowledge_store.as_mut()
        {
            self.knowledge_indexed_manuscripts =
                knowledge.indexed_manuscript_ids().unwrap_or_default();
        }
        let Some(store) = self.manuscript_store.as_mut() else {
            return;
        };
        if self.manuscript_applied != Some(self.manuscript_filter.clone()) || self.manuscript_dirty
        {
            match store.list(&self.manuscript_filter) {
                Ok(rows) => {
                    self.manuscript_rows = rows;
                    let visible = self
                        .manuscript_rows
                        .iter()
                        .map(|row| row.id)
                        .collect::<BTreeSet<_>>();
                    self.manuscript_selected.retain(|id| visible.contains(id));
                    if self
                        .manuscript_detail
                        .as_ref()
                        .is_some_and(|detail| !visible.contains(&detail.id))
                    {
                        self.manuscript_detail = None;
                        self.manuscript_detail_delete_pdf = None;
                        self.manuscript_versions.clear();
                    } else if let Some(detail_id) =
                        self.manuscript_detail.as_ref().map(|detail| detail.id)
                        && let Ok(Some(record)) = store.get(detail_id)
                    {
                        // 发布、退回草稿或归档后，右侧状态和时间立即跟着列表更新。
                        self.manuscript_detail = Some(record);
                    }
                    self.manuscript_count = store.count_by_status().unwrap_or([0; 4]);
                    self.manuscript_dirty = false;
                    self.manuscript_applied = Some(self.manuscript_filter.clone());
                }
                Err(error) => self.status = format!("查询稿件失败：{error:#}"),
            }
        }
    }

    /// 删除 / 归档 / 导入预览三组确认区。可能写入 `action`，在帧末执行。
    fn manuscript_confirm_groups(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<ManuscriptAction>,
    ) {
        if let Some(id) = self.manuscript_delete_confirm {
            let mut do_delete = false;
            let mut do_cancel = false;
            ui.group(|ui| {
                ui.colored_label(WARN, "删除后不可恢复，确认删除这篇稿件吗？");
                ui.horizontal(|ui| {
                    if ui.button("确认删除").clicked() {
                        do_delete = true;
                    }
                    if ui.button("取消").clicked() {
                        do_cancel = true;
                    }
                });
            });
            if do_cancel {
                self.manuscript_delete_confirm = None;
            } else if do_delete {
                self.manuscript_delete_confirm = None;
                *action = Some(ManuscriptAction::Delete(id));
            }
            ui.add_space(6.0);
        }

        if self.manuscript_batch_delete_confirm {
            let deletable = self
                .manuscript_rows
                .iter()
                .filter(|row| {
                    self.manuscript_selected.contains(&row.id)
                        && row.status != ManuscriptStatus::Archived
                })
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let archived = self
                .manuscript_selected
                .len()
                .saturating_sub(deletable.len());
            let mut confirm = false;
            let mut cancel = false;
            ui.group(|ui| {
                ui.colored_label(
                    WARN,
                    format!(
                        "确认删除所选的 {} 篇可删除稿件吗？此操作不可恢复。",
                        deletable.len()
                    ),
                );
                if archived > 0 {
                    ui.weak(format!("另有 {archived} 篇归档稿件受保护，将保留不动。"));
                }
                ui.horizontal(|ui| {
                    if ui.button("确认批量删除").clicked() {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
            if cancel {
                self.manuscript_batch_delete_confirm = false;
            } else if confirm {
                self.manuscript_batch_delete_confirm = false;
                *action = Some(ManuscriptAction::DeleteSelected(deletable));
            }
            ui.add_space(6.0);
        }

        let mut archive_to_confirm: Option<i64> = None;
        if self.manuscript_archive_pending.is_some() {
            let (manuscript_id, do_archive, do_cancel) = {
                let pending = self.manuscript_archive_pending.as_mut().unwrap();
                let manuscript_id = pending.manuscript_id;
                let mut do_archive = false;
                let mut do_cancel = false;
                ui.group(|ui| {
                    ui.colored_label(
                        WARN,
                        "归档将冻结该稿件：标题、正文、时间等关键信息此后均不可修改。",
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add(theme::icon_text_button(
                                theme::Icon::Paperclip,
                                "选择扫描盖章 PDF…",
                            ))
                            .on_hover_text("可多选；归档后仍可在详情页继续添加附件")
                            .clicked()
                            && let Some(paths) = rfd::FileDialog::new()
                                .add_filter("扫描盖章 PDF", &["pdf"])
                                .pick_files()
                        {
                            pending.pdf_paths.extend(paths);
                        }
                        if !pending.pdf_paths.is_empty() {
                            ui.label(format!("已选 {} 个 PDF：", pending.pdf_paths.len()));
                            for path in pending.pdf_paths.iter() {
                                let name = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("PDF")
                                    .to_string();
                                ui.label(format!("  {name}"));
                            }
                        }
                        ui.separator();
                        if ui.button("确认归档").clicked() {
                            do_archive = true;
                        }
                        if ui.button("取消").clicked() {
                            do_cancel = true;
                        }
                    });
                });
                (manuscript_id, do_archive, do_cancel)
            };
            if do_cancel {
                self.manuscript_archive_pending = None;
            } else if do_archive {
                // 保留 pending（含已选 PDF），由 Archive action 读取并执行归档。
                archive_to_confirm = Some(manuscript_id);
            }
            ui.add_space(6.0);
        }
        if let Some(id) = archive_to_confirm {
            *action = Some(ManuscriptAction::Archive(id));
        }

        if self.manuscript_import_preview.is_some() {
            let (confirm, cancel) = {
                let preview = self.manuscript_import_preview.as_mut().unwrap();
                let mut confirm = false;
                let mut cancel = false;
                ui.group(|ui| {
                    let total = preview.manifest.records.len();
                    let selected = preview.selected.iter().filter(|b| **b).count();
                    let archived = preview
                        .manifest
                        .records
                        .iter()
                        .filter(|r| r.status == ManuscriptStatus::Archived)
                        .count();
                    ui.strong("导入预览");
                    ui.label(format!(
                        "共 {total} 篇（归档 {archived} 篇），已勾选 {selected} 篇。"
                    ));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut preview.keyword)
                                .hint_text("按标题/文号过滤")
                                .desired_width(220.0),
                        );
                        if ui
                            .add(theme::icon_text_button(theme::Icon::SquareCheck, "全选"))
                            .clicked()
                        {
                            for b in preview.selected.iter_mut() {
                                *b = true;
                            }
                        }
                        if ui
                            .add(theme::icon_text_button(theme::Icon::Square, "全不选"))
                            .clicked()
                        {
                            for b in preview.selected.iter_mut() {
                                *b = false;
                            }
                        }
                    });
                    ui.checkbox(&mut preview.skip_existing, "跳过与本地同源的已有记录")
                        .on_hover_text("按清单里的源 id 去重，重复导入同一份文件不会产生副本");
                    let keyword = preview.keyword.trim().to_lowercase();
                    egui::ScrollArea::vertical()
                        .id_salt("import_preview_list")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for (index, record) in preview.manifest.records.iter().enumerate() {
                                if !keyword.is_empty()
                                    && !record.title.to_lowercase().contains(&keyword)
                                    && !record.doc_number.to_lowercase().contains(&keyword)
                                {
                                    continue;
                                }
                                ui.checkbox(
                                    &mut preview.selected[index],
                                    format!(
                                        "{} · {} · {}（{}）",
                                        record.doc_date,
                                        record.kind.label(),
                                        truncate(&record.title, 40),
                                        record.status.label(),
                                    ),
                                );
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button("确认导入").clicked() {
                            confirm = true;
                        }
                        if ui.button("取消").clicked() {
                            cancel = true;
                        }
                    });
                });
                (confirm, cancel)
            };
            if cancel {
                self.manuscript_import_preview = None;
            } else if confirm {
                self.confirm_import();
            }
            ui.add_space(6.0);
        }
    }

    fn manuscript_list_ui(&mut self, ui: &mut egui::Ui, action: &mut Option<ManuscriptAction>) {
        if self.manuscript_rows.is_empty() {
            ui.add_space(12.0);
            ui.weak("没有符合条件的稿件。在起草页点“保存到稿件库”，或调整过滤条件。");
            return;
        }
        egui::ScrollArea::both()
            .id_salt("manuscript_list")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                egui::Grid::new("manuscript_grid")
                    .striped(true)
                    .min_col_width(40.0)
                    .show(ui, |ui| {
                        let visible_ids = self
                            .manuscript_rows
                            .iter()
                            .map(|row| row.id)
                            .collect::<Vec<_>>();
                        let mut all_selected = !visible_ids.is_empty()
                            && visible_ids
                                .iter()
                                .all(|id| self.manuscript_selected.contains(id));
                        if ui
                            .checkbox(&mut all_selected, "")
                            .on_hover_text(if all_selected {
                                "取消选择当前列表全部稿件"
                            } else {
                                "选择当前列表全部稿件"
                            })
                            .changed()
                        {
                            if all_selected {
                                self.manuscript_selected.extend(visible_ids);
                            } else {
                                for id in visible_ids {
                                    self.manuscript_selected.remove(&id);
                                }
                            }
                        }
                        ui.strong("状态");
                        ui.strong("文种");
                        ui.strong("密级");
                        ui.strong("标题");
                        ui.strong("文号");
                        ui.strong("成文日期");
                        ui.strong("更新");
                        ui.strong("归档");
                        ui.strong("知识库");
                        ui.strong("操作");
                        ui.end_row();
                        for row in self.manuscript_rows.iter() {
                            let mut batch_selected = self.manuscript_selected.contains(&row.id);
                            if ui.checkbox(&mut batch_selected, "").changed() {
                                if batch_selected {
                                    self.manuscript_selected.insert(row.id);
                                } else {
                                    self.manuscript_selected.remove(&row.id);
                                }
                            }
                            let row_selected = self
                                .manuscript_detail
                                .as_ref()
                                .is_some_and(|detail| detail.id == row.id);
                            let mut row_clicked = false;
                            let mut row_double_clicked = false;
                            let mut row_cell = |response: egui::Response| {
                                row_clicked |= response.clicked();
                                row_double_clicked |= response.double_clicked();
                            };
                            row_cell(
                                ui.selectable_label(
                                    row_selected,
                                    egui::RichText::new(row.status.label())
                                        .color(status_color(row.status)),
                                ),
                            );
                            row_cell(ui.selectable_label(row_selected, row.kind.label()));
                            let security_level = SecurityLevel::from_marking(&row.security_level);
                            row_cell(
                                ui.selectable_label(
                                    row_selected,
                                    egui::RichText::new(security_level_list_label(security_level))
                                        .color(security_level_color(security_level)),
                                ),
                            );
                            row_cell(ui.add_sized(
                                [260.0, 20.0],
                                egui::Button::selectable(row_selected, &row.title).truncate(),
                            ));
                            row_cell(ui.add_sized(
                                [180.0, 20.0],
                                egui::Button::selectable(row_selected, &row.doc_number).truncate(),
                            ));
                            row_cell(ui.selectable_label(row_selected, short_date(&row.doc_date)));
                            row_cell(
                                ui.selectable_label(row_selected, short_date(&row.updated_at)),
                            );
                            row_cell(
                                ui.selectable_label(
                                    row_selected,
                                    row.archived_at
                                        .as_deref()
                                        .map(short_date)
                                        .unwrap_or_else(|| "—".to_string()),
                                ),
                            );
                            // 已入库标记：导入前就看得出哪些进过知识库，免得同一篇
                            // 反复导入。跨来源去重虽已兜底，但让用户看见更省事。
                            let indexed = self.knowledge_indexed_manuscripts.contains(&row.id);
                            let indexed_cell = if indexed {
                                egui::RichText::new("已入库").color(ACCENT)
                            } else {
                                egui::RichText::new("—").color(theme::TEXT_MUTED)
                            };
                            row_cell(
                                ui.selectable_label(row_selected, indexed_cell)
                                    .on_hover_text(if indexed {
                                        "这篇稿件已加入知识库，再次导入会覆盖旧的索引"
                                    } else {
                                        "尚未加入知识库，可勾选后用工具栏的「导入到知识库」"
                                    }),
                            );
                            if row_double_clicked {
                                *action = Some(ManuscriptAction::Edit(row.id));
                            } else if row_clicked {
                                *action = Some(ManuscriptAction::Detail(row.id));
                            }
                            ui.horizontal(|ui| match row.status {
                                ManuscriptStatus::Archived => {
                                    if theme::icon_button(ui, theme::Icon::Eye, "查看详情")
                                        .on_hover_text("在只读公文界面中打开")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::Edit(row.id));
                                    }
                                    if theme::icon_button(ui, theme::Icon::Copy, "基于此公文新建")
                                        .on_hover_text(
                                            "复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件",
                                        )
                                        .clicked()
                                    {
                                        *action =
                                            Some(ManuscriptAction::CreateFromExisting(row.id));
                                    }
                                }
                                ManuscriptStatus::Published => {
                                    if theme::icon_button(ui, theme::Icon::Eye, "查看详情")
                                        .on_hover_text("在只读公文界面中打开")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::Edit(row.id));
                                    }
                                    if theme::icon_button(ui, theme::Icon::Undo, "退回草稿")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::RevertToDraft(row.id));
                                    }
                                    if theme::icon_button(ui, theme::Icon::Copy, "基于此公文新建")
                                        .on_hover_text(
                                            "复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件",
                                        )
                                        .clicked()
                                    {
                                        *action =
                                            Some(ManuscriptAction::CreateFromExisting(row.id));
                                    }
                                    if theme::icon_button(ui, theme::Icon::Archive, "归档")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::ArchivePending(row.id));
                                    }
                                    if theme::danger_icon_button(ui, theme::Icon::Trash, "删除")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::DeletePending(row.id));
                                    }
                                }
                                _ => {
                                    if theme::icon_button(ui, theme::Icon::Eye, "查看详情")
                                        .on_hover_text("打开公文编辑界面")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::Edit(row.id));
                                    }
                                    if theme::icon_button(ui, theme::Icon::Copy, "基于此公文新建")
                                        .on_hover_text(
                                            "复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件",
                                        )
                                        .clicked()
                                    {
                                        *action =
                                            Some(ManuscriptAction::CreateFromExisting(row.id));
                                    }
                                    if theme::icon_button(ui, theme::Icon::Publish, "发布")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::Publish(row.id));
                                    }
                                    if theme::icon_button(ui, theme::Icon::Archive, "归档")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::ArchivePending(row.id));
                                    }
                                    if theme::danger_icon_button(ui, theme::Icon::Trash, "删除")
                                        .clicked()
                                    {
                                        *action = Some(ManuscriptAction::DeletePending(row.id));
                                    }
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
    }

    /// 稿件列表右侧的资料卡。列表行只负责切换这里的选中项；正文查看和编辑
    /// 始终打开独立的公文标签。返回本帧要执行的 PDF 附件操作。
    fn manuscript_detail_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<ManuscriptAction>,
    ) -> Option<PdfAction> {
        let Some(detail_id) = self.manuscript_detail.as_ref().map(|d| d.id) else {
            ui.heading("稿件资料");
            ui.separator();
            ui.add_space(8.0);
            ui.weak("单击左侧任意一行，在这里查看该稿件的元数据、版本和附件。");
            ui.add_space(8.0);
            ui.weak("双击一行或点“查看详情”，可直接打开公文界面。");
            return None;
        };
        let mut clear_selection = false;
        let mut open = false;
        let mut create_from_existing = false;
        let mut pdf_action: Option<PdfAction> = None;
        let mut delete_pdf: Option<i64> = None;
        let mut add_pdfs: Vec<PathBuf> = Vec::new();

        let detail = self.manuscript_detail.as_ref().unwrap();
        ui.horizontal(|ui| {
            ui.heading("稿件资料");
            ui.colored_label(status_color(detail.status), detail.status.label());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, theme::Icon::X, "清除选择").clicked() {
                    clear_selection = true;
                }
            });
        });
        ui.separator();
        let open_label = if matches!(
            detail.status,
            ManuscriptStatus::Published | ManuscriptStatus::Archived
        ) {
            "打开查看"
        } else {
            "打开编辑"
        };
        let open_clicked = theme::accent_scope(ui, |ui| {
            ui.add_sized(
                [ui.available_width(), 30.0],
                theme::primary_button_widget(theme::Icon::PencilLine, open_label),
            )
        })
        .on_hover_text("在独立的公文标签中打开；发布和归档稿自动只读")
        .clicked();
        if open_clicked {
            open = true;
        }
        if ui
            .add_sized(
                [ui.available_width(), 28.0],
                theme::icon_text_button(theme::Icon::Copy, "基于此公文新建"),
            )
            .on_hover_text("复制行文要素和正文，新稿不会继承状态、版本或 PDF 附件")
            .clicked()
        {
            create_from_existing = true;
        }
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .id_salt("manuscript_metadata_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.strong("公文元数据");
                ui.add_space(4.0);
                egui::Grid::new("manuscript_metadata_grid")
                    .num_columns(2)
                    .min_col_width(76.0)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        let profile = &detail.snapshot.profile;
                        metadata_grid_row(ui, "标题", &detail.title);
                        metadata_grid_row(ui, "文种", detail.kind.label());
                        metadata_grid_row(
                            ui,
                            "密级",
                            joined_metadata(&[
                                profile.security_level.as_str(),
                                profile.security_period.as_str(),
                            ])
                            .as_str(),
                        );
                        metadata_grid_row(ui, "文号", present_or_dash(&detail.doc_number));
                        if detail.kind == TemplateKind::OfficialLetter {
                            metadata_grid_row(
                                ui,
                                "函号年份",
                                present_or_dash(&detail.snapshot.document_year()),
                            );
                        }
                        metadata_grid_row(ui, "成文日期", present_or_dash(&detail.doc_date));
                        metadata_grid_row(ui, "发文单位", present_or_dash(&profile.issuing_unit));
                        metadata_grid_row(ui, "主送", present_or_dash(&profile.recipient));
                        metadata_grid_row(ui, "抄送", present_or_dash(&profile.copies_to));
                        metadata_grid_row(
                            ui,
                            "承办单位",
                            present_or_dash(&profile.responsible_unit),
                        );
                        metadata_grid_row(
                            ui,
                            "联系人",
                            joined_metadata(&[
                                profile.contact_person.as_str(),
                                profile.contact_phone.as_str(),
                            ])
                            .as_str(),
                        );
                        if !detail.snapshot.meeting_time.trim().is_empty() {
                            metadata_grid_row(ui, "会议时间", &detail.snapshot.meeting_time);
                        }
                        if !profile.meeting_location.trim().is_empty() {
                            metadata_grid_row(ui, "会议地点", &profile.meeting_location);
                        }
                        if !detail.snapshot.attendees.trim().is_empty() {
                            metadata_grid_row(ui, "参加人员", &detail.snapshot.attendees);
                        }
                        metadata_grid_row(ui, "创建", &short_date(&detail.created_at));
                        metadata_grid_row(ui, "更新", &short_date(&detail.updated_at));
                        if let Some(at) = &detail.published_at {
                            metadata_grid_row(ui, "发布", &short_date(at));
                        }
                        if let Some(at) = &detail.archived_at {
                            metadata_grid_row(ui, "归档", &short_date(at));
                        }
                        if !detail.notes.trim().is_empty() {
                            metadata_grid_row(ui, "备注", &detail.notes);
                        }
                    });

                ui.add_space(10.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("版本历史（{}）", self.manuscript_versions.len()));
                    if !self.manuscript_versions.is_empty()
                        && ui
                            .add(theme::icon_text_button(theme::Icon::Compare, "对照…"))
                            .on_hover_text("默认对照最新版与上一版")
                            .clicked()
                    {
                        *action = Some(ManuscriptAction::OpenVersionDiff {
                            manuscript_id: detail_id,
                        });
                    }
                });
                if self.manuscript_versions.is_empty() {
                    ui.weak("暂无已提交版本。");
                } else {
                    for version in self.manuscript_versions.iter().rev() {
                        theme::card().show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(format!("v{}", version.version_number));
                                ui.label(&version.name);
                                if version.is_latest {
                                    theme::chip(ui, "最新", theme::SUCCESS, theme::SUCCESS_SOFT);
                                }
                                ui.weak(short_date(&version.created_at));
                            });
                            if !version.comment.trim().is_empty() {
                                ui.weak(summarize(&version.comment, 50));
                            }
                            ui.horizontal_wrapped(|ui| {
                                if detail.status != ManuscriptStatus::Archived
                                    && ui.small_button("载入编辑").clicked()
                                {
                                    *action = Some(ManuscriptAction::LoadVersion {
                                        manuscript_id: detail_id,
                                        version_number: version.version_number,
                                    });
                                }
                                if detail.status != ManuscriptStatus::Archived
                                    && ui.small_button("回退至此版").clicked()
                                {
                                    *action = Some(ManuscriptAction::RevertPending {
                                        manuscript_id: detail_id,
                                        version_number: version.version_number,
                                    });
                                }
                                if ui.small_button("与上一版对照").clicked() {
                                    *action = Some(ManuscriptAction::DiffVersion {
                                        manuscript_id: detail_id,
                                        version_number: version.version_number,
                                    });
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                }

                ui.add_space(6.0);
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("PDF 附件（{}）", detail.pdfs.len()));
                    if ui
                        .add(theme::icon_text_button(theme::Icon::Paperclip, "添加…"))
                        .on_hover_text("导入扫描盖章件等；归档后也可补充")
                        .clicked()
                        && let Some(paths) = rfd::FileDialog::new()
                            .add_filter("扫描盖章 PDF", &["pdf"])
                            .pick_files()
                    {
                        add_pdfs.extend(paths);
                    }
                });
                if detail.pdfs.is_empty() {
                    ui.weak("暂无附件。");
                }
                for pdf in &detail.pdfs {
                    theme::card().show(ui, |ui| {
                        ui.add(egui::Label::new(&pdf.file_name).truncate())
                            .on_hover_text(&pdf.file_name);
                        ui.weak(short_date(&pdf.added_at));
                        ui.horizontal_wrapped(|ui| {
                            if ui.small_button("打开").clicked() {
                                pdf_action = Some(PdfAction::Open(pdf.id));
                            }
                            if ui.small_button("另存为").clicked() {
                                pdf_action = Some(PdfAction::SaveAs(pdf.id));
                            }
                            if self.manuscript_detail_delete_pdf == Some(pdf.id) {
                                if ui.small_button("确认删除").clicked() {
                                    delete_pdf = Some(pdf.id);
                                }
                                if ui.small_button("取消").clicked() {
                                    self.manuscript_detail_delete_pdf = None;
                                }
                            } else if ui.small_button("删除").clicked() {
                                self.manuscript_detail_delete_pdf = Some(pdf.id);
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });

        if clear_selection {
            self.manuscript_detail = None;
            self.manuscript_detail_delete_pdf = None;
            self.manuscript_versions.clear();
        } else if create_from_existing {
            *action = Some(ManuscriptAction::CreateFromExisting(detail_id));
        } else if open {
            *action = Some(ManuscriptAction::Edit(detail_id));
        }
        if let Some(pdf_id) = delete_pdf {
            self.manuscript_detail_delete_pdf = None;
            pdf_action = Some(PdfAction::Delete(pdf_id));
        }
        if !add_pdfs.is_empty() {
            let result: anyhow::Result<usize> = (|| {
                let store = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                let mut added = 0;
                for path in &add_pdfs {
                    let bytes = std::fs::read(path)?;
                    let name = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("附件.pdf")
                        .to_string();
                    store.add_pdf(detail_id, &name, &bytes)?;
                    added += 1;
                }
                Ok(added)
            })();
            self.reload_detail();
            match result {
                Ok(added) => self.status = format!("已添加 {added} 个附件。"),
                Err(error) => self.status = format!("添加附件失败：{error:#}"),
            }
        }
        pdf_action
    }

    fn apply_manuscript_action(&mut self, action: ManuscriptAction) {
        match action {
            ManuscriptAction::Detail(id) => self.refresh_detail(id),
            ManuscriptAction::Edit(id) => self.open_in_editor(id),
            ManuscriptAction::CreateFromExisting(id) => self.create_from_existing(id),
            ManuscriptAction::Publish(id) => {
                self.transition_status(id, ManuscriptStatus::Published);
                self.sync_record_status(id);
            }
            ManuscriptAction::RevertToDraft(id) => {
                self.transition_status(id, ManuscriptStatus::Draft);
                self.sync_record_status(id);
            }
            ManuscriptAction::DeletePending(id) => {
                self.manuscript_delete_confirm = Some(id);
            }
            ManuscriptAction::ArchivePending(id) => {
                self.manuscript_archive_pending = Some(ArchivePending {
                    manuscript_id: id,
                    pdf_paths: Vec::new(),
                });
            }
            ManuscriptAction::Archive(id) => {
                let pdfs = self
                    .manuscript_archive_pending
                    .take()
                    .map(|p| p.pdf_paths)
                    .unwrap_or_default();
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.set_status(id, ManuscriptStatus::Archived)?;
                    for path in &pdfs {
                        let bytes = std::fs::read(path)?;
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("扫描件.pdf")
                            .to_string();
                        store.add_pdf(id, &name, &bytes)?;
                    }
                    Ok(())
                })();
                self.manuscript_dirty = true;
                match result {
                    Ok(()) => {
                        self.sync_record_status(id);
                        self.status = format!("已归档，附带 {} 个 PDF 附件。", pdfs.len());
                    }
                    Err(error) => self.status = format!("归档失败：{error:#}"),
                }
            }
            ManuscriptAction::Delete(id) => {
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.delete(id)?;
                    Ok(())
                })();
                self.manuscript_dirty = true;
                match result {
                    Ok(()) => {
                        self.detach_docs_of(id);
                        self.status = format!("已删除稿件 #{id}。");
                    }
                    Err(error) => self.status = format!("删除失败：{error:#}"),
                }
            }
            ManuscriptAction::DeleteSelected(ids) => {
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.delete_many(&ids)?;
                    Ok(())
                })();
                self.manuscript_dirty = true;
                match result {
                    Ok(()) => {
                        for id in &ids {
                            self.detach_docs_of(*id);
                            self.manuscript_selected.remove(id);
                        }
                        self.status = format!("已批量删除 {} 篇稿件。", ids.len());
                    }
                    Err(error) => self.status = format!("批量删除失败：{error:#}"),
                }
            }
            ManuscriptAction::DiffVersion {
                manuscript_id,
                version_number,
            } => {
                // 看某一版"改了什么"就是拿它跟上一版比：旧在左、新在右。
                self.version_diff = Some(VersionDiffState {
                    scope: VersionScope::Manuscript(manuscript_id),
                    from: (version_number > 1).then_some(version_number - 1),
                    to: Some(version_number),
                    to_is_current_config: false,
                    view: DiffViewState::default(),
                });
            }
            ManuscriptAction::OpenVersionDiff { manuscript_id } => {
                let latest = self.manuscript_versions.last().map(|v| v.version_number);
                self.version_diff = Some(VersionDiffState {
                    scope: VersionScope::Manuscript(manuscript_id),
                    from: latest.and_then(|n| (n > 1).then_some(n - 1)),
                    to: latest,
                    to_is_current_config: false,
                    view: DiffViewState::default(),
                });
            }
            ManuscriptAction::LoadVersion {
                manuscript_id,
                version_number,
            } => self.load_manuscript_version(manuscript_id, version_number),
            ManuscriptAction::RevertPending {
                manuscript_id,
                version_number,
            } => self.revert_confirm = Some((manuscript_id, version_number)),
        }
    }

    fn apply_pdf_action(&mut self, action: PdfAction) {
        match action {
            PdfAction::Open(id) => self.open_pdf_attachment(id),
            PdfAction::SaveAs(id) => self.save_pdf_attachment(id),
            PdfAction::Delete(id) => {
                let result: anyhow::Result<()> = (|| {
                    let store = self
                        .manuscript_store
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                    store.remove_pdf(id)?;
                    Ok(())
                })();
                self.manuscript_detail_delete_pdf = None;
                self.reload_detail();
                match result {
                    Ok(()) => self.status = "已删除附件。".into(),
                    Err(error) => self.status = format!("删除附件失败：{error:#}"),
                }
            }
        }
    }

    fn transition_status(&mut self, id: i64, target: ManuscriptStatus) {
        let result: anyhow::Result<()> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.set_status(id, target)?;
            Ok(())
        })();
        self.manuscript_dirty = true;
        match result {
            Ok(()) => self.status = format!("稿件已转为{}。", target.label()),
            Err(error) => self.status = format!("状态流转失败：{error:#}"),
        }
    }

    fn refresh_detail(&mut self, id: i64) {
        let result: anyhow::Result<Option<ManuscriptRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get(id)
        })();
        match result {
            Ok(Some(record)) => {
                self.manuscript_detail = Some(record);
                self.refresh_manuscript_versions(id);
            }
            Ok(None) => {
                self.status = "稿件不存在或已被删除。".into();
                self.manuscript_detail = None;
            }
            Err(error) => {
                self.status = format!("读取稿件失败：{error:#}");
                self.manuscript_detail = None;
            }
        }
    }

    /// 载入详情时同步读取该稿件的版本历史列表。
    fn refresh_manuscript_versions(&mut self, id: i64) {
        self.manuscript_versions = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list_manuscript_versions(id).ok())
            .unwrap_or_default();
    }

    fn reload_detail(&mut self) {
        let Some(id) = self.manuscript_detail.as_ref().map(|d| d.id) else {
            return;
        };
        self.refresh_detail(id);
    }

    /// 打开稿件。已经开着就切到那个标签，否则新开一个。不自动改状态
    /// （打开查看不翻状态）。
    fn open_in_editor(&mut self, id: i64) {
        if self.focus_manuscript(id) {
            let title = self.doc().title();
            self.status = format!("已切换到已打开的《{title}》。");
            return;
        }
        let result: anyhow::Result<Option<ManuscriptRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get(id)
        })();
        match result {
            Ok(Some(record)) => {
                let title = record.title.clone();
                let record_status = record.status;
                let mut session = DraftSession::from_parts(
                    0,
                    Some(record.id),
                    record.snapshot,
                    record.content_markdown,
                );
                session.record_status = record.status;
                session.mark_saved();
                self.open_doc(session);
                self.refresh_committed_baseline(self.active_doc);
                self.status = match record_status {
                    ManuscriptStatus::Published => {
                        format!("已打开已发布稿件《{title}》，当前为只读；退回草稿后可编辑。")
                    }
                    ManuscriptStatus::Archived => {
                        format!("已打开归档稿件《{title}》，当前为只读。")
                    }
                    _ => format!("已打开稿件《{title}》，可继续编辑并保存。"),
                };
            }
            Ok(None) => self.status = "稿件不存在或已被删除。".into(),
            Err(error) => self.status = format!("载入稿件失败：{error:#}"),
        }
    }

    /// 复制现有稿件为一份尚未入库的新稿，开在新标签里。只继承可编辑内容，
    /// 不继承原记录身份、生命周期状态、版本历史和 PDF 附件；首次保存会走
    /// 新建分支，绝不覆盖来源稿。
    fn create_from_existing(&mut self, id: i64) {
        let result: anyhow::Result<Option<ManuscriptRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get(id)
        })();
        match result {
            Ok(Some(record)) => {
                let title = record.title.clone();
                self.open_doc(DraftSession::from_parts(
                    0,
                    None,
                    record.snapshot,
                    record.content_markdown,
                ));
                self.status = format!(
                    "已基于《{title}》新建公文。修改后点“保存到稿件库”将新增一条草稿，不会覆盖原稿。"
                );
            }
            Ok(None) => self.status = "来源稿件不存在或已被删除。".into(),
            Err(error) => self.status = format!("基于现有公文新建失败：{error:#}"),
        }
    }

    /// 起草页“保存到稿件库”：新建记录或更新当前打开的记录。
    fn save_to_manuscript_library(&mut self) {
        // 表单刚改过而正文未动时也要在保存点重跑函号等元数据规则。
        self.draft_page().revalidate();
        let snapshot = self.doc().draft.clone();
        let content = self.doc().generated_markdown.clone();
        let title = export::extract_title(&content, &snapshot.title_hint);
        let current_id = self.doc().manuscript_id;
        let result: anyhow::Result<String> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用，无法保存"))?;
            if let Some(id) = current_id {
                let record = store
                    .get(id)?
                    .ok_or_else(|| anyhow::anyhow!("稿件不存在或已被删除"))?;
                if record.status == ManuscriptStatus::Archived {
                    anyhow::bail!("归档稿件不可修改，请在稿件管理页查看。");
                }
                if record.status == ManuscriptStatus::Published {
                    anyhow::bail!("该稿件已发布，请先在稿件管理页“退回草稿”再修改。");
                }
                store.update(
                    id,
                    &ManuscriptUpdate {
                        snapshot,
                        content_markdown: content,
                        notes: record.notes,
                    },
                )?;
                Ok(format!("已更新稿件《{title}》。"))
            } else {
                let new_id = store.create(
                    &NewManuscript {
                        snapshot,
                        content_markdown: content,
                        notes: String::new(),
                        status: ManuscriptStatus::Draft,
                        ..Default::default()
                    },
                    None,
                )?;
                self.doc_mut().manuscript_id = Some(new_id);
                Ok(format!(
                    "已保存为草稿《{title}》，后续保存会更新同一条记录。"
                ))
            }
        })();
        self.manuscript_dirty = true;
        match result {
            Ok(message) => {
                self.doc_mut().mark_saved();
                self.refresh_committed_baseline(self.active_doc);
                self.status = message;
            }
            Err(error) => self.status = format!("保存失败：{error:#}"),
        }
    }

    /// 新开一篇空白稿件的标签。
    fn new_blank_manuscript(&mut self) {
        let session = DraftSession::blank(0, &self.config);
        self.open_doc(session);
        self.status = "已新建空白稿件：点“保存到稿件库”将新增一条草稿记录。".into();
    }

    fn export_manuscripts_zip(&mut self) {
        let Some(store) = self.manuscript_store.as_mut() else {
            self.status = "稿件库不可用，无法导出。".into();
            return;
        };
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let default_name = format!("公文稿件-{stamp}.zip");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP 稿件包", &["zip"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        let filter = self.manuscript_filter.clone();
        let result: anyhow::Result<manuscript_io::ExportSummary> =
            manuscript_io::export_zip(store, &filter, &path);
        match result {
            Ok(summary) => {
                self.status = format!(
                    "已导出 {} 篇稿件、{} 个 PDF 附件到 {}。",
                    summary.records,
                    summary.pdfs,
                    path.display()
                );
            }
            Err(error) => self.status = format!("导出失败：{error:#}"),
        }
    }

    fn export_selected_manuscripts_zip(&mut self) {
        if self.manuscript_selected.is_empty() {
            self.status = "请先勾选要导出的稿件。".into();
            return;
        }
        let Some(store) = self.manuscript_store.as_mut() else {
            self.status = "稿件库不可用，无法导出。".into();
            return;
        };
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
        let default_name = format!("所选公文稿件-{stamp}.zip");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP 稿件包", &["zip"])
            .set_file_name(&default_name)
            .save_file()
        else {
            return;
        };
        let ids = self.manuscript_selected.iter().copied().collect::<Vec<_>>();
        match manuscript_io::export_zip_selected(store, &ids, &path) {
            Ok(summary) => {
                self.status = format!(
                    "已导出所选 {} 篇稿件、{} 个 PDF 附件到 {}。",
                    summary.records,
                    summary.pdfs,
                    path.display()
                );
            }
            Err(error) => self.status = format!("导出所选稿件失败：{error:#}"),
        }
    }

    fn start_import_manuscript(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP 稿件包", &["zip"])
            .pick_file()
        else {
            return;
        };
        match manuscript_io::read_manifest(&path) {
            Ok(manifest) => {
                let selected = vec![true; manifest.records.len()];
                self.manuscript_import_preview = Some(ImportPreview {
                    manifest,
                    zip_path: path,
                    selected,
                    keyword: String::new(),
                    skip_existing: true,
                });
                self.status = "已读取稿件包，请预览后确认导入。".into();
            }
            Err(error) => self.status = format!("读取稿件包失败：{error:#}"),
        }
    }

    fn confirm_import(&mut self) {
        let Some(preview) = self.manuscript_import_preview.take() else {
            return;
        };
        let opts = manuscript_io::ImportOptions {
            skip_existing_by_id: preview.skip_existing,
            selected: preview.selected.clone(),
        };
        let result: anyhow::Result<manuscript_io::ImportSummary> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            manuscript_io::import_zip(store, &preview.zip_path, &opts)
        })();
        match result {
            Ok(summary) => {
                self.manuscript_dirty = true;
                let mut message = format!(
                    "已导入 {} 篇稿件、{} 个 PDF 附件。",
                    summary.imported, summary.pdfs_imported
                );
                if summary.skipped_existing > 0 {
                    message.push_str(&format!(
                        " 跳过与本地同源的 {} 篇。",
                        summary.skipped_existing
                    ));
                }
                if summary.skipped_pdfs > 0 {
                    message.push_str(&format!(
                        " {} 个附件缺失或过大被跳过。",
                        summary.skipped_pdfs
                    ));
                }
                self.status = message;
            }
            Err(error) => {
                self.manuscript_import_preview = Some(preview);
                self.status = format!("导入失败：{error:#}");
            }
        }
    }

    fn open_pdf_attachment(&mut self, id: i64) {
        let path = {
            let detail = self.manuscript_detail.as_ref();
            let Some(detail) = detail else { return };
            let Some(pdf) = detail.pdfs.iter().find(|p| p.id == id) else {
                return;
            };
            let path = self.temp_pdf_path(detail.id, id);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(error) = std::fs::write(&path, &pdf.bytes) {
                self.status = format!("写入临时 PDF 失败：{error}");
                return;
            }
            path
        };
        if let Err(error) = open_in_os(&path) {
            self.status = format!("打开 PDF 失败：{error}");
        }
    }

    fn save_pdf_attachment(&mut self, id: i64) {
        let Some((file_name, bytes)) = self.manuscript_detail.as_ref().and_then(|d| {
            d.pdfs
                .iter()
                .find(|p| p.id == id)
                .map(|p| (p.file_name.clone(), p.bytes.clone()))
        }) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(&file_name)
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.status = format!("已保存附件到 {}。", path.display()),
            Err(error) => self.status = format!("保存附件失败：{error}"),
        }
    }

    fn temp_pdf_path(&self, manuscript_id: i64, attachment_id: i64) -> PathBuf {
        PathBuf::from(&self.config.output_dir)
            .join("temp")
            .join(format!("manuscript_{manuscript_id}_{attachment_id}.pdf"))
    }

    fn apply_vocab_action(&mut self, action: VocabAction) {
        match action {
            VocabAction::AddUnit { parent } => {
                // 挂在折叠的上级下面时先展开，否则新节点看不见。
                if let Some(entry) = self
                    .config
                    .vocabulary
                    .iter()
                    .find(|entry| {
                        entry.category == VocabularyCategory::Unit
                            && entry.code.trim() == parent.trim()
                    })
                    .map(|entry| entry.id)
                {
                    self.vocabulary_collapsed.remove(&entry);
                }
                let id = units::next_id(&self.config.vocabulary);
                self.config.vocabulary.push(VocabularyEntry {
                    id,
                    category: VocabularyCategory::Unit,
                    canonical: unique_name(
                        &self.config.vocabulary,
                        VocabularyCategory::Unit,
                        "新单位",
                    ),
                    parent,
                    ..Default::default()
                });
                self.vocabulary_selected = Some(id);
                self.vocabulary_delete_confirm = None;
            }
            VocabAction::AddPerson { unit } => {
                if let Some(entry) = self
                    .config
                    .vocabulary
                    .iter()
                    .find(|entry| {
                        entry.category == VocabularyCategory::Unit
                            && entry.code.trim() == unit.trim()
                    })
                    .map(|entry| entry.id)
                {
                    self.vocabulary_collapsed.remove(&entry);
                }
                let id = units::next_id(&self.config.vocabulary);
                self.config.vocabulary.push(VocabularyEntry {
                    id,
                    category: VocabularyCategory::Person,
                    canonical: unique_name(
                        &self.config.vocabulary,
                        VocabularyCategory::Person,
                        "新人员",
                    ),
                    unit,
                    ..Default::default()
                });
                self.vocabulary_selected = Some(id);
                self.vocabulary_delete_confirm = None;
            }
            VocabAction::Delete(id) => {
                let Some(index) = self
                    .config
                    .vocabulary
                    .iter()
                    .position(|entry| entry.id == id)
                else {
                    return;
                };
                let doomed = if self.config.vocabulary[index].category == VocabularyCategory::Unit {
                    units::subtree_indices(&self.config.vocabulary, index)
                } else {
                    vec![index]
                };
                for position in doomed.into_iter().rev() {
                    self.config.vocabulary.remove(position);
                }
                self.vocabulary_selected = None;
                self.vocabulary_delete_confirm = None;
            }
            VocabAction::MoveUp(id) | VocabAction::MoveDown(id) => {
                let up = matches!(action, VocabAction::MoveUp(_));
                let Some(index) = self
                    .config
                    .vocabulary
                    .iter()
                    .position(|entry| entry.id == id)
                else {
                    return;
                };
                let entry = &self.config.vocabulary[index];
                // 同级 = 同一个上级下的单位，或同一个单位下的人员。
                let siblings = if entry.category == VocabularyCategory::Unit {
                    units::child_units(&self.config.vocabulary, entry.parent.trim())
                } else {
                    units::unit_people(&self.config.vocabulary, entry.unit.trim())
                };
                let Some(position) = siblings.iter().position(|value| *value == index) else {
                    return;
                };
                let target = if up {
                    position.checked_sub(1)
                } else {
                    (position + 1 < siblings.len()).then_some(position + 1)
                };
                if let Some(target) = target {
                    self.config.vocabulary.swap(index, siblings[target]);
                }
            }
            VocabAction::Clear => {
                self.config.vocabulary.clear();
                self.vocabulary_selected = None;
                self.vocabulary_collapsed.clear();
                self.vocabulary_delete_confirm = None;
                self.vocabulary_clear_confirm = false;
                self.vocabulary_import_conflicts = None;
                self.status = "当前标准词库已清空；点击“保存更改”写入本机配置。".into();
            }
        }
        units::normalize(&mut self.config.vocabulary);
    }

    /// 切换版本前的三选确认：提交为新版本后切换 / 丢弃修改并切换 / 取消。
    fn version_switch_window(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.version_switch.take() else {
            return;
        };
        let target_label = match prompt.target {
            VersionTarget::Version(number) => format!("v{number}"),
            VersionTarget::Working => "未提交内容".to_string(),
        };
        let mut commit_first = false;
        let mut discard = false;
        let mut cancel = false;
        egui::Window::new("切换版本")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("当前内容相对{}有未提交修改。", prompt.base_label));
                ui.colored_label(WARN, format!("直接切到{target_label}会丢弃这些修改。"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::GitCommit, "提交为新版本后切换")
                        .on_hover_text("先把当前修改固化为一个新版本，再切过去，什么都不丢")
                        .clicked()
                    {
                        commit_first = true;
                    }
                    if ui
                        .add(theme::warning_icon_button(
                            theme::Icon::Undo,
                            "丢弃修改并切换",
                        ))
                        .on_hover_text("丢弃当前未提交的修改，直接切到目标版本")
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if commit_first {
            self.switch_after_commit = Some(prompt.target);
            self.open_version_commit(VersionScope::Manuscript(prompt.manuscript_id));
            return;
        }
        if discard {
            self.draft_page()
                .apply_version_switch(prompt.manuscript_id, prompt.target);
            return;
        }
        if !cancel {
            self.version_switch = Some(prompt);
        }
    }

    /// 打开提交版本对话框：预填默认时间戳版本名（同名自动加序号）、空注释。
    fn open_version_commit(&mut self, scope: VersionScope) {
        let base = default_version_name();
        let name = match &scope {
            VersionScope::Manuscript(id) => {
                let names = self
                    .manuscript_store
                    .as_mut()
                    .and_then(|store| store.list_manuscript_versions(*id).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|row| row.name)
                    .collect::<Vec<_>>();
                unique_version_name(&names, &base)
            }
            VersionScope::Config => {
                let names = self
                    .manuscript_store
                    .as_mut()
                    .and_then(|store| store.list_config_versions().ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|row| row.name)
                    .collect::<Vec<_>>();
                unique_version_name(&names, &base)
            }
        };
        self.version_commit = Some(VersionCommitDraft {
            scope,
            name,
            comment: String::new(),
            error: None,
        });
    }

    /// 提交版本对话框（稿件版 / 配置版共用）。
    fn version_commit_window(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.version_commit.take() else {
            return;
        };
        // 实时预览：相对上一版本是否有变更（与名称/注释无关，先算出来避免闭包借用冲突）。
        let has_changes = match &draft.scope {
            VersionScope::Manuscript(id) => {
                let snapshot = self.doc().draft.clone();
                let content = self.doc().generated_markdown.clone();
                let notes = self
                    .manuscript_store
                    .as_ref()
                    .and_then(|store| store.notes_of(*id).ok())
                    .flatten()
                    .unwrap_or_default();
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| {
                        store
                            .manuscript_version_changed(*id, &snapshot, &content, &notes)
                            .ok()
                    })
                    .unwrap_or(true)
            }
            VersionScope::Config => {
                let config = self.config.clone();
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| store.config_version_changed(&config).ok())
                    .unwrap_or(true)
            }
        };
        let mut close = false;
        let mut submit = false;
        egui::Window::new("提交版本")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("版本名称（默认时间戳，可修改）");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.name)
                        .desired_width(380.0)
                        .hint_text("如 2026-08-07 09:35"),
                );
                ui.label("注释");
                ui.add(
                    egui::TextEdit::multiline(&mut draft.comment)
                        .desired_rows(3)
                        .desired_width(380.0),
                );
                if has_changes {
                    ui.weak("提交后固化为一个新版本，追加在版本链末尾。");
                } else {
                    ui.colored_label(WARN, "相对上一版本没有内容变更，不能提交。");
                }
                if let Some(error) = &draft.error {
                    ui.colored_label(WARN, error);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(has_changes, egui::Button::new("提交"))
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            // 取消提交时也放弃"提交后切换"，免得下次提交莫名跳版本。
            self.switch_after_commit = None;
            return; // 关闭：丢弃草稿。
        }
        if submit {
            match self.run_version_commit(&draft) {
                Ok(message) => {
                    self.doc_mut().loaded_version = None;
                    self.doc_mut().mark_saved();
                    self.refresh_committed_baseline(self.active_doc);
                    self.manuscript_dirty = true;
                    self.status = message;
                    self.doc_mut().draft_diff.view.reset();
                    // "提交为新版本后切换"：提交成功了才真正切过去。
                    if let Some(target) = self.switch_after_commit.take()
                        && let VersionScope::Manuscript(id) = &draft.scope
                    {
                        self.draft_page().apply_version_switch(*id, target);
                    }
                    // 成功：不恢复 draft，对话框关闭。
                }
                Err(error) => {
                    draft.error = Some(format!("{error:#}"));
                    self.version_commit = Some(draft);
                }
            }
        } else {
            self.version_commit = Some(draft);
        }
    }

    /// 执行提交：先同步活稿行 / 配置，再写入版本链。返回状态消息或错误。
    fn run_version_commit(&mut self, draft: &VersionCommitDraft) -> anyhow::Result<String> {
        let name = draft.name.trim();
        anyhow::ensure!(!name.is_empty(), "版本名称不能为空");
        let comment = draft.comment.trim();
        match &draft.scope {
            VersionScope::Manuscript(id) => {
                let id = *id;
                let snapshot = self.doc().draft.clone();
                let content = self.doc().generated_markdown.clone();
                let store = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                let notes = store.notes_of(id)?.context("稿件不存在，无法提交版本")?;
                store.update(
                    id,
                    &ManuscriptUpdate {
                        snapshot: snapshot.clone(),
                        content_markdown: content.clone(),
                        notes: notes.clone(),
                    },
                )?;
                let row = store
                    .commit_manuscript_version(id, name, comment, &snapshot, &content, &notes)?;
                Ok(format!(
                    "已提交版本《{}》（v{}）。",
                    row.name, row.version_number
                ))
            }
            VersionScope::Config => {
                storage::save(&self.config)?;
                let store = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
                let row = store.commit_config_version(name, comment, &self.config)?;
                Ok(format!(
                    "已提交配置版本《{}》（v{}）。",
                    row.name, row.version_number
                ))
            }
        }
    }

    /// 版本对照窗（稿件版 / 配置版共用）。
    fn version_diff_window(&mut self, ctx: &egui::Context) {
        let Some(mut diff) = self.version_diff.take() else {
            return;
        };
        let scope = diff.scope.clone();
        // 关闭按钮交给标题栏：正文对照是个撑满高度的滚动区，放在它下面的页脚
        // 会被顶出可视区，点不到。
        let mut open = true;
        egui::Window::new("版本对照")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(900.0)
            .default_height(620.0)
            // 不设上限时滚动区会把窗口撑满整个屏幕高度。
            .max_height(760.0)
            .show(ctx, |ui| match scope {
                VersionScope::Manuscript(id) => self.manuscript_diff_ui(ui, id, &mut diff),
                VersionScope::Config => self.config_diff_ui(ui, &mut diff),
            });
        if !open {
            return;
        }
        self.version_diff = Some(diff);
    }

    fn manuscript_diff_ui(
        &mut self,
        ui: &mut egui::Ui,
        manuscript_id: i64,
        diff: &mut VersionDiffState,
    ) {
        let versions = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list_manuscript_versions(manuscript_id).ok())
            .unwrap_or_default();
        if versions.is_empty() {
            ui.weak("该稿件还没有版本。到起草页点“提交版本”开始记录历史。");
            return;
        }
        let numbers: Vec<i64> = versions.iter().map(|row| row.version_number).collect();
        let latest = numbers.last().copied().unwrap_or(1);
        // 新侧兜底到最新版；旧侧兜底到它的上一版（v1 没有上一版，此时旧侧为空）。
        let to = diff
            .to
            .filter(|number| numbers.contains(number))
            .unwrap_or(latest);
        let from = diff
            .from
            .filter(|number| numbers.contains(number) && *number < to);
        let mut picked_from: Option<Option<i64>> = None;
        let mut picked_to = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("从");
            let from_label = from.map_or_else(
                || "（空白，整篇算新增）".to_string(),
                |number| version_label(&versions, number),
            );
            egui::ComboBox::from_id_salt(("vdiff_from", manuscript_id))
                .selected_text(from_label)
                .width(230.0)
                .show_ui(ui, |ui| {
                    // 只能选比新侧更早的版本：方向永远是旧→新，选不出颠倒的组合。
                    if ui
                        .selectable_label(from.is_none(), "（空白，整篇算新增）")
                        .clicked()
                    {
                        picked_from = Some(None);
                    }
                    for row in versions.iter().rev().filter(|row| row.version_number < to) {
                        if ui
                            .selectable_label(
                                from == Some(row.version_number),
                                version_label(&versions, row.version_number),
                            )
                            .on_hover_text(version_hover(row))
                            .clicked()
                        {
                            picked_from = Some(Some(row.version_number));
                        }
                    }
                });
            ui.label("到");
            egui::ComboBox::from_id_salt(("vdiff_to", manuscript_id))
                .selected_text(version_label(&versions, to))
                .width(230.0)
                .show_ui(ui, |ui| {
                    for row in versions.iter().rev() {
                        if ui
                            .selectable_label(
                                to == row.version_number,
                                version_label(&versions, row.version_number),
                            )
                            .on_hover_text(version_hover(row))
                            .clicked()
                        {
                            picked_to = Some(row.version_number);
                        }
                    }
                });
            ui.weak("左旧右新");
        });
        if let Some(number) = picked_from {
            diff.from = number;
            diff.view.reset();
        }
        if let Some(number) = picked_to {
            diff.to = Some(number);
            // 新侧往前挪时旧侧可能变得不再更早，顺手把它退回上一版。
            if diff.from.is_some_and(|old| old >= number) {
                diff.from = (number > 1).then_some(number - 1);
            }
            diff.view.reset();
        }
        if picked_from.is_some() || picked_to.is_some() {
            return; // 选择变了：下一帧按新选择重画，免得这一帧算旧的。
        }

        let old = from
            .and_then(|number| {
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| store.get_manuscript_version(manuscript_id, number).ok())
                    .flatten()
            })
            .map(diff::ContentSnapshot::from)
            .unwrap_or_else(|| {
                diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
            });
        let Some(new_record) = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.get_manuscript_version(manuscript_id, to).ok())
            .flatten()
        else {
            ui.weak("版本不存在或已被删除。");
            return;
        };
        ui.weak(format!(
            "v{}《{}》{}（{}）",
            new_record.version_number,
            new_record.name,
            if new_record.comment.is_empty() {
                ""
            } else {
                new_record.comment.as_str()
            },
            short_date(&new_record.created_at),
        ));
        ui.separator();
        let old_label = from.map_or_else(|| "（空白）".to_string(), |number| format!("v{number}"));
        let new_label = format!("v{to}");
        let report = diff::manuscript_diff(&old, &new_record.into());
        let config = DiffViewConfig {
            old_label: &old_label,
            new_label: &new_label,
            // 这里看的可能不是起草页正在编辑的那篇稿件，跳源码会落到无关位置。
            allow_jump: false,
        };
        diff_view::manuscript_diff_ui(ui, &report, &mut diff.view, &config);
    }

    fn config_diff_ui(&mut self, ui: &mut egui::Ui, diff: &mut VersionDiffState) {
        let versions = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list_config_versions().ok())
            .unwrap_or_default();
        if versions.is_empty() {
            ui.weak("还没有配置版本。在“标准词库”页点“提交配置版本”开始记录历史。");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("从");
            let from_label = diff
                .from
                .and_then(|n| versions.iter().find(|v| v.version_number == n))
                .map(|v| format!("v{} · {}", v.version_number, v.name))
                .unwrap_or_else(|| "请选择".to_string());
            egui::ComboBox::from_id_salt(("cdiff_from", 0))
                .selected_text(from_label)
                .width(240.0)
                .show_ui(ui, |ui| {
                    for v in &versions {
                        ui.selectable_value(
                            &mut diff.from,
                            Some(v.version_number),
                            format!("v{} · {} {}", v.version_number, v.name, v.comment),
                        );
                    }
                });
            ui.checkbox(&mut diff.to_is_current_config, "到当前配置");
            if !diff.to_is_current_config {
                ui.label("到");
                let to_label = diff
                    .to
                    .and_then(|n| versions.iter().find(|v| v.version_number == n))
                    .map(|v| format!("v{} · {}", v.version_number, v.name))
                    .unwrap_or_else(|| "请选择".to_string());
                egui::ComboBox::from_id_salt(("cdiff_to", 0))
                    .selected_text(to_label)
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for v in &versions {
                            ui.selectable_value(
                                &mut diff.to,
                                Some(v.version_number),
                                format!("v{} · {} {}", v.version_number, v.name, v.comment),
                            );
                        }
                    });
            }
        });
        let from_num = diff
            .from
            .filter(|n| versions.iter().any(|v| v.version_number == *n));
        let to_num = if diff.to_is_current_config {
            None
        } else {
            diff.to
                .filter(|n| versions.iter().any(|v| v.version_number == *n))
        };
        let old = from_num.and_then(|n| {
            self.manuscript_store
                .as_mut()
                .and_then(|store| store.get_config_version(n).ok())
                .flatten()
        });
        let new = if diff.to_is_current_config {
            Some(self.config.clone())
        } else {
            to_num.and_then(|n| {
                self.manuscript_store
                    .as_mut()
                    .and_then(|store| store.get_config_version(n).ok())
                    .flatten()
            })
        };
        let (Some(a), Some(b)) = (old, new) else {
            ui.weak("请选择两个版本进行对照。");
            return;
        };
        let old_label = from_num.map_or_else(|| "变更前".to_string(), |n| format!("v{n}"));
        let new_label = if diff.to_is_current_config {
            "当前配置".to_string()
        } else {
            to_num.map_or_else(|| "变更后".to_string(), |n| format!("v{n}"))
        };
        let report = diff::config_changes(&a, &b);
        ui.separator();
        ui.strong("词库变更");
        if report.vocabulary.is_empty() {
            ui.weak("词库无变化。");
        } else {
            for change in &report.vocabulary {
                let color = match change.action {
                    "新增" => theme::SUCCESS,
                    "删除" => theme::WARN,
                    _ => theme::ACCENT,
                };
                ui.horizontal(|ui| {
                    ui.colored_label(
                        color,
                        format!("{}·{}", change.category.label(), change.action),
                    );
                    ui.label(&change.label);
                });
                for field in &change.changes {
                    ui.horizontal(|ui| {
                        ui.add_space(18.0);
                        ui.label(field.label);
                        ui.weak(if field.before.is_empty() {
                            "—".to_string()
                        } else {
                            field.before.clone()
                        });
                        ui.label("→");
                        ui.colored_label(
                            ACCENT,
                            if field.after.is_empty() {
                                "—".to_string()
                            } else {
                                field.after.clone()
                            },
                        );
                    });
                }
            }
        }
        ui.separator();
        ui.strong("版式变更");
        if report.profiles.is_empty() {
            ui.weak("版式无变化。");
        } else {
            for kind in &report.profiles {
                ui.strong(kind.kind.label());
                diff_view::field_changes_table(ui, &kind.changes, &old_label, &new_label);
            }
        }
        ui.separator();
        ui.strong("设置变更");
        if report.settings.is_empty() {
            ui.weak("设置无变化。");
        } else {
            diff_view::field_changes_table(ui, &report.settings, &old_label, &new_label);
        }
    }

    /// 配置版本历史窗：列表 + 应用（二次确认）+ 对照。
    fn config_versions_window(&mut self, ctx: &egui::Context) {
        if !self.config_versions_open {
            return;
        }
        let versions = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list_config_versions().ok())
            .unwrap_or_default();
        let mut close = false;
        let mut open_diff: Option<i64> = None;
        egui::Window::new("配置版本历史")
            .collapsible(false)
            .resizable(true)
            .default_width(700.0)
            .default_height(460.0)
            .show(ctx, |ui| {
                if versions.is_empty() {
                    ui.weak("还没有配置版本。修改词库或设置后点“提交配置版本”开始记录历史。");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("config_versions_list")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for v in &versions {
                                ui.horizontal(|ui| {
                                    ui.label(format!("v{}", v.version_number));
                                    ui.label(&v.name);
                                    if !v.comment.is_empty() {
                                        ui.weak(&v.comment);
                                    }
                                    ui.weak(short_date(&v.created_at));
                                    if v.is_latest {
                                        ui.weak("最新");
                                    }
                                    if ui
                                        .add(theme::icon_text_button(
                                            theme::Icon::RotateCcw,
                                            "应用",
                                        ))
                                        .on_hover_text("用该版本替换当前配置（词库、版式、设置）")
                                        .clicked()
                                    {
                                        self.config_apply_confirm = Some(v.version_number);
                                    }
                                    if theme::icon_button(ui, theme::Icon::Compare, "对照版本")
                                        .clicked()
                                    {
                                        open_diff = Some(v.version_number);
                                    }
                                });
                            }
                        });
                }
                if let Some(n) = self.config_apply_confirm {
                    ui.add_space(6.0);
                    ui.group(|ui| {
                        ui.colored_label(
                            WARN,
                            format!("将用配置版本 v{n} 替换当前配置，未保存的词库更改会丢失。"),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("确认应用").color(WARN),
                                ))
                                .clicked()
                            {
                                match self.apply_config_version(n) {
                                    Ok(message) => self.status = message,
                                    Err(error) => {
                                        self.status = format!("应用配置版本失败：{error:#}")
                                    }
                                }
                            }
                            if ui.button("取消").clicked() {
                                self.config_apply_confirm = None;
                            }
                        });
                    });
                }
                if theme::icon_button(ui, theme::Icon::X, "关闭窗口").clicked() {
                    close = true;
                }
            });
        if let Some(n) = open_diff {
            self.version_diff = Some(VersionDiffState {
                scope: VersionScope::Config,
                // 配置也按"从旧到新"：v1 没有上一版时跟当前配置比。
                from: (n > 1).then_some(n - 1).or(Some(n)),
                to: Some(n),
                to_is_current_config: n <= 1,
                view: DiffViewState::default(),
            });
        }
        if close {
            self.config_versions_open = false;
            self.config_apply_confirm = None;
        }
    }

    /// 应用配置版本：覆盖内存配置、整理词库、写回 config.json。
    fn apply_config_version(&mut self, version_number: i64) -> anyhow::Result<String> {
        let store = self
            .manuscript_store
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
        let config = store
            .get_config_version(version_number)?
            .context("配置版本不存在")?;
        self.config = config;
        units::normalize(&mut self.config.vocabulary);
        storage::save(&self.config)?;
        self.config_apply_confirm = None;
        Ok(format!("已应用配置版本 v{version_number}。"))
    }

    /// 回退到某版本：用该版内容覆盖活稿行，并载入起草页。与"载入编辑"的差别就在
    /// 这一步写库——载入只是看看，回退是把稿件库里的当前稿改回去。版本链不动，
    /// 之后提交仍是追加新版本。
    fn revert_to_version(&mut self, manuscript_id: i64, version_number: i64) {
        let result: anyhow::Result<VersionRecord> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            let record = store
                .get_manuscript_version(manuscript_id, version_number)?
                .context("版本不存在或已被删除")?;
            let notes = store
                .notes_of(manuscript_id)?
                .context("稿件不存在，无法回退")?;
            store.update(
                manuscript_id,
                &ManuscriptUpdate {
                    snapshot: record.snapshot.clone(),
                    content_markdown: record.content_markdown.clone(),
                    notes,
                },
            )?;
            Ok(record)
        })();
        self.revert_confirm = None;
        match result {
            Ok(record) => {
                if !self.focus_manuscript(manuscript_id) {
                    self.open_doc(DraftSession::from_parts(
                        0,
                        Some(manuscript_id),
                        record.snapshot.clone(),
                        record.content_markdown.clone(),
                    ));
                }
                self.doc_mut().draft = record.snapshot;
                self.doc_mut().generated_markdown = record.content_markdown;
                self.doc_mut().manuscript_id = Some(manuscript_id);
                // 内容已经写回活稿行，就是"当前未提交内容"，不再挂历史版本横幅。
                self.doc_mut().loaded_version = None;
                self.doc_mut().reset_transient();
                self.doc_mut().mark_saved();
                self.refresh_committed_baseline(self.active_doc);
                self.manuscript_detail = None;
                self.manuscript_dirty = true;
                self.doc_mut().draft_diff.view.reset();
                self.draft_page().revalidate();
                let next = self
                    .manuscript_store
                    .as_mut()
                    .and_then(|store| store.list_manuscript_versions(manuscript_id).ok())
                    .and_then(|rows| rows.last().map(|row| row.version_number + 1))
                    .unwrap_or(1);
                self.status =
                    format!("已回退到 v{version_number} 的内容；继续修改后提交将追加为 v{next}。");
            }
            Err(error) => self.status = format!("回退失败：{error:#}"),
        }
    }

    /// "回退到该版本"的二次确认：会覆盖活稿行里未提交的内容，值得问一句。
    fn revert_confirm_window(&mut self, ctx: &egui::Context) {
        let Some((manuscript_id, version_number)) = self.revert_confirm else {
            return;
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("回退到该版本")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!(
                    "将用 v{version_number} 的内容覆盖这篇稿件的当前内容。"
                ));
                ui.colored_label(WARN, "当前未提交的修改会丢失；已提交的版本不受影响。");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::warning_icon_button(
                            theme::Icon::RotateCcw,
                            "确认回退",
                        ))
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm {
            self.revert_to_version(manuscript_id, version_number);
        } else if cancel {
            self.revert_confirm = None;
        }
    }

    /// 把某版本载入起草页继续编辑（不改版本链、不改活稿行）。
    fn load_manuscript_version(&mut self, manuscript_id: i64, version_number: i64) {
        let result: anyhow::Result<Option<VersionRecord>> = (|| {
            let store = self
                .manuscript_store
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))?;
            store.get_manuscript_version(manuscript_id, version_number)
        })();
        match result {
            Ok(Some(record)) => {
                let name = record.name.clone();
                // 从详情页载入旧版时这篇未必开着；先把它的标签找出来或开出来，
                // 免得把版本内容写到别人的稿子上。
                if !self.focus_manuscript(manuscript_id) {
                    self.open_doc(DraftSession::from_parts(
                        0,
                        Some(manuscript_id),
                        record.snapshot.clone(),
                        record.content_markdown.clone(),
                    ));
                }
                self.doc_mut().draft = record.snapshot;
                self.doc_mut().generated_markdown = record.content_markdown;
                self.doc_mut().manuscript_id = Some(manuscript_id);
                self.doc_mut().loaded_version = Some(LoadedVersion {
                    manuscript_id,
                    version_number,
                    name,
                });
                self.doc_mut().reset_transient();
                self.manuscript_detail = None;
                self.draft_page().revalidate();
                self.status =
                    format!("已载入版本 v{version_number}，可在起草页继续修改后提交为新版本。");
            }
            Ok(None) => self.status = "版本不存在或已被删除。".into(),
            Err(error) => self.status = format!("载入版本失败：{error:#}"),
        }
    }

    /// 字体设置：界面字体 + 编译字体（五个位置各自可以换成本机字体）。
    fn font_settings_ui(&mut self, ui: &mut egui::Ui) {
        // 界面字体与编译字体共用本机字体列表，首次进入设置页就扫盘；结果为空也
        // 只扫一次，需要时点“重新扫描本机字体”。
        if !self.system_fonts_scanned && !self.system_fonts_busy {
            self.start_system_font_scan();
        }
        let before = self.config.fonts.clone();

        // 界面字体不受“使用本机字体编译”开关控制，选了就生效，随时可以换回内置。
        ui.heading("界面字体");
        ui.label(
            "应用窗口、菜单与列表使用的字体。默认随应用内置霞鹜文楷（LXGW Bright）；\
             所选字体文件缺失或读取失败时自动退回内置。",
        );
        ui.add_space(4.0);
        let mut message = {
            let filter = self.font_filter.entry("ui").or_default();
            font_choice_row(
                ui,
                "ui",
                "界面字体",
                "霞鹜文楷（LXGW Bright）",
                "应用窗口、菜单与列表使用的字体；不影响公文预览与导出的字体",
                &mut self.config.fonts.ui_font,
                &self.system_fonts,
                filter,
            )
        };
        ui.horizontal(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
            if ui
                .add_enabled(
                    !self.system_fonts_busy,
                    theme::icon_text_button(theme::Icon::Refresh, "重新扫描本机字体"),
                )
                .clicked()
            {
                self.start_system_font_scan();
            }
            if self.system_fonts_busy {
                ui.weak("正在扫描…");
            } else {
                ui.weak(format!("已收录 {} 个字体", self.system_fonts.len()));
            }
        });
        ui.add_space(12.0);

        ui.heading("编译字体");
        ui.label(
            "默认使用随应用分发的内置字体：标题方正小标宋、一级标题黑体、二级标题楷体、正文仿宋、页码宋体。\
             改用本机字体后，内置 Tectonic 按文件加载所选字体，导出的 TeX 拿到别的机器上编译时按字体名加载。",
        );
        ui.add_space(4.0);
        ui.checkbox(&mut self.config.fonts.use_system_fonts, "使用本机字体编译")
            .on_hover_text("不勾选时下面的选择仍然保留，只是不生效，方便和内置版式来回对照");

        if self.config.fonts.use_system_fonts {
            ui.add_space(4.0);
            for role in FontRole::ALL {
                let filter = self.font_filter.entry(role.key()).or_default();
                if let Some(text) = font_choice_row(
                    ui,
                    role.key(),
                    role.label(),
                    role.bundled_label(),
                    role.hint(),
                    self.config.fonts.choice_mut(role),
                    &self.system_fonts,
                    filter,
                ) {
                    message = Some(text);
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                ui.weak(
                    "只列出 ttf 与 otf。字体集合（ttc，例如 simsun.ttc）一个文件里装着多个字面，\
                     按文件加载必须额外指定序号，内置 Tectonic 上没有验证过，因此不在可选范围内。",
                );
            });
        }

        if let Some(text) = message {
            self.status = text;
        }
        if self.config.fonts != before {
            // 预览也跟着换，否则屏幕上的版式和编译出来的 PDF 对不上。
            theme::configure_fonts(ui.ctx(), &self.config.fonts);
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading("LM Studio 设置");
                ui.label("应用调用本机 OpenAI 兼容接口，默认地址为 http://127.0.0.1:1234/v1。正文不会主动发送到互联网。");
                ui.add_space(8.0);
                field(
                    ui,
                    "接口地址",
                    &mut self.config.lm_studio.base_url,
                    "包含 /v1",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("模型"));
                    if self.models.is_empty() {
                        ui.text_edit_singleline(&mut self.config.lm_studio.model);
                    } else {
                        egui::ComboBox::from_id_salt("model_selector")
                            .selected_text(if self.config.lm_studio.model.is_empty() {
                                "请选择模型"
                            } else {
                                &self.config.lm_studio.model
                            })
                            .width(420.0)
                            .show_ui(ui, |ui| {
                                for model in &self.models {
                                    ui.selectable_value(
                                        &mut self.config.lm_studio.model,
                                        model.clone(),
                                        model,
                                    );
                                }
                            });
                    }
                    if ui
                        .add_enabled(
                            !self.busy,
                            theme::icon_text_button(
                                theme::Icon::PlugZap,
                                "测试连接 / 刷新模型",
                            ),
                        )
                        .clicked()
                    {
                        self.start_model_probe();
                    }
                });
                field(
                    ui,
                    "API Key",
                    &mut self.config.lm_studio.api_key,
                    "本地服务通常可留空",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("温度"));
                    ui.add(
                        egui::Slider::new(&mut self.config.lm_studio.temperature, 0.0..=1.2)
                            .step_by(0.05),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("最大输出 Token"));
                    ui.add(
                        egui::DragValue::new(&mut self.config.lm_studio.max_tokens)
                            .range(256..=32768),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("超时（秒）"));
                    ui.add(
                        egui::DragValue::new(&mut self.config.lm_studio.timeout_seconds)
                            .range(5..=1800),
                    );
                });

                ui.add_space(12.0);
                ui.separator();
                ui.heading("知识库（检索增强起草）");
                ui.label("用 LM Studio 的 embedding 与 rerank 模型检索历史公文，起草时调出相似稿件作参考。两个模型与上面的对话模型相互独立。");
                ui.add_space(4.0);
                ui.checkbox(&mut self.config.rag.enabled, "启用知识库检索增强")
                    .on_hover_text("关闭后，起草页的“参考知识库”开关不生效");
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Embedding 模型").strong());
                field(
                    ui,
                    "接口地址",
                    &mut self.config.rag.embedding.base_url,
                    "包含 /v1",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("模型"));
                    if self.embedding_models.is_empty() {
                        ui.text_edit_singleline(&mut self.config.rag.embedding.model)
                            .on_hover_text("可手填模型名，或点右侧按钮从服务读取");
                    } else {
                        egui::ComboBox::from_id_salt("embedding_model_selector")
                            .selected_text(if self.config.rag.embedding.model.is_empty() {
                                "请选择模型"
                            } else {
                                &self.config.rag.embedding.model
                            })
                            .width(360.0)
                            .show_ui(ui, |ui| {
                                for model in &self.embedding_models {
                                    ui.selectable_value(
                                        &mut self.config.rag.embedding.model,
                                        model.clone(),
                                        model,
                                    );
                                }
                            });
                    }
                    if ui
                        .add_enabled(
                            !self.embedding_probe_busy,
                            theme::icon_text_button(theme::Icon::PlugZap, "测试连接 / 刷新模型"),
                        )
                        .clicked()
                    {
                        self.start_embedding_probe();
                    }
                });
                field(
                    ui,
                    "API Key",
                    &mut self.config.rag.embedding.api_key,
                    "本地服务通常可留空",
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("重排（可选，用于精排检索结果）").strong());
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("重排方式"));
                    egui::ComboBox::from_id_salt("rerank_mode_selector")
                        .selected_text(self.config.rag.rerank.mode.label())
                        .width(360.0)
                        .show_ui(ui, |ui| {
                            for mode in RerankMode::ALL {
                                ui.selectable_value(
                                    &mut self.config.rag.rerank.mode,
                                    mode,
                                    mode.label(),
                                );
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                    ui.weak(match self.config.rag.rerank.mode {
                        RerankMode::None => "直接按混合召回的融合分取前 N 条。够用，只是排序不如重排精准。",
                        RerankMode::Api => "需要能提供 rerank 接口的服务（Jina / Cohere / TEI / Infinity / llama.cpp 等）。注意：LM Studio 目前不提供该接口。",
                        RerankMode::Llm => "复用上面的对话模型给候选片段打分，不必另起服务。代价是每次检索多一次模型调用（低温短输出，通常几秒）。",
                    });
                });
                if self.config.rag.rerank.mode == RerankMode::Api {
                    field(
                        ui,
                        "接口地址",
                        &mut self.config.rag.rerank.base_url,
                        "包含 /v1",
                    );
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("端点路径"));
                        ui.text_edit_singleline(&mut self.config.rag.rerank.path)
                            .on_hover_text("拼在接口地址后，默认 rerank；不同服务路径可能不同");
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("模型"));
                        if self.rerank_models.is_empty() {
                            ui.text_edit_singleline(&mut self.config.rag.rerank.model)
                                .on_hover_text("留空则跳过重排；可手填或点右侧按钮从服务读取");
                        } else {
                            egui::ComboBox::from_id_salt("rerank_model_selector")
                                .selected_text(if self.config.rag.rerank.model.is_empty() {
                                    "请选择（留空跳过重排）"
                                } else {
                                    &self.config.rag.rerank.model
                                })
                                .width(360.0)
                                .show_ui(ui, |ui| {
                                    // 允许清空：rerank 可选。
                                    if ui.selectable_label(self.config.rag.rerank.model.is_empty(), "（不使用）").clicked() {
                                        self.config.rag.rerank.model = String::new();
                                    }
                                    for model in &self.rerank_models {
                                        ui.selectable_value(
                                            &mut self.config.rag.rerank.model,
                                            model.clone(),
                                            model,
                                        );
                                    }
                                });
                        }
                        if ui
                            .add_enabled(
                                !self.rerank_probe_busy,
                                theme::icon_text_button(theme::Icon::PlugZap, "测试连接 / 刷新模型"),
                            )
                            .clicked()
                        {
                            self.start_rerank_probe();
                        }
                    });
                }
                if self.config.rag.rerank.mode == RerankMode::Api {
                    field(
                        ui,
                        "API Key",
                        &mut self.config.rag.rerank.api_key,
                        "本地服务通常可留空",
                    );
                }
                if self.config.rag.rerank.mode != RerankMode::None {
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                        if ui
                            .add_enabled(
                                !self.rerank_probe_busy,
                                theme::icon_text_button(theme::Icon::PlugZap, "验证重排是否真的生效"),
                            )
                            .on_hover_text(
                                "真跑一次重排。只测“连接”是不够的：服务遇到不认识的端点路径\n\
                                 可能照样返回 200，看着像连上了，实际每次重排都在静默失败。",
                            )
                            .clicked()
                        {
                            self.start_rerank_verify();
                        }
                    });
                    if let Some((ok, message)) = self.rerank_verify_result.clone() {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                            ui.colored_label(if ok { theme::ACCENT } else { WARN }, message);
                        });
                    }
                }
                if self.config.rag.rerank.mode == RerankMode::Api {
                    ui.weak("rerank 响应字段名等进阶项可在 config.json 的 rag.rerank 节调整，适配不同服务。");
                }

                ui.add_space(12.0);
                ui.separator();
                ui.heading("输出与录入");
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("输出目录"));
                    ui.text_edit_singleline(&mut self.config.output_dir);
                    if theme::icon_button(ui, theme::Icon::Folder, "选择输出目录").clicked()
                        && let Some(path) = rfd::FileDialog::new().pick_folder()
                    {
                        self.config.output_dir = path.display().to_string();
                    }
                    if theme::icon_button(ui, theme::Icon::Open, "打开输出目录").clicked() {
                        self.draft_page().open_output_dir();
                    }
                });
                ui.checkbox(
                    &mut self.config.allow_free_text,
                    "允许在标准词库之外手工填写单位、联系人等字段",
                )
                .on_hover_text("取消勾选后，起草页这些字段只能从词库中选，杜绝临时手写造成的名称错误");
                ui.checkbox(
                    &mut self.config.show_editor_line_numbers,
                    "Markdown 源码与实时排版模式显示行号",
                )
                .on_hover_text("行号只用于定位，不会写入稿件或导出文件");
                ui.weak(
                    "导出 TeX 时会自动检测 XeLaTeX 或 Tectonic；检测到后编译 PDF 并清理中间文件。",
                );

                ui.add_space(12.0);
                ui.separator();
                self.font_settings_ui(ui);

                ui.add_space(12.0);
                ui.separator();
                ui.heading("导出格式");
                ui.label("这里的选择对所有稿件生效；起草页的“导出”按钮按这里勾选的格式产出。");
                // Word 导出尚未达到当前 LaTeX 链路的成熟度，入口保留但暂不允许启用。
                self.config.export.docx = false;
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("格式"));
                    ui.checkbox(&mut self.config.export.markdown, "Markdown");
                    ui.add_enabled(false, egui::Checkbox::new(&mut self.config.export.docx, "Word"))
                        .on_disabled_hover_text("Word 导出仍在完善，当前请使用 LaTeX/PDF");
                    ui.checkbox(&mut self.config.export.tex, "LaTeX");
                });
                if !self.config.export.any() {
                    ui.colored_label(WARN, "未勾选任何导出格式，起草页的导出按钮不会产生文件。");
                }
                ui.checkbox(&mut self.config.auto_export, "AI 起草或优化完成后自动导出一次")
                    .on_hover_text("不勾选则只出稿，什么时候导出完全由你决定");
                ui.checkbox(&mut self.config.export.overwrite, "覆盖同名文件")
                    .on_hover_text("不勾选时每次导出都会生成“标题-2、标题-3”这样的新文件");

                ui.add_space(12.0);
                ui.separator();
                ui.heading("保存与现场");
                ui.checkbox(&mut self.config.auto_save, "自动保存到稿件库")
                    .on_hover_text(
                        "每 2 分钟以及切换标签、关闭窗口前，把改动静默写回稿件库。
自动保存不会提交版本——版本链什么时候留痕，始终由你决定。",
                    );
                ui.weak("新建的稿件在第一次真正改动时自动入库；下次启动会恢复本次打开的标签。");

                ui.add_space(12.0);
                ui.separator();
                ui.heading("密级与保密期限规则");
                ui.label(
                    "默认取自《保守国家秘密法》第十五条：绝密级不超过三十年、机密级不超过二十年、秘密级不超过十年。本单位口径不同的，直接改下面三个上限。",
                );
                egui::Grid::new("security_rules_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("秘密级上限（年）");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.config.security_rules.secret_max_years,
                            )
                            .range(1..=100),
                        );
                        ui.end_row();
                        ui.label("机密级上限（年）");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.config.security_rules.confidential_max_years,
                            )
                            .range(1..=100),
                        );
                        ui.end_row();
                        ui.label("绝密级上限（年）");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.config.security_rules.top_secret_max_years,
                            )
                            .range(1..=100),
                        );
                        ui.end_row();
                    });
                ui.checkbox(
                    &mut self.config.security_rules.allow_long_term,
                    "期限无法确定时允许标注“长期”",
                );

                ui.add_space(12.0);
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存设置").clicked() {
                    self.persist();
                }
                ui.separator();
                ui.heading("建议流程");
                ui.label(
                    "1. 在 LM Studio 加载中文指令模型并启动 Local Server。\n2. 刷新模型并选择模型。\n3. 在“标准词库”维护全称、常见错写和联系人电话。\n4. 为每类模板保存默认单位、联系人和呈报领导。\n5. 生成草稿 → 在右侧改稿 → 处理审校提示 → 导出签发稿。",
                );
                ui.add_space(8.0);
            });
    }
}

/// 切换公文模板时只替换该模板的版式配置，正文仍是同一份可编辑稿件。
/// 新模板可能采用不同的校验规则，因此同时返回针对现有正文重新计算的提示。
pub(crate) fn switch_template_profile(
    config: &mut AppConfig,
    draft: &mut DraftInput,
    markdown: &str,
    old_kind: TemplateKind,
) -> Vec<String> {
    let mut previous_profile = draft.profile.clone();
    previous_profile.kind = old_kind;
    config.upsert_profile(previous_profile);
    draft.profile = config.profile(draft.kind);
    validator::validate(draft, markdown, &config.vocabulary, &config.security_rules)
}

impl eframe::App for GongwenApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_shortcuts(&ctx);
        self.poll_worker(&ctx);
        egui::Panel::top("app_top")
            .frame(theme::panel(theme::SURFACE, 12))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("app_status")
            .frame(theme::panel(theme::SURFACE, 8))
            .show(ui, |ui| self.status_bar(ui));
        // 标签全关光了就补一格稿件管理，主区不能空着。
        if self.tabs.is_empty() {
            self.open_page(NavPage::Manuscript);
        }
        let active = self.tabs[self.active_tab.min(self.tabs.len() - 1)];
        egui::CentralPanel::default().show(ui, |ui| match active {
            TabRef::Doc(_) => self.draft_page().create_ui(ui),
            TabRef::Page(NavPage::Vocabulary) => self.vocabulary_ui(ui),
            TabRef::Page(NavPage::Manuscript) => self.manuscript_ui(ui),
            TabRef::Page(NavPage::AiPrompts) => self.ai_prompts_ui(ui),
            TabRef::Page(NavPage::Knowledge) => crate::knowledge_ui::knowledge_ui(self, ui),
            TabRef::Page(NavPage::Settings) => self.settings_ui(ui),
        });
        // 起草页在借出会话的那一帧里做不了的事，到这里统一执行。
        self.apply_draft_actions();
        self.auto_create_touched_doc();
        self.autosave_tick();
        self.close_confirm_window(&ctx);
        self.handle_close_request(&ctx);
        self.exit_prompt_window(&ctx);
        // 版本提交、切换确认、回退确认、版本对照窗与配置版本历史窗都是全局浮窗，
        // 任何标签页都渲染。
        self.ai_prompt_picker_window(&ctx);
        self.version_commit_window(&ctx);
        self.version_switch_window(&ctx);
        self.revert_confirm_window(&ctx);
        self.version_diff_window(&ctx);
        self.config_versions_window(&ctx);
        self.knowledge_preview_window(&ctx);
        self.about_window(&ctx);
        if self.any_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::CANVAS.to_normalized_gamma_f32()
    }
}

/// 底部状态栏的紧凑图标按钮：尺寸、内边距都按 Windows 状态栏风格收小。
fn status_icon_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: theme::Icon,
    label: &str,
    tint: Option<egui::Color32>,
) -> egui::Response {
    let image = match tint {
        Some(color) => icon.image().tint(color),
        None => icon.image(),
    };
    ui.add(
        egui::Button::image(image)
            .image_tint_follows_text_color(tint.is_none())
            .selected(selected)
            .frame_when_inactive(selected)
            .min_size(egui::vec2(24.0, 20.0))
            .corner_radius(egui::CornerRadius::same(4)),
    )
    .on_hover_text(label)
}

pub(crate) fn row_label(ui: &mut egui::Ui, label: &str) {
    form_row_label(ui, label);
}

/// 说明直接附在字段名称上，悬停文字即可查看，不额外占用图标宽度。
pub(crate) fn row_label_with_info(ui: &mut egui::Ui, label: &str, tip: impl Into<String>) {
    form_row_label(ui, label).on_hover_text(tip.into());
}

/// 章节说明同样挂在标题文字本身。
pub(crate) fn section_heading_with_info(ui: &mut egui::Ui, heading: &str, tip: impl Into<String>) {
    ui.heading(heading).on_hover_text(tip.into());
}

pub(crate) fn form_row_label(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let height = if label.contains('\n') {
        FORM_CONTROL_HEIGHT + 14.0
    } else {
        FORM_CONTROL_HEIGHT
    };
    ui.add_sized(
        [LABEL_WIDTH, height],
        egui::Label::new(label).wrap_mode(egui::TextWrapMode::Wrap),
    )
}

/// 表格单元格默认不换行，长说明必须给定宽度并显式开启换行，
/// 否则会把整个可调整面板顶宽。
pub(crate) fn wrapped_hint(ui: &mut egui::Ui, text: impl Into<String>, width: f32) {
    ui.add_sized(
        [sane_width(width), 18.0],
        egui::Label::new(egui::RichText::new(text.into()).weak())
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
}

/// `available_width()` 在部分容器里会是无穷大，直接交给 `add_sized` 会让布局失效。
pub(crate) fn sane_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(80.0, 2000.0)
    } else {
        *CONTENT_WIDTH.end()
    }
}

fn present_or_dash(value: &str) -> &str {
    if value.trim().is_empty() {
        "—"
    } else {
        value
    }
}

fn joined_metadata(values: &[&str]) -> String {
    let joined = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if joined.is_empty() {
        "—".into()
    } else {
        joined
    }
}

fn metadata_grid_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).color(theme::TEXT_MUTED));
    ui.add(egui::Label::new(present_or_dash(value)).wrap_mode(egui::TextWrapMode::Wrap));
    ui.end_row();
}

/// 稿件状态的徽标颜色：新建（仅历史遗留）灰、草稿蓝、发布绿、归档橙。
pub(crate) fn status_color(status: ManuscriptStatus) -> egui::Color32 {
    match status {
        ManuscriptStatus::New => theme::TEXT_MUTED,
        ManuscriptStatus::Draft => theme::ACCENT,
        ManuscriptStatus::Published => theme::SUCCESS,
        ManuscriptStatus::Archived => theme::INFO,
    }
}

/// 稿件列表密级颜色：由蓝到红逐级增强；未标密使用弱化文字。
pub(crate) fn security_level_color(level: SecurityLevel) -> egui::Color32 {
    match level {
        SecurityLevel::Unmarked => theme::TEXT_MUTED,
        SecurityLevel::Internal => theme::INFO,
        SecurityLevel::Secret => theme::WARN,
        SecurityLevel::Confidential => theme::ACCENT_ACTIVE,
        SecurityLevel::TopSecret => theme::DANGER,
    }
}

pub(crate) fn security_level_list_label(level: SecurityLevel) -> &'static str {
    match level {
        SecurityLevel::Unmarked => "—",
        _ => level.marking(),
    }
}

/// 时间显示：中文成文日期原样返回；RFC3339/ISO 日期只取前 10 位（YYYY-MM-DD）。
pub(crate) fn short_date(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "—".to_string();
    }
    if value.contains('年') {
        return value.to_string();
    }
    value.chars().take(10).collect()
}

/// 截断字符串并追加省略号，避免把表格/预览行撑得过宽。
/// 中部省略。公文标题两头都有信息量——开头是事由、结尾是文种，
/// 掐掉尾巴会让"…的函"和"…的通知"看起来一模一样。
pub(crate) fn truncate_middle(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head = max.div_ceil(2) - 1;
    let tail = max - head - 1;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// 多行文本的单行摘要：先把换行和连续空白压成单个空格，再截断。
/// 提示词指令通常带手写换行，直接 `truncate` 会把列表撑成几行高。
pub(crate) fn summarize(text: &str, max: usize) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flattened, max)
}

/// 版本名默认值：时间戳精确到分。
pub(crate) fn default_version_name() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// 给默认版本名去重：同名时追加 `-2`、`-3`……
pub(crate) fn unique_version_name(existing: &[String], base: &str) -> String {
    if !existing.iter().any(|name| name == base) {
        return base.to_string();
    }
    let mut index = 2;
    loop {
        let candidate = format!("{base}-{index}");
        if !existing.iter().any(|name| name == &candidate) {
            return candidate;
        }
        index += 1;
    }
}

/// 版本下拉里的一行悬停详情。
pub(crate) fn version_hover(row: &manuscript::VersionRow) -> String {
    let mut out = format!("v{} · {}\n", row.version_number, row.name);
    if !row.comment.is_empty() {
        out.push_str(&format!("注释：{}\n", row.comment));
    }
    out.push_str(&format!(
        "该版标题：{}\n文号：{}\n成文日期：{}\n提交于 {}",
        row.title,
        if row.doc_number.is_empty() {
            "—"
        } else {
            &row.doc_number
        },
        if row.doc_date.is_empty() {
            "—"
        } else {
            &row.doc_date
        },
        short_date(&row.created_at),
    ));
    out
}

/// 版本下拉的选中文案。
pub(crate) fn version_label(rows: &[manuscript::VersionRow], number: i64) -> String {
    rows.iter()
        .find(|row| row.version_number == number)
        .map_or_else(
            || format!("v{number}"),
            |row| {
                format!(
                    "v{} · {}{}",
                    row.version_number,
                    row.name,
                    if row.is_latest { " · 最新" } else { "" }
                )
            },
        )
}

pub(crate) fn vocabulary_matches(entry: &VocabularyEntry, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    format!(
        "{} {} {} {} {} {} {} {} {} {} {} {}",
        entry.code,
        entry.canonical,
        entry.external_name,
        entry.abbr,
        entry.department_code,
        entry.parent,
        entry.unit,
        entry.aliases.join(" "),
        entry.note,
        entry.phone,
        entry.position,
        if entry.seal_on_behalf { "代章" } else { "" }
    )
    .to_lowercase()
    .contains(filter)
}

/// 新建词条时给一个便于立即编辑的占位名称；正式关联使用单位编码，同名是允许的。
pub(crate) fn unique_name(
    vocab: &[VocabularyEntry],
    category: VocabularyCategory,
    base: &str,
) -> String {
    let taken = |candidate: &str| {
        vocab
            .iter()
            .any(|entry| entry.category == category && entry.canonical.trim() == candidate)
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|suffix| format!("{base}{suffix}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_string())
}

/// 标准词库树在界面上的缩进深度。单位以编码关联上级，人员以所属单位编码挂靠，
/// 因此名称是否重复不会影响任意层级的显示。
pub(crate) fn vocabulary_depths(vocab: &[VocabularyEntry]) -> Vec<usize> {
    let mut depth_by_unit: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    vocab
        .iter()
        .map(|entry| match entry.category {
            VocabularyCategory::Unit => {
                let parent = entry.parent.trim();
                let depth = if parent.is_empty() {
                    0
                } else {
                    depth_by_unit
                        .get(parent)
                        .map(|depth| depth + 1)
                        .unwrap_or(0)
                };
                depth_by_unit.insert(entry.code.trim(), depth);
                depth
            }
            VocabularyCategory::Person => depth_by_unit
                .get(entry.unit.trim())
                .map(|depth| depth + 1)
                .unwrap_or(0),
        })
        .collect()
}

/// 按当前可见高度换算编辑框行数；必须在进入 `ScrollArea` 之前调用。
pub(crate) fn visible_rows(ui: &egui::Ui) -> usize {
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let height = ui.available_height();
    if !height.is_finite() {
        return 24;
    }
    (((height - 12.0) / row_height).floor() as i64).clamp(8, 200) as usize
}

/// 一个位置的字体选择行：下拉选本机字体，或浏览一个字体文件。
/// 返回需要显示在状态栏的提示（选了不支持的文件时给出）。
///
/// `key` 只用于下拉框的 id，`label` 是行标题，`bundled_label` 是“内置（…）”
/// 里显示的内置字体名，`hint` 是悬停说明。编译字体的五个位置与界面字体共用。
#[allow(clippy::too_many_arguments)] // Shared form helper; call sites keep these options explicit.
fn font_choice_row(
    ui: &mut egui::Ui,
    key: &str,
    label: &str,
    bundled_label: &str,
    hint: &str,
    choice: &mut crate::models::FontChoice,
    available: &[system_fonts::SystemFont],
    filter: &mut String,
) -> Option<String> {
    let mut message = None;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(label));
        let selected = if choice.is_set() {
            choice.label().to_string()
        } else {
            format!("内置（{bundled_label}）")
        };
        egui::ComboBox::from_id_salt(format!("font_role_{key}"))
            .selected_text(selected)
            .width(300.0)
            .show_ui(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(filter)
                        .hint_text("输入字体名筛选")
                        .desired_width(280.0),
                );
                ui.separator();
                if ui
                    .selectable_label(!choice.is_set(), format!("内置（{bundled_label}）"))
                    .clicked()
                {
                    *choice = crate::models::FontChoice::default();
                }
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        let needle = filter.trim().to_lowercase();
                        let mut shown = 0usize;
                        for font in available.iter().filter(|font| {
                            needle.is_empty()
                                || font.display.to_lowercase().contains(&needle)
                                || font.family.to_lowercase().contains(&needle)
                        }) {
                            shown += 1;
                            // 中文名和英文名不一致时两个都显示：写进 TeX 的是英文名。
                            let label = if font.display == font.family {
                                font.family.clone()
                            } else {
                                format!("{}（{}）", font.display, font.family)
                            };
                            let picked = choice.family == font.family;
                            if ui.selectable_label(picked, label).clicked() {
                                *choice = font.to_choice();
                            }
                        }
                        if shown == 0 {
                            ui.weak("没有匹配的字体。");
                        }
                    });
            })
            .response
            .on_hover_text(hint);
        if theme::icon_button(ui, theme::Icon::Folder, "浏览字体文件").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("字体文件", system_fonts::SUPPORTED_EXTENSIONS)
                .pick_file()
        {
            match system_fonts::read_font(&path) {
                Some(font) => *choice = font.to_choice(),
                None => {
                    message = Some(format!(
                        "无法把「{}」用作{label}：只支持 ttf 与 otf，字体集合（ttc）需要额外指定字面序号，暂不支持。",
                        path.display(),
                    ));
                }
            }
        }
        if choice.is_set() && theme::icon_button(ui, theme::Icon::RotateCcw, "恢复内置字体").clicked()
        {
            *choice = crate::models::FontChoice::default();
        }
    });
    if choice.is_set() {
        ui.horizontal(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
            ui.weak(choice.path.clone());
        });
    }
    message
}

pub(crate) fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) -> bool {
    ui.horizontal(|ui| {
        row_label(ui, label);
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        )
        .changed()
    })
    .inner
}

/// 起草页下拉框中的一个候选项。
/// 单位按层级显示：本列表中没有上级的显示全称，有上级的缩进一级、只显示本级名称，
/// 与成文时“同一上级不重复、跨上级用逗号”的写法对应。
/// 人员、机关代字等没有层级的字段用 `plain_options` 生成，层级一律为 0。
#[derive(Clone)]
pub(crate) struct SelectOption {
    /// 写入配置的值，即词库中的规范名称。
    pub(crate) value: String,
    /// 下拉列表中显示的文字。
    pub(crate) label: String,
    /// 展开后的完整名称，用于收起后的选中项和已选标签。
    pub(crate) full: String,
    /// 上级单位的规范名称，空串表示顶层；只有单位候选项会用到。
    pub(crate) parent: String,
    /// 缩进层级，只相对本列表中可见的上级计算。
    pub(crate) depth: usize,
}

/// 没有层级的候选项：标签就是取值本身。
pub(crate) fn plain_options(values: &[String]) -> Vec<SelectOption> {
    values
        .iter()
        .map(|value| SelectOption {
            value: value.clone(),
            label: value.clone(),
            full: value.clone(),
            parent: String::new(),
            depth: 0,
        })
        .collect()
}

/// 按层级摆放候选项：`allowed` 为 `Some` 时只保留其中的取值。
/// 缩进只相对“列表里还在的上级”计算——找得到上级的缩进一级、只显示本级名称，
/// 找不到的排在顶层、显示完整名称，这样过滤之后也不会出现没有上级却缩进的孤儿项。
pub(crate) fn layout_options(
    pool: &[SelectOption],
    allowed: Option<&[String]>,
) -> Vec<SelectOption> {
    let mut placed: Vec<SelectOption> = Vec::new();
    for option in pool.iter().filter(|option| {
        allowed.is_none_or(|allowed| allowed.iter().any(|item| item.trim() == option.value))
    }) {
        let mut ancestor = option.parent.clone();
        let mut parent_depth = None;
        while !ancestor.is_empty() {
            if let Some(found) = placed.iter().find(|kept| kept.value == ancestor) {
                parent_depth = Some(found.depth);
                break;
            }
            let Some(next) = pool.iter().find(|candidate| candidate.value == ancestor) else {
                break;
            };
            if next.parent == ancestor {
                break;
            }
            ancestor = next.parent.clone();
        }
        let (depth, label) = match parent_depth {
            Some(depth) => (depth + 1, option.label.clone()),
            None => (0, option.full.clone()),
        };
        placed.push(SelectOption {
            label,
            depth,
            ..option.clone()
        });
    }
    placed
}

/// 下拉列表中的显示文字。缩进写进文字本身，而不是靠嵌套布局，
/// 这样下拉框里整行仍然可以点击。用全角空格保证中文字体下的缩进宽度一致。
pub(crate) fn indented_label(option: &SelectOption) -> String {
    format!("{}{}", "　".repeat(option.depth), option.label)
}

/// 从标准词库中单选。允许手工输入时，右侧的“手填/选择”按钮在下拉与文本框之间切换。
#[allow(clippy::too_many_arguments)] // Shared form helper; call sites keep these options explicit.
pub(crate) fn single_select(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    options: &[SelectOption],
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    width: f32,
    hint: &str,
) -> bool {
    let mut changed = false;
    // 词库里没有候选项时只能手写，否则字段将无法填写。
    let manual = manual_fields.contains(id) || options.is_empty();
    // 收起状态显示完整名称，让下级单位也能一眼看出挂在哪个机关下面。
    let selected_text = if value.trim().is_empty() {
        "（未选择）".to_string()
    } else {
        options
            .iter()
            .find(|option| option.value == *value)
            .map(|option| option.full.clone())
            .unwrap_or_else(|| value.clone())
    };
    ui.horizontal(|ui| {
        if manual {
            if options.is_empty() && !allow_free_text {
                ui.colored_label(WARN, "标准词库中暂无该类词条，请先到“标准词库”页维护");
                return;
            }
            changed = ui
                .add(
                    egui::TextEdit::singleline(value)
                        .hint_text(hint)
                        .desired_width(width),
                )
                .changed();
        } else {
            egui::ComboBox::from_id_salt(id)
                .selected_text(selected_text)
                .width(width)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(value.trim().is_empty(), "（未选择）")
                        .clicked()
                    {
                        value.clear();
                        changed = true;
                    }
                    for option in options {
                        let response =
                            ui.selectable_label(*value == option.value, indented_label(option));
                        let response = if option.full == option.label {
                            response
                        } else {
                            response.on_hover_text(&option.full)
                        };
                        if response.clicked() {
                            *value = option.value.clone();
                            changed = true;
                        }
                    }
                });
        }
        if allow_free_text && !options.is_empty() {
            toggle_manual_button(ui, id, manual, manual_fields);
        }
    });
    changed
}

/// 主送、抄送等多值字段：勾选多个规范名称，界面上以标签形式回显，配置中以顿号分隔存储。
/// `excluded` 中的候选项已被另一个字段占用（主送与抄送互斥），只展示不可勾选。
#[allow(clippy::too_many_arguments)]
pub(crate) fn multi_select(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    options: &[SelectOption],
    excluded: &[String],
    excluded_reason: &str,
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    width: f32,
) -> bool {
    let mut changed = false;
    let manual = manual_fields.contains(id) || options.is_empty();
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            if manual {
                if options.is_empty() && !allow_free_text {
                    ui.colored_label(WARN, "标准词库中暂无该类词条，请先到“标准词库”页维护");
                    return;
                }
                changed = ui
                    .add(
                        egui::TextEdit::singleline(value)
                            .hint_text("多个单位用顿号“、”分隔")
                            .desired_width(width),
                    )
                    .changed();
            } else {
                let mut selected = split_units(value);
                egui::ComboBox::from_id_salt(id)
                    .selected_text(if selected.is_empty() {
                        "（未选择）".to_string()
                    } else {
                        format!("已选 {} 个", selected.len())
                    })
                    .width(width)
                    // 多选需要连续勾选，点一次就收起会很难用。
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show_ui(ui, |ui| {
                        for option in options {
                            let label = indented_label(option);
                            let mut checked = selected.contains(&option.value);
                            if !checked && excluded.contains(&option.value) {
                                ui.add_enabled(false, egui::Checkbox::new(&mut false, label))
                                    .on_disabled_hover_text(excluded_reason);
                                continue;
                            }
                            let response = ui.checkbox(&mut checked, label);
                            if option.full != option.label {
                                response.clone().on_hover_text(&option.full);
                            }
                            if response.changed() {
                                if checked {
                                    selected.push(option.value.clone());
                                } else {
                                    selected.retain(|item| *item != option.value);
                                }
                                changed = true;
                            }
                        }
                    });
                // 无论是刚勾选还是配置里的旧值，一律按词库顺序回写：
                // 导出内容取决于词库排序，而不是勾选的先后顺序。
                let ordered = sort_by_vocabulary(selected, options);
                if ordered != split_units(value) {
                    *value = join_units(&ordered);
                    changed = true;
                }
            }
            if allow_free_text && !options.is_empty() {
                toggle_manual_button(ui, id, manual, manual_fields);
            }
        });

        if !manual {
            let selected = split_units(value);
            if !selected.is_empty() {
                let mut remove = None;
                ui.set_max_width(width);
                ui.horizontal_wrapped(|ui| {
                    for (index, item) in selected.iter().enumerate() {
                        let known = options.iter().find(|option| option.value == *item);
                        // 已选标签显示完整名称，免得只看到“办公室”分不清是哪家的。
                        let display = known.map(|option| option.full.as_str()).unwrap_or(item);
                        let known = known.is_some();
                        let button = theme::removable_tag_button(display, !known);
                        let response = ui.add(button);
                        let response = if known {
                            response.on_hover_text("点击移除")
                        } else {
                            response.on_hover_text("该名称不在标准词库中，点击移除")
                        }
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if response.clicked() {
                            remove = Some(index);
                        }
                    }
                });
                if let Some(index) = remove {
                    let mut selected = selected;
                    selected.remove(index);
                    *value = join_units(&sort_by_vocabulary(selected, options));
                    changed = true;
                }
            }
        }
    });
    changed
}

/// 按标准词库中的先后顺序（即单位层级顺序）排列已选项；
/// 词库里没有的旧值保持原有相对顺序排在最后。
pub(crate) fn sort_by_vocabulary(mut units: Vec<String>, options: &[SelectOption]) -> Vec<String> {
    units.sort_by_key(|item| {
        options
            .iter()
            .position(|option| option.value == *item)
            .unwrap_or(usize::MAX)
    });
    units
}

/// 联系人与电话是一组：从词库选人时自动带出绑定电话，电话框只读回显。
pub(crate) fn contact_pair(
    ui: &mut egui::Ui,
    person: &mut String,
    phone: &mut String,
    contacts: &[(String, String)],
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    width: f32,
) {
    let names = plain_options(
        &contacts
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
    );
    let manual = manual_fields.contains("contact_person") || names.is_empty();
    ui.vertical(|ui| {
        if single_select(
            ui,
            "contact_person",
            person,
            &names,
            manual_fields,
            allow_free_text,
            width,
            "姓名标准写法",
        ) && !manual
        {
            *phone = contacts
                .iter()
                .find(|(name, _)| name == person)
                .map(|(_, bound)| bound.clone())
                .unwrap_or_default();
        }
        ui.horizontal(|ui| {
            ui.label("电话");
            if manual {
                ui.add(
                    egui::TextEdit::singleline(phone)
                        .hint_text("座机或手机")
                        .desired_width((width - 40.0).max(110.0)),
                );
            } else if phone.trim().is_empty() {
                ui.colored_label(
                    WARN,
                    if person.trim().is_empty() {
                        "选择联系人后自动带出".to_string()
                    } else {
                        format!("“{}”在词库中未维护电话", person.trim())
                    },
                );
            } else {
                ui.label(phone.as_str())
                    .on_hover_text("电话与联系人在标准词库中绑定，改电话请到“标准词库”页");
            }
        });
    });
}

pub(crate) fn toggle_manual_button(
    ui: &mut egui::Ui,
    id: &str,
    manual: bool,
    manual_fields: &mut BTreeSet<String>,
) {
    let (icon, label, tip) = if manual {
        (theme::Icon::Book, "选择", "切换回从标准词库选择")
    } else {
        (
            theme::Icon::PencilLine,
            "手填",
            "临时手工输入（不推荐，容易写错名称）",
        )
    };
    if ui
        .add_sized(
            [TOGGLE_WIDTH, FORM_CONTROL_HEIGHT],
            theme::icon_text_button(icon, label),
        )
        .on_hover_text(tip)
        .clicked()
    {
        if manual {
            manual_fields.remove(id);
        } else {
            manual_fields.insert(id.to_string());
        }
    }
}

/// 用系统默认程序打开文件或文件夹。
pub(crate) fn open_in_os(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // explorer 成功时也可能返回非 0 退出码，只看能否启动。
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// 在文件管理器中定位并选中文件。
pub(crate) fn reveal_in_os(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // explorer 的 /select 参数不接受常规转义，必须整体作为原始命令行传入。
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = path.parent().unwrap_or(path);
        open_in_os(target)
    }
}

/// 导出全部勾选格式；生成 TeX 后自动检测本机编译器，有可用引擎时把 PDF 一并加入结果。
/// 编译失败不阻断导出，而是以警告形式返回，保留已写好的 md/docx/tex。
pub(crate) fn export_and_compile(
    output_dir: &Path,
    input: &DraftInput,
    markdown: &str,
    selection: &ExportSelection,
    vocabulary: &[VocabularyEntry],
    fonts: &FontConfig,
    mut progress: impl FnMut(&str),
) -> anyhow::Result<(Vec<PathBuf>, Option<String>)> {
    progress("正在生成导出文件…");
    let display = UnitDisplay::new(vocabulary);
    // 字体文件在写 TeX 之前就要落实：TeX 里写死了按哪个文件加载，等到编译时
    // 才发现文件不在就只能报错，而这里还来得及退回内置字体。
    let (fonts, mut warnings) = system_fonts::resolve(fonts);
    let mut files = export::export_all(output_dir, input, markdown, selection, &display, &fonts)?;
    if let Some(tex) = files
        .iter()
        .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
    {
        progress("正在使用内置 Tectonic 离线编译 PDF…");
        match texcompile::compile_pdf_if_available(tex, &fonts) {
            Ok(Some(pdf)) => {
                files.push(pdf);
                progress("PDF 编译完成，正在整理导出结果…");
            }
            Ok(None) => {}
            Err(error) => warnings.push(format!("{error:#}")),
        }
    }
    let warning = (!warnings.is_empty()).then(|| warnings.join("\n"));
    Ok((files, warning))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个单位候选池：`(取值, 上级)`，全称按上级递归拼接。
    fn pool(units: &[(&str, &str)]) -> Vec<SelectOption> {
        fn full(units: &[(&str, &str)], value: &str) -> String {
            match units.iter().find(|(name, _)| *name == value) {
                Some((name, parent)) if !parent.is_empty() => {
                    format!("{}{name}", full(units, parent))
                }
                _ => value.to_string(),
            }
        }
        units
            .iter()
            .map(|(value, parent)| SelectOption {
                value: (*value).to_string(),
                label: (*value).to_string(),
                full: full(units, value),
                parent: (*parent).to_string(),
                depth: 0,
            })
            .collect()
    }

    #[test]
    fn selection_follows_vocabulary_order_not_click_order() {
        let options = plain_options(&[
            "甲单位".to_string(),
            "乙单位".to_string(),
            "丙单位".to_string(),
        ]);
        let clicked = vec!["丙单位".to_string(), "甲单位".to_string()];
        assert_eq!(
            sort_by_vocabulary(clicked, &options),
            vec!["甲单位".to_string(), "丙单位".to_string()]
        );
    }

    #[test]
    fn unknown_units_keep_their_relative_order_at_the_end() {
        let options = plain_options(&["甲单位".to_string()]);
        let mixed = vec![
            "临时单位A".to_string(),
            "甲单位".to_string(),
            "临时单位B".to_string(),
        ];
        assert_eq!(
            sort_by_vocabulary(mixed, &options),
            vec![
                "甲单位".to_string(),
                "临时单位A".to_string(),
                "临时单位B".to_string()
            ]
        );
    }

    #[test]
    fn unit_options_indent_children_and_spell_out_top_level_units() {
        let pool = pool(&[
            ("中央网信办", ""),
            ("新闻舆论处", "中央网信办"),
            ("舆情组", "新闻舆论处"),
            ("中央宣传部", ""),
            ("网络信息处", "中央宣传部"),
        ]);
        let laid_out = layout_options(&pool, None)
            .into_iter()
            .map(|option| (option.depth, option.label))
            .collect::<Vec<_>>();
        assert_eq!(
            laid_out,
            [
                (0, "中央网信办".to_string()),
                (1, "新闻舆论处".to_string()),
                (2, "舆情组".to_string()),
                (0, "中央宣传部".to_string()),
                (1, "网络信息处".to_string()),
            ]
        );
    }

    #[test]
    fn narrowed_lists_reindent_against_the_units_that_remain() {
        let pool = pool(&[
            ("中央网信办", ""),
            ("新闻舆论处", "中央网信办"),
            ("舆情组", "新闻舆论处"),
        ]);
        // 只剩下级时，它自己排在顶层并显示全称，不会孤零零地缩进。
        let laid_out = layout_options(&pool, Some(&["舆情组".to_string()]));
        assert_eq!(laid_out.len(), 1);
        assert_eq!(laid_out[0].depth, 0);
        assert_eq!(laid_out[0].label, "中央网信办新闻舆论处舆情组");

        // 上级还在列表里时，跨过被过滤掉的中间层缩进一级。
        let laid_out = layout_options(
            &pool,
            Some(&["中央网信办".to_string(), "舆情组".to_string()]),
        )
        .into_iter()
        .map(|option| (option.depth, option.label))
        .collect::<Vec<_>>();
        assert_eq!(
            laid_out,
            [(0, "中央网信办".to_string()), (1, "舆情组".to_string())]
        );
    }

    #[test]
    fn vocabulary_tree_depth_uses_codes_with_duplicate_names() {
        let vocab = vec![
            VocabularyEntry {
                code: "00".into(),
                canonical: "甲厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "0001".into(),
                canonical: "办公室".into(),
                parent: "00".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "000101".into(),
                canonical: "综合科".into(),
                parent: "0001".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "01".into(),
                canonical: "乙厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "0101".into(),
                canonical: "办公室".into(),
                parent: "01".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Person,
                canonical: "张三".into(),
                unit: "0101".into(),
                ..Default::default()
            },
        ];
        assert_eq!(vocabulary_depths(&vocab), [0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn manuscript_security_levels_use_distinct_colors() {
        let colors = SecurityLevel::ALL.map(security_level_color);
        for (index, color) in colors.iter().enumerate() {
            for other in colors.iter().skip(index + 1) {
                assert_ne!(color, other);
            }
        }
    }

    #[test]
    fn switching_template_preserves_markdown_and_loads_its_profile() {
        let mut config = AppConfig::default();
        let old_kind = TemplateKind::OfficialLetter;
        let new_kind = TemplateKind::PhoneNotice;

        let mut draft = DraftInput {
            kind: new_kind,
            profile: config.profile(old_kind),
            ..Default::default()
        };
        draft.profile.issuing_unit = "原函稿单位".into();
        let markdown = "# 已有正文\n\n切换版式后不得清空。".to_string();

        let _warnings = switch_template_profile(&mut config, &mut draft, &markdown, old_kind);

        assert_eq!(markdown, "# 已有正文\n\n切换版式后不得清空。");
        assert_eq!(draft.profile.kind, new_kind);
        assert!(draft.profile.issuing_unit.is_empty());
        assert_eq!(
            config.profile(old_kind).issuing_unit,
            "原函稿单位",
            "切换后仍应保存原模板的版式配置"
        );
    }
}
