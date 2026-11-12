//! 知识库主工作区。页面导航与人工确认保留在界面线程。

use super::*;
use crate::wiki::{FrameCategory, FrameTheme, WikiVersionMeta};

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(super) enum Route {
    #[default]
    Browse,
    Frame,
    Inbox,
    Review,
    History,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum FrameTab {
    #[default]
    Existing,
    Proposal,
    Import,
}

pub(super) struct ReviewedBatch {
    title: String,
    pages: Vec<(String, bool)>,
    run: Option<WikiRun>,
}

pub(super) struct HistoryCache {
    key: (i64, i64, i64),
    versions: Vec<WikiVersionMeta>,
    old: String,
    new: String,
    refs: Vec<(i64, String)>,
    lines: Option<Vec<DiffLine>>,
}

impl WikiUiState {
    pub(crate) fn open_frame_proposal(&mut self) {
        self.route = Route::Frame;
        self.frame_tab = FrameTab::Proposal;
    }

    pub(crate) fn open_inbox(&mut self) {
        self.route = Route::Inbox;
        self.show_processed = self.review.is_none() && !self.reviewed.is_empty();
        self.rail_hidden = false;
    }
}

pub(super) fn show(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let route = app.wiki.route;
    toolbar(app, ui);
    if route != app.wiki.route {
        ui.ctx().request_repaint();
        return;
    }
    if let Some(notice) = &app.wiki.notice {
        ui.weak(notice);
    }
    ui.separator();
    match route {
        Route::Browse => browse(app, ui),
        Route::Frame => frame(app, ui),
        Route::Inbox => inbox(app, ui),
        Route::Review => review(app, ui),
        Route::History => history(app, ui),
    }
}

fn toolbar(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        match app.wiki.route {
            Route::Browse => {
                ui.heading("知识库");
                let search = ui.add(
                    egui::TextEdit::singleline(&mut app.wiki.search)
                        .hint_text("搜索条目")
                        .desired_width(210.0),
                );
                if search.changed() {
                    let query = app.wiki.search.trim();
                    app.wiki.search_hits = if query.is_empty() {
                        Vec::new()
                    } else {
                        app.wiki_store
                            .as_ref()
                            .and_then(|store| store.search(query, 30).ok())
                            .unwrap_or_default()
                    };
                }
                if ui.button("框架管理").clicked() {
                    app.wiki.route = Route::Frame;
                }
                if ui.button("缺口清单").clicked() {
                    app.wiki.gap_open = true;
                }
                let pending = app.wiki.review.as_ref().map_or(0, pending_count);
                if ui.button(format!("待审收件箱 {pending}")).clicked() {
                    app.wiki.open_inbox();
                }
            }
            route => {
                let (back, label, destination) = match route {
                    Route::Frame => ("知识库", "框架管理", Route::Browse),
                    Route::Inbox => ("知识库", "待审收件箱", Route::Browse),
                    Route::Review => ("待审收件箱", "人工审核", Route::Inbox),
                    Route::History => ("知识库", "版本历史", Route::Browse),
                    Route::Browse => unreachable!(),
                };
                if ui.button(format!("‹ {back}")).clicked() {
                    app.wiki.route = destination;
                }
                ui.heading(label);
            }
        }
        if app.wiki.has_run() {
            let label = if app.wiki.busy() {
                "AI 工作状态 · 运行中"
            } else {
                "AI 工作状态"
            };
            if ui
                .selectable_label(app.wiki.rail_job_visible(), label)
                .clicked()
            {
                app.wiki.toggle_job();
            }
        }
        if app.wiki.route == Route::Browse
            && app.wiki.rail_hidden
            && ui.small_button("引用与依据").clicked()
        {
            app.wiki.rail_hidden = false;
            app.wiki.job_tab = false;
        }
    });
}

pub(super) fn right_panel(
    ui: &mut egui::Ui,
    id: &'static str,
    content: impl FnOnce(&mut egui::Ui),
) {
    let max = (ui.available_width() * 0.34).clamp(150.0, 380.0);
    egui::Panel::right(id)
        .default_size(max.min(300.0))
        .size_range(150.0..=max)
        .frame(theme::panel(theme::surface_sunk(), 12))
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let mut pane = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            pane.set_clip_rect(rect.intersect(ui.clip_rect()));
            pane.set_max_width(rect.width());
            pane.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            content(&mut pane);
            ui.allocate_rect(rect, egui::Sense::hover());
        });
}

fn left_panel(ui: &mut egui::Ui, id: &'static str, content: impl FnOnce(&mut egui::Ui)) {
    let width = (ui.available_width() * 0.24).clamp(108.0, 236.0);
    egui::Panel::left(id)
        .exact_size(width)
        .frame(theme::panel(theme::surface_sunk(), 10))
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            content(ui);
        });
}

fn job_rail(app: &mut GongwenApp, ui: &mut egui::Ui, id: &'static str, frame: bool) {
    if app.wiki.rail_job_visible() {
        right_panel(ui, id, |ui| {
            ui.heading("AI 工作状态");
            ui.separator();
            job::job_contents(app, ui, frame);
        });
    }
}

fn browse(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let mut nav = None;
    if !app.wiki.rail_hidden {
        right_panel(ui, "wiki_browse_references", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (sources, title) in [(false, "被引用"), (true, "依据")] {
                    if ui
                        .selectable_label(
                            !app.wiki.job_tab && app.wiki.sources_tab == sources,
                            title,
                        )
                        .clicked()
                    {
                        app.wiki.job_tab = false;
                        app.wiki.sources_tab = sources;
                    }
                }
                if app.wiki.has_run() {
                    ui.selectable_value(&mut app.wiki.job_tab, true, "AI 工作状态");
                }
            });
            ui.separator();
            if app.wiki.job_tab {
                job::job_contents(app, ui, false);
            } else if app.wiki.sources_tab {
                let refs = app.wiki.doc_refs.clone();
                evidence(app, ui, &refs);
            } else {
                backlinks_pane(app, ui, &mut nav);
            }
        });
    }
    left_panel(ui, "wiki_browse_tree", |ui| {
        if app.wiki.search.trim().is_empty() {
            tree_pane(app, ui, &mut nav, false);
        } else {
            ui.strong("搜索结果");
            ui.separator();
            if app.wiki.search_hits.is_empty() {
                ui.weak("没有匹配的条目");
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for hit in &app.wiki.search_hits {
                    if ui.link(&hit.title).clicked() {
                        nav = Some(hit.uuid.clone());
                    }
                }
            });
        }
    });
    reader(app, ui, &mut nav);
    if let Some(uuid) = nav {
        open_page(app, &uuid);
    }
}

fn frame(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut app.wiki.frame_tab, FrameTab::Existing, "现有框架");
        ui.selectable_value(&mut app.wiki.frame_tab, FrameTab::Proposal, "从资料归纳");
        ui.selectable_value(&mut app.wiki.frame_tab, FrameTab::Import, "导入大纲");
    });
    ui.separator();
    job_rail(app, ui, "wiki_frame_job", true);
    if let Some(error) = &app.wiki.frame_error {
        ui.colored_label(theme::warn(), error);
    }
    match app.wiki.frame_tab {
        FrameTab::Existing => {
            left_panel(ui, "wiki_manage_tree", |ui| {
                tree_pane(app, ui, &mut None, true)
            });
            egui::ScrollArea::vertical()
                .id_salt("wiki_manage_detail")
                .show(ui, |ui| {
                    ui.strong("节点详情");
                    let busy = app.wiki.busy();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut app.wiki.new_node_name)
                                .hint_text("新主题名称")
                                .desired_width(150.0),
                        );
                        let name = app.wiki.new_node_name.trim();
                        if ui
                            .add_enabled(!busy && !name.is_empty(), egui::Button::new("新增主题"))
                            .clicked()
                            && let Some(store) = &app.wiki_store
                        {
                            match store.create_theme(name) {
                                Ok(id) => {
                                    app.wiki.selected_node = Some(id);
                                    app.wiki.node_name_edit = name.to_owned();
                                    app.wiki.new_node_name.clear();
                                    app.wiki.invalidate();
                                }
                                Err(error) => {
                                    app.wiki.notice = Some(format!("建主题失败：{error:#}"))
                                }
                            }
                        }
                    });
                    node_edit_strip(app, ui);
                    if let Some(id) = app.wiki.selected_node
                        && let Some(store) = &app.wiki_store
                        && let Ok(docs) = store.frame_doc_ids(id)
                        && !docs.is_empty()
                    {
                        let refs = docs
                            .into_iter()
                            .map(|id| (id, String::new()))
                            .collect::<Vec<_>>();
                        evidence(app, ui, &refs);
                    }
                });
        }
        FrameTab::Proposal => frame_proposal(app, ui),
        FrameTab::Import => frame_import(app, ui),
    }
}

fn frame_proposal(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if app.wiki.frame_proposal.is_none() {
        ui.heading("从资料归纳框架");
        if ui
            .add_enabled(!app.wiki.busy(), egui::Button::new("开始归纳"))
            .clicked()
        {
            app.start_wiki_frame();
        }
        return;
    }
    let mut confirm = false;
    let mut discard = false;
    let mut open_doc = None;
    ui.horizontal_wrapped(|ui| {
        ui.strong("框架提案");
        confirm = theme::primary_icon_button_enabled(
            ui,
            !app.wiki.busy(),
            theme::Icon::Check,
            "确认框架",
        )
        .clicked();
        discard = ui.button("丢弃提案").clicked();
    });
    ui.separator();
    let proposal = app.wiki.frame_proposal.as_mut().expect("存在框架提案");
    let selection = &mut app.wiki.frame_node;
    left_panel(ui, "wiki_proposed_tree", |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (index, theme) in proposal.themes.iter().enumerate() {
                let node = Some((index, None));
                if outline_row(ui, &theme.name, 0, None, *selection == node).clicked() {
                    *selection = node;
                }
                for (child, category) in theme.categories.iter().enumerate() {
                    let node = Some((index, Some(child)));
                    if outline_row(ui, &category.name, 1, None, *selection == node).clicked() {
                        *selection = node;
                    }
                }
            }
        });
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        if ui.button("新增主题").clicked() {
            *selection = Some((proposal.themes.len(), None));
            proposal.themes.push(FrameTheme {
                name: String::new(),
                categories: Vec::new(),
            });
        }
        if selection.is_none() && !proposal.themes.is_empty() {
            *selection = Some((0, None));
        }
        if let Some((theme, child)) = *selection {
            let siblings = child.map_or(proposal.themes.len(), |_| {
                proposal.themes[theme].categories.len()
            });
            let row = child.unwrap_or(theme);
            let mut target = row;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        row > 0,
                        theme::icon_text_button(theme::Icon::ArrowUp, "上移"),
                    )
                    .clicked()
                {
                    target = row - 1;
                }
                if ui
                    .add_enabled(
                        row + 1 < siblings,
                        theme::icon_text_button(theme::Icon::ArrowDown, "下移"),
                    )
                    .clicked()
                {
                    target = row + 1;
                }
            });
            if target != row {
                if child.is_some() {
                    proposal.themes[theme].categories.swap(row, target);
                    *selection = Some((theme, Some(target)));
                } else {
                    proposal.themes.swap(row, target);
                    *selection = Some((target, None));
                }
            }
        }
        let Some((index, child)) = *selection else {
            return;
        };
        let parents = proposal
            .themes
            .iter()
            .map(|theme| theme.name.clone())
            .collect::<Vec<_>>();
        let Some(theme) = proposal.themes.get_mut(index) else {
            return;
        };
        ui.separator();
        if let Some(child) = child {
            let Some(category) = theme.categories.get_mut(child) else {
                return;
            };
            ui.label("分类名称");
            ui.text_edit_singleline(&mut category.name);
            let mut parent = index;
            egui::ComboBox::from_id_salt("wiki_proposal_parent")
                .selected_text(&theme.name)
                .show_ui(ui, |ui| {
                    for (index, name) in parents.iter().enumerate() {
                        ui.selectable_value(&mut parent, index, name);
                    }
                });
            ui.strong("归纳依据");
            for id in &category.doc_ids {
                let label = doc_title(&app.knowledge_docs, *id);
                if ui.link(label).clicked() {
                    open_doc = Some(*id);
                }
            }
            if ui.button("移除分类").clicked() {
                theme.categories.remove(child);
                *selection = Some((index, None));
            } else if parent != index {
                let category = theme.categories.remove(child);
                let categories = &mut proposal.themes[parent].categories;
                *selection = Some((parent, Some(categories.len())));
                categories.push(category);
            }
        } else {
            ui.label("主题名称");
            ui.text_edit_singleline(&mut theme.name);
            if ui.button("新增分类").clicked() {
                *selection = Some((index, Some(theme.categories.len())));
                theme.categories.push(FrameCategory {
                    name: String::new(),
                    doc_ids: Vec::new(),
                });
            }
            if ui.button("移除主题").clicked() {
                proposal.themes.remove(index);
                *selection = None;
            }
        }
    });
    if let Some(id) = open_doc {
        app.knowledge_open_preview(id);
    }
    if discard {
        app.wiki.frame_proposal = None;
        app.wiki.frame_node = None;
        app.wiki.frame_error = None;
    } else if confirm {
        let themes = &app
            .wiki
            .frame_proposal
            .as_ref()
            .expect("存在框架提案")
            .themes;
        if themes.is_empty() {
            app.wiki.frame_error = Some("至少保留一个主题。".into());
        } else if let Some(store) = &app.wiki_store {
            match store.apply_frame(themes) {
                Ok((created, reused)) => {
                    app.wiki.notice = Some(format!(
                        "框架已确认 · 新建 {created} 个节点 · 沿用 {reused} 个节点"
                    ));
                    app.wiki.frame_proposal = None;
                    app.wiki.frame_node = None;
                    app.wiki.frame_error = None;
                    app.wiki.frame_tab = FrameTab::Existing;
                    app.wiki.invalidate();
                }
                Err(error) => app.wiki.frame_error = Some(format!("框架未写入：{error:#}")),
            }
        }
    }
}

fn frame_import(app: &mut GongwenApp, ui: &mut egui::Ui) {
    ui.strong("Markdown 大纲");
    ui.weak("用一级标题表示主题、二级标题表示分类。也可粘贴两级列表。");
    let parsed = crate::wiki::parse_frame_outline(&app.wiki.import_text);
    if ui
        .add_enabled(
            parsed.is_ok() && !app.wiki.busy(),
            egui::Button::new("确认导入"),
        )
        .clicked()
        && let Ok(themes) = crate::wiki::parse_frame_outline(&app.wiki.import_text)
        && let Some(store) = &app.wiki_store
    {
        let themes = themes
            .into_iter()
            .map(|theme| FrameTheme {
                name: theme.name,
                categories: theme
                    .categories
                    .into_iter()
                    .map(|name| FrameCategory {
                        name,
                        doc_ids: Vec::new(),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        match store.apply_frame(&themes) {
            Ok((created, reused)) => {
                app.wiki.notice = Some(format!("已导入 {created} 个节点 · 沿用 {reused} 个节点"));
                app.wiki.import_text.clear();
                app.wiki.frame_tab = FrameTab::Existing;
                app.wiki.invalidate();
            }
            Err(error) => app.wiki.notice = Some(format!("框架未导入：{error:#}")),
        }
    }
    ui.columns(2, |cols| {
        egui::ScrollArea::vertical()
            .id_salt("wiki_outline_source")
            .show(&mut cols[0], |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut app.wiki.import_text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(12)
                        .hint_text("# 安全生产\n## 监督检查\n## 应急管理"),
                );
            });
        egui::ScrollArea::vertical()
            .id_salt("wiki_outline_tree")
            .show(&mut cols[1], |ui| {
                ui.strong("预览");
                match &parsed {
                    Ok(themes) => {
                        for theme in themes {
                            ui.strong(&theme.name);
                            for category in &theme.categories {
                                ui.label(format!("　{category}"));
                            }
                        }
                    }
                    Err(error) if !app.wiki.import_text.trim().is_empty() => {
                        ui.weak(error);
                    }
                    _ => {}
                }
            });
    });
}

fn pending_count(review: &ReviewState) -> usize {
    review.done.iter().filter(|done| !**done).count()
}

fn inbox(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let pending = app.wiki.review.as_ref().map_or(0, pending_count);
    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut app.wiki.show_processed,
            false,
            format!("待审核 {pending}"),
        );
        ui.selectable_value(&mut app.wiki.show_processed, true, "已处理");
    });
    ui.separator();
    if app.wiki.show_processed {
        left_panel(ui, "wiki_processed_batches", |ui| {
            ui.strong("已处理批次");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (index, batch) in app.wiki.reviewed.iter().enumerate().rev() {
                    ui.selectable_value(&mut app.wiki.processed_batch, index, &batch.title);
                }
            });
        });
        if let Some(batch) = app.wiki.reviewed.get(app.wiki.processed_batch) {
            if !app.wiki.rail_hidden
                && let Some(run) = &batch.run
            {
                right_panel(ui, "wiki_processed_run", |ui| {
                    ui.heading("AI 工作状态");
                    job::show_run(ui, run, false, None);
                });
            }
            ui.heading(&batch.title);
            for (title, accepted) in &batch.pages {
                ui.label(format!(
                    "{title}  ·  {}",
                    if *accepted { "已采纳" } else { "已退回" }
                ));
            }
        } else {
            ui.weak("本次会话暂无已处理批次");
        }
        return;
    }
    if !app.wiki.rail_hidden && app.wiki.page_run.is_some() {
        right_panel(ui, "wiki_inbox_job", |ui| {
            ui.heading("AI 工作状态");
            job::page_job_contents(app, ui);
            if pending > 0 {
                ui.weak("等待人工确认");
            }
        });
    }
    let Some(review) = &mut app.wiki.review else {
        ui.weak("没有待审核的维基页面");
        return;
    };
    left_panel(ui, "wiki_inbox_batches", |ui| {
        ui.strong("待审批次");
        ui.separator();
        ui.label(&review.set.category);
        ui.weak(format!("{pending} 页待审"));
    });
    egui::ScrollArea::vertical()
        .id_salt("wiki_inbox_items")
        .show(ui, |ui| {
            egui::Grid::new("wiki_pending_items")
                .striped(true)
                .spacing([22.0, 8.0])
                .show(ui, |ui| {
                    for label in ["页面", "类型", "依据", "状态"] {
                        ui.strong(label);
                    }
                    ui.end_row();
                    for index in 0..review.set.pages.len() {
                        let page = &review.set.pages[index];
                        let status = if !review.done[index] {
                            "待审核"
                        } else if review.accepted[index] {
                            "已采纳"
                        } else {
                            "已退回"
                        };
                        let selected = ui.add_enabled(
                            !review.done[index],
                            egui::Button::selectable(review.selected == index, &page.title),
                        );
                        ui.label(if page.uuid.is_some() {
                            "更新"
                        } else {
                            "新建"
                        });
                        ui.label(format!("{} 篇", page.doc_refs.len()));
                        ui.label(status);
                        ui.end_row();
                        if selected.clicked() {
                            review.select(index);
                        }
                    }
                });
            ui.separator();
            let page = &review.set.pages[review.selected];
            ui.heading(&page.title);
            if let Some(version) = page.base_version {
                ui.weak(format!("基线 v{version}"));
            }
            if !page.change_note.is_empty() {
                ui.label(&page.change_note);
            }
            if theme::primary_icon_button(ui, theme::Icon::PencilLine, "打开审核台").clicked()
            {
                app.wiki.route = Route::Review;
                app.wiki.job_tab = false;
                app.wiki.rail_hidden = false;
                app.wiki.review_diff = false;
            }
        });
}

fn review(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let Some(state) = &mut app.wiki.review else {
        app.wiki.open_inbox();
        return;
    };
    let selected = state.selected;
    let previous = (0..selected).rev().find(|&index| !state.done[index]);
    let next = (selected + 1..state.done.len()).find(|&index| !state.done[index]);
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut app.wiki.review_diff, false, "版式对照");
        ui.selectable_value(&mut app.wiki.review_diff, true, "变更对比");
        if ui
            .add_enabled(previous.is_some(), egui::Button::new("‹ 上一篇"))
            .clicked()
            && let Some(index) = previous
        {
            state.select(index);
        }
        ui.weak(format!("{} / {}", state.selected + 1, state.done.len()));
        if ui
            .add_enabled(next.is_some(), egui::Button::new("下一篇 ›"))
            .clicked()
            && let Some(index) = next
        {
            state.select(index);
        }
    });
    let page = state.set.pages[state.selected].clone();
    ui.heading(&page.title);
    let current = page
        .uuid
        .as_ref()
        .and_then(|uuid| app.wiki_store.as_ref()?.get_page(uuid).ok().flatten());
    let stale = page.uuid.is_some()
        && current.as_ref().map(|page| page.meta.current_version) != page.base_version;
    if let Some(version) = page.base_version {
        ui.weak(format!("更新 · 基线 v{version}"));
    }
    if stale {
        ui.colored_label(theme::warn(), "当前版本已变化，请退回并重新发起。");
    }
    if let Some(error) = &state.error {
        ui.colored_label(theme::warn(), error);
    }
    ui.separator();

    let mut accept = false;
    let mut reject = false;
    let mut reject_all = false;
    egui::Panel::bottom("wiki_review_actions")
        .resizable(false)
        .frame(theme::panel(theme::surface_sunk(), 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("返回收件箱").clicked() {
                    app.wiki.route = Route::Inbox;
                }
                ui.weak(if state.edit_body == state.edit_base {
                    "待确认"
                } else {
                    "已编辑 · 待确认"
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    accept = theme::primary_icon_button_enabled(
                        ui,
                        !stale,
                        theme::Icon::Check,
                        "采纳并入库",
                    )
                    .clicked();
                    reject = ui.button("退回").clicked();
                    ui.menu_button("更多", |ui| {
                        if ui.button("全部退回").clicked() {
                            reject_all = true;
                            ui.close();
                        }
                    });
                });
            });
        });
    if !app.wiki.rail_hidden {
        right_panel(ui, "wiki_review_evidence", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut app.wiki.job_tab, false, "依据");
                ui.selectable_value(&mut app.wiki.job_tab, true, "AI 工作状态");
            });
            ui.separator();
            if app.wiki.job_tab {
                job::page_job_contents(app, ui);
            } else {
                evidence(app, ui, &page.doc_refs);
            }
        });
    }
    let link_map = build_link_map(&app.wiki.tree, &app.wiki.pages);
    let Some(state) = &mut app.wiki.review else {
        return;
    };
    let id = (state.set.run_id.clone(), state.selected);
    let mut target = None;
    ui.columns(2, |columns| {
        columns[0].strong("Markdown 源码");
        columns[0].separator();
        body::source_pane(
            &mut columns[0],
            ("wiki_source_editor", &id),
            &mut state.edit_body,
        );
        columns[1].strong(if app.wiki.review_diff {
            "变更对比"
        } else {
            "版式预览"
        });
        columns[1].separator();
        let height = columns[1].available_height();
        if app.wiki.review_diff
            && let Some(current) = &current
        {
            let stale = state.diff.as_ref().is_none_or(|cached| {
                cached.uuid != current.meta.uuid
                    || cached.version != current.meta.current_version
                    || cached.current != current.body
                    || cached.edit != state.edit_body
            });
            if stale {
                state.diff = Some(ReviewDiffCache {
                    uuid: current.meta.uuid.clone(),
                    version: current.meta.current_version,
                    current: current.body.clone(),
                    edit: state.edit_body.clone(),
                    lines: line_diff(&current.body, &state.edit_body),
                });
            }
            if let Some(lines) = state.diff.as_ref().and_then(|diff| diff.lines.as_deref()) {
                diff_view(
                    &mut columns[1],
                    &format!("v{} → 提案", current.meta.current_version),
                    lines,
                    height,
                );
                return;
            }
            columns[1].weak("变更较大，显示全文预览。");
        }
        target = body::scroll_pane(&mut columns[1], ("wiki_review_preview", &id), |ui| {
            show_page_body(ui, &state.edit_body, &link_map, &page.doc_refs)
        });
    });
    if let Some(target) = target {
        follow_body_target(app, target);
    }
    if accept {
        review_adopt(app);
    } else if reject_all && let Some(review) = &mut app.wiki.review {
        review.done.fill(true);
    } else if reject
        && let Some(review) = &mut app.wiki.review
        && review.action_allowed()
    {
        review_mark_done_advance(review);
    }
    if app
        .wiki
        .review
        .as_ref()
        .is_some_and(|review| pending_count(review) == 0)
        && let Some(review) = app.wiki.review.take()
    {
        let total = review.set.pages.len();
        app.wiki.notice = Some(format!(
            "本批已处理 · 采纳 {} 页 · 退回 {} 页",
            review.adopted,
            total - review.adopted
        ));
        app.wiki.reviewed.push(ReviewedBatch {
            title: review.set.category,
            pages: review
                .set
                .pages
                .into_iter()
                .zip(review.accepted)
                .map(|(page, accepted)| (page.title, accepted))
                .collect(),
            run: app.wiki.page_run.clone(),
        });
        app.wiki.processed_batch = app.wiki.reviewed.len() - 1;
        app.wiki.open_inbox();
        app.wiki.invalidate();
    }
}

fn history(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let Some(page) = app.wiki.open.as_ref().map(|page| page.meta.clone()) else {
        app.wiki.route = Route::Browse;
        return;
    };
    let Some(store) = &app.wiki_store else { return };
    if app.wiki.history_uuid.as_deref() != Some(&page.uuid) {
        app.wiki.history_uuid = Some(page.uuid.clone());
        app.wiki.history_a = Some((page.current_version - 1).max(1));
        app.wiki.history_b = Some(page.current_version);
        app.wiki.history_cache = None;
    }
    let a = app.wiki.history_a.unwrap_or(page.current_version);
    let b = app.wiki.history_b.unwrap_or(page.current_version);
    let key = (page.current_version, a, b);
    if app
        .wiki
        .history_cache
        .as_ref()
        .is_none_or(|cache| cache.key != key)
    {
        let load = || -> anyhow::Result<_> {
            let versions = store.list_versions(&page.uuid)?;
            let old = store.get_version_body(&page.uuid, a)?.unwrap_or_default();
            let new = store.get_version_body(&page.uuid, b)?.unwrap_or_default();
            let refs = store.version_doc_refs(&page.uuid, b)?;
            let lines = line_diff(&old, &new);
            Ok(HistoryCache {
                key,
                versions,
                old,
                new,
                refs,
                lines,
            })
        };
        match load() {
            Ok(cache) => app.wiki.history_cache = Some(cache),
            Err(error) => {
                ui.colored_label(theme::warn(), format!("读取版本失败：{error:#}"));
                return;
            }
        }
    }
    let mut chosen_a = a;
    let mut chosen_b = b;
    let mut open_doc = None;
    if app.wiki.rail_job_visible() {
        job_rail(app, ui, "wiki_history_job", false);
    } else {
        right_panel(ui, "wiki_history_details", |ui| {
            let cache = app.wiki.history_cache.as_ref().expect("版本已载入");
            ui.heading("版本详情");
            if let Some(meta) = cache.versions.iter().find(|version| version.version == b) {
                ui.strong(format!("v{b}"));
                ui.weak(crate::app::widgets::short_date(&meta.created_at));
                if meta.review_edited {
                    ui.label("经人工编辑");
                }
                if meta.source_run_id == "manual" {
                    ui.weak("人工建立");
                } else {
                    ui.weak("来源作业").on_hover_text(&meta.source_run_id);
                }
            }
            show_doc_refs(ui, &cache.refs, &mut open_doc);
        });
    }
    let Some(cache) = &app.wiki.history_cache else {
        return;
    };
    left_panel(ui, "wiki_history_versions", |ui| {
        ui.strong("版本历史");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for meta in &cache.versions {
                let suffix = if meta.version == page.current_version {
                    " · 当前版本"
                } else {
                    ""
                };
                ui.selectable_value(
                    &mut chosen_b,
                    meta.version,
                    format!("v{}{suffix}", meta.version),
                );
                ui.weak(crate::app::widgets::short_date(&meta.created_at));
            }
        });
    });
    ui.heading(&page.title);
    ui.horizontal(|ui| {
        version_picker(ui, "wiki_history_a", &mut chosen_a, &cache.versions);
        ui.label("→");
        version_picker(ui, "wiki_history_b", &mut chosen_b, &cache.versions);
        ui.selectable_value(&mut app.wiki.history_full, false, "变更");
        ui.selectable_value(&mut app.wiki.history_full, true, "全文");
    });
    if (chosen_a, chosen_b) != (a, b) {
        app.wiki.history_a = Some(chosen_a);
        app.wiki.history_b = Some(chosen_b);
        app.wiki.history_confirm_rollback = false;
        ui.ctx().request_repaint();
    }
    let mut rollback = false;
    egui::Panel::bottom("wiki_history_actions")
        .resizable(false)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if app.wiki.history_confirm_rollback {
                    rollback = ui.button(format!("确认以 v{b} 生成新版本")).clicked();
                    if ui.button("取消").clicked() {
                        app.wiki.history_confirm_rollback = false;
                    }
                } else if ui
                    .add_enabled(
                        b != page.current_version && chosen_b == b,
                        egui::Button::new(format!("回滚到 v{b}")),
                    )
                    .clicked()
                {
                    app.wiki.history_confirm_rollback = true;
                }
                if let Some(error) = &app.wiki.history_error {
                    ui.colored_label(theme::warn(), error);
                }
            });
        });
    history_compare(ui, a, b, app.wiki.history_full, cache);
    if let Some(id) = open_doc {
        app.knowledge_open_preview(id);
    }
    if rollback {
        app.wiki.history_confirm_rollback = false;
        if let Some(store) = &app.wiki_store {
            match store.rollback(&page.uuid, b) {
                Ok(version) => {
                    app.wiki.notice = Some(format!("已从 v{b} 生成 v{version}"));
                    app.knowledge_dirty = true;
                    app.wiki.invalidate();
                    app.wiki.route = Route::Browse;
                    open_page(app, &page.uuid);
                }
                Err(error) => app.wiki.history_error = Some(format!("回滚失败：{error:#}")),
            }
        }
    }
}

fn version_picker(
    ui: &mut egui::Ui,
    id: &'static str,
    selected: &mut i64,
    versions: &[WikiVersionMeta],
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("v{selected}"))
        .show_ui(ui, |ui| {
            for meta in versions {
                ui.selectable_value(selected, meta.version, format!("v{}", meta.version));
            }
        });
}

fn history_compare(ui: &mut egui::Ui, old: i64, new: i64, full: bool, cache: &HistoryCache) {
    ui.columns(2, |cols| {
        cols[0].strong(format!("v{old}"));
        cols[1].strong(format!("v{new}"));
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt(("wiki_history_compare", old, new))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let Some(lines) = cache.lines.as_deref().filter(|_| !full) else {
                ui.columns(2, |cols| {
                    cols[0].label(&cache.old);
                    cols[1].label(&cache.new);
                });
                return;
            };
            let mut lines = lines.iter().peekable();
            while let Some(line) = lines.next() {
                if let DiffLine::Same(text) = line {
                    ui.columns(2, |cols| {
                        cols[0].weak(text);
                        cols[1].weak(text);
                    });
                    continue;
                }
                let mut deleted = Vec::new();
                let mut added = Vec::new();
                for changed in std::iter::once(line).chain(std::iter::from_fn(|| {
                    if matches!(lines.peek(), Some(DiffLine::Add(_) | DiffLine::Del(_))) {
                        lines.next()
                    } else {
                        None
                    }
                })) {
                    match changed {
                        DiffLine::Del(text) => deleted.push(text.as_str()),
                        DiffLine::Add(text) => added.push(text.as_str()),
                        DiffLine::Same(_) => unreachable!(),
                    }
                }
                ui.columns(2, |cols| {
                    if !deleted.is_empty() {
                        cols[0].label(
                            egui::RichText::new(deleted.join("\n"))
                                .background_color(theme::danger_soft()),
                        );
                    }
                    if !added.is_empty() {
                        cols[1].label(
                            egui::RichText::new(added.join("\n"))
                                .background_color(theme::success_soft()),
                        );
                    }
                });
            }
        });
}

fn doc_title(docs: &[crate::knowledge::KnowledgeDocRow], id: i64) -> String {
    docs.iter()
        .find(|doc| doc.id == id)
        .map_or_else(|| format!("资料 E{id}"), |doc| doc.title.clone())
}

pub(super) fn read_evidence(app: &mut GongwenApp, id: i64) {
    let result = app
        .knowledge_store
        .as_mut()
        .map(|store| store.get_doc_content(id));
    app.wiki.evidence_original = None;
    app.wiki.evidence_error = None;
    match result {
        Some(Ok(Some((title, markdown)))) => {
            let mut chars = markdown.chars();
            let mut excerpt: String = chars.by_ref().take(4000).collect();
            if chars.next().is_some() {
                excerpt.push('…');
            }
            app.wiki.evidence_original = Some((id, title, excerpt));
        }
        Some(Ok(None)) => app.wiki.evidence_error = Some(format!("资料 E{id} 已删除或替换。")),
        Some(Err(error)) => app.wiki.evidence_error = Some(format!("读取原文失败：{error:#}")),
        None => app.wiki.evidence_error = Some("资料库不可用。".into()),
    }
}

fn evidence(app: &mut GongwenApp, ui: &mut egui::Ui, refs: &[(i64, String)]) {
    if refs.is_empty() {
        ui.weak("暂无依据");
        return;
    }
    let mut open = None;
    let mut paper = None;
    let mut close = false;
    egui::ScrollArea::vertical()
        .id_salt("wiki_evidence")
        .max_height(if app.wiki.evidence_original.is_some() {
            160.0
        } else {
            ui.available_height()
        })
        .show(ui, |ui| {
            for (id, note) in refs {
                let title = doc_title(&app.knowledge_docs, *id);
                egui::CollapsingHeader::new(format!("〔E{id}〕 {title}"))
                    .id_salt(id)
                    .show(ui, |ui| {
                        if !note.is_empty() {
                            ui.label(note);
                        }
                        if ui.link("在此阅读原文").clicked() {
                            open = Some(*id);
                        }
                    });
            }
        });
    if let Some(error) = &app.wiki.evidence_error {
        ui.colored_label(theme::warn(), error);
    }
    if let Some((id, title, markdown)) = &app.wiki.evidence_original
        && refs.iter().any(|(source, _)| source == id)
    {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong("原文开头");
            if ui.link("版式预览").clicked() {
                paper = Some(*id);
            }
            close = ui.small_button("关闭").clicked();
        });
        ui.strong(title);
        egui::ScrollArea::vertical()
            .id_salt(("wiki_original", id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(markdown);
            });
    }
    if close {
        app.wiki.evidence_original = None;
    }
    if let Some(id) = open {
        read_evidence(app, id);
    }
    if let Some(id) = paper {
        app.knowledge_open_preview(id);
    }
}
