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
    /// 版本历史窗口：A/B 两版行级对比、回滚（回滚也产生新版本）。
    history_open: bool,
    /// 对比的 A（旧）/ B（新）版本号。
    history_a: Option<i64>,
    history_b: Option<i64>,
    /// 回滚二次确认开关与失败原因。
    history_confirm_rollback: bool,
    history_error: Option<String>,
    /// 缺口清单：红链聚合（树缓存重建时刷新）与窗口开关。
    gaps: Vec<crate::wiki::RedLinkGap>,
    gap_open: bool,
    /// 「从缺口建页」对话框：标题来自红链，归属手选分类。
    create_open: bool,
    create_title: String,
    create_node: Option<i64>,
    create_body: String,
    create_error: Option<String>,
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
    version_history_window(app, ui.ctx());
    gap_window(app, ui.ctx());
    create_page_window(app, ui.ctx());
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
                            if page.uuid.is_some() {
                                theme::accent_scope(ui, |ui| {
                                    ui.small("更新现有页");
                                });
                            }
                        });
                        if !page.change_note.is_empty() {
                            ui.weak(format!("变更说明：{}", page.change_note));
                        }
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
                        if let Some(uuid) = page.uuid.clone() {
                            // 更新提案：diff（库中当前正文 → 源文本当前内容）。
                            let current = app
                                .wiki_store
                                .as_ref()
                                .and_then(|store| store.get_page(&uuid).ok().flatten());
                            match current {
                                Some(current_page) => {
                                    match line_diff(&current_page.body, &review.edit_body) {
                                        Some(lines) => diff_view(
                                            ui,
                                            &format!(
                                                "v{} → 提案",
                                                current_page.meta.current_version
                                            ),
                                            &lines,
                                        ),
                                        None => {
                                            ui.weak("当前正文行数太多，算不动 diff；按预览显示。");
                                            egui::ScrollArea::vertical()
                                                .max_height(preview_h)
                                                .show(ui, |ui| {
                                                    show_page_body(
                                                        ui,
                                                        &review.edit_body,
                                                        link_map,
                                                        &mut preview_nav,
                                                    );
                                                });
                                        }
                                    }
                                }
                                None => {
                                    ui.colored_label(
                                        theme::warn(),
                                        "读不到本页当前正文（可能已被删除）；请退回本页。",
                                    );
                                }
                            }
                        } else {
                            ui.weak("预览（渲染的是下方源文本的当前内容）");
                            egui::ScrollArea::vertical()
                                .max_height(preview_h)
                                .show(ui, |ui| {
                                    show_page_body(ui, &review.edit_body, link_map, &mut preview_nav);
                                });
                        }
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
    // 本批处理过当前阅读页时重载正文（采纳后是新版、退回也该重读元数据）。
    let touched_open = app.wiki.review.as_ref().is_some_and(|review| {
        review.set.pages.iter().enumerate().any(|(index, page)| {
            review.done.get(index) == Some(&true)
                && page.uuid.is_some()
                && app
                    .wiki
                    .open
                    .as_ref()
                    .is_some_and(|open| page.uuid.as_deref() == Some(open.meta.uuid.as_str()))
        })
    });
    if touched_open && let Some(uuid) = app.wiki.open.as_ref().map(|open| open.meta.uuid.clone()) {
        open_page(app, &uuid);
    }
    // 全部处理完：给结论、清审核台、刷新树（新页面挂进分类）。
    if let Some(review) = &app.wiki.review
        && review.done.iter().all(|done| *done)
    {
        let adopted = review.adopted;
        let total = review.set.pages.len();
        let action = if review.set.pages.iter().any(|page| page.uuid.is_some()) {
            "更新"
        } else {
            "整编"
        };
        app.wiki.notice = Some(format!(
            "{action}已处理完：采纳 {adopted} 页、退回 {} 页。",
            total - adopted
        ));
        app.wiki.review = None;
        app.wiki.review_open = false;
        app.wiki.tree_dirty = true;
    }
}

/// 采纳当前页：新建页走 `create_page`（建页 + v1）；更新提案走
/// `update_page`（以库中现状为准写新版本）。两者都带来源与「审核已修改」标记。
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
            review.adopted += 1;
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
fn version_history_window(app: &mut GongwenApp, ctx: &egui::Context) {
    if !app.wiki.history_open {
        return;
    }
    let Some(page) = app.wiki.open.clone() else {
        app.wiki.history_open = false;
        return;
    };
    let uuid = page.meta.uuid.clone();
    let mut open = true;
    let mut rollback = false;
    egui::Window::new(format!("版本历史 · {}", page.meta.title))
        .collapsible(false)
        .resizable(true)
        .default_width(720.0)
        .default_height(480.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            let versions = app
                .wiki_store
                .as_ref()
                .map(|store| store.list_versions(&uuid).unwrap_or_default())
                .unwrap_or_default();
            if versions.is_empty() {
                ui.weak("还没有版本记录。");
                return;
            }
            // 版本清单：版本号、时间、来源、是否经人工修改。
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for v in &versions {
                        let current = v.version == page.meta.current_version;
                        let edited = if v.review_edited {
                            " · 人工修改"
                        } else {
                            ""
                        };
                        let label = format!(
                            "v{}{} · {}{edited}",
                            v.version,
                            if current { "（当前）" } else { "" },
                            v.created_at
                        );
                        ui.weak(label)
                            .on_hover_text(format!("来源：{}", v.source_run_id));
                    }
                });
            });
            ui.add_space(4.0);
            let mut a = app.wiki.history_a.unwrap_or(page.meta.current_version);
            let mut b = app.wiki.history_b.unwrap_or(page.meta.current_version);
            ui.horizontal(|ui| {
                ui.label("对比");
                egui::ComboBox::from_id_salt("wiki_hist_a")
                    .selected_text(format!("v{a}"))
                    .show_ui(ui, |ui| {
                        for v in &versions {
                            ui.selectable_value(&mut a, v.version, format!("v{}", v.version));
                        }
                    });
                ui.label("→");
                egui::ComboBox::from_id_salt("wiki_hist_b")
                    .selected_text(format!("v{b}"))
                    .show_ui(ui, |ui| {
                        for v in &versions {
                            ui.selectable_value(&mut b, v.version, format!("v{}", v.version));
                        }
                    });
            });
            app.wiki.history_a = Some(a);
            app.wiki.history_b = Some(b);
            ui.add_space(4.0);
            ui.separator();
            let (body_a, body_b) = app.wiki_store.as_ref().map_or((None, None), |store| {
                (
                    store.get_version_body(&uuid, a).ok().flatten(),
                    store.get_version_body(&uuid, b).ok().flatten(),
                )
            });
            match (body_a, body_b) {
                (Some(ref old), Some(ref new)) if a != b => match line_diff(old, new) {
                    Some(lines) => diff_view(ui, &format!("v{a} → v{b}"), &lines),
                    None => {
                        ui.weak("两版行数太多，算不动行级 diff；请改选正文较小的版本。");
                    }
                },
                (Some(_), Some(_)) => {
                    ui.weak("A 与 B 是同一版本，无差异。");
                }
                _ => {
                    ui.colored_label(theme::warn(), "读取版本正文失败。");
                }
            }
            ui.add_space(4.0);
            ui.separator();
            ui.horizontal(|ui| {
                if !app.wiki.history_confirm_rollback {
                    if ui
                        .add_enabled(
                            b != page.meta.current_version,
                            egui::Button::new(format!("回滚到 v{b}")),
                        )
                        .on_hover_text("以 v{b} 的内容生成一个新版本，历史版本全部保留")
                        .clicked()
                    {
                        app.wiki.history_confirm_rollback = true;
                    }
                } else {
                    ui.colored_label(
                        theme::warn(),
                        format!(
                            "确认把本页回滚到 v{b}？当前 v{} 会保留为历史。",
                            page.meta.current_version
                        ),
                    );
                    if ui.button("确认回滚").clicked() {
                        rollback = true;
                    }
                    if ui.button("取消").clicked() {
                        app.wiki.history_confirm_rollback = false;
                    }
                }
            });
            if let Some(error) = &app.wiki.history_error {
                ui.colored_label(theme::warn(), error);
            }
        });
    if !open {
        app.wiki.history_open = false;
    }
    if rollback {
        app.wiki.history_confirm_rollback = false;
        let target = app.wiki.history_b.unwrap_or(0);
        match app
            .wiki_store
            .as_ref()
            .map(|store| store.rollback(&uuid, target))
        {
            Some(Ok(_)) => {
                app.wiki.notice = Some(format!("已回滚到 v{target}（产生新版本，历史保留）。"));
                app.wiki.history_open = false;
                app.wiki.tree_dirty = true;
                open_page(app, &uuid);
            }
            Some(Err(error)) => app.wiki.history_error = Some(format!("回滚失败：{error:#}")),
            None => app.wiki.history_error = Some("知识库不可用。".into()),
        }
    }
}

/// 行级 diff 的一行：相同 / 新增（B 有 A 无）/ 删除（A 有 B 无）。
enum DiffLine {
    Same(String),
    Add(String),
    Del(String),
}

/// 渲染行级 diff 小结 + 清单：删除行红色、新增行主题强调色、相同行弱色。
fn diff_view(ui: &mut egui::Ui, title: &str, lines: &[DiffLine]) {
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
    egui::Window::new("缺口清单")
        .collapsible(false)
        .resizable(true)
        .default_width(460.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .open(&mut open)
        .show(ctx, |ui| {
            let gaps = app.wiki.gaps.clone();
            let empty = empty_categories(app);
            if gaps.is_empty() && empty.is_empty() {
                ui.weak("没有缺口：红链都有落点，分类都有页面。");
                return;
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
                ui.label(egui::RichText::new("还没有页面的分类").strong());
                for (theme, category, id) in &empty {
                    ui.horizontal(|ui| {
                        ui.label(format!("{theme} / {category}"));
                        if ui.button("整编此分类").clicked() {
                            app.start_wiki_write(theme.clone(), category.clone());
                            app.wiki.gap_open = false;
                        }
                        let _ = id;
                    });
                }
            }
        });
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
            ui.weak("正文写法与公文一致，可用 [[页面链接]]；来源记为「人工建页」。");
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

/// 还没有任何页面（含导览页）的分类：theme、category、node_id。
fn empty_categories(app: &GongwenApp) -> Vec<(String, String, i64)> {
    app.wiki
        .tree
        .iter()
        .flat_map(|t| {
            t.categories
                .iter()
                .filter(|c| {
                    app.wiki
                        .pages
                        .get(&c.id)
                        .is_none_or(|pages| pages.is_empty())
                })
                .map(|c| (t.theme.name.clone(), c.name.clone(), c.id))
                .collect::<Vec<_>>()
        })
        .collect()
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
    app.wiki.gaps = store.red_link_gaps().unwrap_or_default();
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

/// 供其他页面（资料库问答的维基依据等）跳转打开指定维基页。
pub(crate) fn open_wiki_page(app: &mut GongwenApp, uuid: &str) {
    open_page(app, uuid);
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
    // 缺口入口：待建页红链 + 无页面覆盖的分类。
    let gap_count = app.wiki.gaps.len() + empty_categories(app).len();
    if gap_count > 0
        && ui
            .add(theme::icon_text_button(
                theme::Icon::List,
                &format!("缺口（{gap_count}）"),
            ))
            .on_hover_text("待建页红链与还没有页面的分类")
            .clicked()
    {
        app.wiki.gap_open = true;
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
            let busy = app.wiki.frame_progress.is_some() || app.wiki.write_progress.is_some();
            if ui
                .add_enabled(!busy, theme::icon_text_button(theme::Icon::Refresh, "更新"))
                .on_hover_text(
                    "让 AI 按资料库新内容更新本页；提案经 diff 审核（可编辑）后才产生新版本",
                )
                .clicked()
            {
                app.start_wiki_update(vec![(page.meta.uuid.clone(), page.meta.title.clone())]);
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::History, "版本"))
                .on_hover_text("版本历史：两版对比、回滚（回滚也产生新版本，不丢历史）")
                .clicked()
            {
                app.wiki.history_open = true;
                app.wiki.history_confirm_rollback = false;
                app.wiki.history_error = None;
                let current = page.meta.current_version;
                app.wiki.history_b = Some(current);
                app.wiki.history_a = (current > 1).then_some(current - 1);
            }
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
