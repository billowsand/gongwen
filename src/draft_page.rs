//! 起草页：一篇打开的稿件对应一个 [`DraftSession`]，页面本身是借用一组
//! 应用级资源的 [`DraftPage`]。多篇稿件同时打开时，会话之间互不干扰，
//! 只有配置、稿件库和后台任务通道是共享的。
//!
//! 各功能域已拆分到 `draft_page/` 子模块（表单、Ribbon、编辑器、Markdown、
//! 表格、查找替换、版本、页面外壳、后台任务），这里保留 `DraftSession` /
//! `DraftPage` 结构体、`ExportLinks` 与共享类型/常量的再导出。

use crate::app::{DraftAction, VersionSwitchPrompt, WorkerResult};
use crate::export;
use crate::highlight::MarkdownHighlighter;
use crate::manuscript::ManuscriptStore;
use crate::models::{AppConfig, DraftInput, ManuscriptStatus, ReviewNote, TemplateKind};
use crate::theme;
use eframe::egui;
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

mod editor;
mod find;
mod form;
mod markdown;
mod page;
mod ribbon;
mod table;
mod tasks;
mod versions;

pub(crate) use editor::{PreviewAnchor, editor_id};
pub(crate) use find::{
    DraftDiffState, FileAction, LoadedVersion, MarkdownFindState, blank_line_padding,
    jump_to_source, markdown_matches_mode, select_source_range,
};
pub(crate) use form::FormSectionState;
pub(crate) use markdown::{
    body_stats, chinese_today, continue_ordered_list, display_width, editor_cursor,
    editor_selection, is_table_separator_line, is_table_source_line, line_at_byte, line_ranges,
    markdown_heading_level, split_row, table_column_count, tidy_blank_lines, toggle_bullet,
};
pub(crate) use table::{TableOp, table_grid_picker};
// test-only names: only compiled in test builds (kept for the root test modules)
#[cfg(test)]
pub(crate) use find::{expanded_replacement, markdown_matches};
#[cfg(test)]
pub(crate) use form::{
    FormSection, check_form, document_number_row, field_error, layout_thumbnail,
    required_row_label, section_header_ui,
};
#[cfg(test)]
pub(crate) use markdown::{map_lines, set_heading, toggle_bold};
#[cfg(test)]
pub(crate) use table::{blank_table, render_table, table_at};

/// 功能区「输出」分区里仿 WinEdt 的三个成品入口。
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

/// 导出目录里当前文稿最近一次产出的 tex/pdf/docx，供「输出」分区那三枚入口点亮与打开。
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
    /// 导出目录会随时间攒下很多子目录，只翻最近的这些，免得功能区拖慢整帧。
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
        let mut dirs: Vec<(std::time::SystemTime, String, PathBuf)> = Vec::new();
        let mut loose: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                if self.matches(&entry.file_name()) {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    dirs.push((modified, name, entry.path()));
                }
            } else if self.matches(&entry.file_name()) {
                loose.push(entry.path());
            }
        }
        // 文件系统时间戳精度不足时同一批目录会得到相同 mtime，此时按目录名倒序
        // 兜底：导出目录名带分钟级时间戳，名字越靠后就是越新的导出。
        dirs.sort_by(|(a_modified, a_name, _), (b_modified, b_name, _)| {
            b_modified.cmp(a_modified).then_with(|| b_name.cmp(a_name))
        });
        for (_, _, dir) in dirs.iter().take(Self::MAX_DIRS) {
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

/// 功能区控件统一高度：按钮、下拉框、状态标签共用一个交互高度，
/// 免得较高的控件把整行基线撑歪。
pub(crate) const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;

const SCREEN_PT: f32 = 96.0 / 72.0;
/// 一帧内宽度变化超过这么多点，就当成跳变而非拖动：拖分隔条一帧只走几个像素，
/// 而切换显示方式、窗口最大化是一步到位的，没有"连续缩放"可言。
const DRAG_STEP_MAX: f32 = 64.0;

const OFFICIAL_PAGE_WIDTH: f32 = 595.28 * SCREEN_PT;
const OFFICIAL_PAGE_HEIGHT: f32 = 841.89 * SCREEN_PT;
const OFFICIAL_PAGE_MARGIN_LEFT: f32 = 79.35 * SCREEN_PT;
const OFFICIAL_PAGE_MARGIN_TOP: f32 = 52.0 * SCREEN_PT;
const OFFICIAL_BODY_SIZE: f32 = 16.0 * SCREEN_PT;

/// A4 公文版心宽度：(595.28 - 79.35 - 73.70) 磅，按 96 dpi 换算。
/// 实时排版模式固定用这个换行宽度，不随窗口拉宽而改变每行字数。
const OFFICIAL_EDITOR_CONTENT_WIDTH: f32 = (595.28 - 79.35 - 73.70) * SCREEN_PT;

/// 功能区分组之间的竖线。自绘 1.5px 的 `border_strong` 竖线并留更宽间距，
/// 比 egui 默认的浅色细线分隔感强得多——功能区二十来个按钮靠它分组，
/// 太细了扫一眼看不出边界。
pub(crate) fn toolbar_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(12.0, TOOLBAR_CONTROL_HEIGHT),
        egui::Sense::hover(),
    );
    ui.painter().vline(
        rect.center().x,
        rect.top() + 6.0..=rect.bottom() - 6.0,
        egui::Stroke::new(1.5, theme::border_strong()),
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
    pub(crate) warnings: Vec<ReviewNote>,
    /// 上一次编译由孤行探针实测出来的提示，以及它对应的正文快照。孤行是排版
    /// 结果，只对那一份正文成立；正文一改这批提示立即作废，等下次编译再报。
    pub(crate) proof_warnings: Vec<ReviewNote>,
    pub(crate) proof_markdown: String,
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
    /// 要素区三段（版头/主体/版记）的展开状态与待跳转目标。
    pub(crate) form_sections: FormSectionState,
    /// 审校区当前的显示方式。
    pub(crate) preview_mode: PreviewMode,
    /// 审校提示的按需右侧抽屉。导出成品不进抽屉，走「输出」分区的三枚格式入口。
    pub(crate) result_drawer_open: bool,
    /// “清空审校稿”的二次确认。清空会同时丢掉审校提示、查找状态和导出结果，
    /// 必须由模态框拦住，不能在拥挤的功能区里单击即执行。
    pub(crate) clear_review_confirm: bool,
    /// 公文预览的缩放倍率；None 表示按面板宽度自适应。
    pub(crate) preview_zoom: Option<f32>,
    /// 上一帧自适应算出的倍率，用作手动加减档的起点。
    pub(crate) preview_fit_scale: f32,
    /// 版面上一次"落定"时所用的缩放倍率。拖动分隔条、缩放窗口的过程中它保持
    /// 不变，让字号恒定、排版缓存全部命中；真正的视觉缩放交给层变换。
    /// 0 表示还没排过，第一帧直接按目标倍率落定。
    pub(crate) preview_layout_scale: f32,
    /// 上一帧量到的预览可视宽度，用来判断宽度是否还在变化。
    pub(crate) preview_last_width: f32,
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
    /// 「插入 → 表格」里手填的行列数，记住上一次填的值。行数含表头。
    pub(crate) table_size: (usize, usize),
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
        Self::with_markdown(key, config, String::new())
    }

    /// 新开一篇以现成正文打底的稿件（从外部文档导入）。要素仍按配置留空，
    /// 只有正文是给定的；内容指纹算在正文之上，所以“导入完还没动”不算改过。
    pub(crate) fn with_markdown(key: DocKey, config: &AppConfig, markdown: String) -> Self {
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
            markdown,
        )
    }

    pub(crate) fn from_parts(
        key: DocKey,
        manuscript_id: Option<i64>,
        mut draft: DraftInput,
        generated_markdown: String,
    ) -> Self {
        if draft.kind.has_document_number() && draft.profile.document_year.trim().is_empty() {
            draft.profile.document_year = draft.document_year();
        }
        let mut session = Self {
            key,
            manuscript_id,
            draft,
            generated_markdown,
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_markdown: String::new(),
            output_files: Vec::new(),
            export_error: None,
            loaded_version: None,
            draft_diff: DraftDiffState::default(),
            manual_fields: BTreeSet::new(),
            // 打开稿件第一眼就是满屏审校稿；要改抬头文号再展开要素区。
            form_collapsed: true,
            form_sections: FormSectionState::default(),
            preview_mode: PreviewMode::Source,
            result_drawer_open: false,
            clear_review_confirm: false,
            preview_zoom: None,
            preview_fit_scale: 1.0,
            preview_layout_scale: 0.0,
            preview_last_width: 0.0,
            preview_anchor: None,
            pending_source_jump: None,
            pending_source_selection: None,
            pending_render_jump: false,
            markdown_find: MarkdownFindState::default(),
            table_size: (3, 3),
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
        self.clear_review_confirm = false;
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

// ── 功能区的文本操作 ────────────────────────────────────────────────────────
// 下面这一组都是纯字符串函数：功能区的按钮只负责取光标位置、调用它们、把结果
// 写回审校稿。逻辑不碰 egui，所以能单独测。

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ColumnAlign;
    use crate::validator;

    /// 表单的必填标记必须和审校的必填校验说同一套话。填报区显示"齐了"、
    /// 生成完审校又报"缺少主送单位"，比不标还糟。
    #[test]
    fn form_check_agrees_with_validator_on_an_empty_draft() {
        let rules = crate::models::SecurityRules::default();
        for kind in TemplateKind::ALL {
            let mut draft = DraftInput {
                kind,
                ..Default::default()
            };
            draft.profile.security_level.clear();
            let check = check_form(&draft);
            let missing: usize = FormSection::ALL
                .iter()
                .map(|section| check.progress[section.index()].missing())
                .sum();
            assert!(
                check.errors.contains_key("security_level"),
                "{kind:?}：密级是所有文种的必填项"
            );
            // 审校报的“缺少某某单位/领导”都属于要素，表单必须已经标出来。
            // 正文类提示（结语、一级标题、议程固定字段）不在要素区的职责内。
            let element_warnings = validator::validate(&draft, "正文", &[], &rules)
                .into_iter()
                .filter(|warning| {
                    warning.contains("缺少")
                        && !warning.contains("结语")
                        && !warning.contains("一级标题")
                        && !warning.contains("固定字段")
                })
                .collect::<Vec<_>>();
            for warning in element_warnings {
                assert!(
                    missing > 0,
                    "{kind:?}：审校报“{warning}”，表单却认为必填项已齐"
                );
            }
        }
    }

    /// 必填项填全之后，三段的进度必须都到头——否则徽章会永远停在“缺 N 项”。
    #[test]
    fn form_check_clears_once_required_letter_elements_are_filled() {
        let mut draft = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        draft.profile.security_level = "机密".into();
        draft.profile.issuing_unit = "甲单位".into();
        draft.profile.recipient = "乙单位".into();
        let check = check_form(&draft);
        for section in FormSection::ALL {
            assert_eq!(
                check.progress[section.index()].missing(),
                0,
                "{}段仍报缺项：{:?}",
                section.label(),
                check.errors
            );
        }
    }

    /// 三段折叠头、必填标签、内联报错、文号复合行和缩略导航图都是纯绘制代码，
    /// 编译期查不出 id 冲突、越界或 0 宽度这类问题，拿真实布局跑一遍每个文种。
    #[test]
    fn form_chrome_draws_without_panicking() {
        let ctx = egui::Context::default();
        for kind in TemplateKind::ALL {
            let mut draft = DraftInput {
                kind,
                ..Default::default()
            };
            draft.profile.security_level.clear();
            let check = check_form(&draft);
            let mut manual = BTreeSet::new();
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                assert!(
                    layout_thumbnail(ui, kind, &check).is_none(),
                    "没有点击时缩略图不应产生跳转"
                );
                for section in FormSection::ALL {
                    let mut open = true;
                    section_header_ui(
                        ui,
                        section,
                        kind,
                        &mut open,
                        check.progress[section.index()],
                    );
                    assert!(open, "没有点击时分段的展开状态不应改变");
                }
                egui::Grid::new("smoke_grid").num_columns(2).show(ui, |ui| {
                    required_row_label(ui, "密级", Some("说明"));
                    ui.label("控件");
                    ui.end_row();
                    field_error(ui, &check, "security_level");
                    document_number_row(
                        ui,
                        &mut draft.profile,
                        &[],
                        &mut manual,
                        true,
                        240.0,
                        "例如：X政函",
                        "smoke_code",
                    );
                });
            });
        }
    }

    /// 导入内容要独占段落：插入点两侧各自补到有空行为止，文首文末不补。
    #[test]
    fn imported_block_gets_blank_lines_on_both_sides() {
        let splice = |text: &str, pos: usize, block: &str| {
            let lead = blank_line_padding(&text[..pos], true);
            let tail = blank_line_padding(&text[pos..], false);
            let mut out = text.to_string();
            out.insert_str(pos, &format!("{lead}{block}{tail}"));
            out
        };
        // 空稿：两侧都不补。
        assert_eq!(splice("", 0, "# 标题"), "# 标题");
        // 插在一行中间：前后各补一个空行，把那一行切开（偏移按字节，汉字占 3 字节）。
        assert_eq!(splice("前后", 3, "表格"), "前\n\n表格\n\n后");
        // 已有一个换行的一侧只补差的那个。
        assert_eq!(splice("上\n", 4, "块"), "上\n\n块");
        // 两侧已经是空行就一个都不补。
        assert_eq!(splice("上\n\n\n\n下", 5, "块"), "上\n\n块\n\n下");
    }

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

    /// 表格解析：光标所在的行列要认得出来，分隔行上的光标算表头。
    #[test]
    fn table_at_locates_cursor_row_and_column() {
        let text = "前言

| 序号 | 名称 |
| --- | :---: |
| 1 | 甲 |
| 2 | 乙 |

后记";
        let at = |needle: &str| text.find(needle).expect("片段存在");

        let table = table_at(text, at("甲")).expect("光标在表格里");
        assert_eq!(table.rows.len(), 3, "表头 + 两行数据，分隔行不算");
        assert_eq!(table.row, 1);
        assert_eq!(table.column, 1);
        assert_eq!(table.aligns, vec![ColumnAlign::Auto, ColumnAlign::Center]);

        // 分隔行上的光标归到表头，列仍按竖线数。
        let table = table_at(text, at(":---:")).expect("分隔行也在表格里");
        assert_eq!(table.row, 0);
        assert_eq!(table.column, 1);

        assert!(table_at(text, at("前言")).is_none());
        assert!(table_at(text, at("后记")).is_none());
    }

    /// 缺了分隔行的几行竖线不是表格，不能当表格改。
    #[test]
    fn table_at_rejects_pipes_without_a_separator_row() {
        let text = "| 甲 | 乙 |
| 丙 | 丁 |";
        assert!(table_at(text, 3).is_none());
    }

    /// 增删行列之后整表重排：列宽按最宽的一格算，中文按两格宽。
    #[test]
    fn render_table_pads_by_display_width() {
        let rows = vec![
            vec!["序号".to_string(), "名称".to_string()],
            vec!["1".to_string(), "甲单位".to_string()],
        ];
        let rendered = render_table(&rows, &[ColumnAlign::Auto, ColumnAlign::Center]);
        assert_eq!(
            rendered,
            "| 序号 | 名称   |
| ---- | :----: |
| 1    | 甲单位 |"
        );
        // 重新解析一遍，行列与对齐都不变。
        let table = table_at(&rendered, 3).expect("是一张表格");
        assert_eq!(table.rows, rows);
        assert_eq!(table.aligns, vec![ColumnAlign::Auto, ColumnAlign::Center]);
    }

    /// 窄列写上对齐冒号之后，分隔行仍要能被认成分隔行：GFM 要求去掉冒号后
    /// 还有三条短横，`:-:` 那样写出来的表会整张退化成普通段落。
    #[test]
    fn narrow_columns_still_render_a_valid_separator_row() {
        for align in [
            ColumnAlign::Auto,
            ColumnAlign::Left,
            ColumnAlign::Center,
            ColumnAlign::Right,
        ] {
            let rows = vec![
                vec!["甲".to_string(), "乙".to_string()],
                vec!["1".to_string(), "2".to_string()],
            ];
            let rendered = render_table(&rows, &[align, ColumnAlign::Auto]);
            let separator = rendered.lines().nth(1).expect("有分隔行");
            assert!(
                is_table_separator_line(separator),
                "{align:?} 的分隔行不合法：{separator}"
            );
            // 导出器与光标定位也要认得它。
            let table = table_at(&rendered, 0).expect("是一张表格");
            assert_eq!(table.aligns, vec![align, ColumnAlign::Auto]);
            assert!(matches!(
                export::parse_markdown(&rendered).first(),
                Some(export::MarkdownBlock::Table { .. })
            ));
        }
    }

    /// 一列的「表格」不成表：GFM 要求分隔行至少切出两格，所以插入时兜底到两列。
    #[test]
    fn blank_table_never_has_a_single_column() {
        let table = blank_table(2, 1);
        assert_eq!(table_column_count(table.lines().next().expect("有表头")), 2);
        assert!(matches!(
            export::parse_markdown(&table).first(),
            Some(export::MarkdownBlock::Table { .. })
        ));
    }

    /// 网格选择器插出来的空表：首行是表头，分隔行认得出来。
    #[test]
    fn blank_table_has_a_header_and_separator() {
        let table = blank_table(3, 2);
        let lines = table.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 4, "3 行 + 1 条分隔行");
        assert!(is_table_separator_line(lines[1]), "{}", lines[1]);
        assert_eq!(table_column_count(lines[0]), 2);
        // 导出器也要认得它是一张表。
        assert!(matches!(
            export::parse_markdown(&table).first(),
            Some(export::MarkdownBlock::Table { rows, .. }) if rows.len() == 3
        ));
    }

    /// 标题层级：反复点不同层级不会把 `#` 越堆越多，降为正文能清干净。
    #[test]
    fn set_heading_replaces_existing_level() {
        assert_eq!(set_heading("正文一段", 2), "## 正文一段");
        assert_eq!(set_heading("## 一级标题", 3), "### 一级标题");
        assert_eq!(set_heading("### 二级标题", 0), "二级标题");
        assert_eq!(set_heading("  ## 带缩进", 2), "## 带缩进");
    }

    /// 项目符号是开关：第二次点回到普通段落。
    #[test]
    fn toggle_bullet_switches_both_ways() {
        assert_eq!(toggle_bullet("一条"), "- 一条");
        assert_eq!(toggle_bullet("- 一条"), "一条");
        assert_eq!(toggle_bullet("* 一条"), "一条");
        assert_eq!(toggle_bullet(""), "", "空行不加符号");
    }

    /// 选区覆盖到的每一行都要改到，选区落在行中间也按整行算。
    #[test]
    fn map_lines_covers_every_touched_line() {
        let text = "第一行
第二行
第三行";
        let start = text.find("一行").expect("命中");
        let end = text.find("二行").expect("命中");
        let (updated, span) = map_lines(text, &(start..end), |line| set_heading(line, 2));
        assert_eq!(
            updated,
            "## 第一行
## 第二行
第三行"
        );
        assert_eq!(
            &updated[span],
            "## 第一行
## 第二行"
        );
    }

    /// 加粗是开关：选中已加粗的文字（连标记一起选，或只选中间）都能取消。
    #[test]
    fn toggle_bold_wraps_and_unwraps() {
        let text = "这是重点内容";
        let range = text.find("重点").expect("命中")..text.find("内容").expect("命中");
        let (bolded, selection) = toggle_bold(text, &range);
        assert_eq!(bolded, "这是**重点**内容");
        assert_eq!(&bolded[selection.clone()], "重点");

        // 只选中间：靠两侧的标记识别出已加粗。
        let (plain, restored) = toggle_bold(&bolded, &selection);
        assert_eq!(plain, "这是重点内容");
        assert_eq!(&plain[restored], "重点");

        // 连标记一起选中同样能取消。
        let whole = bolded.find("**").expect("命中")..bolded.rfind("**").expect("命中") + 2;
        assert_eq!(toggle_bold(&bolded, &whole).0, "这是重点内容");
    }

    /// 清理空行：连续空行压成一行，行尾空格去掉，文末只留一个换行。
    #[test]
    fn tidy_blank_lines_collapses_runs() {
        assert_eq!(
            tidy_blank_lines(
                "标题   



正文


"
            ),
            "标题

正文
"
        );
        assert_eq!(tidy_blank_lines(""), "");
    }

    /// 字数按导出后的正文算：标记、竖线、图片引用都不计。
    #[test]
    fn body_stats_counts_visible_text_only() {
        let markdown = "# 测试函

<!-- [正文] -->

## 一、要求

**重点**内容。

| 甲 | 乙 |
| --- | --- |
| 1 | 2 |

![图](images/a.png)

又一段。";
        let (characters, paragraphs) = body_stats(markdown);
        // 测试函(3) + 要求(2，标题里手写的“一、”由导出器统一编号，不计)
        // + 重点内容。(5) + 表格四格(4) + 又一段。(4)
        assert_eq!(characters, 18);
        assert_eq!(paragraphs, 2, "只有两个真正的段落");
    }

    /// 中文日期是公文成文日期的写法：年份逐位、月日用中文数字。
    #[test]
    fn chinese_today_reads_like_a_document_date() {
        let today = chinese_today();
        assert!(export::chinese_date_parts(&today).is_some(), "{today}");
        assert!(!today.chars().any(|ch| ch.is_ascii_digit()), "{today}");
    }

    /// 收集一帧内所有文本及其中心点，用于定位可点击控件。
    fn text_centers(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Pos2)> {
        fn collect(shape: &egui::epaint::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::epaint::Shape::Text(text) => {
                    let s = text.galley.text().to_string();
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    out.push((s, rect.center()));
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            collect(&clipped.shape, &mut out);
        }
        out
    }

    /// 回归测试：在文种下拉框里点选“普通公文”不应崩溃。
    ///
    /// 曾因时序 bug 崩溃：`form_header_ui` 里的 `versioned` 在文种下拉框渲染之前
    /// 求值，点选“普通公文”的那一帧 kind 已变、versioned 还是旧值 true，残留渲染
    /// 的版本下拉框走到 `match` 的 `PlainDocument => unreachable!()`，整个程序退出。
    /// 测试用真实点击事件走完 展开面板 → 打开文种下拉 → 点选“普通公文” 的路径。
    #[test]
    fn switching_kind_combo_to_plain_document_does_not_panic() {
        let ctx = egui::Context::default();
        crate::theme::configure_icons(&ctx);
        crate::theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        ctx.set_pixels_per_point(2.0);
        let mut config = AppConfig::default();
        let markdown = "# 测试标题\n\n正文内容。".to_string();
        let mut doc = DraftSession::with_markdown(1, &config, markdown);
        assert_eq!(doc.draft.kind, TemplateKind::OfficialLetter);
        let (sender, _keep) = std::sync::mpsc::channel();
        let mut status = String::new();
        let mut version_switch = None;
        let mut revert_confirm = None;
        let mut actions = Vec::new();
        let mut export_links = ExportLinks::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0));

        let mut frame = |ctx: &egui::Context, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let output = ctx.clone().run_ui(raw, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut page = DraftPage {
                        doc: &mut doc,
                        config: &mut config,
                        store: None,
                        sender: &sender,
                        status: &mut status,
                        version_switch: &mut version_switch,
                        revert_confirm: &mut revert_confirm,
                        actions: &mut actions,
                        export_links: &mut export_links,
                    };
                    page.create_ui(ui);
                });
            });
            output.shapes
        };
        // egui 的点击要按下、抬起各一帧才算数。
        let click = |frame: &mut dyn FnMut(
            &egui::Context,
            Vec<egui::Event>,
        ) -> Vec<egui::epaint::ClippedShape>,
                     ctx: &egui::Context,
                     pos: egui::Pos2| {
            let press = vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let release = vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }];
            let _ = frame(ctx, press);
            frame(ctx, release)
        };

        // 公文要素区默认收起，先点 ribbon 上的“公文要素”按钮展开它。
        let shapes = frame(&ctx, vec![]);
        let expand = text_centers(&shapes)
            .into_iter()
            .find(|(s, _)| s == "公文要素")
            .expect("应能找到 ribbon 上的“公文要素”按钮");
        let shapes = click(&mut frame, &ctx, expand.1);
        // 打开文种下拉框：先点当前文种“公函”，再等一帧让选项渲染出来。
        let combo = text_centers(&shapes)
            .into_iter()
            .find(|(s, p)| s == "公函" && p.x < 500.0)
            .expect("应能找到文种下拉框上的“公函”");
        let _ = click(&mut frame, &ctx, combo.1);
        let shapes = frame(&ctx, vec![]);
        // 点选“普通公文”。切换那一帧内 kind 即变，若时序 bug 复活会在这里 panic。
        let plain = text_centers(&shapes)
            .into_iter()
            .find(|(s, _)| s.contains("普通公文"))
            .expect("下拉里应有“普通公文”");
        let _ = click(&mut frame, &ctx, plain.1);
        assert_eq!(doc.draft.kind, TemplateKind::PlainDocument);
    }
}

#[cfg(test)]
mod split_resize_tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    /// 把 `markdown_render` 放进一个可控的 egui 上下文里跑若干帧，
    /// 返回逐帧耗时、字体图集整体重建次数，以及每帧实际排版所用的缩放。
    struct Harness {
        ctx: egui::Context,
        doc: DraftSession,
        config: AppConfig,
        sender: Sender<WorkerResult>,
        status: String,
        version_switch: Option<VersionSwitchPrompt>,
        revert_confirm: Option<(i64, i64)>,
        actions: Vec<DraftAction>,
        export_links: ExportLinks,
        _keep: Receiver<WorkerResult>,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::configure_icons(&ctx);
            theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
            ctx.set_pixels_per_point(2.0);
            let config = AppConfig::default();
            let mut markdown = String::from("# 关于加强全省中小学教育教学质量管理工作的函\n\n");
            for i in 1..=40 {
                markdown.push_str(&format!("## 第{i}节 关于进一步做好相关工作的意见\n\n"));
                for _ in 0..4 {
                    markdown.push_str("各地各校要深刻认识本项工作的重要意义，紧紧围绕立德树人根本任务，统筹推进课程建设、师资培养与教学评价改革，确保各项部署落到实处、见到实效。\n\n");
                }
            }
            let doc = DraftSession::with_markdown(1, &config, markdown);
            let (sender, _keep) = std::sync::mpsc::channel();
            Self {
                ctx,
                doc,
                config,
                sender,
                status: String::new(),
                version_switch: None,
                revert_confirm: None,
                actions: Vec::new(),
                export_links: ExportLinks::default(),
                _keep,
            }
        }

        /// 画一帧并取回纸张（最大的那块白色矩形）在屏幕上的位置与大小。
        /// 这是"眼睛看到的版面"，用来验证层变换画出的画面与重排出的一致。
        fn paper(&mut self, width: f32) -> egui::Rect {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 900.0),
                )),
                ..Default::default()
            };
            let out = self.ctx.clone().run_ui(raw, |ui| {
                let mut page = DraftPage {
                    doc: &mut self.doc,
                    config: &mut self.config,
                    store: None,
                    sender: &self.sender,
                    status: &mut self.status,
                    version_switch: &mut self.version_switch,
                    revert_confirm: &mut self.revert_confirm,
                    actions: &mut self.actions,
                    export_links: &mut self.export_links,
                };
                page.markdown_render(ui);
            });
            // 形状会嵌套（Frame 的底 + 阴影 + 内容都装在 Shape::Vec 里），要递归摊平。
            fn collect(shape: &egui::epaint::Shape, out: &mut Vec<egui::Rect>) {
                match shape {
                    egui::epaint::Shape::Rect(rect)
                        if rect.fill == theme::paper::bg() && rect.rect.is_finite() =>
                    {
                        out.push(rect.rect);
                    }
                    egui::epaint::Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect(shape, out);
                        }
                    }
                    _ => {}
                }
            }
            let mut rects = Vec::new();
            for clipped in &out.shapes {
                collect(&clipped.shape, &mut rects);
            }
            rects
                .into_iter()
                .max_by(|a, b| {
                    (a.width() * a.height())
                        .partial_cmp(&(b.width() * b.height()))
                        .unwrap()
                })
                .expect("本帧应当画出纸张")
        }

        /// 用给定的原始输入画一帧审校提示面板。
        fn warnings_frame(&mut self, raw: egui::RawInput) {
            let _ = self.ctx.clone().run_ui(raw, |ui| {
                let mut page = DraftPage {
                    doc: &mut self.doc,
                    config: &mut self.config,
                    store: None,
                    sender: &self.sender,
                    status: &mut self.status,
                    version_switch: &mut self.version_switch,
                    revert_confirm: &mut self.revert_confirm,
                    actions: &mut self.actions,
                    export_links: &mut self.export_links,
                };
                page.warnings_ui(ui);
            });
        }

        /// 在审校面板的 `at` 处点一下（按下与抬起分两帧，egui 才认这是一次点击）。
        fn click_warning_at(&mut self, at: egui::Pos2) {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
            for pressed in [true, false] {
                self.warnings_frame(egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![
                        egui::Event::PointerMoved(at),
                        egui::Event::PointerButton {
                            pos: at,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                });
            }
        }
    }

    /// 清空审校稿不能只删正文：所有依赖旧正文的提示、定位、查找、导出结果和
    /// 版本来源标记必须一起复位，否则界面会继续展示已经失效的信息。
    #[test]
    fn clearing_review_output_resets_dependent_ui_state() {
        let mut harness = Harness::new();
        harness.doc.output_files.push(PathBuf::from("old.pdf"));
        harness.doc.export_error = Some("旧错误".into());
        harness.doc.loaded_version = Some(LoadedVersion {
            manuscript_id: 7,
            version_number: 3,
            name: "旧版本".into(),
        });
        harness.doc.draft_diff.base = Some(2);
        harness.doc.result_drawer_open = true;
        harness.doc.preview_anchor = Some(PreviewAnchor {
            range: 0..1,
            text: "#".into(),
        });
        harness.doc.pending_source_jump = Some(8);
        harness.doc.pending_source_selection = Some(1..4);
        harness.doc.pending_render_jump = true;
        harness.doc.markdown_find.open = true;

        {
            let mut page = DraftPage {
                doc: &mut harness.doc,
                config: &mut harness.config,
                store: None,
                sender: &harness.sender,
                status: &mut harness.status,
                version_switch: &mut harness.version_switch,
                revert_confirm: &mut harness.revert_confirm,
                actions: &mut harness.actions,
                export_links: &mut harness.export_links,
            };
            page.clear_review_output();
        }

        assert!(harness.doc.generated_markdown.is_empty());
        assert!(harness.doc.output_files.is_empty());
        assert!(harness.doc.export_error.is_none());
        assert!(harness.doc.loaded_version.is_none());
        assert!(harness.doc.draft_diff.base.is_none());
        assert!(!harness.doc.result_drawer_open);
        assert!(harness.doc.preview_anchor.is_none());
        assert!(harness.doc.pending_source_jump.is_none());
        assert!(harness.doc.pending_source_selection.is_none());
        assert!(!harness.doc.pending_render_jump);
        assert!(!harness.doc.markdown_find.open);
    }

    /// 孤行提示要能点：点中之后切回 Markdown 视图，并选中出问题的那一段。
    ///
    /// 逐点扫过面板而不是写死坐标——egui 改了边距也不会让这条测试失灵，而提示
    /// 一旦不可点，扫遍整块面板也不会触发，测试照样红。
    #[test]
    fn clicking_a_located_warning_selects_that_paragraph() {
        let mut harness = Harness::new();
        let filler: String = "工作要点部署要求经研究决定开展检查评估现将有关事项通知如下请各单位遵照执行并及时反馈情况我们将根据反馈意见完善制度"
            .chars()
            .take(54)
            .collect();
        harness.doc.generated_markdown = format!("# 标题\n\n短段落。\n\n{filler}。\n");
        harness.doc.preview_mode = PreviewMode::Rendered;

        {
            let mut page = DraftPage {
                doc: &mut harness.doc,
                config: &mut harness.config,
                store: None,
                sender: &harness.sender,
                status: &mut harness.status,
                version_switch: &mut harness.version_switch,
                revert_confirm: &mut harness.revert_confirm,
                actions: &mut harness.actions,
                export_links: &mut harness.export_links,
            };
            page.revalidate();
        }
        let expected = harness
            .doc
            .warnings
            .iter()
            .find_map(|note| note.span.clone())
            .expect("孤行提示应带定位范围");
        assert_eq!(
            harness.doc.generated_markdown[expected.clone()],
            format!("{filler}。")
        );

        // 先画一帧完成布局，之后的点击才有可命中的控件。
        harness.warnings_frame(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            ..Default::default()
        });
        let mut hit = None;
        'scan: for y in 0..40 {
            for x in 0..30 {
                harness.click_warning_at(egui::pos2(4.0 + x as f32 * 30.0, 4.0 + y as f32 * 8.0));
                if harness.doc.preview_mode == PreviewMode::Source {
                    hit = Some((x, y));
                    break 'scan;
                }
            }
        }
        assert!(
            hit.is_some(),
            "整块审校面板都点不动，孤行提示没有做成可点的"
        );
        assert_eq!(
            harness.doc.pending_source_selection,
            Some(expected),
            "点击后应选中提示指向的那一段"
        );
    }

    impl Harness {
        /// 画一帧，窗格宽度为 `width`。返回 (耗时 ms, 是否整体重建了字体图集)。
        fn frame(&mut self, width: f32) -> (f32, bool) {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 900.0),
                )),
                ..Default::default()
            };
            let started = std::time::Instant::now();
            let out = self.ctx.clone().run_ui(raw, |ui| {
                let mut page = DraftPage {
                    doc: &mut self.doc,
                    config: &mut self.config,
                    store: None,
                    sender: &self.sender,
                    status: &mut self.status,
                    version_switch: &mut self.version_switch,
                    revert_confirm: &mut self.revert_confirm,
                    actions: &mut self.actions,
                    export_links: &mut self.export_links,
                };
                page.markdown_render(ui);
            });
            let elapsed = started.elapsed().as_secs_f32() * 1000.0;
            let rebuilt = out
                .textures_delta
                .set
                .iter()
                .any(|(_, delta)| delta.pos.is_none() && delta.image.width() > 1000);
            (elapsed, rebuilt)
        }
    }

    fn percentile(values: &mut [f32], p: f32) -> f32 {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        values[((values.len() as f32 - 1.0) * p) as usize]
    }

    /// 拖动分隔条的过程中，版面缩放必须冻结在上一次落定的值上——
    /// 这是"字号恒定、缓存全命中"的前提，也是不再顿挫的根本原因。
    #[test]
    fn layout_scale_is_frozen_while_width_changes() {
        let mut harness = Harness::new();
        // egui 要一两帧才量准滚动区，先热一下再取基准。
        harness.frame(900.0);
        harness.frame(900.0);
        let settled = harness.doc.preview_layout_scale;
        assert!(settled > 0.0, "首帧应当直接落定");

        // 连续改变宽度：排版缩放不许动。
        for i in 1..=20 {
            harness.frame(900.0 + i as f32 * 3.7);
            assert_eq!(
                harness.doc.preview_layout_scale, settled,
                "第 {i} 帧宽度仍在变，版面缩放不应重排"
            );
        }
        // 但"眼睛看到的倍率"必须一路跟着宽度连续走，否则就不是丝滑而是冻住。
        assert!(
            harness.doc.preview_fit_scale > settled,
            "显示倍率应随宽度增长，实际 {} vs {settled}",
            harness.doc.preview_fit_scale
        );
    }

    /// 宽度一停下来，下一帧就按精确倍率重排，文字恢复锐利。
    #[test]
    fn layout_settles_on_the_frame_after_motion_stops() {
        let mut harness = Harness::new();
        harness.frame(900.0);
        harness.frame(900.0);
        for i in 1..=10 {
            harness.frame(900.0 + i as f32 * 3.7);
        }
        let frozen = harness.doc.preview_layout_scale;
        let shown = harness.doc.preview_fit_scale;
        assert_ne!(frozen, shown, "拖动中两者本就该不同");

        // 松手：宽度不再变化。
        harness.frame(900.0 + 10.0 * 3.7);
        assert_eq!(
            harness.doc.preview_layout_scale, shown,
            "落定帧必须按精确倍率重排"
        );
    }

    /// 显示倍率与宽度之间保持严格连续——没有任何档位/台阶。
    #[test]
    fn displayed_scale_is_continuous_in_width() {
        let mut harness = Harness::new();
        harness.frame(900.0);
        harness.frame(900.0);
        let mut previous = None;
        // 亚像素步进：每一步都必须带来一个不同的、单调增长的显示倍率。
        for step in 0..40 {
            let width = 900.0 + step as f32 * 0.25;
            harness.frame(width);
            let shown = harness.doc.preview_fit_scale;
            if let Some(previous) = previous {
                assert!(
                    shown > previous,
                    "宽度 {width} 处显示倍率没有随宽度增长：{previous} → {shown}（出现了台阶）"
                );
            }
            previous = Some(shown);
        }
    }

    /// 最要紧的一条：拖动中间那些帧是用层变换画出来的，画面必须与"老老实实
    /// 重排一遍"得到的画面重合——纸张一样大、一样居中。否则就是拿糊掉的画面
    /// 换性能，不是丝滑。
    #[test]
    fn transformed_frame_matches_a_real_relayout() {
        let width = 1000.0;

        // 甲：正常落定（无变换）时纸张的位置与大小。
        let mut settled = Harness::new();
        settled.frame(width);
        settled.frame(width);
        let expected = settled.paper(width);

        // 乙：从别处拖过来、本帧仍在动，因而走层变换路径。
        let mut dragging = Harness::new();
        dragging.frame(width - 40.0);
        dragging.frame(width - 40.0);
        let frozen = dragging.doc.preview_layout_scale;
        let actual = dragging.paper(width);
        assert_ne!(
            dragging.doc.preview_layout_scale, 0.0,
            "应当仍冻结在旧倍率上"
        );
        assert_eq!(
            dragging.doc.preview_layout_scale, frozen,
            "这一帧宽度在变，不应重排"
        );

        // 半个像素以内即认为重合。
        assert!(
            (actual.width() - expected.width()).abs() < 0.5,
            "纸张宽度对不上：变换后 {:.2}，重排应为 {:.2}",
            actual.width(),
            expected.width()
        );
        // 高度不可能逐像素相等：换行是离散的，每行的 ascent/descent 又各自取整，
        // 所以 k·H(s) 与 H(k·s) 必然差一点点。要紧的是这点误差摊到"一屏"之内
        // 有多大——变换以可视区顶边为支点，误差随离支点的距离线性累积。
        let relative = (actual.height() - expected.height()).abs() / expected.height();
        assert!(
            relative < 0.005,
            "纸张高度相对误差 {:.3}% 偏大：变换后 {:.2}，重排应为 {:.2}",
            relative * 100.0,
            actual.height(),
            expected.height()
        );
        let drift_in_one_screen = relative * 900.0;
        assert!(
            drift_in_one_screen < 3.0,
            "一屏之内会漂 {drift_in_one_screen:.2} 像素，松手时看得出跳动"
        );
        assert!(
            (actual.center().x - expected.center().x).abs() < 0.5,
            "纸张没有居中：变换后中心 {:.2}，重排应为 {:.2}",
            actual.center().x,
            expected.center().x
        );
        assert!(
            (actual.top() - expected.top()).abs() < 0.5,
            "纸张顶边漂了：变换后 {:.2}，重排应为 {:.2}",
            actual.top(),
            expected.top()
        );
    }

    /// 跳变（切换显示方式、窗口最大化）不该走变换路径糊一帧，直接重排。
    #[test]
    fn large_jumps_relayout_immediately() {
        let mut harness = Harness::new();
        harness.frame(900.0);
        harness.frame(900.0);
        harness.frame(1400.0);
        assert_eq!(
            harness.doc.preview_layout_scale, harness.doc.preview_fit_scale,
            "一步到位的跳变应当立即按精确倍率重排"
        );
    }

    /// 拖动整段过程中：字体图集一次都不许重建，版面一次都不许重排。
    ///
    /// 这两条是确定性的，也正是省下时间的因果本身——不拿墙钟时间做断言：
    /// 机器负载一高就会误报，而它证明不了任何这两条之外的东西。真实耗时打出来
    /// 供人看（本机实测：改动前 均值 3.56 / 最差 5.96 ms、重建 55 次；
    /// 改动后 均值 0.44 / 最差 0.81 ms、重建 0 次）。
    #[test]
    fn dragging_neither_rebuilds_the_atlas_nor_relayouts() {
        let mut harness = Harness::new();
        for i in 0..40 {
            harness.frame(700.0 + i as f32);
        }
        let frozen = harness.doc.preview_layout_scale;

        let mut times = Vec::new();
        let mut rebuilds = 0;
        let mut relayouts = 0;
        for i in 0..120 {
            let (elapsed, rebuilt) = harness.frame(700.0 + i as f32 * 3.7);
            times.push(elapsed);
            rebuilds += u32::from(rebuilt);
            if harness.doc.preview_layout_scale != frozen {
                relayouts += 1;
            }
        }
        let mean = times.iter().sum::<f32>() / times.len() as f32;
        let p50 = percentile(&mut times, 0.5);
        let p95 = percentile(&mut times, 0.95);
        let max = percentile(&mut times, 1.0);
        println!(
            "拖动 120 帧：均值 {mean:.2} / 中位 {p50:.2} / p95 {p95:.2} / 最差 {max:.2} ms，\
             图集重建 {rebuilds} 次，重排 {relayouts} 次"
        );

        assert_eq!(rebuilds, 0, "拖动期间字号恒定，字体图集不应重建");
        assert_eq!(relayouts, 0, "拖动期间版面缩放应始终冻结，一次都不该重排");
    }
}
