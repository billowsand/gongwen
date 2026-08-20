//! 应用外壳：`GongwenApp` 主状态、启动、事件循环与各功能页的调度。
//!
//! 各功能页的实现已拆分到 `app/` 子模块（词库、稿件库、版本管理、AI 提示词、
//! 窗口外壳、设置、会话、后台任务、标签管理、通用部件），这里保留结构体、
//! `new`、`eframe::App` 实现与跨模块共享的常量/辅助项。

use crate::draft_page::{DocKey, DraftSession, ExportLinks};
use crate::knowledge;
use crate::knowledge::KnowledgeStore;
use crate::manuscript;
use crate::manuscript::{ManuscriptFilter, ManuscriptRecord, ManuscriptRow, ManuscriptStore};
use crate::models::{AppConfig, DraftInput, TemplateKind, VocabularySetupStatus};
use crate::pdf_viewer::{PdfKey, PdfSession};
use crate::qa;
use crate::rag;
use crate::storage;
use crate::system_fonts;
use crate::theme;
use crate::units;
use crate::validator;
use crate::vocabulary_xlsx;
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

mod ai_prompts;
mod chrome;
mod jobs;
mod manuscript_ui;
mod proofread_ui;
mod session;
mod settings;
mod tabs;
mod versioning;
mod vocabulary;
mod widgets;

pub(crate) use ai_prompts::{AiPromptDraft, AiPromptPicker};
pub(crate) use jobs::{DocJob, KnowledgeMode, WorkerResult};
pub(crate) use manuscript_ui::{ArchivePending, ImportPreview, PdfExportDialog, ZipPasswordDialog};
pub(crate) use proofread_ui::ProofreadPageState;
pub(crate) use session::{DraftAction, ExitPrompt};
pub(crate) use tabs::{NavPage, TabRef};
pub(crate) use versioning::{
    VersionCommitDraft, VersionDiffState, VersionScope, VersionSwitchPrompt, VersionTarget,
};
pub(crate) use widgets::*;

#[derive(Debug, Clone)]
pub(crate) struct VocabularyMoveDraft {
    pub(crate) id: u64,
    /// 单位条目表示新上级编码；人员条目表示新所属单位编码。
    pub(crate) destination: String,
    pub(crate) position: units::SiblingPosition,
}

pub(crate) fn accent() -> egui::Color32 {
    theme::accent()
}
pub(crate) fn warn() -> egui::Color32 {
    theme::warn()
}
/// 四个汉字约 58 px，另留 18 px 呼吸空间；超长标签显式换行而不扩大此列。
pub(crate) const LABEL_WIDTH: f32 = 76.0;
/// 手填态下“切回词库选择”的窄图标按钮宽度。手填是逃生舱，不再占常驻列，
/// 只有真的切到手填时才借走这一个图标的位置。
pub(crate) const MANUAL_BACK_WIDTH: f32 = 32.0;
/// 左侧表单的输入框、下拉框和切换按钮使用同一最小高度。
pub(crate) const FORM_CONTROL_HEIGHT: f32 = 30.0;
/// 最窄状态下输入控件仍需容纳约 10 个汉字及下拉箭头。原先这里是 150，
/// 省下的 72px 常驻切换列直接补给字段本身。
pub(crate) const FORM_FIELD_MIN_WIDTH: f32 = 190.0;
/// 标签列、字段之间的网格/控件间距预留。
pub(crate) const FORM_LAYOUT_GUTTER: f32 = 24.0;
/// 76 标签 + 190 字段 + 24 间距 = 290 px。
pub(crate) const FORM_CONTENT_MIN_WIDTH: f32 =
    LABEL_WIDTH + FORM_FIELD_MIN_WIDTH + FORM_LAYOUT_GUTTER;
/// 内容 290 + 面板左右内边距 24 + 非浮动滚动条及留白 18 = 332 px。
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
/// 标签条统一动效时长（秒）。悬停/按下用短档，选中过渡用中档。
const TAB_HOVER_ANIM: f32 = theme::anim::FAST;
const TAB_PRESS_ANIM: f32 = theme::anim::FAST;
const TAB_SELECT_ANIM: f32 = theme::anim::MEDIUM;
/// 切换标签后内容区整体淡入的时长：这是「场景切换」的主要视觉信号。
const CONTENT_FADE_ANIM: f32 = theme::anim::MEDIUM;
/// 内容区切换时从右往左轻滑的位移（px），与淡入同步收尾。
const CONTENT_SLIDE_PX: f32 = 10.0;
pub(crate) const FORM_PANEL_MAX_WIDTH: f32 = 620.0;
/// 表单内容的宽度区间。可调整面板必须拿到确定的宽度：控件一旦请求
/// `f32::INFINITY`，面板会被内容顶到远超 `size_range` 的宽度。
pub(crate) const CONTENT_WIDTH: std::ops::RangeInclusive<f32> = FORM_CONTENT_MIN_WIDTH..=700.0;

pub struct GongwenApp {
    config: AppConfig,
    /// macOS 原生透明标题栏的实测控件尺寸；无值时使用跨平台自绘标题栏。
    macos_titlebar_metrics: Option<crate::macos_window::NativeTitlebarMetrics>,
    /// 已打开的稿件，每篇一个起草页标签。空表示当前只在导航页里。
    docs: Vec<DraftSession>,
    /// 应用内打开的 PDF，只在本次运行中保留，不写入会话恢复表。
    pdfs: Vec<PdfSession>,
    /// `docs` 中当前显示的那一篇；`view` 不是 `View::Doc` 时无意义。
    active_doc: usize,
    /// 下一篇打开的稿件用的 key，只增不减。
    next_doc_key: DocKey,
    next_pdf_key: PdfKey,
    /// 起草页回传给外壳执行的动作，帧末统一处理。
    draft_actions: Vec<DraftAction>,
    /// 导出目录里最近的 tex/pdf/docx 索引，工具栏三枚成品入口共用一份。
    export_links: ExportLinks,
    models: Vec<String>,
    vocabulary_import_conflicts: Option<Vec<vocabulary_xlsx::Conflict>>,
    /// 词库有尚未写入本机配置的编辑。
    vocabulary_dirty: bool,
    /// 首次建库引导里的顶级单位名称草稿。
    vocabulary_setup_name: String,
    /// “精确移动”对话框草稿。
    vocabulary_move: Option<VocabularyMoveDraft>,
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
    proofread_page: ProofreadPageState,
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
    /// 「导出 PDF」批量导出是否正在后台执行（防重复触发）。
    manuscript_pdf_export_busy: bool,
    /// 「导出 PDF」选项弹窗；None 表示未打开。
    manuscript_pdf_export: Option<PdfExportDialog>,
    /// ZIP 导入/导出密码弹窗；所有 ZIP 操作必须先经过这里。
    manuscript_zip_password: Option<ZipPasswordDialog>,
    /// 用户明确勾选“记住密码”后加载到内存的密码；不写入 AppConfig。
    remembered_zip_password: Option<String>,
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
    /// 上一帧补发给窗口后端的 IME 状态，见 `crate::ime`。
    ime: crate::ime::ImeState,
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
    /// 输入区模式：检索 / 问答。
    pub(crate) knowledge_mode: KnowledgeMode,
    /// 问答历史：每轮一个问题 + 答案 + 引用片段 + 降级告警。
    pub(crate) knowledge_qa_history: Vec<qa::QaTurn>,
    /// 正在生成的问答问题（等待答案期间显示在对话区）。
    pub(crate) knowledge_qa_pending: Option<String>,
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
    /// 上一帧的活动标签。标签变化时重置内容淡入动画，给场景切换一个明确的信号。
    last_content_tab: Option<TabRef>,
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

impl GongwenApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut config = storage::load().unwrap_or_default();
        let macos_titlebar_metrics = crate::macos_window::configure_native_titlebar(cc);
        // 预览字体要按配置装，所以先读配置再装字体。
        theme::set_current(config.theme);
        theme::set_current_paper(config.paper);
        theme::configure_fonts(&cc.egui_ctx, &config.fonts);
        theme::configure_icons(&cc.egui_ctx);
        theme::configure_style(&cc.egui_ctx);

        // 载入即整理：补齐词条 id，按层级重排单位并重新生成层级编码。
        units::normalize(&mut config.vocabulary);
        // 旧版本没有建库状态：已有词条即视为完成，避免升级后误弹引导。
        if config.vocabulary_setup == VocabularySetupStatus::Pending
            && !config.vocabulary.is_empty()
        {
            config.vocabulary_setup = VocabularySetupStatus::Completed;
        }
        // 旧配置没有提示词库，这里补齐预置项并给未编号的条目分配 id。
        config.ensure_ai_prompts();
        // 密级现在是所有文稿类型的必要填报项目：旧配置里的空密级回填默认“机密、20年”。
        config.ensure_security_defaults();
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
            macos_titlebar_metrics,
            docs,
            pdfs: Vec::new(),
            active_doc: 0,
            next_doc_key: 1,
            next_pdf_key: 1,
            draft_actions: Vec::new(),
            export_links: ExportLinks::default(),
            models: Vec::new(),
            vocabulary_import_conflicts: None,
            vocabulary_dirty: false,
            vocabulary_setup_name: String::new(),
            vocabulary_move: None,
            about_window_open: false,
            vocabulary_filter: String::new(),
            vocabulary_selected: None,
            vocabulary_collapsed: BTreeSet::new(),
            vocabulary_delete_confirm: None,
            vocabulary_clear_confirm: false,
            ai_prompt_picker: None,
            proofread_page: ProofreadPageState::default(),
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
            manuscript_pdf_export_busy: false,
            manuscript_pdf_export: None,
            manuscript_zip_password: None,
            remembered_zip_password: storage::load_remembered_zip_password().ok().flatten(),
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
            ime: None,
            status: "就绪。先在“设置”中连接本地模型服务。".into(),
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
            knowledge_mode: KnowledgeMode::default(),
            knowledge_qa_history: Vec::new(),
            knowledge_qa_pending: None,
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
            last_content_tab: None,
            embedding_probe_busy: false,
            rerank_probe_busy: false,
            rerank_verify_result: None,
            knowledge_preview: None,
        };
        app.restore_session();
        if app.config.vocabulary_setup == VocabularySetupStatus::Pending {
            app.open_page(NavPage::Vocabulary);
        }
        app
    }

    // ---------- AI 管理（优化提示词库） ----------

    // ---------- 稿件管理（SQLite 稿件库） ----------
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
        egui::Panel::top("window_titlebar")
            .frame(
                egui::Frame::new()
                    .fill(theme::surface())
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| self.window_titlebar(ui));
        egui::Panel::top("app_top")
            .frame(theme::panel(theme::surface(), 12))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("app_status")
            .frame(theme::panel(theme::surface(), 8))
            .show(ui, |ui| self.status_bar(ui));
        // 标签全关光了就补一格稿件管理，主区不能空着。
        if self.tabs.is_empty() {
            self.open_page(NavPage::Manuscript);
        }
        let active = self.tabs[self.active_tab.min(self.tabs.len() - 1)];
        // 场景切换：活动标签变了就重置淡入/滑动动画（0 时长直接落位），
        // 下面的 CentralPanel 再从 0 淡入到 1、从右往左轻滑回原位，
        // 让切换有明确的「进入新场景」感。
        if self.last_content_tab != Some(active) {
            self.last_content_tab = Some(active);
            ctx.animate_bool_with_time(egui::Id::new("content_fade"), false, 0.0);
            ctx.animate_value_with_time(egui::Id::new("content_slide"), CONTENT_SLIDE_PX, 0.0);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let fade = ui.ctx().animate_bool_with_time(
                egui::Id::new("content_fade"),
                true,
                CONTENT_FADE_ANIM,
            );
            let slide = ui.ctx().animate_value_with_time(
                egui::Id::new("content_slide"),
                0.0,
                CONTENT_FADE_ANIM,
            );
            ui.set_opacity(fade);
            // 轻滑入场靠平移子 Ui 的 max_rect 实现，不另开图层。
            //
            // 曾经试过 `LayerId::new(Order::Background, ...)` + `set_transform_layer`：
            // 自己 new 出来的 LayerId 从没经过 `Area`，egui 的 `Memory::areas()` 里
            // 既没有它的 AreaState 也不在 order 列表里，后果是三重的——
            // 1. `compare_order` 用 `order_map` 决胜，未注册层取到 None，`None < Some(_)`，
            //    这一层被排到根 background 层下面；而 CentralPanel 收尾时会在 background
            //    层登记一个覆盖整个中央区的 hover 矩形，`hit_test` 的 included_layers
            //    扫到它就 break，正文区所有控件被整体丢弃 → 点击、拖拽、聚焦全废；
            // 2. `Areas::layer_id_at` 只遍历注册过的层，`rect_contains_pointer` 恒为 false
            //    → ScrollArea 不再吃滚轮；
            // 3. 同理 tooltip、`clicked_elsewhere`、菜单命中判定一并失效。
            // 平移 max_rect 没有这些问题：内容仍在 CentralPanel 自己的层里，
            // 裁剪也由 CentralPanel 的 clip_rect 负责。
            let mut content_ui = ui.new_child(
                egui::UiBuilder::new().max_rect(
                    ui.available_rect_before_wrap()
                        .translate(egui::vec2(slide, 0.0)),
                ),
            );
            match active {
                TabRef::Doc(_) => self.draft_page().create_ui(&mut content_ui),
                TabRef::Page(NavPage::Vocabulary) => self.vocabulary_ui(&mut content_ui),
                TabRef::Page(NavPage::Proofread) => self.proofread_ui(&mut content_ui),
                TabRef::Page(NavPage::Manuscript) => self.manuscript_ui(&mut content_ui),
                TabRef::Page(NavPage::AiPrompts) => self.ai_prompts_ui(&mut content_ui),
                TabRef::Page(NavPage::Knowledge) => {
                    crate::knowledge_ui::knowledge_ui(self, &mut content_ui)
                }
                TabRef::Page(NavPage::Settings) => self.settings_ui(&mut content_ui),
                TabRef::Pdf(key) => self.pdf_ui(key, &mut content_ui),
            }
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
        // 缩放边框放在最后：它要盖在所有浮窗之上，贴边那几像素归窗口缩放。
        self.window_resize_borders(&ctx);
        // 所有编辑框都画完了，这时 `output.ime` 才是本帧最终的那一个。
        crate::ime::follow_cursor(&ctx, &mut self.ime);
        if self.any_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::canvas().to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SecurityLevel, VocabularyCategory, VocabularyEntry};

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
