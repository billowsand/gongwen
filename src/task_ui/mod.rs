use crate::task_assistant::{Role, Status, Store, Task};
#[derive(Default)]
pub(crate) struct State {
    pub selected: Option<String>,
    pub db_error: Option<String>,
    pub draft: Option<Task>,
    pub query: String,
    pub category: String,
    pub view: String,
    pub role_filter: String,
    pub status_filter: String,
    pub sort_deadline: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub show_closed: bool,
    pub auto_organize: Option<bool>,
    pub progress_text: String,
    pub new_category_name: String,
    pub candidate_targets: std::collections::HashMap<String, String>,
    pub candidate_edits:
        std::collections::HashMap<String, crate::task_assistant::extract::Proposal>,
    pub delete_confirm: Option<String>,
    pub candidate_create_id: Option<String>,
    pub cancel_reason: String,
    pub new_category: String,
    pub category_source: String,
    pub category_target: String,
    pub source_title: String,
    pub source_text: String,
    pub knowledge_id: Option<i64>,
    pub request_extract: Option<(String, String, String)>,
    pub extraction_busy: bool,
    pub discard_extraction: bool,
    pub request_knowledge: Option<i64>,
    pub history_candidates: bool,
    pub extraction_error: Option<String>,
    pub pending: Vec<crate::task_assistant::CandidateRow>,
}
pub(crate) fn show(
    ui: &mut egui::Ui,
    store: Option<&mut Store>,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
) {
    egui::ScrollArea::vertical()
        .id_salt("task_assistant_page")
        .auto_shrink([false, false])
        .show(ui, |ui| show_inner(ui, store, knowledge, state));
}
fn show_inner(
    ui: &mut egui::Ui,
    store: Option<&mut Store>,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
) {
    let Some(store) = store else {
        ui.heading("任务助手");
        ui.colored_label(
            crate::theme::warn(),
            state
                .db_error
                .as_deref()
                .unwrap_or("任务数据库暂不可用，请检查稿件库路径与权限。"),
        );
        return;
    };
    ui.heading("任务助手");
    ui.horizontal_wrapped(|ui| {
        ui.label("筛选");
        ui.add(egui::TextEdit::singleline(&mut state.query).desired_width(220.));
        ui.checkbox(&mut state.show_closed, "包含归档与回收站");
        if state.auto_organize.is_none() {
            match store.auto() {
                Ok(value) => state.auto_organize = Some(value),
                Err(e) => state.error = Some(format!("读取自动整理设置失败：{e:#}")),
            }
        }
        let auto = state.auto_organize.get_or_insert(false);
        if ui.checkbox(auto, "新入库资料自动整理").changed()
            && let Err(e) = store.set_auto(*auto)
        {
            state.error = Some(format!("保存自动整理设置失败：{e:#}"));
        }
        if ui
            .add_enabled(state.draft.is_none(), egui::Button::new("＋ 新建任务"))
            .clicked()
        {
            state.candidate_create_id = None;
            state.draft = Some(Task::blank());
            state.selected = None;
        }
    });
    if let Some(e) = &state.error {
        ui.colored_label(crate::theme::warn(), e);
    }
    if let Some(n) = &state.notice {
        ui.label(n);
    }
    if matches!(state.view.as_str(), "archived" | "deleted") {
        state.show_closed = true;
    }
    match store.candidates() {
        Ok(items) => state.pending = items,
        Err(e) => state.error = Some(format!("读取待整理失败：{e:#}")),
    }
    let mut all = match store.list(true) {
        Ok(v) => v,
        Err(e) => {
            ui.colored_label(crate::theme::warn(), format!("读取任务失败：{e:#}"));
            return;
        }
    };
    if state.sort_deadline {
        all.sort_by(|a, b| {
            a.deadline
                .as_deref()
                .unwrap_or("9999-12-31")
                .cmp(b.deadline.as_deref().unwrap_or("9999-12-31"))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
    }
    ui.columns(2, |cols| {
        egui::ScrollArea::vertical()
            .id_salt("task_list")
            .max_height(480.0)
            .show(&mut cols[0], |ui| {
                let q = state.query.to_lowercase();
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("task_view")
                        .selected_text(match state.view.as_str() {
                            "today" => "今天与逾期",
                            "overdue" => "逾期",
                            "waiting" => "等待",
                            "blocked" => "阻塞",
                            "done" => "已完成",
                            "cancelled" => "已取消",
                            "archived" => "归档",
                            "deleted" => "回收站",
                            _ => "我的工作",
                        })
                        .show_ui(ui, |ui| {
                            for (v, n) in [
                                ("all", "我的工作"),
                                ("today", "今天与逾期"),
                                ("overdue", "逾期"),
                                ("waiting", "等待"),
                                ("blocked", "阻塞"),
                                ("done", "已完成"),
                                ("cancelled", "已取消"),
                                ("archived", "归档"),
                                ("deleted", "回收站"),
                            ] {
                                ui.selectable_value(&mut state.view, v.into(), n);
                            }
                        });
                    egui::ComboBox::from_id_salt("task_filter_role")
                        .selected_text(if state.role_filter.is_empty() {
                            "全部角色"
                        } else {
                            &state.role_filter
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut state.role_filter, String::new(), "全部角色");
                            for (r, n) in [
                                (Role::Responsible, "负责"),
                                (Role::Collaborate, "配合"),
                                (Role::Follow, "跟进"),
                                (Role::Watch, "关注"),
                            ] {
                                ui.selectable_value(&mut state.role_filter, r.as_str().into(), n);
                            }
                        });
                    egui::ComboBox::from_id_salt("task_filter_status")
                        .selected_text(if state.status_filter.is_empty() {
                            "全部状态"
                        } else {
                            &state.status_filter
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut state.status_filter,
                                String::new(),
                                "全部状态",
                            );
                            for s in [
                                Status::Todo,
                                Status::InProgress,
                                Status::Waiting,
                                Status::Blocked,
                                Status::Done,
                                Status::Cancelled,
                            ] {
                                ui.selectable_value(
                                    &mut state.status_filter,
                                    s.as_str().into(),
                                    s.as_str(),
                                );
                            }
                        });
                    ui.checkbox(&mut state.sort_deadline, "按期限排序");
                });
                let categories = match store.categories() {
                    Ok(v) => v,
                    Err(e) => {
                        state.error = Some(format!("读取分类失败：{e:#}"));
                        Vec::new()
                    }
                };
                egui::ComboBox::from_id_salt("task_filter_category")
                    .selected_text(if state.category.is_empty() {
                        "全部分类"
                    } else {
                        &state.category
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut state.category, String::new(), "全部分类");
                        for c in categories {
                            ui.selectable_value(&mut state.category, c.clone(), c);
                        }
                    });
                for t in &all {
                    let today = chrono::Local::now()
                        .date_naive()
                        .format("%Y-%m-%d")
                        .to_string();
                    let view_ok = match state.view.as_str() {
                        "today" => {
                            (t.deadline.as_deref().is_some_and(|d| d <= today.as_str())
                                || t.next_deadline
                                    .as_deref()
                                    .is_some_and(|d| d <= today.as_str()))
                                && !t.status.terminal()
                                && !t.archived
                                && !t.deleted
                        }
                        "overdue" => {
                            t.deadline.as_deref().is_some_and(|d| d < today.as_str())
                                && !t.status.terminal()
                        }
                        "waiting" => t.status == Status::Waiting,
                        "blocked" => {
                            t.status == Status::Blocked
                                || (!t.status.terminal()
                                    && crate::task_assistant::relations::blocked(&all, t))
                        }
                        "done" => t.status == Status::Done,
                        "cancelled" => t.status == Status::Cancelled,
                        "archived" => t.archived,
                        "deleted" => t.deleted,
                        _ => !t.status.terminal() || state.status_filter == t.status.as_str(),
                    };
                    if (!state.show_closed && (t.archived || t.deleted))
                        || !view_ok
                        || (!state.category.is_empty() && state.category != t.category)
                        || (!state.role_filter.is_empty() && state.role_filter != t.role.as_str())
                        || (!state.status_filter.is_empty()
                            && state.status_filter != t.status.as_str())
                    {
                        continue;
                    }
                    let text_match = q.is_empty()
                        || [&t.name, &t.next_step, &t.collaborators, &t.category]
                            .iter()
                            .any(|s| s.to_lowercase().contains(&q));
                    let source_match = if q.is_empty() {
                        false
                    } else {
                        match store.source_matches(&t.id, &q) {
                            Ok(v) => v,
                            Err(e) => {
                                state.error = Some(format!("搜索材料来源失败：{e:#}"));
                                false
                            }
                        }
                    };
                    if !text_match && !source_match {
                        continue;
                    }
                    let label = format!(
                        "{}  · {}{}",
                        t.name,
                        t.status.as_str(),
                        if t.needs_review { " · 待核对" } else { "" }
                    );
                    if ui
                        .selectable_label(state.selected.as_deref() == Some(&t.id), label)
                        .clicked()
                    {
                        state.selected = Some(t.id.clone());
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "下一步：{}",
                            if t.next_step.is_empty() {
                                "待安排"
                            } else {
                                &t.next_step
                            }
                        ))
                        .small()
                        .weak(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} · 任务期限 {}",
                            t.category,
                            t.role.as_str(),
                            t.deadline.as_deref().unwrap_or("未设")
                        ))
                        .small()
                        .weak(),
                    );
                }
            });
        egui::ScrollArea::vertical()
            .id_salt("task_detail")
            .max_height(480.0)
            .show(&mut cols[1], |ui| {
                if let Some(mut d) = state.draft.take() {
                    if edit(ui, &mut d, &all, &mut *store, state) {
                        state.draft = Some(d);
                    }
                } else if let Some(id) = &state.selected {
                    if let Some(t) = all.iter().find(|t| &t.id == id) {
                        ui.heading(&t.name);
                        ui.label(format!(
                            "分类：{} · 角色：{} · 状态：{}",
                            t.category,
                            t.role.as_str(),
                            t.status.as_str()
                        ));
                        ui.label(format!("目标：{}", t.goal));
                        ui.label(format!(
                            "下一步：{}",
                            if t.next_step.is_empty() {
                                "（稍后补充）"
                            } else {
                                t.next_step.as_str()
                            }
                        ));
                        ui.label(format!("期限：{}", t.deadline.as_deref().unwrap_or("未设")));
                        ui.label(format!("协作：{}", t.collaborators));
                        if t.needs_review {
                            ui.colored_label(crate::theme::warn(), "材料或关系变更后待人工核对");
                        }
                        if let Some(parent) = t
                            .parent_id
                            .as_ref()
                            .and_then(|id| all.iter().find(|x| &x.id == id))
                            && ui.button(format!("上级：{}", parent.name)).clicked()
                        {
                            state.selected = Some(parent.id.clone());
                        }
                        let prerequisites = all
                            .iter()
                            .filter(|x| t.prerequisites.contains(&x.id))
                            .collect::<Vec<_>>();
                        for p in prerequisites {
                            if ui
                                .button(format!("前置：{} · {}", p.name, p.status.as_str()))
                                .clicked()
                            {
                                state.selected = Some(p.id.clone());
                            }
                        }
                        for child in all
                            .iter()
                            .filter(|x| x.parent_id.as_deref() == Some(t.id.as_str()))
                        {
                            if ui
                                .button(format!(
                                    "子任务：{} · {}",
                                    child.name,
                                    child.status.as_str()
                                ))
                                .clicked()
                            {
                                state.selected = Some(child.id.clone());
                            }
                        }
                        for successor in all.iter().filter(|x| x.prerequisites.contains(&t.id)) {
                            if ui
                                .button(format!(
                                    "后续：{} · {}",
                                    successor.name,
                                    successor.status.as_str()
                                ))
                                .clicked()
                            {
                                state.selected = Some(successor.id.clone());
                            }
                        }
                        if !t.archived && !t.deleted {
                            ui.text_edit_singleline(&mut state.progress_text);
                            if ui.button("记进展").clicked() {
                                match store.record_progress(&t.id, &state.progress_text) {
                                    Ok(()) => {
                                        state.progress_text.clear();
                                        state.notice = Some("进展已记入历史".into())
                                    }
                                    Err(e) => state.error = Some(format!("记录进展失败：{e:#}")),
                                }
                            }
                        }
                        ui.separator();
                        if !t.archived && !t.deleted && ui.button("编辑").clicked() {
                            state.draft = Some(t.clone());
                        }
                        if !t.archived && !t.deleted {
                            ui.horizontal_wrapped(|ui| {
                                for status in [
                                    Status::Todo,
                                    Status::InProgress,
                                    Status::Waiting,
                                    Status::Blocked,
                                    Status::Done,
                                ] {
                                    if ui.small_button(status.as_str()).clicked() {
                                        let mut update = t.clone();
                                        update.status = status;
                                        match store.save(&update, "状态变更", status.as_str()) {
                                            Ok(()) => state.notice = Some("状态已更新".into()),
                                            Err(e) => {
                                                state.error = Some(format!("无法变更状态：{e:#}"))
                                            }
                                        }
                                    }
                                }
                            });
                            if ui
                                .add_enabled(t.status.terminal(), egui::Button::new("归档"))
                                .clicked()
                            {
                                let mut update = t.clone();
                                update.archived = true;
                                if let Err(e) = store.save(&update, "归档", "用户归档") {
                                    state.error = Some(format!("归档失败：{e:#}"))
                                }
                            }
                            if ui.button("移入回收站").clicked() {
                                state.delete_confirm = Some(t.id.clone());
                            }
                        } else if ui.button("恢复").clicked()
                            && let Err(e) = store.restore_task(&t.id)
                        {
                            state.error = Some(format!("恢复失败：{e:#}"))
                        }
                        ui.separator();
                        ui.collapsing("来源原文快照", |ui| match store.sources(&t.id) {
                            Ok(sources) => {
                                for (index, (title, content)) in sources.iter().enumerate() {
                                    ui.push_id(("task_source", index), |ui| {
                                        ui.collapsing(title, |ui| {
                                            ui.label(content);
                                        });
                                    });
                                }
                            }
                            Err(e) => {
                                ui.colored_label(
                                    crate::theme::warn(),
                                    format!("读取来源失败：{e:#}"),
                                );
                            }
                        });
                        ui.heading("历史");
                        match store.history(&t.id) {
                            Ok(hist) => {
                                for (at, action, detail) in hist {
                                    ui.label(format!("{at} · {action} · {detail}"));
                                }
                            }
                            Err(e) => {
                                ui.colored_label(
                                    crate::theme::warn(),
                                    format!("读取历史失败：{e:#}"),
                                );
                            }
                        }
                    }
                } else {
                    ui.label("选择任务查看详情，或新建任务。");
                }
            });
    });
    ui.separator();
    ui.collapsing("分类管理", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.text_edit_singleline(&mut state.new_category);
            if ui.button("新增分类").clicked() {
                match store.add_category(&state.new_category) {
                    Ok(()) => state.new_category.clear(),
                    Err(e) => state.error = Some(format!("新增分类失败：{e:#}")),
                }
            }
        });
        let cats = match store.categories() {
            Ok(v) => v,
            Err(e) => {
                state.error = Some(format!("读取分类失败：{e:#}"));
                Vec::new()
            }
        };
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("task_cat_from")
                .selected_text(if state.category_source.is_empty() {
                    "选择要删除的分类"
                } else {
                    &state.category_source
                })
                .show_ui(ui, |ui| {
                    for c in &cats {
                        ui.selectable_value(&mut state.category_source, c.clone(), c);
                    }
                });
            egui::ComboBox::from_id_salt("task_cat_to")
                .selected_text(if state.category_target.is_empty() {
                    "迁移到"
                } else {
                    &state.category_target
                })
                .show_ui(ui, |ui| {
                    for c in &cats {
                        ui.selectable_value(&mut state.category_target, c.clone(), c);
                    }
                });
            if ui.button("迁移并删除分类").clicked() {
                match store.move_category(&state.category_source, &state.category_target) {
                    Ok(()) => {
                        state.category_source.clear();
                        state.notice = Some("分类及所有任务（含归档项）已迁移".into())
                    }
                    Err(e) => state.error = Some(format!("分类迁移失败：{e:#}")),
                }
            }
            ui.text_edit_singleline(&mut state.new_category_name);
            if ui.button("重命名所选分类").clicked() {
                match store.rename_category(&state.category_source, &state.new_category_name) {
                    Ok(()) => {
                        state.category_source = state.new_category_name.clone();
                        state.new_category_name.clear();
                        state.notice = Some("分类已重命名".into())
                    }
                    Err(e) => state.error = Some(format!("分类重命名失败：{e:#}")),
                }
            }
        });
    });
    ui.collapsing("材料整理（结果须逐项人工核对）", |ui| {
        ui.label("粘贴材料标题和原文；模型只提出候选，不会直接改任务。");
        ui.label("材料标题");
        ui.text_edit_singleline(&mut state.source_title);
        ui.label("材料原文");
        ui.add(egui::TextEdit::multiline(&mut state.source_text).desired_rows(5));
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!state.extraction_busy, |ui| {
                if ui.button("识别材料").clicked() {
                    state.request_extract = Some((
                        state.source_title.clone(),
                        state.source_text.clone(),
                        "pasted".into(),
                    ));
                }
            });
            if state.extraction_busy {
                ui.label("正在后台识别，完成前数据库恢复会保持禁用。");
                if ui.button("取消本次结果").clicked() {
                    state.discard_extraction = true;
                }
                if state.discard_extraction {
                    ui.label("已取消采纳；等待当前请求结束后释放忙状态。");
                }
            }
        });
        if let Some(e) = &state.extraction_error {
            ui.colored_label(crate::theme::warn(), e);
        }
        ui.separator();
        ui.label("或从知识库选择一篇资料：");
        match knowledge {
            Some(knowledge) => match knowledge.list_docs(None) {
                Ok(docs) => {
                    egui::ComboBox::from_id_salt("task_knowledge_source")
                        .selected_text(
                            state
                                .knowledge_id
                                .and_then(|id| {
                                    docs.iter().find(|d| d.id == id).map(|d| d.title.as_str())
                                })
                                .unwrap_or("选择知识库资料"),
                        )
                        .show_ui(ui, |ui| {
                            for doc in &docs {
                                ui.selectable_value(
                                    &mut state.knowledge_id,
                                    Some(doc.id),
                                    &doc.title,
                                );
                            }
                        });
                    if ui
                        .add_enabled(!state.extraction_busy, egui::Button::new("整理选中的资料"))
                        .clicked()
                    {
                        if let Some(id) = state.knowledge_id {
                            state.request_knowledge = Some(id);
                        } else {
                            state.extraction_error = Some("请先选择一篇知识库资料".into());
                        }
                    }
                }
                Err(e) => state.extraction_error = Some(format!("读取知识库列表失败：{e:#}")),
            },
            None => {
                ui.label("知识库暂不可用。");
            }
        }
        if ui.button("刷新待整理").clicked() {
            match store.candidates() {
                Ok(v) => state.pending = v,
                Err(e) => state.error = Some(format!("读取待整理失败：{e:#}")),
            }
        }
        ui.checkbox(
            &mut state.history_candidates,
            "显示已处理记录（已采纳 / 已忽略）",
        );
        let pending = state.pending.clone();
        for (id, kind, payload, evidence, candidate_state) in &pending {
            if state.history_candidates
                != matches!(candidate_state.as_str(), "accepted" | "ignored")
            {
                continue;
            }
            if candidate_state == "accepted" {
                ui.group(|ui| {
                    ui.label(format!("已采纳 · {}", candidate_label(kind)));
                    ui.label(format!("摘录：{evidence}"));
                    candidate_source_ui(ui, store, id);
                });
                continue;
            }
            ui.group(|ui| {
                let original =
                    match serde_json::from_str::<crate::task_assistant::extract::Proposal>(payload)
                    {
                        Ok(p) => p,
                        Err(e) => {
                            state.error = Some(format!("候选数据无法解析：{e}"));
                            ui.colored_label(
                                crate::theme::warn(),
                                format!("无法读取候选：{payload}"),
                            );
                            return;
                        }
                    };
                let proposal = state.candidate_edits.entry(id.clone()).or_insert(original);
                ui.label(format!(
                    "{} · {}",
                    candidate_label(kind),
                    proposal.name.as_deref().unwrap_or("待核对事项")
                ));
                optional_line(ui, "建议名称", &mut proposal.name);
                optional_line(ui, "建议完成条件", &mut proposal.goal);
                optional_line(ui, "建议下一步", &mut proposal.next_step);
                optional_line(ui, "建议期限", &mut proposal.deadline);
                optional_line(ui, "建议协作分工", &mut proposal.collaborators);
                optional_line(ui, "意见提出主体", &mut proposal.speaker);
                optional_line(ui, "意见针对事项", &mut proposal.about);
                optional_line(ui, "意见/待决内容", &mut proposal.note);
                ui.label(format!(
                    "状态：{}；原文摘录：{evidence}",
                    match candidate_state.as_str() {
                        "later" => "稍后处理",
                        "ignored" => "已忽略",
                        _ => "待核对",
                    }
                ));
                candidate_source_ui(ui, store, id);
                if ui.button("保存候选修改").clicked() {
                    match store.edit_candidate(id, proposal) {
                        Ok(()) => state.notice = Some("候选修改已保存，任务尚未变更".into()),
                        Err(e) => state.error = Some(format!("保存候选修改失败：{e:#}")),
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("稍后").clicked()
                        && let Err(e) = store.candidate_done(id, "later")
                    {
                        state.error = Some(format!("延期失败：{e:#}"))
                    }
                    if candidate_state != "pending"
                        && ui.button("恢复到待整理").clicked()
                        && let Err(e) = store.candidate_done(id, "pending")
                    {
                        state.error = Some(format!("恢复失败：{e:#}"))
                    }
                    if ui.button("忽略").clicked()
                        && let Err(e) = store.candidate_done(id, "ignored")
                    {
                        state.error = Some(format!("忽略失败：{e:#}"))
                    }
                    {
                        let p = proposal;
                        if p.kind == "new_task" {
                            if ui
                                .add_enabled(
                                    state.draft.is_none(),
                                    egui::Button::new("核对并编辑新任务"),
                                )
                                .clicked()
                            {
                                let mut draft = crate::task_assistant::extract::proposal_task(p);
                                draft.needs_review = false;
                                state.candidate_create_id = Some(id.clone());
                                state.draft = Some(draft);
                                state.selected = None;
                            }
                        } else {
                            let target = state
                                .candidate_targets
                                .entry(id.clone())
                                .or_insert_with(|| p.task_id.clone().unwrap_or_default());
                            let previous_target = target.clone();
                            egui::ComboBox::from_id_salt(("task_candidate_target", id))
                                .selected_text(
                                    all.iter()
                                        .find(|t| t.id == *target)
                                        .map(|t| t.name.as_str())
                                        .unwrap_or("人工选择目标任务"),
                                )
                                .show_ui(ui, |ui| {
                                    for t in all.iter().filter(|t| !t.deleted && !t.archived) {
                                        ui.selectable_value(target, t.id.clone(), &t.name);
                                    }
                                });
                            if previous_target != *target
                                && !target.is_empty()
                                && let Err(e) = store.recheck_candidate(id, target)
                            {
                                state.error = Some(format!("绑定目标失败：{e:#}"));
                            }
                            if let Some(current) = all.iter().find(|t| t.id == *target) {
                                ui.label(format!(
                                    "当前版本 {}：{}；完成条件：{}；下一步：{}；期限：{}；分工：{}",
                                    current.revision,
                                    current.name,
                                    current.goal,
                                    current.next_step,
                                    current.deadline.as_deref().unwrap_or("未设"),
                                    current.collaborators
                                ));
                            }
                            if !target.is_empty()
                                && ui
                                    .button(if p.kind == "completion_evidence" {
                                        "核对并确认任务完成"
                                    } else {
                                        "核对并采纳到所选任务"
                                    })
                                    .clicked()
                            {
                                match store
                                    .edit_candidate(id, p)
                                    .and_then(|_| store.apply_candidate(id, target))
                                {
                                    Ok(()) => {
                                        state.notice = Some("候选已采纳并记入来源历史".into())
                                    }
                                    Err(e) => state.error = Some(format!("无法采纳：{e:#}")),
                                }
                            }
                            if !target.is_empty() && ui.button("以当前版本重新核对").clicked()
                            {
                                match store.recheck_candidate(id, target) {
                                    Ok(()) => {
                                        state.notice = Some(
                                            "候选已绑定当前任务版本，请再次检查建议后采纳".into(),
                                        )
                                    }
                                    Err(e) => state.error = Some(format!("重新核对失败：{e:#}")),
                                }
                            }
                        }
                    }
                });
            });
        }
    });
    if let Some(id) = state.delete_confirm.clone() {
        let task = all.iter().find(|t| t.id == id);
        egui::Window::new("确认移入回收站")
            .id(egui::Id::new("task_delete_confirm"))
            .collapsible(false)
            .show(ui.ctx(), |ui| {
                if let Some(t) = task {
                    ui.label(format!("将删除「{}」。任务不会级联删除。", t.name));
                    let children = all.iter().filter(|x| x.parent_id.as_deref() == Some(&id)).count();
                    let dependents = all.iter().filter(|x| x.prerequisites.contains(&id)).count();
                    ui.label(format!("{children} 个子任务会变为独立任务，{dependents} 个后续任务会解除该前置并标记待核对。"));
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("确认移入回收站").clicked() {
                        match store.soft_delete(&id) {
                            Ok(()) => state.notice = Some("任务已移入回收站，关联任务待核对".into()),
                            Err(e) => state.error = Some(format!("删除失败：{e:#}")),
                        }
                        state.delete_confirm = None;
                    }
                    if ui.button("取消").clicked() { state.delete_confirm = None; }
                });
            });
    }
}
fn edit(
    ui: &mut egui::Ui,
    d: &mut Task,
    all: &[Task],
    store: &mut Store,
    state: &mut State,
) -> bool {
    let mut keep = true;
    ui.heading(if d.revision == 0 {
        "新建任务"
    } else {
        "编辑任务"
    });
    ui.label("名称（必填）");
    ui.text_edit_singleline(&mut d.name);
    ui.label("分类");
    let cats = match store.categories() {
        Ok(v) => v,
        Err(e) => {
            state.error = Some(format!("读取分类失败：{e:#}"));
            Vec::new()
        }
    };
    egui::ComboBox::from_id_salt("task_edit_cat")
        .selected_text(&d.category)
        .show_ui(ui, |ui| {
            for c in cats {
                ui.selectable_value(&mut d.category, c.clone(), c);
            }
        });
    ui.label("我的角色");
    egui::ComboBox::from_id_salt("task_role")
        .selected_text(d.role.as_str())
        .show_ui(ui, |ui| {
            for (r, n) in [
                (Role::Responsible, "负责"),
                (Role::Collaborate, "配合"),
                (Role::Follow, "跟进"),
                (Role::Watch, "关注"),
            ] {
                ui.selectable_value(&mut d.role, r, n);
            }
        });
    ui.label("协作分工");
    ui.text_edit_singleline(&mut d.collaborators);
    ui.label("目标 / 完成条件（必填）");
    ui.text_edit_multiline(&mut d.goal);
    ui.label("下一步");
    ui.text_edit_singleline(&mut d.next_step);
    ui.horizontal(|ui| {
        ui.label("下一步期限");
        let mut s = d.next_deadline.clone().unwrap_or_default();
        ui.text_edit_singleline(&mut s);
        d.next_deadline = (!s.trim().is_empty()).then_some(s);
    });
    ui.horizontal(|ui| {
        ui.label("任务期限");
        let mut s = d.deadline.clone().unwrap_or_default();
        ui.text_edit_singleline(&mut s);
        d.deadline = (!s.trim().is_empty()).then_some(s);
    });
    ui.label("日期格式 YYYY-MM-DD");
    if d.next_deadline
        .as_ref()
        .zip(d.deadline.as_ref())
        .is_some_and(|(a, b)| a > b)
    {
        ui.colored_label(crate::theme::warn(), "下一步期限晚于任务期限，请核对。");
    }
    if let Some(parent) = all.iter().find(|t| Some(&t.id) == d.parent_id.as_ref())
        && d.deadline
            .as_ref()
            .zip(parent.deadline.as_ref())
            .is_some_and(|(a, b)| a > b)
    {
        ui.colored_label(crate::theme::warn(), "本任务期限晚于上级任务期限，请核对。");
    }
    ui.label("状态");
    egui::ComboBox::from_id_salt("task_edit_status")
        .selected_text(d.status.as_str())
        .show_ui(ui, |ui| {
            for status in [
                Status::Todo,
                Status::InProgress,
                Status::Waiting,
                Status::Blocked,
                Status::Done,
                Status::Cancelled,
            ] {
                ui.selectable_value(&mut d.status, status, status.as_str());
            }
        });
    ui.checkbox(&mut d.needs_review, "待核对（取消勾选表示我已核对）");
    if d.status == Status::Cancelled {
        ui.label("取消理由（必填）");
        ui.text_edit_singleline(&mut state.cancel_reason);
    }
    ui.label("上级任务");
    egui::ComboBox::from_id_salt("task_parent")
        .selected_text(
            d.parent_id
                .as_ref()
                .and_then(|id| all.iter().find(|t| &t.id == id))
                .map(|t| t.name.as_str())
                .unwrap_or("无上级"),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut d.parent_id, None, "无上级");
            for t in all
                .iter()
                .filter(|t| t.id != d.id && !t.deleted && !t.archived)
            {
                ui.selectable_value(&mut d.parent_id, Some(t.id.clone()), &t.name);
            }
        });
    ui.label("前置任务（全部完成并核对后才可进入进行中/完成）");
    for t in all.iter().filter(|t| t.id != d.id && !t.deleted) {
        let mut selected = d.prerequisites.contains(&t.id);
        if ui.checkbox(&mut selected, &t.name).changed() {
            if selected {
                d.prerequisites.push(t.id.clone())
            } else {
                d.prerequisites.retain(|id| id != &t.id)
            }
        }
    }
    ui.separator();
    let save_button = ui.button("保存");
    if save_button.clicked() {
        let detail = if d.status == Status::Cancelled {
            state.cancel_reason.trim()
        } else {
            "用户保存"
        };
        let result = if let Some(candidate_id) = state.candidate_create_id.as_deref() {
            store
                .create_from_candidate(candidate_id, d)
                .map_err(|e| format!("{e:#}"))
        } else {
            store
                .save(d, if d.revision == 0 { "创建" } else { "编辑" }, detail)
                .map_err(|e| format!("{e:#}"))
        };
        match result {
            Ok(()) => {
                state.notice = Some("任务已保存".into());
                state.selected = Some(d.id.clone());
                keep = false;
                state.candidate_create_id = None;
                state.error = None
            }
            Err(e) => state.error = Some(format!("保存失败：{e}")),
        }
    }
    let cancel_button = ui.button("放弃本次编辑");
    if cancel_button.clicked() {
        state.candidate_create_id = None;
        keep = false;
    }
    keep
}

fn optional_line(ui: &mut egui::Ui, label: &str, value: &mut Option<String>) {
    let mut text = value.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.text_edit_singleline(&mut text).changed() {
            *value = (!text.trim().is_empty()).then_some(text);
        }
    });
}

fn candidate_label(kind: &str) -> &str {
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

fn candidate_source_ui(ui: &mut egui::Ui, store: &Store, id: &str) {
    ui.push_id(("candidate_source", id), |ui| {
        ui.collapsing("查看来源标题与完整原文", |ui| {
            match store.candidate_source(id) {
                Ok((title, text)) => {
                    ui.strong(title);
                    ui.label(text);
                }
                Err(e) => {
                    ui.colored_label(crate::theme::warn(), format!("读取来源失败：{e:#}"));
                }
            }
        });
    });
}

#[cfg(test)]
mod tests;
