//! 维基知识库页：左侧主题→分类→页面树，中间页面阅读器（`[[内链]]` 可点击跳转），
//! 右侧 backlinks 窄栏（常驻可折叠）。设计方案见 `docs/wiki-knowledge-base-design.md` §9。
//!
//! 口径 14（配色跟随主题）：本页不引入独立配色，全部取 `theme` 语义色。
//! 口径 13（渲染复用）：正文渲染集中在 [`show_page_body`]——它是换接点，后续切片
//! 把现有预览渲染链接入后，这里整体换掉、链接只作为行内挂件保留。

use std::collections::HashMap;

use crate::app::GongwenApp;
use crate::theme;
use crate::wiki::{Backlink, WikiPageMeta, WikiTree};
use eframe::egui;

/// 知识库页的界面状态：框架树缓存、当前阅读页、backlinks 栏开合。
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
            ui.weak(format!("{} 个主题", app.wiki.tree.len()));
        });
    });
    ui.add_space(4.0);
    if app.wiki.tree.is_empty() {
        ui.weak("还没有知识框架。框架建立（从资料归纳 / 导入大纲）将在后续切片接入。");
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for t in &app.wiki.tree {
            let theme_selected = app
                .wiki
                .open
                .as_ref()
                .is_some_and(|p| t.theme_guide.as_deref() == Some(p.meta.uuid.as_str()));
            if tree_row(ui, &t.theme.name, theme_selected).clicked()
                && let Some(g) = &t.theme_guide
            {
                *nav = Some(g.clone());
            }
            for c in &t.categories {
                tree_row(ui, &format!("　{}", c.name), false);
                for m in app.wiki.pages.get(&c.id).into_iter().flatten() {
                    let label = if m.kind == "guide" {
                        format!("　　{}（导览）", m.title)
                    } else {
                        format!("　　{}", m.title)
                    };
                    if tree_row(ui, &label, page_selected(app, &m.uuid)).clicked() {
                        *nav = Some(m.uuid.clone());
                    }
                }
            }
        }
    });
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
