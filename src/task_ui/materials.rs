//! 「材料」视图：识别过的材料都存在这里，可以看原文、看它产生的建议、重新识别和删除。
//!
//! 材料就是 `task_sources` 里批量整理存下的快照（表单「从文字填写」的快照挂在任务
//! 历史上，不列）。重新识别按快照原文再调模型，只替换还没处理的建议；删除时已被任务
//! 历史引用的快照保留、只从列表消失，历史里的「原文」照样能看。
use super::{Snapshot, State, View};
use crate::task_assistant::{SourceRow, Store};
use crate::theme;

/// 「待核对 2 · 已采纳 1 · 已忽略 0」，没有任何建议时说清楚。
pub(super) fn counts(s: &SourceRow) -> String {
    if s.pending + s.accepted + s.ignored == 0 {
        return "没有建议".into();
    }
    format!(
        "待核对 {} · 已采纳 {} · 已忽略 {}",
        s.pending, s.accepted, s.ignored
    )
}

fn origin(source_ref: &str) -> &'static str {
    if source_ref == "pasted" {
        "粘贴"
    } else if source_ref.starts_with("knowledge:") {
        "知识库"
    } else {
        "其他"
    }
}

pub(super) fn list(ui: &mut egui::Ui, state: &mut State, data: &Snapshot) {
    let query = state.query.trim().to_owned();
    let rows = data
        .sources
        .iter()
        .filter(|s| query.is_empty() || s.title.contains(&query))
        .collect::<Vec<_>>();
    theme::caption(
        ui,
        &format!(
            "识别过的材料 · {} 份。选中一份可看原文与它产生的建议，也能重新识别或删除。",
            rows.len()
        ),
    );
    if rows.is_empty() {
        ui.add_space(6.0);
        theme::caption(
            ui,
            if query.is_empty() {
                "还没有识别过材料。在「待整理建议」粘贴材料并识别后，材料会存在这里。"
            } else {
                "没有标题匹配搜索的材料。"
            },
        );
        return;
    }
    for source in rows {
        ui.add_space(4.0);
        let selected = state.material_selected.as_deref() == Some(source.id.as_str());
        let response = theme::card()
            .fill(if selected {
                theme::accent_soft()
            } else {
                theme::surface()
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(egui::RichText::new(&source.title).strong()).truncate());
                theme::caption(
                    ui,
                    &format!(
                        "{} · {} · {}",
                        super::short_time(&source.created_at),
                        origin(&source.source_ref),
                        counts(source)
                    ),
                );
            })
            .response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                true,
                selected,
                &source.title,
            )
        });
        if response.clicked() {
            state.material_selected = Some(source.id.clone());
        }
    }
}

pub(super) fn detail(ui: &mut egui::Ui, store: &mut Store, state: &mut State, data: &Snapshot) {
    let Some(source) = state
        .material_selected
        .as_deref()
        .and_then(|id| data.sources.iter().find(|s| s.id == id))
    else {
        ui.add_space(24.0);
        theme::caption(ui, "在左边选一份材料，查看原文和它产生的建议。");
        return;
    };
    ui.heading(&source.title);
    theme::caption(
        ui,
        &format!(
            "{} 识别 · 来源：{} · {}",
            super::short_time(&source.created_at),
            origin(&source.source_ref),
            counts(source)
        ),
    );
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if state.extraction_busy {
            theme::spinner(ui, 14.0, theme::accent());
            theme::caption(ui, "正在识别，完成后建议会更新。");
        } else if theme::primary_icon_button(ui, theme::Icon::Refresh, "重新识别")
            .on_hover_text("按存下的原文再调一次模型，替换还没处理的建议；已采纳、已忽略的保留")
            .clicked()
        {
            state.request_rerun = Some(source.id.clone());
        }
        let total = source.pending + source.accepted + source.ignored;
        if total > 0 && ui.button("在待整理建议中查看").clicked() {
            state.view = View::Inbox;
            state.inbox_source = Some(source.id.clone());
            state.history_candidates = source.pending == 0;
        }
        if ui
            .add(theme::warning_icon_button(theme::Icon::Trash, "删除"))
            .clicked()
        {
            state.material_delete = Some(source.id.clone());
        }
    });
    ui.add_space(10.0);
    ui.label(egui::RichText::new("产生的建议").strong());
    let mine = data
        .candidates
        .iter()
        .filter(|c| c.5 == source.id)
        .collect::<Vec<_>>();
    if mine.is_empty() {
        theme::caption(ui, "这份材料没有产生建议。可以重新识别再试一次。");
    }
    for (_, kind, _, evidence, candidate_state, _) in mine {
        ui.horizontal(|ui| {
            theme::chip(
                ui,
                super::inbox::kind_label(kind),
                theme::accent(),
                theme::accent_soft(),
            );
            theme::caption(
                ui,
                match candidate_state.as_str() {
                    "accepted" => "已采纳",
                    "ignored" => "已忽略",
                    "later" => "稍后处理",
                    _ => "待核对",
                },
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(evidence)
                        .color(theme::text_soft())
                        .size(theme::font_sizes::SMALL),
                )
                .truncate(),
            );
        });
    }
    ui.add_space(10.0);
    ui.label(egui::RichText::new("原文").strong());
    theme::caption(ui, "识别时保存的快照，不随原资料后来的修改而变。");
    if state
        .material_text
        .as_ref()
        .is_none_or(|(id, _, _)| id != &source.id)
    {
        match store.source(&source.id) {
            Ok((title, content)) => {
                state.material_text = Some((source.id.clone(), title, content));
            }
            Err(e) => {
                theme::caption(ui, &format!("读取原文失败：{e:#}"));
                return;
            }
        }
    }
    if let Some((_, _, content)) = &state.material_text {
        ui.add_space(4.0);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(content.as_str()).wrap().selectable(true));
        });
    }
}

pub(super) fn delete_dialog(
    ui: &mut egui::Ui,
    store: &mut Store,
    state: &mut State,
    data: &Snapshot,
) {
    let Some(id) = state.material_delete.clone() else {
        return;
    };
    let Some(source) = data.sources.iter().find(|s| s.id == id) else {
        state.material_delete = None;
        return;
    };
    egui::Window::new("确认删除材料")
        .id(egui::Id::new("task_material_delete"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.label(format!(
                "删除「{}」？",
                super::inbox::short_title(&source.title)
            ));
            let open = source.pending + source.ignored;
            if open > 0 {
                theme::caption(
                    ui,
                    &format!("它的 {open} 条待核对或已忽略的建议会一并删除。"),
                );
            }
            if source.accepted > 0 {
                theme::caption(
                    ui,
                    &format!(
                        "已有 {} 条建议采纳进任务：原文快照会保留，任务历史里的「原文」仍能查看，只是不再列在这里。",
                        source.accepted
                    ),
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .add(theme::warning_icon_button(theme::Icon::Trash, "删除材料"))
                    .clicked()
                {
                    match store.delete_source(&id) {
                        Ok(_) => {
                            state.ok("材料已删除");
                            if state.material_selected.as_deref() == Some(id.as_str()) {
                                state.material_selected = None;
                            }
                            if state.inbox_source.as_deref() == Some(id.as_str()) {
                                state.inbox_source = None;
                            }
                        }
                        Err(e) => state.fail(format!("删除材料失败：{e:#}")),
                    }
                    state.material_delete = None;
                }
                if ui.button("取消").clicked() {
                    state.material_delete = None;
                }
            });
        });
}
