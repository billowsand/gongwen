//! 右栏任务详情：标题与操作、提醒横条、事实条（负责人 / 期限 / 分类），
//! 正文三块（当前进展可就地更新、下一步、风险与措施），关系，历史时间线。
use super::{Replace, Snapshot, State, View, banner, days_from, short_time};
use crate::task_assistant::{Status, Store, Task};
use crate::theme;

/// 这几类历史说明了任务为什么被标成待核对，横条上取最近一条给出原因。
const REVIEW_REASONS: [&str; 6] = [
    "依赖复核",
    "关系解除",
    "恢复",
    "采纳新任务候选",
    "采纳材料建议",
    "分类迁移",
];

pub(super) fn show(ui: &mut egui::Ui, store: &mut Store, state: &mut State, data: &Snapshot) {
    let Some(t) = state.selected.as_deref().and_then(|id| data.task(id)) else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            theme::caption(ui, "从列表选择任务查看详情，或新建任务。");
        });
        return;
    };
    let t = t.clone();
    let readonly = t.archived || t.deleted;
    ui.add(egui::Label::new(egui::RichText::new(&t.name).heading()).wrap());
    ui.add_space(4.0);
    header_actions(ui, store, state, &t, readonly);
    status_feedback(ui, store, state, &t);
    if t.needs_review && !readonly {
        let reason = state
            .cache
            .history(store, &t.id)
            .ok()
            .and_then(|h| {
                h.iter()
                    .find(|e| REVIEW_REASONS.contains(&e.action.as_str()))
                    .map(|e| e.detail.lines().next().unwrap_or_default().to_owned())
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "材料或关系变更后需要人工核对安排".into());
        ui.add_space(6.0);
        if banner(
            ui,
            theme::Icon::TriangleAlert,
            theme::warn(),
            theme::warn_soft(),
            &format!("待核对：{reason}"),
            "已核对",
        ) {
            let mut update = t.clone();
            update.needs_review = false;
            match store.save(&update, "核对", "用户确认已核对") {
                Ok(()) => state.ok("已标记为核对过"),
                Err(e) => state.fail(format!("无法标记已核对：{e:#}")),
            }
        }
    }
    let pending = data
        .candidates
        .iter()
        .filter(|c| super::nav::pending_candidate(&c.4))
        .filter(|c| {
            serde_json::from_str::<crate::task_assistant::extract::Proposal>(&c.2)
                .is_ok_and(|p| p.task_id.as_deref() == Some(t.id.as_str()))
        })
        .count();
    if pending > 0 {
        ui.add_space(6.0);
        if banner(
            ui,
            theme::Icon::Sparkles,
            theme::accent(),
            theme::accent_soft(),
            &format!("有 {pending} 条与本任务有关的 AI 建议待核对"),
            "去处理",
        ) {
            state.view = View::Inbox;
        }
    }
    ui.add_space(12.0);
    info(ui, &t, &data.today, data.vocabulary);
    if !readonly {
        sync_progress_box(state, &t);
    }
    progress(ui, store, state, &t, readonly);
    section(ui, "下一步工作", |_| {});
    body(ui, &t.next_step, "还没有安排下一步。");
    section(ui, "风险问题及应对措施", |ui| {
        if !t.risks.trim().is_empty() {
            theme::chip(ui, "有风险", theme::warn(), theme::warn_soft());
        }
    });
    body(ui, &t.risks, "未填");
    relations(ui, state, &t, data);
    ui.add_space(14.0);
    theme::hairline(ui);
    history(ui, store, state, &t);
    ui.add_space(12.0);
}

fn header_actions(
    ui: &mut egui::Ui,
    store: &mut Store,
    state: &mut State,
    t: &Task,
    readonly: bool,
) {
    ui.horizontal_wrapped(|ui| {
        if readonly {
            let (label, action) = if t.deleted {
                ("在回收站 · 只读", "从回收站恢复")
            } else {
                ("已归档 · 只读", "移出归档")
            };
            theme::chip(ui, label, theme::text_soft(), theme::surface_sunk());
            if ui
                .add(theme::secondary_icon_button(theme::Icon::RotateCcw, action))
                .clicked()
            {
                match store.restore_task(&t.id) {
                    Ok(()) => {
                        state.ok("任务已恢复，原有关系请重新核对");
                        state.view = View::Active;
                    }
                    Err(e) => state.fail(format!("恢复失败：{e:#}")),
                }
            }
            return;
        }
        let mut chosen = t.status;
        egui::ComboBox::from_id_salt(("task_status", &t.id))
            .selected_text(format!("状态：{}", t.status.as_str()))
            .show_ui(ui, |ui| {
                for s in [
                    Status::Todo,
                    Status::InProgress,
                    Status::Waiting,
                    Status::Blocked,
                    Status::Done,
                    Status::Cancelled,
                ] {
                    ui.selectable_value(&mut chosen, s, s.as_str());
                }
            });
        if chosen != t.status {
            state.status_error = None;
            if chosen == Status::Cancelled {
                state.status_cancel = Some(t.id.clone());
                state.cancel_reason.clear();
            } else {
                let mut update = t.clone();
                update.status = chosen;
                match store.save(&update, "状态变更", chosen.as_str()) {
                    Ok(()) => state.ok(format!("状态已改为「{}」", chosen.as_str())),
                    Err(e) => state.status_error = Some((t.id.clone(), format!("{e:#}"))),
                }
            }
        }
        if ui
            .add(theme::secondary_icon_button(theme::Icon::Edit, "编辑"))
            .clicked()
        {
            state.open_draft(Replace::Edit(Box::new(t.clone())));
        }
        ui.menu_button("更多", |ui| {
            let archive = ui
                .add_enabled(
                    t.status.terminal(),
                    theme::menu_item(theme::Icon::Archive, "归档"),
                )
                .on_disabled_hover_text("完成或取消后才能归档");
            if archive.clicked() {
                let mut update = t.clone();
                update.archived = true;
                match store.save(&update, "归档", "用户归档") {
                    Ok(()) => state.ok("任务已归档，可在「归档」视图里移出"),
                    Err(e) => state.fail(format!("归档失败：{e:#}")),
                }
                ui.close();
            }
            if ui
                .add(
                    egui::Button::image_and_text(
                        theme::Icon::Trash.image().tint(theme::danger()),
                        egui::RichText::new("移入回收站…").color(theme::danger()),
                    )
                    .min_size(egui::vec2(0.0, 26.0)),
                )
                .clicked()
            {
                state.delete_confirm = Some(t.id.clone());
                ui.close();
            }
        });
    });
}

/// 状态下拉旁的反馈：取消理由输入与状态切换失败原因，就近显示。
fn status_feedback(ui: &mut egui::Ui, store: &mut Store, state: &mut State, t: &Task) {
    if state.status_cancel.as_deref() == Some(t.id.as_str()) {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(theme::field(
                &mut state.cancel_reason,
                "取消理由（必填），如：上级通知暂缓",
                (ui.available_width() - 160.0).max(160.0),
            ));
            if ui
                .add(theme::warning_icon_button(theme::Icon::X, "确认取消"))
                .clicked()
            {
                let mut update = t.clone();
                update.status = Status::Cancelled;
                let reason = state.cancel_reason.trim().to_owned();
                match store.save(&update, "状态变更", &reason) {
                    Ok(()) => {
                        state.status_cancel = None;
                        state.cancel_reason.clear();
                        state.ok("任务已取消");
                    }
                    Err(e) => state.status_error = Some((t.id.clone(), format!("{e:#}"))),
                }
            }
            if ui.button("不取消了").clicked() {
                state.status_cancel = None;
                state.cancel_reason.clear();
                state.status_error = None;
            }
        });
    }
    if let Some((id, e)) = &state.status_error
        && id == &t.id
    {
        ui.add_space(4.0);
        theme::notice(
            ui,
            theme::Icon::TriangleAlert,
            theme::danger(),
            theme::danger_soft(),
            format!("状态没有改：{e}"),
        );
    }
}

fn info(ui: &mut egui::Ui, t: &Task, today: &str, vocabulary: &[crate::models::VocabularyEntry]) {
    let (owner, found) =
        super::owner::label_bound(vocabulary, t.owner_id, &t.owner_name, &t.owner_identity);
    let owner = if t.owner_name.trim().is_empty() {
        muted("未指定")
    } else if found || t.owner_id.is_none() {
        egui::RichText::new(owner)
    } else {
        egui::RichText::new(owner).color(theme::warn())
    };
    let deadline = match &t.deadline {
        Some(d) => match days_from(today, d) {
            Some(n) if n < 0 && !t.status.terminal() => {
                egui::RichText::new(format!("{d} · 已逾期 {} 天", -n)).color(theme::danger())
            }
            Some(0) if !t.status.terminal() => {
                egui::RichText::new(format!("{d} · 今天到期")).color(theme::warn())
            }
            _ => egui::RichText::new(d.clone()),
        },
        None => muted("未定"),
    };
    // 「谁、何时、归哪类」是一眼要看的事实，横排成一条；窄了自动折行。
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(28.0, 8.0);
        fact(ui, "负责人", owner);
        fact(ui, "任务期限", deadline);
        fact(ui, "分类", egui::RichText::new(t.category.clone()));
    });
}

fn fact(ui: &mut egui::Ui, label: &str, value: egui::RichText) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        theme::caption(ui, label);
        ui.label(value);
    });
}

fn muted(text: &str) -> egui::RichText {
    egui::RichText::new(text).color(theme::text_muted())
}

/// 正文区块的标题行：左边标题，右边可放一个就地操作。
fn section(ui: &mut egui::Ui, title: &str, trailing: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
    });
    ui.add_space(2.0);
}

/// 区块正文：有内容按正文字号换行显示，没有就给一句淡色提示。
fn body(ui: &mut egui::Ui, text: &str, empty: &str) {
    if text.trim().is_empty() {
        ui.label(muted(empty));
    } else {
        ui.add(egui::Label::new(text.trim()).wrap().selectable(true));
    }
}

fn relations(ui: &mut egui::Ui, state: &mut State, t: &Task, data: &Snapshot) {
    let mut links: Vec<(&str, &Task)> = Vec::new();
    if let Some(parent) = t.parent_id.as_deref().and_then(|id| data.task(id)) {
        links.push(("上级", parent));
    }
    links.extend(
        data.tasks
            .iter()
            .filter(|x| t.prerequisites.contains(&x.id))
            .map(|x| ("前置", x)),
    );
    links.extend(
        data.tasks
            .iter()
            .filter(|x| x.parent_id.as_deref() == Some(t.id.as_str()))
            .map(|x| ("子任务", x)),
    );
    links.extend(
        data.tasks
            .iter()
            .filter(|x| x.prerequisites.contains(&t.id))
            .map(|x| ("后续", x)),
    );
    if links.is_empty() {
        return;
    }
    section(ui, "关系", |_| {});
    ui.horizontal_wrapped(|ui| {
        for (kind, other) in links {
            let done = other.status == Status::Done;
            let text = format!("{kind} · {} · {}", other.name, other.status.as_str());
            let text = if done {
                egui::RichText::new(text).color(theme::success())
            } else {
                egui::RichText::new(text)
            };
            if ui
                .add(egui::Button::new(text).small())
                .on_hover_text("跳到这项任务")
                .clicked()
            {
                state.selected = Some(other.id.clone());
            }
        }
    });
}

/// 「更新进展」框预填当前进展。换了任务就重填；同一任务的进展被别处改了（编辑表单、
/// 采纳建议）而用户还没动过框里的字时也重填，免得点「更新」用旧文字覆盖新进展。
pub(super) fn sync_progress_box(state: &mut State, t: &Task) {
    match &state.progress_task {
        Some((id, base)) if id == &t.id => {
            if base != &t.progress {
                if state.progress_text == *base {
                    state.progress_text = t.progress.clone();
                }
                state.progress_task = Some((t.id.clone(), t.progress.clone()));
            }
        }
        _ => {
            state.progress_task = Some((t.id.clone(), t.progress.clone()));
            state.progress_text = t.progress.clone();
        }
    }
}

/// 当前进展：平时按正文显示，点「更新进展」就地变成编辑框，不再另起一个重复的框。
fn progress(ui: &mut egui::Ui, store: &mut Store, state: &mut State, t: &Task, readonly: bool) {
    let editing = !readonly && state.progress_editing.as_deref() == Some(t.id.as_str());
    section(ui, "当前进展", |ui| {
        if !readonly
            && !editing
            && ui
                .add(theme::icon_text_button(theme::Icon::PencilLine, "更新进展"))
                .on_hover_text("改写当前进展，旧进展留在历史里")
                .clicked()
        {
            state.progress_editing = Some(t.id.clone());
            state.progress_text = t.progress.clone();
            state.progress_task = Some((t.id.clone(), t.progress.clone()));
            state.progress_focus = true;
        }
    });
    if !editing {
        body(ui, &t.progress, "还没有记录进展。");
        return;
    }
    let response = ui.add(
        egui::TextEdit::multiline(&mut state.progress_text)
            .hint_text("如：已收到两家单位反馈，还差三家")
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    if std::mem::take(&mut state.progress_focus) {
        response.request_focus();
    }
    // 没改动就不更新，免得 Ctrl+Enter 连按在历史里堆出一串相同的进展。
    let changed = state.progress_text.trim() != t.progress;
    let (submit_key, cancel_key) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter) && i.modifiers.command,
            i.key_pressed(egui::Key::Escape),
        )
    });
    let focused = response.has_focus() || response.lost_focus();
    ui.horizontal(|ui| {
        let clicked =
            theme::primary_icon_button_enabled(ui, changed, theme::Icon::Check, "更新").clicked();
        let cancel = ui.button("取消").clicked();
        theme::caption(
            ui,
            &format!(
                "{} 更新 · Esc 取消 · 旧进展留在历史里",
                theme::primary_shortcut("Enter")
            ),
        );
        if clicked || (changed && submit_key && focused) {
            let text = state.progress_text.trim().to_owned();
            match store.record_progress(&t.id, &text) {
                Ok(()) => {
                    state.progress_text = text;
                    state.progress_editing = None;
                    state.ok("进展已更新，旧进展保留在历史里");
                }
                Err(e) => state.fail(format!("记录进展失败：{e:#}")),
            }
        } else if cancel || (cancel_key && focused) {
            state.progress_editing = None;
            state.progress_text = t.progress.clone();
        }
    });
}

/// 历史按时间线排：左列时间，右边动作与说明，扫一眼就能看出先后。
fn history(ui: &mut egui::Ui, store: &mut Store, state: &mut State, t: &Task) {
    let entries = match state.cache.history(store, &t.id) {
        Ok(entries) => entries
            .iter()
            .map(|e| {
                (
                    short_time(&e.at),
                    e.action.clone(),
                    e.detail.clone(),
                    e.source_id.clone(),
                )
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            theme::caption(ui, &e);
            return;
        }
    };
    let count = entries.len();
    section(ui, "历史", |ui| {
        if count > 0 {
            theme::caption(ui, &format!("{count} 条，新的在前"));
        }
    });
    if entries.is_empty() {
        theme::caption(ui, "还没有记录。");
        return;
    }
    for (index, (at, action, detail, source)) in entries.into_iter().enumerate() {
        ui.add_space(if index == 0 { 2.0 } else { 8.0 });
        ui.push_id(("task_history", index), |ui| {
            // 时间列与动作那一行同高、同样居中，两者落在一条基线上。
            let line = ui.spacing().interact_size.y;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(84.0, line),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_width(84.0);
                        ui.set_min_height(line);
                        theme::caption(ui, &at);
                    },
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.set_min_height(line);
                        ui.label(egui::RichText::new(&action).strong());
                        if let Some(source) = &source
                            && ui
                                .small_button("原文")
                                .on_hover_text("查看当时依据的材料快照")
                                .clicked()
                        {
                            match store.source(source) {
                                Ok(view) => state.source_view = Some(view),
                                Err(e) => state.fail(format!("读取来源失败：{e:#}")),
                            }
                        }
                    });
                    let detail = detail.trim();
                    if !detail.is_empty() {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(detail)
                                    .size(theme::font_sizes::SMALL)
                                    .color(theme::text_soft()),
                            )
                            .wrap(),
                        );
                    }
                });
            });
        });
    }
}
