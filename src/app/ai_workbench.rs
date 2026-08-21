//! 起草页统一 AI 工作台与修改提案审阅窗。
//!
//! 界面沿用应用现有的卡片、强调按钮、状态 chip 与版本对照渲染器；工作台只做
//! 任务分流和事实确认，正文生成仍走 draft_page 的统一后台链路。

use crate::app::GongwenApp;
use crate::diff::{ContentSnapshot, draft_changes, manuscript_diff};
use crate::diff_view::{DiffViewConfig, manuscript_diff_ui};
use crate::draft_page::{AiTaskRequest, AiWorkflowKind, RagKindFilter};
use crate::manuscript::ManuscriptFilter;
use crate::models::{DraftInput, ManuscriptStatus, TemplateKind};
use crate::theme;
use eframe::egui;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SimilarStrategy {
    #[default]
    Structure,
    RelatedFacts,
    Partial,
    Rolling,
}

impl SimilarStrategy {
    const ALL: [Self; 4] = [
        Self::Structure,
        Self::RelatedFacts,
        Self::Partial,
        Self::Rolling,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Structure => "结构仿写",
            Self::RelatedFacts => "同类改稿",
            Self::Partial => "局部更新",
            Self::Rolling => "续期换届",
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Self::Structure => {
                "只复用基准稿的章节结构、论述顺序和通用公文表达，基准稿中的具体事实一律不继承。"
            }
            Self::RelatedFacts => {
                "按确认变化清单替换主题及其关联事实；没有在本次材料中重新确认的基准稿事实不得沿用。"
            }
            Self::Partial => {
                "只更新变化清单指向的事项和章节；其余内容仅可保留通用表述，不得把旧稿具体事实当作本次事实。"
            }
            Self::Rolling => {
                "按新周期更新年度、批次、阶段、时限和工作安排；必须检查并清除上一周期遗留事实。"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PolishScope {
    #[default]
    Full,
    Selection,
}

#[derive(Clone)]
struct BaselineRow {
    id: i64,
    title: String,
    kind: TemplateKind,
    status: ManuscriptStatus,
}

struct LoadedBaseline {
    id: i64,
    title: String,
    markdown: String,
}

struct ConfirmedFact {
    label: String,
    value: String,
    confirmed: bool,
}

struct ChangeItem {
    field: String,
    before: String,
    after: String,
    confirmed: bool,
}

pub(crate) struct AiWorkbench {
    workflow: AiWorkflowKind,
    baselines: Vec<BaselineRow>,
    baseline_id: Option<i64>,
    baseline: Option<LoadedBaseline>,
    similar_strategy: SimilarStrategy,
    changes: Vec<ChangeItem>,
    raw_material: String,
    facts: Vec<ConfirmedFact>,
    /// 提取事实单时的材料快照；原材料后来变了就必须重新确认。
    facts_source: String,
    selected_prompt: u32,
    custom_instruction: String,
    polish_scope: PolishScope,
    selection: Option<Range<usize>>,
    rag_kind_filter: RagKindFilter,
    error: Option<String>,
}

impl GongwenApp {
    pub(crate) fn open_ai_workbench(&mut self, selection: Option<Range<usize>>) {
        if self.doc().busy {
            return;
        }
        // 有待审提案时，AI 按钮优先回到提案，避免用户无意间启动第二次改写。
        if let Some(proposal) = self.doc_mut().ai_proposal.as_mut() {
            proposal.open = true;
            return;
        }
        let baselines = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.list(&ManuscriptFilter::default()).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|row| BaselineRow {
                id: row.id,
                title: row.title,
                kind: row.kind,
                status: row.status,
            })
            .collect();
        let has_text = !self.doc().generated_markdown.trim().is_empty();
        let selected_prompt = self
            .config
            .ai_prompt(self.config.last_ai_prompt)
            .filter(|prompt| prompt.applies_to(self.doc().draft.kind))
            .map_or_else(
                || {
                    self.config
                        .ai_prompts
                        .iter()
                        .find(|prompt| prompt.builtin_key == "polish")
                        .map_or(0, |prompt| prompt.id)
                },
                |prompt| prompt.id,
            );
        self.ai_workbench = Some(AiWorkbench {
            workflow: if has_text {
                AiWorkflowKind::Polish
            } else {
                AiWorkflowKind::Material
            },
            baselines,
            baseline_id: None,
            baseline: None,
            similar_strategy: SimilarStrategy::default(),
            changes: Vec::new(),
            raw_material: String::new(),
            facts: Vec::new(),
            facts_source: String::new(),
            selected_prompt,
            custom_instruction: String::new(),
            polish_scope: if selection.is_some() {
                PolishScope::Selection
            } else {
                PolishScope::Full
            },
            selection,
            rag_kind_filter: self.doc().rag_kind_filter,
            error: None,
        });
    }

    pub(crate) fn ai_workbench_window(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.ai_workbench.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("ai_workbench_anim"));
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut execute = false;
        let mut load_baseline = None;
        let pending = pending_metadata(&self.doc().draft);
        let has_current = !self.doc().generated_markdown.trim().is_empty();
        let rag_enabled = self.config.rag.enabled;

        let win = egui::Window::new("AI 起草工作台")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(860.0)
            .default_height(650.0)
            .min_width(720.0)
            .min_height(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (kind, icon) in [
                        (AiWorkflowKind::Similar, theme::Icon::Copy),
                        (AiWorkflowKind::Knowledge, theme::Icon::Book),
                        (AiWorkflowKind::Material, theme::Icon::FilePlus),
                        (AiWorkflowKind::Polish, theme::Icon::WandSparkles),
                    ] {
                        let selected = state.workflow == kind;
                        if theme::nav_button(ui, selected, icon, kind.label()).clicked() {
                            state.workflow = kind;
                            state.error = None;
                        }
                    }
                });
                let body_height = (ui.available_height() - 58.0).max(260.0);
                egui::ScrollArea::vertical()
                    .id_salt("ai_workbench_body")
                    .max_height(body_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            workflow_header(ui, state.workflow);
                            ui.add_space(8.0);
                            match state.workflow {
                                AiWorkflowKind::Similar => {
                                    similar_ui(ui, &mut state, &mut load_baseline)
                                }
                                AiWorkflowKind::Knowledge | AiWorkflowKind::Material => {
                                    material_ui(ui, &mut state, rag_enabled)
                                }
                                AiWorkflowKind::Polish => polish_ui(
                                    ui,
                                    &mut state,
                                    &self.config.ai_prompts,
                                    self.doc().draft.kind,
                                    has_current,
                                ),
                            }
                        });

                        if state.workflow != AiWorkflowKind::Polish && !pending.is_empty() {
                            ui.add_space(8.0);
                            theme::card().fill(theme::warn_soft()).show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    theme::chip(ui, "待核实", theme::warn(), theme::warn_soft());
                                    ui.label(format!(
                                        "当前公文要素仍缺：{}。可以先起草，但正式导出会被暂停。",
                                        pending.join("、")
                                    ));
                                });
                            });
                        }
                        if let Some(error) = &state.error {
                            ui.add_space(8.0);
                            ui.colored_label(theme::danger(), error);
                        }
                    });
                ui.add_space(10.0);
                ui.separator();
                ui.horizontal(|ui| {
                    let label = if state.workflow == AiWorkflowKind::Polish || has_current {
                        "生成修改提案"
                    } else {
                        "开始起草"
                    };
                    if theme::primary_icon_button(ui, theme::Icon::Sparkles, label).clicked() {
                        execute = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak("AI 结果先经过格式校验；已有正文不会被直接覆盖");
                    });
                });
            });
        if let Some(window) = win {
            theme::window_enter_anim(ctx, egui::Id::new("ai_workbench_anim"), &window.response);
        }

        if let Some(id) = load_baseline {
            self.load_ai_baseline(&mut state, id);
        }
        if execute && self.execute_ai_workbench(&mut state) {
            return;
        }
        if !open || cancel {
            return;
        }
        self.ai_workbench = Some(state);
    }

    fn load_ai_baseline(&mut self, state: &mut AiWorkbench, id: i64) {
        let record = self
            .manuscript_store
            .as_mut()
            .and_then(|store| store.get(id).ok().flatten());
        let Some(record) = record else {
            state.error = Some("无法读取所选基准稿，请刷新稿件库后重试。".into());
            return;
        };
        let current = self.doc().draft.clone();
        state.changes = draft_changes(&record.snapshot, &current)
            .into_iter()
            .map(|change| ChangeItem {
                field: change.label.to_string(),
                before: change.before,
                after: change.after,
                confirmed: false,
            })
            .collect();
        state.baseline = Some(LoadedBaseline {
            id: record.id,
            title: record.title,
            markdown: record.content_markdown,
        });
        state.error = None;
    }

    fn execute_ai_workbench(&mut self, state: &mut AiWorkbench) -> bool {
        state.error = None;
        let current_has_text = !self.doc().generated_markdown.trim().is_empty();
        let request = match state.workflow {
            AiWorkflowKind::Similar => {
                let Some(baseline) = &state.baseline else {
                    state.error = Some("请先选择一篇基准稿。".into());
                    return false;
                };
                if state.changes.iter().any(|item| {
                    item.field.trim().is_empty() || item.after.trim().is_empty() || !item.confirmed
                }) {
                    state.error = Some("请补齐并确认变化清单；不适用的空白行可以删除。".into());
                    return false;
                }
                if state.changes.is_empty() && state.raw_material.trim().is_empty() {
                    state.error = Some("请至少填写一项本次变化或补充材料。".into());
                    return false;
                }
                let changes = state
                    .changes
                    .iter()
                    .map(|item| format!("- {}：{} → {}", item.field, item.before, item.after))
                    .collect::<Vec<_>>()
                    .join("\n");
                AiTaskRequest {
                    kind: AiWorkflowKind::Similar,
                    label: format!("{}《{}》", state.similar_strategy.label(), baseline.title),
                    instruction: format!(
                        "仿写策略：{}\n{}\n\n【已确认变化清单】\n{}",
                        state.similar_strategy.label(),
                        state.similar_strategy.instruction(),
                        if changes.is_empty() {
                            "（无）"
                        } else {
                            &changes
                        }
                    ),
                    material: state.raw_material.trim().to_string(),
                    baseline: baseline.markdown.clone(),
                    use_rag: false,
                    review_before_apply: current_has_text,
                }
            }
            AiWorkflowKind::Knowledge | AiWorkflowKind::Material => {
                if state.raw_material.trim().is_empty() {
                    state.error = Some("请先填写主题、材料或写作要求。".into());
                    return false;
                }
                if state.facts.is_empty() {
                    state.error = Some("请先点击“提取事实单”，确认材料中的事实后再起草。".into());
                    return false;
                }
                if state.facts_source != state.raw_material {
                    state.error =
                        Some("原始材料在提取事实单后发生了变化，请重新提取并确认事实单。".into());
                    return false;
                }
                if state.facts.iter().any(|fact| {
                    fact.label.trim().is_empty() || fact.value.trim().is_empty() || !fact.confirmed
                }) {
                    state.error = Some("事实单仍有未填写或未确认的项目。".into());
                    return false;
                }
                if state.workflow == AiWorkflowKind::Knowledge && !self.config.rag.enabled {
                    state.error = Some("知识库检索尚未启用，请先在设置中配置并启用 RAG。".into());
                    return false;
                }
                let fact_sheet = state
                    .facts
                    .iter()
                    .map(|fact| format!("- {}：{}", fact.label.trim(), fact.value.trim()))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.doc_mut().rag_kind_filter = state.rag_kind_filter;
                AiTaskRequest {
                    kind: state.workflow,
                    label: state.workflow.label().to_string(),
                    instruction: String::new(),
                    material: format!(
                        "【已确认事实单——优先于原始材料】\n{fact_sheet}\n\n【原始材料与写作要求】\n{}",
                        state.raw_material.trim()
                    ),
                    baseline: String::new(),
                    use_rag: state.workflow == AiWorkflowKind::Knowledge,
                    review_before_apply: current_has_text,
                }
            }
            AiWorkflowKind::Polish => {
                if !current_has_text {
                    state.error = Some("当前没有可润色的审校稿。".into());
                    return false;
                }
                let preset = self
                    .config
                    .ai_prompt(state.selected_prompt)
                    .map(|prompt| prompt.instruction.trim())
                    .unwrap_or("");
                let mut instruction = [preset, state.custom_instruction.trim()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if instruction.is_empty() {
                    instruction = "只做公文格式规整，不改写事实和措辞。".into();
                }
                if state.polish_scope == PolishScope::Selection {
                    let Some(range) = state.selection.clone() else {
                        state.error = Some("当前没有有效选区，请改为“全文”或重新选择正文。".into());
                        return false;
                    };
                    let current = &self.doc().generated_markdown;
                    let Some(selected) = current.get(range) else {
                        state.error = Some("正文已发生变化，原选区失效，请重新打开工作台。".into());
                        return false;
                    };
                    instruction.push_str(&format!(
                        "\n\n【唯一允许修改的原文片段】\n{selected}\n【范围硬约束】只修改上述片段，片段之外的文字和 Markdown 标记必须逐字保持不变；仍须返回完整 Markdown。"
                    ));
                }
                AiTaskRequest {
                    kind: AiWorkflowKind::Polish,
                    label: self
                        .config
                        .ai_prompt(state.selected_prompt)
                        .map_or("受控润色", |prompt| prompt.name.as_str())
                        .to_string(),
                    instruction,
                    material: String::new(),
                    baseline: String::new(),
                    use_rag: false,
                    review_before_apply: true,
                }
            }
        };
        self.draft_page().start_ai_task(request);
        true
    }

    pub(crate) fn ai_proposal_window(&mut self, ctx: &egui::Context) {
        if self.docs.is_empty() || !self.showing_doc() {
            return;
        }
        let index = self.active_doc;
        let Some(mut proposal) = self.docs[index].ai_proposal.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("ai_proposal_anim"));
            return;
        };
        if !proposal.open {
            self.docs[index].ai_proposal = Some(proposal);
            return;
        }
        let mut keep = true;
        let mut accept = false;
        let mut discard = false;
        let old = ContentSnapshot::new(
            self.docs[index].draft.clone(),
            proposal.before.clone(),
            String::new(),
        );
        let new = ContentSnapshot::new(
            self.docs[index].draft.clone(),
            proposal.result.markdown.clone(),
            String::new(),
        );
        let report = manuscript_diff(&old, &new);
        let win = egui::Window::new("审阅 AI 修改提案")
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(1020.0)
            .default_height(720.0)
            .min_width(760.0)
            .min_height(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(&proposal.label);
                    theme::chip(
                        ui,
                        &format!("{} 处正文变化", report.body.changed_count),
                        theme::accent(),
                        theme::accent_soft(),
                    );
                });
                ui.weak("左侧为当前审校稿，右侧为 AI 提案；接受前不会覆盖正文或自动导出。 ");
                if !proposal.fact_changes.is_empty() {
                    ui.add_space(8.0);
                    theme::card().fill(theme::danger_soft()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            theme::chip(ui, "关键事实变化", theme::danger(), theme::danger_soft());
                            ui.label(format!(
                                "检测到 {} 项单位、人员、日期、数字或文件依据变化。",
                                proposal.fact_changes.len()
                            ));
                        });
                        egui::Grid::new("ai_fact_changes")
                            .striped(true)
                            .num_columns(3)
                            .show(ui, |ui| {
                                ui.strong("类型");
                                ui.strong("变化");
                                ui.strong("内容");
                                ui.end_row();
                                for change in &proposal.fact_changes {
                                    ui.label(change.kind.label());
                                    ui.colored_label(
                                        if change.change == crate::ai_guard::FactChangeKind::Added {
                                            theme::success()
                                        } else {
                                            theme::danger()
                                        },
                                        change.change.label(),
                                    );
                                    ui.label(&change.value);
                                    ui.end_row();
                                }
                            });
                        ui.checkbox(
                            &mut proposal.fact_changes_confirmed,
                            "我已逐项核对，上述事实变化符合本次修改要求",
                        );
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can_accept =
                        proposal.fact_changes.is_empty() || proposal.fact_changes_confirmed;
                    if theme::primary_icon_button_enabled(
                        ui,
                        can_accept,
                        theme::Icon::SquareCheck,
                        "接受提案",
                    )
                    .on_disabled_hover_text("请先确认关键事实变化")
                    .clicked()
                    {
                        accept = true;
                    }
                    if ui
                        .add(theme::secondary_icon_button(theme::Icon::Trash, "放弃提案"))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button("稍后处理").clicked() {
                        proposal.open = false;
                    }
                });
                ui.separator();
                let _ = manuscript_diff_ui(
                    ui,
                    &report,
                    &mut proposal.view,
                    &DiffViewConfig {
                        old_label: "当前审校稿",
                        new_label: "AI 修改提案",
                        allow_jump: false,
                        allow_export: false,
                    },
                );
            });
        if let Some(window) = win {
            theme::window_enter_anim(ctx, egui::Id::new("ai_proposal_anim"), &window.response);
        }
        if accept {
            let label = proposal.label.clone();
            GongwenApp::take_generated(&mut self.docs[index], proposal.result);
            self.status = format!("已接受“{label}”修改提案，并重新执行审校。 ");
            return;
        }
        if discard {
            self.status = format!("已放弃“{}”修改提案，当前审校稿未改变。", proposal.label);
            return;
        }
        if !keep {
            proposal.open = false;
        }
        self.docs[index].ai_proposal = Some(proposal);
    }
}

fn workflow_header(ui: &mut egui::Ui, workflow: AiWorkflowKind) {
    let (title, description) = match workflow {
        AiWorkflowKind::Similar => (
            "选择基准稿并确认变化",
            "程序只把基准稿当作结构与通用表达参考，旧单位、旧日期、旧数据不会自动继承。",
        ),
        AiWorkflowKind::Knowledge => (
            "依据主题和知识库起草",
            "先确认本次事实，再检索同文种知识片段；检索内容不能覆盖已确认事实。",
        ),
        AiWorkflowKind::Material => (
            "把零散材料整理成公文",
            "先从材料形成可编辑事实单，确认后再按当前文种和版式生成 Markdown。",
        ),
        AiWorkflowKind::Polish => (
            "在事实锁定下修改现有稿件",
            "AI 只生成修改提案；单位、人名、日期、数字和文件依据发生变化时必须另行确认。",
        ),
    };
    ui.heading(title);
    ui.weak(description);
}

fn similar_ui(ui: &mut egui::Ui, state: &mut AiWorkbench, load_baseline: &mut Option<i64>) {
    ui.label(egui::RichText::new("1. 基准稿").strong());
    let selected = state
        .baseline_id
        .and_then(|id| state.baselines.iter().find(|row| row.id == id))
        .map_or("请选择稿件".to_string(), |row| row.title.clone());
    egui::ComboBox::from_id_salt("ai_baseline")
        .selected_text(selected)
        .width(ui.available_width().min(620.0))
        .show_ui(ui, |ui| {
            if state.baselines.is_empty() {
                ui.weak("稿件库中暂无可选稿件");
            }
            for row in &state.baselines {
                let label = format!(
                    "{} · {} · {}",
                    row.title,
                    row.kind.label(),
                    row.status.label()
                );
                if ui
                    .selectable_value(&mut state.baseline_id, Some(row.id), label)
                    .clicked()
                {
                    *load_baseline = Some(row.id);
                }
            }
        });
    if let Some(baseline) = &state.baseline {
        ui.weak(format!(
            "已载入《{}》（稿件 #{}）",
            baseline.title, baseline.id
        ));
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new("2. 仿写方式").strong());
    egui::ComboBox::from_id_salt("similar_strategy")
        .selected_text(state.similar_strategy.label())
        .show_ui(ui, |ui| {
            for strategy in SimilarStrategy::ALL {
                ui.selectable_value(&mut state.similar_strategy, strategy, strategy.label());
            }
        });
    ui.weak(state.similar_strategy.instruction());
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("3. 变化清单").strong());
        if ui.button("＋ 添加变化").clicked() {
            state.changes.push(ChangeItem {
                field: String::new(),
                before: String::new(),
                after: String::new(),
                confirmed: false,
            });
        }
    });
    if state.changes.is_empty() {
        ui.weak("选择基准稿后会自动比较当前公文要素；正文中的业务变化可手动添加。 ");
    }
    let mut remove = None;
    egui::Grid::new("ai_change_sheet")
        .striped(true)
        .num_columns(5)
        .min_col_width(90.0)
        .show(ui, |ui| {
            ui.strong("事项");
            ui.strong("原稿");
            ui.strong("本次");
            ui.strong("确认");
            ui.label("");
            ui.end_row();
            for (index, item) in state.changes.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(&mut item.field).desired_width(120.0));
                ui.add(egui::TextEdit::singleline(&mut item.before).desired_width(170.0));
                ui.add(egui::TextEdit::singleline(&mut item.after).desired_width(170.0));
                ui.checkbox(&mut item.confirmed, "已核实");
                if theme::icon_button(ui, theme::Icon::X, "删除此项").clicked() {
                    remove = Some(index);
                }
                ui.end_row();
            }
        });
    if let Some(index) = remove {
        state.changes.remove(index);
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new("4. 补充材料（可选）").strong());
    ui.add(
        egui::TextEdit::multiline(&mut state.raw_material)
            .desired_rows(5)
            .hint_text("填写本次新增背景、任务、时间节点和办理要求……"),
    );
}

fn material_ui(ui: &mut egui::Ui, state: &mut AiWorkbench, rag_enabled: bool) {
    if state.workflow == AiWorkflowKind::Knowledge {
        ui.horizontal_wrapped(|ui| {
            theme::chip(
                ui,
                if rag_enabled {
                    "知识库已启用"
                } else {
                    "知识库未启用"
                },
                if rag_enabled {
                    theme::success()
                } else {
                    theme::danger()
                },
                if rag_enabled {
                    theme::success_soft()
                } else {
                    theme::danger_soft()
                },
            );
            egui::ComboBox::from_id_salt("workbench_rag_kind")
                .selected_text(state.rag_kind_filter.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut state.rag_kind_filter,
                        RagKindFilter::Follow,
                        RagKindFilter::Follow.label(),
                    );
                    ui.selectable_value(
                        &mut state.rag_kind_filter,
                        RagKindFilter::All,
                        RagKindFilter::All.label(),
                    );
                    for kind in TemplateKind::ALL {
                        ui.selectable_value(
                            &mut state.rag_kind_filter,
                            RagKindFilter::Only(kind),
                            kind.label(),
                        );
                    }
                });
        });
        ui.add_space(6.0);
    }
    ui.label(egui::RichText::new("1. 主题、材料与写作要求").strong());
    ui.add(
        egui::TextEdit::multiline(&mut state.raw_material)
            .desired_rows(8)
            .hint_text("可粘贴工作要点、会议记录、上级要求、时间数据和附件说明……"),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("2. 起草事实单").strong());
        if ui.button("提取事实单").clicked() {
            state.facts = facts_from_material(&state.raw_material);
            state.facts_source = state.raw_material.clone();
            state.error = None;
        }
        if ui.button("＋ 添加事实").clicked() {
            state.facts.push(ConfirmedFact {
                label: String::new(),
                value: String::new(),
                confirmed: false,
            });
        }
    });
    if state.facts.is_empty() {
        ui.weak("事实单由程序按材料原文拆分，不会调用模型；每项需人工确认。 ");
        return;
    }
    let mut remove = None;
    egui::ScrollArea::vertical()
        .id_salt("ai_fact_sheet")
        .max_height(230.0)
        .show(ui, |ui| {
            egui::Grid::new("ai_fact_grid")
                .striped(true)
                .num_columns(4)
                .show(ui, |ui| {
                    ui.strong("事实项");
                    ui.strong("确认内容");
                    ui.strong("状态");
                    ui.label("");
                    ui.end_row();
                    for (index, fact) in state.facts.iter_mut().enumerate() {
                        ui.add(egui::TextEdit::singleline(&mut fact.label).desired_width(130.0));
                        ui.add(egui::TextEdit::singleline(&mut fact.value).desired_width(380.0));
                        ui.checkbox(&mut fact.confirmed, "已核实");
                        if theme::icon_button(ui, theme::Icon::X, "删除此项").clicked() {
                            remove = Some(index);
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(index) = remove {
        state.facts.remove(index);
    }
}

fn polish_ui(
    ui: &mut egui::Ui,
    state: &mut AiWorkbench,
    prompts: &[crate::models::AiPrompt],
    current_kind: TemplateKind,
    has_current: bool,
) {
    if !has_current {
        ui.colored_label(theme::danger(), "当前审校稿为空，不能进行受控润色。 ");
        return;
    }
    ui.label(egui::RichText::new("1. 修改范围").strong());
    ui.horizontal(|ui| {
        ui.radio_value(&mut state.polish_scope, PolishScope::Full, "全文");
        ui.add_enabled_ui(state.selection.is_some(), |ui| {
            ui.radio_value(&mut state.polish_scope, PolishScope::Selection, "当前选区");
        });
        if let Some(range) = &state.selection {
            ui.weak(format!(
                "已锁定 {} 字节选区",
                range.end.saturating_sub(range.start)
            ));
        }
    });
    ui.add_space(8.0);
    ui.label(egui::RichText::new("2. 润色目标").strong());
    let selected = prompts
        .iter()
        .find(|prompt| prompt.id == state.selected_prompt)
        .map_or("请选择", |prompt| prompt.name.as_str());
    egui::ComboBox::from_id_salt("controlled_polish_prompt")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for prompt in prompts
                .iter()
                .filter(|prompt| prompt.applies_to(current_kind))
            {
                ui.selectable_value(&mut state.selected_prompt, prompt.id, &prompt.name);
            }
        });
    ui.add_space(8.0);
    ui.label(egui::RichText::new("3. 补充要求（可选）").strong());
    ui.add(
        egui::TextEdit::multiline(&mut state.custom_instruction)
            .desired_rows(5)
            .hint_text("例如：压缩第二部分，但不得改变任务、责任单位和完成时限。"),
    );
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        theme::chip(ui, "事实锁定", theme::success(), theme::success_soft());
        ui.weak("单位、人名、日期、数字、文件依据将自动比对；变化必须另行确认。 ");
    });
}

fn facts_from_material(material: &str) -> Vec<ConfirmedFact> {
    let mut facts = Vec::new();
    for raw in material
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(30)
    {
        let line = raw.trim_start_matches(['-', '*', '•']).trim();
        let split = line
            .find(['：', ':'])
            .filter(|index| *index > 0 && *index < 20);
        let (label, value) = match split {
            Some(index) => (
                line[..index].trim().to_string(),
                line[index + line[index..].chars().next().unwrap().len_utf8()..]
                    .trim()
                    .to_string(),
            ),
            None => (format!("材料要点 {}", facts.len() + 1), line.to_string()),
        };
        if !value.is_empty() {
            facts.push(ConfirmedFact {
                label,
                value,
                confirmed: false,
            });
        }
    }
    facts
}

fn pending_metadata(input: &DraftInput) -> Vec<&'static str> {
    let mut pending = Vec::new();
    if input.title_hint.trim().is_empty() {
        pending.push("标题提示");
    }
    if input.profile.security_level.trim().is_empty() {
        pending.push("密级");
    }
    match input.kind {
        TemplateKind::OfficialLetter | TemplateKind::PhoneNotice => {
            if input.profile.issuing_unit.trim().is_empty()
                && input.profile.joint_issuing_units.trim().is_empty()
            {
                pending.push("发文单位");
            }
            if input.profile.recipient.trim().is_empty() {
                pending.push("主送单位");
            }
        }
        TemplateKind::MeetingAgenda => {
            if input.meeting_time.trim().is_empty() {
                pending.push("会议时间");
            }
            if input.profile.meeting_location.trim().is_empty() {
                pending.push("会议地点");
            }
            if input.attendees.trim().is_empty() {
                pending.push("参加人员");
            }
        }
        TemplateKind::WhitePaper | TemplateKind::RedHeadApproval => {
            if input.profile.reporting_leaders.trim().is_empty() {
                pending.push("呈报领导");
            }
        }
        TemplateKind::PlainDocument => {}
    }
    pending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_sheet_preserves_source_words() {
        let facts = facts_from_material("时间：2026年8月21日\n- 报送3份材料");
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].label, "时间");
        assert_eq!(facts[0].value, "2026年8月21日");
        assert_eq!(facts[1].value, "报送3份材料");
        assert!(facts.iter().all(|fact| !fact.confirmed));
    }
}
