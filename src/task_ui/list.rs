//! 中栏任务列表：一行表头（视图与数量在左，负责人筛选、排序在右）+ 整行可点的任务行。
//!
//! 任务行只用三行：名称与期限、下一步、负责人与状态。标签只留给要人注意的
//! 情况（待核对、阻塞、有风险），负责人、状态、分类用淡色文字，不再一排五个同样的标签。
use super::{Replace, Snapshot, State, View, days_from, short_date};
use crate::task_assistant::{Store, Task};
use crate::theme;

/// `compact_nav`：视图栏收成了列表上方的下拉框，下拉框里已有视图与数量，表头不再重复。
pub(super) fn show(
    ui: &mut egui::Ui,
    store: &mut Store,
    state: &mut State,
    data: &Snapshot,
    compact_nav: bool,
) {
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
                && (state.owner_filter.is_empty() || state.owner_filter == t.owner_name)
                && (query.is_empty()
                    || [
                        &t.name,
                        &t.next_step,
                        &t.owner_name,
                        &t.progress,
                        &t.risks,
                        &t.category,
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
    let heading = (!compact_nav || !query.is_empty()).then_some(heading);
    toolbar(ui, state, &data.tasks, heading.as_deref());
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
            let show_category = state.category.is_empty();
            for t in rows {
                if row(
                    ui,
                    t,
                    data,
                    state.selected.as_deref() == Some(&t.id),
                    show_category,
                ) {
                    clicked = Some(t.id.clone());
                }
            }
        });
    if let Some(id) = clicked {
        state.selected = Some(id);
    }
}

fn toolbar(ui: &mut egui::Ui, state: &mut State, tasks: &[Task], heading: Option<&str>) {
    // 够宽时表头与筛选同一行；中栏窄（带下拉视图栏、窄窗口单栏）时筛选换到下一行。
    match heading {
        Some(heading) if ui.available_width() >= 400.0 => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(heading).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    filters(ui, state, tasks);
                });
            });
        }
        Some(heading) => {
            ui.label(egui::RichText::new(heading).strong());
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| filters(ui, state, tasks));
        }
        None => {
            ui.horizontal_wrapped(|ui| filters(ui, state, tasks));
        }
    }
    ui.add_space(6.0);
}

/// 负责人筛选、排序与分类筛选标签。右到左排时先画的在最右。
fn filters(ui: &mut egui::Ui, state: &mut State, tasks: &[Task]) {
    let right_to_left = ui.layout().main_dir() == egui::Direction::RightToLeft;
    let mut owner = |ui: &mut egui::Ui| {
        egui::ComboBox::from_id_salt("task_filter_owner")
            .selected_text(if state.owner_filter.is_empty() {
                "全部负责人"
            } else {
                &state.owner_filter
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.owner_filter, String::new(), "全部负责人");
                let names = tasks
                    .iter()
                    .map(|t| t.owner_name.trim())
                    .filter(|n| !n.is_empty())
                    .collect::<std::collections::BTreeSet<_>>();
                for name in names {
                    ui.selectable_value(&mut state.owner_filter, name.to_owned(), name);
                }
            });
    };
    // 分段按钮里的横排随外层方向，右到左时倒着加，读起来仍是「按期限 · 按更新」。
    let sort = |ui: &mut egui::Ui, sort_deadline: &mut bool| {
        theme::segmented(ui, |ui| {
            let options = [(true, "按期限"), (false, "按更新")];
            let mut add = |(value, label): (bool, &str)| {
                ui.selectable_value(sort_deadline, value, label);
            };
            if right_to_left {
                options.into_iter().rev().for_each(&mut add);
            } else {
                options.into_iter().for_each(&mut add);
            }
        });
    };
    let category = |ui: &mut egui::Ui, category: &mut String| {
        if !category.is_empty()
            && ui
                .add(theme::removable_tag_button(category, false))
                .on_hover_text("取消分类筛选")
                .clicked()
        {
            category.clear();
        }
    };
    // 视觉顺序固定为：分类标签、负责人、排序。
    if right_to_left {
        sort(ui, &mut state.sort_deadline);
        owner(ui);
        category(ui, &mut state.category);
    } else {
        category(ui, &mut state.category);
        owner(ui);
        sort(ui, &mut state.sort_deadline);
    }
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

/// 一行任务。返回是否被点选。`show_category`：没按分类筛选时才在行里写分类。
fn row(ui: &mut egui::Ui, t: &Task, data: &Snapshot, selected: bool, show_category: bool) -> bool {
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
            "下一步：未填".to_owned()
        } else {
            format!("下一步：{}", t.next_step)
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(next)
                    .size(theme::font_sizes::SMALL)
                    .color(theme::text_muted()),
            )
            .truncate(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let small = |text: &str, color| {
                egui::RichText::new(text)
                    .size(theme::font_sizes::SMALL)
                    .color(color)
            };
            // 淡色的事实：负责人 · 状态 · 分类。
            if t.owner_name.trim().is_empty() {
                ui.label(small("未指定负责人", theme::text_muted()));
            } else {
                let (label, found) = super::owner::label_bound(
                    data.vocabulary,
                    t.owner_id,
                    &t.owner_name,
                    &t.owner_identity,
                );
                let color = if found || t.owner_id.is_none() {
                    theme::text_soft()
                } else {
                    theme::warn()
                };
                ui.label(small(&t.owner_name, color)).on_hover_text(label);
            }
            let held = !t.status.terminal() && super::nav::held_up(&data.tasks, t);
            if !held {
                ui.label(small("·", theme::text_muted()));
                ui.label(small(t.status.as_str(), theme::text_soft()));
            }
            if show_category && t.category != "未分类" {
                ui.label(small("·", theme::text_muted()));
                ui.label(small(&t.category, theme::text_soft()));
            }
            // 要人注意的才用标签。
            if t.needs_review || held || !t.risks.trim().is_empty() {
                ui.add_space(4.0);
            }
            if held {
                theme::chip(ui, "阻塞", theme::danger(), theme::danger_soft());
            }
            if t.needs_review {
                theme::chip(ui, "待核对", theme::warn(), theme::warn_soft());
            }
            if !t.risks.trim().is_empty() {
                theme::chip(ui, "有风险", theme::warn(), theme::warn_soft());
            }
        });
    })
    .response
    .clicked()
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
