//! 「管理分类」弹窗：每个分类一行，就地改名、迁移后删除。
use super::{Snapshot, State};
use crate::task_assistant::Store;
use crate::theme;

pub(super) fn window(ui: &mut egui::Ui, store: &mut Store, state: &mut State, data: &Snapshot) {
    if !state.categories_open {
        return;
    }
    let mut open = true;
    egui::Window::new("管理分类")
        .id(egui::Id::new("task_categories"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ui.ctx(), |ui| {
            theme::caption(
                ui,
                "每个任务一个分类。删除分类时，其下任务（含归档与回收站）迁到另一个分类。",
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add(theme::field(&mut state.new_category, "新分类名称", 240.0));
                if ui
                    .add(theme::secondary_icon_button(theme::Icon::Plus, "新增"))
                    .clicked()
                {
                    match store.add_category(&state.new_category) {
                        Ok(()) => {
                            state.ok(format!("已新增分类「{}」", state.new_category.trim()));
                            state.new_category.clear();
                        }
                        Err(e) => state.fail(format!("新增分类失败：{e:#}")),
                    }
                }
            });
            ui.separator();
            for name in &data.categories {
                ui.push_id(("task_category", name), |ui| {
                    row(ui, store, state, data, name)
                });
            }
        });
    if !open {
        state.categories_open = false;
        state.category_rename = None;
        state.category_delete = None;
    }
}

fn row(ui: &mut egui::Ui, store: &mut Store, state: &mut State, data: &Snapshot, name: &str) {
    let count = data.tasks.iter().filter(|t| t.category == name).count();
    let fixed = name == "未分类";
    let renaming = state
        .category_rename
        .as_ref()
        .is_some_and(|(old, _)| old == name);
    ui.horizontal(|ui| {
        if renaming {
            let (_, new) = state.category_rename.as_mut().expect("刚判断过");
            ui.add(theme::field(new, "新名称", 200.0));
            if ui.button("确定").clicked() {
                let new = new.clone();
                match store.rename_category(name, &new) {
                    Ok(()) => {
                        if state.category == name {
                            state.category = new.trim().to_owned();
                        }
                        state.category_rename = None;
                        state.ok("分类已改名");
                    }
                    Err(e) => state.fail(format!("分类改名失败：{e:#}")),
                }
            }
            if ui.button("取消").clicked() {
                state.category_rename = None;
            }
            return;
        }
        ui.label(name);
        theme::caption(ui, &format!("{count} 项"));
        if fixed {
            theme::caption(ui, "默认分类，不能改名或删除");
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("删除…").clicked() {
                state.category_delete = Some((name.to_owned(), "未分类".to_owned()));
            }
            if ui.small_button("改名").clicked() {
                state.category_rename = Some((name.to_owned(), name.to_owned()));
            }
        });
    });
    let deleting = state
        .category_delete
        .as_ref()
        .is_some_and(|(from, _)| from == name);
    if !deleting {
        return;
    }
    ui.indent(("task_category_delete", name), |ui| {
        ui.horizontal_wrapped(|ui| {
            theme::caption(ui, &format!("把 {count} 项任务迁到"));
            let (_, target) = state.category_delete.as_mut().expect("刚判断过");
            egui::ComboBox::from_id_salt(("task_category_target", name))
                .selected_text(target.as_str())
                .show_ui(ui, |ui| {
                    for c in data.categories.iter().filter(|c| c.as_str() != name) {
                        ui.selectable_value(target, c.clone(), c);
                    }
                });
            let target = target.clone();
            if ui
                .add(theme::warning_icon_button(theme::Icon::Trash, "迁移并删除"))
                .clicked()
            {
                match store.move_category(name, &target) {
                    Ok(()) => {
                        if state.category == name {
                            state.category.clear();
                        }
                        state.category_delete = None;
                        state.ok(format!("分类「{name}」已删除，任务迁到「{target}」"));
                    }
                    Err(e) => state.fail(format!("分类迁移失败：{e:#}")),
                }
            }
            if ui.button("取消").clicked() {
                state.category_delete = None;
            }
        });
    });
}
