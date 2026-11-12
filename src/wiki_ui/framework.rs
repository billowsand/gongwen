//! 分类框架的管理详情和分类口径编辑。

use super::*;
use crate::wiki::{MAX_FRAME_GUIDANCE_CHARS, WikiNode};

pub(super) struct DefinitionDraft {
    name: String,
    guidance: String,
}

pub(super) fn existing(app: &mut GongwenApp, ui: &mut egui::Ui) {
    let busy = app.wiki.busy();
    workspaces::left_panel(ui, "wiki_manage_tree", |ui| {
        ui.horizontal(|ui| {
            ui.strong("分类框架");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.menu_button("＋ 主题", |ui| {
                        if let Some(name) = new_name(ui, &mut app.wiki.new_node_name, "新主题名称")
                        {
                            create(app, None, &name);
                        }
                    });
                });
            });
        });
        ui.add_space(8.0);
        tree_pane(app, ui, &mut None, true);
    });
    detail_pane(ui, "wiki_manage_detail", |ui| detail(app, ui, busy));
}

pub(super) fn detail_pane(
    ui: &mut egui::Ui,
    id: &'static str,
    content: impl FnOnce(&mut egui::Ui),
) {
    body::scroll_pane(ui, id, |ui| {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(24, 16))
            .show(ui, |ui| {
                ui.set_max_width(720.0);
                content(ui);
            });
    });
}

fn detail(app: &mut GongwenApp, ui: &mut egui::Ui, busy: bool) {
    let Some(id) = app.wiki.selected_node else {
        ui.heading("分类定义");
        ui.weak("选择左侧节点，或添加主题以建立框架。");
        return;
    };
    let Some((branch, node)) = app.wiki.tree.iter().find_map(|branch| {
        if branch.theme.id == id {
            Some((branch, &branch.theme))
        } else {
            branch
                .categories
                .iter()
                .find(|node| node.id == id)
                .map(|node| (branch, node))
        }
    }) else {
        app.wiki.selected_node = None;
        return;
    };
    let node = node.clone();
    let parent = node.parent_id.map(|_| branch.theme.name.clone());
    let inherited = parent.as_ref().map(|_| branch.theme.guidance.clone());
    let can_compile = parent.is_some() || branch.theme_guide.is_none();
    let draft = app
        .wiki
        .node_drafts
        .entry(id)
        .or_insert_with(|| DefinitionDraft {
            name: node.name.clone(),
            guidance: node.guidance.clone(),
        });
    ui.weak(parent.as_deref().unwrap_or("主题"));
    ui.heading(&node.name);
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    ui.push_id(id, |ui| {
        definition_fields(
            ui,
            if parent.is_some() {
                "分类名称"
            } else {
                "主题名称"
            },
            &mut draft.name,
            &mut draft.guidance,
        );
    });
    if let Some(guidance) = inherited.filter(|text| !text.is_empty()) {
        egui::CollapsingHeader::new("上级主题口径")
            .id_salt(id)
            .show(ui, |ui| {
                ui.label(guidance);
            });
    }
    let changed = draft.name.trim() != node.name || draft.guidance.trim() != node.guidance;
    let mut save = false;
    let mut compile = false;
    let mut add_child = None;
    let mut remove = false;
    let mut move_to = None;
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        save = ui
            .add_enabled(
                !busy
                    && changed
                    && !draft.name.trim().is_empty()
                    && crate::wiki::validate_frame_guidance(draft.guidance.trim()).is_ok(),
                egui::Button::new("保存定义"),
            )
            .clicked();
        if can_compile {
            compile = ui
                .add_enabled(
                    !busy && !changed,
                    theme::icon_text_button(
                        theme::Icon::Sparkles,
                        if parent.is_some() {
                            "整编此分类"
                        } else {
                            "整编主题导览"
                        },
                    ),
                )
                .on_disabled_hover_text("先保存分类定义，再发起整编。")
                .clicked();
        }
        ui.add_enabled_ui(!busy, |ui| {
            if parent.is_none() {
                ui.menu_button("＋ 分类", |ui| {
                    add_child = new_name(ui, &mut app.wiki.new_category_name, "新分类名称");
                });
            }
            ui.menu_button("更多", |ui| {
                if let Some(current) = node.parent_id {
                    ui.menu_button("移动到主题", |ui| {
                        for branch in &app.wiki.tree {
                            if branch.theme.id != current && ui.button(&branch.theme.name).clicked()
                            {
                                move_to = Some(branch.theme.id);
                                ui.close();
                            }
                        }
                    });
                }
                remove = ui
                    .button(egui::RichText::new("删除节点").color(theme::warn()))
                    .clicked();
                if remove {
                    ui.close();
                }
            });
        });
    });
    if save && let Some(store) = &app.wiki_store {
        match store.save_node_definition(id, &draft.name, &draft.guidance) {
            Ok(()) => {
                app.wiki.node_drafts.remove(&id);
                app.wiki.invalidate();
                app.wiki.notice = Some("分类定义已保存。".into());
            }
            Err(error) => app.wiki.notice = Some(format!("保存失败：{error:#}")),
        }
    }
    if compile {
        app.start_wiki_write(id, Vec::new());
    }
    if let Some(name) = add_child {
        create(app, Some(id), &name);
    }
    if remove {
        delete(app, &node);
    }
    if let Some(target) = move_to
        && let Some(store) = &app.wiki_store
    {
        match store.move_category(id, target) {
            Ok(()) => {
                app.wiki.node_expansion.insert(target, true);
                app.wiki.invalidate();
                app.wiki.notice = Some(format!("已移动「{}」。", node.name));
            }
            Err(error) => app.wiki.notice = Some(format!("移动失败：{error:#}")),
        }
    }
    associated_documents(app, ui, id);
}

fn associated_documents(app: &mut GongwenApp, ui: &mut egui::Ui, id: i64) {
    if app
        .wiki
        .frame_documents
        .as_ref()
        .is_none_or(|(selected, _)| *selected != id)
        && let Some(store) = &app.wiki_store
    {
        match store.frame_documents(id) {
            Ok(docs) => app.wiki.frame_documents = Some((id, docs)),
            Err(error) => {
                ui.colored_label(theme::warn(), format!("关联资料读取失败：{error:#}"));
                return;
            }
        }
    }
    let Some((_, docs)) = &app.wiki.frame_documents else {
        return;
    };
    if docs.is_empty() {
        return;
    }
    ui.add_space(18.0);
    ui.separator();
    ui.add_space(6.0);
    ui.strong(format!("关联资料 · {} 篇", docs.len()));
    let mut open = None;
    for (id, title) in docs {
        let title = if title.is_empty() {
            format!("E{id}")
        } else {
            title.clone()
        };
        if ui
            .link(title)
            .on_hover_text(format!("打开资料 E{id}"))
            .clicked()
        {
            open = Some(*id);
        }
    }
    if let Some(id) = open {
        app.knowledge_open_preview(id);
    }
}

pub(super) fn definition_fields(
    ui: &mut egui::Ui,
    name_label: &str,
    name: &mut String,
    guidance: &mut String,
) {
    ui.label(name_label);
    ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
    ui.add_space(12.0);
    ui.strong("分类口径");
    ui.weak("写明收录范围、排除条件和容易混淆的归属。模型归纳框架、整编和更新页面时会读取。");
    ui.add(
        egui::TextEdit::multiline(guidance)
            .desired_width(f32::INFINITY)
            .desired_rows(6)
            .hint_text("收录：日常监督检查、问题台账和整改复查。\n排除：应急预案、演练和事故处置。\n同时涉及两类时，按资料的主要内容归入。"),
    );
    let count = guidance.trim().chars().count();
    let limit = format!("{count} / {MAX_FRAME_GUIDANCE_CHARS} 字");
    if count > MAX_FRAME_GUIDANCE_CHARS {
        ui.colored_label(theme::warn(), limit);
    } else {
        ui.weak(limit);
    }
}

fn new_name(ui: &mut egui::Ui, edit: &mut String, hint: &str) -> Option<String> {
    let mut name = None;
    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(edit)
                .hint_text(hint)
                .desired_width(160.0),
        );
        let text = edit.trim();
        if !text.is_empty()
            && (ui.button("添加").clicked()
                || (field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))))
        {
            name = Some(text.to_owned());
            ui.close();
        }
    });
    name
}

fn create(app: &mut GongwenApp, parent: Option<i64>, name: &str) {
    let Some(store) = &app.wiki_store else { return };
    let result = match parent {
        Some(parent) => store.create_category(parent, name),
        None => store.create_theme(name),
    };
    match result {
        Ok(id) => {
            if let Some(parent) = parent {
                app.wiki.node_expansion.insert(parent, true);
                app.wiki.new_category_name.clear();
            } else {
                app.wiki.new_node_name.clear();
            }
            app.wiki.selected_node = Some(id);
            app.wiki.invalidate();
        }
        Err(error) => app.wiki.notice = Some(format!("新建失败：{error:#}")),
    }
}

fn delete(app: &mut GongwenApp, node: &WikiNode) {
    let Some(store) = &app.wiki_store else { return };
    match store.delete_node(node.id) {
        Ok(()) => {
            app.wiki.notice = Some(format!("已删除「{}」。", node.name));
            app.wiki.selected_node = None;
            app.wiki.node_drafts.remove(&node.id);
            app.wiki.invalidate();
        }
        Err(error) => app.wiki.notice = Some(format!("删除失败：{error:#}")),
    }
}
