//! 起草页：一篇打开的稿件对应一个 [`DraftSession`]，页面本身是借用一组
//! 应用级资源的 [`DraftPage`]。多篇稿件同时打开时，会话之间互不干扰，
//! 只有配置、稿件库和后台任务通道是共享的。

use crate::{
    app::{
        ACCENT, CONTENT_WIDTH, DocJob, DraftAction, FORM_CONTENT_MIN_WIDTH, FORM_CONTROL_HEIGHT,
        FORM_FIELD_MIN_WIDTH, FORM_LAYOUT_GUTTER, FORM_PANEL_DEFAULT_WIDTH, FORM_PANEL_MAX_WIDTH,
        FORM_PANEL_MIN_WIDTH, LABEL_WIDTH, SelectOption, TOGGLE_WIDTH, VersionScope,
        VersionSwitchPrompt, VersionTarget, WARN, WorkerResult, contact_pair, export_and_compile,
        layout_options, multi_select, open_in_os, plain_options, reveal_in_os, row_label,
        row_label_with_info, section_heading_with_info, single_select, summarize,
        switch_template_profile, truncate, version_hover, visible_rows,
    },
    diff,
    diff_view::{self, DiffViewAction, DiffViewConfig, DiffViewState},
    export,
    highlight::MarkdownHighlighter,
    lmstudio, manuscript,
    manuscript::ManuscriptStore,
    models::{
        AppConfig, CorrespondenceScope, DraftInput, GeneratedDraft, JointContact,
        JointIssuanceMode, LetterVersion, ManuscriptStatus, SecurityLevel, StyleMode, TemplateKind,
        VocabularyCategory, join_units, split_units,
    },
    preview, prompt, rag, storage, theme, units,
    units::UnitDisplay,
    validator,
};
use eframe::egui;
use std::{
    collections::BTreeSet,
    ops::Range,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    thread,
    time::{Duration, Instant},
};

/// 工具栏上仿 WinEdt 的三个成品入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportKind {
    Tex,
    Pdf,
    Word,
}

impl ExportKind {
    pub(crate) const ALL: [Self; 3] = [Self::Tex, Self::Pdf, Self::Word];

    /// 悬停说明里用的格式名。
    fn label(self) -> &'static str {
        match self {
            Self::Tex => "TEX",
            Self::Pdf => "PDF",
            Self::Word => "WORD",
        }
    }

    fn icon(self) -> theme::Icon {
        match self {
            Self::Tex => theme::Icon::Tex,
            Self::Pdf => theme::Icon::FileTypePdf,
            Self::Word => theme::Icon::FileTypeDoc,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Tex => "tex",
            Self::Pdf => "pdf",
            Self::Word => "docx",
        }
    }
}

/// 导出目录里当前文稿最近一次产出的 tex/pdf/docx，供工具栏那三枚入口点亮与打开。
///
/// 导出的落盘结构是 `输出目录/<文件名主干>/<文件名主干>.{md,docx,tex,pdf}`，
/// 一次导出一个子目录；同一文稿多次导出会按分钟时间戳攒出多个子目录，也会和
/// 别的文稿的目录混在一起。所以这里先按当前文稿的导出主干前缀过滤出属于它的
/// 子目录，再按目录修改时间从新到旧翻，先翻到的就是"最近一次"，三种格式都找齐
/// 即停。逐帧翻盘太贵，按目录 + 前缀 + 节流缓存，导出完成时由外壳调
/// [`ExportLinks::invalidate`] 主动作废。
#[derive(Default)]
pub(crate) struct ExportLinks {
    dir: String,
    /// 当前文稿的导出主干前缀（`document_stem_prefix`）；换文稿后缓存自动作废。
    stem: Option<String>,
    scanned_at: Option<Instant>,
    tex: Option<PathBuf>,
    pdf: Option<PathBuf>,
    docx: Option<PathBuf>,
}

impl ExportLinks {
    const TTL: Duration = Duration::from_secs(3);
    /// 导出目录会随时间攒下很多子目录，只翻最近的这些，免得工具栏拖慢整帧。
    const MAX_DIRS: usize = 24;

    pub(crate) fn invalidate(&mut self) {
        self.scanned_at = None;
    }

    pub(crate) fn path(&self, kind: ExportKind) -> Option<&Path> {
        match kind {
            ExportKind::Tex => self.tex.as_deref(),
            ExportKind::Pdf => self.pdf.as_deref(),
            ExportKind::Word => self.docx.as_deref(),
        }
    }

    /// 目录条目是否属于当前绑定的文稿：未绑定时全部认；绑定时按导出主干前缀
    /// 匹配——`stem` 本身、`stem-N` 编号变体（同名覆盖关掉后同分钟多次导出），
    /// 或摊在根目录的 `stem.<ext>` 成品。用 `-`/`.` 作分界而不是裸前缀，
    /// 避免“关于X”误匹配“关于X的补充”这类邻居。
    fn matches(&self, file_name: &std::ffi::OsStr) -> bool {
        let Some(stem) = self.stem.as_deref() else {
            return true;
        };
        let Some(name) = file_name.to_str() else {
            return false;
        };
        name == stem
            || name
                .strip_prefix(stem)
                .is_some_and(|suffix| suffix.starts_with('-') || suffix.starts_with('.'))
    }

    fn refresh(&mut self, dir: &str, stem: Option<&str>) {
        let fresh = self.dir == dir
            && self.stem.as_deref() == stem
            && self.scanned_at.is_some_and(|at| at.elapsed() < Self::TTL);
        if fresh {
            return;
        }
        self.dir = dir.to_owned();
        self.stem = stem.map(str::to_owned);
        self.scanned_at = Some(Instant::now());
        self.tex = None;
        self.pdf = None;
        self.docx = None;

        let root = Path::new(dir.trim());
        if dir.trim().is_empty() || !root.is_dir() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        let mut loose: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                if self.matches(&entry.file_name()) {
                    dirs.push((modified, entry.path()));
                }
            } else if self.matches(&entry.file_name()) {
                loose.push(entry.path());
            }
        }
        dirs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        for (_, dir) in dirs.iter().take(Self::MAX_DIRS) {
            if self.complete() {
                return;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                self.take(&entry.path());
            }
        }
        // 兜底：有人把成品直接摊在输出目录根下时也认，但排在子目录之后。
        for path in loose {
            self.take(&path);
        }
    }

    fn complete(&self) -> bool {
        self.tex.is_some() && self.pdf.is_some() && self.docx.is_some()
    }

    /// 先到先得：调用顺序已经保证是从新到旧。
    fn take(&mut self, path: &Path) {
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            return;
        };
        let ext = ext.to_ascii_lowercase();
        let Some(kind) = ExportKind::ALL.into_iter().find(|k| k.extension() == ext) else {
            return;
        };
        let slot = match kind {
            ExportKind::Tex => &mut self.tex,
            ExportKind::Pdf => &mut self.pdf,
            ExportKind::Word => &mut self.docx,
        };
        if slot.is_none() {
            *slot = Some(path.to_path_buf());
        }
    }
}

/// 工具栏控件统一高度：按钮、下拉框、状态标签共用一个交互高度，
/// 免得较高的控件把整行基线撑歪。
pub(crate) const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;

const SCREEN_PT: f32 = 96.0 / 72.0;
const OFFICIAL_PAGE_WIDTH: f32 = 595.28 * SCREEN_PT;
const OFFICIAL_PAGE_HEIGHT: f32 = 841.89 * SCREEN_PT;
const OFFICIAL_PAGE_MARGIN_LEFT: f32 = 79.35 * SCREEN_PT;
const OFFICIAL_PAGE_MARGIN_TOP: f32 = 52.0 * SCREEN_PT;
const OFFICIAL_BODY_SIZE: f32 = 16.0 * SCREEN_PT;

/// A4 公文版心宽度：(595.28 - 79.35 - 73.70) 磅，按 96 dpi 换算。
/// 实时排版模式固定用这个换行宽度，不随窗口拉宽而改变每行字数。
const OFFICIAL_EDITOR_CONTENT_WIDTH: f32 = (595.28 - 79.35 - 73.70) * SCREEN_PT;

/// 工具栏分组之间的竖线。
pub(crate) fn toolbar_separator(ui: &mut egui::Ui) {
    ui.add_sized(
        [8.0, TOOLBAR_CONTROL_HEIGHT],
        egui::Separator::default().vertical().shrink(4.0),
    );
}

/// 打开的稿件在本次运行中的唯一标识。稿件库 id 不够用：尚未入库的新稿没有
/// id，后台任务回投时也需要区分“同一篇稿件的前后两次任务”。
pub(crate) type DocKey = u64;

/// 一篇打开的稿件：内容、审校结果与这篇稿子自己的视图状态。
pub(crate) struct DraftSession {
    pub(crate) key: DocKey,
    /// 对应稿件库记录；为 None 时首次保存会新建记录。
    pub(crate) manuscript_id: Option<i64>,
    pub(crate) draft: DraftInput,
    pub(crate) generated_markdown: String,
    pub(crate) warnings: Vec<String>,
    pub(crate) output_files: Vec<PathBuf>,
    /// 最近一次导出失败的原因；成功导出或换稿后清空。
    pub(crate) export_error: Option<String>,
    /// 当前编辑内容来自的版本（起草页横幅）。
    pub(crate) loaded_version: Option<LoadedVersion>,
    /// 起草页“版本对照”模式的状态。
    pub(crate) draft_diff: DraftDiffState,
    /// 已切换为“手工输入”的字段 id；其余字段一律从标准词库选择。
    pub(crate) manual_fields: BTreeSet<String>,
    /// 左侧的公文要素填报区是否已收起。默认收起。
    pub(crate) form_collapsed: bool,
    /// 审校区当前的显示方式。
    pub(crate) preview_mode: PreviewMode,
    /// 审校提示的按需右侧抽屉。导出成品不进抽屉，走工具栏的三枚格式入口。
    pub(crate) result_drawer_open: bool,
    /// 公文预览的缩放倍率；None 表示按面板宽度自适应。
    pub(crate) preview_zoom: Option<f32>,
    /// 上一帧自适应算出的倍率，用作手动加减档的起点。
    pub(crate) preview_fit_scale: f32,
    /// 在公文预览里点中的那一块：预览和源码两边都会给它铺底色。
    pub(crate) preview_anchor: Option<PreviewAnchor>,
    /// 待处理的“跳到源码”请求，编辑框下次绘制时把光标挪过去并滚动到位。
    pub(crate) pending_source_jump: Option<usize>,
    /// 查找命中需要选中完整范围；普通预览点击仍只移动光标。
    pub(crate) pending_source_selection: Option<Range<usize>>,
    /// 查找导航后，公文预览下一帧滚动到当前命中所在的版式块。
    pub(crate) pending_render_jump: bool,
    /// 审校区查找/替换条的状态。
    pub(crate) markdown_find: MarkdownFindState,
    /// 审校区的特殊标记符号栏是否显示（可隐藏，用于快速插入正文/附件等区段标记）。
    pub(crate) mark_toolbar_visible: bool,
    /// 审校区的语法高亮缓存。
    pub(crate) highlighter: MarkdownHighlighter,
    /// 本篇正在跑生成/优化/导出。任务按篇计数，切到别的稿件照常编辑。
    pub(crate) busy: bool,
    /// 本篇已发起的后台任务序号；结果回投时对不上就说明已被新任务取代。
    pub(crate) job_seq: u64,
    /// 本次优化用的提示词名称，只用于完成后的状态栏文案。
    pub(crate) ai_prompt_last_label: String,
    /// 上次写入稿件库时的内容基线：`(要素 JSON, 正文)`。为 None 表示这篇
    /// 还没入过库。脏判定就是拿它和当前内容比。
    pub(crate) saved_baseline: Option<(String, String)>,
    /// 最新已提交版本的内容基线，判断“已存库但还没固化成版本”。
    /// 为 None 表示这篇一个版本都还没提交过。
    pub(crate) committed_baseline: Option<(String, String)>,
    /// 稿件库里这条记录的生命周期状态。发布与归档的稿件开成只读标签。
    pub(crate) record_status: ManuscriptStatus,
    /// 版本抽屉是否展开。
    pub(crate) versions_open: bool,
    /// 标签刚打开时的内容指纹。“第一次改动才自动入库”靠它区分
    /// “刚复制过来还没动” 和 “真的改了”。
    opened_fingerprint: String,
    /// 起草时是否检索知识库注入相似稿件片段作参考（仅起草、优化不用）。
    pub(crate) use_knowledge_rag: bool,
    /// 知识库检索的文种过滤。
    pub(crate) rag_kind_filter: RagKindFilter,
}

/// 起草时知识库检索的文种范围。
///
/// 之所以不用 `Option<TemplateKind>`：那样 `None` 既要表示"跟随当前文种"、
/// 又要表示"不限文种"，只能二选一，结果是**跨文种参考根本选不出来**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RagKindFilter {
    /// 跟随当前稿件的文种，相关度最高。
    #[default]
    Follow,
    /// 不限文种，全库检索。
    All,
    /// 指定文种。
    Only(TemplateKind),
}

impl RagKindFilter {
    /// 解析成检索层要的过滤条件：None = 不限文种。
    pub(crate) fn resolve(self, current: TemplateKind) -> Option<TemplateKind> {
        match self {
            Self::Follow => Some(current),
            Self::All => None,
            Self::Only(kind) => Some(kind),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Follow => "跟随当前文种",
            Self::All => "全部文种",
            Self::Only(kind) => kind.label(),
        }
    }
}

impl DraftSession {
    /// 新开一篇空白稿件。文种与版式沿用配置里最近一次使用的。
    pub(crate) fn blank(key: DocKey, config: &AppConfig) -> Self {
        let kind = config.last_template;
        let mut profile = config.profile(kind);
        if profile.document_year.trim().is_empty() {
            profile.document_year = chrono::Local::now().format("%Y").to_string();
        }
        Self::from_parts(
            key,
            None,
            DraftInput {
                kind,
                profile,
                ..Default::default()
            },
            String::new(),
        )
    }

    pub(crate) fn from_parts(
        key: DocKey,
        manuscript_id: Option<i64>,
        mut draft: DraftInput,
        generated_markdown: String,
    ) -> Self {
        if draft.kind == TemplateKind::OfficialLetter
            && draft.profile.document_year.trim().is_empty()
        {
            draft.profile.document_year = draft.document_year();
        }
        let mut session = Self {
            key,
            manuscript_id,
            draft,
            generated_markdown,
            warnings: Vec::new(),
            output_files: Vec::new(),
            export_error: None,
            loaded_version: None,
            draft_diff: DraftDiffState::default(),
            manual_fields: BTreeSet::new(),
            // 打开稿件第一眼就是满屏审校稿；要改抬头文号再展开要素区。
            form_collapsed: true,
            preview_mode: PreviewMode::Source,
            result_drawer_open: false,
            preview_zoom: None,
            preview_fit_scale: 1.0,
            preview_anchor: None,
            pending_source_jump: None,
            pending_source_selection: None,
            pending_render_jump: false,
            markdown_find: MarkdownFindState::default(),
            mark_toolbar_visible: true,
            highlighter: MarkdownHighlighter::default(),
            busy: false,
            job_seq: 0,
            ai_prompt_last_label: String::new(),
            saved_baseline: None,
            committed_baseline: None,
            record_status: ManuscriptStatus::Draft,
            versions_open: false,
            opened_fingerprint: String::new(),
            use_knowledge_rag: false,
            rag_kind_filter: RagKindFilter::default(),
        };
        session.opened_fingerprint = session.fingerprint();
        session
    }

    /// 内容指纹：公文要素 + 正文。脏判定与“动过没有”都拿它比。
    fn fingerprint(&self) -> String {
        format!(
            "{}\u{1}{}",
            serde_json::to_string(&self.draft).unwrap_or_default(),
            self.generated_markdown
        )
    }

    /// 已发布或已归档：整篇只读，改不动也存不进去。
    pub(crate) fn read_only(&self) -> bool {
        self.manuscript_id.is_some()
            && matches!(
                self.record_status,
                ManuscriptStatus::Published | ManuscriptStatus::Archived
            )
    }

    /// 打开之后真的动过内容——尚未入库的新稿据此决定要不要自动建记录。
    pub(crate) fn touched(&self) -> bool {
        self.fingerprint() != self.opened_fingerprint
    }

    /// 把当前内容记为“已保存”。载入稿件、保存成功、载入版本之后都要调。
    pub(crate) fn mark_saved(&mut self) {
        self.opened_fingerprint = self.fingerprint();
        self.saved_baseline = Some((
            serde_json::to_string(&self.draft).unwrap_or_default(),
            self.generated_markdown.clone(),
        ));
    }

    /// 相对稿件库里的记录有没有未写入的改动。尚未入库的稿子，只要动过就算脏。
    pub(crate) fn is_dirty(&self) -> bool {
        if self.read_only() {
            return false;
        }
        match &self.saved_baseline {
            // 正文是大头，先比它；一致时才去序列化要素。
            Some((draft, content)) => {
                *content != self.generated_markdown
                    || serde_json::to_string(&self.draft).unwrap_or_default() != *draft
            }
            None => {
                !self.generated_markdown.trim().is_empty()
                    || !self.draft.title_hint.trim().is_empty()
            }
        }
    }

    /// 已经写进稿件库，但相对最新提交版本还有变化——该固化一个版本了。
    /// 尚未入库的稿子不算：它连库里的记录都还没有，谈不上提交版本。
    pub(crate) fn has_uncommitted(&self) -> bool {
        if self.manuscript_id.is_none() || self.read_only() || self.is_dirty() {
            return false;
        }
        match &self.committed_baseline {
            Some((draft, content)) => {
                *content != self.generated_markdown
                    || serde_json::to_string(&self.draft).unwrap_or_default() != *draft
            }
            // 一版都没提交过：只要有正文，就还欠一个版本。
            None => !self.generated_markdown.trim().is_empty(),
        }
    }

    /// 标签上的状态标记：实心点=有改动没写库，空心圈=已存库但没提交版本。
    pub(crate) fn dirty_mark(&self) -> &'static str {
        if self.is_dirty() {
            "●"
        } else if self.has_uncommitted() {
            "○"
        } else {
            ""
        }
    }

    /// 记下最新已提交版本的内容，作为“未提交版本”判定的基线。
    pub(crate) fn set_committed_baseline(&mut self, latest: Option<(String, String)>) {
        self.committed_baseline = latest;
    }

    /// 标签的悬停说明：完整标题加上身份与保存状态。
    pub(crate) fn tab_hover(&self) -> String {
        let mut lines = vec![self.title()];
        lines.push(format!("文种：{}", self.draft.kind.label()));
        if !self.draft.profile.document_number.trim().is_empty() {
            lines.push(format!(
                "文号：{}",
                self.draft.profile.document_number.trim()
            ));
        }
        lines.push(match self.manuscript_id {
            Some(id) => format!("稿件库记录 #{id} · {}", self.record_status.label()),
            None => "尚未保存到稿件库".to_string(),
        });
        if let Some(loaded) = &self.loaded_version {
            lines.push(format!(
                "来自版本 v{}《{}》",
                loaded.version_number, loaded.name
            ));
        }
        if self.is_dirty() {
            lines.push("● 有未保存的改动".to_string());
        } else if self.has_uncommitted() {
            lines.push("○ 已存库，但相对最新版本有改动".to_string());
        }
        lines.join(
            "
",
        )
    }

    /// 标签与状态栏用的短标题。正文里的标题优先，其次是要素里的标题提示。
    pub(crate) fn title(&self) -> String {
        let title = export::extract_title(&self.generated_markdown, &self.draft.title_hint);
        if title.trim().is_empty() {
            "未命名公文".to_string()
        } else {
            title
        }
    }

    /// 换稿（载入别的记录或别的版本）后要清掉的一次性状态。
    pub(crate) fn reset_transient(&mut self) {
        self.output_files.clear();
        self.export_error = None;
        self.preview_anchor = None;
        self.pending_source_jump = None;
        self.pending_source_selection = None;
        self.pending_render_jump = false;
    }
}

/// 起草页一帧的执行上下文：一篇会话 + 它需要的应用级资源。
/// 用借用而不是 `&mut GongwenApp`，多篇稿件才能各自独立地拿到可变引用。
pub(crate) struct DraftPage<'a> {
    pub(crate) doc: &'a mut DraftSession,
    pub(crate) config: &'a mut AppConfig,
    pub(crate) store: Option<&'a mut ManuscriptStore>,
    pub(crate) sender: &'a Sender<WorkerResult>,
    pub(crate) status: &'a mut String,
    /// 版本切换的三选确认框（全局浮窗，由应用外壳渲染）。
    pub(crate) version_switch: &'a mut Option<VersionSwitchPrompt>,
    /// 等待二次确认的“回退到该版本”。
    pub(crate) revert_confirm: &'a mut Option<(i64, i64)>,
    /// 需要由应用外壳执行的动作，帧末统一处理。
    pub(crate) actions: &'a mut Vec<DraftAction>,
    /// 导出目录的成品索引，多篇稿件共用一份。
    pub(crate) export_links: &'a mut ExportLinks,
}

/// 审校区的显示方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewMode {
    /// 带语法高亮的 Markdown 源码，导出以它为准。
    Source,
    /// 非活动行按公文版式排版，只有光标所在行显示 Markdown 标记。
    Hybrid,
    /// 按公文字体与行距渲染的版式预览。
    Rendered,
    /// 左源码、右版式。
    Split,
    /// 左最新提交版本、右当前未提交内容的修订对照。
    VersionDiff,
}

/// 文件按钮不能在借用 `self` 的过程中直接改 `self`，先记下来再执行。
pub(crate) enum FileAction {
    Open(PathBuf),
    Reveal(PathBuf),
}

/// 起草页横幅：当前编辑内容来自哪一版本。
pub(crate) struct LoadedVersion {
    pub(crate) manuscript_id: i64,
    pub(crate) version_number: i64,
    pub(crate) name: String,
}

/// 起草页"版本对照"模式的状态：左侧基准版本 + 视图状态 + diff 缓存。
#[derive(Default)]
pub(crate) struct DraftDiffState {
    /// 左侧基准版本号；None 表示跟最新版比（每次提交后自动跟上）。
    pub(crate) base: Option<i64>,
    pub(crate) view: DiffViewState,
    /// 上一次算出的 diff 及其输入指纹。这个模式每帧都要渲染，
    /// 内容没动就直接复用，长稿才不会边打字边重算。
    pub(crate) cache: Option<(u64, diff::ManuscriptDiff)>,
}

/// 审校区的查找/替换条。匹配范围每帧按当前正文重新计算，避免编辑或替换后保存
/// 已经过期的字节位置。
#[derive(Default)]
pub(crate) struct MarkdownFindState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) replacement: String,
    pub(crate) case_sensitive: bool,
    pub(crate) regex: bool,
    pub(crate) current: usize,
    pub(crate) focus_query: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum FindAction {
    Previous,
    Next,
    Replace,
    ReplaceAll,
}

/// 在公文预览里点中的那一块：记下它在 Markdown 中的字节范围，以及点击当时
/// 这段范围里的原文。
pub(crate) struct PreviewAnchor {
    range: Range<usize>,
    text: String,
}

impl PreviewAnchor {
    /// 正文改过之后，旧的字节范围可能越界、落在字符中间，或者已经指向别的内容。
    /// 只有范围内的原文仍与点击时一致，才认为锚点还指着同一段；`str::get`
    /// 顺带挡掉越界和非字符边界，避免把半个汉字切开。
    pub(crate) fn range_in(&self, text: &str) -> Option<Range<usize>> {
        (text.get(self.range.clone()) == Some(self.text.as_str())).then(|| self.range.clone())
    }
}

/// 审校区编辑框的固定 id：预览点击回跳时要按它取回光标状态。
pub(crate) fn editor_id() -> egui::Id {
    egui::Id::new("gw-markdown-editor")
}

fn source_line_at_char(text: &str, char_index: usize) -> usize {
    text.chars()
        .take(char_index.min(text.chars().count()))
        .filter(|ch| *ch == '\n')
        .count()
}

fn active_source_line(ctx: &egui::Context, text: &str) -> usize {
    egui::TextEdit::load_state(ctx, editor_id())
        .and_then(|state| state.cursor.char_range())
        .map_or(0, |range| source_line_at_char(text, range.primary.index.0))
}

#[derive(Clone, Copy)]
struct EditorLineVisual {
    top: f32,
    bottom: f32,
    baseline: f32,
}

fn editor_line_visuals(output: &egui::text_edit::TextEditOutput) -> Vec<EditorLineVisual> {
    let mut lines = Vec::new();
    let mut top = None;
    let mut baseline = None;
    let mut bottom = output.galley_pos.y;
    for placed in &output.galley.rows {
        let row_top = output.galley_pos.y + placed.pos.y;
        let row_bottom = row_top + placed.size.y;
        top.get_or_insert(row_top);
        if baseline.is_none() {
            baseline = placed
                .glyphs
                .iter()
                .find(|glyph| glyph.font_height > OFFICIAL_BODY_SIZE * 0.5)
                .or_else(|| placed.glyphs.first())
                .map(|glyph| row_top + glyph.pos.y);
        }
        bottom = row_bottom;
        if placed.ends_with_newline {
            let top = top.take().unwrap_or(row_top);
            lines.push(EditorLineVisual {
                top,
                bottom,
                baseline: baseline.take().unwrap_or((top + bottom) * 0.5),
            });
        }
    }
    if let Some(top) = top {
        lines.push(EditorLineVisual {
            top,
            bottom,
            baseline: baseline.unwrap_or((top + bottom) * 0.5),
        });
    }
    lines
}

/// 按 galley 的实际行高绘制源码行号。一个 Markdown 段落自动换行时，
/// 只在第一个视觉行旁显示编号，不把软换行误当成新的源码行。
fn paint_editor_line_numbers(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput) {
    let x = output.galley_pos.x - 10.0;
    let painter = ui.painter();
    let font = egui::FontId::new(11.5, egui::FontFamily::Proportional);
    for (index, line) in editor_line_visuals(output).into_iter().enumerate() {
        painter.text(
            egui::pos2(x, (line.top + line.bottom) * 0.5),
            egui::Align2::RIGHT_CENTER,
            (index + 1).to_string(),
            font.clone(),
            theme::TEXT_MUTED,
        );
    }
}

fn markdown_heading_level(line: &str) -> Option<u8> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    ((1..=6).contains(&hashes)
        && trimmed
            .as_bytes()
            .get(hashes)
            .is_some_and(u8::is_ascii_whitespace))
    .then_some(hashes as u8)
}

fn is_table_source_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && trimmed.matches('|').count() >= 2
}

fn is_table_separator_line(line: &str) -> bool {
    let cells = line
        .trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .collect::<Vec<_>>();
    cells.len() >= 2
        && cells.iter().all(|cell| {
            let value = cell.trim().trim_matches(':');
            value.len() >= 3 && value.chars().all(|ch| ch == '-')
        })
}

fn table_column_count(line: &str) -> usize {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .count()
        .max(1)
}

/// 实时排版中不写入 Markdown 的视觉层：公文自动编号和表格框线。
fn paint_hybrid_decorations(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    active_line: usize,
) {
    let visuals = editor_line_visuals(output);
    let source_lines = text.split('\n').collect::<Vec<_>>();
    let painter = ui.painter();
    let mut counters = [0usize; 4];

    for (index, line) in source_lines.iter().enumerate() {
        let Some(level) = markdown_heading_level(line) else {
            continue;
        };
        let prefix = export::official_heading_prefix(level, &mut counters);
        if index == active_line {
            continue;
        }
        let (Some(prefix), Some(visual)) = (prefix, visuals.get(index)) else {
            continue;
        };
        let family = match level {
            2 => theme::FONT_HEITI,
            3 => theme::FONT_KAITI,
            _ => theme::FONT_FANGSONG,
        };
        let font = egui::FontId::new(OFFICIAL_BODY_SIZE, theme::official_family(family));
        let prefix_galley = painter.layout_no_wrap(prefix, font, egui::Color32::BLACK);
        let prefix_baseline = prefix_galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
            .unwrap_or(prefix_galley.size().y);
        painter.galley(
            egui::pos2(
                output.galley_pos.x + OFFICIAL_BODY_SIZE * 2.0,
                visual.baseline - prefix_baseline,
            ),
            prefix_galley,
            egui::Color32::BLACK,
        );
    }

    let stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
    let mut index = 0usize;
    while index < source_lines.len() {
        if !is_table_source_line(source_lines[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < source_lines.len() && is_table_source_line(source_lines[index]) {
            index += 1;
        }
        let end = index;
        let columns = source_lines[start..end]
            .iter()
            .map(|line| table_column_count(line))
            .max()
            .unwrap_or(1);
        for row in (start..end).filter(|row| !is_table_separator_line(source_lines[*row])) {
            let Some(visual) = visuals.get(row) else {
                continue;
            };
            let rect = egui::Rect::from_min_max(
                egui::pos2(output.galley_pos.x, visual.top),
                egui::pos2(
                    output.galley_pos.x + OFFICIAL_EDITOR_CONTENT_WIDTH,
                    visual.bottom,
                ),
            );
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
            for column in 1..columns {
                let x = rect.left() + rect.width() * column as f32 / columns as f32;
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    stroke,
                );
            }
        }
    }
}

pub(crate) fn find_query_id() -> egui::Id {
    egui::Id::new("gw-markdown-find-query")
}

/// 起草时检索知识库并把结果拼成提示词参考节。检索失败降级为空串，不阻塞
/// 起草——RAG 是增强而非硬依赖；但降级原因会随 `notes` 回给调用方显示，
/// 不再是只有翻服务端日志才知道的静默失败。
///
/// 返回 (参考节, 给用户看的说明)。
fn retrieve_reference(
    rag_cfg: &crate::models::RagConfig,
    chat: &crate::models::LmStudioConfig,
    input: &DraftInput,
    instruction: &str,
    kind_filter: Option<TemplateKind>,
) -> (String, Vec<String>) {
    let query = if input.title_hint.trim().is_empty() {
        instruction.trim().to_string()
    } else {
        format!("{}\n{}", input.title_hint.trim(), instruction.trim())
    };
    if query.trim().is_empty() {
        return (String::new(), Vec::new());
    }
    let db_path = match storage::manuscript_db_path() {
        Ok(path) => path,
        Err(error) => return (String::new(), vec![format!("知识库路径不可用：{error:#}")]),
    };
    let outcome = match rag::retrieve(rag_cfg, chat, &db_path, &query, kind_filter) {
        Ok(outcome) => outcome,
        Err(error) => {
            return (
                String::new(),
                vec![format!("知识库检索失败，本次未注入参考：{error:#}")],
            );
        }
    };
    let mut notes = outcome.warnings;
    if outcome.chunks.is_empty() {
        notes.push("知识库没有检索到相关片段，本次未注入参考。".into());
        return (String::new(), notes);
    }
    let refs: Vec<prompt::ReferenceChunk> = outcome
        .chunks
        .into_iter()
        .map(|chunk| prompt::ReferenceChunk {
            kind_label: chunk.kind.label().to_string(),
            doc_title: chunk.doc_title,
            section: chunk.section,
            text: chunk.text,
        })
        .collect();
    notes.push(format!("已注入 {} 段知识库参考。", refs.len()));
    (prompt::format_reference_section(&refs), notes)
}

/// 返回非重叠匹配的 UTF-8 字节范围，正好可以安全交给 `String::replace_range`。
#[cfg(test)]
pub(crate) fn markdown_matches(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    markdown_matches_mode(text, query, case_sensitive, false).unwrap_or_default()
}

/// 按普通文本或正则表达式返回非重叠匹配；正则无效时把编译错误交给界面展示。
pub(crate) fn markdown_matches_mode(
    text: &str,
    query: &str,
    case_sensitive: bool,
    regex_mode: bool,
) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = if regex_mode {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let regex = regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(regex
        .find_iter(text)
        .map(|matched| matched.start()..matched.end())
        .collect())
}

fn expanded_replacement(
    text: &str,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
    regex_mode: bool,
    range: &Range<usize>,
) -> String {
    if !regex_mode {
        return replacement.to_string();
    }
    let Ok(regex) = regex::RegexBuilder::new(query)
        .case_insensitive(!case_sensitive)
        .build()
    else {
        return replacement.to_string();
    };
    let Some(captures) = regex.captures_at(text, range.start) else {
        return replacement.to_string();
    };
    if captures.get(0).map(|matched| matched.range()) != Some(range.clone()) {
        return replacement.to_string();
    }
    let mut expanded = String::new();
    captures.expand(replacement, &mut expanded);
    expanded
}

/// 把编辑框的光标挪到源码的 `offset` 字节处，聚焦并滚动到可见位置。
pub(crate) fn jump_to_source(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    offset: usize,
) {
    // egui 的光标按字符计，源码范围按字节计，这里换算一次；
    // 位置退到最近的字符边界，避免正文已改动时从半个汉字中间切开。
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let index = text[..offset].chars().count();
    let cursor = egui::text::CCursor::new(index);
    let mut state = output.state.clone();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
    state.store(ui.ctx(), editor_id());
    ui.ctx()
        .memory_mut(|memory| memory.request_focus(editor_id()));
    // egui 只在光标“变化”时自动滚动，程序设定的光标不算，只能自己滚。
    let rect = output
        .galley
        .pos_from_cursor(cursor)
        .translate(output.galley_pos.to_vec2());
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
}

/// 选中查找命中的完整字节范围，并把当前命中滚动到编辑器中央。
pub(crate) fn select_source_range(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    range: Range<usize>,
) {
    if range.is_empty()
        || range.end > text.len()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return;
    }
    let start = egui::text::CCursor::new(text[..range.start].chars().count());
    let end = egui::text::CCursor::new(text[..range.end].chars().count());
    let mut state = output.state.clone();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(start, end)));
    state.store(ui.ctx(), editor_id());
    let rect = output
        .galley
        .pos_from_cursor(end)
        .translate(output.galley_pos.to_vec2());
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
}

impl DraftPage<'_> {
    pub(crate) fn revalidate(&mut self) {
        self.doc.warnings = validator::validate(
            &self.doc.draft,
            &self.doc.generated_markdown,
            &self.config.vocabulary,
            &self.config.security_rules,
        );
        if self.doc.draft.kind == TemplateKind::OfficialLetter
            && self.doc.draft.profile.letter_version == LetterVersion::Formal
        {
            let year = self.doc.draft.document_year();
            let code = self.doc.draft.profile.department_code.trim();
            let serial = self
                .doc
                .draft
                .profile
                .document_number
                .trim()
                .parse::<u64>()
                .ok();
            if !year.is_empty()
                && !code.is_empty()
                && let Some(serial) = serial
                && let Some(highest) = self.store.as_deref().and_then(|store| {
                    store
                        .highest_letter_serial(&year, code, self.doc.manuscript_id)
                        .ok()
                        .flatten()
                })
                && serial < highest
            {
                self.doc.warnings.push(format!(
                    "函号流水号 {serial} 低于机关代字“{code}”{year}年度已有最高函号 {highest}"
                ));
            }
        }
        self.doc.warnings.sort();
        self.doc.warnings.dedup();
    }

    pub(crate) fn open_result_drawer(&mut self) {
        self.doc.result_drawer_open = true;
    }

    /// 起草页各单位下拉框的候选池，按词库的树形顺序排列。
    /// 缩进和标签由 `layout_options` 按实际参与的子集算，这里只提供取值、全称和上级。
    pub(crate) fn unit_pool(&self, external: bool) -> Vec<SelectOption> {
        let display = UnitDisplay::new(&self.config.vocabulary);
        self.config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .filter(|entry| !entry.canonical.trim().is_empty())
            .map(|entry| SelectOption {
                full: display.full_name_for(&entry.canonical, external),
                parent: display.parent_of(&entry.canonical),
                label: display.name_for(&entry.canonical, external),
                value: entry.canonical.trim().to_string(),
                depth: 0,
            })
            .collect()
    }

    /// 联系人及其绑定电话，两者始终成对取用。
    pub(crate) fn contacts(&self) -> Vec<(String, String)> {
        self.config
            .vocabulary
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Person)
            .map(|entry| {
                (
                    entry.canonical.trim().to_string(),
                    entry.phone.trim().to_string(),
                )
            })
            .filter(|(name, _)| !name.is_empty())
            .collect()
    }

    /// 规格 §2.3“人随事走”：只保留属于给定单位的人员；单位列表为空时不过滤。
    /// 返回 `(人员, 是否应用了过滤)`，供界面提示。
    pub(crate) fn filtered_contacts(
        &self,
        units: &[String],
        include_parent_unit_handlers: bool,
    ) -> (Vec<(String, String)>, bool) {
        let clean = units
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if clean.is_empty() {
            return (self.contacts(), false);
        }
        let display = UnitDisplay::new(&self.config.vocabulary);
        let mut seen = std::collections::HashSet::new();
        let mut matched = Vec::new();
        for unit in &clean {
            let people = if include_parent_unit_handlers {
                display.responsible_people_of(unit)
            } else {
                display.people_of(unit)
            };
            for (name, phone) in people {
                if seen.insert(name.clone()) {
                    matched.push((name, phone));
                }
            }
        }
        if matched.is_empty() {
            if include_parent_unit_handlers {
                // 公函版记严格执行承办权限，不能因候选为空而回落显示未授权人员。
                (matched, true)
            } else {
                // 白头件沿用原有兜底，避免呈报领导下拉框没有候选项。
                (self.contacts(), true)
            }
        } else {
            (matched, true)
        }
    }

    /// 发起一次后台任务：记在本篇名下并推进序号，结果只认这一对标识。
    fn begin_job(&mut self) -> (DocKey, u64) {
        self.doc.busy = true;
        self.doc.job_seq += 1;
        (self.doc.key, self.doc.job_seq)
    }

    /// 以右侧编辑框的当前内容为准导出，可以反复调用；这是“改稿—导出”的闭环出口。
    pub(crate) fn start_export_current(&mut self) {
        if self.doc.busy {
            return;
        }
        if self.doc.generated_markdown.trim().is_empty() {
            *self.status = "还没有可导出的内容：请先生成草稿，或直接在右侧粘贴稿件。".into();
            return;
        }
        if !self.config.export.any() {
            *self.status = "请至少勾选一种导出格式。".into();
            return;
        }
        let (key, seq) = self.begin_job();
        *self.status = "正在导出当前审校稿…".into();
        self.doc.output_files.clear();
        self.doc.export_error = None;
        let input = self.doc.draft.clone();
        let output_dir = PathBuf::from(&self.config.output_dir);
        let selection = self.config.export.clone();
        let markdown = self.doc.generated_markdown.clone();
        let vocabulary = self.config.vocabulary.clone();
        let fonts = self.config.fonts.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = export_and_compile(
                &output_dir,
                &input,
                &markdown,
                &selection,
                &vocabulary,
                &fonts,
                |message| {
                    let _ = tx.send(WorkerResult::Doc {
                        key,
                        seq,
                        job: DocJob::ExportProgress(message.into()),
                    });
                },
            )
            .map_err(|error: anyhow::Error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::Exported(result),
            });
        });
    }

    /// AI 面板随时可开：有稿件就是优化，没稿件就在面板里写明要起草什么。
    pub(crate) fn can_optimize(&self) -> bool {
        !self.doc.busy
    }

    /// 规格 §7：AI 起草 / 优化。审校稿为空时按 `instruction` 里的素材从零起草，
    /// 非空时按 `instruction` 改写现有稿件——两种场景共用一个入口。
    ///
    /// 无论 `instruction` 写什么，`prompt::output_contract` 都会以更高优先级
    /// 拼在后面，所以自定义指令再离谱也不会破坏导出所需的 Markdown 结构。
    /// `label` 只用于状态栏文案。
    pub(crate) fn start_optimize(&mut self, instruction: String, label: String) {
        if self.doc.busy {
            return;
        }
        let current = self.doc.generated_markdown.trim().to_string();
        let drafting = current.is_empty();
        if drafting && instruction.trim().is_empty() {
            *self.status =
                "还没有可处理的内容：请先粘贴稿件，或在 AI 面板里写明要起草什么。".into();
            return;
        }
        let export_now = self.config.auto_export;
        if export_now && !self.config.export.any() {
            *self.status = "已勾选“完成后自动导出”，请先在设置里选择至少一种导出格式。".into();
            return;
        }

        let time_context = prompt::TimeContext::now();
        if self.doc.draft.date_is_auto {
            self.doc.draft.date = time_context.today.clone();
        }
        self.config.upsert_profile(self.doc.draft.profile.clone());
        let (key, seq) = self.begin_job();
        *self.status = if drafting {
            format!("正在按“{label}”起草…")
        } else {
            format!("正在按“{label}”优化…")
        };
        self.doc.ai_prompt_last_label = label;
        self.doc.output_files.clear();
        self.doc.export_error = None;
        // 记住这次用的提示词，重启后选择面板仍能标出“上次使用”。
        let _ = storage::save(self.config);

        let input = self.doc.draft.clone();
        let config = self.config.clone();
        let selection = self.config.export.clone();
        let tx = self.sender.clone();
        // 仅起草（审校稿为空）时启用知识库检索；优化已有稿件不注入参考。
        let use_rag = drafting && self.doc.use_knowledge_rag && self.config.rag.enabled;
        // 文种过滤：`RagKindFilter::Follow` 跟随当前文种，`All` 不限文种。
        let rag_kind = self.doc.rag_kind_filter.resolve(self.doc.draft.kind);
        let rag_cfg = self.config.rag.clone();
        thread::spawn(move || {
            let result = (|| {
                let system = prompt::build_system_prompt(&time_context);
                let user = if drafting {
                    let reference = if use_rag {
                        let _ = tx.send(WorkerResult::Doc {
                            key,
                            seq,
                            job: DocJob::ExportProgress("正在检索知识库…".into()),
                        });
                        let (reference, notes) = retrieve_reference(
                            &rag_cfg,
                            &config.lm_studio,
                            &input,
                            &instruction,
                            rag_kind,
                        );
                        // 检索的降级说明要让用户看见，不能只留在服务端日志里。
                        if !notes.is_empty() {
                            let _ = tx.send(WorkerResult::Doc {
                                key,
                                seq,
                                job: DocJob::ExportProgress(format!(
                                    "知识库：{}",
                                    notes.join("　")
                                )),
                            });
                        }
                        reference
                    } else {
                        String::new()
                    };
                    prompt::build_draft_prompt(&input, &config.vocabulary, &instruction, &reference)
                } else {
                    prompt::build_optimize_prompt(&input, &current, &instruction)
                };
                let raw = lmstudio::generate(&config.lm_studio, &system, &user)?;
                let cleaned = prompt::sanitize_model_markdown(&raw);
                let normalized = prompt::normalize_generated_markdown(&input, &cleaned);
                let markdown = export::finalize_markdown(&input, &normalized);
                let title = export::extract_title(&markdown, &input.title_hint);
                let mut warnings = validator::validate(
                    &input,
                    &markdown,
                    &config.vocabulary,
                    &config.security_rules,
                );
                let files = if export_now {
                    let (files, compile_warning) = export_and_compile(
                        PathBuf::from(&config.output_dir).as_path(),
                        &input,
                        &markdown,
                        &selection,
                        &config.vocabulary,
                        &config.fonts,
                        |message| {
                            let _ = tx.send(WorkerResult::Doc {
                                key,
                                seq,
                                job: DocJob::ExportProgress(message.into()),
                            });
                        },
                    )?;
                    if let Some(warning) = compile_warning {
                        warnings.push(warning);
                    }
                    files
                } else {
                    vec![]
                };
                Ok(GeneratedDraft {
                    markdown,
                    title,
                    warnings,
                    files,
                })
            })()
            .map_err(|error: anyhow::Error| format!("{error:#}"));
            let job = if drafting {
                DocJob::Drafted(result)
            } else {
                DocJob::Optimized(result)
            };
            let _ = tx.send(WorkerResult::Doc { key, seq, job });
        });
    }

    pub(crate) fn open_output_dir(&mut self) {
        let dir = PathBuf::from(self.config.output_dir.trim());
        if dir.as_os_str().is_empty() {
            *self.status = "尚未设置输出目录。".into();
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&dir) {
            *self.status = format!("无法创建输出目录：{error}");
            return;
        }
        match open_in_os(&dir) {
            Ok(()) => *self.status = format!("已打开输出目录 {}。", dir.display()),
            Err(error) => *self.status = format!("打开输出目录失败：{error}"),
        }
    }

    /// 仿 WinEdt 的成品入口：TEX / PDF / WORD 三枚，当前文稿的导出目录里有
    /// 对应成品才点亮，点开的是当前文稿最近一次导出（属于它的子目录中修改时间
    /// 最新）留下的那一份。右键可在文件管理器里定位。
    fn export_open_buttons(&mut self, ui: &mut egui::Ui) {
        // 导出目录里同一文稿的文件夹都以“去掉时间戳的导出主干”为前缀，
        // 用它过滤出属于当前文稿的目录，避免打开别的文稿的成品。
        let stem = export::document_stem_prefix(
            &self.doc.draft,
            &export::extract_title(&self.doc.generated_markdown, &self.doc.draft.title_hint),
        );
        self.export_links
            .refresh(&self.config.output_dir, Some(&stem));
        let mut action = None;
        for kind in ExportKind::ALL {
            let path = self.export_links.path(kind).map(Path::to_path_buf);
            let lit = path.is_some();
            let label = kind.label();
            let tint = if lit { ACCENT } else { theme::TEXT_MUTED };
            let response = ui.add_enabled(
                lit,
                egui::Button::image(kind.icon().image_sized(20.0).tint(tint))
                    .image_tint_follows_text_color(false)
                    .min_size(egui::vec2(34.0, 26.0))
                    .corner_radius(egui::CornerRadius::same(6)),
            );
            let Some(path) = path else {
                response.on_hover_text(format!("导出目录里还没有 {label} 文件"));
                continue;
            };
            let response =
                response.on_hover_text(format!("打开最近导出的 {label}：{}", path.display()));
            response.context_menu(|ui| {
                if ui.button("在文件管理器中定位").clicked() {
                    action = Some(FileAction::Reveal(path.clone()));
                    ui.close();
                }
            });
            if response.clicked() {
                action = Some(FileAction::Open(path));
            }
        }
        if let Some(action) = action {
            self.run_file_action(action);
        }
    }

    pub(crate) fn run_file_action(&mut self, action: FileAction) {
        let (result, path, verb) = match action {
            FileAction::Open(path) => (open_in_os(&path), path, "打开"),
            FileAction::Reveal(path) => (reveal_in_os(&path), path, "定位"),
        };
        match result {
            Ok(()) => {
                *self.status = format!(
                    "已{verb} {}。",
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("文件")
                )
            }
            Err(error) => *self.status = format!("{verb}失败：{error}"),
        }
    }

    pub(crate) fn create_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("draft_toolbar")
            .frame(theme::panel(theme::SURFACE, 10))
            .show(ui, |ui| self.toolbar(ui));
        if self.doc.versions_open {
            egui::Panel::right("draft_versions")
                .default_size(300.0)
                .size_range(240.0..=460.0)
                .frame(theme::panel(theme::CANVAS, 12))
                .show(ui, |ui| self.versions_drawer(ui));
        }
        if self.doc.result_drawer_open {
            // 新 ID：旧 ID 上持久化的是"底部抽屉"的外框矩形，沿用会让右侧抽屉按那条横条的
            // 位置排版，内容整体左移到中央区底下。换个 ID 直接丢掉那份旧状态。
            egui::Panel::right("review_result_drawer_right_v1")
                .default_size(300.0)
                .size_range(240.0..=460.0)
                .frame(theme::panel(theme::CANVAS, 12))
                .show(ui, |ui| self.result_drawer_ui(ui));
        }
        if !self.doc.form_collapsed {
            // 新 ID 用于丢弃旧版本持久化的超宽面板尺寸，让紧凑默认值立即生效。
            egui::Panel::left("create_form_compact_v3")
                .default_size(FORM_PANEL_DEFAULT_WIDTH)
                .size_range(FORM_PANEL_MIN_WIDTH..=FORM_PANEL_MAX_WIDTH)
                .frame(theme::panel(theme::CANVAS, 12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("公文要素填报");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, theme::Icon::PanelClose, "收起公文要素填报区")
                                .on_hover_text("收起左侧填报区，扩大审校稿空间")
                                .clicked()
                            {
                                self.doc.form_collapsed = true;
                            }
                        });
                    });
                    ui.separator();
                    // 必须在进入纵向 ScrollArea 前取得有限宽度；滚动内容内部的
                    // available_width 首帧可能为无穷大，会反向把侧栏撑到最大值。
                    let form_content_width =
                        (ui.available_width() - 18.0).max(FORM_CONTENT_MIN_WIDTH);
                    let editable = !self.doc.read_only();
                    egui::ScrollArea::vertical()
                        .id_salt("form_scroll")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.add_enabled_ui(editable, |ui| self.form_ui(ui, form_content_width));
                        });
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.read_only_banner(ui);
                if let Some(loaded) = &self.doc.loaded_version
                    && self.doc.manuscript_id == Some(loaded.manuscript_id)
                {
                    ui.group(|ui| {
                        ui.colored_label(
                            ACCENT,
                            format!(
                                "当前编辑内容来自版本 v{}《{}》，提交版本将追加为新版本。",
                                loaded.version_number, loaded.name
                            ),
                        );
                    });
                    ui.add_space(6.0);
                }
                self.preview_ui(ui);
            });
    }

    /// 只读稿件的顶部横幅。发布件可以就地退回草稿继续改，归档件只作说明。
    fn read_only_banner(&mut self, ui: &mut egui::Ui) {
        if !self.doc.read_only() {
            return;
        }
        let published = self.doc.record_status == ManuscriptStatus::Published;
        ui.horizontal(|ui| {
            ui.colored_label(
                WARN,
                if published {
                    "这篇已发布，当前为只读。可预览、导出、看版本，但不能改。"
                } else {
                    "这篇已归档，归档稿不可修改。可预览、导出、查看版本历史。"
                },
            );
            if published
                && let Some(id) = self.doc.manuscript_id
                && ui
                    .add(theme::icon_text_button(theme::Icon::Undo, "退回草稿并编辑"))
                    .clicked()
            {
                self.actions.push(DraftAction::RevertToDraft(id));
            }
        });
        ui.add_space(6.0);
    }

    /// 右侧版本抽屉：本篇的版本链在起草页里就地看完，不再跳去稿件管理。
    fn versions_drawer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("版本历史");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, theme::Icon::X, "收起版本历史").clicked() {
                    self.doc.versions_open = false;
                }
            });
        });
        ui.separator();
        let Some(id) = self.doc.manuscript_id else {
            ui.weak("这篇还没保存到稿件库，先点“保存”。");
            return;
        };
        let versions = self.draft_version_rows();
        if versions.is_empty() {
            ui.weak("还没有提交过版本。点工具栏的“提交版本”固化当前内容。");
            return;
        }
        let current = self.current_version_target();
        let mut load: Option<i64> = None;
        let mut diff: Option<i64> = None;
        egui::ScrollArea::vertical()
            .id_salt("draft_versions_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 最新版在最上：回看历史通常从近往远找。
                for row in versions.iter().rev() {
                    let active = current == VersionTarget::Version(row.version_number);
                    let frame = if active {
                        theme::card().fill(theme::ACCENT_SOFT)
                    } else {
                        theme::card()
                    };
                    frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("v{}", row.version_number)).strong(),
                            );
                            ui.label(truncate(&row.name, 14));
                            if row.is_latest {
                                theme::chip(ui, "最新", theme::SUCCESS, theme::SUCCESS_SOFT);
                            }
                        });
                        if !row.comment.trim().is_empty() {
                            ui.weak(summarize(&row.comment, 40));
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!active, egui::Button::new("载入编辑").small())
                                .on_hover_text("把这一版内容载入起草页继续改；提交会追加为新版本")
                                .clicked()
                            {
                                load = Some(row.version_number);
                            }
                            if ui
                                .add(egui::Button::new("与上一版对照").small())
                                .on_hover_text(version_hover(row))
                                .clicked()
                            {
                                diff = Some(row.version_number);
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        if let Some(version_number) = load {
            self.request_version_switch(VersionTarget::Version(version_number));
        }
        if let Some(to) = diff {
            self.actions.push(DraftAction::OpenVersionDiff {
                manuscript_id: id,
                to,
            });
        }
    }

    /// 标签栏下方的统一工具栏。这一条把原来分散在底部操作栏和审校稿标题栏里的
    /// 按钮全部收拢：左起是要素开关、视图切换、编辑动作、AI、存稿与版本、导出，
    /// 末尾是审校结果状态。宽度不够时整体折行——右对齐一挤就重叠，不用。
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        let saved = self.doc.manuscript_id.is_some();
        // 只读稿件（已发布、已归档）不能改内容，但照常预览、导出、看版本。
        let editable = !self.doc.read_only();
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal_wrapped(|ui| {
                // 一、公文要素填报的开关
                let tip = if self.doc.form_collapsed {
                    "展开公文要素填报区"
                } else {
                    "收起公文要素填报区"
                };
                let icon = if self.doc.form_collapsed {
                    theme::Icon::PanelOpen
                } else {
                    theme::Icon::PanelClose
                };
                if theme::nav_button(ui, !self.doc.form_collapsed, icon, "公文要素")
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.doc.form_collapsed = !self.doc.form_collapsed;
                }
                toolbar_separator(ui);

                // 二、稿件本身的编辑动作
                if theme::icon_button(ui, theme::Icon::SearchClear, "查找和替换（Ctrl+F）")
                    .clicked()
                {
                    self.doc.markdown_find.open = true;
                    self.doc.markdown_find.focus_query = true;
                }
                if theme::nav_button(
                    ui,
                    self.doc.mark_toolbar_visible,
                    theme::Icon::BookmarkCheck,
                    "标记",
                )
                .on_hover_text(
                    "显示/隐藏审校区的特殊标记符号栏（快速插入正文标记、附件标记）",
                )
                .clicked()
                {
                    self.doc.mark_toolbar_visible = !self.doc.mark_toolbar_visible;
                }
                if theme::icon_button_enabled(ui, has_draft, theme::Icon::Copy, "复制全文")
                    .clicked()
                {
                    ui.ctx().copy_text(self.doc.generated_markdown.clone());
                    *self.status = "审校稿已复制到剪贴板。".into();
                }
                if theme::danger_icon_button_enabled(
                    ui,
                    has_draft && editable,
                    theme::Icon::Trash,
                    "清空审校稿",
                )
                .clicked()
                {
                    self.doc.generated_markdown.clear();
                    self.doc.warnings.clear();
                    self.doc.output_files.clear();
                    self.doc.export_error = None;
                }
                if theme::icon_button_enabled(ui, has_draft, theme::Icon::Refresh, "重新校验")
                    .clicked()
                {
                    self.revalidate();
                    if !self.doc.warnings.is_empty() {
                        self.open_result_drawer();
                    }
                    *self.status = "已重新执行规则校验。".into();
                }
                toolbar_separator(ui);

                // 四、AI：有稿件是优化，没稿件是从零起草
                if theme::primary_icon_button_enabled(
                    ui,
                    !self.doc.busy && editable,
                    theme::Icon::Sparkles,
                    if has_draft { "AI 优化" } else { "AI 起草" },
                )
                .on_hover_text(if has_draft {
                    "选一条提示词改写当前稿件，也可临时写一条；输出格式标准内置生效，不会破坏导出结构"
                } else {
                    "审校稿为空：在面板里写明要起草什么，结合左侧公文要素从零生成"
                })
                .clicked()
                {
                    self.actions.push(DraftAction::OpenAiPromptPicker);
                }
                // RAG 开关：仅起草（审校稿为空）有意义。勾选后起草时自动检索知识库
                // 相似稿件片段注入提示词作参考。
                //
                // 全局开关关着时置灰而不是照常可勾——否则勾了也什么都不会发生，
                // 界面上还没有任何提示，只会让人以为功能坏了。
                if editable && !has_draft {
                    let rag_enabled = self.config.rag.enabled;
                    ui.add_enabled_ui(rag_enabled, |ui| {
                        ui.checkbox(&mut self.doc.use_knowledge_rag, "参考知识库")
                            .on_hover_text(if rag_enabled {
                                "起草时自动检索知识库中的相似稿件片段，注入提示词作写作风格参考"
                            } else {
                                "知识库检索增强尚未启用：请到「设置 → 知识库」勾选启用，并配置 embedding 模型"
                            });
                    });
                    if rag_enabled && self.doc.use_knowledge_rag {
                        egui::ComboBox::from_id_salt("rag_kind_filter")
                            .selected_text(self.doc.rag_kind_filter.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.doc.rag_kind_filter,
                                    RagKindFilter::Follow,
                                    RagKindFilter::Follow.label(),
                                );
                                ui.selectable_value(
                                    &mut self.doc.rag_kind_filter,
                                    RagKindFilter::All,
                                    RagKindFilter::All.label(),
                                );
                                for kind in TemplateKind::ALL {
                                    ui.selectable_value(
                                        &mut self.doc.rag_kind_filter,
                                        RagKindFilter::Only(kind),
                                        kind.label(),
                                    );
                                }
                            });
                    }
                }
                toolbar_separator(ui);

                // 五、入库与版本
                if ui
                    .add_enabled(
                        editable,
                        theme::icon_text_button(theme::Icon::Save, "保存"),
                    )
                    .on_hover_text(if saved {
                        "更新稿件库中已打开的那条记录（Ctrl+S）"
                    } else {
                        "保存为稿件库中的一条新草稿记录（Ctrl+S）"
                    })
                    .clicked()
                {
                    self.actions.push(DraftAction::SaveToLibrary);
                }
                if ui
                    .add_enabled(
                        saved && editable,
                        theme::icon_text_button(theme::Icon::GitCommit, "提交版本"),
                    )
                    .on_hover_text(if saved {
                        "把当前内容固化为一个新版本（需相对上一版本有变更）"
                    } else {
                        "先“保存”，再提交版本"
                    })
                    .clicked()
                    && let Some(id) = self.doc.manuscript_id
                {
                    self.actions
                        .push(DraftAction::OpenVersionCommit(VersionScope::Manuscript(id)));
                }
                self.version_switch_picker(ui);
                toolbar_separator(ui);

                // 六、导出。格式与自动导出在设置页统一管理，这里只管出文件。
                if ui
                    .add_enabled(
                        !self.doc.busy && has_draft,
                        theme::secondary_icon_button(theme::Icon::FileDown, "导出"),
                    )
                    .on_hover_text("按设置里勾选的格式导出当前审校稿，可反复导出")
                    .clicked()
                {
                    self.start_export_current();
                }
                if theme::icon_button(ui, theme::Icon::Folder, "打开输出目录")
                    .on_hover_text(self.config.output_dir.clone())
                    .clicked()
                {
                    self.open_output_dir();
                }
                self.export_open_buttons(ui);
                // 缩放只作用于版式预览；源码与版本对照按界面字号显示。
                if matches!(
                    self.doc.preview_mode,
                    PreviewMode::Rendered | PreviewMode::Split
                ) {
                    toolbar_separator(ui);
                    self.zoom_controls(ui);
                }

                // 视图入口仿 Zed 收到最右侧，只保留稳定的图标锚点。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (mode, icon, label, tip) in [
                        (
                            PreviewMode::VersionDiff,
                            theme::Icon::GitCommit,
                            "版本对照",
                            "版本对照：最新提交版本与当前修订逐字比较",
                        ),
                        (
                            PreviewMode::Split,
                            theme::Icon::Compare,
                            "对照",
                            "对照：左边改 Markdown，右边看公文版式",
                        ),
                        (
                            PreviewMode::Rendered,
                            theme::Icon::Eye,
                            "公文预览",
                            "公文预览：按导出后的字体与行距排版",
                        ),
                        (
                            PreviewMode::Hybrid,
                            theme::Icon::Book,
                            "实时排版",
                            "实时排版：当前行显示 Markdown 标记，离开后按公文格式渲染",
                        ),
                        (
                            PreviewMode::Source,
                            theme::Icon::PencilLine,
                            "Markdown",
                            "Markdown：带语法高亮的源码，导出以此为准",
                        ),
                    ] {
                        if theme::view_icon_button(
                            ui,
                            self.doc.preview_mode == mode,
                            icon,
                            label,
                        )
                        .on_hover_text(tip)
                        .clicked()
                        {
                            self.doc.preview_mode = mode;
                        }
                    }
                });
            });
        });
    }

    pub(crate) fn form_ui(&mut self, ui: &mut egui::Ui, available_width: f32) {
        // 左侧是输入表单，不使用表格的交替行底色。所有可交互控件统一为白色表面和
        // 相同最小高度，避免下拉框与条纹背景融在一起，也避免“手填”按钮上下跳动。
        {
            let style = ui.style_mut();
            style.spacing.interact_size.y = FORM_CONTROL_HEIGHT;
            style.visuals.widgets.inactive.bg_fill = theme::SURFACE;
            style.visuals.widgets.inactive.weak_bg_fill = theme::SURFACE;
        }
        let free_text = self.config.allow_free_text;
        // 单位共用一份名录：发文、主送、抄送、承办、落款都从这里选，一律按层级缩进显示。
        let external_names = self.doc.draft.kind == TemplateKind::OfficialLetter
            && self.doc.draft.profile.correspondence_scope == CorrespondenceScope::External;
        let unit_pool = self.unit_pool(external_names);
        let units = layout_options(&unit_pool, None);
        // 机关代字是单位的属性，候选项就是词库里各单位绑定过的代字。
        let department_codes = plain_options(&units::department_codes(&self.config.vocabulary));
        // 规格 §2.4：一个单位只能占发文、主送、抄送三者之一，勾选时互相禁用。
        let picked_as_recipient = split_units(&self.doc.draft.profile.recipient);
        let picked_as_copy = split_units(&self.doc.draft.profile.copies_to);
        let picked_as_issuing =
            if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                split_units(&self.doc.draft.profile.joint_issuing_units)
            } else {
                split_units(&self.doc.draft.profile.issuing_unit)
            };
        let blocked_for_recipient = [picked_as_copy.clone(), picked_as_issuing.clone()].concat();
        let blocked_for_copies = [picked_as_recipient.clone(), picked_as_issuing].concat();
        // 规格 §2.3“人随事走”：联系人按承办单位过滤，呈报领导按落款单位过滤。
        let filter_units = match self.doc.draft.kind {
            TemplateKind::OfficialLetter
                if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 =>
            {
                split_units(&self.doc.draft.profile.joint_responsible_units)
            }
            TemplateKind::OfficialLetter => vec![self.doc.draft.profile.responsible_unit.clone()],
            TemplateKind::WhitePaper => vec![self.doc.draft.profile.signing_unit.clone()],
            _ => vec![],
        };
        let (contacts, people_filtered) = self.filtered_contacts(
            &filter_units,
            self.doc.draft.kind == TemplateKind::OfficialLetter,
        );
        let people_filter_note = if people_filtered {
            match self.doc.draft.kind {
                TemplateKind::OfficialLetter
                    if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 =>
                {
                    Some("联系人已按所选承办单位过滤。".to_string())
                }
                TemplateKind::OfficialLetter => Some(format!(
                    "联系人已按承办单位“{}”过滤。",
                    self.doc.draft.profile.responsible_unit.trim()
                )),
                TemplateKind::WhitePaper => Some(format!(
                    "呈报领导已按落款单位“{}”过滤。",
                    self.doc.draft.profile.signing_unit.trim()
                )),
                _ => None,
            }
        } else {
            None
        };
        let mut joint_contact_names = join_units(
            &self
                .doc
                .draft
                .profile
                .joint_contacts
                .iter()
                .map(|contact| contact.name.clone())
                .collect::<Vec<_>>(),
        );
        let people = plain_options(
            &contacts
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>(),
        );
        let leader_options = contacts
            .iter()
            .map(|(name, _)| {
                let entry = self.config.vocabulary.iter().find(|entry| {
                    entry.category == VocabularyCategory::Person && entry.canonical.trim() == name
                });
                let position = entry.map(|entry| entry.position.trim()).unwrap_or_default();
                let code = entry.map(|entry| entry.code.trim()).unwrap_or_default();
                let full = if position.is_empty() {
                    name.clone()
                } else {
                    format!("{name} {position}")
                };
                let label = if code.is_empty() {
                    full.clone()
                } else {
                    format!("{code} {full}")
                };
                SelectOption {
                    value: name.clone(),
                    label,
                    full,
                    parent: String::new(),
                    depth: 0,
                }
            })
            .collect::<Vec<_>>();
        let rules = self.config.security_rules.clone();
        let content_width = available_width.clamp(*CONTENT_WIDTH.start(), *CONTENT_WIDTH.end());
        let field_width = (content_width - LABEL_WIDTH - TOGGLE_WIDTH - FORM_LAYOUT_GUTTER)
            .max(FORM_FIELD_MIN_WIDTH);

        ui.add_space(4.0);
        ui.heading("1. 文种与基础信息");
        ui.add_space(4.0);
        let old_kind = self.doc.draft.kind;
        egui::Grid::new("basic_grid")
            .num_columns(2)
            .striped(false)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                row_label_with_info(ui, "公文模板", self.doc.draft.kind.description());
                egui::ComboBox::from_id_salt("template_kind")
                    .selected_text(self.doc.draft.kind.label())
                    .width(field_width)
                    .show_ui(ui, |ui| {
                        for kind in TemplateKind::ALL {
                            ui.selectable_value(&mut self.doc.draft.kind, kind, kind.label());
                        }
                    });
                ui.end_row();

                if self.doc.draft.kind != TemplateKind::PlainDocument {
                    row_label_with_info(
                        ui,
                        "稿件版本",
                        match self.doc.draft.kind {
                            TemplateKind::OfficialLetter => {
                                "预览版自动留空函号流水号和成文日期的“日”；正式版使用填写值。"
                            }
                            TemplateKind::PhoneNotice | TemplateKind::WhitePaper => {
                                "预览版自动留空落款成文日期的“日”；正式版使用完整日期。"
                            }
                            TemplateKind::MeetingAgenda => {
                                "会议议程无落款日期，稿件版本仅用于与其他文种保持一致。"
                            }
                            TemplateKind::PlainDocument => unreachable!(),
                        },
                    );
                    egui::ComboBox::from_id_salt("letter_version")
                        .selected_text(self.doc.draft.profile.letter_version.label())
                        .width(field_width)
                        .show_ui(ui, |ui| {
                            for version in LetterVersion::ALL {
                                ui.selectable_value(
                                    &mut self.doc.draft.profile.letter_version,
                                    version,
                                    version.label(),
                                );
                            }
                        });
                    ui.end_row();
                }

                row_label_with_info(
                    ui,
                    "排版风格",
                    "紧缩风格会把正文中层级最深的标题与紧随其后的正文合并为一行，例如：一、任务目标。测试正文。",
                );
                egui::ComboBox::from_id_salt("style_mode")
                    .selected_text(self.doc.draft.profile.style_mode.label())
                    .width(field_width)
                    .show_ui(ui, |ui| {
                        for mode in StyleMode::ALL {
                            ui.selectable_value(
                                &mut self.doc.draft.profile.style_mode,
                                mode,
                                mode.label(),
                            );
                        }
                    });
                ui.end_row();

                row_label(ui, "标题提示");
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.title_hint)
                        .hint_text("可留空，由模型拟题")
                        .desired_width(field_width),
                );
                ui.end_row();
            });
        if self.doc.draft.kind != old_kind {
            self.doc.warnings = switch_template_profile(
                self.config,
                &mut self.doc.draft,
                &self.doc.generated_markdown,
                old_kind,
            );
            self.doc.output_files.clear();
            self.doc.export_error = None;
        }

        ui.add_space(10.0);
        section_heading_with_info(
            ui,
            "2. 行文要素",
            "带下拉框的字段一律从标准词库中选择；确需临时填写时点右侧“手填”切换。",
        );
        ui.add_space(4.0);
        egui::Grid::new("meta_grid")
            .num_columns(2)
            .striped(false)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| match self.doc.draft.kind {
                TemplateKind::OfficialLetter => {
                    row_label(ui, "行文范围");
                    egui::ComboBox::from_id_salt("correspondence_scope")
                        .selected_text(self.doc.draft.profile.correspondence_scope.label())
                        .width(field_width)
                        .show_ui(ui, |ui| {
                            for scope in CorrespondenceScope::ALL {
                                ui.selectable_value(
                                    &mut self.doc.draft.profile.correspondence_scope,
                                    scope,
                                    scope.label(),
                                );
                            }
                        });
                    ui.end_row();

                    row_label(ui, "发文方式");
                    egui::ComboBox::from_id_salt("joint_issuance_mode")
                        .selected_text(self.doc.draft.profile.joint_issuance_mode.label())
                        .width(field_width)
                        .show_ui(ui, |ui| {
                            for mode in JointIssuanceMode::ALL {
                                ui.selectable_value(
                                    &mut self.doc.draft.profile.joint_issuance_mode,
                                    mode,
                                    mode.label(),
                                );
                            }
                        });
                    ui.end_row();

                    row_label(ui, "发文单位");
                    if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                        multi_select(
                            ui,
                            "joint_issuing_units",
                            &mut self.doc.draft.profile.joint_issuing_units,
                            &units,
                            &[],
                            "",
                            &mut self.doc.manual_fields,
                            free_text,
                            field_width,
                        );
                    } else {
                        let changed = single_select(
                            ui,
                            "issuing_unit",
                            &mut self.doc.draft.profile.issuing_unit,
                            &units,
                            &mut self.doc.manual_fields,
                            free_text,
                            field_width,
                            "使用标准全称",
                        );
                        // 规格 §2.1：单位绑定机关代字时自动带出。
                        if changed && self.doc.draft.profile.department_code.trim().is_empty() {
                            let code = UnitDisplay::new(&self.config.vocabulary)
                                .department_code_of(&self.doc.draft.profile.issuing_unit);
                            if !code.is_empty() {
                                self.doc.draft.profile.department_code = code;
                            }
                        }
                    }
                    ui.end_row();

                    if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                        let selected_issuing = split_units(&self.doc.draft.profile.joint_issuing_units);
                        if !selected_issuing
                            .iter()
                            .any(|unit| unit == &self.doc.draft.profile.main_issuing_unit)
                        {
                            self.doc.draft.profile.main_issuing_unit =
                                selected_issuing.first().cloned().unwrap_or_default();
                        }
                        let issuing_preview = UnitDisplay::new(&self.config.vocabulary)
                            .join_hierarchical_for(&selected_issuing, external_names);
                        row_label_with_info(
                            ui,
                            "主发文\n单位",
                            if issuing_preview.is_empty() {
                                "用于确定红头主单位；选择联合发文单位后可在此指定。".to_string()
                            } else {
                                format!("用于确定红头主单位；落款将显示为：{issuing_preview}")
                            },
                        );
                        single_select(
                            ui,
                            "main_issuing_unit",
                            &mut self.doc.draft.profile.main_issuing_unit,
                            &layout_options(&unit_pool, Some(&selected_issuing)),
                            &mut self.doc.manual_fields,
                            false,
                            field_width,
                            "用于红头",
                        );
                        ui.end_row();
                    }

                    row_label(ui, "机关代字");
                    single_select(
                        ui,
                        "department_code",
                        &mut self.doc.draft.profile.department_code,
                        &department_codes,
                        &mut self.doc.manual_fields,
                        free_text,
                        field_width,
                        "例如：X政函",
                    );
                    ui.end_row();

                    row_label_with_info(
                        ui,
                        "发文年份",
                        "函号中的年度标识，独立保存于稿件元数据；旧稿会从成文日期兼容推导。",
                    );
                    ui.add(
                        egui::TextEdit::singleline(
                            &mut self.doc.draft.profile.document_year,
                        )
                        .hint_text("例如：2026")
                        .char_limit(4)
                        .desired_width(field_width),
                    );
                    ui.end_row();

                    row_label(ui, "发文序号");
                    ui.add_enabled(
                        self.doc.draft.profile.letter_version == LetterVersion::Formal,
                        egui::TextEdit::singleline(&mut self.doc.draft.profile.document_number)
                            .hint_text("仅填序号，例如：12")
                            .desired_width(field_width),
                    );
                    ui.end_row();

                    let recipient_preview = UnitDisplay::new(&self.config.vocabulary)
                        .join_hierarchical_for(
                            &split_units(&self.doc.draft.profile.recipient),
                            external_names,
                        );
                    row_label_with_info(
                        ui,
                        "主送单位",
                        if recipient_preview.is_empty() {
                            "可从标准词库多选；成文时会按单位层级整理名称。".to_string()
                        } else {
                            format!("成文时将显示为：{recipient_preview}")
                        },
                    );
                    multi_select(
                        ui,
                        "recipient",
                        &mut self.doc.draft.profile.recipient,
                        &units,
                        &blocked_for_recipient,
                        "已在发文单位或抄送单位中选择",
                        &mut self.doc.manual_fields,
                        free_text,
                        field_width,
                    );
                    ui.end_row();

                    let copies_preview = UnitDisplay::new(&self.config.vocabulary)
                        .join_hierarchical_for(
                            &split_units(&self.doc.draft.profile.copies_to),
                            external_names,
                        );
                    row_label_with_info(
                        ui,
                        "抄送单位",
                        if copies_preview.is_empty() {
                            "可从标准词库多选；已作为发文或主送单位的名称不能重复选择。"
                                .to_string()
                        } else {
                            format!("成文时将显示为：{copies_preview}")
                        },
                    );
                    multi_select(
                        ui,
                        "copies_to",
                        &mut self.doc.draft.profile.copies_to,
                        &units,
                        &blocked_for_copies,
                        "已在发文单位或主送单位中选择",
                        &mut self.doc.manual_fields,
                        free_text,
                        field_width,
                    );
                    ui.end_row();

                    row_label(ui, "承办单位");
                    if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                        multi_select(
                            ui,
                            "joint_responsible_units",
                            &mut self.doc.draft.profile.joint_responsible_units,
                            &units,
                            &[],
                            "",
                            &mut self.doc.manual_fields,
                            free_text,
                            field_width,
                        );
                    } else {
                        single_select(
                            ui,
                            "responsible_unit",
                            &mut self.doc.draft.profile.responsible_unit,
                            &units,
                            &mut self.doc.manual_fields,
                            free_text,
                            field_width,
                            "处室/部门标准名称",
                        );
                    }
                    ui.end_row();

                    let mut contact_tip = people_filter_note.clone().unwrap_or_else(|| {
                        "联系人从标准词库选择，并自动带出与姓名绑定的电话。".to_string()
                    });
                    if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1
                        && !joint_contact_names.trim().is_empty()
                    {
                        let phone_summary = split_units(&joint_contact_names)
                            .into_iter()
                            .map(|name| {
                                let phone = contacts
                                    .iter()
                                    .find(|(candidate, _)| candidate == &name)
                                    .map(|(_, phone)| phone.as_str())
                                    .unwrap_or("（未维护电话）");
                                format!("{name}：{phone}")
                            })
                            .collect::<Vec<_>>()
                            .join("；");
                        contact_tip.push_str(&format!(" 当前联系人及电话：{phone_summary}"));
                    }
                    row_label_with_info(ui, "联系人\n及电话", contact_tip);
                    if self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1 {
                        multi_select(
                            ui,
                            "joint_contacts",
                            &mut joint_contact_names,
                            &people,
                            &[],
                            "",
                            &mut self.doc.manual_fields,
                            false,
                            field_width,
                        );
                    } else {
                        contact_pair(
                            ui,
                            &mut self.doc.draft.profile.contact_person,
                            &mut self.doc.draft.profile.contact_phone,
                            &contacts,
                            &mut self.doc.manual_fields,
                            free_text,
                            field_width,
                        );
                    }
                    ui.end_row();
                }
                TemplateKind::PhoneNotice => {
                    row_label_with_info(
                        ui,
                        "发文单位",
                        "电话通知不设置机关代字、发文序号、抄送及承办联系版记。",
                    );
                    single_select(
                        ui,
                        "phone_notice_issuing_unit",
                        &mut self.doc.draft.profile.issuing_unit,
                        &units,
                        &mut self.doc.manual_fields,
                        free_text,
                        field_width,
                        "使用标准全称",
                    );
                    ui.end_row();

                    row_label(ui, "主送单位");
                    multi_select(
                        ui,
                        "phone_notice_recipient",
                        &mut self.doc.draft.profile.recipient,
                        &units,
                        &[],
                        "",
                        &mut self.doc.manual_fields,
                        free_text,
                        field_width,
                    );
                    ui.end_row();

                }
                TemplateKind::PlainDocument => {
                    ui.label("");
                    ui.weak("普通公文不设置红头、发文单位、主送单位、落款和版记。");
                    ui.end_row();
                }
                TemplateKind::MeetingAgenda => {
                    row_label_with_info(
                        ui,
                        "会议时间",
                        "会议时间、地点和参加人员均可留空；模型将从写作素材中提取，依据不足时标记待核实。",
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.doc.draft.meeting_time)
                            .hint_text("例如：2026年8月5日（星期三）14:30")
                            .desired_width(field_width),
                    );
                    ui.end_row();

                    row_label(ui, "会议地点");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.doc.draft.profile.meeting_location)
                            .hint_text("会议室或线上会议")
                            .desired_width(field_width),
                    );
                    ui.end_row();

                    row_label(ui, "参加人员");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.doc.draft.attendees)
                            .hint_text("人员/单位及范围")
                            .desired_width(field_width),
                    );
                    ui.end_row();

                }
                TemplateKind::WhitePaper => {
                    let leaders_display = UnitDisplay::new(&self.config.vocabulary)
                        .reporting_leaders(&self.doc.draft.profile.reporting_leaders);
                    let mut leader_tip = people_filter_note.clone().unwrap_or_else(|| {
                        "呈报领导从标准词库中的人员条目选择。".to_string()
                    });
                    if !leaders_display.is_empty() {
                        leader_tip.push_str(&format!(" 成文称谓：{leaders_display}"));
                    }
                    row_label_with_info(ui, "呈报领导", leader_tip);
                    multi_select(
                        ui,
                        "reporting_leaders",
                        &mut self.doc.draft.profile.reporting_leaders,
                        &leader_options,
                        &[],
                        "",
                        &mut self.doc.manual_fields,
                        free_text,
                        field_width,
                    );
                    ui.end_row();

                    row_label(ui, "落款单位");
                    single_select(
                        ui,
                        "signing_unit",
                        &mut self.doc.draft.profile.signing_unit,
                        &units,
                        &mut self.doc.manual_fields,
                        free_text,
                        field_width,
                        "标准名称",
                    );
                    ui.end_row();
                }
            });

        if self.doc.draft.kind == TemplateKind::OfficialLetter
            && self.doc.draft.profile.joint_issuance_mode == JointIssuanceMode::Mode1
        {
            let previous = self.doc.draft.profile.joint_contacts.clone();
            self.doc.draft.profile.joint_contacts = split_units(&joint_contact_names)
                .into_iter()
                .map(|name| {
                    let phone = contacts
                        .iter()
                        .find(|(candidate, _)| candidate == &name)
                        .map(|(_, phone)| phone.clone())
                        .or_else(|| {
                            previous
                                .iter()
                                .find(|contact| contact.name == name)
                                .map(|contact| contact.phone.clone())
                        })
                        .unwrap_or_default();
                    JointContact { name, phone }
                })
                .collect();
        }

        ui.add_space(10.0);
        ui.heading(if self.doc.draft.kind == TemplateKind::PlainDocument {
            "3. 密级与打印"
        } else {
            "3. 密级与成文日期"
        });
        ui.add_space(4.0);
        egui::Grid::new("security_grid")
            .num_columns(2)
            .striped(false)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                row_label(ui, "密级");
                let mut level = SecurityLevel::from_marking(&self.doc.draft.profile.security_level);
                let previous = level;
                egui::ComboBox::from_id_salt("security_level")
                    .selected_text(level.label())
                    .width(field_width)
                    .show_ui(ui, |ui| {
                        for option in SecurityLevel::ALL {
                            ui.selectable_value(&mut level, option, option.label());
                        }
                    });
                if level != previous {
                    self.doc.draft.profile.security_level = level.marking().to_string();
                    self.doc.draft.profile.security_period = rules.default_period(level);
                }
                ui.end_row();

                let max_period = rules.max_years(level);
                if let Some(max) = max_period {
                    row_label_with_info(
                        ui,
                        "保密期限",
                        format!(
                            "{}级公文的保密期限不得超过 {} 年；期限无法确定时选“长期”。",
                            level.label(),
                            max
                        ),
                    );
                } else {
                    row_label(ui, "保密期限");
                }
                let options = rules.period_options(level);
                if options.is_empty() {
                    self.doc.draft.profile.security_period.clear();
                    ui.weak("不标注密级时无需填写保密期限");
                } else {
                    let period = &mut self.doc.draft.profile.security_period;
                    egui::ComboBox::from_id_salt("security_period")
                        .selected_text(if period.trim().is_empty() {
                            "（未选择）"
                        } else {
                            period.as_str()
                        })
                        .width(field_width)
                        .show_ui(ui, |ui| {
                            for option in &options {
                                if ui
                                    .selectable_label(period.as_str() == option, option)
                                    .clicked()
                                {
                                    *period = option.clone();
                                }
                            }
                        });
                }
                ui.end_row();

                if self.doc.draft.kind != TemplateKind::PlainDocument {
                    row_label_with_info(
                        ui,
                        "指人专办",
                        "启用后在密级后空一个全角空格，并以黑体标注“指人专办”；未选择密级时不可用。",
                    );
                    let enabled = level != SecurityLevel::Unmarked;
                    ui.add_enabled(
                        enabled,
                        egui::Checkbox::new(&mut self.doc.draft.profile.special_handling, "启用"),
                    );
                    ui.end_row();
                }

                if let Some(message) = rules.check(level, &self.doc.draft.profile.security_period) {
                    ui.label("");
                    ui.add_sized(
                        [field_width, 20.0],
                        egui::Label::new(egui::RichText::new(message).color(WARN))
                            .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                    ui.end_row();
                }

                if self.doc.draft.kind != TemplateKind::PlainDocument {
                    row_label(ui, "成文日期");
                    ui.horizontal(|ui| {
                        let enabled = !self.doc.draft.date_is_auto;
                        ui.add_enabled(
                            enabled,
                            egui::TextEdit::singleline(&mut self.doc.draft.date)
                                .hint_text("例如：2026年8月5日")
                                .desired_width((field_width - 96.0).max(110.0)),
                        );
                        ui.checkbox(&mut self.doc.draft.date_is_auto, "本机日期")
                            .on_hover_text("勾选后每次生成都按本机当天日期刷新");
                    });
                    ui.end_row();
                }

                if self.doc.draft.kind.uses_letter_layout() {
                    row_label(ui, "打印方式");
                    ui.checkbox(&mut self.doc.draft.profile.duplex_printing, "双面打印")
                        .on_hover_text(
                            "勾选后页码按装订外侧排布：奇数页靠右、偶数页靠左；不勾选时页码居中",
                        );
                    ui.end_row();
                }
            });

        ui.add_space(6.0);
        if ui
            .add(theme::icon_text_button(
                theme::Icon::BookmarkCheck,
                "保存为该模板默认值",
            ))
            .on_hover_text("下次打开本模板时自动带出以上要素")
            .clicked()
        {
            self.actions.push(DraftAction::Persist);
        }

        if self.doc.draft.kind == TemplateKind::MeetingAgenda {
            ui.collapsing("查看会议议程固定 Markdown 格式", |ui| {
                ui.monospace(prompt::MEETING_AGENDA_MARKDOWN_TEMPLATE);
            });
        }
        ui.add_space(8.0);
    }

    /// 审校区的特殊标记符号栏：一排快速插入按钮，把 `<!-- [正文] -->`、
    /// `<!-- [附件] -->` 这类区段标记按独占一行插到编辑器光标所在行首，
    /// 没有光标时追加到文末。右侧的收起按钮可整条隐藏，工具栏“标记”按钮再唤出。
    fn mark_toolbar_ui(&mut self, ui: &mut egui::Ui) {
        let editable = !self.doc.read_only();
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                ui.strong("特殊标记");
                ui.add_enabled_ui(editable, |ui| {
                    for (label, marker, tip) in [
                        (
                            "正文标记",
                            "<!-- [正文] -->",
                            "在光标处插入“<!-- [正文] -->”，声明正式标题之后的正文区段起点",
                        ),
                        (
                            "附件标记",
                            "<!-- [附件] -->",
                            "在光标处插入“<!-- [附件] -->”，把其后内容切换为附件区段",
                        ),
                    ] {
                        if ui.button(label).on_hover_text(tip).clicked() {
                            self.insert_section_marker(ui, marker, label);
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, theme::Icon::Collapse, "隐藏特殊标记栏")
                        .on_hover_text("隐藏特殊标记栏；需要时从工具栏的“标记”按钮再打开")
                        .clicked()
                    {
                        self.doc.mark_toolbar_visible = false;
                    }
                });
            });
        });
    }

    /// 把区段标记（`<!-- [正文] -->` 等）插入审校稿：编辑框有焦点时插到
    /// 光标所在行行首，否则追加到文末。标记必须独占一行导出器才认，
    /// 已存在同名标记时不再重复插入。
    fn insert_section_marker(&mut self, ui: &egui::Ui, marker: &str, label: &str) {
        if self.doc.read_only() {
            return;
        }
        if self
            .doc
            .generated_markdown
            .lines()
            .any(|line| line.trim() == marker)
        {
            *self.status = format!("{label}已在稿中，不重复插入。");
            return;
        }
        let cursor_char = if ui.ctx().memory(|memory| memory.has_focus(editor_id())) {
            egui::TextEdit::load_state(ui.ctx(), editor_id())
                .and_then(|state| state.cursor.char_range())
                .map(|range| range.primary.index.0)
        } else {
            None
        };
        let text = &mut self.doc.generated_markdown;
        let pos = match cursor_char {
            Some(char_index) => text
                .char_indices()
                .nth(char_index)
                .map_or(text.len(), |(byte, _)| byte),
            None => text.len(),
        };
        let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |i| line_start + i);
        let line_empty = text[line_start..line_end].trim().is_empty();
        let insertion = if line_empty {
            format!("{marker}\n")
        } else {
            format!("\n{marker}\n")
        };
        text.insert_str(line_start, &insertion);
        *self.status = format!("已插入{label}。");
        // 让编辑框下一次绘制时把光标挪到插入内容之后并滚动到位。
        self.doc.pending_source_jump = Some(line_start + insertion.len());
    }

    pub(crate) fn preview_ui(&mut self, ui: &mut egui::Ui) {
        if self.doc.markdown_find.open {
            egui::Panel::top("preview_find")
                .frame(theme::panel(theme::SURFACE, 12))
                .show(ui, |ui| self.markdown_find_ui(ui));
        }
        // 特殊标记符号栏：仅在有源码编辑器的视图（源码/实时排版/对照）里
        // 显示在编辑器上方，可整条隐藏。
        if self.doc.mark_toolbar_visible
            && matches!(
                self.doc.preview_mode,
                PreviewMode::Source | PreviewMode::Hybrid | PreviewMode::Split
            )
        {
            egui::Panel::top("preview_mark_toolbar")
                .frame(theme::panel(theme::SURFACE, 12))
                .show(ui, |ui| self.mark_toolbar_ui(ui));
        }

        egui::CentralPanel::default()
            .frame(theme::panel(theme::CANVAS, 10))
            .show(ui, |ui| match self.doc.preview_mode {
                PreviewMode::Source => self.markdown_editor(ui),
                PreviewMode::Hybrid => self.markdown_hybrid_editor(ui),
                PreviewMode::Rendered => self.markdown_render(ui),
                PreviewMode::VersionDiff => self.version_diff_mode_ui(ui),
                PreviewMode::Split => {
                    egui::Panel::left("preview_split")
                        .default_size(420.0)
                        .size_range(280.0..=900.0)
                        .frame(egui::Frame::new().inner_margin(egui::Margin {
                            right: 8,
                            ..egui::Margin::ZERO
                        }))
                        .show(ui, |ui| self.markdown_editor(ui));
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| self.markdown_render(ui));
                }
            });
    }

    pub(crate) fn result_drawer_ui(&mut self, ui: &mut egui::Ui) {
        // 标题与关闭按钮独占一行——右侧抽屉只有 300 点上下，挤不下一整排。
        ui.horizontal(|ui| {
            ui.strong(if self.doc.warnings.is_empty() {
                "审校提示".to_string()
            } else {
                format!("审校提示 {}", self.doc.warnings.len())
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, theme::Icon::X, "关闭审校提示（Esc）").clicked() {
                    self.doc.result_drawer_open = false;
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("warning_result_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                // 导出失败的原因原先挂在"导出文件"页上，那一页去掉后改挂这里，
                // 否则只剩状态栏一闪而过的一行，看不到全文。
                if let Some(error) = &self.doc.export_error {
                    egui::Frame::new()
                        .fill(theme::DANGER_SOFT)
                        .stroke(egui::Stroke::new(1.0, theme::DANGER.gamma_multiply(0.35)))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                egui::RichText::new("导出失败")
                                    .color(theme::DANGER)
                                    .strong(),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(error).color(theme::TEXT_SOFT),
                                )
                                .wrap_mode(egui::TextWrapMode::Wrap),
                            );
                        });
                    ui.add_space(8.0);
                }
                self.warnings_ui(ui);
            });
    }

    /// 审校区顶栏：显示方式切换、缩放、以及作用于当前稿件的几个动作。
    /// VS Code 风格的紧凑查找/替换条：Enter / Shift+Enter 在命中间移动，
    /// 当前命中同时选中源码，并在公文预览中标亮它所在的版式块。
    pub(crate) fn markdown_find_ui(&mut self, ui: &mut egui::Ui) {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.close_markdown_find();
            return;
        }

        let match_result = markdown_matches_mode(
            &self.doc.generated_markdown,
            &self.doc.markdown_find.query,
            self.doc.markdown_find.case_sensitive,
            self.doc.markdown_find.regex,
        );
        let regex_error = match_result.as_ref().err().cloned();
        let matches = match_result.unwrap_or_default();
        if matches.is_empty() {
            self.doc.markdown_find.current = 0;
        } else {
            self.doc.markdown_find.current = self.doc.markdown_find.current.min(matches.len() - 1);
        }
        let anchored = self
            .doc
            .preview_anchor
            .as_ref()
            .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown));
        if !matches.is_empty() && anchored.as_ref() != matches.get(self.doc.markdown_find.current) {
            self.select_find_match(matches.get(self.doc.markdown_find.current).cloned());
        }

        let mut action = None;
        let mut query_changed = false;
        let mut close = false;
        ui.horizontal(|ui| {
            ui.strong("查找");
            let input_width = (ui.available_width() - 250.0).clamp(120.0, 300.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.doc.markdown_find.query)
                    .id(find_query_id())
                    .desired_width(input_width)
                    .hint_text("输入要查找的文字"),
            );
            if self.doc.markdown_find.focus_query {
                response.request_focus();
                self.doc.markdown_find.focus_query = false;
            }
            query_changed = response.changed();
            if response.has_focus() {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter))
                {
                    action = Some(FindAction::Previous);
                } else if ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                {
                    action = Some(FindAction::Next);
                }
            }

            let count = if self.doc.markdown_find.query.is_empty() {
                "无结果".to_string()
            } else if matches.is_empty() {
                "0 个结果".to_string()
            } else {
                format!("{} / {}", self.doc.markdown_find.current + 1, matches.len())
            };
            ui.label(egui::RichText::new(count).color(theme::TEXT_MUTED));
            if theme::icon_button_enabled(
                ui,
                !matches.is_empty(),
                theme::Icon::ArrowUp,
                "上一个匹配（Shift+Enter）",
            )
            .clicked()
            {
                action = Some(FindAction::Previous);
            }
            if theme::icon_button_enabled(
                ui,
                !matches.is_empty(),
                theme::Icon::ArrowDown,
                "下一个匹配（Enter）",
            )
            .clicked()
            {
                action = Some(FindAction::Next);
            }
            let case_response = ui
                .add(
                    egui::Button::new("Aa")
                        .small()
                        .selected(self.doc.markdown_find.case_sensitive),
                )
                .on_hover_text("区分大小写");
            if case_response.clicked() {
                self.doc.markdown_find.case_sensitive = !self.doc.markdown_find.case_sensitive;
                query_changed = true;
            }
            let regex_response = ui
                .add(
                    egui::Button::new(".*")
                        .small()
                        .selected(self.doc.markdown_find.regex),
                )
                .on_hover_text("使用正则表达式；替换内容支持 $1 和 $name 捕获组");
            if regex_response.clicked() {
                self.doc.markdown_find.regex = !self.doc.markdown_find.regex;
                query_changed = true;
            }
            if theme::icon_button(ui, theme::Icon::X, "关闭查找（Esc）").clicked() {
                close = true;
            }
        });
        if let Some(error) = &regex_error {
            ui.colored_label(WARN, format!("正则表达式无效：{error}"));
        }
        ui.horizontal(|ui| {
            ui.strong("替换");
            let input_width = (ui.available_width() - 220.0).clamp(120.0, 300.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.doc.markdown_find.replacement)
                    .desired_width(input_width)
                    .hint_text("输入替换文字"),
            );
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("替换"))
                .clicked()
            {
                action = Some(FindAction::Replace);
            }
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("全部替换"))
                .clicked()
            {
                action = Some(FindAction::ReplaceAll);
            }
        });

        if close {
            self.close_markdown_find();
            return;
        }
        if query_changed {
            self.doc.markdown_find.current = 0;
            let refreshed = markdown_matches_mode(
                &self.doc.generated_markdown,
                &self.doc.markdown_find.query,
                self.doc.markdown_find.case_sensitive,
                self.doc.markdown_find.regex,
            )
            .unwrap_or_default();
            self.select_find_match(refreshed.first().cloned());
            return;
        }
        if let Some(action) = action {
            self.run_find_action(action, matches);
        }
    }

    pub(crate) fn close_markdown_find(&mut self) {
        self.doc.markdown_find.open = false;
        self.doc.pending_source_selection = None;
        self.doc.pending_render_jump = false;
        self.doc.preview_anchor = None;
    }

    pub(crate) fn select_find_match(&mut self, range: Option<Range<usize>>) {
        self.doc.pending_source_jump = None;
        let Some(range) = range else {
            self.doc.preview_anchor = None;
            self.doc.pending_source_selection = None;
            self.doc.pending_render_jump = false;
            return;
        };
        self.doc.pending_source_selection = Some(range.clone());
        self.doc.pending_render_jump = true;
        self.doc.preview_anchor =
            self.doc
                .generated_markdown
                .get(range.clone())
                .map(|text| PreviewAnchor {
                    range,
                    text: text.to_owned(),
                });
    }

    pub(crate) fn run_find_action(&mut self, action: FindAction, matches: Vec<Range<usize>>) {
        if matches.is_empty() {
            return;
        }
        match action {
            FindAction::Previous => {
                self.doc.markdown_find.current = if self.doc.markdown_find.current == 0 {
                    matches.len() - 1
                } else {
                    self.doc.markdown_find.current - 1
                };
                self.select_find_match(matches.get(self.doc.markdown_find.current).cloned());
            }
            FindAction::Next => {
                self.doc.markdown_find.current =
                    (self.doc.markdown_find.current + 1) % matches.len();
                self.select_find_match(matches.get(self.doc.markdown_find.current).cloned());
            }
            FindAction::Replace => {
                let range = matches[self.doc.markdown_find.current].clone();
                let replacement = self.find_replacement(&range);
                self.doc
                    .generated_markdown
                    .replace_range(range.clone(), &replacement);
                let next_offset = range.start + replacement.len();
                let refreshed = markdown_matches_mode(
                    &self.doc.generated_markdown,
                    &self.doc.markdown_find.query,
                    self.doc.markdown_find.case_sensitive,
                    self.doc.markdown_find.regex,
                )
                .unwrap_or_default();
                self.doc.markdown_find.current = refreshed
                    .iter()
                    .position(|candidate| candidate.start >= next_offset)
                    .unwrap_or(0);
                self.select_find_match(refreshed.get(self.doc.markdown_find.current).cloned());
                *self.status = "已替换当前匹配。".into();
            }
            FindAction::ReplaceAll => {
                let count = matches.len();
                if self.doc.markdown_find.regex {
                    let pattern = self.doc.markdown_find.query.clone();
                    if let Ok(regex) = regex::RegexBuilder::new(&pattern)
                        .case_insensitive(!self.doc.markdown_find.case_sensitive)
                        .build()
                    {
                        self.doc.generated_markdown = regex
                            .replace_all(
                                &self.doc.generated_markdown,
                                self.doc.markdown_find.replacement.as_str(),
                            )
                            .into_owned();
                    }
                } else {
                    for range in matches.into_iter().rev() {
                        self.doc
                            .generated_markdown
                            .replace_range(range, &self.doc.markdown_find.replacement);
                    }
                }
                self.doc.markdown_find.current = 0;
                self.select_find_match(None);
                *self.status = format!("已替换 {count} 处匹配。");
            }
        }
    }

    fn find_replacement(&self, range: &Range<usize>) -> String {
        expanded_replacement(
            &self.doc.generated_markdown,
            &self.doc.markdown_find.query,
            &self.doc.markdown_find.replacement,
            self.doc.markdown_find.case_sensitive,
            self.doc.markdown_find.regex,
            range,
        )
    }

    /// 预览缩放：默认按面板宽度自适应，也可以手动锁定倍率。
    /// 顶栏是从右往左排的，这里的添加顺序与视觉顺序相反：屏幕上读作
    /// “适应宽度、缩小、120%、放大”。
    pub(crate) fn zoom_controls(&mut self, ui: &mut egui::Ui) {
        // 自适应时以当前实际倍率为起点加减，避免首次放大反而变小。
        let current = self.doc.preview_zoom.unwrap_or(self.doc.preview_fit_scale);
        if theme::icon_button(ui, theme::Icon::ZoomIn, "放大").clicked() {
            self.doc.preview_zoom = Some((current + 0.1).min(2.0));
        }
        ui.label(egui::RichText::new(format!("{:.0}%", current * 100.0)).color(theme::TEXT_MUTED));
        if theme::icon_button(ui, theme::Icon::ZoomOut, "缩小").clicked() {
            self.doc.preview_zoom = Some((current - 0.1).max(0.4));
        }
        if theme::icon_button_enabled(
            ui,
            self.doc.preview_zoom.is_some(),
            theme::Icon::FitWidth,
            "适应宽度",
        )
        .on_hover_text("回到按窗格宽度自动缩放")
        .clicked()
        {
            self.doc.preview_zoom = None;
        }
        ui.separator();
    }

    /// Markdown 源码编辑框，带语法高亮。
    pub(crate) fn markdown_editor(&mut self, ui: &mut egui::Ui) {
        self.markdown_editor_impl(ui, false);
    }

    /// 实时公文排版编辑器：Markdown 始终是唯一数据源，只改变屏幕上的布局。
    pub(crate) fn markdown_hybrid_editor(&mut self, ui: &mut egui::Ui) {
        self.markdown_editor_impl(ui, true);
    }

    fn markdown_editor_impl(&mut self, ui: &mut egui::Ui, hybrid: bool) {
        // 行数必须在进入 ScrollArea 之前算：滚动方向上的 available_height
        // 是无穷大，拿进去算会得到 usize::MAX 行，整个界面将无法布局。
        let rows = visible_rows(ui);
        // 拆开借用：编辑框要可变借文本，布局器要可变借高亮缓存。
        let jump = self.doc.pending_source_jump.take();
        let selection = self.doc.pending_source_selection.take();
        let anchor = self
            .doc
            .preview_anchor
            .as_ref()
            .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown));
        let search_matches = if self.doc.markdown_find.open {
            markdown_matches_mode(
                &self.doc.generated_markdown,
                &self.doc.markdown_find.query,
                self.doc.markdown_find.case_sensitive,
                self.doc.markdown_find.regex,
            )
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        let editable = !self.doc.read_only();
        let active_line =
            if hybrid && editable && ui.ctx().memory(|memory| memory.has_focus(editor_id())) {
                active_source_line(ui.ctx(), &self.doc.generated_markdown)
            } else {
                usize::MAX
            };
        let show_line_numbers = self.config.show_editor_line_numbers;
        let text = &mut self.doc.generated_markdown;
        let highlighter = &mut self.doc.highlighter;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
            if hybrid {
                highlighter.layout_hybrid(
                    ui,
                    buffer.as_str(),
                    wrap_width.min(OFFICIAL_EDITOR_CONTENT_WIDTH),
                    active_line,
                    anchor.as_ref(),
                    &search_matches,
                )
            } else {
                highlighter.layout(
                    ui,
                    buffer.as_str(),
                    wrap_width,
                    anchor.as_ref(),
                    &search_matches,
                )
            }
        };
        if hybrid {
            let viewport_width = ui.available_width();
            egui::ScrollArea::both()
                .id_salt("hybrid_editor_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let side_space = ((viewport_width - OFFICIAL_PAGE_WIDTH) * 0.5).max(18.0);
                    ui.horizontal_top(|ui| {
                        ui.add_space(side_space);
                        egui::Frame::new()
                            .fill(egui::Color32::WHITE)
                            .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG))
                            .shadow(egui::epaint::Shadow {
                                offset: [0, 3],
                                blur: 14,
                                spread: 0,
                                color: egui::Color32::from_black_alpha(42),
                            })
                            .inner_margin(egui::Margin::ZERO)
                            .show(ui, |ui| {
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.set_min_size(egui::vec2(
                                        OFFICIAL_PAGE_WIDTH,
                                        OFFICIAL_PAGE_HEIGHT,
                                    ));
                                    ui.set_max_width(OFFICIAL_PAGE_WIDTH);
                                    ui.add_space(OFFICIAL_PAGE_MARGIN_TOP);
                                    ui.horizontal_top(|ui| {
                                        let gutter = if show_line_numbers { 38.0 } else { 0.0 };
                                        ui.add_space((OFFICIAL_PAGE_MARGIN_LEFT - gutter).max(0.0));
                                        if show_line_numbers {
                                            ui.add_space(gutter);
                                        }
                                        let output = egui::TextEdit::multiline(text)
                                        .id(editor_id())
                                        .interactive(editable)
                                        .frame(egui::Frame::NONE)
                                        .margin(egui::Margin::ZERO)
                                        .code_editor()
                                        .layouter(&mut layouter)
                                        .desired_width(OFFICIAL_EDITOR_CONTENT_WIDTH)
                                        .desired_rows(rows)
                                        .hint_text(
                                            "生成结果将在这里显示，也可以直接粘贴已有稿件再导出……",
                                        )
                                        .show(ui);
                                        if show_line_numbers {
                                            paint_editor_line_numbers(ui, &output);
                                        }
                                        paint_hybrid_decorations(ui, &output, text, active_line);
                                        if let Some(range) = selection {
                                            select_source_range(ui, &output, text, range);
                                        } else if let Some(offset) = jump {
                                            jump_to_source(ui, &output, text, offset);
                                        }
                                        if output.cursor_range.is_some_and(|range| {
                                            source_line_at_char(text, range.primary.index.0)
                                                != active_line
                                        }) {
                                            ui.ctx().request_repaint();
                                        }
                                    });
                                });
                            });
                    });
                });
        } else {
            theme::card().show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("preview_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let mut show_editor = |ui: &mut egui::Ui| {
                            egui::TextEdit::multiline(text)
                                .id(editor_id())
                                .interactive(editable)
                                .frame(egui::Frame::NONE)
                                .code_editor()
                                .layouter(&mut layouter)
                                .desired_width(f32::INFINITY)
                                .desired_rows(rows)
                                .hint_text("生成结果将在这里显示，也可以直接粘贴已有稿件再导出……")
                                .show(ui)
                        };
                        let output = if show_line_numbers {
                            ui.horizontal_top(|ui| {
                                ui.add_space(38.0);
                                show_editor(ui)
                            })
                            .inner
                        } else {
                            show_editor(ui)
                        };
                        if show_line_numbers {
                            paint_editor_line_numbers(ui, &output);
                        }
                        if let Some(range) = selection {
                            select_source_range(ui, &output, text, range);
                        } else if let Some(offset) = jump {
                            jump_to_source(ui, &output, text, offset);
                        }
                    });
            });
        }
    }

    /// 公文版式预览。正文为空时也照排——红头、密级、文号、主送、落款这些
    /// 行文要素来自表单，填完就能先看版式。
    pub(crate) fn markdown_render(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::both()
            .id_salt("render_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let display = UnitDisplay::new(&self.config.vocabulary);
                let anchor = self
                    .doc
                    .preview_anchor
                    .as_ref()
                    .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown));
                let output = preview::official_preview(
                    ui,
                    &self.doc.draft,
                    &display,
                    &self.doc.generated_markdown,
                    self.doc.preview_zoom,
                    anchor.as_ref(),
                    self.doc.pending_render_jump,
                );
                self.doc.pending_render_jump = false;
                self.doc.preview_fit_scale = output.scale;
                // 点中版式上的某一块：源码里同步高亮，并把光标带过去。
                if let Some(range) = output.clicked {
                    self.doc.pending_source_selection = None;
                    self.doc.pending_source_jump = Some(range.start);
                    self.doc.preview_anchor =
                        self.doc
                            .generated_markdown
                            .get(range.clone())
                            .map(|text| PreviewAnchor {
                                range,
                                text: text.to_owned(),
                            });
                    ui.ctx().request_repaint();
                }
                ui.add_space(12.0);
            });
    }

    pub(crate) fn warnings_ui(&mut self, ui: &mut egui::Ui) {
        if self.doc.warnings.is_empty() {
            ui.horizontal(|ui| {
                theme::dot(ui, theme::SUCCESS);
                ui.add_space(2.0);
                ui.label(egui::RichText::new("暂无审校提示").color(theme::TEXT_MUTED));
            });
            return;
        }
        egui::Frame::new()
            .fill(theme::WARN_SOFT)
            .stroke(egui::Stroke::new(1.0, theme::WARN.gamma_multiply(0.35)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // 条数由抽屉标题写，这里不再重复一遍。
                for warning in &self.doc.warnings {
                    // 提示往往很长，必须显式换行，否则会把底部面板顶宽。
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("· {warning}")).color(theme::TEXT_SOFT),
                        )
                        .wrap_mode(egui::TextWrapMode::Wrap),
                    );
                }
            });
    }

    /// 起草页的"版本对照"模式：左侧是已提交的基准版本，右侧是当前未提交内容。
    /// 只读——点某处改动会切回 Markdown 模式并把光标定到那一段。
    pub(crate) fn version_diff_mode_ui(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.doc.manuscript_id else {
            self.diff_placeholder(
                ui,
                "这篇稿件还没有保存到稿件库。先点“保存到稿件库”，再提交一个版本，才有可对照的基准。",
            );
            return;
        };
        let versions = self.draft_version_rows();
        if versions.is_empty() {
            self.diff_placeholder(
                ui,
                "这篇稿件还没有提交过版本。点“提交版本”固化第一版之后，这里会逐字标出后续修改。",
            );
            return;
        }
        let latest = versions.last().map_or(1, |row| row.version_number);
        // 基准默认跟着最新版走：提交一版之后对照自动以新版为基准，不用手动换选。
        let base = self
            .doc
            .draft_diff
            .base
            .filter(|number| versions.iter().any(|row| row.version_number == *number))
            .unwrap_or(latest);

        let mut picked_base = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("基准版本");
            let base_label = versions
                .iter()
                .find(|row| row.version_number == base)
                .map_or_else(
                    || format!("v{base}"),
                    |row| {
                        format!(
                            "v{} · {}{}",
                            row.version_number,
                            row.name,
                            if row.is_latest { " · 最新" } else { "" }
                        )
                    },
                );
            egui::ComboBox::from_id_salt("draft_diff_base")
                .selected_text(base_label)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for row in versions.iter().rev() {
                        if ui
                            .selectable_label(
                                row.version_number == base,
                                format!(
                                    "v{} · {}{}",
                                    row.version_number,
                                    row.name,
                                    if row.is_latest { " · 最新" } else { "" }
                                ),
                            )
                            .on_hover_text(version_hover(row))
                            .clicked()
                        {
                            picked_base = Some(row.version_number);
                        }
                    }
                })
                .response
                .on_hover_text("换成更早的版本，可以看到从那一版至今累计改了什么");
            ui.label("→");
            ui.strong("当前未提交内容");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::RotateCcw,
                        "回退到基准版",
                    ))
                    .on_hover_text("用基准版内容覆盖当前稿件内容（需二次确认）")
                    .clicked()
                {
                    *self.revert_confirm = Some((id, base));
                }
            });
        });
        if let Some(number) = picked_base {
            self.doc.draft_diff.base = Some(number);
            self.doc.draft_diff.view.reset();
        }
        ui.separator();

        self.sync_draft_diff(id, base);
        let old_label = format!("v{base}（已提交）");
        let config = DiffViewConfig {
            old_label: &old_label,
            new_label: "当前（未提交）",
            allow_jump: true,
        };
        // 缓存与视图状态是同一个结构体的两个字段，分别借用互不冲突。
        let Some((_, report)) = &self.doc.draft_diff.cache else {
            return;
        };
        let action =
            diff_view::manuscript_diff_ui(ui, report, &mut self.doc.draft_diff.view, &config);
        if let Some(DiffViewAction::JumpToSource(range)) = action {
            self.jump_to_source(range);
        }
    }

    /// 版本对照模式下的空状态提示。
    pub(crate) fn diff_placeholder(&self, ui: &mut egui::Ui, message: &str) {
        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(message).color(theme::TEXT_MUTED))
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
    }

    /// 重算起草页对照结果，输入没变就复用上一帧的。这个模式每帧都要渲染，
    /// 长稿边打字边全量 diff 会掉帧。
    pub(crate) fn sync_draft_diff(&mut self, id: i64, base: i64) {
        let notes = self
            .store
            .as_deref()
            .and_then(|store| store.notes_of(id).ok())
            .flatten()
            .unwrap_or_default();
        let draft_json = serde_json::to_string(&self.doc.draft).unwrap_or_default();
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::hash::DefaultHasher::new();
            id.hash(&mut hasher);
            base.hash(&mut hasher);
            self.doc.generated_markdown.hash(&mut hasher);
            notes.hash(&mut hasher);
            draft_json.hash(&mut hasher);
            hasher.finish()
        };
        if self
            .doc
            .draft_diff
            .cache
            .as_ref()
            .is_some_and(|(cached, _)| *cached == key)
        {
            return;
        }
        let old = self
            .store
            .as_deref_mut()
            .and_then(|store| store.get_manuscript_version(id, base).ok())
            .flatten()
            .map(diff::ContentSnapshot::from)
            .unwrap_or_else(|| {
                diff::ContentSnapshot::new(DraftInput::default(), String::new(), String::new())
            });
        let new = diff::ContentSnapshot::new(
            self.doc.draft.clone(),
            self.doc.generated_markdown.clone(),
            notes,
        );
        self.doc.draft_diff.cache = Some((key, diff::manuscript_diff(&old, &new)));
    }

    /// 切回 Markdown 源码并把光标 / 选区定位到给定范围。
    pub(crate) fn jump_to_source(&mut self, range: Range<usize>) {
        self.doc.preview_mode = PreviewMode::Source;
        self.select_find_match(Some(range));
    }

    /// 起草页的版本切换下拉：紧跟"提交版本"，让"改完提交"与"回看旧版"在同一处闭合。
    pub(crate) fn version_switch_picker(&mut self, ui: &mut egui::Ui) {
        let versions = self.draft_version_rows();
        let enabled = self.doc.manuscript_id.is_some() && !versions.is_empty();
        let current = self.current_version_target();
        // 工具栏寸土寸金：这里只显示版本号，完整名称交给悬停说明。
        let label = match current {
            VersionTarget::Version(number) => format!("v{number}"),
            VersionTarget::Working => "未提交".to_string(),
        };
        let mut picked = None;
        ui.add_enabled_ui(enabled, |ui| {
            let response = egui::ComboBox::from_id_salt("draft_version_switch")
                .selected_text(label)
                .width(84.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(current == VersionTarget::Working, "当前未提交内容")
                        .on_hover_text("稿件库中这篇稿件的当前内容（尚未固化为版本）")
                        .clicked()
                    {
                        picked = Some(VersionTarget::Working);
                    }
                    ui.separator();
                    // 最新版在最上：回看历史时通常是从近往远找。
                    for row in versions.iter().rev() {
                        let text = format!(
                            "v{} · {}{}",
                            row.version_number,
                            row.name,
                            if row.is_latest { " · 最新" } else { "" }
                        );
                        if ui
                            .selectable_label(
                                current == VersionTarget::Version(row.version_number),
                                text,
                            )
                            .on_hover_text(version_hover(row))
                            .clicked()
                        {
                            picked = Some(VersionTarget::Version(row.version_number));
                        }
                    }
                })
                .response;
            if self.doc.manuscript_id.is_none() {
                response.on_hover_text("先“保存到稿件库”，才能切换版本");
            } else if versions.is_empty() {
                response.on_hover_text("这篇稿件还没有提交过版本，先点“提交版本”");
            } else {
                response.on_hover_text("切换起草页显示的版本；有未提交修改时会先问你怎么处置");
            }
        });
        if let Some(target) = picked {
            self.request_version_switch(target);
        }
    }

    /// 起草页当前稿件的版本列表；没打开稿件时为空。
    pub(crate) fn draft_version_rows(&mut self) -> Vec<manuscript::VersionRow> {
        let Some(id) = self.doc.manuscript_id else {
            return Vec::new();
        };
        self.store
            .as_deref_mut()
            .and_then(|store| store.list_manuscript_versions(id).ok())
            .unwrap_or_default()
    }

    /// 起草页内容当前对应哪个版本：载入过某版就是它，否则是活稿行。
    pub(crate) fn current_version_target(&self) -> VersionTarget {
        match (&self.doc.loaded_version, self.doc.manuscript_id) {
            (Some(loaded), Some(id)) if loaded.manuscript_id == id => {
                VersionTarget::Version(loaded.version_number)
            }
            _ => VersionTarget::Working,
        }
    }

    /// 请求切换版本：先看内存内容相对来源有没有改动，有就弹三选确认，没有就直接切。
    pub(crate) fn request_version_switch(&mut self, target: VersionTarget) {
        let Some(id) = self.doc.manuscript_id else {
            return;
        };
        let current = self.current_version_target();
        if current == target {
            return;
        }
        if self.draft_has_unsaved_edits(id) {
            let base_label = match current {
                VersionTarget::Version(number) => format!("v{number}"),
                VersionTarget::Working => "稿件库中的当前稿".to_string(),
            };
            *self.version_switch = Some(VersionSwitchPrompt {
                manuscript_id: id,
                target,
                base_label,
            });
        } else {
            self.apply_version_switch(id, target);
        }
    }

    /// 内存里的内容相对"它的来源"有没有改动：载入了某版就跟那一版比，否则跟活稿行比。
    /// 按来源比而不是一律跟最新版比，没动过手的切换才不会被反复追问。
    pub(crate) fn draft_has_unsaved_edits(&mut self, id: i64) -> bool {
        let origin = match self.current_version_target() {
            VersionTarget::Version(number) => self
                .store
                .as_deref_mut()
                .and_then(|store| store.get_manuscript_version(id, number).ok())
                .flatten()
                .map(|record| (record.snapshot, record.content_markdown)),
            VersionTarget::Working => self
                .store
                .as_deref()
                .and_then(|store| store.snapshot_of(id).ok())
                .flatten(),
        };
        let Some((snapshot, content)) = origin else {
            return false;
        };
        content != self.doc.generated_markdown
            || serde_json::to_string(&snapshot).ok() != serde_json::to_string(&self.doc.draft).ok()
    }

    pub(crate) fn apply_version_switch(&mut self, id: i64, target: VersionTarget) {
        match target {
            VersionTarget::Version(number) => {
                self.actions.push(DraftAction::LoadManuscriptVersion {
                    manuscript_id: id,
                    version_number: number,
                })
            }
            VersionTarget::Working => self.load_working_copy(id),
        }
    }

    /// 切回"当前未提交内容"：从活稿行重载。活稿行是最后一次"保存到稿件库"的结果，
    /// 也就是离开历史版本后唯一还能取回的工作区。
    pub(crate) fn load_working_copy(&mut self, id: i64) {
        let loaded = self
            .store
            .as_deref()
            .and_then(|store| store.snapshot_of(id).ok())
            .flatten();
        match loaded {
            Some((snapshot, content)) => {
                self.doc.draft = snapshot;
                self.doc.generated_markdown = content;
                self.doc.loaded_version = None;
                self.doc.output_files.clear();
                self.doc.export_error = None;
                self.doc.draft_diff.view.reset();
                self.revalidate();
                *self.status = "已切回未提交内容（稿件库中这篇稿件的当前内容）。".into();
            }
            None => *self.status = "稿件不存在或稿件库不可用。".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前缀匹配必须用 `-` 作分界：`关于X` 不应误匹配 `关于X的补充`，
    /// 同时要认得同分钟重复导出产生的 `stem-N` 编号变体。
    #[test]
    fn export_links_stem_matching_uses_dash_boundary() {
        let mut links = ExportLinks {
            stem: Some("普通公文-关于X".into()),
            ..ExportLinks::default()
        };
        let matches = |name: &str| {
            ExportLinks {
                stem: Some("普通公文-关于X".into()),
                ..ExportLinks::default()
            }
            .matches(std::ffi::OsStr::new(name))
        };
        assert!(matches("普通公文-关于X-202601011200"));
        assert!(matches("普通公文-关于X-2"));
        assert!(matches("普通公文-关于X"));
        // 摊在根目录的成品（不带文件夹）也认。
        assert!(matches("普通公文-关于X.pdf"));
        // 标题互为前缀时不能误匹配。
        assert!(!matches("普通公文-关于X的补充-202601011300"));
        assert!(!matches("普通公文-关于X的补充.pdf"));
        // 完全无关的文稿。
        assert!(!matches("电话通知-202601011300"));
        assert!(!matches("其他文件.txt"));
        // 未绑定文稿时全部认（退化为旧行为）。
        links.stem = None;
        assert!(links.matches(std::ffi::OsStr::new("电话通知-202601011300")));
    }

    /// 输出目录里混着当前文稿的多次导出与别的文稿的最新导出时，
    /// 三枚成品入口必须只认当前文稿最新时间戳文件夹里的文件，不能串到别的文稿。
    #[test]
    fn export_links_only_open_latest_dir_of_current_stem() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let stem = "XX〔2025〕12号-关于测试的通知";

        // 按时间顺序创建：当前文稿旧导出 → 当前文稿新导出 → 别的文稿最新导出。
        let a_old = dir.join(format!("{stem}-202601011000"));
        let a_new = dir.join(format!("{stem}-202601011100"));
        let b_latest = dir.join("YY〔2025〕3号-关于别的事-202601011200");
        for folder in [&a_old, &a_new, &b_latest] {
            std::fs::create_dir_all(folder).unwrap();
        }
        // 当前文稿旧导出只产 tex+pdf；新导出三格式齐全。
        touch_export(&a_old, "tex");
        touch_export(&a_old, "pdf");
        touch_export(&a_new, "tex");
        touch_export(&a_new, "pdf");
        touch_export(&a_new, "docx");
        // 别的文稿最新导出也有 pdf——旧实现会在这里取到它的 pdf/docx。
        touch_export(&b_latest, "pdf");
        touch_export(&b_latest, "docx");
        // 拉开目录修改时间，避免同一时间单位内排序不稳。
        std::thread::sleep(Duration::from_millis(20));

        let mut links = ExportLinks::default();
        links.refresh(dir.to_str().unwrap(), Some(stem));
        for kind in ExportKind::ALL {
            let path = links.path(kind).expect("当前文稿最新导出应点亮该格式按钮");
            assert_eq!(
                path.parent().map(Path::to_path_buf),
                Some(a_new.clone()),
                "{kind:?} 应指向当前文稿最新导出目录"
            );
        }
    }

    /// 当前文稿最新目录缺某种格式时，可以从同文稿更早的导出补齐，
    /// 但不能越过它去拿别的文稿的成品。
    #[test]
    fn export_links_fills_missing_kind_from_older_dir_of_same_stem() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let stem = "普通公文-工作安排";

        let a_old = dir.join(format!("{stem}-202601011000"));
        let a_new = dir.join(format!("{stem}-202601011100"));
        let b_latest = dir.join("普通公文-别的工作-202601011200");
        for folder in [&a_old, &a_new, &b_latest] {
            std::fs::create_dir_all(folder).unwrap();
        }
        touch_export(&a_old, "tex");
        touch_export(&a_new, "docx");
        // 别的文稿有 pdf。
        touch_export(&b_latest, "pdf");
        std::thread::sleep(Duration::from_millis(20));

        let mut links = ExportLinks::default();
        links.refresh(dir.to_str().unwrap(), Some(stem));
        assert_eq!(
            links
                .path(ExportKind::Tex)
                .map(|p| p.parent().unwrap().to_path_buf()),
            Some(a_old.clone()),
            "tex 应从同文稿更早导出补齐"
        );
        assert_eq!(
            links
                .path(ExportKind::Word)
                .map(|p| p.parent().unwrap().to_path_buf()),
            Some(a_new.clone())
        );
        assert!(
            links.path(ExportKind::Pdf).is_none(),
            "pdf 不能取自别的文稿"
        );
    }

    /// 按导出的落盘命名：成品文件名主干与所在文件夹同名。
    fn touch_export(folder: &std::path::Path, extension: &str) {
        let stem = folder.file_name().unwrap().to_string_lossy();
        std::fs::write(folder.join(format!("{stem}.{extension}")), b"x").unwrap();
    }

    #[test]
    fn markdown_find_returns_utf8_safe_ranges() {
        let text = "标题：重点工作\n正文：重点工作";
        let matches = markdown_matches(text, "重点", true);
        assert_eq!(matches.len(), 2);
        assert_eq!(&text[matches[0].clone()], "重点");
        assert_eq!(&text[matches[1].clone()], "重点");
        assert!(matches.iter().all(|range| {
            text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
        }));
    }

    #[test]
    fn markdown_find_can_ignore_ascii_case() {
        let text = "Markdown markdown MARKDOWN";
        assert_eq!(markdown_matches(text, "markdown", false).len(), 3);
        assert_eq!(markdown_matches(text, "markdown", true).len(), 1);
    }

    #[test]
    fn regex_find_reports_invalid_patterns_and_expands_named_captures() {
        let text = "日期：2026-08-08";
        let matches = markdown_matches_mode(
            text,
            r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})",
            true,
            true,
        )
        .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            expanded_replacement(
                text,
                r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})",
                "$year年$month月$day日",
                true,
                true,
                &matches[0],
            ),
            "2026年08月08日"
        );
        assert!(markdown_matches_mode(text, "(", true, true).is_err());
    }

    #[test]
    fn reverse_replacement_keeps_all_match_ranges_valid() {
        let mut text = "甲处、甲处、甲处".to_string();
        let matches = markdown_matches(&text, "甲处", true);
        for range in matches.into_iter().rev() {
            text.replace_range(range, "乙单位");
        }
        assert_eq!(text, "乙单位、乙单位、乙单位");
    }
}
