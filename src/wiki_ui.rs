//! 维基知识库页：左侧主题→分类→页面树，中间页面阅读器（`[[内链]]` 可点击跳转），
//! 右侧 backlinks 窄栏（常驻可折叠）。设计方案见 `docs/wiki-knowledge-base-design.md` §9。
//!
//! 口径 14（配色跟随主题）：本页不引入独立配色，全部取 `theme` 语义色。
//! 正文复用研究报告预览。维基跳转控件由 [`show_page_body`] 提供。

use std::collections::HashMap;

use crate::app::GongwenApp;
use crate::models::NumberingConfig;
use crate::theme;
use crate::wiki::{Backlink, WikiPage, WikiPageMeta, WikiTree};

mod job;
#[cfg(test)]
mod samples;
mod workspaces;
use eframe::egui;
pub(crate) use job::{Outcome, WikiRequest, WikiRun, source_rail};
use workspaces::{FrameTab, Route};

/// 知识库页的界面状态：框架树缓存、当前阅读页、backlinks 栏开合、框架编辑。
#[derive(Default)]
pub(crate) struct WikiUiState {
    /// 框架树缓存（list_tree 结果）。
    tree: Vec<WikiTree>,
    /// 各节点（主题/分类）下的页面清单，node_id → 页面。
    pages: HashMap<i64, Vec<WikiPageMeta>>,
    /// 缓存已建过没有；首帧与写操作后置脏。
    tree_loaded: bool,
    tree_dirty: bool,
    /// 当前阅读页（含正文与当前版本元数据）。
    open: Option<crate::wiki::WikiPage>,
    /// 当前页的入链（backlinks）与已解析的链接跳转表（打开页面时算一次）。
    backlinks: Vec<Backlink>,
    backlink_context: HashMap<String, String>,
    open_links: HashMap<String, LinkTarget>,
    /// 当前页当前版本的依据清单与「依据已失效」标记（打开页面时算一次，阅读器不每帧查库）。
    doc_refs: Vec<(i64, String)>,
    missing_sources: bool,
    route: Route,
    rail_hidden: bool,
    job_tab: bool,
    sources_tab: bool,
    evidence_original: Option<(i64, String, String)>,
    evidence_error: Option<String>,
    pub(crate) source_pages: HashMap<i64, Vec<(String, String)>>,
    search: String,
    search_hits: Vec<crate::wiki::WikiSearchHit>,
    collapsed_nodes: std::collections::HashSet<i64>,
    pub(crate) frame_run: Option<WikiRun>,
    pub(crate) page_run: Option<WikiRun>,
    frame_tab: FrameTab,
    frame_node: Option<(usize, Option<usize>)>,
    review_diff: bool,
    reviewed: Vec<workspaces::ReviewedBatch>,
    show_processed: bool,
    processed_batch: usize,
    history_full: bool,
    history_cache: Option<workspaces::HistoryCache>,
    /// 树编辑选中的框架节点 id（改名 / 移动 / 删除的对象）。
    selected_node: Option<i64>,
    move_target: Option<(i64, i64)>,
    /// 选中节点的名称编辑框内容。
    node_name_edit: String,
    /// 「新建主题」的名称输入。
    new_node_name: String,
    /// 「在主题下新建分类」的名称输入（与新建主题分开，避免误建错层级）。
    new_category_name: String,
    /// 框架树缓存读取失败的原因（失败时保留脏标记，下帧重试）。
    cache_error: Option<String>,
    /// 「导入框架」对话框开关与粘贴内容。
    import_text: String,
    /// 「从资料归纳」作业：停止请求、失败原因与待确认提案。
    pub(crate) request_stop_frame: bool,
    pub(crate) frame_error: Option<String>,
    pub(crate) frame_proposal: Option<crate::pi_agent::wiki_jobs::FrameProposal>,
    /// 「整编此分类」作业与审核台。
    pub(crate) request_stop_write: bool,
    pub(crate) write_error: Option<String>,
    pub(crate) review: Option<ReviewState>,
    /// 版本历史窗口：A/B 两版行级对比、回滚（回滚也产生新版本）。
    /// 版本历史窗口当前绑定的页面 UUID：切页时重置 A/B，避免串页回滚。
    history_uuid: Option<String>,
    /// 对比的 A（旧）/ B（新）版本号。
    history_a: Option<i64>,
    history_b: Option<i64>,
    /// 回滚二次确认开关与失败原因。
    history_confirm_rollback: bool,
    history_error: Option<String>,
    /// 缺口清单：红链聚合与「需复核」页（树缓存重建时刷新）及窗口开关。
    gaps: Vec<crate::wiki::RedLinkGap>,
    needs_review: Vec<(String, String)>,
    gap_open: bool,
    /// 「从缺口建页」对话框：标题来自红链，归属手选分类。
    create_open: bool,
    create_title: String,
    create_node: Option<i64>,
    create_body: String,
    create_error: Option<String>,
    /// 页面改名 / 删除对话框（页面身份是 UUID；删除要过反链保护）。
    page_op_open: bool,
    page_op_uuid: String,
    page_op_rename: String,
    page_op_confirm_delete: bool,
    page_op_error: Option<String>,
    /// 最近一次框架操作的提示（成功说明或失败原因）。
    notice: Option<String>,
}

impl WikiUiState {
    /// 资料库增删后让知识库页下一帧重读框架、页面与缺口缓存。
    pub(crate) fn invalidate(&mut self) {
        self.tree_dirty = true;
        self.evidence_original = None;
        self.evidence_error = None;
    }
}

/// 整编审核台：一批待审页面提案 + 逐页处理状态 + 编辑缓冲。
/// 审核时可直接改正文（纠正口径），采纳时把「是否经人工修改」记进版本。
pub(crate) struct ReviewState {
    pub(crate) set: crate::pi_agent::wiki_jobs::PageProposalSet,
    /// 每页是否已处理（采纳或退回）。
    pub(crate) done: Vec<bool>,
    /// 已采纳页数（全部处理完时给结论提示）。
    pub(crate) adopted: usize,
    accepted: Vec<bool>,
    pub(crate) selected: usize,
    /// 编辑框：当前页的 Markdown 源（可改，采纳的是这里的内容）。
    pub(crate) edit_body: String,
    /// 编辑框载入时的原始正文；与当前内容不同即「审核已修改」。
    pub(crate) edit_base: String,
    drafts: HashMap<usize, String>,
    /// 更新提案的 diff 缓存：正文/版本/编辑内容不变时不重算 LCS（大页每帧重算会卡）。
    diff: Option<ReviewDiffCache>,
    /// 上一次采纳/退回的时间：双击的第二次点击会落在同一位置的按钮上，
    /// 必须挡住，否则第二页还没被审阅就被入库。
    last_action: Option<std::time::Instant>,
    /// 采纳/写库失败的即时提示。
    pub(crate) error: Option<String>,
}

/// 更新提案的 diff 缓存（审核台切换页面或编辑正文会使它失效）。
struct ReviewDiffCache {
    uuid: String,
    version: i64,
    current: String,
    edit: String,
    lines: Option<Vec<DiffLine>>,
}

impl ReviewState {
    pub(crate) fn new(set: crate::pi_agent::wiki_jobs::PageProposalSet) -> Self {
        let body = set
            .pages
            .first()
            .map(|page| page.body.clone())
            .unwrap_or_default();
        Self {
            done: vec![false; set.pages.len()],
            adopted: 0,
            accepted: vec![false; set.pages.len()],
            selected: 0,
            edit_body: body.clone(),
            edit_base: body,
            drafts: HashMap::new(),
            diff: None,
            last_action: None,
            set,
            error: None,
        }
    }

    /// 双击防抖：一次点击只处理一页。400ms 覆盖 egui 的双击间隔。
    fn action_allowed(&mut self) -> bool {
        let now = std::time::Instant::now();
        if self
            .last_action
            .is_some_and(|at| now.duration_since(at) < std::time::Duration::from_millis(400))
        {
            return false;
        }
        self.last_action = Some(now);
        true
    }

    fn select(&mut self, index: usize) {
        if index == self.selected
            || index >= self.set.pages.len()
            || self.done.get(index) == Some(&true)
        {
            return;
        }
        if !self.done[self.selected] && self.edit_body != self.edit_base {
            self.drafts.insert(self.selected, self.edit_body.clone());
        }
        self.selected = index;
        self.error = None;
        let body = self.set.pages[index].body.clone();
        self.edit_body = self.drafts.remove(&index).unwrap_or_else(|| body.clone());
        self.edit_base = body;
    }
}

/// 框架里找到一个节点：名字、是不是主题、所属主题 id。
fn find_node(tree: &[WikiTree], id: i64) -> Option<(String, bool, i64)> {
    for t in tree {
        if t.theme.id == id {
            return Some((t.theme.name.clone(), true, t.theme.id));
        }
        if let Some(c) = t.categories.iter().find(|c| c.id == id) {
            return Some((c.name.clone(), false, t.theme.id));
        }
    }
    None
}

/// 页面入口。
pub(crate) fn wiki_ui(app: &mut GongwenApp, ui: &mut egui::Ui, numbering: &NumberingConfig) {
    // 库打不开时只报错，与资料库、稿件管理页的处置一致。
    if app.wiki_store.is_none() {
        ui.colored_label(
            theme::warn(),
            app.wiki_error
                .clone()
                .unwrap_or_else(|| "知识库（维基）不可用。".to_string()),
        );
        return;
    }
    if !app.wiki.tree_loaded || app.wiki.tree_dirty {
        // 读取失败时保留脏标记，下帧重试；不能把空缓存当成「框架为空」。
        if rebuild_cache(app) {
            if let Some(uuid) = app.wiki.open.as_ref().map(|page| page.meta.uuid.clone()) {
                open_page(app, &uuid);
            }
            app.wiki.tree_loaded = true;
            app.wiki.tree_dirty = false;
        }
    }
    if let Some(error) = &app.wiki.cache_error {
        ui.colored_label(theme::warn(), error);
    }
    workspaces::show(app, ui, numbering);
    gap_window(app, ui.ctx());
    create_page_window(app, ui.ctx());
    page_op_window(app, ui.ctx());
}

/// 采纳当前页：新建页走 `create_page`（建页 + v1）；更新提案走
/// `update_page`（校验作业开始时的版本后写新版本）。两者都带来源与「审核已修改」标记。
fn review_adopt(app: &mut GongwenApp) {
    let Some(review) = app.wiki.review.as_mut() else {
        return;
    };
    if review.done.get(review.selected) == Some(&true) {
        return;
    }
    // 防双击：采纳后选中项会移到下一页、按钮位置不变，未防抖会把下一页也采纳。
    if !review.action_allowed() {
        return;
    }
    let Some(page) = review.set.pages.get(review.selected).cloned() else {
        return;
    };
    let body = review.edit_body.clone();
    if let Err(error) = crate::pi_agent::wiki_jobs::validate_review_body(&body, &page.doc_refs) {
        review.error = Some(error);
        return;
    }
    let edited = review.edit_body != review.edit_base;
    let run_id = review.set.run_id.clone();
    let node_id = review.set.node_id;
    let store = app.wiki_store.as_ref();
    let result = match (&page.uuid, store) {
        (Some(uuid), Some(store)) => store
            .get_page(uuid)
            .map_err(|error| format!("读取目标页失败：{error:#}"))
            .and_then(|current| current.ok_or_else(|| "目标页已不存在。".to_string()))
            .and_then(|current| {
                let input = crate::wiki::PageInput {
                    title: page.title.clone(),
                    node_id: current.meta.node_id,
                    kind: if current.meta.kind == "guide" {
                        "guide"
                    } else {
                        "normal"
                    },
                    body: body.clone(),
                    source_run_id: run_id.clone(),
                    review_edited: edited,
                    doc_refs: page.doc_refs.clone(),
                    base_version: page.base_version,
                };
                store
                    .update_page(uuid, &input)
                    .map(|_| ())
                    .map_err(|error| format!("写新版本失败：{error:#}"))
            }),
        (None, Some(store)) => {
            let input = crate::wiki::PageInput {
                title: page.title.clone(),
                node_id,
                kind: if page.kind == "guide" {
                    "guide"
                } else {
                    "normal"
                },
                body,
                source_run_id: run_id,
                review_edited: edited,
                doc_refs: page.doc_refs.clone(),
                base_version: page.base_version,
            };
            store
                .create_page(&input)
                .map(|_| ())
                .map_err(|error| format!("采纳失败：{error:#}"))
        }
        (_, None) => Err("知识库不可用。".to_string()),
    };
    match result {
        Ok(()) => {
            app.knowledge_dirty = true;
            app.wiki.tree_dirty = true;
            review.adopted += 1;
            review.accepted[review.selected] = true;
            review_mark_done_advance(review);
        }
        Err(message) => review.error = Some(message),
    }
}

/// 把当前页标记为已处理（采纳或退回），选中下一个待审页（没有则停在原地）。
fn review_mark_done_advance(review: &mut ReviewState) {
    if let Some(done) = review.done.get_mut(review.selected) {
        *done = true;
    }
    review.error = None;
    if let Some(next) = review.done.iter().position(|done| !*done) {
        review.select(next);
    }
}

/// 版本历史窗口：顶部 A/B 两版选择 + 行级 diff（红删主题增），
/// 底部回滚（二次确认；回滚产生新版本，历史不丢）。
/// 行级 diff 的一行：相同 / 新增（B 有 A 无）/ 删除（A 有 B 无）。
enum DiffLine {
    Same(String),
    Add(String),
    Del(String),
}

/// 渲染行级 diff 小结 + 清单：删除行红色、新增行主题强调色、相同行弱色。
fn diff_view(ui: &mut egui::Ui, title: &str, lines: &[DiffLine], max_height: f32) {
    ui.weak(format!(
        "{title}：− 删除 {} 行，+ 新增 {} 行",
        lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Del(_)))
            .count(),
        lines
            .iter()
            .filter(|l| matches!(l, DiffLine::Add(_)))
            .count()
    ));
    egui::ScrollArea::vertical()
        .max_height(max_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for line in lines {
                match line {
                    DiffLine::Same(text) => {
                        ui.weak(format!("  {text}"));
                    }
                    DiffLine::Del(text) => {
                        ui.label(egui::RichText::new(format!("− {text}")).color(theme::warn()));
                    }
                    DiffLine::Add(text) => {
                        ui.label(
                            egui::RichText::new(format!("+ {text}")).color(theme::accent_text()),
                        );
                    }
                }
            }
        });
}

/// 行数乘积上限：动态规划表超此数就不算 diff，由调用方降级提示。
const DIFF_MAX_CELLS: usize = 4_000_000;

/// 行级 diff（LCS 动态规划）：语义同 `diff` 的「最短编辑脚本」。
/// 维基正文单页至多两万字符，正常远低于此上限。
fn line_diff(old: &str, new: &str) -> Option<Vec<DiffLine>> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len().saturating_mul(b.len()) > DIFF_MAX_CELLS {
        return None;
    }
    let (n, m) = (a.len(), b.len());
    // dp[i][j] = a[i..] 与 b[j..] 的最长公共子序列长度。
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut lines = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            lines.push(DiffLine::Same(a[i].to_string()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            lines.push(DiffLine::Del(a[i].to_string()));
            i += 1;
        } else {
            lines.push(DiffLine::Add(b[j].to_string()));
            j += 1;
        }
    }
    for rest in &a[i..] {
        lines.push(DiffLine::Del((*rest).to_string()));
    }
    for rest in &b[j..] {
        lines.push(DiffLine::Add((*rest).to_string()));
    }
    Some(lines)
}

/// 缺口清单：待建页红链（可直接建页补齐，建页后旧红链自动解析）与
/// 无页面覆盖的分类（可一键发起整编）。
fn gap_window(app: &mut GongwenApp, ctx: &egui::Context) {
    if !app.wiki.gap_open {
        return;
    }
    let mut open = true;
    let mut nav = None;
    egui::Window::new("缺口清单")
        .collapsible(false)
        .resizable(true)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            // 缺口可能很多：整个清单放进有高度上限的滚动区，否则后面的条目
            // 会被窗口裁掉且无法滚动。
            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - 12.0).max(220.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let gaps = app.wiki.gaps.clone();
                    let empty = empty_categories(app);
                    let missing = app.wiki.needs_review.clone();
                    let no_guide = nodes_without_guide(app);
                    if gaps.is_empty()
                        && empty.is_empty()
                        && missing.is_empty()
                        && no_guide.is_empty()
                    {
                        ui.weak("没有缺口：红链都有落点，分类都有条目页，节点都有导览页。");
                        return;
                    }
                    if !missing.is_empty() {
                        ui.strong("需复核（依据资料已删除或替换）");
                        for (uuid, title) in &missing {
                            if ui.link(title).clicked() {
                                nav = Some(uuid.clone());
                            }
                        }
                        ui.add_space(6.0);
                    }
                    if !no_guide.is_empty() {
                        ui.label(egui::RichText::new("缺导览页的节点").strong());
                        for (theme, category, id) in &no_guide {
                            ui.horizontal(|ui| {
                                let label = if theme.is_empty() {
                                    format!("{category}（主题）")
                                } else {
                                    format!("{theme} / {category}")
                                };
                                ui.label(label);
                                if ui.button("整编导览").clicked()
                                    && app.start_wiki_write(*id, Vec::new())
                                {
                                    app.wiki.gap_open = false;
                                }
                            });
                        }
                        ui.add_space(6.0);
                    }
                    if !gaps.is_empty() {
                        ui.label(egui::RichText::new("待建页（正文里的红链）").strong());
                        for gap in &gaps {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&gap.title).strong());
                                let sources = gap
                                    .refs
                                    .iter()
                                    .map(|link| link.from_title.as_str())
                                    .collect::<Vec<_>>()
                                    .join("、");
                                ui.weak(format!("被 {} 页引用", gap.refs.len()))
                                    .on_hover_text(format!("引用来源：{sources}"));
                                if ui.button("建此页").clicked() {
                                    app.wiki.create_open = true;
                                    app.wiki.create_title = gap.title.clone();
                                    app.wiki.create_node = None;
                                    app.wiki.create_body = String::new();
                                    app.wiki.create_error = None;
                                }
                            });
                        }
                        ui.add_space(6.0);
                    }
                    if !empty.is_empty() {
                        ui.label(egui::RichText::new("还没有条目页的分类").strong());
                        for (theme, category, id) in &empty {
                            ui.horizontal(|ui| {
                                ui.label(format!("{theme} / {category}"));
                                if ui.button("整编此分类").clicked()
                                    && app.start_wiki_write(*id, Vec::new())
                                {
                                    app.wiki.gap_open = false;
                                }
                            });
                        }
                    }
                });
        });
    if let Some(uuid) = nav {
        open_page(app, &uuid);
        open = false;
    }
    if !open {
        app.wiki.gap_open = false;
    }
}

/// 「从缺口建页」对话框：标题固定（红链目标），归属手选分类，正文人工撰写；
/// 写库后指向该标题的既有红链自动解析为页面链接。
fn create_page_window(app: &mut GongwenApp, ctx: &egui::Context) {
    if !app.wiki.create_open {
        return;
    }
    let mut open = true;
    let mut create = false;
    egui::Window::new("建页（缺口补齐）")
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("标题");
                ui.label(egui::RichText::new(&app.wiki.create_title).strong());
            });
            ui.horizontal(|ui| {
                ui.label("归属分类");
                let options: Vec<(i64, String)> = app
                    .wiki
                    .tree
                    .iter()
                    .flat_map(|t| {
                        t.categories
                            .iter()
                            .map(|c| (c.id, format!("{} / {}", t.theme.name, c.name)))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let mut selected = app.wiki.create_node.unwrap_or(0);
                egui::ComboBox::from_id_salt("wiki_create_node")
                    .selected_text(
                        options
                            .iter()
                            .find(|(id, _)| Some(*id) == app.wiki.create_node)
                            .map(|(_, label)| label.clone())
                            .unwrap_or_else(|| "选一个分类".into()),
                    )
                    .show_ui(ui, |ui| {
                        for (id, label) in &options {
                            ui.selectable_value(&mut selected, *id, label);
                        }
                    });
                app.wiki.create_node = (selected != 0).then_some(selected);
            });
            ui.weak(
                "正文写法与公文一致，可用 [[页面链接]]；来源记为「人工建页」。                 人工建页没有资料依据，只供人工阅读，不参与问答/写作自动取证；                 需要作为知识被引用时，请再走一次页面更新补上依据。",
            );
            ui.add(
                egui::TextEdit::multiline(&mut app.wiki.create_body)
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
            if let Some(error) = &app.wiki.create_error {
                ui.colored_label(theme::warn(), error);
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(app.wiki.create_node.is_some(), egui::Button::new("创建"))
                    .clicked()
                {
                    create = true;
                }
                if ui.button("取消").clicked() {
                    app.wiki.create_open = false;
                }
            });
        });
    if !open {
        app.wiki.create_open = false;
    }
    if create && let (Some(node_id), Some(store)) = (app.wiki.create_node, app.wiki_store.as_ref())
    {
        let input = crate::wiki::PageInput {
            title: app.wiki.create_title.clone(),
            node_id,
            kind: "normal",
            body: app.wiki.create_body.clone(),
            source_run_id: "manual".into(),
            review_edited: false,
            doc_refs: Vec::new(),
            base_version: None,
        };
        match store.create_page(&input) {
            Ok(uuid) => {
                app.wiki.notice = Some(format!("已建页「{}」。", input.title));
                app.wiki.create_open = false;
                app.wiki.tree_dirty = true;
                open_page(app, &uuid);
            }
            Err(error) => app.wiki.create_error = Some(format!("建页失败：{error:#}")),
        }
    }
}

/// 页面改名 / 删除：改名按 UUID 跟随（不产生新版本），删除要先过反链保护，
/// 并把确认后的删除交给存储层再次判定。
fn page_op_window(app: &mut GongwenApp, ctx: &egui::Context) {
    if !app.wiki.page_op_open {
        return;
    }
    let uuid = app.wiki.page_op_uuid.clone();
    if uuid.is_empty() {
        app.wiki.page_op_open = false;
        return;
    }
    let mut open = true;
    let mut rename = false;
    let mut delete = false;
    egui::Window::new("页面改名与删除")
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.weak(
                "页面身份按 UUID 保存：改名不会断开任何链接，也不产生新版本；                 删除前必须先解除其他页面对本页的引用。",
            );
            ui.horizontal(|ui| {
                ui.label("标题");
                ui.add(
                    egui::TextEdit::singleline(&mut app.wiki.page_op_rename)
                        .desired_width(280.0),
                );
            });
            if let Some(error) = &app.wiki.page_op_error {
                ui.colored_label(theme::warn(), error);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !app.wiki.page_op_rename.trim().is_empty(),
                        egui::Button::new("改名"),
                    )
                    .clicked()
                {
                    rename = true;
                }
                if ui
                    .button(egui::RichText::new("删除本页").color(theme::warn()))
                    .clicked()
                {
                    app.wiki.page_op_confirm_delete = true;
                }
                if ui.button("取消").clicked() {
                    app.wiki.page_op_open = false;
                }
            });
            if app.wiki.page_op_confirm_delete {
                ui.add_space(4.0);
                ui.colored_label(
                    theme::warn(),
                    "确认删除？本页的全部版本历史会一起删除，不能恢复。",
                );
                ui.horizontal(|ui| {
                    if ui
                        .button(egui::RichText::new("确认删除").color(theme::warn()))
                        .clicked()
                    {
                        delete = true;
                    }
                    if ui.button("取消删除").clicked() {
                        app.wiki.page_op_confirm_delete = false;
                    }
                });
            }
        });
    if !open {
        app.wiki.page_op_open = false;
    }
    if rename {
        let title = app.wiki.page_op_rename.trim().to_string();
        let result = app
            .wiki_store
            .as_ref()
            .map(|store| store.rename_page(&uuid, &title))
            .unwrap_or_else(|| Err(anyhow::anyhow!("知识库不可用")));
        match result {
            Ok(()) => {
                app.wiki.notice = Some(format!("页面已改名为「{title}」。"));
                app.wiki.page_op_open = false;
                app.wiki.tree_dirty = true;
                open_page(app, &uuid);
            }
            Err(error) => app.wiki.page_op_error = Some(format!("改名失败：{error:#}")),
        }
    } else if delete {
        let result = app
            .wiki_store
            .as_ref()
            .map(|store| store.delete_page(&uuid))
            .unwrap_or_else(|| Err(anyhow::anyhow!("知识库不可用")));
        match result {
            Ok(()) => {
                app.wiki.notice = Some("页面已删除（含全部版本历史）。".into());
                app.wiki.page_op_open = false;
                app.wiki.open = None;
                app.wiki.doc_refs = Vec::new();
                app.wiki.missing_sources = false;
                app.wiki.backlinks = Vec::new();
                app.wiki.backlink_context.clear();
                app.wiki.tree_dirty = true;
                app.knowledge_dirty = true;
            }
            Err(error) => app.wiki.page_op_error = Some(format!("删除失败：{error:#}")),
        }
    }
}

/// 还没有任何页面（含导览页）的分类：theme、category、node_id。
fn empty_categories(app: &GongwenApp) -> Vec<(String, String, i64)> {
    app.wiki
        .tree
        .iter()
        .flat_map(|t| {
            t.categories
                .iter()
                .filter(|c| {
                    // 只有导览页不算「有覆盖」：导览本身不承载条目知识。
                    app.wiki
                        .pages
                        .get(&c.id)
                        .is_none_or(|pages| !pages.iter().any(|page| page.kind == "normal"))
                })
                .map(|c| (t.theme.name.clone(), c.name.clone(), c.id))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 还没有导览页的主题/分类：(主题名, 节点名, 节点 id)；主题项的主题名为空。
fn nodes_without_guide(app: &GongwenApp) -> Vec<(String, String, i64)> {
    let mut out = Vec::new();
    for t in &app.wiki.tree {
        if t.theme_guide.is_none() {
            out.push((String::new(), t.theme.name.clone(), t.theme.id));
        }
        for c in &t.categories {
            if !t.category_guides.contains_key(&c.id) {
                out.push((t.theme.name.clone(), c.name.clone(), c.id));
            }
        }
    }
    out
}

// ── 缓存与链接解析 ──────────────────────────────────────────────────────────

/// 重建框架树、页面清单与缺口/需复核缓存。任何一步读失败都返回 false，
/// 由调用方保留脏标记重试，不能把「读失败」显示成「还没有知识框架」。
fn rebuild_cache(app: &mut GongwenApp) -> bool {
    let Some(store) = app.wiki_store.as_ref() else {
        return false;
    };
    let tree = match store.list_tree() {
        Ok(tree) => tree,
        Err(error) => {
            app.wiki.cache_error = Some(format!("知识框架读取失败：{error:#}"));
            return false;
        }
    };
    let mut pages = HashMap::new();
    for t in &tree {
        match store.list_pages(t.theme.id) {
            Ok(v) => {
                pages.insert(t.theme.id, v);
            }
            Err(error) => {
                app.wiki.cache_error = Some(format!("页面清单读取失败：{error:#}"));
                return false;
            }
        }
        for c in &t.categories {
            match store.list_pages(c.id) {
                Ok(v) => {
                    pages.insert(c.id, v);
                }
                Err(error) => {
                    app.wiki.cache_error = Some(format!("页面清单读取失败：{error:#}"));
                    return false;
                }
            }
        }
    }
    let gaps = store.red_link_gaps().unwrap_or_default();
    let needs_review = store.pages_needing_review().unwrap_or_default();
    app.wiki.tree = tree;
    app.wiki.pages = pages;
    app.wiki.gaps = gaps;
    if !app.wiki.search.trim().is_empty() {
        app.wiki.search_hits = store.search(app.wiki.search.trim(), 30).unwrap_or_default();
    }
    app.wiki.needs_review = needs_review;
    app.wiki.cache_error = None;
    true
}

/// 链接跳转目标：已建页、框架节点（导览页或待建导览）。解析表里没有的目标
/// 即红链（待建页），走行内渲染的兜底分支。
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkTarget {
    Page { uuid: String, title: String },
    Node { name: String, guide: Option<String> },
}

fn build_link_map(
    tree: &[WikiTree],
    pages: &HashMap<i64, Vec<WikiPageMeta>>,
) -> HashMap<String, LinkTarget> {
    let mut nodes = HashMap::new();
    for branch in tree {
        for node in std::iter::once(&branch.theme).chain(&branch.categories) {
            let guide = if node.id == branch.theme.id {
                branch.theme_guide.clone()
            } else {
                branch.category_guides.get(&node.id).cloned()
            };
            nodes
                .entry(node.name.clone())
                .and_modify(|target| *target = None)
                .or_insert_with(|| {
                    Some(LinkTarget::Node {
                        name: node.name.clone(),
                        guide,
                    })
                });
        }
    }
    let mut map: HashMap<_, _> = nodes
        .into_iter()
        .filter_map(|(name, target)| Some((name, target?)))
        .collect();
    for page in pages.values().flatten() {
        map.insert(
            page.title.clone(),
            LinkTarget::Page {
                uuid: page.uuid.clone(),
                title: page.title.clone(),
            },
        );
    }
    map
}

fn saved_link_map(app: &GongwenApp, page: &WikiPage) -> HashMap<String, LinkTarget> {
    let Some(bindings) = app
        .wiki_store
        .as_ref()
        .and_then(|store| store.link_bindings(&page.meta.uuid, &page.body).ok())
    else {
        return HashMap::new();
    };
    bindings
        .into_iter()
        .filter_map(|(name, link)| {
            if let Some(uuid) = link.to_page {
                let title = app
                    .wiki
                    .pages
                    .values()
                    .flatten()
                    .find(|page| page.uuid == uuid)
                    .map_or_else(|| name.clone(), |page| page.title.clone());
                return Some((name, LinkTarget::Page { uuid, title }));
            }
            let node = link.to_node?;
            let (node_name, _, _) = find_node(&app.wiki.tree, node)?;
            let guide = app
                .wiki
                .pages
                .get(&node)
                .into_iter()
                .flatten()
                .find(|page| page.kind == "guide")
                .map(|page| page.uuid.clone());
            Some((
                name,
                LinkTarget::Node {
                    name: node_name,
                    guide,
                },
            ))
        })
        .collect()
}

/// 供其他页面（资料库问答的维基依据等）跳转打开指定维基页。
pub(crate) fn open_wiki_page(app: &mut GongwenApp, uuid: &str) {
    app.wiki.route = Route::Browse;
    open_page(app, uuid);
}

fn open_page(app: &mut GongwenApp, uuid: &str) {
    let Some(store) = app.wiki_store.as_ref() else {
        return;
    };
    match store.get_page(uuid) {
        Ok(Some(page)) => {
            // 阅读器每帧都要显示依据与失效提示：在这里查一次并缓存，
            // 不要每帧再查两遍库。
            app.wiki.doc_refs = store.page_doc_refs(uuid).unwrap_or_default();
            app.wiki.missing_sources = store.page_has_missing_sources(uuid).unwrap_or(false);
            app.wiki.backlinks = store.backlinks(uuid).unwrap_or_default();
            app.wiki.backlink_context =
                app.wiki
                    .backlinks
                    .iter()
                    .take(16)
                    .filter_map(|backlink| {
                        let from = store.get_page(&backlink.from_uuid).ok().flatten()?;
                        let bindings = store.link_bindings(&from.meta.uuid, &from.body).ok()?;
                        let (span, link) = crate::wiki::wikilink_spans(&from.body)
                            .into_iter()
                            .find(|(_, link)| {
                                bindings.get(&link.target).is_some_and(|edge| {
                                    edge.to_page.as_deref() == Some(uuid)
                                        || (page.meta.kind == "guide"
                                            && edge.to_node == Some(page.meta.node_id))
                                })
                            })?;
                        let before = &from.body[..span.start];
                        let after = &from.body[span.end..];
                        let start = before
                            .char_indices()
                            .rev()
                            .nth(19)
                            .map_or(0, |(index, _)| index);
                        let end = after
                            .char_indices()
                            .nth(24)
                            .map_or(after.len(), |(index, _)| index);
                        let excerpt = format!("{}{}{}", &before[start..], link.text, &after[..end]);
                        Some((
                            backlink.from_uuid.clone(),
                            excerpt.split_whitespace().collect::<Vec<_>>().join(" "),
                        ))
                    })
                    .collect();
            app.wiki.open_links = saved_link_map(app, &page);
            app.wiki.collapsed_nodes.remove(&page.meta.node_id);
            if let Some((_, _, parent)) = find_node(&app.wiki.tree, page.meta.node_id) {
                app.wiki.collapsed_nodes.remove(&parent);
            }
            app.wiki.open = Some(page);
        }
        Ok(None) => {
            app.wiki.open = None;
            app.wiki.doc_refs = Vec::new();
            app.wiki.missing_sources = false;
            app.wiki.backlinks = Vec::new();
            app.wiki.backlink_context.clear();
            app.wiki.open_links.clear();
        }
        Err(_) => {}
    }
}

// ── 左栏：框架树 ────────────────────────────────────────────────────────────

fn tree_pane(app: &mut GongwenApp, ui: &mut egui::Ui, nav: &mut Option<String>, manage: bool) {
    ui.strong("分类框架");
    ui.separator();
    if app.wiki.tree.is_empty() {
        ui.weak("尚未建立分类框架");
    }
    egui::ScrollArea::vertical()
        .id_salt(("wiki_tree", manage))
        .show(ui, |ui| {
            for branch in &app.wiki.tree {
                let theme = &branch.theme;
                if !node_row(
                    ui,
                    theme,
                    manage,
                    0,
                    &mut app.wiki.collapsed_nodes,
                    &mut app.wiki.selected_node,
                    &mut app.wiki.node_name_edit,
                ) {
                    continue;
                }
                if !manage {
                    for page in app.wiki.pages.get(&theme.id).into_iter().flatten() {
                        page_row(app, ui, page, nav);
                    }
                }
                for category in &branch.categories {
                    if !node_row(
                        ui,
                        category,
                        manage,
                        1,
                        &mut app.wiki.collapsed_nodes,
                        &mut app.wiki.selected_node,
                        &mut app.wiki.node_name_edit,
                    ) || manage
                    {
                        continue;
                    }
                    for page in app.wiki.pages.get(&category.id).into_iter().flatten() {
                        page_row(app, ui, page, nav);
                    }
                }
            }
        });
}

fn node_row(
    ui: &mut egui::Ui,
    node: &crate::wiki::WikiNode,
    manage: bool,
    indent: usize,
    collapsed_nodes: &mut std::collections::HashSet<i64>,
    selected_node: &mut Option<i64>,
    node_name_edit: &mut String,
) -> bool {
    let mut expanded = !collapsed_nodes.contains(&node.id);
    ui.horizontal(|ui| {
        ui.add_space(indent as f32 * 14.0);
        let icon = if expanded {
            theme::Icon::ChevronDown
        } else {
            theme::Icon::ChevronRight
        };
        let hint = if expanded { "折叠" } else { "展开" };
        if (!manage || indent == 0) && theme::icon_button(ui, icon, hint).clicked() {
            expanded = !expanded;
            if expanded {
                collapsed_nodes.remove(&node.id);
            } else {
                collapsed_nodes.insert(node.id);
            }
        }
        if ui
            .selectable_label(manage && *selected_node == Some(node.id), &node.name)
            .clicked()
        {
            if manage {
                *selected_node = Some(node.id);
                *node_name_edit = node.name.clone();
            } else if expanded {
                collapsed_nodes.insert(node.id);
                expanded = false;
            } else {
                collapsed_nodes.remove(&node.id);
                expanded = true;
            }
        }
    });
    expanded
}

/// 树里选中节点的编辑条：改名、分类跨主题移动、删除、在主题下建分类。
/// 交互复用标准词库页（口径 15）：行内输入 + 明确按钮，存储层守卫兜底。
fn node_edit_strip(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let Some(id) = app.wiki.selected_node else {
        return;
    };
    let Some((name, is_theme, theme_id)) = find_node(&app.wiki.tree, id) else {
        app.wiki.selected_node = None;
        return;
    };
    // 作业运行中不改进框架：删掉正在整编的目标分类会让整批提案作废。
    let busy = app.wiki.busy();
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(if is_theme { "主题" } else { "分类" });
            ui.add(egui::TextEdit::singleline(&mut app.wiki.node_name_edit).desired_width(120.0));
            let new_name = app.wiki.node_name_edit.trim().to_string();
            if ui
                .add_enabled(
                    !busy && !new_name.is_empty() && new_name != name,
                    egui::Button::new("改名"),
                )
                .clicked()
                && let Some(store) = app.wiki_store.as_ref()
            {
                match store.rename_node(id, &new_name) {
                    Ok(()) => {
                        app.wiki.notice = Some(format!("「{name}」已改名为「{new_name}」。"));
                        app.wiki.tree_dirty = true;
                    }
                    Err(error) => app.wiki.notice = Some(format!("改名失败：{error:#}")),
                }
            }
        });
        if is_theme {
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut app.wiki.new_category_name)
                    .desired_width(120.0)
                    .hint_text("分类名称");
                let resp = ui.add(edit);
                let category = app.wiki.new_category_name.trim().to_string();
                let create = ui
                    .add_enabled(!busy && !category.is_empty(), egui::Button::new("建分类"))
                    .clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if create {
                    if let Some(store) = app.wiki_store.as_ref() {
                        match store.create_category(id, &category) {
                            Ok(cid) => {
                                app.wiki.notice = Some(format!("已建分类「{category}」。"));
                                app.wiki.tree_dirty = true;
                                app.wiki.selected_node = Some(cid);
                                app.wiki.node_name_edit = category.clone();
                            }
                            Err(error) => app.wiki.notice = Some(format!("建分类失败：{error:#}")),
                        }
                    }
                    app.wiki.new_category_name.clear();
                }
            });
        } else {
            // 分类跨主题移动。
            ui.horizontal(|ui| {
                ui.label("移到主题");
                let current = theme_id;
                let mut target = app
                    .wiki
                    .move_target
                    .filter(|(node, _)| *node == id)
                    .map_or(current, |(_, target)| target);
                egui::ComboBox::from_id_salt("wiki_move_theme")
                    .selected_text(
                        app.wiki
                            .tree
                            .iter()
                            .find(|t| t.theme.id == target)
                            .map(|t| t.theme.name.as_str())
                            .unwrap_or(""),
                    )
                    .show_ui(ui, |ui| {
                        for t in &app.wiki.tree {
                            ui.selectable_value(&mut target, t.theme.id, &t.theme.name);
                        }
                    });
                app.wiki.move_target = Some((id, target));
                if ui
                    .add_enabled(!busy && target != current, egui::Button::new("移过去"))
                    .clicked()
                    && let Some(store) = app.wiki_store.as_ref()
                {
                    match store.move_category(id, target) {
                        Ok(()) => {
                            app.wiki.notice = Some(format!("已把「{name}」移走。"));
                            app.wiki.tree_dirty = true;
                        }
                        Err(error) => app.wiki.notice = Some(format!("移动失败：{error:#}")),
                    }
                }
            });
        }
        let has_theme_guide = app
            .wiki
            .tree
            .iter()
            .any(|t| t.theme.id == id && t.theme_guide.is_some());
        if (!is_theme || !has_theme_guide)
            && ui
                .add_enabled(
                    !app.wiki.busy(),
                    theme::icon_text_button(
                        theme::Icon::Sparkles,
                        if is_theme {
                            "整编主题导览"
                        } else {
                            "整编此分类"
                        },
                    ),
                )
                .clicked()
        {
            app.start_wiki_write(id, Vec::new());
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(egui::RichText::new("删除节点").color(theme::warn())),
                )
                .clicked()
                && let Some(store) = app.wiki_store.as_ref()
            {
                match store.delete_node(id) {
                    Ok(()) => {
                        app.wiki.notice = Some(format!("已删除「{name}」。"));
                        app.wiki.selected_node = None;
                        app.wiki.tree_dirty = true;
                    }
                    Err(error) => app.wiki.notice = Some(format!("删不掉：{error:#}")),
                }
            }
        });
    });
}

fn page_row(app: &GongwenApp, ui: &mut egui::Ui, m: &WikiPageMeta, nav: &mut Option<String>) {
    let label = if m.kind == "guide" {
        format!("　　{}（导览）", m.title)
    } else {
        format!("　　{}", m.title)
    };
    if tree_row(ui, &label, page_selected(app, &m.uuid)).clicked() {
        *nav = Some(m.uuid.clone());
    }
}

fn page_selected(app: &GongwenApp, uuid: &str) -> bool {
    app.wiki.open.as_ref().is_some_and(|p| p.meta.uuid == uuid)
}

/// 树里的一行：整行可点，选中态底色跟随主题强调色。
fn tree_row(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let text = if selected {
        egui::RichText::new(label).color(theme::accent_text())
    } else {
        egui::RichText::new(label)
    };
    let mut resp = ui.selectable_label(selected, text);
    if selected {
        resp = resp.on_hover_text("当前阅读");
    }
    resp
}

// ── 中栏：页面阅读器 ─────────────────────────────────────────────────────────

fn reader(
    app: &mut GongwenApp,
    ui: &mut egui::Ui,
    nav: &mut Option<String>,
    numbering: &NumberingConfig,
) {
    let Some(page) = app.wiki.open.clone() else {
        ui.weak("在左侧框架树里选一个页面开始阅读。");
        return;
    };
    if let Some((name, is_theme, theme_id)) = find_node(&app.wiki.tree, page.meta.node_id) {
        let path = if is_theme {
            name
        } else {
            let parent = find_node(&app.wiki.tree, theme_id)
                .map(|node| node.0)
                .unwrap_or_default();
            format!("{parent} / {name}")
        };
        ui.weak(path);
    }
    // 页头：标题 + 元信息 + backlinks 栏开关。
    ui.horizontal(|ui| {
        ui.heading(&page.meta.title);
        if page.meta.kind == "guide" {
            theme::accent_scope(ui, |ui| {
                ui.small("导览页");
            });
        }
        ui.weak(format!("v{}", page.meta.current_version));
        if ui
            .small_button("改名")
            .on_hover_text("页面身份按 UUID 保存，改名后所有链接仍指本页，不产生新版本")
            .clicked()
        {
            app.wiki.page_op_open = true;
            app.wiki.page_op_uuid = page.meta.uuid.clone();
            app.wiki.page_op_rename = page.meta.title.clone();
            app.wiki.page_op_confirm_delete = false;
            app.wiki.page_op_error = None;
        }
        let refs = app.wiki.backlinks.len();
        let delete = ui.add_enabled(refs == 0, egui::Button::new("删除").small());
        if refs > 0 {
            delete.on_hover_text(format!(
                "被 {refs} 个页面引用，先解除引用才能删除（删除保护）"
            ));
        } else if delete
            .on_hover_text("删除本页及其全部版本历史，不能恢复")
            .clicked()
        {
            app.wiki.page_op_open = true;
            app.wiki.page_op_uuid = page.meta.uuid.clone();
            app.wiki.page_op_rename = page.meta.title.clone();
            app.wiki.page_op_confirm_delete = true;
            app.wiki.page_op_error = None;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.wiki.busy();
            if ui
                .add_enabled(!busy, theme::icon_text_button(theme::Icon::Refresh, "更新"))
                .on_hover_text(
                    "让 AI 按资料库新内容更新本页；提案经 diff 审核（可编辑）后才产生新版本",
                )
                .clicked()
            {
                app.start_wiki_update(
                    vec![(page.meta.uuid.clone(), page.meta.title.clone())],
                    None,
                );
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::History, "版本"))
                .on_hover_text("版本历史：两版对比、回滚（回滚也产生新版本，不丢历史）")
                .clicked()
            {
                app.wiki.route = Route::History;
                app.wiki.history_confirm_rollback = false;
                app.wiki.history_error = None;
                app.wiki.rail_hidden = false;
            }
            let icon = if !app.wiki.rail_hidden {
                theme::Icon::PanelClose
            } else {
                theme::Icon::PanelOpen
            };
            if ui.add(theme::icon_text_button(icon, "被引用")).clicked() {
                app.wiki.rail_hidden = !app.wiki.rail_hidden;
            }
        });
    });
    if app.wiki.missing_sources {
        ui.colored_label(theme::warn(), "部分依据已删除或替换，请复核本页。");
    }
    if app.wiki.doc_refs.is_empty() && !app.wiki.missing_sources {
        ui.weak("本页尚未登记依据，仅供人工阅读。");
    }
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    let result = egui::ScrollArea::vertical()
        .id_salt(&page.meta.uuid)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            show_page_body(
                ui,
                &page.body,
                &app.wiki.open_links,
                &app.wiki.doc_refs,
                numbering,
            )
        });
    if let Some(target) = result.inner {
        follow_body_target(app, target);
        *nav = None;
    }
}

fn show_doc_refs(ui: &mut egui::Ui, refs: &[(i64, String)], open: &mut Option<i64>) {
    if refs.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.weak("本页依据");
        for (id, note) in refs {
            if ui.link(format!("E{id}")).on_hover_text(note).clicked() {
                *open = Some(*id);
            }
        }
    });
}

fn link_display_text<'a>(
    link: &'a crate::wiki::RawLink,
    targets: &'a HashMap<String, LinkTarget>,
) -> &'a str {
    if link.text != link.target {
        return &link.text;
    }
    match targets.get(&link.target) {
        Some(LinkTarget::Page { title, .. }) => title,
        Some(LinkTarget::Node { name, .. }) => name,
        None => &link.text,
    }
}

#[derive(Clone)]
enum BodyTarget {
    Page(String),
    Source(i64),
    Missing,
}

/// 正文沿用研究报告预览。先把维基链接换成显示文字，再把命中范围交给排版层。
fn show_page_body(
    ui: &mut egui::Ui,
    body: &str,
    link_map: &HashMap<String, LinkTarget>,
    refs: &[(i64, String)],
    numbering: &NumberingConfig,
) -> Option<BodyTarget> {
    let mut targets = Vec::new();
    let mut markdown = String::with_capacity(body.len());
    let mut end = 0;
    for (span, link) in crate::wiki::wikilink_spans(body) {
        markdown.push_str(&body[end..span.start]);
        let start = markdown.len();
        markdown.push_str(link_display_text(&link, link_map));
        let target = match link_map.get(&link.target) {
            Some(LinkTarget::Page { uuid, .. }) => BodyTarget::Page(uuid.clone()),
            Some(LinkTarget::Node {
                guide: Some(uuid), ..
            }) => BodyTarget::Page(uuid.clone()),
            _ => BodyTarget::Missing,
        };
        let hint = if matches!(target, BodyTarget::Missing) {
            format!("待建页 · {}", link.target)
        } else {
            format!("打开 · {}", link_display_text(&link, link_map))
        };
        targets.push((start..markdown.len(), hint, target));
        end = span.end;
    }
    markdown.push_str(&body[end..]);
    for (start, _) in markdown.match_indices("〔E") {
        let after = start + "〔E".len();
        let Some((number, _)) = markdown[after..].split_once('〕') else {
            continue;
        };
        let Ok(id) = number.parse::<i64>() else {
            continue;
        };
        let Some((_, note)) = refs.iter().find(|(source, _)| *source == id) else {
            continue;
        };
        let span = start..after + number.len() + '〕'.len_utf8();
        if targets
            .iter()
            .any(|(link, _, _)| link.start < span.end && span.start < link.end)
        {
            continue;
        }
        targets.push((span, format!("依据 E{id} · {note}"), BodyTarget::Source(id)));
    }
    let links = targets
        .iter()
        .map(|(source, hint, target)| crate::preview::InlineLink {
            source: source.clone(),
            hint: hint.clone(),
            color: if matches!(target, BodyTarget::Missing) {
                theme::warn()
            } else {
                theme::accent()
            },
        })
        .collect();
    let clicked = crate::preview::research_body_preview(
        ui,
        crate::preview::InlineLinks { markdown, links },
        numbering,
    )
    .clicked;
    if let Some(clicked) = clicked
        && let Some((_, _, target)) = targets.iter().find(|(span, _, _)| *span == clicked)
    {
        return Some(target.clone());
    }
    // 标题、公式和部分表格还使用整块命中，保留对这些位置的跳转入口。
    let mut chosen = None;
    if !targets.is_empty() {
        egui::CollapsingHeader::new("本页链接").show(ui, |ui| {
            let mut shown = std::collections::HashSet::new();
            for (_, label, target) in &targets {
                if shown.insert(label) && ui.link(label).clicked() {
                    chosen = Some(target.clone());
                }
            }
        });
    }
    chosen
}

fn follow_body_target(app: &mut GongwenApp, target: BodyTarget) {
    match target {
        BodyTarget::Page(uuid) => open_wiki_page(app, &uuid),
        BodyTarget::Source(id) => {
            app.wiki.sources_tab = true;
            app.wiki.job_tab = false;
            app.wiki.rail_hidden = false;
            workspaces::read_evidence(app, id);
        }
        BodyTarget::Missing => app.wiki.gap_open = true,
    }
}

fn backlinks_pane(app: &mut GongwenApp, ui: &mut egui::Ui, nav: &mut Option<String>) {
    ui.horizontal(|ui| {
        ui.heading("被引用");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::PanelClose, ""))
                .clicked()
            {
                app.wiki.rail_hidden = true;
            }
        });
    });
    ui.add_space(4.0);
    if app.wiki.backlinks.is_empty() {
        ui.weak("还没有其他页面引用当前页。");
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for b in &app.wiki.backlinks {
            if ui
                .selectable_label(false, format!("　{}", b.from_title))
                .on_hover_text(format!("引用文字：{}", b.text))
                .clicked()
            {
                *nav = Some(b.from_uuid.clone());
            }
            if let Some(context) = app.wiki.backlink_context.get(&b.from_uuid) {
                ui.weak(context);
            }
            ui.separator();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{DIFF_MAX_CELLS, DiffLine, line_diff};

    fn kinds(lines: &[DiffLine]) -> String {
        lines
            .iter()
            .map(|line| match line {
                DiffLine::Same(_) => '=',
                DiffLine::Add(_) => '+',
                DiffLine::Del(_) => '-',
            })
            .collect()
    }

    #[test]
    fn line_diff_marks_added_and_removed_lines() {
        let lines = line_diff("甲\n乙\n丙", "甲\n丁\n丙").expect("小文本应出 diff");
        assert_eq!(kinds(&lines), "=-+=");
        let same: Vec<&str> = lines
            .iter()
            .filter_map(|line| match line {
                DiffLine::Same(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(same, vec!["甲", "丙"]);
    }

    #[test]
    fn line_diff_handles_pure_growth_at_tail() {
        let lines = line_diff("甲", "甲\n乙\n丙").expect("小文本应出 diff");
        assert_eq!(kinds(&lines), "=++");
    }

    #[test]
    fn line_diff_empty_old_is_all_adds() {
        let lines = line_diff("", "甲\n乙").expect("小文本应出 diff");
        assert_eq!(kinds(&lines), "++");
    }

    #[test]
    fn line_diff_bails_out_on_huge_inputs() {
        let old: Vec<String> = (0..DIFF_MAX_CELLS / 100 + 2)
            .map(|i| format!("行{i}"))
            .collect();
        let new: Vec<String> = (0..DIFF_MAX_CELLS / 100 + 2)
            .map(|i| format!("列{i}"))
            .collect();
        assert!(line_diff(&old.join("\n"), &new.join("\n")).is_none());
    }
}
