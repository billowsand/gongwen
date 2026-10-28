//! 「待整理建议」视图：材料输入与候选卡，取代原来折叠在页尾的「材料整理」。
//!
//! 候选的识别、采纳与过期保护都在 `task_assistant::Store`；这里只负责摆放。
//! 选了目标任务的候选会把右栏切到该任务，采纳前能对照它的当前内容。
use super::{Replace, Snapshot, State};
use crate::task_assistant::Store;
use crate::task_assistant::extract::Proposal;
use crate::theme;

const FIELDS: [(&str, &str); 8] = [
    ("name", "建议名称"),
    ("goal", "建议完成条件"),
    ("next_step", "建议下一步"),
    ("deadline", "建议期限"),
    ("collaborators", "建议协作分工"),
    ("speaker", "意见提出主体"),
    ("about", "意见针对事项"),
    ("note", "意见 / 待决内容"),
];

/// 各类候选默认显示的字段；其它字段有值时也照样显示，不藏模型给的内容。
fn relevant(kind: &str) -> &'static [&'static str] {
    match kind {
        "new_task" => &["name", "goal", "next_step", "deadline", "collaborators"],
        "progress" => &["next_step", "note"],
        "deadline" => &["deadline"],
        "responsibility" => &["collaborators"],
        "scope" => &["name", "goal"],
        "completion_evidence" => &["note"],
        "opinion" | "decision" => &["speaker", "about", "note"],
        _ => &[
            "name",
            "goal",
            "next_step",
            "deadline",
            "collaborators",
            "note",
        ],
    }
}

fn slot<'a>(p: &'a mut Proposal, key: &str) -> &'a mut Option<String> {
    match key {
        "name" => &mut p.name,
        "goal" => &mut p.goal,
        "next_step" => &mut p.next_step,
        "deadline" => &mut p.deadline,
        "collaborators" => &mut p.collaborators,
        "speaker" => &mut p.speaker,
        "about" => &mut p.about,
        _ => &mut p.note,
    }
}

pub(super) fn kind_label(kind: &str) -> &str {
    match kind {
        "new_task" => "新增任务",
        "progress" => "进展反馈",
        "decision" => "待决事项",
        "opinion" => "相关意见",
        "responsibility" => "分工调整",
        "deadline" => "期限调整",
        "scope" => "范围调整",
        "completion_evidence" => "完成证据",
        _ => "待核变化",
    }
}

pub(super) fn show(
    ui: &mut egui::Ui,
    store: &mut Store,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
    data: &Snapshot,
) {
    material(ui, knowledge, state);
    ui.add_space(10.0);
    let rows = data
        .candidates
        .iter()
        .filter(|c| state.history_candidates != super::nav::pending_candidate(&c.4))
        .collect::<Vec<_>>();
    theme::caption(
        ui,
        &if state.history_candidates {
            format!(
                "已处理的建议 · {} 条（在「更多」菜单里切回待核对）",
                rows.len()
            )
        } else {
            format!("待核对的建议 · {} 条", rows.len())
        },
    );
    if rows.is_empty() {
        ui.add_space(6.0);
        theme::caption(
            ui,
            if state.history_candidates {
                "还没有已采纳或已忽略的建议。"
            } else {
                "暂无待核对的建议。粘贴材料识别后，结果会出现在这里。"
            },
        );
        return;
    }
    for (id, kind, payload, evidence, candidate_state) in rows {
        ui.add_space(6.0);
        ui.push_id(("task_candidate", id), |ui| {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                candidate(
                    ui,
                    store,
                    state,
                    data,
                    id,
                    kind,
                    payload,
                    evidence,
                    candidate_state,
                );
            });
        });
    }
}

fn material(
    ui: &mut egui::Ui,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new("粘贴材料").strong());
        theme::caption(
            ui,
            "通知、纪要、周报都可以。模型只提出建议，不会直接改任务，每条建议都要人核对后采纳。",
        );
        ui.add_space(4.0);
        ui.add(theme::field(
            &mut state.source_title,
            "材料标题，如：10月28日调度会纪要",
            ui.available_width(),
        ));
        ui.add_space(4.0);
        let text = ui.add(
            egui::TextEdit::multiline(&mut state.source_text)
                .hint_text("粘贴材料原文")
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut state.focus_material) {
            text.request_focus();
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if state.extraction_busy {
                theme::spinner(ui, 14.0, theme::accent());
                theme::caption(ui, "正在后台识别，完成前数据库恢复会保持禁用。");
                if !state.discard_extraction {
                    if ui.button("取消本次结果").clicked() {
                        state.discard_extraction = true;
                    }
                } else {
                    theme::caption(ui, "已取消，等当前请求结束后释放。");
                }
            } else if theme::primary_icon_button_enabled(
                ui,
                !state.source_text.trim().is_empty(),
                theme::Icon::Sparkles,
                "识别",
            )
            .clicked()
            {
                state.request_extract = Some((
                    state.source_title.clone(),
                    state.source_text.clone(),
                    "pasted".into(),
                ));
            }
        });
        if let Some(e) = state.extraction_error.clone() {
            ui.add_space(4.0);
            if super::banner(
                ui,
                theme::Icon::TriangleAlert,
                theme::warn(),
                theme::warn_soft(),
                &e,
                "关闭",
            ) {
                state.extraction_error = None;
            }
        }
        ui.add_space(6.0);
        theme::hairline(ui);
        ui.add_space(6.0);
        knowledge_picker(ui, knowledge, state);
    });
}

fn knowledge_picker(
    ui: &mut egui::Ui,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
) {
    let Some(knowledge) = knowledge else {
        theme::caption(ui, "知识库暂不可用。");
        return;
    };
    if state.knowledge_docs.is_none() {
        match knowledge.list_docs(None) {
            Ok(docs) => {
                state.knowledge_docs = Some(docs.into_iter().map(|d| (d.id, d.title)).collect());
            }
            Err(e) => {
                state.knowledge_docs = Some(Vec::new());
                state.extraction_error = Some(format!("读取知识库列表失败：{e:#}"));
            }
        }
    }
    let docs = state.knowledge_docs.take().unwrap_or_default();
    let mut refresh = false;
    ui.horizontal_wrapped(|ui| {
        theme::caption(ui, "或整理知识库里的一篇");
        egui::ComboBox::from_id_salt("task_knowledge_source")
            .selected_text(
                state
                    .knowledge_id
                    .and_then(|id| docs.iter().find(|d| d.0 == id).map(|d| d.1.as_str()))
                    .unwrap_or("选择资料"),
            )
            .width(200.0)
            .truncate()
            .show_ui(ui, |ui| {
                for (id, title) in &docs {
                    ui.selectable_value(&mut state.knowledge_id, Some(*id), title);
                }
            });
        if ui
            .add_enabled(
                !state.extraction_busy && state.knowledge_id.is_some(),
                egui::Button::new("整理这篇"),
            )
            .clicked()
            && let Some(id) = state.knowledge_id
        {
            state.request_knowledge = Some(id);
        }
        refresh = theme::icon_button(ui, theme::Icon::Refresh, "刷新知识库列表").clicked();
    });
    // 列表只在进入本视图或点刷新时读一次，不随每帧重绘查库。
    state.knowledge_docs = (!refresh).then_some(docs);
}

#[allow(clippy::too_many_arguments)]
fn candidate(
    ui: &mut egui::Ui,
    store: &mut Store,
    state: &mut State,
    data: &Snapshot,
    id: &str,
    kind: &str,
    payload: &str,
    evidence: &str,
    candidate_state: &str,
) {
    ui.horizontal(|ui| {
        theme::chip(ui, kind_label(kind), theme::accent(), theme::accent_soft());
        let state_label = match candidate_state {
            "accepted" => "已采纳",
            "ignored" => "已忽略",
            "later" => "稍后处理",
            _ => "待核对",
        };
        theme::caption(ui, state_label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("原文")
                .on_hover_text("查看这条建议来自的材料快照")
                .clicked()
            {
                match store.candidate_source(id) {
                    Ok(view) => state.source_view = Some(view),
                    Err(e) => state.fail(format!("读取来源失败：{e:#}")),
                }
            }
        });
    });
    quote(ui, evidence);
    if candidate_state == "accepted" {
        return;
    }
    let original = match serde_json::from_str::<Proposal>(payload) {
        Ok(p) => p,
        Err(e) => {
            theme::caption(ui, &format!("这条建议的数据无法读取：{e}"));
            return;
        }
    };
    let mut proposal = state
        .candidate_edits
        .remove(id)
        .unwrap_or_else(|| original.clone());
    ui.add_space(4.0);
    let wanted = relevant(kind);
    for (key, label) in FIELDS {
        let value = slot(&mut proposal, key);
        if !wanted.contains(&key) && value.is_none() {
            continue;
        }
        optional_line(ui, label, value);
    }
    ui.add_space(4.0);
    if kind == "new_task" {
        actions_new(ui, store, state, id, candidate_state, &proposal);
    } else {
        actions_existing(ui, store, state, data, id, candidate_state, &proposal);
    }
    state.candidate_edits.insert(id.to_owned(), proposal);
}

fn quote(ui: &mut egui::Ui, evidence: &str) {
    let response = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 10,
            right: 4,
            top: 2,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(evidence)
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_soft()),
                )
                .wrap(),
            );
        })
        .response;
    let bar = egui::Rect::from_min_size(response.rect.min, egui::vec2(2.0, response.rect.height()));
    ui.painter().rect_filled(bar, 0.0, theme::border_strong());
}

fn actions_new(
    ui: &mut egui::Ui,
    store: &mut Store,
    state: &mut State,
    id: &str,
    candidate_state: &str,
    p: &Proposal,
) {
    ui.horizontal_wrapped(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::Edit, "编辑为新任务").clicked() {
            let mut draft = crate::task_assistant::extract::proposal_task(p);
            draft.needs_review = false;
            state.open_draft(Replace::Candidate(id.to_owned(), Box::new(draft)));
        }
        secondary_actions(ui, store, state, id, candidate_state, p);
    });
}

#[allow(clippy::too_many_arguments)]
fn actions_existing(
    ui: &mut egui::Ui,
    store: &mut Store,
    state: &mut State,
    data: &Snapshot,
    id: &str,
    candidate_state: &str,
    p: &Proposal,
) {
    let mut target = state
        .candidate_targets
        .get(id)
        .cloned()
        .unwrap_or_else(|| p.task_id.clone().unwrap_or_default());
    let previous = target.clone();
    ui.horizontal_wrapped(|ui| {
        theme::caption(ui, "目标任务");
        egui::ComboBox::from_id_salt(("task_candidate_target", id))
            .selected_text(
                data.task(&target)
                    .map(|t| t.name.as_str())
                    .unwrap_or("请选择目标任务"),
            )
            .width(220.0)
            .truncate()
            .show_ui(ui, |ui| {
                for t in data.tasks.iter().filter(|t| !t.deleted && !t.archived) {
                    ui.selectable_value(&mut target, t.id.clone(), &t.name);
                }
            });
        if !target.is_empty()
            && ui
                .small_button("在右栏查看")
                .on_hover_text("对照目标任务的当前内容")
                .clicked()
        {
            state.selected = Some(target.clone());
        }
    });
    if previous != target && !target.is_empty() {
        // 人选了目标就按该任务的当前版本核对，右栏随之切过去便于对照。
        state.selected = Some(target.clone());
        state.expired_candidates.remove(id);
        if let Err(e) = store.recheck_candidate(id, &target) {
            state.fail(format!("绑定目标失败：{e:#}"));
        }
    }
    state
        .candidate_targets
        .insert(id.to_owned(), target.clone());
    ui.horizontal_wrapped(|ui| {
        if !target.is_empty() {
            let label = match p.kind.as_str() {
                "completion_evidence" => "确认任务完成",
                "opinion" | "decision" => "记入任务历史",
                _ => "采纳到目标任务",
            };
            if theme::primary_icon_button(ui, theme::Icon::Check, label).clicked() {
                match store
                    .edit_candidate(id, p)
                    .and_then(|_| store.apply_candidate(id, &target))
                {
                    Ok(()) => {
                        state.expired_candidates.remove(id);
                        state.ok("建议已采纳并记入来源历史");
                    }
                    Err(e) => {
                        let text = format!("{e:#}");
                        if text.contains("过期") || text.contains("已变化") {
                            state.expired_candidates.insert(id.to_owned());
                        }
                        state.fail(format!("无法采纳：{text}"));
                    }
                }
            }
        }
        secondary_actions(ui, store, state, id, candidate_state, p);
    });
    if state.expired_candidates.contains(id) && !target.is_empty() {
        ui.add_space(4.0);
        if super::banner(
            ui,
            theme::Icon::RotateCcw,
            theme::warn(),
            theme::warn_soft(),
            "目标任务在识别后被改过。对照右栏的当前内容确认建议仍然成立，再重新核对后采纳。",
            "以当前版本重新核对",
        ) {
            match store.recheck_candidate(id, &target) {
                Ok(()) => {
                    state.expired_candidates.remove(id);
                    state.selected = Some(target.clone());
                    state.ok("已绑定目标任务的当前版本，请再看一遍建议后采纳");
                }
                Err(e) => state.fail(format!("重新核对失败：{e:#}")),
            }
        }
    }
}

fn secondary_actions(
    ui: &mut egui::Ui,
    store: &mut Store,
    state: &mut State,
    id: &str,
    candidate_state: &str,
    p: &Proposal,
) {
    if candidate_state == "pending"
        && ui.button("稍后").clicked()
        && let Err(e) = store.candidate_done(id, "later")
    {
        state.fail(format!("延期失败：{e:#}"));
    }
    if candidate_state != "pending"
        && ui.button("恢复到待核对").clicked()
        && let Err(e) = store.candidate_done(id, "pending")
    {
        state.fail(format!("恢复失败：{e:#}"));
    }
    if candidate_state != "ignored"
        && ui.button("忽略").clicked()
        && let Err(e) = store.candidate_done(id, "ignored")
    {
        state.fail(format!("忽略失败：{e:#}"));
    }
    if ui
        .button("保存修改")
        .on_hover_text("只保存对建议的修改，任务暂不变化")
        .clicked()
    {
        match store.edit_candidate(id, p) {
            Ok(()) => state.ok("建议的修改已保存，任务尚未变化"),
            Err(e) => state.fail(format!("保存建议修改失败：{e:#}")),
        }
    }
}

fn optional_line(ui: &mut egui::Ui, label: &str, value: &mut Option<String>) {
    let mut text = value.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(96.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_width(96.0);
                theme::caption(ui, label);
            },
        );
        if ui
            .add(theme::field(&mut text, "（无）", ui.available_width()))
            .changed()
        {
            *value = (!text.trim().is_empty()).then_some(text);
        }
    });
}
