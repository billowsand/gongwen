//! 新建 / 编辑任务表单：字段分三组，保存与放弃固定在底栏，表单内容自己滚动。
use super::{FillField, Snapshot, State, fill};
use crate::task_assistant::{Status, Store, Task};
use crate::theme;

/// 前置任务搜索一次最多列出的条数。
const PICK_LIMIT: usize = 8;

/// 返回 false 表示表单已关闭（保存成功或放弃）。
pub(super) fn show(
    ui: &mut egui::Ui,
    d: &mut Task,
    store: &mut Store,
    state: &mut State,
    data: &Snapshot,
    height: f32,
) -> bool {
    let mut keep = true;
    fill::prune_marks(state, d);
    ui.horizontal(|ui| {
        ui.heading(if d.revision == 0 {
            "新建任务"
        } else {
            "编辑任务"
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::caption(ui, "未保存");
        });
    });
    if state.candidate_create_id.is_some() {
        theme::caption(ui, "来自材料建议；保存后该建议记为已采纳。");
    }
    ui.add_space(4.0);
    let footer = if state.form_error.is_some() {
        84.0
    } else {
        46.0
    };
    let body = (ui.available_height().min(height) - footer).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("task_form")
        .auto_shrink([false, false])
        .max_height(body)
        .show(ui, |ui| fields(ui, d, state, data));
    theme::hairline(ui);
    ui.add_space(4.0);
    if let Some(e) = &state.form_error {
        theme::notice(
            ui,
            theme::Icon::TriangleAlert,
            theme::danger(),
            theme::danger_soft(),
            e.clone(),
        );
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::Save, "保存").clicked() {
            let detail = if d.status == Status::Cancelled {
                state.cancel_reason.trim().to_owned()
            } else {
                "用户保存".to_owned()
            };
            let result = if let Some(candidate_id) = state.candidate_create_id.as_deref() {
                store.create_from_candidate(candidate_id, d)
            } else if let Some(src) = state
                .fill_source
                .as_deref()
                .filter(|_| !state.fill_marks.is_empty())
            {
                let title = crate::task_assistant::extract::auto_title(src, chrono::Local::now());
                store.save_with_source(
                    d,
                    if d.revision == 0 { "创建" } else { "编辑" },
                    &detail,
                    &title,
                    src,
                )
            } else {
                store.save(d, if d.revision == 0 { "创建" } else { "编辑" }, &detail)
            };
            match result {
                Ok(()) => {
                    state.selected = Some(d.id.clone());
                    state.close_draft();
                    state.ok("任务已保存");
                    keep = false;
                }
                Err(e) => state.form_error = Some(format!("没有保存：{e:#}")),
            }
        }
        if ui
            .add(theme::secondary_icon_button(theme::Icon::X, "放弃"))
            .clicked()
        {
            state.close_draft();
            keep = false;
        }
    });
    keep
}

fn group(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(title)
            .strong()
            .color(theme::text_soft()),
    );
}

fn label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    theme::caption(ui, text);
}

fn fields(ui: &mut egui::Ui, d: &mut Task, state: &mut State, data: &Snapshot) {
    let full = ui.available_width();
    fill::show(ui, d, state);
    group(ui, "要做什么");
    fill::field(
        ui,
        state,
        d,
        data,
        FillField::Name,
        "任务名称（必填）",
        |ui, d| {
            ui.add(theme::field(
                &mut d.name,
                "如：牵头起草年度工作总结",
                ui.available_width(),
            ));
        },
    );
    fill::owner_field(ui, d, state, data);
    group(ui, "进展与安排");
    fill::field(
        ui,
        state,
        d,
        data,
        FillField::Progress,
        "当前进展",
        |ui, d| {
            ui.add(
                egui::TextEdit::multiline(&mut d.progress)
                    .hint_text("目前进展，有啥填啥")
                    .desired_rows(3)
                    .desired_width(ui.available_width()),
            );
        },
    );
    fill::field(
        ui,
        state,
        d,
        data,
        FillField::NextStep,
        "下一步工作",
        |ui, d| {
            ui.add(
                egui::TextEdit::multiline(&mut d.next_step)
                    .hint_text("接下来要做什么")
                    .desired_rows(2)
                    .desired_width(ui.available_width()),
            );
        },
    );
    fill::field(
        ui,
        state,
        d,
        data,
        FillField::Risks,
        "风险问题及应对措施",
        |ui, d| {
            ui.add(
                egui::TextEdit::multiline(&mut d.risks)
                    .hint_text("风险以及应对措施")
                    .desired_rows(3)
                    .desired_width(ui.available_width()),
            );
        },
    );
    ui.columns(2, |cols| {
        label(&mut cols[0], "状态");
        egui::ComboBox::from_id_salt("task_edit_status")
            .selected_text(d.status.as_str())
            .show_ui(&mut cols[0], |ui| {
                for s in [
                    Status::Todo,
                    Status::InProgress,
                    Status::Waiting,
                    Status::Blocked,
                    Status::Done,
                    Status::Cancelled,
                ] {
                    ui.selectable_value(&mut d.status, s, s.as_str());
                }
            });
        fill::field(
            &mut cols[1],
            state,
            d,
            data,
            FillField::Deadline,
            "任务期限",
            |ui, d| {
                super::date::field(ui, "deadline", &mut d.deadline, ui.available_width());
            },
        );
    });
    if let Some(parent) = d.parent_id.as_deref().and_then(|id| data.task(id))
        && d.deadline
            .as_ref()
            .zip(parent.deadline.as_ref())
            .is_some_and(|(a, b)| a > b)
    {
        hint(ui, "本任务期限晚于上级任务期限，请核对。");
    }
    if d.status == Status::Cancelled {
        label(ui, "取消理由（必填）");
        ui.add(theme::field(
            &mut state.cancel_reason,
            "如：上级通知暂缓",
            full,
        ));
    }
    ui.add_space(4.0);
    ui.checkbox(&mut d.needs_review, "标记为待核对");
    let has_more = d.category != "未分类" || d.parent_id.is_some() || !d.prerequisites.is_empty();
    ui.add_space(4.0);
    let open_more = std::mem::take(&mut state.fill_open_more).then_some(true);
    egui::CollapsingHeader::new(
        egui::RichText::new("更多设置")
            .strong()
            .color(theme::text_soft()),
    )
    .id_salt(("task_form_more", &d.id))
    .default_open(has_more)
    .open(open_more)
    .show_unindented(ui, |ui| more(ui, d, state, data));
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(theme::font_sizes::SMALL)
            .color(theme::warn()),
    );
}

fn more(ui: &mut egui::Ui, d: &mut Task, state: &mut State, data: &Snapshot) {
    label(ui, "分类");
    egui::ComboBox::from_id_salt("task_edit_cat")
        .selected_text(&d.category)
        .show_ui(ui, |ui| {
            for c in &data.categories {
                ui.selectable_value(&mut d.category, c.clone(), c);
            }
        });
    label(ui, "上级任务");
    egui::ComboBox::from_id_salt("task_parent")
        .selected_text(
            d.parent_id
                .as_deref()
                .and_then(|id| data.task(id))
                .map(|t| t.name.as_str())
                .unwrap_or("无上级"),
        )
        .width(ui.available_width() - 8.0)
        .truncate()
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut d.parent_id, None, "无上级");
            for t in data
                .tasks
                .iter()
                .filter(|t| t.id != d.id && !t.deleted && !t.archived)
            {
                ui.selectable_value(&mut d.parent_id, Some(t.id.clone()), &t.name);
            }
        });
    label(
        ui,
        "前置任务（全部完成并核对后，本任务才能进入进行中或完成）",
    );
    prerequisites(ui, d, state, data, ui.available_width());
}

/// 已选前置以可删除标签显示，下面一个搜索框按名称添加。
fn prerequisites(ui: &mut egui::Ui, d: &mut Task, state: &mut State, data: &Snapshot, width: f32) {
    if !d.prerequisites.is_empty() {
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for id in &d.prerequisites {
                let (name, missing) = match data.task(id) {
                    Some(t) => (format!("{} · {}", t.name, t.status.as_str()), t.deleted),
                    None => ("已不存在的任务".to_owned(), true),
                };
                if ui
                    .add(theme::removable_tag_button(&name, missing))
                    .on_hover_text("移除这个前置")
                    .clicked()
                {
                    remove = Some(id.clone());
                }
            }
        });
        if let Some(id) = remove {
            d.prerequisites.retain(|p| p != &id);
        }
    }
    ui.add(theme::field(
        &mut state.prerequisite_query,
        "搜索任务名添加前置",
        width,
    ));
    let query = state.prerequisite_query.trim().to_lowercase();
    if query.is_empty() {
        return;
    }
    let matches = data
        .tasks
        .iter()
        .filter(|t| {
            t.id != d.id
                && !t.deleted
                && !t.archived
                && !d.prerequisites.contains(&t.id)
                && t.name.to_lowercase().contains(&query)
        })
        .take(PICK_LIMIT)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        theme::caption(ui, "没有匹配的任务。");
        return;
    }
    for t in matches {
        if ui
            .add(
                egui::Button::new(format!("＋ {} · {}", t.name, t.status.as_str()))
                    .frame_when_inactive(false),
            )
            .clicked()
        {
            d.prerequisites.push(t.id.clone());
            state.prerequisite_query.clear();
        }
    }
}
