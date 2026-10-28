//! 单任务草稿的文字填写、建议、出处与撤销；这里不写数据库。
use super::{Snapshot, State, View};
use crate::task_assistant::extract::{Fill, MAX_SOURCE_BYTES};
use crate::task_assistant::{Role, Task};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FillField {
    Name,
    Goal,
    NextStep,
    NextDeadline,
    Deadline,
    Collaborators,
    Role,
    Category,
    Parent,
}
impl FillField {
    pub fn value(self, t: &Task) -> String {
        match self {
            Self::Name => t.name.clone(),
            Self::Goal => t.goal.clone(),
            Self::NextStep => t.next_step.clone(),
            Self::NextDeadline => t.next_deadline.clone().unwrap_or_default(),
            Self::Deadline => t.deadline.clone().unwrap_or_default(),
            Self::Collaborators => t.collaborators.clone(),
            Self::Role => t.role.as_str().into(),
            Self::Category => t.category.clone(),
            Self::Parent => t.parent_id.clone().unwrap_or_default(),
        }
    }
    fn empty(self, t: &Task) -> bool {
        match self {
            Self::Role => t.role == Role::Responsible,
            Self::Category => t.category == "未分类",
            _ => self.value(t).trim().is_empty(),
        }
    }
    fn write(self, t: &mut Task, value: &str) {
        match self {
            Self::Name => t.name = value.into(),
            Self::Goal => t.goal = value.into(),
            Self::NextStep => t.next_step = value.into(),
            Self::NextDeadline => t.next_deadline = (!value.is_empty()).then(|| value.into()),
            Self::Deadline => t.deadline = (!value.is_empty()).then(|| value.into()),
            Self::Collaborators => t.collaborators = value.into(),
            Self::Category => t.category = value.into(),
            Self::Parent => t.parent_id = (!value.is_empty()).then(|| value.into()),
            Self::Role => {
                t.role = match value {
                    "配合" => Role::Collaborate,
                    "跟进" => Role::Follow,
                    "关注" => Role::Watch,
                    _ => Role::Responsible,
                }
            }
        }
    }
}

pub(crate) struct FillRequest {
    pub draft_id: String,
    pub text: String,
    pub current: Task,
}

pub(crate) fn prune_marks(state: &mut State, draft: &Task) {
    state
        .fill_marks
        .retain(|field, v| field.value(draft) == v.value);
}

/// 以填写框里的文字为原文应用结果（样张与测试用；后台结果走 `apply_from`）。
#[cfg(test)]
pub(crate) fn apply_fill(state: &mut State, draft: &mut Task, fill: Fill) {
    let source = state.fill_text.clone();
    apply_from(state, draft, &source, fill);
}

/// `source` 是发请求那一刻的原文：识别期间用户改了填写框，出处仍按当时的文字核对。
fn apply_from(state: &mut State, draft: &mut Task, source: &str, mut fill: Fill) {
    // 编辑已有任务时上下文里有它自己；把自己设成上级只会在保存时报循环。
    if fill.parent_id.as_ref().is_some_and(|v| v.value == draft.id) {
        fill.parent_id = None;
        fill.dropped += 1;
    }
    // 多次识别仍保留旧标记的出处，以原文合集保存；每段原文保持完整。
    let combined = match state.fill_source.as_deref() {
        Some(old) if old != source && !old.contains(source) => {
            format!("{old}\n\n--- 后续填写材料 ---\n\n{source}")
        }
        Some(old) => old.to_owned(),
        None => source.to_owned(),
    };
    if combined.len() > MAX_SOURCE_BYTES {
        state.fill_error = Some("多次填写的来源合计超过 1 MB，请先保存或撤销后分批整理。".into());
        return;
    }
    prune_marks(state, draft);
    state.fill_before.get_or_insert_with(|| draft.clone());
    state.fill_source = Some(combined);
    state.fill_summary = Some((fill.dropped, fill.others));
    for (field, value) in [
        (FillField::Name, fill.name),
        (FillField::Goal, fill.goal),
        (FillField::NextStep, fill.next_step),
        (FillField::NextDeadline, fill.next_deadline),
        (FillField::Deadline, fill.deadline),
        (FillField::Collaborators, fill.collaborators),
        (FillField::Role, fill.role),
        (FillField::Category, fill.category),
        (FillField::Parent, fill.parent_id),
    ] {
        let Some(v) = value else {
            continue;
        };
        if field.empty(draft) {
            field.write(draft, &v.value);
            state.fill_suggestions.remove(&field);
            state.fill_marks.insert(field, v);
        } else if field.value(draft) != v.value {
            state.fill_suggestions.insert(field, v);
        }
    }
    state.fill_open_more = state
        .fill_marks
        .keys()
        .chain(state.fill_suggestions.keys())
        .any(|f| {
            matches!(
                f,
                FillField::Category | FillField::Parent | FillField::Collaborators
            )
        });
}

pub(crate) fn undo_fill(state: &mut State, draft: &mut Task) {
    if let Some(before) = state.fill_before.take() {
        for (field, v) in &state.fill_marks {
            if field.value(draft) == v.value {
                field.write(draft, &field.value(&before));
            }
        }
    }
    state.fill_marks.clear();
    state.fill_suggestions.clear();
    state.fill_source = None;
    state.fill_summary = None;
}

/// 忙状态也属于请求身份；关闭再打开同一任务不应接收旧请求的结果。
pub(crate) fn fill_result_matches(state: &State, id: &str) -> bool {
    state.fill_busy && !state.fill_discard && state.draft.as_ref().is_some_and(|d| d.id == id)
}

pub(crate) fn receive_fill(
    state: &mut State,
    id: &str,
    source: &str,
    result: Result<Fill, String>,
) {
    if !fill_result_matches(state, id) {
        if state.draft.as_ref().is_some_and(|d| d.id == id) {
            state.fill_busy = false;
            state.fill_discard = false;
        }
        return;
    }
    state.fill_busy = false;
    match result {
        Ok(fill) => {
            state.fill_error = None;
            if let Some(mut draft) = state.draft.take() {
                apply_from(state, &mut draft, source, fill);
                state.draft = Some(draft);
            }
        }
        Err(e) => state.fill_error = Some(e),
    }
}

pub(super) fn show(ui: &mut egui::Ui, draft: &mut Task, state: &mut State) {
    if state.candidate_create_id.is_some() {
        return;
    }
    if draft.revision == 0 {
        card(ui, draft, state);
    } else {
        egui::CollapsingHeader::new("从文字补充…")
            .id_salt(("task_fill_open", &draft.id))
            .default_open(false)
            .show_unindented(ui, |ui| card(ui, draft, state));
    }
}

fn card(ui: &mut egui::Ui, draft: &mut Task, state: &mut State) {
    theme::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(theme::Icon::Sparkles.image_sized(16.0));
            ui.strong("从文字填写");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if state.fill_busy {
                    theme::spinner(ui, 14.0, theme::accent());
                    if ui.small_button("取消").clicked() {
                        state.fill_discard = true;
                    }
                } else if theme::primary_icon_button_enabled(
                    ui,
                    !state.fill_text.trim().is_empty(),
                    theme::Icon::Sparkles,
                    "识别并填入",
                )
                .clicked()
                {
                    state.request_fill = Some(FillRequest {
                        draft_id: draft.id.clone(),
                        text: state.fill_text.clone(),
                        current: draft.clone(),
                    });
                }
            });
        });
        let rows = state.fill_text.lines().count().clamp(3, 8);
        let input = ui.add(
            egui::TextEdit::multiline(&mut state.fill_text)
                .id_salt(("task_fill_text", &draft.id))
                .hint_text("粘贴通知、纪要片段或领导交办的话")
                .desired_rows(rows)
                .desired_width(ui.available_width()),
        );
        if std::mem::take(&mut state.fill_focus) {
            input.request_focus();
        }
        theme::caption(ui, "AI 只填空白栏；已有内容的栏给出建议，保存前都可改。");
        if state.fill_discard {
            theme::caption(ui, "已取消本次结果，等待请求结束。");
        }
        if let Some(e) = &state.fill_error {
            ui.colored_label(theme::warn(), e);
        }
        if let Some((dropped, others)) = state.fill_summary {
            ui.horizontal_wrapped(|ui| {
                theme::caption(
                    ui,
                    &format!(
                        "AI 填了 {} 栏，{} 栏有不同建议",
                        state.fill_marks.len(),
                        state.fill_suggestions.len()
                    ),
                );
                if ui.small_button("全部撤销").clicked() {
                    undo_fill(state, draft);
                }
            });
            if dropped > 0 {
                theme::caption(
                    ui,
                    &format!("另有 {dropped} 栏因无法核对出处或值不合规则已丢弃"),
                );
            }
            if others > 0 {
                ui.horizontal_wrapped(|ui| {
                    theme::caption(ui, &format!("这段文字里还有 {others} 件事"));
                    if ui.small_button("转到批量整理").clicked() {
                        state.source_text = state.fill_text.clone();
                        state.source_title.clear();
                        state.view = View::Inbox;
                    }
                });
            }
        }
    });
}

/// 一处统一绘制字段标签、AI 边框与建议，人工编辑后立即取消标记。
pub(super) fn field(
    ui: &mut egui::Ui,
    state: &mut State,
    draft: &mut Task,
    data: &Snapshot,
    field: FillField,
    label: &str,
    body: impl FnOnce(&mut egui::Ui, &mut Task),
) {
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        theme::caption(ui, label);
        if let Some(v) = state.fill_marks.get(&field) {
            theme::chip(ui, "AI", theme::accent(), theme::accent_soft())
                .on_hover_text(format!("原文：{}", v.evidence));
        }
    });
    let stroke = if state.fill_marks.contains_key(&field) {
        egui::Stroke::new(1.0, theme::accent())
    } else {
        egui::Stroke::NONE
    };
    egui::Frame::new()
        .stroke(stroke)
        .corner_radius(theme::chrome_radius(7))
        .inner_margin(2.0)
        .show(ui, |ui| body(ui, draft));
    prune_marks(state, draft);
    if let Some(v) = state.fill_suggestions.get(&field).cloned() {
        let readable = if field == FillField::Parent {
            data.task(&v.value)
                .map(|t| t.name.as_str())
                .unwrap_or("上下文任务已不存在")
                .to_owned()
        } else {
            v.value.clone()
        };
        ui.horizontal_wrapped(|ui| {
            theme::caption(ui, &format!("AI 建议改为：{readable}"));
            if ui
                .small_button("替换")
                .on_hover_text(format!("原文：{}", v.evidence))
                .clicked()
            {
                field.write(draft, &v.value);
                state.fill_suggestions.remove(&field);
                state.fill_marks.insert(field, v);
            }
            if ui.small_button("忽略").clicked() {
                state.fill_suggestions.remove(&field);
            }
        });
    }
}
