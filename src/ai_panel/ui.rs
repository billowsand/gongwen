//! AI 侧栏的界面：标题行、任务流卡片、输入区。挂在 `DraftPage` 上。

use super::skill_job::asking_labels;
use super::{ProposalSummary, TurnState};
use crate::agent::skill::POLISH;
use crate::app::{DraftAction, GongwenApp};
use crate::draft_page::{AiProposal, DraftPage};
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

/// 用主题底色覆盖滚动边缘，形成向背景消隐的柔和过渡；不增加交互层。
fn conversation_edge_fades(ui: &egui::Ui, rect: egui::Rect, top: bool, bottom: bool) {
    let height = 32.0_f32.min(rect.height() / 2.0);
    if height <= 0.0 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    for (enabled, edge, direction) in [(top, rect.top(), 1.0), (bottom, rect.bottom(), -1.0)] {
        if !enabled {
            continue;
        }
        let base = mesh.vertices.len() as u32;
        // 多段平滑曲线比单条线性渐变更接近轻柔虚化，中心文字仍然清晰。
        for step in 0..=8 {
            let t = step as f32 / 8.0;
            let alpha = 1.0 - t * t * (3.0 - 2.0 * t);
            let color = theme::canvas().gamma_multiply(alpha);
            let y = edge + direction * height * t;
            mesh.colored_vertex(egui::pos2(rect.left(), y), color);
            mesh.colored_vertex(egui::pos2(rect.right(), y), color);
            if step > 0 {
                let index = base + (step - 1) * 2;
                mesh.add_triangle(index, index + 1, index + 2);
                mesh.add_triangle(index + 1, index + 3, index + 2);
            }
        }
    }
    ui.painter().with_clip_rect(rect).add(mesh);
}

/// 卡片上的按钮动作，画完一轮后统一执行，避免一边借着轮次一边改状态。
pub(super) enum CardAction {
    Stop,
    Accept,
    Discard,
    /// 丢弃这一轮的检查点（不再能接着跑）。
    DiscardRun(u64),
    /// 从最近的检查点接着跑（崩溃 / 停止 / 出错后）。
    Resume(u64),
    /// 丢掉检查点，从第一步重来。
    Restart(u64),
    /// 从某个旧检查点重跑。
    RerunFrom(u64, i64),
    Review,
    Workspace(u64),
    Rerun(u64),
    /// 交上这一轮选择题的回答。
    Answer(u64),
    /// 按当前框内大纲与修改要求再优化一轮，不进入正文起草。
    RefineOutline(u64),
    /// 打开审校抽屉（审核类技能把有改法的问题放在那里）。
    OpenDrawer,
    /// 风格学习的结果存成风格档案。
    SaveStyle(u64),
    /// 到 AI 管理页「风格」细改。
    OpenStyles,
    /// 在审阅视图里定位到这段文字（缺口所在句）。
    Locate(String),
    /// 撤回 AI 对某处缺口的概括：(轮次, 缺口)。
    Revert(u64, usize),
}

impl DraftPage<'_> {
    /// 右侧 AI 侧栏。`create_ui` 在审校抽屉之前调用，侧栏因此贴在最右边。
    pub(crate) fn ai_panel_side(&mut self, ui: &mut egui::Ui) {
        let mut open = self.doc.ai_panel.open;
        egui::Panel::right("ai_panel_v1")
            .default_size(PANEL_DEFAULT_WIDTH)
            .size_range(PANEL_MIN_WIDTH..=PANEL_MAX_WIDTH)
            .frame(theme::panel(theme::canvas(), 12))
            .show_collapsible(ui, &mut open, |ui| {
                // 侧栏宽度只由用户拖动决定。内容画在不回报尺寸的子 Ui 里、超出的裁掉：
                // 否则哪行字没折行，egui 会按内容把侧栏撑到最大宽度，左半截还会被正文区盖住。
                let rect = ui.max_rect();
                let mut content = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                content.set_clip_rect(rect.intersect(ui.clip_rect()));
                self.ai_panel_ui(&mut content);
                ui.allocate_rect(rect, egui::Sense::hover());
            });
        // 标题行的「收起」直接改 `ai_panel.open`，两边取与。
        self.doc.ai_panel.open = open && self.doc.ai_panel.open;
    }

    /// 打开或收起侧栏。带着选区打开时，选区锁进输入区、技能定在润色。
    pub(crate) fn toggle_ai_panel(&mut self, selection: Option<Range<usize>>) {
        let selection = selection
            .filter(|range| !range.is_empty())
            .and_then(|range| {
                self.doc
                    .generated_markdown
                    .get(range.clone())
                    .map(|text| (range, text.to_string()))
            });
        let rag_enabled = self.config.rag.enabled;
        let panel = &mut self.doc.ai_panel;
        if !panel.composer.primed {
            // 设置里启用了知识库，用到知识库的技能就默认检索；不然启用了也白启用。
            panel.composer.use_rag = rag_enabled;
            panel.composer.primed = true;
        }
        if panel.open && selection.is_none() {
            panel.open = false;
            return;
        }
        panel.open = true;
        // 打开时重读一次：用户可能刚改过配置目录里的技能文件。
        let _ = panel.reload_skills();
        if let Some(selection) = selection {
            panel.composer.selection = Some(selection);
            if panel
                .skills
                .iter()
                .any(|skill| skill.id == POLISH && skill.enabled)
            {
                panel.composer.skill = Some(POLISH.to_string());
            }
        }
        // 与审校抽屉同在右侧，同时开着中央区太窄。
        self.doc.result_drawer_open = false;
    }

    fn ai_panel_ui(&mut self, ui: &mut egui::Ui) {
        self.ai_panel_header(ui);
        ui.add_space(8.0);
        egui::Panel::bottom("ai_panel_composer")
            .show_separator_line(false)
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
            let title = self.doc.ai_panel.session_title();
            if !self.doc.ai_panel.turns.is_empty() {
                ui.label(
                    egui::RichText::new(crate::agent::tools::short(&title, 14))
                        .small()
                        .color(theme::text_muted()),
                )
                .on_hover_text(title);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::titlebar_icon_button(ui, true, theme::Icon::X, "收起 AI 侧栏").clicked() {
                    self.doc.ai_panel.open = false;
                }
                let history = theme::titlebar_icon_button(ui, true, theme::Icon::History, "历史会话");
                if history.clicked() {
                    self.refresh_ai_sessions();
                }
                egui::Popup::menu(&history)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        ui.set_min_width(300.0);
                        self.ai_sessions_menu(ui);
                    });
                if theme::titlebar_icon_button(ui, true, theme::Icon::Plus, "新会话：从空白开始，追问不带之前的轮次")
                    .clicked()
                {
                    self.new_ai_session();
                }
                let options =
                    theme::titlebar_icon_button(ui, true, theme::Icon::Settings, "会话与技能管理");
                egui::Popup::menu(&options).show(|ui| {
                    let can_compact = !self.doc.busy
                        && self
                            .doc
                            .ai_panel
                            .turns
                            .iter()
                            .filter(|turn| turn.id > self.doc.ai_panel.session.compacted_upto)
                            .count()
                            > super::history::KEEP_RECENT;
                    if ui
                        .add_enabled(
                            can_compact,
                            theme::menu_item(theme::Icon::Archive, "压缩会话"),
                        )
                        .on_hover_text(
                            "最近 3 轮之前的并成一段摘要，后面的追问只带摘要；原始记录仍可翻看。也可以在输入框敲 /compact",
                        )
                        .on_disabled_hover_text("会话还短，不用压缩")
                        .clicked()
                    {
                        self.start_compact();
                        ui.close();
                    }
                    if ui
                        .add(theme::menu_item(theme::Icon::WandSparkles, "管理技能…"))
                        .on_hover_text("AI 管理页「技能」：启用停用、复制改写、新建、导入导出")
                        .clicked()
                    {
                        self.actions.push(DraftAction::OpenSkillSettings);
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
                ui.weak("今天想写些什么？");
                ui.add_space(10.0);
                for example in [
                    "起草一份冬季森林防火的通知……",
                    "根据以下会议纪要整理成通知……",
                    "仿照去年的通知写今年的……",
                    "压缩到800字，不改任务和时限",
                    "签发前帮我审一下",
                    "核对一下文中引用的文件名称和文号",
                    "把文中的日期都列出来，看看哪些已经过了",
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
        // 「从这里重跑」的候选：先把各轮存下的检查点读出来（绘制里不查库）。
        let checkpoints = self.checkpoint_index();
        let doc = &mut *self.doc;
        let running = doc.ai_panel.running();
        let scroll = egui::ScrollArea::vertical()
            .id_salt(("ai_panel_turns", doc.key))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let session = &doc.ai_panel.session;
                if !session.summary.trim().is_empty() {
                    let folded = doc
                        .ai_panel
                        .turns
                        .iter()
                        .filter(|turn| turn.id <= session.compacted_upto)
                        .count();
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("前面 {folded} 轮已压缩成摘要，追问只带摘要"))
                            .small()
                            .color(theme::text_soft()),
                    )
                    .id_salt(("ai_session_summary", doc.key))
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&session.summary)
                                .small()
                                .color(theme::text_soft()),
                        );
                    });
                    ui.add_space(8.0);
                }
                for turn in &mut doc.ai_panel.turns {
                    request_bubble(ui, turn);
                    ui.add_space(6.0);
                    egui::Frame::NONE.inner_margin(10).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        turn_card(
                            ui,
                            turn,
                            doc.ai_proposal.as_mut(),
                            checkpoints
                                .get(&turn.id)
                                .map(Vec::as_slice)
                                .unwrap_or_default(),
                            &mut action,
                        );
                    });
                    ui.add_space(12.0);
                }
            });
        // 只淡出仍有内容的方向，读到首尾时完整消息和操作按钮保持清晰。
        conversation_edge_fades(
            ui,
            scroll.inner_rect,
            scroll.state.offset.y > 0.5,
            scroll.content_size.y - scroll.state.offset.y > scroll.inner_rect.height() + 0.5,
        );
        if running {
            // 计时与流式文字都要动；忙碌时外壳只按 100 ms 刷新，这里提到 50 ms。
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        match action {
            Some(CardAction::Stop) => self.stop_ai_task(),
            Some(CardAction::Accept) => {
                GongwenApp::accept_ai_proposal(self.doc, self.config, self.status);
            }
            Some(CardAction::Discard) => GongwenApp::discard_ai_proposal(self.doc, self.status),
            Some(CardAction::Resume(id)) => self.resume_checkpoint(id),
            Some(CardAction::Restart(id)) => {
                self.drop_checkpoints(id);
                if let Some(request) = self
                    .doc
                    .ai_panel
                    .turns
                    .iter()
                    .find(|turn| turn.id == id)
                    .and_then(|turn| turn.request.clone())
                {
                    self.start_panel_request(request);
                }
            }
            Some(CardAction::DiscardRun(id)) => self.drop_checkpoints(id),
            Some(CardAction::RerunFrom(id, seq)) => self.rerun_from_checkpoint(id, seq),
            Some(CardAction::Review) => {
                if let Some(turn) = self
                    .doc
                    .ai_panel
                    .turns
                    .iter()
                    .rev()
                    .find(|turn| matches!(turn.state, TurnState::Proposed(_)))
                {
                    self.doc.ai_panel.workspace_view = Some(turn.id);
                    self.doc.ai_panel.workspace_compare = true;
                }
            }
            Some(CardAction::Workspace(id)) => {
                self.doc.ai_panel.workspace_view = Some(id);
                self.doc.ai_panel.workspace_compare = false;
            }
            Some(CardAction::OpenDrawer) => self.open_result_drawer(),
            Some(CardAction::SaveStyle(id)) => self.save_learned_style(id),
            Some(CardAction::OpenStyles) => self.actions.push(DraftAction::OpenStyleSettings),
            Some(CardAction::Locate(text)) => match self.doc.ai_proposal.as_mut() {
                Some(proposal) => {
                    proposal.open = true;
                    proposal.locate = Some(text);
                }
                None => *self.status = "提案已不在了，没法定位。".into(),
            },
            Some(CardAction::Revert(turn_id, gap_id)) => self.revert_generalized(turn_id, gap_id),
            Some(CardAction::Answer(id)) => {
                let asking = self
                    .doc
                    .ai_panel
                    .turns
                    .iter()
                    .any(|turn| turn.id == id && turn.state == TurnState::Asking);
                if asking {
                    self.answer_suspended(id);
                } else {
                    self.apply_research_answers(id);
                }
            }
            Some(CardAction::RefineOutline(id)) => self.refine_outline(id),
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

    /// 风格学习的结果存成风格档案（用户点了「保存为风格」）。
    fn save_learned_style(&mut self, turn_id: u64) {
        let Some(profile) = self
            .doc
            .ai_panel
            .turn_mut(turn_id)
            .and_then(|turn| turn.style.clone())
        else {
            return;
        };
        let saved = crate::agent::style::StyleBook::load().and_then(|mut book| {
            book.upsert(profile.clone());
            book.save()
        });
        match saved {
            Ok(()) => {
                if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) {
                    turn.style = None;
                    turn.notes.push(format!(
                        "已保存为风格「{}」，可在 AI 管理页「风格」里细改、设为默认或停用。",
                        profile.name
                    ));
                }
                let _ = self.doc.ai_panel.reload_skills();
                *self.status = format!("已保存风格「{}」。", profile.name);
            }
            Err(error) => *self.status = format!("保存风格失败：{error:#}"),
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
    let width = (ui.available_width() * 0.88 - 20.0).max(0.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        egui::Frame::new()
            .fill(theme::accent_soft())
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
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
            });
    });
}

fn turn_card(
    ui: &mut egui::Ui,
    turn: &mut super::AiTurn,
    proposal: Option<&mut AiProposal>,
    checkpoints: &[crate::manuscript::ai_checkpoints::CheckpointSummary],
    action: &mut Option<CardAction>,
) {
    let seconds = turn.elapsed().as_secs_f32();
    let chars = turn.content.chars().count();
    // 耗时先从右往左占位，状态与阶段提示在剩下的宽度里从左排。阶段提示（如「第 2 轮 · 查
    // 「……」」）可能很长，只能截断：横排不折行，放任它会把卡片连同侧栏一起撑宽，
    // 侧栏左半截被正文区盖住。
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(if chars == 0 {
                    format!("{seconds:.0} s")
                } else {
                    format!("{chars} 字 · {seconds:.0} s")
                })
                .small()
                .color(theme::text_muted()),
            );
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                match &turn.state {
                    TurnState::Waiting => {
                        theme::spinner(ui, 14.0, theme::accent());
                        phase_label(
                            ui,
                            if turn.phase.is_empty() {
                                "等待模型响应…"
                            } else {
                                &turn.phase
                            },
                        );
                    }
                    // 思考型模型可能先想几十秒才出第一个正文字（qwen3.5 9B 实测 27 s），
                    // 这段时间要明说在做什么，不然看着像卡住了。研究式起草在预研、检索时
                    // 也是这个状态，有阶段提示就显示提示。
                    TurnState::Streaming if turn.content.is_empty() => {
                        theme::spinner(ui, 14.0, theme::accent());
                        phase_label(
                            ui,
                            if turn.phase.is_empty() {
                                "正在思考…"
                            } else {
                                &turn.phase
                            },
                        );
                    }
                    TurnState::Streaming => {
                        theme::chip(
                            ui,
                            "生成中 · 未校验",
                            theme::text_soft(),
                            theme::surface_sunk(),
                        );
                        if !turn.phase.is_empty() {
                            phase_label(ui, &turn.phase);
                        }
                    }
                    TurnState::Checking => {
                        theme::spinner(ui, 14.0, theme::accent());
                        phase_label(ui, &turn.phase);
                    }
                    TurnState::Asking => {
                        theme::chip(ui, "等你回答", theme::warn(), theme::warn_soft());
                    }
                    TurnState::Proposed(_) => {
                        theme::chip(ui, "待确认", theme::accent(), theme::accent_soft());
                    }
                    TurnState::Reported { .. } => {
                        theme::chip(ui, "问题清单", theme::accent(), theme::accent_soft());
                    }
                    TurnState::Accepted => {
                        let text = if turn.excluded_hunks > 0 {
                            format!("已写入正文（排除 {} 处）", turn.excluded_hunks)
                        } else {
                            "已写入正文".to_string()
                        };
                        theme::chip(ui, &text, theme::success(), theme::success_soft());
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
                    TurnState::Interrupted => {
                        theme::chip(ui, "已中断", theme::warn(), theme::warn_soft());
                    }
                    TurnState::Expired => {
                        theme::chip(ui, "已过期", theme::text_soft(), theme::surface_sunk());
                    }
                }
            });
        });
    });

    if !turn.provenance.is_empty() {
        ui.label(
            egui::RichText::new(&turn.provenance)
                .small()
                .color(theme::text_muted()),
        );
    }
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

    if !turn.steps.is_empty() {
        egui::CollapsingHeader::new(
            egui::RichText::new(format!("过程（{} 步）", turn.steps.len()))
                .small()
                .color(theme::text_soft()),
        )
        .id_salt(("ai_turn_steps", turn.id))
        .default_open(true)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("ai_turn_steps_scroll", turn.id))
                .max_height(180.0)
                .stick_to_bottom(turn.state.running())
                .show(ui, |ui| {
                    for step in &turn.steps {
                        ui.label(egui::RichText::new(step).small().color(theme::text_soft()));
                    }
                });
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

    if turn.is_workspace {
        if ui.button("查看工作稿").clicked() {
            *action = Some(CardAction::Workspace(turn.id));
        }
    } else if !turn.content.is_empty() {
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

    let state = turn.state.clone();
    match &state {
        TurnState::Waiting | TurnState::Streaming | TurnState::Checking => {
            if turn
                .replies
                .first()
                .is_some_and(|reply| reply.outline_base.is_some())
            {
                outline_ui(ui, turn, action);
            }
            ui.add_space(6.0);
            if ui
                .add(theme::secondary_icon_button(theme::Icon::Square, "停止"))
                .clicked()
            {
                *action = Some(CardAction::Stop);
            }
        }
        TurnState::Asking => {
            ui.add_space(6.0);
            let labels = asking_labels(&turn.questions);
            questions_ui(ui, turn, labels, action);
        }
        TurnState::Proposed(summary) => {
            ui.add_space(6.0);
            theme::hairline(ui);
            ui.add_space(6.0);
            proposal_actions(ui, summary, proposal, action);
            if let Some(research) = &turn.research {
                ui.add_space(6.0);
                ledger_ui(ui, turn.id, research, action);
            }
            if !turn.questions.is_empty() {
                ui.add_space(6.0);
                theme::hairline(ui);
                ui.add_space(6.0);
                questions_ui(
                    ui,
                    turn,
                    ("这几处要你确认", "按回答修订", "保留待核实"),
                    action,
                );
            }
        }
        TurnState::Reported { fixes } => {
            ui.add_space(6.0);
            findings_ui(ui, &turn.findings, *fixes, action);
            if turn.style.is_some() {
                ui.horizontal_wrapped(|ui| {
                    if theme::primary_icon_button(ui, theme::Icon::Save, "保存为风格")
                        .on_hover_text("存进风格档案，以后起草、改写时按文种与场合自动选用")
                        .clicked()
                    {
                        *action = Some(CardAction::SaveStyle(turn.id));
                    }
                    if ui
                        .add(theme::secondary_icon_button(
                            theme::Icon::Settings,
                            "到 AI 管理页细改",
                        ))
                        .clicked()
                    {
                        *action = Some(CardAction::OpenStyles);
                    }
                });
            }
            rerun_button(ui, turn, "重新做一遍", action);
        }
        TurnState::Stopped => {
            ui.add_space(6.0);
            ui.weak(format!("已停止，生成了 {chars} 字；半截输出不能采用。"));
            resume_actions(ui, turn, checkpoints, action);
            rerun_button(ui, turn, "重新生成", action);
        }
        TurnState::Failed(error) => {
            ui.add_space(6.0);
            ui.colored_label(theme::danger(), error);
            resume_actions(ui, turn, checkpoints, action);
            rerun_button(ui, turn, "重试", action);
        }
        TurnState::Interrupted => {
            ui.add_space(6.0);
            ui.weak("程序关闭时这一轮还没跑完。");
            resume_actions(ui, turn, checkpoints, action);
            rerun_button(ui, turn, "重新生成", action);
        }
        TurnState::Expired => {
            ui.add_space(6.0);
            ui.weak("提案交出之后正文改过了，这份提案不能再接受。");
            rerun_button(ui, turn, "重新生成", action);
        }
        _ => {}
    }
    if !turn.state.running() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(turn.usage.label(turn.elapsed()))
                .small()
                .color(theme::text_muted()),
        );
    }
}

/// 阶段提示：一行放不下就截断，悬停看全文。
fn phase_label(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).weak()).truncate())
        .on_hover_text(text);
}

/// 审核类技能的问题清单：按分组列出，有改法的注明已进审校抽屉。不改稿。
fn findings_ui(
    ui: &mut egui::Ui,
    findings: &[crate::agent::board::Finding],
    fixes: usize,
    action: &mut Option<CardAction>,
) {
    if findings.is_empty() {
        ui.label("没有发现问题。");
        return;
    }
    let mut groups: Vec<&str> = Vec::new();
    for finding in findings {
        if !groups.contains(&finding.group.as_str()) {
            groups.push(&finding.group);
        }
    }
    for group in groups {
        let items: Vec<_> = findings.iter().filter(|f| f.group == group).collect();
        ui.label(egui::RichText::new(format!("{group}（{}）", items.len())).strong());
        for finding in items {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("· {}", finding.text));
                if let Some(fix) = &finding.fix {
                    ui.weak(format!("→ 改为「{}」", fix.after));
                }
            });
            if !finding.excerpt.is_empty() {
                ui.weak(format!("  原文：{}", finding.excerpt));
            }
            if !finding.source.is_empty() {
                ui.label(
                    egui::RichText::new(format!("  {}", finding.source))
                        .small()
                        .color(theme::text_muted()),
                );
            }
        }
        ui.add_space(4.0);
    }
    ui.horizontal_wrapped(|ui| {
        if fixes > 0 {
            ui.weak(format!("{fixes} 条有改法的已放进审校抽屉，逐条采纳。"));
            if ui.small_button("打开审校抽屉").clicked() {
                *action = Some(CardAction::OpenDrawer);
            }
        } else {
            ui.weak("只出清单，不改稿。");
        }
    });
}

/// 缺口题在正文里的位置：所在小节、所在整句与缺口字面。研究式起草交付后从快照里查，
/// 流程挂起时从黑板里查。
struct GapContext {
    section: String,
    sentence: String,
    literal: String,
}

fn gap_context(
    question: &crate::agent::clarify::Question,
    research: Option<&super::ResearchSnapshot>,
    run: Option<&super::SkillRun>,
) -> Option<GapContext> {
    let crate::agent::clarify::Target::Gap(id) = question.target else {
        return None;
    };
    let (ledger, text) = match (research, run) {
        (Some(research), _) => (&research.ledger, research.raw.as_str()),
        (None, Some(run)) => {
            let board = &run.suspension.checkpoint.board;
            (&board.ledger, board.workspace.as_str())
        }
        _ => return None,
    };
    let gap = ledger.get(id)?;
    let pos = text
        .find(&gap.sentence)
        .or_else(|| text.find(&gap.literal))
        .unwrap_or(0);
    Some(GapContext {
        section: crate::agent::gaps::section_of(text, pos),
        sentence: crate::agent::evidence::strip_citations(&gap.sentence)
            .trim()
            .to_string(),
        literal: gap.literal.clone(),
    })
}

/// 所在整句，缺口高亮；点一下在审阅视图里定位。
fn gap_context_ui(ui: &mut egui::Ui, context: &GapContext, action: &mut Option<CardAction>) {
    if !context.section.is_empty() {
        ui.label(
            egui::RichText::new(format!("所在：{}", context.section))
                .small()
                .color(theme::text_muted()),
        );
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let plain = egui::TextFormat {
        font_id: font.clone(),
        color: theme::text_soft(),
        ..Default::default()
    };
    let marked = egui::TextFormat {
        font_id: font,
        color: theme::warn(),
        underline: egui::Stroke::new(1.0, theme::warn()),
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    let sentence = &context.sentence;
    match sentence.find(&context.literal) {
        Some(pos) => {
            job.append(&sentence[..pos], 0.0, plain.clone());
            job.append(&context.literal, 0.0, marked);
            job.append(&sentence[pos + context.literal.len()..], 0.0, plain);
        }
        None => job.append(sentence, 0.0, plain),
    }
    let response = egui::Frame::new()
        .fill(theme::surface_sunk())
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).sense(egui::Sense::click()))
        })
        .inner;
    if response
        .on_hover_text("在审阅视图里定位到这一句")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        *action = Some(CardAction::Locate(context.sentence.clone()));
    }
}

/// 一道题现在的作答状态，题卡右上角显示。
fn reply_state(
    question: &crate::agent::clarify::Question,
    draft: &super::ReplyDraft,
) -> (&'static str, egui::Color32, egui::Color32) {
    if draft.skip {
        let label = if matches!(question.target, crate::agent::clarify::Target::Element(_)) {
            "先不定"
        } else {
            "保留待核实"
        };
        (label, theme::text_muted(), theme::surface_sunk())
    } else if !draft.custom.trim().is_empty() {
        ("已填写", theme::success(), theme::success_soft())
    } else if draft.choice.is_some() {
        ("已选", theme::success(), theme::success_soft())
    } else if question.skippable {
        ("未答", theme::warn(), theme::warn_soft())
    } else {
        ("必答", theme::danger(), theme::danger_soft())
    }
}

/// 题卡里的一枚选项。选中的实心强调色带勾；AI 建议用淡强调色底，和程序给的选项分开。
fn option_pill(ui: &mut egui::Ui, label: &str, selected: bool, suggestion: bool) -> egui::Response {
    let (fill, stroke, color) = if selected {
        (theme::accent(), theme::accent(), theme::accent_text())
    } else if suggestion {
        (theme::accent_soft(), theme::accent_soft(), theme::accent())
    } else {
        (theme::surface(), theme::border_strong(), theme::text())
    };
    let text = egui::RichText::new(label).color(color);
    let button = if selected {
        egui::Button::image_and_text(theme::Icon::Check.image(), text)
            .image_tint_follows_text_color(true)
    } else {
        egui::Button::new(text)
    };
    ui.add(
        button
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(egui::CornerRadius::same(255))
            .min_size(egui::vec2(0.0, 26.0)),
    )
}

/// 选择题。程序推荐的选项已经替用户选上；写了自己的答案就以它为准。
///
/// 每道题一张卡：题号与作答状态、题目、所在句、程序给的选项、AI 建议写法（点了填进输入框，
/// 可以接着改）、自己写、保留待核实，自上而下一层一层分开。
fn questions_ui(
    ui: &mut egui::Ui,
    turn: &mut super::AiTurn,
    (heading, submit, skip): (&str, &str, &str),
    action: &mut Option<CardAction>,
) {
    if turn.state == TurnState::Asking
        && turn.run.as_ref().is_some_and(|run| {
            crate::agent::outline::step_index(&run.skill, &run.suspension).is_some()
        })
    {
        outline_ui(ui, turn, action);
        return;
    }
    use crate::agent::clarify::Action;
    let asking = turn.state == TurnState::Asking;
    let super::AiTurn {
        id: turn_id,
        questions,
        replies,
        research,
        run,
        ..
    } = turn;
    let total = questions.len();
    let answered = replies
        .iter()
        .filter(|draft| !draft.skip && (draft.choice.is_some() || !draft.custom.trim().is_empty()))
        .count();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(heading).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::caption(ui, &format!("已答 {answered} / {total}"));
        });
    });
    for (index, (question, draft)) in questions.iter().zip(replies.iter_mut()).enumerate() {
        ui.add_space(8.0);
        let context = gap_context(question, research.as_ref(), run.as_deref());
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::caption(ui, &format!("第 {} 题 / 共 {total} 题", index + 1));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, fg, bg) = reply_state(question, draft);
                    theme::chip(ui, label, fg, bg);
                });
            });
            ui.add_space(2.0);
            ui.add(egui::Label::new(egui::RichText::new(&question.text).strong()).wrap());
            if let Some(context) = &context {
                ui.add_space(4.0);
                gap_context_ui(ui, context, action);
            }

            let fixed: Vec<usize> = (0..question.choices.len())
                .filter(|i| !matches!(question.choices[*i].action, Action::Suggest(_)))
                .collect();
            let suggested: Vec<usize> = (0..question.choices.len())
                .filter(|i| matches!(question.choices[*i].action, Action::Suggest(_)))
                .collect();
            if !fixed.is_empty() {
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    for &i in &fixed {
                        let choice = &question.choices[i];
                        let selected = draft.choice == Some(i)
                            && draft.custom.trim().is_empty()
                            && !draft.skip;
                        let label = if choice.recommended {
                            format!("{}（推荐）", choice.label)
                        } else {
                            choice.label.clone()
                        };
                        let response = option_pill(ui, &label, selected, false);
                        let response = if choice.detail.is_empty() {
                            response
                        } else {
                            response.on_hover_text(&choice.detail)
                        };
                        if response.clicked() {
                            // 再点一次取消选择。
                            draft.choice = (!selected).then_some(i);
                            draft.custom.clear();
                            draft.skip = false;
                        }
                    }
                });
            }
            if !suggested.is_empty() {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add(
                        theme::Icon::Sparkles
                            .image()
                            .tint(theme::accent())
                            .fit_to_exact_size(egui::vec2(13.0, 13.0)),
                    );
                    theme::caption(ui, "AI 建议 · 点一下填进下面的框，可以接着改");
                });
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    for &i in &suggested {
                        let Action::Suggest(value) = &question.choices[i].action else {
                            continue;
                        };
                        let selected = !draft.skip && draft.custom.trim() == value.as_str();
                        if option_pill(ui, value, selected, true)
                            .on_hover_text(&question.choices[i].detail)
                            .clicked()
                        {
                            draft.custom = value.clone();
                            draft.choice = None;
                            draft.skip = false;
                        }
                    }
                });
            }
            if let Some(hint) = &question.custom_hint {
                ui.add_space(8.0);
                if !question.choices.is_empty() {
                    theme::caption(ui, "或者自己写");
                    ui.add_space(2.0);
                }
                // 预填了多行内容（如大纲）就给多行框，在原文上改。
                let edit = if question.prefill.contains('\n') {
                    egui::TextEdit::multiline(&mut draft.custom)
                        .desired_rows(question.prefill.lines().count().clamp(3, 14))
                } else {
                    egui::TextEdit::singleline(&mut draft.custom)
                };
                let response = ui.add(
                    edit.hint_text(hint.as_str())
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 5)),
                );
                if response.changed() && !draft.custom.is_empty() {
                    draft.skip = false;
                }
            }
            if question.skippable {
                // 六要素题跳过不是「按现有信息写」，而是正文留占位、事后不再问。
                let skip = if matches!(question.target, crate::agent::clarify::Target::Element(_)) {
                    "先不定，正文留待核实"
                } else {
                    skip
                };
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(
                            &mut draft.skip,
                            egui::RichText::new(skip)
                                .size(theme::font_sizes::SMALL)
                                .color(theme::text_soft()),
                        );
                    });
                });
            }
        });
    }
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::SquareCheck, submit).clicked() {
            *action = Some(CardAction::Answer(*turn_id));
        }
        if !asking {
            theme::caption(
                ui,
                "AI 把回答写进所在段落，改完仍是提案；没答的保留待核实，不会替你猜",
            );
        }
    });
}

/// 大纲编辑与提示词优化共用同一份当前文本，确认前可反复迭代。
fn outline_ui(ui: &mut egui::Ui, turn: &mut super::AiTurn, action: &mut Option<CardAction>) {
    let running = turn.state.running();
    let Some(reply) = turn.replies.first_mut() else {
        return;
    };
    ui.label(egui::RichText::new("确认大纲").strong());
    theme::caption(
        ui,
        "可直接修改大纲，也可填写修改要求让 AI 优化；满意后再开始写作。",
    );
    ui.add_space(6.0);
    let rows = reply.custom.lines().count().clamp(4, 14);
    ui.add(
        egui::TextEdit::multiline(&mut reply.custom)
            .id_salt(("outline_text", turn.id))
            .desired_rows(rows)
            .desired_width(f32::INFINITY)
            .hint_text("每行一章：章标题：要点"),
    );
    ui.add_space(8.0);
    if !reply.outline_candidate.is_empty() {
        ui.collapsing("AI 优化结果（未覆盖你的手工修改）", |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut reply.outline_candidate)
                    .desired_rows(5)
                    .desired_width(f32::INFINITY),
            );
            if ui.button("使用这份优化结果").clicked() {
                reply.custom = std::mem::take(&mut reply.outline_candidate);
            }
        });
    }
    if running {
        theme::caption(
            ui,
            "正在优化，可以继续手工修改；返回结果不会覆盖新增的修改。",
        );
    }
    ui.label("修改要求");
    ui.add(
        egui::TextEdit::multiline(&mut reply.outline_instruction)
            .id_salt(("outline_instruction", turn.id))
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text("例如：合并前两章，增加国内外做法对比，把对策建议细分为三项"),
    );
    ui.add_space(8.0);
    let has_outline = !reply.custom.trim().is_empty();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !running && has_outline && !reply.outline_instruction.trim().is_empty(),
                theme::secondary_icon_button(theme::Icon::Sparkles, "优化大纲"),
            )
            .clicked()
        {
            *action = Some(CardAction::RefineOutline(turn.id));
        }
        ui.add_enabled_ui(!running && has_outline, |ui| {
            if theme::primary_icon_button(ui, theme::Icon::SquareCheck, "确认大纲，开始写作")
                .clicked()
            {
                *action = Some(CardAction::Answer(turn.id));
            }
        });
    });
}

/// 核实清单：每处缺口怎么处理的、证据出自哪里。
fn ledger_ui(
    ui: &mut egui::Ui,
    turn_id: u64,
    research: &super::ResearchSnapshot,
    action: &mut Option<CardAction>,
) {
    use crate::agent::gaps::GapStatus;
    if research.ledger.gaps.is_empty() && research.sources.is_empty() {
        return;
    }
    let (resolved, handled, pending) = research.ledger.counts();
    egui::CollapsingHeader::new(
        egui::RichText::new(format!(
            "核实清单：已补全或概括 {resolved} · 你已处理 {handled} · 待确认 {pending} · 证据 {} 段",
            research.sources.len()
        ))
        .small()
        .color(theme::text_soft()),
    )
    .id_salt(("ai_turn_ledger", turn_id))
    .default_open(false)
    .show(ui, |ui| {
        let source_of = |id: &usize| {
            research
                .sources
                .iter()
                .find(|(source, _)| source == id)
                .map_or_else(|| format!("[K{id}]"), |(_, label)| label.clone())
        };
        for gap in &research.ledger.gaps {
            let (mark, color, status) = match &gap.status {
                GapStatus::Resolved(ids) => (
                    "✓",
                    theme::success(),
                    format!(
                        "已补全，来源 {}",
                        ids.iter().map(source_of).collect::<Vec<_>>().join("、")
                    ),
                ),
                GapStatus::Answered(value) => ("✓", theme::success(), format!("你填了：{value}")),
                GapStatus::Kept => ("✓", theme::success(), "你确认保留原文".into()),
                GapStatus::Skipped => ("·", theme::text_muted(), "保留待核实".into()),
                GapStatus::Dropped => ("·", theme::text_muted(), "你选了删去".into()),
                GapStatus::Generalized(_) => (
                    "~",
                    theme::accent(),
                    format!("知识库查不到，AI 写成了概括表述：{}", gap.sentence.trim()),
                ),
                GapStatus::NoAnswer => ("?", theme::warn(), "知识库里没找到".into()),
                GapStatus::Open => ("?", theme::warn(), "等你提供".into()),
            };
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(color, mark);
                ui.label(
                    egui::RichText::new(format!("{}「{}」", gap.kind.label(), gap.hint)).small(),
                );
                ui.label(
                    egui::RichText::new(status)
                        .small()
                        .color(theme::text_soft()),
                );
                if matches!(gap.status, GapStatus::Generalized(_))
                    && ui
                        .small_button("撤回")
                        .on_hover_text("恢复成「【待核实】」占位，改为出题问你")
                        .clicked()
                {
                    *action = Some(CardAction::Revert(turn_id, gap.id));
                }
            });
        }
        if !research.sources.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("证据").small().strong());
            for (id, label) in &research.sources {
                ui.label(
                    egui::RichText::new(format!("[K{id}] {label}"))
                        .small()
                        .color(theme::text_soft()),
                );
            }
        }
    });
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

/// 中断 / 停止 / 出错后的「接着跑 · 从头重来 · 丢弃」（内核加固第 4 期）。
///
/// 有检查点才给「接着跑」，并注明停在哪一步、什么时候存的；找不到检查点就只留原来的
/// 「重新生成」，不假装能续。三个按钮只在侧栏发起的轮次上出现。
fn resume_actions(
    ui: &mut egui::Ui,
    turn: &super::AiTurn,
    checkpoints: &[crate::manuscript::ai_checkpoints::CheckpointSummary],
    action: &mut Option<CardAction>,
) {
    if turn.request.is_none() {
        return;
    }
    ui.add_space(6.0);
    if turn.resumable.is_none() {
        ui.weak("这一轮没有留下检查点，只能重新生成。");
        return;
    }
    let label = turn.resumable.as_deref().unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::SquareCheck, "接着跑")
            .on_hover_text(format!("从最近一步接着跑（{label}）"))
            .clicked()
        {
            *action = Some(CardAction::Resume(turn.id));
        }
        if ui
            .add(theme::secondary_icon_button(
                theme::Icon::Refresh,
                "从头重来",
            ))
            .on_hover_text("丢掉检查点，从第一步重新跑一遍")
            .clicked()
        {
            *action = Some(CardAction::Restart(turn.id));
        }
        if ui
            .add(theme::secondary_icon_button(theme::Icon::Eraser, "丢弃"))
            .on_hover_text("删掉这一轮的检查点，不再能接着跑")
            .clicked()
        {
            *action = Some(CardAction::DiscardRun(turn.id));
        }
    });
    ui.label(
        egui::RichText::new(format!("可接着跑：{label}"))
            .small()
            .color(theme::text_muted()),
    );
    rerun_from_list(ui, turn.id, checkpoints, action);
}

/// 「从这里重跑」：列出差几步之后存的检查点，点一个从那一步之后重跑。
///
/// 只在正文没被改过时才允许（判定在 `DraftPage::rerun_from_checkpoint`）；这里先说明这一点，
/// 点下去被拒了状态栏会说原因。
fn rerun_from_list(
    ui: &mut egui::Ui,
    turn_id: u64,
    checkpoints: &[crate::manuscript::ai_checkpoints::CheckpointSummary],
    action: &mut Option<CardAction>,
) {
    if checkpoints.len() < 2 {
        // 只有一份就是「接着跑」那一份，没有「选一个」的余地。
        return;
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new(
        egui::RichText::new(format!("从这里重跑（存了 {} 份）", checkpoints.len()))
            .small()
            .color(theme::text_soft()),
    )
    .id_salt(("ai_turn_rerun", turn_id))
    .default_open(false)
    .show(ui, |ui| {
        ui.weak("从这一步之后重跑，会覆盖它之后的产物。正文改过就不允许。");
        for stored in checkpoints.iter().rev() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("#{} {}", stored.seq, stored.label))
                        .small()
                        .color(theme::text_muted()),
                );
                if stored.partial {
                    theme::chip(ui, "证据已省略", theme::warn(), theme::warn_soft());
                }
                if ui.small_button("重跑").clicked() {
                    *action = Some(CardAction::RerunFrom(turn_id, stored.seq));
                }
            });
        }
    });
}

/// 结果卡下半部：摘要 chip、关键事实确认与采用 / 对照 / 放弃。
pub(super) fn proposal_actions(
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
        // 卡片上核对的是完整清单；审阅窗里排除改动后清单变短，那边的勾选作废逻辑
        // 靠 confirmed_facts 比对，这里同样记下核对的是哪份。
        let mut checked = proposal.fact_changes_confirmed;
        if ui
            .checkbox(&mut checked, "我已逐项核对，上述事实变化符合本次要求")
            .changed()
        {
            proposal.fact_changes_confirmed = checked;
            proposal.confirmed_facts = if checked {
                proposal.fact_changes.clone()
            } else {
                Vec::new()
            };
        }
    }
    let can_accept = proposal.fact_changes.is_empty() || proposal.fact_changes_confirmed;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        // 在审阅窗里挑过「不要这处」的，卡片上看得到这次只接受其余改动。
        let label = match (summary.was_empty, proposal.excluded.len()) {
            (true, 0) => "采用为正文".to_string(),
            (false, 0) => "接受提案".to_string(),
            (true, n) => format!("采用为正文（排除 {n} 处）"),
            (false, n) => format!("接受提案（排除 {n} 处）"),
        };
        if theme::primary_icon_button_enabled(ui, can_accept, theme::Icon::SquareCheck, &label)
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
