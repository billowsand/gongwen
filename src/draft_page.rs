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
use crate::models::{
    AppConfig, DraftInput, ExportSelection, GeneratedDraft, ManuscriptStatus, ResearchMetadata,
    ReviewNote, TemplateKind, TemplateProfile,
};
use crate::theme;
use eframe::egui;
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

mod bib_marks;
mod candidate_panel;
mod candidates;
mod caret;
mod citation_marks;
mod diff_editor;
mod diff_gaps;
pub(crate) mod diff_hunks;
mod editor;
mod find;
mod form;
mod format_clean;
mod heading_focus;
mod markdown;
mod navigator;
mod page;
mod paste;
mod phone_record;
mod references;
mod revise;
mod ribbon;
mod source_nav;
mod statistics;
mod table;
mod tasks;
mod timeline;
mod version_diff;
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
    markdown_heading_level, split_row, tidy_blank_lines, toggle_bullet,
};
// test-only names: only compiled in test builds
#[cfg(test)]
pub(crate) use markdown::table_column_count;
pub(crate) use navigator::PreviewScroll;
pub(crate) use source_nav::{SourceMinimap, SourceOutline};
pub(crate) use table::{TableOp, table_grid_picker};
pub(crate) use tasks::{review_notes, reviewed_draft};
pub(crate) use timeline::VersionTimelineState;
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
pub(crate) use table::{
    NUMBERED_TABLE_MARKER, blank_table, numbered_table_at, numbered_table_skeleton, render_table,
    table_at,
};

/// 功能区「输出」分区里的成品入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportKind {
    Pdf,
    Word,
    Typst,
}

impl ExportKind {
    pub(crate) const ALL: [Self; 3] = [Self::Pdf, Self::Word, Self::Typst];

    /// 悬停说明里用的格式名。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Word => "WORD",
            Self::Typst => "Typst 包",
        }
    }

    fn icon(self) -> theme::Icon {
        match self {
            Self::Pdf => theme::Icon::FileTypePdf,
            Self::Word => theme::Icon::FileTypeDoc,
            Self::Typst => theme::Icon::Package,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Word => "docx",
            Self::Typst => "zip",
        }
    }

    /// 只出这一种格式的导出选项；覆盖策略沿用设置。
    pub(crate) fn selection(self, overwrite: bool) -> ExportSelection {
        ExportSelection {
            markdown: false,
            docx: self == Self::Word,
            pdf: self == Self::Pdf,
            typst: self == Self::Typst,
            overwrite,
        }
    }

    /// 文件是不是这种成品。zip 只认 Typst 包，源码包也是 zip，不能混进来。
    fn matches_file(self, path: &Path) -> bool {
        let extension_matches = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(self.extension()));
        extension_matches
            && (self != Self::Typst
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("-Typst包.zip")))
    }

    /// 一次导出产出的文件里这种成品的那一份。
    pub(crate) fn find_in(self, files: &[PathBuf]) -> Option<&PathBuf> {
        files.iter().find(|file| self.matches_file(file))
    }
}

/// 导出目录里当前文稿最近一次产出的 PDF / Word / Typst 包，供成品入口点亮与打开。
///
/// 导出的落盘结构是 `输出目录/<文件名主干>/<文件名主干>.{md,docx,pdf}`，
/// 同一文稿按版本名称或“待提交”分子目录，不覆盖同名文件时再追加编号，也会和
/// 别的文稿的目录混在一起。所以这里先按当前文稿的导出主干前缀过滤出属于它的
/// 子目录，再按目录及成品文件的修改时间从新到旧翻，先翻到的就是"最近一次"，各种格式都找齐
/// 即停。逐帧翻盘太贵，按目录 + 前缀 + 节流缓存，导出完成时由外壳调
/// [`ExportLinks::invalidate`] 主动作废。
#[derive(Default)]
pub(crate) struct ExportLinks {
    dir: String,
    /// 当前文稿的导出主干前缀（`document_stem_prefix`）；换文稿后缓存自动作废。
    stem: Option<String>,
    scanned_at: Option<Instant>,
    pdf: Option<PathBuf>,
    docx: Option<PathBuf>,
    typst: Option<PathBuf>,
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
            ExportKind::Pdf => self.pdf.as_deref(),
            ExportKind::Word => self.docx.as_deref(),
            ExportKind::Typst => self.typst.as_deref(),
        }
    }

    /// 目录条目是否属于当前绑定的文稿：未绑定时全部认；绑定时按导出主干前缀
    /// 匹配——`stem` 本身、`stem-版本名称`（或“待提交”，重复导出可追加编号），
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
        self.pdf = None;
        self.docx = None;
        self.typst = None;

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
                if self.matches(&entry.file_name()) {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path();
                    // 同一个版本覆盖导出时，目录自身的修改时间可能不变，必须也看成品。
                    let modified = [
                        format!("{name}.pdf"),
                        format!("{name}.docx"),
                        format!("{name}-源码包.zip"),
                        format!("{name}-Typst包.zip"),
                    ]
                    .iter()
                    .filter_map(|file| std::fs::metadata(path.join(file)).ok()?.modified().ok())
                    .chain(meta.modified().ok())
                    .max()
                    .unwrap_or(std::time::UNIX_EPOCH);
                    dirs.push((modified, name, path));
                }
            } else if self.matches(&entry.file_name()) {
                loose.push(entry.path());
            }
        }
        // 修改时间相同时按名称排序，使结果稳定；新旧命名的目录都可识别。
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
        self.pdf.is_some() && self.docx.is_some() && self.typst.is_some()
    }

    /// 先到先得：调用顺序已经保证是从新到旧。
    fn take(&mut self, path: &Path) {
        if !path.is_file() {
            return;
        }
        let Some(kind) = ExportKind::ALL.into_iter().find(|k| k.matches_file(path)) else {
            return;
        };
        let slot = match kind {
            ExportKind::Pdf => &mut self.pdf,
            ExportKind::Word => &mut self.docx,
            ExportKind::Typst => &mut self.typst,
        };
        if slot.is_none() {
            *slot = Some(path.to_path_buf());
        }
    }
}

/// 功能区控件统一高度：按钮、下拉框、状态标签共用一个交互高度，
/// 免得较高的控件把整行基线撑歪。
pub(crate) const TOOLBAR_CONTROL_HEIGHT: f32 = 28.0;

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

/// AI 返回但尚未落入正文的修改提案。接受前复用版本对照视图，并对关键事实变化
/// 另设确认门槛。
pub(crate) struct AiProposal {
    pub(crate) before: String,
    pub(crate) result: GeneratedDraft,
    pub(crate) label: String,
    pub(crate) fact_changes: Vec<crate::ai_guard::FactChange>,
    pub(crate) fact_changes_confirmed: bool,
    /// 勾选「我已逐项核对」时核对的那份事实清单。排除 / 恢复变更块会让清单变化，
    /// 变了就把勾选作废，按新清单重新确认（审阅窗帧首比对）。
    pub(crate) confirmed_facts: Vec<crate::ai_guard::FactChange>,
    /// 用户在审阅窗里排除的变更块（按 `diff_hunks::hunks` 的序号）。提案正文变了
    /// （答完核实题、撤回概括等会在已完成的提案上继续修订）就清空，按新的块重新挑——
    /// `install_ai_proposal` 每次装提案都新建 `AiProposal`，天然满足这一点。
    pub(crate) excluded: BTreeSet<usize>,
    pub(crate) view: crate::diff_view::DiffViewState,
    pub(crate) open: bool,
    /// 侧栏里点了缺口所在句：下一帧在审阅窗里把这句所在的改动滚进视野。
    pub(crate) locate: Option<String>,
}

/// 一篇打开的稿件：内容、审校结果与这篇稿子自己的视图状态。
pub(crate) struct DraftSession {
    pub(crate) key: DocKey,
    /// 对应稿件库记录；为 None 时首次保存会新建记录。
    pub(crate) manuscript_id: Option<i64>,
    pub(crate) draft: DraftInput,
    pub(crate) generated_markdown: String,
    /// 当前稿件的中英文统计与实际分页缓存。
    pub(crate) statistics: statistics::DocumentStatistics,
    pub(crate) warnings: Vec<ReviewNote>,
    /// 上一次编译由孤行探针实测出来的提示，以及它对应的正文快照。孤行是排版
    /// 结果，只对那一份正文成立；正文一改这批提示立即作废，等下次编译再报。
    pub(crate) proof_warnings: Vec<ReviewNote>,
    pub(crate) proof_markdown: String,
    pub(crate) output_files: Vec<PathBuf>,
    /// 最近一次导出失败的原因；成功导出或换稿后清空。
    pub(crate) export_error: Option<String>,
    /// 点了「打印…」：这次导出结束后打开 PDF 并弹打印对话框。导出失败、
    /// 排版失败或没导出 PDF 时由外壳清掉，不打印。
    pub(crate) print_after_export: bool,
    /// 点了「输出」分区的 PDF / Word / Typst 包图标：这次导出结束后打开这份成品
    /// （Typst 包在文件管理器里定位）。导出失败或没出这种成品时由外壳清掉。
    pub(crate) open_after_export: Option<ExportKind>,
    /// 当前编辑内容来自的版本（起草页横幅）。
    pub(crate) loaded_version: Option<LoadedVersion>,
    /// 起草页“版本对照”模式的状态。
    pub(crate) draft_diff: DraftDiffState,
    /// 已切换为“手工输入”的字段 id；其余字段一律从标准词库选择。
    pub(crate) manual_fields: BTreeSet<String>,
    /// 左侧的文档要素填报区是否已收起。默认收起。
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
    /// 打开着的标题聚焦编辑对话框；None 表示没开。
    pub(crate) heading_focus: Option<heading_focus::HeadingFocus>,
    /// 公文引用插入与管理弹窗，插入点在打开时冻结。
    pub(crate) reference_picker: Option<references::ReferencePicker>,
    /// 公文引用的来源清单：源码编辑框据此给引用画线、出悬停卡片。用到时才读稿件库，
    /// 引用面板关闭或在卡片里登记后换新。
    pub(crate) citation_index: Option<references::CitationIndex>,
    /// 这次打开稿件后 .bib 是从哪个文件读进来的，文档要素的「重新读入」用。不存盘：
    /// 稿件里只存文件名与内容，换台机器路径就不作数了。
    pub(crate) bibliography_source: Option<std::path::PathBuf>,
    /// 候选区：写稿时暂时移出正文、以后可能还要用的文字。按稿件各存一份。
    pub(crate) candidates: candidates::CandidateState,
    /// 公文预览的缩放倍率；None 表示按面板宽度自适应。
    pub(crate) preview_zoom: Option<f32>,
    /// 上一帧自适应算出的倍率，用作手动加减档的起点。
    pub(crate) preview_fit_scale: f32,
    /// 拖动分隔条、缩放窗口时冻结版面的缩放状态（见 `preview::freeze`）。
    pub(crate) preview_freeze: crate::preview::ScaleFreeze,
    /// 公文预览滚动区上一帧的滚动位置与内容总高。右缘的导航刻度靠它把标题的
    /// 版面位置换算成刻度条上的位置，也靠它判断当前滚到了哪一节。
    pub(crate) preview_scroll: PreviewScroll,
    /// 源码模式的标题目录缓存；正文改变时重建。
    pub(crate) source_outline: SourceOutline,
    /// 源码模式的全文缩略图与编辑器滚动量。
    pub(crate) source_minimap: SourceMinimap,
    /// 在公文预览里点中的那一块：预览和源码两边都会给它铺底色。
    pub(crate) preview_anchor: Option<PreviewAnchor>,
    /// 分栏模式里编辑器的非空文字选区；预览按可见字符映射它，不复用整行锚点。
    pub(crate) preview_selection: Option<Range<usize>>,
    /// 源码编辑框中正在拖动的文字选区；按稿件隔离，避免切换标签后误改别篇。
    pub(crate) text_drag: Option<editor::TextDrag>,
    /// 最近一次智能粘贴（插入的字节范围、内容与原始纯文本），编辑区上方的
    /// 「改为原样粘贴」按钮用它。下一次改动那段文字、换稿或约 15 秒后清掉。
    pub(crate) recent_paste: Option<paste::RecentPaste>,
    /// 待处理的“跳到源码”请求，编辑框下次绘制时把光标挪过去并滚动到位。
    pub(crate) pending_source_jump: Option<usize>,
    /// 查找命中需要选中完整范围；普通预览点击仍只移动光标。
    pub(crate) pending_source_selection: Option<Range<usize>>,
    /// 查找导航后，公文预览下一帧滚动到当前命中所在的版式块。
    pub(crate) pending_render_jump: bool,
    /// 分栏模式中上一次同步到预览的 Markdown 行首；只在跨行时触发滚动。
    pub(crate) preview_cursor_line: Option<usize>,
    /// 切换显示方式后，源码编辑框下一帧把光标滚回视野并恢复焦点：换模式
    /// 重排后光标可能落在滚动区外，不拉回来用户就找不到刚才编辑的位置。
    pub(crate) pending_source_reveal: bool,
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
    /// 后台任务开始时冻结的审校稿；需要审阅的 AI 结果回来后与它做差异对照。
    pub(crate) ai_review_baseline: Option<String>,
    /// 尚未接受的 AI 修改提案。提案不参与自动保存，也不能直接导出。
    pub(crate) ai_proposal: Option<AiProposal>,
    /// 右侧 AI 侧栏：输入区、任务流与停止开关。按稿件各存一份，只在内存里。
    pub(crate) ai_panel: crate::ai_panel::AiPanel,
    /// 本篇待确认的修订建议。词表、文档规则与模型检查器共用这一份。
    pub(crate) revisions: crate::revision::RevisionSet,
    /// 逐句复核的结论缓存：句子指纹 → 模型原话。改了别处的句子不必重问一遍，
    /// 这是「一轮几十次本地推理」能反复跑的前提。
    pub(crate) revise_cache: std::collections::BTreeMap<u64, Option<String>>,
    /// 上次写入稿件库时的内容基线：`(要素 JSON, 正文)`。为 None 表示这篇
    /// 还没入过库。脏判定就是拿它和当前内容比。
    pub(crate) saved_baseline: Option<(String, String)>,
    /// 最新已提交版本的内容基线，判断“已存库但还没固化成版本”。
    /// 为 None 表示这篇一个版本都还没提交过。
    pub(crate) committed_baseline: Option<(String, String)>,
    /// 稿件库里这条记录的生命周期状态。发布与归档的稿件开成只读标签。
    pub(crate) record_status: ManuscriptStatus,
    /// 标签刚打开时的内容指纹。“第一次改动才自动入库”靠它区分
    /// “刚复制过来还没动” 和 “真的改了”。
    opened_fingerprint: String,
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

    /// 从文件夹导入一篇研究报告。研究报告元数据与正文分别保存，正文资源已在
    /// 导入阶段复制入应用资源库。
    pub(crate) fn with_research_markdown(
        key: DocKey,
        markdown: String,
        title: String,
        research: ResearchMetadata,
    ) -> Self {
        Self::from_parts(
            key,
            None,
            DraftInput {
                kind: TemplateKind::ResearchReport,
                title_hint: title,
                profile: TemplateProfile::for_kind(TemplateKind::ResearchReport),
                research,
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
        if (draft.kind.has_document_number() || draft.kind.has_minutes_number())
            && draft.profile.document_year.trim().is_empty()
        {
            draft.profile.document_year = draft.document_year();
        }
        let mut session = Self {
            key,
            manuscript_id,
            draft,
            generated_markdown,
            statistics: statistics::DocumentStatistics::default(),
            warnings: Vec::new(),
            proof_warnings: Vec::new(),
            proof_markdown: String::new(),
            output_files: Vec::new(),
            export_error: None,
            print_after_export: false,
            open_after_export: None,
            loaded_version: None,
            draft_diff: DraftDiffState::default(),
            manual_fields: BTreeSet::new(),
            // 打开稿件第一眼就是满屏审校稿；要改抬头文号再展开要素区。
            form_collapsed: true,
            form_sections: FormSectionState::default(),
            preview_mode: PreviewMode::Source,
            result_drawer_open: false,
            clear_review_confirm: false,
            heading_focus: None,
            reference_picker: None,
            citation_index: None,
            bibliography_source: None,
            candidates: candidates::CandidateState::default(),
            preview_zoom: None,
            preview_fit_scale: 1.0,
            preview_freeze: crate::preview::ScaleFreeze::default(),
            preview_scroll: PreviewScroll::default(),
            source_outline: SourceOutline::default(),
            source_minimap: SourceMinimap::default(),
            preview_anchor: None,
            preview_selection: None,
            text_drag: None,
            recent_paste: None,
            pending_source_jump: None,
            pending_source_selection: None,
            pending_render_jump: false,
            preview_cursor_line: None,
            pending_source_reveal: false,
            markdown_find: MarkdownFindState::default(),
            table_size: (3, 3),
            highlighter: MarkdownHighlighter::default(),
            busy: false,
            job_seq: 0,
            ai_prompt_last_label: String::new(),
            ai_review_baseline: None,
            ai_proposal: None,
            ai_panel: crate::ai_panel::AiPanel::default(),
            revisions: crate::revision::RevisionSet::default(),
            revise_cache: std::collections::BTreeMap::new(),
            saved_baseline: None,
            committed_baseline: None,
            record_status: ManuscriptStatus::Draft,
            opened_fingerprint: String::new(),
        };
        session.opened_fingerprint = session.fingerprint();
        session
    }

    /// 内容指纹：文档要素 + 正文。脏判定与“动过没有”都拿它比。
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

    /// 起草页点了「打印…」：记下「导出完自动打开并弹打印对话框」。导出真开始了
    /// 才记；导出失败或没排出 PDF 时由外壳清掉。
    pub(crate) fn request_print_after_export(&mut self) {
        self.print_after_export = true;
    }

    /// 取走「导出后打印」标记：取一次就清，免得下一次无关的导出又弹一次。
    pub(crate) fn take_print_after_export(&mut self) -> bool {
        std::mem::take(&mut self.print_after_export)
    }

    /// 取走「导出后打开」的成品种类：取一次就清。
    pub(crate) fn take_open_after_export(&mut self) -> Option<ExportKind> {
        self.open_after_export.take()
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

    /// 标签与状态栏用的短标题，口径见 [`export::document_title`]。
    pub(crate) fn title(&self) -> String {
        let title = export::document_title(&self.draft, &self.generated_markdown);
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
        self.heading_focus = None;
        self.ai_review_baseline = None;
        self.ai_proposal = None;
        // 建议锚在正文上，换了正文就全部失效；忽略记录也只对这一篇成立。
        self.revisions.clear();
        self.revise_cache.clear();
        self.preview_anchor = None;
        self.preview_selection = None;
        self.recent_paste = None;
        self.pending_source_jump = None;
        self.pending_source_selection = None;
        self.pending_render_jump = false;
        self.preview_cursor_line = None;
        self.pending_source_reveal = false;
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
    /// 检查器埋点。采纳、忽略、撤销都要记一笔，否则没法判断哪个检查器在帮倒忙。
    pub(crate) metrics: &'a mut crate::metrics::Metrics,
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
mod split_resize_tests;
#[cfg(test)]
mod tests;
