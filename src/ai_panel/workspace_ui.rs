//! 左侧 AI 工作稿：流式源码、原地对照与人工采纳，不改写正式正文。
//!
//! 两条呈现路径：锤子稿纸的源码模式把正文与工作稿放进同一张纸，用纸签切换；
//! 其它主题 / 预览模式沿用原来的标签行。两边共用取稿、稿头控件与正文绘制。

use super::TurnState;
use super::ui::{CardAction, proposal_actions};
use crate::app::GongwenApp;
use crate::diff::{ContentSnapshot, manuscript_diff};
use crate::diff_view::{DiffViewConfig, HunkExclude, manuscript_diff_ui};
use crate::draft_page::DraftPage;
use eframe::egui;

/// 锤子稿纸上的视图纸签高度；上沿再留 4pt 木桌，纸签才不贴住功能区。
const SHEET_TAB_HEIGHT: f32 = 26.0;
/// 纸签条总高（纸签 + 上沿木桌缝）。
const SHEET_TAB_STRIP: f32 = SHEET_TAB_HEIGHT + 4.0;
/// 「正文」纸签宽度。
const SHEET_TAB_WIDTH: f32 = 84.0;
/// 「AI 工作稿」纸签略宽，四个字加状态点不挤。
const SHEET_TAB_WIDTH_AI: f32 = 100.0;

/// 一轮工作稿的视图快照：绘制期间不再反复借 `ai_panel`。
struct WorkspaceView {
    id: u64,
    state: TurnState,
    excluded_hunks: usize,
    provenance: String,
    /// Pi 成品是否已经写进正文（与「已撤销写入」区分）。
    pi_written: bool,
    pi_undone: bool,
    /// 工作稿字数：生成中在稿头显示，待采纳时由提案处理条报字数。
    chars: usize,
}

impl DraftPage<'_> {
    /// 当前要显示的工作稿轮次；没有时把「查看工作稿」复位回正文。
    fn workspace_view(&mut self) -> Option<WorkspaceView> {
        let panel = &mut self.doc.ai_panel;
        let Some(id) = panel.workspace_turn_id() else {
            panel.workspace_view = None;
            return None;
        };
        let turn = panel.turns.iter().find(|turn| turn.id == id)?;
        Some(WorkspaceView {
            id,
            state: turn.state.clone(),
            excluded_hunks: turn.excluded_hunks,
            provenance: turn.provenance.clone(),
            pi_written: turn.pi_write.is_some(),
            pi_undone: turn.pi_write.as_ref().is_some_and(|write| write.undone),
            chars: turn.content.chars().count(),
        })
    }

    /// 旧路径：非锤子主题（或非源码模式）下的标签行 + 内容。
    pub(crate) fn ai_workspace_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(view) = self.workspace_view() else {
            return false;
        };
        let showing = self.workspace_tabs_row_ui(ui, &view);
        ui.separator();
        if !showing {
            return false;
        }
        self.ai_workspace_content_ui(ui, &view)
    }

    /// 锤子稿纸：两张纸签 + 同一张纸。返回纸张矩形（供外层木纹避开）。
    ///
    /// 纸签长在纸面上沿，选中签与纸同色；正文走原编辑器，AI 稿走校样纸。
    /// 只在源码模式生效——版式模式本身就是一张虚拟 A4，不再叠第二层纸。
    pub(crate) fn smartisan_workspace_sheet_ui(&mut self, ui: &mut egui::Ui) -> Option<egui::Rect> {
        let view = self.workspace_view()?;
        let showing = self.doc.ai_panel.workspace_view == Some(view.id);
        let full = ui.available_rect_before_wrap();
        if full.height() <= SHEET_TAB_STRIP + 120.0 || full.width() <= 2.0 {
            return None;
        }
        let strip = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), SHEET_TAB_STRIP));
        let body_rect = egui::Rect::from_min_max(strip.left_bottom(), full.max);
        ui.allocate_rect(strip, egui::Sense::hover());
        let mut body = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        // 纸面上沿 = 纸签脚线：两种视图的外框顶边都压在这里，切换时纸不动。
        let base = body_rect.top();
        let paper = if showing {
            self.ai_workspace_sheet_ui(&mut body)
        } else {
            Some(self.source_editor_ui(&mut body, 0))
        };
        self.sheet_tabs_ui(ui, strip, base, &view, showing);
        paper
    }

    /// 纸签条：正文 / AI 工作稿。选中签与纸面同色，脚线压进纸边。
    fn sheet_tabs_ui(
        &mut self,
        ui: &egui::Ui,
        strip: egui::Rect,
        base: f32,
        view: &WorkspaceView,
        showing: bool,
    ) {
        let mut x = strip.left() + 14.0;
        for (index, (label, selected, ai)) in
            [("正文", !showing, false), ("AI 工作稿", showing, true)]
                .into_iter()
                .enumerate()
        {
            let width = if ai {
                SHEET_TAB_WIDTH_AI
            } else {
                SHEET_TAB_WIDTH
            };
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, base - SHEET_TAB_HEIGHT),
                egui::vec2(width, SHEET_TAB_HEIGHT),
            );
            let response = ui.interact(
                rect,
                ui.scope_id().with(("workspace_sheet_tab", index)),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, selected, label)
            });
            // 选中签填纸色：AI 稿是校样纸，正文是稿纸；未选签由主题画牛皮纸。
            let paper = if ai {
                crate::theme::smartisan::PROOF_PAPER
            } else {
                crate::theme::smartisan::PAPER
            };
            crate::theme::smartisan::sheet_tab(
                ui.painter(),
                rect,
                selected,
                response.hovered(),
                response.is_pointer_button_down_on(),
                paper,
                base,
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(crate::theme::font_sizes::SMALL + 1.0),
                if selected {
                    crate::theme::text()
                } else {
                    crate::theme::text_soft()
                },
            );
            // 未选中的 AI 签上点一枚状态色小点：生成中、待采纳、失败都看得到。
            if ai
                && !selected
                && (view.state.running()
                    || matches!(view.state, TurnState::Proposed(_) | TurnState::Failed(_)))
            {
                ui.painter().circle_filled(
                    egui::pos2(rect.right() - 10.0, rect.center().y),
                    2.5,
                    workspace_state_color(&view.state),
                );
            }
            if response.clicked() {
                if ai {
                    self.doc.ai_panel.workspace_view = Some(view.id);
                    self.doc.ai_panel.workspace_compare = false;
                } else {
                    self.doc.ai_panel.workspace_view = None;
                }
            }
            x += width + 2.0;
        }
    }

    /// 旧路径的标签行：正文 / AI 工作稿 + 状态。返回点击后的「是否在看工作稿」。
    fn workspace_tabs_row_ui(&mut self, ui: &mut egui::Ui, view: &WorkspaceView) -> bool {
        let mut showing = self.doc.ai_panel.workspace_view == Some(view.id);
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(!showing, "正文").clicked() {
                self.doc.ai_panel.workspace_view = None;
                showing = false;
            }
            if ui.selectable_label(showing, "AI 工作稿").clicked() {
                self.doc.ai_panel.workspace_view = Some(view.id);
                self.doc.ai_panel.workspace_compare = false;
                showing = true;
            }
            if showing {
                let label = workspace_state_label(view);
                // 采纳时排除了几处的，在标签上看得出来。
                if matches!(view.state, TurnState::Accepted) && view.excluded_hunks > 0 {
                    ui.weak(format!("{label}（排除 {} 处）", view.excluded_hunks));
                } else {
                    ui.weak(label);
                }
            }
        });
        showing
    }

    /// 工作稿内容：控件 → 成品对照 → 只读说明 → 提案对照 → 正文。
    fn ai_workspace_content_ui(&mut self, ui: &mut egui::Ui, view: &WorkspaceView) -> bool {
        if !view.provenance.is_empty() {
            ui.weak(&view.provenance);
        }
        let mut action = None;
        ui.push_id(("ai_workspace_controls", self.doc.key, view.id), |ui| {
            self.workspace_controls_ui(ui, &view.state, &mut action);
        });
        if let Some(action) = action
            && self.apply_workspace_action(action)
        {
            return false;
        }
        let note = self.workspace_readonly_note(view.id);
        ui.weak(note);
        if self.workspace_proposal_compare_ui(ui, &view.state) {
            return true;
        }
        self.workspace_text_ui(ui, view.id, view.state.running());
        true
    }

    /// 锤子稿纸的校样纸：稿头（状态 + 处理条）固定在纸顶，正文在下面滚动。
    fn ai_workspace_sheet_ui(&mut self, ui: &mut egui::Ui) -> Option<egui::Rect> {
        let view = self.workspace_view()?;
        let card = egui::Frame::new()
            .fill(crate::theme::smartisan::PROOF_PAPER)
            .stroke(egui::Stroke::NONE)
            .corner_radius(crate::theme::chrome_radius(crate::theme::PANE_RADIUS))
            .shadow(crate::theme::paper_shadow(26))
            .inner_margin(egui::Margin::ZERO)
            .outer_margin(egui::Margin {
                left: crate::theme::PANE_GAP,
                right: crate::theme::PANE_GAP,
                top: 0,
                bottom: crate::theme::PANE_GAP,
            });
        let panel = egui::CentralPanel::default().frame(card).show(ui, |ui| {
            self.ai_workspace_sheet_body(ui, &view);
        });
        let inset = f32::from(crate::theme::PANE_GAP) + f32::from(crate::theme::PANE_RADIUS);
        Some(panel.response.rect.shrink(inset))
    }

    fn ai_workspace_sheet_body(&mut self, ui: &mut egui::Ui, view: &WorkspaceView) {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 18,
                right: 12,
                top: 12,
                bottom: 14,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let action = self.workspace_sheet_header_ui(ui, view);
                if let Some(action) = action
                    && self.apply_workspace_action(action)
                {
                    return;
                }
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                if !view.provenance.is_empty() {
                    ui.weak(&view.provenance);
                    ui.add_space(4.0);
                }
                let note = self.workspace_readonly_note(view.id);
                ui.weak(note);
                ui.add_space(6.0);
                if self.workspace_proposal_compare_ui(ui, &view.state) {
                    return;
                }
                self.workspace_sheet_text_ui(ui, view.id, view.state.running());
            });
    }

    /// 稿头：状态一行（左：状态点 + 名称；右：运行中时可停止），
    /// 下面接提案的处理条（字数、事实变化、采纳 / 对照 / 放弃）。
    fn workspace_sheet_header_ui(
        &mut self,
        ui: &mut egui::Ui,
        view: &WorkspaceView,
    ) -> Option<CardAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            crate::theme::dot(ui, workspace_state_color(&view.state));
            ui.label(egui::RichText::new(workspace_state_label(view)).strong());
            if view.state.running() && view.chars > 0 {
                ui.label(
                    egui::RichText::new(format!("{} 字", view.chars))
                        .small()
                        .color(crate::theme::text_soft()),
                );
            }
            if matches!(view.state, TurnState::Accepted) && view.excluded_hunks > 0 {
                ui.label(
                    egui::RichText::new(format!("排除 {} 处", view.excluded_hunks))
                        .small()
                        .color(crate::theme::text_soft()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if workspace_unmerged(&view.state) {
                    proof_stamp_ui(ui);
                }
                if view.state.running() && ui.button("停止生成").clicked() {
                    action = Some(CardAction::Stop);
                }
            });
        });
        if let TurnState::Proposed(summary) = &view.state {
            ui.add_space(4.0);
            proposal_actions(ui, summary, self.doc.ai_proposal.as_mut(), &mut action);
            if self.doc.ai_panel.workspace_compare && ui.button("查看 Markdown").clicked() {
                self.doc.ai_panel.workspace_compare = false;
            }
        } else if let TurnState::Failed(error) = &view.state {
            ui.label(error);
        }
        action
    }

    fn workspace_controls_ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &TurnState,
        action: &mut Option<CardAction>,
    ) {
        if let TurnState::Proposed(summary) = state {
            proposal_actions(ui, summary, self.doc.ai_proposal.as_mut(), action);
            if self.doc.ai_panel.workspace_compare && ui.button("查看 Markdown").clicked() {
                self.doc.ai_panel.workspace_compare = false;
            }
        } else if state.running() {
            if ui.button("停止生成").clicked() {
                *action = Some(CardAction::Stop);
            }
        } else if let TurnState::Failed(error) = state {
            ui.label(error);
        }
    }

    /// 处理稿头 / 控件上点下的动作；返回真表示视图已切回正文，不必再画工作稿。
    fn apply_workspace_action(&mut self, action: CardAction) -> bool {
        match action {
            CardAction::Accept => {
                GongwenApp::accept_ai_proposal(self.doc, self.config, self.status)
            }
            CardAction::Discard => {
                GongwenApp::discard_ai_proposal(self.doc, self.status);
                true
            }
            CardAction::Review => {
                self.doc.ai_panel.workspace_compare = true;
                false
            }
            CardAction::Stop => {
                self.stop_ai_task();
                false
            }
            _ => false,
        }
    }

    /// 提案的工作稿 ↔ 正文原地对照；对照里挑掉的块，接受时也一并排除。
    fn workspace_proposal_compare_ui(&mut self, ui: &mut egui::Ui, state: &TurnState) -> bool {
        if !self.doc.ai_panel.workspace_compare || !matches!(state, TurnState::Proposed(_)) {
            return false;
        }
        let Some(proposal) = self.doc.ai_proposal.as_mut() else {
            return false;
        };
        let old = ContentSnapshot::new(
            self.doc.draft.clone(),
            proposal.before.clone(),
            String::new(),
        );
        let new = ContentSnapshot::new(
            self.doc.draft.clone(),
            proposal.result.markdown.clone(),
            String::new(),
        );
        let report = manuscript_diff(&old, &new);
        let hunks = crate::draft_page::diff_hunks::hunks(&report.body, &proposal.result.markdown);
        let toggled = manuscript_diff_ui(
            ui,
            &report,
            &mut proposal.view,
            &DiffViewConfig {
                old_label: "当前正文",
                new_label: "AI 工作稿",
                exclude: Some(HunkExclude {
                    hunks: &hunks,
                    excluded: &proposal.excluded,
                }),
            },
        );
        // 与审阅窗同一套「不要这处」：这里也能挑，接受时一样按排除合并。
        if let Some(hunk) = toggled
            && !proposal.excluded.remove(&hunk)
        {
            proposal.excluded.insert(hunk);
        }
        true
    }

    fn workspace_readonly_note(&self, id: u64) -> &'static str {
        if self
            .doc
            .ai_panel
            .turns
            .iter()
            .find(|turn| turn.id == id)
            .is_some_and(|turn| turn.pi_write.is_some() || turn.pi_pending.is_some())
        {
            "AI 成品只读；保存与导出使用正文。"
        } else {
            "AI 工作稿只读；采纳后进入正文。保存与导出使用正文。"
        }
    }

    /// 当前轮次要展示的 Markdown：提案在时用提案正文，否则用工作稿。
    fn workspace_text(&self, id: u64) -> String {
        let Some(turn) = self.doc.ai_panel.turns.iter().find(|turn| turn.id == id) else {
            return String::new();
        };
        if matches!(turn.state, TurnState::Proposed(_)) {
            self.doc.ai_proposal.as_ref().map_or_else(
                || turn.content.clone(),
                |proposal| proposal.result.markdown.clone(),
            )
        } else {
            turn.content.clone()
        }
    }

    /// 旧路径的正文：普通滚动区，不加纸面。
    fn workspace_text_ui(&mut self, ui: &mut egui::Ui, id: u64, running: bool) {
        let mut text = self.workspace_text(id);
        let key = self.doc.key;
        let fonts = self.config.editor_fonts;
        let size = self.config.editor_font_size;
        let research = self.doc.draft.kind.is_research();
        let highlighter = &mut self.doc.highlighter;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| {
            highlighter.layout(
                ui,
                buffer.as_str(),
                width,
                size,
                None,
                &[],
                &fonts,
                research,
            )
        };
        egui::ScrollArea::vertical()
            .id_salt(("ai_workspace_scroll", key, id))
            .auto_shrink([false; 2])
            .stick_to_bottom(running)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .id_salt(("ai_workspace_text", key, id))
                        .desired_width(f32::INFINITY)
                        .desired_rows(20)
                        .layouter(&mut layouter),
                );
            });
    }

    /// 校样纸上的正文：左边留住与稿纸相同的边距，横线按实际行高铺；
    /// 不显示行号，但版心与正文对齐，切换视图时文字不横跳。
    fn workspace_sheet_text_ui(&mut self, ui: &mut egui::Ui, id: u64, running: bool) {
        let mut text = self.workspace_text(id);
        let key = self.doc.key;
        let fonts = self.config.editor_fonts;
        let size = self.config.editor_font_size;
        let research = self.doc.draft.kind.is_research();
        let highlighter = &mut self.doc.highlighter;
        let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, width: f32| {
            highlighter.layout(
                ui,
                buffer.as_str(),
                width,
                size,
                None,
                &[],
                &fonts,
                research,
            )
        };
        egui::ScrollArea::vertical()
            .id_salt(("ai_workspace_scroll", key, id))
            .auto_shrink([false; 2])
            .stick_to_bottom(running)
            .show(ui, |ui| {
                let paper_slot = ui.painter().add(egui::Shape::Noop);
                let paper_rect = egui::Rect::from_min_max(
                    ui.max_rect().min,
                    egui::pos2(ui.max_rect().right(), ui.clip_rect().bottom()),
                );
                let output = ui
                    .horizontal_top(|ui| {
                        // 与源码编辑器同宽的行号槽：工作稿不显示行号，
                        // 但版心要落在同一列，切回正文时文字不横跳。
                        ui.add_space(38.0);
                        egui::TextEdit::multiline(&mut text)
                            .id_salt(("ai_workspace_text", key, id))
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY)
                            .desired_rows(20)
                            .layouter(&mut layouter)
                            .show(ui)
                    })
                    .inner;
                crate::theme::smartisan::proof_manuscript(
                    ui.painter(),
                    paper_slot,
                    paper_rect,
                    &output,
                    false,
                );
            });
    }
}

/// 纸面上的「未合并」小印章：只在这份稿还没进正文时出现。
fn proof_stamp_ui(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 20.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, crate::theme::smartisan::PROOF_STAMP),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "未合并",
        egui::FontId::proportional(11.0),
        crate::theme::smartisan::PROOF_STAMP,
    );
}

/// 还没写进正文的状态都盖「未合并」章；已采纳 / 已放弃 / 历史稿不再盖。
fn workspace_unmerged(state: &TurnState) -> bool {
    !matches!(
        state,
        TurnState::Accepted | TurnState::Discarded | TurnState::Superseded | TurnState::Expired
    )
}

/// 稿头 / 纸签上的状态名。带「内容未完成」的几档要让人知道这份稿不完整。
fn workspace_state_label(view: &WorkspaceView) -> &'static str {
    match &view.state {
        TurnState::Waiting | TurnState::Streaming => "生成中",
        TurnState::Checking => "检查中",
        TurnState::Proposed(_) => "待采纳",
        TurnState::Asking | TurnState::Pending => "等待回答",
        TurnState::Stopped => "已停止 · 内容未完成",
        TurnState::Failed(_) => "生成失败 · 内容未完成",
        TurnState::Interrupted => "已中断 · 内容未完成",
        TurnState::Accepted if view.pi_undone => "已撤销写入",
        TurnState::Accepted if view.pi_written => "已写入正文",
        TurnState::Accepted => "已采纳",
        TurnState::Conflict => "与正文冲突 · 成品已保留",
        TurnState::Discarded => "已放弃",
        TurnState::Superseded | TurnState::Expired => "历史工作稿",
        TurnState::Reported { .. } => "工作稿",
    }
}

/// 状态色：待采纳用纸签赭色，生成中用强调色，中断类用警告色，已采纳用成功色。
fn workspace_state_color(state: &TurnState) -> egui::Color32 {
    match state {
        TurnState::Proposed(_) => crate::theme::smartisan::INDEX_MARK,
        TurnState::Accepted => crate::theme::success(),
        TurnState::Stopped | TurnState::Failed(_) | TurnState::Interrupted => crate::theme::warn(),
        TurnState::Discarded | TurnState::Superseded | TurnState::Expired => {
            crate::theme::text_muted()
        }
        _ => crate::theme::accent(),
    }
}
