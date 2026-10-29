//! 单任务草稿的文字填写、建议、出处与撤销；这里不写数据库。
use super::{Snapshot, State, View};
use crate::task_assistant::Task;
use crate::task_assistant::extract::{Fill, MAX_SOURCE_BYTES};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FillField {
    Name,
    Owner,
    Progress,
    NextStep,
    Deadline,
    Risks,
}
impl FillField {
    pub fn value(self, t: &Task) -> String {
        match self {
            Self::Name => t.name.clone(),
            Self::Owner => t.owner_name.clone(),
            Self::Progress => t.progress.clone(),
            Self::NextStep => t.next_step.clone(),
            Self::Deadline => t.deadline.clone().unwrap_or_default(),
            Self::Risks => t.risks.clone(),
        }
    }
    fn empty(self, t: &Task) -> bool {
        self.value(t).trim().is_empty()
    }
    fn write(self, t: &mut Task, value: &str) {
        match self {
            Self::Name => t.name = value.into(),
            Self::Owner => t.owner_name = value.into(),
            Self::Progress => t.progress = value.into(),
            Self::NextStep => t.next_step = value.into(),
            Self::Deadline => t.deadline = (!value.is_empty()).then(|| value.into()),
            Self::Risks => t.risks = value.into(),
        }
    }
}

pub(crate) struct FillRequest {
    pub draft_id: String,
    pub text: String,
    pub current: Task,
}

pub(crate) fn prune_marks(state: &mut State, draft: &Task) {
    let owner_id = state.fill_owner_id;
    state.fill_marks.retain(|field, v| {
        if *field == FillField::Owner {
            owner_id.is_some() && draft.owner_id == owner_id && draft.owner_name == v.value
        } else {
            field.value(draft) == v.value
        }
    });
}

/// 以填写框里的文字为原文应用结果（样张与测试用；后台结果走 `apply_from`）。
#[cfg(test)]
pub(crate) fn apply_fill(
    state: &mut State,
    draft: &mut Task,
    fill: Fill,
    vocab: &[crate::models::VocabularyEntry],
) {
    let source = state.fill_text.clone();
    apply_from(state, draft, &source, fill, vocab);
}

/// `source` 是发请求那一刻的原文：识别期间用户改了填写框，出处仍按当时的文字核对。
fn apply_from(
    state: &mut State,
    draft: &mut Task,
    source: &str,
    fill: Fill,
    vocab: &[crate::models::VocabularyEntry],
) {
    state.fill_owner_note = None;
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
        (FillField::Owner, fill.owner),
        (FillField::Progress, fill.progress),
        (FillField::NextStep, fill.next_step),
        (FillField::Deadline, fill.deadline),
        (FillField::Risks, fill.risks),
    ] {
        let Some(v) = value else {
            continue;
        };
        if field == FillField::Owner {
            let matches = super::owner::match_name(vocab, &v.value);
            if matches.len() == 1 {
                let person = matches[0];
                let canonical = person.canonical.clone();
                if draft.owner_id.is_none() && draft.owner_name.is_empty() {
                    draft.owner_id = Some(person.id);
                    draft.owner_name = canonical.clone();
                    state.fill_owner_id = Some(person.id);
                    state.fill_suggestions.remove(&field);
                    state.fill_owner_suggestion_id = None;
                    state.fill_marks.insert(
                        field,
                        crate::task_assistant::extract::FilledValue {
                            value: canonical,
                            evidence: v.evidence,
                        },
                    );
                } else if draft.owner_id != Some(person.id) {
                    state.fill_owner_suggestion_id = Some(person.id);
                    state.fill_suggestions.insert(
                        field,
                        crate::task_assistant::extract::FilledValue {
                            value: canonical,
                            evidence: v.evidence,
                        },
                    );
                } else {
                    state.fill_suggestions.remove(&field);
                    state.fill_owner_suggestion_id = None;
                }
            } else if matches.len() > 1 {
                state.owner_query = v.value.clone();
                state.fill_owner_note = Some(format!(
                    "原文提到『{}』，词库里有 {} 位同名人员，请在负责人里选。",
                    v.value,
                    matches.len()
                ));
                state
                    .fill_summary
                    .get_or_insert((fill.dropped, fill.others));
            } else {
                state.fill_owner_note = Some(format!(
                    "原文提到『{}』，标准词库里没有此人，未填。",
                    v.value
                ));
            }
            continue;
        }
        if field.empty(draft) {
            field.write(draft, &v.value);
            state.fill_suggestions.remove(&field);
            state.fill_marks.insert(field, v);
        } else if field.value(draft) != v.value {
            state.fill_suggestions.insert(field, v);
        }
    }
    state.fill_open_more = false;
}

pub(crate) fn undo_fill(state: &mut State, draft: &mut Task) {
    if let Some(before) = state.fill_before.take() {
        for (field, v) in &state.fill_marks {
            if (*field == FillField::Owner
                && state.fill_owner_id == draft.owner_id
                && draft.owner_name == v.value)
                || (*field != FillField::Owner && field.value(draft) == v.value)
            {
                field.write(draft, &field.value(&before));
                if *field == FillField::Owner {
                    draft.owner_id = before.owner_id;
                }
            }
        }
    }
    state.fill_marks.clear();
    state.fill_suggestions.clear();
    state.fill_source = None;
    state.fill_summary = None;
    state.fill_owner_id = None;
    state.fill_owner_suggestion_id = None;
    state.fill_owner_note = None;
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
    vocab: &[crate::models::VocabularyEntry],
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
                apply_from(state, &mut draft, source, fill, vocab);
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
        if let Some(note) = &state.fill_owner_note {
            ui.colored_label(theme::warn(), note);
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
                theme::caption(ui, &format!("另有 {dropped} 栏没能核对出处，已略过"));
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
        let readable = if field == FillField::Owner {
            super::owner::match_name(data.vocabulary, &v.value)
                .first()
                .map(|e| super::owner::label(data.vocabulary, Some(e.id), &e.canonical).0)
                .unwrap_or(v.value.clone())
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

pub(super) fn owner_field(ui: &mut egui::Ui, draft: &mut Task, state: &mut State, data: &Snapshot) {
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        theme::caption(ui, "负责人");
        if let Some(v) = state.fill_marks.get(&FillField::Owner) {
            theme::chip(ui, "AI", theme::accent(), theme::accent_soft())
                .on_hover_text(format!("原文：{}", v.evidence));
        }
    });
    let stroke = if state.fill_marks.contains_key(&FillField::Owner) {
        egui::Stroke::new(1.0, theme::accent())
    } else {
        egui::Stroke::NONE
    };
    let changed = egui::Frame::new()
        .stroke(stroke)
        .corner_radius(theme::chrome_radius(7))
        .inner_margin(2.0)
        .show(ui, |ui| {
            super::owner::picker(
                ui,
                ("task_owner", &draft.id),
                data.vocabulary,
                &mut draft.owner_id,
                &mut draft.owner_name,
                &mut state.owner_query,
            )
        })
        .inner;
    if changed {
        state.fill_marks.remove(&FillField::Owner);
        state.fill_suggestions.remove(&FillField::Owner);
        state.fill_owner_id = None;
        state.fill_owner_suggestion_id = None;
    }
    if let Some(v) = state.fill_suggestions.get(&FillField::Owner).cloned() {
        let proposed = state.fill_owner_suggestion_id.and_then(|id| {
            data.vocabulary
                .iter()
                .find(|e| e.id == id && e.category == crate::models::VocabularyCategory::Person)
        });
        let shown = proposed
            .map(|person| {
                super::owner::label(data.vocabulary, Some(person.id), &person.canonical).0
            })
            .unwrap_or_else(|| v.value.clone());
        ui.horizontal_wrapped(|ui| {
            theme::caption(ui, &format!("AI 建议负责人：{shown}"));
            if ui
                .add_enabled(proposed.is_some(), egui::Button::new("替换").small())
                .clicked()
                && let Some(person) = proposed
            {
                draft.owner_id = Some(person.id);
                draft.owner_name = person.canonical.clone();
                state.fill_suggestions.remove(&FillField::Owner);
                state.fill_marks.insert(FillField::Owner, v.clone());
                state.fill_owner_id = Some(person.id);
                state.fill_owner_suggestion_id = None;
            }
            if ui.small_button("忽略").clicked() {
                state.fill_suggestions.remove(&FillField::Owner);
            }
        });
    }
}
