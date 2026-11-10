//! 维基知识库页：左侧主题→分类→页面树，中间页面阅读器（`[[内链]]` 可点击跳转），
//! 右侧 backlinks 窄栏（常驻可折叠）。设计方案见 `docs/wiki-knowledge-base-design.md` §9。
//!
//! 口径 14（配色跟随主题）：本页不引入独立配色，全部取 `theme` 语义色。
//! 口径 13（渲染复用）：正文渲染集中在 [`show_page_body`]——它是换接点，后续切片
//! 把现有预览渲染链接入后，这里整体换掉、链接只作为行内挂件保留。

use std::collections::HashMap;

use crate::app::GongwenApp;
use crate::theme;
use crate::wiki::{Backlink, OutlineTheme, WikiPageMeta, WikiTree};
use eframe::egui;

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
    /// 当前页的入链（backlinks）。
    backlinks: Vec<Backlink>,
    /// 右侧 backlinks 栏是否展开。
    backlinks_open: bool,
    /// 树编辑选中的框架节点 id（改名 / 移动 / 删除的对象）。
    selected_node: Option<i64>,
    /// 选中节点的名称编辑框内容。
    node_name_edit: String,
    /// 「新建主题 / 在主题下建分类」的名称输入。
    new_node_name: String,
    /// 「导入框架」对话框开关与粘贴内容。
    import_open: bool,
    import_text: String,
    /// 「从资料归纳」作业：进度行、停止请求、失败原因与待确认提案。
    pub(crate) frame_progress: Option<String>,
    pub(crate) request_stop_frame: bool,
    pub(crate) frame_error: Option<String>,
    pub(crate) frame_proposal: Option<crate::pi_agent::wiki_jobs::FrameProposal>,
    /// 「整编此分类」作业与审核台。
    pub(crate) write_progress: Option<String>,
    pub(crate) request_stop_write: bool,
    pub(crate) write_error: Option<String>,
    pub(crate) review: Option<ReviewState>,
    /// 审核台窗口开关：关闭窗口不丢提案，树上方有入口可 reopen。
    pub(crate) review_open: bool,
    /// 最近一次框架操作的提示（成功说明或失败原因）。
    notice: Option<String>,
}

/// 整编审核台：一批待审页面提案 + 逐页处理状态 + 编辑缓冲。
/// 审核时可直接改正文（纠正口径），采纳时把「是否经人工修改」记进版本。
pub(crate) struct ReviewState {
    pub(crate) set: crate::pi_agent::wiki_jobs::PageProposalSet,
    /// 每页是否已处理（采纳或退回）。
    pub(crate) done: Vec<bool>,
    /// 已采纳页数（全部处理完时给结论提示）。
    pub(crate) adopted: usize,
    pub(crate) selected: usize,
    /// 编辑框：当前页的 Markdown 源（可改，采纳的是这里的内容）。
    pub(crate) edit_body: String,
    /// 编辑框载入时的原始正文；与当前内容不同即「审核已修改」。
    pub(crate) edit_base: String,
    /// 采纳/写库失败的即时提示。
    pub(crate) error: Option<String>,
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
            selected: 0,
            edit_body: body.clone(),
            edit_base: body,
            set,
            error: None,
        }
    }

    fn select(&mut self, index: usize) {
        if index >= self.set.pages.len() || self.done.get(index) == Some(&true) {
            return;
        }
        self.selected = index;
        self.error = None;
        let body = self.set.pages[index].body.clone();
        self.edit_body = body.clone();
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
pub(crate) fn wiki_ui(app: &mut GongwenApp, ui: &mut egui::Ui) {
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
        rebuild_cache(app);
        app.wiki.tree_loaded = true;
        app.wiki.tree_dirty = false;
    }
    // 链接解析表：标题 → 跳转目标。每帧从缓存重建（条目量级为数十，代价可忽略）。
    let link_map = build_link_map(&app.wiki.tree, &app.wiki.pages);

    let height = ui.available_height();
    let rail_w = if app.wiki.backlinks_open { 210.0 } else { 0.0 };
    let tree_w = 240.0_f32.min(ui.available_width() * 0.35);
    let mut nav: Option<String> = None;
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(tree_w, height), |ui| {
            ui.vertical(|ui| tree_pane(app, ui, &mut nav));
        });
        ui.separator();
        let mid_w = (ui.available_width() - rail_w).max(240.0);
        ui.allocate_ui(egui::vec2(mid_w, height), |ui| {
            ui.vertical(|ui| reader(app, ui, &link_map, &mut nav));
        });
        if app.wiki.backlinks_open {
            ui.separator();
            ui.allocate_ui(egui::vec2(rail_w, height), |ui| {
                ui.vertical(|ui| backlinks_pane(app, ui, &mut nav));
            });
        }
    });
    // 链接点击产生的跳转统一在这里落库加载，避免边遍历边改缓存。
    if let Some(uuid) = nav {
        open_page(app, &uuid);
    }
    import_window(app, ui.ctx());
    frame_proposal_window(app, ui.ctx());
    review_desk(app, ui.ctx(), &link_map);
}

/// 「从资料归纳」的进度、停止与错误提示，贴在树上方。
fn frame_job_strip(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if let Some(progress) = &app.wiki.frame_progress {
        ui.horizontal(|ui| {
            ui.weak(progress);
            if ui.button("停止").clicked() {
                app.wiki.request_stop_frame = true;
            }
        });
    }
    if app.wiki.frame_error.is_some() {
        let error = app.wiki.frame_error.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.colored_label(theme::warn(), &error);
            if ui.small_button("知道了").clicked() {
                app.wiki.frame_error = None;
            }
        });
    }
}

/// 「整编此分类」的进度、停止与错误提示，与归纳的提示条并列。
fn write_job_strip(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if let Some(progress) = &app.wiki.write_progress {
        ui.horizontal(|ui| {
            ui.weak(progress);
            if ui.button("停止").clicked() {
                app.wiki.request_stop_write = true;
            }
        });
    }
    if app.wiki.write_error.is_some() {
        let error = app.wiki.write_error.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.colored_label(theme::warn(), &error);
            if ui.small_button("知道了").clicked() {
                app.wiki.write_error = None;
            }
        });
    }
}

/// 框架提案确认窗：树形预览归纳结果，确认后写入（同名跳过），丢弃则不落库。
fn frame_proposal_window(app: &mut GongwenApp, ctx: &egui::Context) {
    let Some(proposal) = app.wiki.frame_proposal.clone() else {
        return;
    };
    let mut open = true;
    let mut accepted = false;
    let mut discarded = false;
    egui::Window::new("框架提案 · 待确认")
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.weak(
                "AI 归纳的框架只是提案：确认前可在下方核对，写入时与现有框架同名的节点自动跳过。",
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for theme in &proposal.themes {
                        ui.label(egui::RichText::new(&theme.name).strong());
                        for category in &theme.categories {
                            let basis = if category.doc_ids.is_empty() {
                                String::new()
                            } else {
                                format!("（依据 {} 篇）", category.doc_ids.len())
                            };
                            ui.weak(format!("　├ {}{basis}", category.name));
                        }
                    }
                });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("写入框架").clicked() {
                    accepted = true;
                }
                if ui.button("丢弃").clicked() {
                    discarded = true;
                }
            });
        });
    if accepted {
        let mut created = 0usize;
        let mut skipped = 0usize;
        if let Some(store) = app.wiki_store.as_ref() {
            for theme in &proposal.themes {
                let theme_id = match app
                    .wiki
                    .tree
                    .iter()
                    .find(|t| t.theme.name == theme.name)
                    .map(|t| t.theme.id)
                {
                    Some(id) => {
                        skipped += 1;
                        id
                    }
                    None => match store.create_theme(&theme.name) {
                        Ok(id) => {
                            created += 1;
                            id
                        }
                        Err(error) => {
                            app.wiki.notice =
                                Some(format!("写入主题「{}」失败：{error:#}", theme.name));
                            break;
                        }
                    },
                };
                let existing: Vec<String> = app
                    .wiki
                    .tree
                    .iter()
                    .find(|t| t.theme.id == theme_id)
                    .map(|t| t.categories.iter().map(|c| c.name.clone()).collect())
                    .unwrap_or_default();
                for category in &theme.categories {
                    if existing.contains(&category.name) {
                        skipped += 1;
                        continue;
                    }
                    match store.create_category(theme_id, &category.name) {
                        Ok(_) => created += 1,
                        Err(error) => {
                            app.wiki.notice =
                                Some(format!("写入分类「{}」失败：{error:#}", category.name));
                            break;
                        }
                    }
                }
            }
        }
        if app.wiki.notice.is_none() {
            app.wiki.notice = Some(format!(
                "框架提案已写入：新建 {created} 个节点（同名跳过 {skipped} 个）。"
            ));
        }
        app.wiki.tree_dirty = true;
        app.wiki.frame_proposal = None;
    } else if discarded || !open {
        app.wiki.frame_proposal = None;
    }
}

/// 整编审核台：左列逐页把关，右上渲染预览（口径 13 换接点），右下可直接改
/// Markdown 源；只有点「采纳本页」才经 `store.create_page` 写库（版本带来源与
/// 「审核已修改」标记）。关掉窗口不丢提案，树上方「整编审核」按钮可 reopen。
fn review_desk(app: &mut GongwenApp, ctx: &egui::Context, link_map: &HashMap<String, LinkTarget>) {
    if app.wiki.review.is_none() || !app.wiki.review_open {
        return;
    }
    let mut open = true;
    let mut adopt = false;
    let mut reject = false;
    let mut preview_nav: Option<String> = None;
    egui::Window::new("整编审核")
        .collapsible(false)
        .resizable(true)
        .default_width(800.0)
        .default_height(520.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            let Some(review) = app.wiki.review.as_mut() else {
                return;
            };
            let pending = review.done.iter().filter(|done| !**done).count();
            ui.weak(format!(
                "分类「{}」 · 待审 {pending}/{} 页 · 作业 {}",
                review.set.category,
                review.set.pages.len(),
                review.set.run_id
            ));
            ui.weak("页面只是 AI 提案：逐页核对，正文可直接改；只有点「采纳本页」才会写库。");
            ui.separator();
            ui.horizontal(|ui| {
                // 左列：本批页面清单（✓ = 已处理，点名字切换待审页）。
                let list_w = 200.0_f32.min(ui.available_width() * 0.3);
                ui.allocate_ui(egui::vec2(list_w, ui.available_height()), |ui| {
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for index in 0..review.set.pages.len() {
                                let page = &review.set.pages[index];
                                let done = review.done[index];
                                let label = if done {
                                    format!("✓ {}", page.title)
                                } else if page.kind == "guide" {
                                    format!("{}（导览）", page.title)
                                } else {
                                    page.title.clone()
                                };
                                let selected = !done && review.selected == index;
                                if done {
                                    ui.weak(label);
                                } else if ui.selectable_label(selected, label).clicked() {
                                    review.select(index);
                                }
                            }
                        });
                    });
                });
                ui.separator();
                // 右侧：预览 + 源编辑 + 采纳/退回。
                let detail = review.set.pages.get(review.selected).cloned();
                let selected_done = review.done.get(review.selected) == Some(&true);
                let right_w = ui.available_width();
                let right_h = ui.available_height();
                ui.allocate_ui(egui::vec2(right_w, right_h), |ui| {
                    ui.vertical(|ui| {
                        let Some(page) = detail else {
                            ui.weak("这批提案已处理完。");
                            return;
                        };
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&page.title).strong());
                            if page.kind == "guide" {
                                theme::accent_scope(ui, |ui| {
                                    ui.small("导览页");
                                });
                            }
                        });
                        if !page.doc_refs.is_empty() {
                            ui.weak(format!(
                                "本页依据：{}",
                                page.doc_refs
                                    .iter()
                                    .map(|(id, note)| format!("E{id}{note}"))
                                    .collect::<Vec<_>>()
                                    .join("；")
                            ));
                        }
                        if let Some(error) = &review.error {
                            ui.colored_label(theme::warn(), error);
                        }
                        ui.add_space(4.0);
                        let preview_h = (ui.available_height() * 0.45).max(120.0);
                        ui.weak("预览（渲染的是下方源文本的当前内容）");
                        egui::ScrollArea::vertical()
                            .max_height(preview_h)
                            .show(ui, |ui| {
                                show_page_body(ui, &review.edit_body, link_map, &mut preview_nav);
                            });
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::multiline(&mut review.edit_body)
                                .desired_rows(10)
                                .desired_width(f32::INFINITY)
                                .hint_text("页面 Markdown 源：与普通公文正文一致，唯一扩展是 [[页面链接]]。"),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if selected_done {
                                ui.weak("本页已处理。");
                            } else {
                                if ui.button("采纳本页").clicked() {
                                    adopt = true;
                                }
                                if ui.button("退回本页").clicked() {
                                    reject = true;
                                }
                            }
                        });
                    });
                });
            });
        });
    if !open {
        // 关掉窗口只是收起审核台：提案保留，树上方「整编审核」入口可 reopen。
        app.wiki.review_open = false;
    }
    if let Some(uuid) = preview_nav {
        open_page(app, &uuid);
    }
    if adopt {
        review_adopt(app);
    } else if reject && let Some(review) = app.wiki.review.as_mut() {
        review_mark_done_advance(review);
    }
    // 全部处理完：给结论、清审核台、刷新树（新页面挂进分类）。
    if let Some(review) = &app.wiki.review
        && review.done.iter().all(|done| *done)
    {
        let adopted = review.adopted;
        let total = review.set.pages.len();
        app.wiki.notice = Some(format!(
            "整编已处理完：采纳 {adopted} 页、退回 {} 页。",
            total - adopted
        ));
        app.wiki.review = None;
        app.wiki.review_open = false;
        app.wiki.tree_dirty = true;
    }
}

/// 采纳当前页：把编辑后的正文经 `store.create_page` 写库（建页 + v1）。
fn review_adopt(app: &mut GongwenApp) {
    let Some(review) = app.wiki.review.as_mut() else {
        return;
    };
    if review.done.get(review.selected) == Some(&true) {
        return;
    }
    let Some(page) = review.set.pages.get(review.selected).cloned() else {
        return;
    };
    let body = review.edit_body.clone();
    let input = crate::wiki::PageInput {
        title: page.title.clone(),
        node_id: review.set.node_id,
        kind: if page.kind == "guide" {
            "guide"
        } else {
            "normal"
        },
        body: body.clone(),
        source_run_id: review.set.run_id.clone(),
        review_edited: body != review.edit_base,
        doc_refs: page.doc_refs.clone(),
    };
    let result = app
        .wiki_store
        .as_ref()
        .map(|store| store.create_page(&input).map(|_| ()));
    match result {
        Some(Ok(())) => {
            review.adopted += 1;
            review_mark_done_advance(review);
        }
        Some(Err(error)) => review.error = Some(format!("采纳失败：{error:#}")),
        None => review.error = Some("知识库不可用。".into()),
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

/// 导入框架对话框：粘贴 Markdown 大纲（# 主题 / ## 分类，或 - 列表两级），
/// 实时预览解析结果，确认后写库——与现有节点同名的跳过，不覆盖已有框架。
fn import_window(app: &mut GongwenApp, ctx: &egui::Context) {
    if !app.wiki.import_open {
        return;
    }
    let mut open = true;
    let mut close = false;
    let mut done = false;
    egui::Window::new("导入框架")
        .collapsible(false)
        .resizable(true)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.weak("粘贴 Markdown 大纲：# 主题、## 分类（或 - 主题、缩进 - 分类），只分两级。");
            ui.add(
                egui::TextEdit::multiline(&mut app.wiki.import_text)
                    .desired_rows(10)
                    .desired_width(f32::INFINITY)
                    .hint_text("# 安全生产\n## 监督检查\n## 应急管理\n# 政务服务\n……"),
            );
            ui.add_space(4.0);
            let parsed = crate::wiki::parse_frame_outline(&app.wiki.import_text);
            match &parsed {
                Ok(themes) => {
                    ui.weak(format!("解析出 {} 个主题：", themes.len()));
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            for theme in themes {
                                ui.label(format!(
                                    "{}（{} 个分类）",
                                    theme.name,
                                    theme.categories.len()
                                ));
                                for category in &theme.categories {
                                    ui.weak(format!("　├ {}", category));
                                }
                            }
                        });
                }
                Err(error) => {
                    ui.colored_label(theme::warn(), format!("还不能导入：{error}"));
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(parsed.is_ok(), egui::Button::new("确认导入"))
                    .clicked()
                {
                    done = true;
                }
                if ui.button("取消").clicked() {
                    close = true;
                }
            });
        });
    if close {
        open = false;
    }
    if done && let Ok(themes) = parsed_themes(app) {
        let mut created = 0usize;
        let mut skipped = 0usize;
        if let Some(store) = app.wiki_store.as_ref() {
            for OutlineTheme { name, categories } in themes {
                let existing = app
                    .wiki
                    .tree
                    .iter()
                    .find(|t| t.theme.name == name)
                    .map(|t| t.theme.id);
                let theme_id = match existing {
                    Some(id) => {
                        skipped += 1;
                        id
                    }
                    None => match store.create_theme(&name) {
                        Ok(id) => {
                            created += 1;
                            id
                        }
                        Err(error) => {
                            app.wiki.notice = Some(format!("导入「{name}」失败：{error:#}"));
                            break;
                        }
                    },
                };
                let existing_categories: Vec<String> = app
                    .wiki
                    .tree
                    .iter()
                    .find(|t| t.theme.id == theme_id)
                    .map(|t| t.categories.iter().map(|c| c.name.clone()).collect())
                    .unwrap_or_default();
                for category in categories {
                    if existing_categories.contains(&category) {
                        skipped += 1;
                        continue;
                    }
                    match store.create_category(theme_id, &category) {
                        Ok(_) => created += 1,
                        Err(error) => {
                            app.wiki.notice =
                                Some(format!("导入分类「{category}」失败：{error:#}"));
                            break;
                        }
                    }
                }
            }
        }
        if app.wiki.notice.is_none() {
            app.wiki.notice = Some(format!(
                "已导入 {created} 个节点（同名跳过 {skipped} 个）。"
            ));
        }
        app.wiki.tree_dirty = true;
        app.wiki.import_open = false;
        app.wiki.import_text.clear();
    } else if !open {
        app.wiki.import_open = false;
    }
}

/// 导入解析结果（确认时重解析一次，以对话框里看到的那份为准）。
fn parsed_themes(app: &GongwenApp) -> Result<Vec<OutlineTheme>, String> {
    crate::wiki::parse_frame_outline(&app.wiki.import_text)
}

// ── 缓存与链接解析 ──────────────────────────────────────────────────────────

fn rebuild_cache(app: &mut GongwenApp) {
    let Some(store) = app.wiki_store.as_ref() else {
        return;
    };
    let Ok(tree) = store.list_tree() else {
        return;
    };
    let mut pages = HashMap::new();
    for t in &tree {
        if let Ok(v) = store.list_pages(t.theme.id) {
            pages.insert(t.theme.id, v);
        }
        for c in &t.categories {
            if let Ok(v) = store.list_pages(c.id) {
                pages.insert(c.id, v);
            }
        }
    }
    app.wiki.tree = tree;
    app.wiki.pages = pages;
}

/// 链接跳转目标：已建页、框架节点（导览页或待建导览）。解析表里没有的目标
/// 即红链（待建页），走行内渲染的兜底分支。
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkTarget {
    Page(String),
    Node { name: String, guide: Option<String> },
}

fn build_link_map(
    tree: &[WikiTree],
    pages: &HashMap<i64, Vec<WikiPageMeta>>,
) -> HashMap<String, LinkTarget> {
    let mut map = HashMap::new();
    for t in tree {
        for m in pages.get(&t.theme.id).into_iter().flatten() {
            map.insert(m.title.clone(), LinkTarget::Page(m.uuid.clone()));
        }
        if let Some(g) = &t.theme_guide {
            map.insert(
                t.theme.name.clone(),
                LinkTarget::Node {
                    name: t.theme.name.clone(),
                    guide: Some(g.clone()),
                },
            );
        }
        for c in &t.categories {
            for m in pages.get(&c.id).into_iter().flatten() {
                map.insert(m.title.clone(), LinkTarget::Page(m.uuid.clone()));
            }
            map.insert(
                c.name.clone(),
                LinkTarget::Node {
                    name: c.name.clone(),
                    guide: t.category_guides.get(&c.id).cloned(),
                },
            );
        }
    }
    map
}

fn open_page(app: &mut GongwenApp, uuid: &str) {
    let Some(store) = app.wiki_store.as_ref() else {
        return;
    };
    match store.get_page(uuid) {
        Ok(Some(page)) => {
            app.wiki.backlinks = store.backlinks(uuid).unwrap_or_default();
            app.wiki.open = Some(page);
        }
        Ok(None) => {
            app.wiki.open = None;
            app.wiki.backlinks = Vec::new();
        }
        Err(_) => {}
    }
}

// ── 左栏：框架树 ────────────────────────────────────────────────────────────

fn tree_pane(app: &mut GongwenApp, ui: &mut egui::Ui, nav: &mut Option<String>) {
    ui.horizontal(|ui| {
        ui.heading("知识框架");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::FileUp, "导入框架"))
                .on_hover_text("粘贴 Markdown 大纲（# 主题 / ## 分类）批量建框架")
                .clicked()
            {
                app.wiki.import_open = true;
            }
            let busy = app.wiki.frame_progress.is_some();
            if ui
                .add_enabled(
                    !busy,
                    theme::icon_text_button(theme::Icon::Sparkles, "从资料归纳"),
                )
                .on_hover_text("让 AI 浏览资料库，归纳两级框架提案；确认后才写入")
                .clicked()
            {
                app.start_wiki_frame();
            }
        });
    });
    frame_job_strip(app, ui);
    write_job_strip(app, ui);
    // 审核台入口：作业交回提案后常驻，关掉窗口可从这儿 reopen。
    if let Some(review) = &app.wiki.review {
        let pending = review.done.iter().filter(|done| !**done).count();
        if pending > 0 {
            let label = format!("整编审核（待审 {pending} 页）");
            if ui
                .add(theme::icon_text_button(theme::Icon::BookmarkCheck, &label))
                .clicked()
            {
                app.wiki.review_open = true;
            }
        }
    }
    // 新建主题：输入 + 建（复用标准词库页的「行内输入即建」交互）。
    ui.horizontal(|ui| {
        let edit = egui::TextEdit::singleline(&mut app.wiki.new_node_name)
            .desired_width(140.0)
            .hint_text("新主题名称");
        let resp = ui.add(edit);
        let name = app.wiki.new_node_name.trim().to_string();
        let create = ui
            .add_enabled(!name.is_empty(), egui::Button::new("建主题"))
            .clicked()
            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if create {
            if let Some(store) = app.wiki_store.as_ref() {
                match store.create_theme(&name) {
                    Ok(id) => {
                        app.wiki.notice = Some(format!("已建主题「{name}」。"));
                        app.wiki.tree_dirty = true;
                        app.wiki.selected_node = Some(id);
                        app.wiki.node_name_edit = name.clone();
                    }
                    Err(error) => app.wiki.notice = Some(format!("建主题失败：{error:#}")),
                }
            }
            app.wiki.new_node_name.clear();
        }
    });
    if let Some(notice) = &app.wiki.notice {
        ui.weak(notice);
    }
    ui.add_space(4.0);
    if app.wiki.tree.is_empty() {
        ui.weak("还没有知识框架：先建主题、点右上角「导入框架」粘贴大纲，或「从资料归纳」让 AI 提一份框架。");
        return;
    }
    node_edit_strip(app, ui);
    ui.add_space(2.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        for t in &app.wiki.tree {
            let theme_selected = app.wiki.selected_node == Some(t.theme.id);
            if tree_row(ui, &t.theme.name, theme_selected).clicked() {
                app.wiki.selected_node = Some(t.theme.id);
                app.wiki.node_name_edit = t.theme.name.clone();
            }
            // 主题名下的页面（含导览页）排在分类前面。
            for m in app.wiki.pages.get(&t.theme.id).into_iter().flatten() {
                page_row(app, ui, m, nav);
            }
            for c in &t.categories {
                let category_selected = app.wiki.selected_node == Some(c.id);
                if tree_row(ui, &format!("　{}", c.name), category_selected).clicked() {
                    app.wiki.selected_node = Some(c.id);
                    app.wiki.node_name_edit = c.name.clone();
                }
                for m in app.wiki.pages.get(&c.id).into_iter().flatten() {
                    page_row(app, ui, m, nav);
                }
            }
        }
    });
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
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(if is_theme { "主题" } else { "分类" });
            ui.add(egui::TextEdit::singleline(&mut app.wiki.node_name_edit).desired_width(120.0));
            let new_name = app.wiki.node_name_edit.trim().to_string();
            if ui
                .add_enabled(
                    !new_name.is_empty() && new_name != name,
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
                let edit = egui::TextEdit::singleline(&mut app.wiki.new_node_name)
                    .desired_width(120.0)
                    .hint_text("分类名称");
                let resp = ui.add(edit);
                let category = app.wiki.new_node_name.trim().to_string();
                let create = ui
                    .add_enabled(!category.is_empty(), egui::Button::new("建分类"))
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
                    app.wiki.new_node_name.clear();
                }
            });
        } else {
            // 分类跨主题移动。
            ui.horizontal(|ui| {
                ui.label("移到主题");
                let current = theme_id;
                let mut target = current;
                egui::ComboBox::from_id_salt("wiki_move_theme")
                    .selected_text(
                        app.wiki
                            .tree
                            .iter()
                            .find(|t| t.theme.id == current)
                            .map(|t| t.theme.name.as_str())
                            .unwrap_or(""),
                    )
                    .show_ui(ui, |ui| {
                        for t in &app.wiki.tree {
                            ui.selectable_value(&mut target, t.theme.id, &t.theme.name);
                        }
                    });
                if target != current
                    && ui.button("移过去").clicked()
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
            // AI 整编：把该分类交给 Pi 作业，提案进审核台逐页把关后才写库。
            ui.horizontal(|ui| {
                let theme_name = app
                    .wiki
                    .tree
                    .iter()
                    .find(|t| t.theme.id == theme_id)
                    .map(|t| t.theme.name.clone())
                    .unwrap_or_default();
                let busy = app.wiki.frame_progress.is_some() || app.wiki.write_progress.is_some();
                if ui
                    .add_enabled(
                        !busy && !theme_name.is_empty(),
                        theme::icon_text_button(theme::Icon::Sparkles, "整编此分类"),
                    )
                    .on_hover_text(
                        "AI 按本分类写导览页与条目页；提案须经人工审核（可直接改正文）后才写入",
                    )
                    .clicked()
                {
                    app.start_wiki_write(theme_name, name.clone());
                }
            });
        }
        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("删除节点").color(theme::warn()))
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
    link_map: &HashMap<String, LinkTarget>,
    nav: &mut Option<String>,
) {
    let Some(page) = app.wiki.open.clone() else {
        ui.weak("在左侧框架树里选一个页面开始阅读。");
        return;
    };
    // 页头：标题 + 元信息 + backlinks 栏开关。
    ui.horizontal(|ui| {
        ui.heading(&page.meta.title);
        if page.meta.kind == "guide" {
            theme::accent_scope(ui, |ui| {
                ui.small("导览页");
            });
        }
        ui.weak(format!("v{}", page.meta.current_version));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let icon = if app.wiki.backlinks_open {
                theme::Icon::PanelClose
            } else {
                theme::Icon::PanelOpen
            };
            if ui.add(theme::icon_text_button(icon, "被引用")).clicked() {
                app.wiki.backlinks_open = !app.wiki.backlinks_open;
            }
        });
    });
    if let Ok(refs) = app
        .wiki_store
        .as_ref()
        .map(|s| s.page_doc_refs(&page.meta.uuid))
        .unwrap_or_else(|| Ok(Vec::new()))
        && !refs.is_empty()
    {
        ui.weak(format!(
            "本页依据：{}",
            refs.iter()
                .map(|(id, note)| format!("E{id}{note}"))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    egui::ScrollArea::vertical().show(ui, |ui| {
        show_page_body(ui, &page.body, link_map, nav);
    });
}

/// 正文渲染（口径 13 的换接点）：按段落/标题骨架排版，`[[链接]]` 渲染为
/// 可点内链——已建页跳页面、框架名跳导览页、待建目标显示为红链样式。
/// 接入现有预览渲染链时整体替换本函数，链接挂件逻辑原样保留。
fn show_page_body(
    ui: &mut egui::Ui,
    body: &str,
    link_map: &HashMap<String, LinkTarget>,
    nav: &mut Option<String>,
) {
    for line in body.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            ui.add_space(4.0);
            continue;
        }
        if let Some(heading) = line.strip_prefix('#') {
            let level = heading.chars().take_while(|c| *c == '#').count();
            let text = heading[level..].trim();
            if text.is_empty() {
                continue;
            }
            let size = match level {
                1 => 19.0,
                2 => 17.0,
                _ => 15.5,
            };
            ui.label(egui::RichText::new(text).strong().size(size));
            ui.add_space(2.0);
            continue;
        }
        // 列表行：无序列出换成圆点并缩进，有序行保留原编号。
        let (indent, text) = if let Some(rest) = line.strip_prefix("- ") {
            ("　", rest)
        } else {
            ("", line)
        };
        ui.horizontal_wrapped(|ui| {
            if !indent.is_empty() {
                ui.label("•");
            }
            show_inline(ui, text, link_map, nav);
        });
    }
}

/// 一行内的行内排版：普通文字直接铺，`[[目标|显示]]` 按解析表渲染为内链。
fn show_inline(
    ui: &mut egui::Ui,
    line: &str,
    link_map: &HashMap<String, LinkTarget>,
    nav: &mut Option<String>,
) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut text_start = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < bytes.len() && !(bytes[j] == b']' && bytes[j + 1] == b']') {
                j += 1;
            }
            if j + 1 < bytes.len() {
                if start > text_start {
                    ui.label(&line[text_start..i]);
                }
                let inner = &line[start..j];
                let (target, text) = match inner.split_once('|') {
                    Some((t, x)) => (t.trim(), x.trim()),
                    None => (inner.trim(), inner.trim()),
                };
                let hit = if target.is_empty() || text.is_empty() {
                    None
                } else {
                    link_map.get(target)
                };
                match hit {
                    Some(LinkTarget::Page(uuid)) => {
                        let resp = ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .color(theme::accent_text())
                                    .underline(),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if resp.clicked() {
                            *nav = Some(uuid.clone());
                        }
                        resp.on_hover_text(format!("维基页：{target}"));
                    }
                    Some(LinkTarget::Node { name, guide }) => {
                        let resp = ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .color(theme::accent_text())
                                    .underline(),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if resp.clicked()
                            && let Some(g) = guide
                        {
                            *nav = Some(g.clone());
                        }
                        let hover = match guide {
                            Some(_) => format!("导览页：{name}"),
                            None => format!("框架节点「{name}」还没有导览页"),
                        };
                        resp.on_hover_text(hover);
                    }
                    _ => {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text).color(theme::warn()).underline(),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text(format!("待建页：{target}"));
                    }
                }
                i = j + 2;
                text_start = i;
                continue;
            }
        }
        i += 1;
    }
    if text_start < line.len() {
        ui.label(&line[text_start..]);
    }
}

// ── 右栏：backlinks ──────────────────────────────────────────────────────────

fn backlinks_pane(app: &mut GongwenApp, ui: &mut egui::Ui, nav: &mut Option<String>) {
    ui.horizontal(|ui| {
        ui.heading("被引用");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::PanelClose, ""))
                .clicked()
            {
                app.wiki.backlinks_open = false;
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
        }
    });
}
