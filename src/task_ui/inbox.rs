//! 「待整理建议」视图：材料输入与候选卡，取代原来折叠在页尾的「材料整理」。
//!
//! 候选的识别、采纳与过期保护都在 `task_assistant::Store`；这里只负责摆放。
//! 选了目标任务的候选会把右栏切到该任务，采纳前能对照它的当前内容。
use super::{Replace, Snapshot, State};
use crate::task_assistant::extract::{Proposal, auto_title};
use crate::task_assistant::{SourceRow, Store};
use crate::theme;

const FIELDS: [(&str, &str); 11] = [
    ("name", "建议名称"),
    ("owner", "建议负责人"),
    ("progress", "当前进展"),
    ("next_step", "建议下一步"),
    ("deadline", "建议期限"),
    ("risks", "风险与应对措施"),
    ("speaker", "意见提出主体"),
    ("about", "意见针对事项"),
    ("note", "意见 / 待决内容"),
    ("goal", "旧候选说明"),
    ("collaborators", "旧候选责任说明"),
];

/// 各类候选默认显示的字段；其它字段有值时也照样显示，不藏模型给的内容。
fn relevant(kind: &str) -> &'static [&'static str] {
    match kind {
        "new_task" => &[
            "name",
            "owner",
            "progress",
            "next_step",
            "deadline",
            "risks",
        ],
        "progress" => &["progress", "next_step", "note"],
        "deadline" => &["deadline"],
        "responsibility" | "owner" => &["owner"],
        "risk" => &["risks"],
        "scope" => &["name"],
        "completion_evidence" => &["note"],
        "opinion" | "decision" => &["speaker", "about", "note"],
        _ => &[
            "name",
            "owner",
            "progress",
            "next_step",
            "deadline",
            "risks",
            "note",
        ],
    }
}

fn slot<'a>(p: &'a mut Proposal, key: &str) -> &'a mut Option<String> {
    match key {
        "name" => &mut p.name,
        "owner" => &mut p.owner,
        "progress" => &mut p.progress,
        "next_step" => &mut p.next_step,
        "deadline" => &mut p.deadline,
        "risks" => &mut p.risks,
        "goal" => &mut p.goal,
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
        "responsibility" | "owner" => "负责人调整",
        "risk" => "风险变化",
        "deadline" => "期限调整",
        "scope" => "范围调整",
        "completion_evidence" => "完成证据",
        _ => "待核变化",
    }
}

/// 一次材料识别的结果，由应用层的后台轮询交回界面。
pub(crate) enum ExtractionOutcome {
    /// 新材料已存进材料库。
    Saved {
        source_id: String,
        title: String,
        text: String,
        added: usize,
    },
    /// 同标题同正文的材料之前识别过：这次没调模型，让人选查看还是重新识别。
    Known { source: SourceRow, text: String },
    /// 重新识别了一份已存的材料。
    Rerun {
        source_id: String,
        title: String,
        result: crate::task_assistant::Rerun,
    },
    /// 用户点了「取消本次结果」，迟到的结果已丢弃。
    Discarded,
}

/// 收下识别结果：结果写在材料卡「识别」按钮旁边，不在页顶插提示条挤动三栏；
/// 不在「待整理建议」视图时另弹一条底部浮动提示。新材料识别成功、输入框还是刚交出去
/// 的那份原文就清空，下一次粘贴从空框开始；识别过的材料不清，留着卡片让人选怎么办。
pub(crate) fn receive_extraction(state: &mut State, outcome: ExtractionOutcome) {
    state.extraction_error = None;
    state.known_source = None;
    state.extraction_source = None;
    let message = match outcome {
        ExtractionOutcome::Saved {
            source_id,
            title,
            text,
            added,
        } => {
            if state.source_text == text {
                state.source_text.clear();
                state.source_title.clear();
                state.source_title_auto = None;
            }
            let title = short_title(&title);
            if added > 0 {
                state.extraction_source = Some(source_id);
            }
            if added == 0 {
                format!(
                    "「{title}」识别完成，但模型没有从中找出任务或进展。材料已存进「材料」，可在那里重新识别，或用「新建任务 → 从文字填写」。"
                )
            } else {
                format!("「{title}」识别完成，新增 {added} 条待核对的建议。")
            }
        }
        ExtractionOutcome::Known { source, text } => {
            let title = short_title(&source.title);
            let still_there = state.source_text == text;
            state.extraction_result = None;
            if state.view != super::View::Inbox || !still_there {
                state.ok(format!(
                    "「{title}」之前识别过，可在「材料」里查看或重新识别。"
                ));
            }
            if still_there {
                state.known_source = Some(source);
            }
            return;
        }
        ExtractionOutcome::Rerun {
            source_id,
            title,
            result,
        } => {
            if result.added > 0 {
                state.extraction_source = Some(source_id);
            }
            rerun_message(&short_title(&title), result)
        }
        ExtractionOutcome::Discarded => "本次识别结果已丢弃，材料可重新整理。".to_owned(),
    };
    if state.view != super::View::Inbox {
        state.ok(format!("{message}见「待整理建议」。"));
    }
    state.extraction_result = Some(message);
}

fn rerun_message(title: &str, r: crate::task_assistant::Rerun) -> String {
    let mut out = if r.added == 0 {
        format!("「{title}」重新识别完成，没有新的建议")
    } else {
        format!("「{title}」重新识别完成，新增 {} 条待核对的建议", r.added)
    };
    if r.replaced > 0 {
        out.push_str(&format!("，替换了 {} 条未处理的旧建议", r.replaced));
    }
    if r.skipped > 0 {
        out.push_str(&format!(
            "；{} 条与已采纳或已忽略的相同，没有重复生成",
            r.skipped
        ));
    }
    out.push('。');
    out
}

/// 「之前识别过」卡片：说清是哪一份、上次的结果如何，给出查看与重新识别两条路。
fn known_card(ui: &mut egui::Ui, state: &mut State, data: &Snapshot, stored: &SourceRow) {
    // 计数取当前数据，卡片停留期间采纳或忽略了也对得上。
    let source = data
        .sources
        .iter()
        .find(|s| s.id == stored.id)
        .unwrap_or(stored);
    egui::Frame::new()
        .fill(theme::accent_soft())
        .corner_radius(theme::chrome_radius(7))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.add(theme::Icon::History.image_sized(14.0).tint(theme::accent()));
                ui.label(
                    egui::RichText::new(format!(
                        "这份材料 {} 识别过（标题与正文都相同），这次没有调用模型。",
                        super::short_time(&source.created_at)
                    ))
                    .color(theme::accent()),
                );
            });
            theme::caption(
                ui,
                &format!(
                    "上次的建议：{}{}",
                    super::materials::counts(source),
                    if source.hidden {
                        "。这份材料已从「材料」中删除，重新识别会恢复它。"
                    } else {
                        ""
                    }
                ),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if theme::primary_icon_button(ui, theme::Icon::Refresh, "重新识别")
                    .on_hover_text(
                        "按存下的原文再调一次模型，替换还没处理的建议；已采纳、已忽略的保留",
                    )
                    .clicked()
                {
                    state.request_rerun = Some(source.id.clone());
                    state.known_source = None;
                }
                if source.pending + source.accepted + source.ignored > 0
                    && ui.button("查看这批建议").clicked()
                {
                    state.inbox_source = Some(source.id.clone());
                    state.history_candidates = source.pending == 0;
                    state.known_source = None;
                }
                if !source.hidden && ui.button("打开材料").clicked() {
                    state.view = super::View::Materials;
                    state.material_selected = Some(source.id.clone());
                    state.known_source = None;
                }
                if ui.button("关闭").clicked() {
                    state.known_source = None;
                }
            });
        });
}

pub(super) fn short_title(title: &str) -> String {
    let mut chars = title.trim().chars();
    let head = chars.by_ref().take(20).collect::<String>();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

pub(super) fn show(
    ui: &mut egui::Ui,
    store: &mut Store,
    knowledge: Option<&mut crate::knowledge::KnowledgeStore>,
    state: &mut State,
    data: &Snapshot,
) {
    material(ui, knowledge, state, data);
    ui.add_space(10.0);
    let rows = data
        .candidates
        .iter()
        .filter(|c| state.history_candidates != super::nav::pending_candidate(&c.4))
        .filter(|c| state.inbox_source.as_ref().is_none_or(|id| &c.5 == id))
        .collect::<Vec<_>>();
    if let Some(id) = state.inbox_source.clone() {
        let title = data
            .sources
            .iter()
            .find(|s| s.id == id)
            .map(|s| short_title(&s.title))
            .unwrap_or_else(|| "所选材料".into());
        if super::banner(
            ui,
            theme::Icon::Paperclip,
            theme::accent(),
            theme::accent_soft(),
            &format!("只看「{title}」的建议"),
            "显示全部",
        ) {
            state.inbox_source = None;
        }
        ui.add_space(4.0);
    }
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
    for (id, kind, payload, evidence, candidate_state, _) in rows {
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
    data: &Snapshot,
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
            "材料标题（选填，留空取首行）",
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
        if text.changed() {
            // 原文换了：上次的结果不再对应框里的内容，自动取的标题也跟着作废。
            state.extraction_result = None;
            state.known_source = None;
            if state.source_title_auto.as_deref() == Some(state.source_title.as_str()) {
                state.source_title.clear();
            }
            state.source_title_auto = None;
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
                if state.source_title.trim().is_empty() {
                    state.source_title = auto_title(&state.source_text, chrono::Local::now());
                    state.source_title_auto = Some(state.source_title.clone());
                }
                state.extraction_result = None;
                state.known_source = None;
                state.request_extract = Some((
                    state.source_title.clone(),
                    state.source_text.clone(),
                    "pasted".into(),
                ));
            }
            if !state.extraction_busy
                && let Some(result) = &state.extraction_result
            {
                // 与按钮同一行，出现和消失都不改变卡片高度。
                ui.add(theme::Icon::Check.image_sized(14.0).tint(theme::success()));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(result)
                            .color(theme::success())
                            .size(theme::font_sizes::SMALL),
                    )
                    .wrap(),
                );
                if let Some(id) = state.extraction_source.clone()
                    && state.inbox_source.as_deref() != Some(id.as_str())
                    && ui.small_button("只看这批").clicked()
                {
                    state.inbox_source = Some(id);
                    state.history_candidates = false;
                }
            }
        });
        if !state.extraction_busy
            && let Some(source) = state.known_source.clone()
        {
            ui.add_space(4.0);
            known_card(ui, state, data, &source);
        }
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
            state.extraction_result = None;
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
    let previous_owner = proposal.owner.clone();
    for (key, label) in FIELDS {
        let value = slot(&mut proposal, key);
        if !wanted.contains(&key) && value.is_none() {
            continue;
        }
        optional_line(ui, label, value);
    }
    if proposal.owner != previous_owner {
        proposal.owner_id = None;
    }
    if let Some(name) = proposal.owner.as_deref() {
        let selected = proposal.owner_id.and_then(|id| {
            data.vocabulary.iter().find(|e| {
                e.id == id
                    && e.category == crate::models::VocabularyCategory::Person
                    && e.canonical.trim() == name.trim()
            })
        });
        let matches = super::owner::match_name(data.vocabulary, name);
        if let Some(person) = selected.or_else(|| (matches.len() == 1).then(|| matches[0])) {
            proposal.owner_id = Some(person.id);
            proposal.owner = Some(person.canonical.clone());
            theme::caption(
                ui,
                &format!(
                    "负责人：{}",
                    super::owner::label(data.vocabulary, Some(person.id), &person.canonical).0
                ),
            );
        } else {
            proposal.owner_id = None;
            theme::caption(
                ui,
                if matches.is_empty() {
                    "原文提到的负责人不在标准词库中，请选择".into()
                } else {
                    format!(
                        "原文提到『{name}』，词库里有 {} 位同名人员，请选择",
                        matches.len()
                    )
                }
                .as_str(),
            );
            let mut owner_id = None;
            let mut owner_name = String::new();
            // 每张候选卡各有一个搜索框，和表单的负责人搜索也分开。
            let query = state
                .candidate_owner_queries
                .entry(id.to_owned())
                .or_insert_with(|| name.to_owned());
            let _ = super::owner::picker(
                ui,
                ("candidate_owner", id),
                data.vocabulary,
                &mut owner_id,
                &mut owner_name,
                query,
            );
            if owner_id.is_some() {
                proposal.owner_id = owner_id;
                proposal.owner = Some(owner_name);
            }
        }
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
        } else if crate::task_assistant::extract::converts_to_new_task(&p.kind)
            && theme::primary_icon_button(ui, theme::Icon::Plus, "建成新任务")
                .on_hover_text(
                    "材料讲的工作还没有对应任务：带着这条建议的内容新建一项，补好名称再保存",
                )
                .clicked()
        {
            let mut draft = crate::task_assistant::extract::proposal_task(p);
            draft.needs_review = false;
            state.open_draft(Replace::Candidate(id.to_owned(), Box::new(draft)));
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
