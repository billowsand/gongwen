//! AI 侧栏的界面：标题行、任务流卡片、输入区。挂在 `DraftPage` 上。

use super::{
    PanelMode, ProposalSummary, TurnRequest, TurnState, locate_selection, polish_instruction,
};
use crate::app::{DraftAction, GongwenApp};
use crate::draft_page::{AiProposal, AiTaskRequest, AiWorkflowKind, DraftPage};
use crate::theme;
use eframe::egui;
use std::ops::Range;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// 侧栏默认宽度与可调范围。
const PANEL_DEFAULT_WIDTH: f32 = 420.0;
const PANEL_MIN_WIDTH: f32 = 340.0;
const PANEL_MAX_WIDTH: f32 = 640.0;
/// 单张卡片里正文区的最大高度；更长的在卡片内滚动，不把任务流撑成一长条。
const CARD_TEXT_MAX_HEIGHT: f32 = 360.0;

/// 卡片上的按钮动作，画完一轮后统一执行，避免一边借着轮次一边改状态。
enum CardAction {
    Stop,
    Accept,
    Discard,
    Review,
    Rerun(u64),
}

impl DraftPage<'_> {
    /// 右侧 AI 侧栏。`create_ui` 在审校抽屉之前调用，侧栏因此贴在最右边。
    pub(crate) fn ai_panel_side(&mut self, ui: &mut egui::Ui) {
        let mut open = self.doc.ai_panel.open;
        egui::Panel::right("ai_panel_v1")
            .default_size(PANEL_DEFAULT_WIDTH)
            .size_range(PANEL_MIN_WIDTH..=PANEL_MAX_WIDTH)
            .frame(theme::panel(theme::canvas(), 12))
            .show_collapsible(ui, &mut open, |ui| self.ai_panel_ui(ui));
        // 标题行的「收起」直接改 `ai_panel.open`，两边取与。
        self.doc.ai_panel.open = open && self.doc.ai_panel.open;
    }

    /// 打开或收起侧栏。带着选区打开时，选区锁进输入区、切到润色。
    pub(crate) fn toggle_ai_panel(&mut self, selection: Option<Range<usize>>) {
        let selection = selection
            .filter(|range| !range.is_empty())
            .and_then(|range| {
                self.doc
                    .generated_markdown
                    .get(range.clone())
                    .map(|text| (range, text.to_string()))
            });
        let has_text = !self.doc.generated_markdown.trim().is_empty();
        let rag_enabled = self.config.rag.enabled;
        let panel = &mut self.doc.ai_panel;
        if !panel.composer.primed {
            // 设置里启用了知识库，起草就默认检索；不然启用了也白启用。
            panel.composer.use_rag = rag_enabled;
            panel.composer.primed = true;
        }
        if panel.open && selection.is_none() {
            panel.open = false;
            return;
        }
        panel.open = true;
        if let Some(selection) = selection {
            panel.composer.selection = Some(selection);
            panel.composer.mode = PanelMode::Polish;
        } else if !has_text && panel.turns.is_empty() {
            panel.composer.mode = PanelMode::Draft;
        }
        // 与审校抽屉同在右侧，同时开着中央区太窄。
        self.doc.result_drawer_open = false;
    }

    fn ai_panel_ui(&mut self, ui: &mut egui::Ui) {
        self.ai_panel_header(ui);
        ui.separator();
        egui::Panel::bottom("ai_panel_composer")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.ai_composer_ui(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.ai_turns_ui(ui));
    }

    fn ai_panel_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(
                theme::Icon::Sparkles
                    .image_sized(16.0)
                    .tint(theme::accent()),
            );
            ui.label(egui::RichText::new("AI 助手").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, theme::Icon::X, "收起 AI 侧栏").clicked() {
                    self.doc.ai_panel.open = false;
                }
                ui.menu_button("•••", |ui| {
                    // 仿写、知识起草与大纲第 ② – ④ 期并入侧栏，在那之前走旧工作台。
                    if ui
                        .add(theme::menu_item(
                            theme::Icon::Copy,
                            "仿照起草 / 知识起草 / 大纲…",
                        ))
                        .on_hover_text("打开原来的 AI 起草工作台")
                        .clicked()
                    {
                        self.actions
                            .push(DraftAction::OpenAiWorkbench { selection: None });
                        ui.close();
                    }
                    let can_clear = self
                        .doc
                        .ai_panel
                        .turns
                        .iter()
                        .any(|turn| !turn.state.running());
                    if ui
                        .add_enabled(can_clear, theme::menu_item(theme::Icon::Eraser, "清空记录"))
                        .clicked()
                    {
                        self.doc.ai_panel.clear_history();
                        ui.close();
                    }
                });
            });
        });
    }

    fn ai_turns_ui(&mut self, ui: &mut egui::Ui) {
        if self.doc.ai_panel.turns.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.weak("在下面写要求，Ctrl+Enter 发送");
                ui.add_space(10.0);
                for example in [
                    "润色：压缩第二部分，不改任务和时限",
                    "润色：语气改成上行文",
                    "起草：根据以下材料写一份通知……",
                ] {
                    ui.label(
                        egui::RichText::new(example)
                            .small()
                            .color(theme::text_muted()),
                    );
                }
            });
            return;
        }
        let mut action = None;
        let doc = &mut *self.doc;
        let running = doc.ai_panel.running();
        egui::ScrollArea::vertical()
            .id_salt(("ai_panel_turns", doc.key))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for turn in &doc.ai_panel.turns {
                    request_bubble(ui, turn);
                    ui.add_space(6.0);
                    theme::card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        turn_card(ui, turn, doc.ai_proposal.as_mut(), &mut action);
                    });
                    ui.add_space(12.0);
                }
            });
        if running {
            // 计时与流式文字都要动；忙碌时外壳只按 100 ms 刷新，这里提到 50 ms。
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        match action {
            Some(CardAction::Stop) => self.stop_ai_task(),
            Some(CardAction::Accept) => {
                GongwenApp::accept_ai_proposal(self.doc, self.status);
            }
            Some(CardAction::Discard) => GongwenApp::discard_ai_proposal(self.doc, self.status),
            Some(CardAction::Review) => {
                if let Some(proposal) = self.doc.ai_proposal.as_mut() {
                    proposal.open = true;
                }
            }
            Some(CardAction::Rerun(id)) => {
                let request = self
                    .doc
                    .ai_panel
                    .turns
                    .iter()
                    .find(|turn| turn.id == id)
                    .and_then(|turn| turn.request.clone());
                if let Some(request) = request {
                    self.start_panel_request(request);
                }
            }
            None => {}
        }
    }

    fn ai_composer_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        theme::hairline(ui);
        ui.add_space(6.0);
        let kind = self.doc.draft.kind;
        let rag_enabled = self.config.rag.enabled;
        let running = self.doc.ai_panel.running();
        let selection_now =
            crate::draft_page::editor_selection(ui.ctx(), &self.doc.generated_markdown)
                .filter(|range| !range.is_empty());
        let mut pick_selection = false;
        {
            let composer = &mut self.doc.ai_panel.composer;
            ui.horizontal_wrapped(|ui| {
                theme::segmented(ui, |ui| {
                    for mode in [PanelMode::Polish, PanelMode::Draft] {
                        if ui
                            .selectable_label(composer.mode == mode, mode.label())
                            .clicked()
                        {
                            composer.mode = mode;
                            composer.error = None;
                        }
                    }
                });
                match composer.mode {
                    PanelMode::Polish => {
                        let selected = composer
                            .preset
                            .and_then(|id| self.config.ai_prompt(id))
                            .map_or("不用预设", |prompt| prompt.name.as_str());
                        egui::ComboBox::from_id_salt(("ai_panel_preset", self.doc.key))
                            .selected_text(selected)
                            .width(120.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut composer.preset, None, "不用预设");
                                for prompt in self
                                    .config
                                    .ai_prompts
                                    .iter()
                                    .filter(|prompt| prompt.applies_to(kind))
                                {
                                    ui.selectable_value(
                                        &mut composer.preset,
                                        Some(prompt.id),
                                        &prompt.name,
                                    );
                                }
                            });
                        if let Some((_, text)) = &composer.selection {
                            if ui
                                .add(theme::removable_tag_button(
                                    &format!("选区 {} 字", text.chars().count()),
                                    false,
                                ))
                                .on_hover_text("只改这段，其余逐字保持不变；点 × 改为全文")
                                .clicked()
                            {
                                composer.selection = None;
                            }
                        } else if selection_now.is_some() {
                            if ui
                                .small_button("用当前选区")
                                .on_hover_text("只改编辑器里选中的那段")
                                .clicked()
                            {
                                pick_selection = true;
                            }
                        } else {
                            ui.weak("全文");
                        }
                    }
                    PanelMode::Draft => {
                        ui.add_enabled_ui(rag_enabled, |ui| {
                            ui.checkbox(&mut composer.use_rag, "检索知识库")
                                .on_disabled_hover_text("知识库尚未启用，可在设置中配置");
                        });
                    }
                }
            });
        }
        if pick_selection && let Some(range) = selection_now {
            let text = self.doc.generated_markdown[range.clone()].to_string();
            self.doc.ai_panel.composer.selection = Some((range, text));
        }
        ui.add_space(4.0);

        // Ctrl+Enter 发送：在输入框画出来之前先把按键吃掉，否则 TextEdit 会把它当换行。
        // Enter 留给换行与内置输入法上屏。
        let input_id = egui::Id::new(("ai_panel_input", self.doc.key));
        let mut send = ui.memory(|memory| memory.has_focus(input_id))
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        let composer = &mut self.doc.ai_panel.composer;
        ui.add(
            egui::TextEdit::multiline(&mut composer.text)
                .id(input_id)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text(composer.mode.hint()),
        );
        let error = composer.error.clone();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if let Some(error) = &error {
                ui.colored_label(theme::danger(), error);
            } else {
                ui.label(
                    egui::RichText::new(format!("{} 发送", theme::primary_shortcut("Enter")))
                        .small()
                        .color(theme::text_muted()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if running {
                    if ui
                        .add(theme::secondary_icon_button(theme::Icon::Square, "停止"))
                        .clicked()
                    {
                        self.stop_ai_task();
                        send = false;
                    }
                } else if theme::primary_icon_button(ui, theme::Icon::Sparkles, "发送").clicked()
                {
                    send = true;
                }
            });
        });
        if send && !self.doc.ai_panel.running() {
            self.send_ai_panel();
        }
    }

    /// 按输入区的内容发起一轮。
    fn send_ai_panel(&mut self) {
        let composer = &self.doc.ai_panel.composer;
        let polish = composer.mode == PanelMode::Polish;
        let request = TurnRequest {
            mode: composer.mode,
            text: composer.text.trim().to_string(),
            selection: composer.selection.clone().filter(|_| polish),
            preset: composer.preset.filter(|_| polish),
            use_rag: composer.use_rag && !polish,
        };
        if self.start_panel_request(request) {
            let composer = &mut self.doc.ai_panel.composer;
            composer.text.clear();
            composer.error = None;
            // 选区只管这一轮：改完之后原文已变，留着只会让下一轮报「选区失效」。
            composer.selection = None;
        }
    }

    /// 把一轮请求翻译成后台任务并发起。发不出去时把原因写进输入区，返回 false。
    fn start_panel_request(&mut self, request: TurnRequest) -> bool {
        match self.panel_task(&request) {
            Ok((task, title, context)) => {
                let prompt = if request.text.is_empty() {
                    title.clone()
                } else {
                    request.text.clone()
                };
                self.doc
                    .ai_panel
                    .push_turn(title, prompt, context, Some(request));
                self.start_ai_task(task);
                if !self.doc.busy {
                    // 没能发起（原因已写进状态栏）：撤掉刚开的这一轮。
                    self.doc.ai_panel.turns.pop();
                    self.doc.ai_panel.composer.error = Some(self.status.clone());
                    return false;
                }
                true
            }
            Err(message) => {
                self.doc.ai_panel.composer.error = Some(message);
                false
            }
        }
    }

    /// 校验请求并组装 `AiTaskRequest`、卡片抬头与上下文 chip。
    fn panel_task(
        &self,
        request: &TurnRequest,
    ) -> Result<(AiTaskRequest, String, Vec<String>), String> {
        if self.doc.read_only() {
            return Err("这篇稿件已发布或归档，只读。".into());
        }
        if self.doc.busy {
            return Err("这篇稿件还有任务在跑，稍等一下。".into());
        }
        let markdown = &self.doc.generated_markdown;
        match request.mode {
            PanelMode::Polish => {
                if markdown.trim().is_empty() {
                    return Err("正文还是空的：先起草或粘贴稿件，或切到「起草」。".into());
                }
                let preset = request.preset.and_then(|id| self.config.ai_prompt(id));
                if request.text.is_empty() && preset.is_none() {
                    return Err("写下修改要求，或选一个润色预设。".into());
                }
                let selected = match &request.selection {
                    Some(selection) => Some(
                        locate_selection(markdown, selection)
                            .ok_or("锁定的选区已被改动或删除，请重新选择。")?,
                    ),
                    None => None,
                };
                let instruction = polish_instruction(
                    preset.map_or("", |prompt| prompt.instruction.as_str()),
                    &request.text,
                    selected.as_deref(),
                );
                let title = match preset {
                    Some(prompt) => format!("润色 · {}", prompt.name),
                    None => "润色".into(),
                };
                let context = vec![match &selected {
                    Some(text) => format!("选区 {} 字", text.chars().count()),
                    None => "全文".into(),
                }];
                Ok((
                    AiTaskRequest {
                        kind: AiWorkflowKind::Polish,
                        label: title.clone(),
                        instruction,
                        material: String::new(),
                        query: String::new(),
                        baseline: String::new(),
                        use_rag: false,
                        review_before_apply: true,
                    },
                    title,
                    context,
                ))
            }
            PanelMode::Draft => {
                if request.text.is_empty() {
                    return Err("写下材料或起草要求。".into());
                }
                let use_rag = request.use_rag && self.config.rag.enabled;
                let mut context = Vec::new();
                if use_rag {
                    context.push("知识库".into());
                }
                if !markdown.trim().is_empty() {
                    context.push("生成新稿提案".into());
                }
                let title = if use_rag {
                    "起草 · 知识库"
                } else {
                    "起草"
                }
                .to_string();
                Ok((
                    AiTaskRequest {
                        kind: if use_rag {
                            AiWorkflowKind::Knowledge
                        } else {
                            AiWorkflowKind::Material
                        },
                        label: title.clone(),
                        instruction: String::new(),
                        material: format!("【原始材料与写作要求】\n{}", request.text),
                        query: request.text.clone(),
                        baseline: String::new(),
                        use_rag,
                        // 红线 1：哪怕是空稿，模型的产物也要用户点一次才落入正文。
                        review_before_apply: true,
                    },
                    title,
                    context,
                ))
            }
        }
    }

    /// 停止正在跑的 AI 任务。
    ///
    /// 阻塞读打断不了，后台线程要等下一行到达才真正停；所以这里不等它，立刻递增
    /// 任务序号、释放「忙」——之后它回投的任何东西都会因序号对不上被丢掉。
    pub(crate) fn stop_ai_task(&mut self) {
        if let Some(cancel) = self.doc.ai_panel.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if !self.doc.ai_panel.running() {
            return;
        }
        self.doc.job_seq += 1;
        self.doc.busy = false;
        self.doc.ai_review_baseline = None;
        self.doc.ai_panel.finish(TurnState::Stopped);
        *self.status = "已停止生成。".into();
    }
}

fn request_bubble(ui: &mut egui::Ui, turn: &super::AiTurn) {
    egui::Frame::new()
        .fill(theme::surface_sunk())
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(&turn.title)
                        .small()
                        .color(theme::text_soft()),
                );
                for chip in &turn.context {
                    theme::chip(ui, chip, theme::text_soft(), theme::surface());
                }
            });
            if turn.prompt != turn.title {
                ui.label(&turn.prompt);
            }
        });
}

fn turn_card(
    ui: &mut egui::Ui,
    turn: &super::AiTurn,
    proposal: Option<&mut AiProposal>,
    action: &mut Option<CardAction>,
) {
    let seconds = turn.elapsed().as_secs_f32();
    let chars = turn.content.chars().count();
    ui.horizontal(|ui| {
        match &turn.state {
            TurnState::Waiting => {
                theme::spinner(ui, 14.0, theme::accent());
                ui.weak(if turn.phase.is_empty() {
                    "等待模型响应…"
                } else {
                    &turn.phase
                });
            }
            // 思考型模型可能先想几十秒才出第一个正文字（qwen3.5 9B 实测 27 s），
            // 这段时间要明说在想，不然看着像卡住了。
            TurnState::Streaming if turn.content.is_empty() => {
                theme::spinner(ui, 14.0, theme::accent());
                ui.weak("正在思考…");
            }
            TurnState::Streaming => {
                theme::chip(
                    ui,
                    "生成中 · 未校验",
                    theme::text_soft(),
                    theme::surface_sunk(),
                );
            }
            TurnState::Checking => {
                theme::spinner(ui, 14.0, theme::accent());
                ui.weak(&turn.phase);
            }
            TurnState::Proposed(_) => {
                theme::chip(ui, "待确认", theme::accent(), theme::accent_soft());
            }
            TurnState::Accepted | TurnState::Applied => {
                theme::chip(ui, "已写入正文", theme::success(), theme::success_soft());
            }
            TurnState::Discarded => {
                theme::chip(ui, "已放弃", theme::text_soft(), theme::surface_sunk());
            }
            TurnState::Superseded => {
                theme::chip(
                    ui,
                    "已被新任务取代",
                    theme::text_soft(),
                    theme::surface_sunk(),
                );
            }
            TurnState::Stopped => {
                theme::chip(ui, "已停止", theme::warn(), theme::warn_soft());
            }
            TurnState::Failed(_) => {
                theme::chip(ui, "失败", theme::danger(), theme::danger_soft());
            }
        }
        if turn.state == TurnState::Streaming && !turn.phase.is_empty() {
            ui.weak(&turn.phase);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{chars} 字 · {seconds:.0} s"))
                    .small()
                    .color(theme::text_muted()),
            );
        });
    });

    for note in &turn.notes {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                theme::Icon::Book
                    .image_sized(12.0)
                    .tint(theme::text_muted()),
            );
            ui.label(egui::RichText::new(note).small().color(theme::text_soft()));
        });
    }

    if !turn.reasoning.is_empty() {
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("思考过程（{} 字）", turn.reasoning.chars().count()))
                .small()
                .color(theme::text_muted()),
        )
        .id_salt(("ai_turn_reasoning", turn.id))
        .default_open(false)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("ai_turn_reasoning_scroll", turn.id))
                .max_height(160.0)
                .stick_to_bottom(turn.state.running())
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(&turn.reasoning)
                            .small()
                            .color(theme::text_muted()),
                    );
                });
        });
    }

    if !turn.content.is_empty() {
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt(("ai_turn_content", turn.id))
            .max_height(CARD_TEXT_MAX_HEIGHT)
            .stick_to_bottom(turn.state.running())
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                markdown_preview(ui, &turn.content, turn.state == TurnState::Streaming);
            });
    }

    match &turn.state {
        TurnState::Waiting | TurnState::Streaming | TurnState::Checking => {
            ui.add_space(6.0);
            if ui
                .add(theme::secondary_icon_button(theme::Icon::Square, "停止"))
                .clicked()
            {
                *action = Some(CardAction::Stop);
            }
        }
        TurnState::Proposed(summary) => {
            ui.add_space(6.0);
            theme::hairline(ui);
            ui.add_space(6.0);
            proposal_actions(ui, summary, proposal, action);
        }
        TurnState::Stopped => {
            ui.add_space(6.0);
            ui.weak(format!("已停止，生成了 {chars} 字；半截输出不能采用。"));
            rerun_button(ui, turn, "重新生成", action);
        }
        TurnState::Failed(error) => {
            ui.add_space(6.0);
            ui.colored_label(theme::danger(), error);
            rerun_button(ui, turn, "重试", action);
        }
        _ => {}
    }
}

fn rerun_button(
    ui: &mut egui::Ui,
    turn: &super::AiTurn,
    label: &str,
    action: &mut Option<CardAction>,
) {
    if turn.request.is_some()
        && ui
            .add(theme::secondary_icon_button(theme::Icon::Refresh, label))
            .clicked()
    {
        *action = Some(CardAction::Rerun(turn.id));
    }
}

/// 结果卡下半部：摘要 chip、关键事实确认与采用 / 对照 / 放弃。
fn proposal_actions(
    ui: &mut egui::Ui,
    summary: &ProposalSummary,
    proposal: Option<&mut AiProposal>,
    action: &mut Option<CardAction>,
) {
    let Some(proposal) = proposal else {
        ui.weak("提案已不在了。");
        return;
    };
    ui.horizontal_wrapped(|ui| {
        let size = if summary.was_empty {
            format!("新稿 {} 字", summary.chars)
        } else {
            format!("提案 {} 字", summary.chars)
        };
        theme::chip(ui, &size, theme::accent(), theme::accent_soft());
        if summary.fact_changes > 0 {
            theme::chip(
                ui,
                &format!("关键事实变化 {} 项", summary.fact_changes),
                theme::danger(),
                theme::danger_soft(),
            );
        }
        if summary.warnings > 0 {
            theme::chip(
                ui,
                &format!("审校提示 {} 条", summary.warnings),
                theme::warn(),
                theme::warn_soft(),
            );
        }
        if summary.truncated {
            theme::chip(ui, "输出被截断", theme::warn(), theme::warn_soft());
        }
    });
    if !proposal.fact_changes.is_empty() {
        egui::CollapsingHeader::new(
            egui::RichText::new("查看关键事实变化")
                .small()
                .color(theme::text_soft()),
        )
        .id_salt("ai_panel_fact_changes")
        .show(ui, |ui| {
            for change in &proposal.fact_changes {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        if change.change == crate::ai_guard::FactChangeKind::Added {
                            theme::success()
                        } else {
                            theme::danger()
                        },
                        change.change.label(),
                    );
                    ui.weak(change.kind.label());
                    ui.label(&change.value);
                });
            }
        });
        ui.checkbox(
            &mut proposal.fact_changes_confirmed,
            "我已逐项核对，上述事实变化符合本次要求",
        );
    }
    let can_accept = proposal.fact_changes.is_empty() || proposal.fact_changes_confirmed;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        let label = if summary.was_empty {
            "采用为正文"
        } else {
            "接受提案"
        };
        if theme::primary_icon_button_enabled(ui, can_accept, theme::Icon::SquareCheck, label)
            .on_disabled_hover_text("请先核对并勾选关键事实变化")
            .clicked()
        {
            *action = Some(CardAction::Accept);
        }
        if !summary.was_empty
            && ui
                .add(theme::secondary_icon_button(
                    theme::Icon::Compare,
                    "查看对照",
                ))
                .clicked()
        {
            *action = Some(CardAction::Review);
        }
        if ui
            .add(theme::secondary_icon_button(theme::Icon::Trash, "放弃"))
            .clicked()
        {
            *action = Some(CardAction::Discard);
        }
    });
}

/// 流式文字的轻量预览：`#` 开头的行加粗，其余照原样。正在生成时末尾跟一个光标。
fn markdown_preview(ui: &mut egui::Ui, text: &str, caret: bool) {
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let last = lines.peek().is_none();
        let mut shown = line.to_string();
        if last && caret {
            shown.push('▍');
        }
        let rich = if line.trim_start().starts_with('#') {
            egui::RichText::new(shown.trim_start_matches('#').trim_start()).strong()
        } else {
            egui::RichText::new(shown)
        };
        ui.label(rich);
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
