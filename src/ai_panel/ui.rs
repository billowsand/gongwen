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

/// 卡片上的按钮动作，画完一轮后统一执行，避免一边借着轮次一边改状态。
enum CardAction {
    Stop,
    Accept,
    Discard,
    Review,
    Rerun(u64),
    /// 交上这一轮选择题的回答。
    Answer(u64),
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
            .show_collapsible(ui, &mut open, |ui| self.ai_panel_ui(ui));
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
                if theme::icon_button(ui, theme::Icon::X, "收起 AI 侧栏").clicked() {
                    self.doc.ai_panel.open = false;
                }
                let history = theme::icon_button(ui, theme::Icon::History, "历史会话");
                if history.clicked() {
                    self.refresh_ai_sessions();
                }
                egui::Popup::menu(&history)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        ui.set_min_width(300.0);
                        self.ai_sessions_menu(ui);
                    });
                if theme::icon_button(ui, theme::Icon::Plus, "新会话：从空白开始，追问不带之前的轮次")
                    .clicked()
                {
                    self.new_ai_session();
                }
                ui.menu_button("•••", |ui| {
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
                ui.weak("在下面写要求，Ctrl+Enter 发送；技能会按你的话自动选");
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
        let doc = &mut *self.doc;
        let running = doc.ai_panel.running();
        egui::ScrollArea::vertical()
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
    turn: &mut super::AiTurn,
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
            // 这段时间要明说在做什么，不然看着像卡住了。研究式起草在预研、检索时
            // 也是这个状态，有阶段提示就显示提示。
            TurnState::Streaming if turn.content.is_empty() => {
                theme::spinner(ui, 14.0, theme::accent());
                ui.weak(if turn.phase.is_empty() {
                    "正在思考…"
                } else {
                    &turn.phase
                });
            }
            TurnState::Streaming => {
                theme::chip(
                    ui,
                    "生成中 · 未校验",
                    theme::text_soft(),
                    theme::surface_sunk(),
                );
                if !turn.phase.is_empty() {
                    ui.weak(&turn.phase);
                }
            }
            TurnState::Checking => {
                theme::spinner(ui, 14.0, theme::accent());
                ui.weak(&turn.phase);
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
            TurnState::Interrupted => {
                theme::chip(ui, "已中断", theme::warn(), theme::warn_soft());
            }
            TurnState::Expired => {
                theme::chip(ui, "已过期", theme::text_soft(), theme::surface_sunk());
            }
        }
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

    let state = turn.state.clone();
    match &state {
        TurnState::Waiting | TurnState::Streaming | TurnState::Checking => {
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
            rerun_button(ui, turn, "重新生成", action);
        }
        TurnState::Failed(error) => {
            ui.add_space(6.0);
            ui.colored_label(theme::danger(), error);
            rerun_button(ui, turn, "重试", action);
        }
        TurnState::Interrupted => {
            ui.add_space(6.0);
            ui.weak("程序关闭时这一轮还没跑完。");
            rerun_button(ui, turn, "重新生成", action);
        }
        TurnState::Expired => {
            ui.add_space(6.0);
            ui.weak("提案交出之后正文改过了，这份提案不能再接受。");
            rerun_button(ui, turn, "重新生成", action);
        }
        _ => {}
    }
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
    run: Option<&super::skill_job::SkillRun>,
) -> Option<GapContext> {
    let crate::agent::clarify::Target::Gap(id) = question.target else {
        return None;
    };
    let (ledger, text) = match (research, run) {
        (Some(research), _) => (&research.ledger, research.raw.as_str()),
        (None, Some(run)) => (&run.board.ledger, run.board.workspace.as_str()),
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

/// 选择题。程序推荐的选项已经替用户选上；写了自己的答案就以它为准。
fn questions_ui(
    ui: &mut egui::Ui,
    turn: &mut super::AiTurn,
    (heading, submit, skip): (&str, &str, &str),
    action: &mut Option<CardAction>,
) {
    let asking = turn.state == TurnState::Asking;
    ui.label(egui::RichText::new(heading).strong());
    let super::AiTurn {
        id: turn_id,
        questions,
        replies,
        research,
        run,
        ..
    } = turn;
    for (question, draft) in questions.iter().zip(replies.iter_mut()) {
        ui.add_space(8.0);
        let context = gap_context(question, research.as_ref(), run.as_deref());
        ui.label(egui::RichText::new(&question.text).strong());
        if let Some(context) = &context {
            gap_context_ui(ui, context, action);
        }
        ui.horizontal_wrapped(|ui| {
            for (index, choice) in question.choices.iter().enumerate() {
                let selected =
                    draft.choice == Some(index) && draft.custom.trim().is_empty() && !draft.skip;
                let label = if choice.recommended {
                    format!("{}（推荐）", choice.label)
                } else {
                    choice.label.clone()
                };
                let response = ui.selectable_label(selected, label);
                let response = if choice.detail.is_empty() {
                    response
                } else {
                    response.on_hover_text(&choice.detail)
                };
                if response.clicked() {
                    // 再点一次取消选择。
                    draft.choice = (!selected).then_some(index);
                    draft.custom.clear();
                    draft.skip = false;
                }
            }
        });
        if let Some(hint) = &question.custom_hint {
            // 预填了多行内容（如大纲）就给多行框，在原文上改。
            let edit = if question.prefill.contains('\n') {
                egui::TextEdit::multiline(&mut draft.custom)
                    .desired_rows(question.prefill.lines().count().clamp(3, 14))
            } else {
                egui::TextEdit::singleline(&mut draft.custom)
            };
            let response = ui.add(edit.hint_text(hint.as_str()).desired_width(f32::INFINITY));
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
            ui.checkbox(&mut draft.skip, skip);
        }
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::SquareCheck, submit).clicked() {
            *action = Some(CardAction::Answer(*turn_id));
        }
        if !asking {
            ui.weak("AI 把回答写进所在段落，改完仍是提案；没回答的保留待核实，不会替你猜");
        }
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
