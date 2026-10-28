//! 中栏任务列表：筛选条 + 整行可点的任务行。
use super::{Replace, Snapshot, State, View, days_from, short_date};
use crate::task_assistant::{Role, Store, Task};
use crate::theme;

pub(super) fn show(ui: &mut egui::Ui, store: &mut Store, state: &mut State, data: &Snapshot) {
    toolbar(ui, state);
    let query = state.query.trim().to_lowercase();
    let hits = if query.is_empty() {
        None
    } else {
        match state.cache.hits(store, &query) {
            Ok(hits) => Some(hits.clone()),
            Err(e) => {
                state.error = Some(e);
                None
            }
        }
    };
    let mut rows = data
        .tasks
        .iter()
        .filter(|t| {
            let in_scope = if query.is_empty() {
                state.view.matches(&data.tasks, t, &data.today)
            } else {
                // 搜索不受视图限制，回收站里的只在回收站视图里搜。
                t.deleted == (state.view == View::Deleted)
            };
            in_scope
                && (state.category.is_empty() || state.category == t.category)
                && (state.role_filter.is_empty() || state.role_filter == t.role.as_str())
                && (query.is_empty()
                    || [
                        &t.name,
                        &t.next_step,
                        &t.collaborators,
                        &t.category,
                        &t.goal,
                    ]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&query))
                    || hits.as_ref().is_some_and(|h| h.contains(&t.id)))
        })
        .collect::<Vec<_>>();
    if state.sort_deadline {
        rows.sort_by(|a, b| {
            a.deadline
                .as_deref()
                .unwrap_or("9999-12-31")
                .cmp(b.deadline.as_deref().unwrap_or("9999-12-31"))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
    }
    let heading = if query.is_empty() {
        format!("{} · {} 项", state.view.label(), rows.len())
    } else {
        format!(
            "搜索「{}」· {} 项（不限视图）",
            state.query.trim(),
            rows.len()
        )
    };
    theme::caption(ui, &heading);
    ui.add_space(2.0);
    if rows.is_empty() {
        empty(ui, state, data, !query.is_empty());
        return;
    }
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("task_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for t in rows {
                if row(ui, t, data, state.selected.as_deref() == Some(&t.id), state) {
                    clicked = Some(t.id.clone());
                }
            }
        });
    if let Some(id) = clicked {
        state.selected = Some(id);
    }
}

fn toolbar(ui: &mut egui::Ui, state: &mut State) {
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("task_filter_role")
            .selected_text(if state.role_filter.is_empty() {
                "全部角色"
            } else {
                &state.role_filter
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.role_filter, String::new(), "全部角色");
                for r in [
                    Role::Responsible,
                    Role::Collaborate,
                    Role::Follow,
                    Role::Watch,
                ] {
                    ui.selectable_value(&mut state.role_filter, r.as_str().into(), r.as_str());
                }
            });
        theme::segmented(ui, |ui| {
            ui.selectable_value(&mut state.sort_deadline, true, "按期限");
            ui.selectable_value(&mut state.sort_deadline, false, "按更新");
        });
        if !state.category.is_empty()
            && ui
                .add(theme::removable_tag_button(&state.category, false))
                .on_hover_text("取消分类筛选")
                .clicked()
        {
            state.category.clear();
        }
    });
    ui.add_space(4.0);
}

fn empty(ui: &mut egui::Ui, state: &mut State, data: &Snapshot, searching: bool) {
    ui.add_space(16.0);
    if searching {
        theme::caption(ui, "没有找到匹配的任务。换个关键词，或清除搜索。");
        return;
    }
    if data.tasks.iter().all(|t| t.deleted) {
        ui.label("还没有任务。");
        theme::caption(
            ui,
            "新建一个，或把通知、纪要粘贴进来，让 AI 拆成待核对的建议。",
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::primary_icon_button(ui, theme::Icon::Plus, "新建一个任务").clicked() {
                state.open_draft(Replace::Blank);
            }
            if ui
                .add(theme::secondary_icon_button(
                    theme::Icon::Paperclip,
                    "去粘贴材料",
                ))
                .clicked()
            {
                state.view = View::Inbox;
                state.focus_material = true;
            }
        });
    } else {
        theme::caption(ui, "这个视图里没有任务。");
    }
}

/// 一行任务。返回是否被点选。
fn row(ui: &mut egui::Ui, t: &Task, data: &Snapshot, selected: bool, state: &State) -> bool {
    let frame = egui::Frame::new()
        .fill(if selected {
            theme::accent_soft()
        } else {
            egui::Color32::TRANSPARENT
        })
        .corner_radius(theme::chrome_radius(6))
        .inner_margin(egui::Margin::symmetric(10, 6));
    theme::clickable_card(ui, ("task_row", &t.id), frame, selected, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                deadline_chip(ui, t, &data.today);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&t.name).strong()).truncate());
                });
            });
        });
        let next = if t.next_step.is_empty() {
            "下一步：未安排".to_owned()
        } else {
            match &t.next_deadline {
                Some(d) => format!("下一步：{} · {}", t.next_step, short_date(d, &data.today)),
                None => format!("下一步：{}", t.next_step),
            }
        };
        let next_late = !t.status.terminal()
            && t.next_deadline
                .as_deref()
                .is_some_and(|d| d < data.today.as_str());
        ui.add(
            egui::Label::new(
                egui::RichText::new(next)
                    .size(theme::font_sizes::SMALL)
                    .color(if next_late {
                        theme::danger()
                    } else {
                        theme::text_muted()
                    }),
            )
            .truncate(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if t.needs_review {
                theme::chip(ui, "待核对", theme::warn(), theme::warn_soft());
            }
            if !t.status.terminal() && super::nav::held_up(&data.tasks, t) {
                theme::chip(ui, "阻塞", theme::danger(), theme::danger_soft());
            } else {
                neutral(ui, t.status.as_str());
            }
            neutral(ui, t.role.as_str());
            if state.category.is_empty() && t.category != "未分类" {
                neutral(ui, &t.category);
            }
        });
    })
    .response
    .clicked()
}

fn neutral(ui: &mut egui::Ui, text: &str) {
    theme::chip(ui, text, theme::text_soft(), theme::surface_sunk());
}

/// 行右上角只放一个最要紧的期限：逾期 > 今天到期 > 普通日期。
fn deadline_chip(ui: &mut egui::Ui, t: &Task, today: &str) {
    let Some(d) = t.deadline.as_deref() else {
        return;
    };
    if t.status.terminal() {
        theme::chip(
            ui,
            &short_date(d, today),
            theme::text_muted(),
            theme::surface_sunk(),
        );
        return;
    }
    match days_from(today, d) {
        Some(n) if n < 0 => {
            theme::chip(
                ui,
                &format!("逾期 {} 天", -n),
                theme::danger(),
                theme::danger_soft(),
            );
        }
        Some(0) => {
            theme::chip(ui, "今天到期", theme::warn(), theme::warn_soft());
        }
        _ => {
            theme::chip(
                ui,
                &short_date(d, today),
                theme::text_soft(),
                theme::surface_sunk(),
            );
        }
    }
}
