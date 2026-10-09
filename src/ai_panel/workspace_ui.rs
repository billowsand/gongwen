//! 左侧 AI 工作稿：流式源码、原地对照与人工采纳，不改写正式正文。

use super::TurnState;
use super::ui::{CardAction, proposal_actions};
use crate::app::GongwenApp;
use crate::diff::{ContentSnapshot, manuscript_diff};
use crate::diff_view::{DiffViewConfig, HunkExclude, manuscript_diff_ui};
use crate::draft_page::DraftPage;
use eframe::egui;

impl DraftPage<'_> {
    /// 返回是否已绘制工作稿；查看正文时由原编辑器继续绘制。
    pub(crate) fn ai_workspace_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let panel = &mut self.doc.ai_panel;
        let selected = panel.workspace_view.and_then(|id| {
            panel
                .turns
                .iter()
                .find(|turn| turn.id == id && turn.is_workspace)
        });
        let latest = selected.or_else(|| panel.turns.iter().rev().find(|turn| turn.is_workspace));
        let Some(turn) = latest else {
            panel.workspace_view = None;
            return false;
        };
        let id = turn.id;
        let state = turn.state.clone();
        let excluded_hunks = turn.excluded_hunks;
        let provenance = turn.provenance.clone();
        let showing = panel.workspace_view == Some(id);
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(!showing, "正文").clicked() {
                panel.workspace_view = None;
            }
            if ui.selectable_label(showing, "AI 工作稿").clicked() {
                panel.workspace_view = Some(id);
                panel.workspace_compare = false;
            }
            if showing {
                let label = match &state {
                    TurnState::Waiting | TurnState::Streaming => "生成中",
                    TurnState::Checking => "检查中",
                    TurnState::Proposed(_) => "待采纳",
                    TurnState::Asking | TurnState::Pending => "等待回答",
                    TurnState::Stopped => "已停止 · 内容未完成",
                    TurnState::Failed(_) => "生成失败 · 内容未完成",
                    TurnState::Interrupted => "已中断 · 内容未完成",
                    TurnState::Accepted => "已采纳",
                    TurnState::Discarded => "已放弃",
                    TurnState::Superseded | TurnState::Expired => "历史工作稿",
                    TurnState::Reported { .. } => "工作稿",
                };
                // 采纳时排除了几处的，在标签上看得出来。
                if matches!(state, TurnState::Accepted) && excluded_hunks > 0 {
                    ui.weak(format!("{label}（排除 {excluded_hunks} 处）"));
                } else {
                    ui.weak(label);
                }
            }
        });
        ui.separator();
        if panel.workspace_view != Some(id) {
            return false;
        }
        if !provenance.is_empty() {
            ui.weak(provenance);
        }
        let mut action = None;
        ui.push_id(("ai_workspace_controls", self.doc.key, id), |ui| {
            if let TurnState::Proposed(summary) = &state {
                proposal_actions(ui, summary, self.doc.ai_proposal.as_mut(), &mut action);
                if self.doc.ai_panel.workspace_compare && ui.button("查看 Markdown").clicked() {
                    self.doc.ai_panel.workspace_compare = false;
                }
            } else if state.running() {
                if ui.button("停止生成").clicked() {
                    action = Some(CardAction::Stop);
                }
            } else if let TurnState::Failed(error) = &state {
                ui.label(error);
            }
        });
        match action {
            Some(CardAction::Accept) => {
                if GongwenApp::accept_ai_proposal(self.doc, self.config, self.status) {
                    return false;
                }
            }
            Some(CardAction::Discard) => {
                GongwenApp::discard_ai_proposal(self.doc, self.status);
                return false;
            }
            Some(CardAction::Review) => self.doc.ai_panel.workspace_compare = true,
            Some(CardAction::Stop) => self.stop_ai_task(),
            _ => {}
        }
        ui.weak("AI 工作稿只读；采纳后进入正文。保存与导出使用正文。");
        if self.doc.ai_panel.workspace_compare
            && matches!(state, TurnState::Proposed(_))
            && let Some(proposal) = self.doc.ai_proposal.as_mut()
        {
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
            let hunks =
                crate::draft_page::diff_hunks::hunks(&report.body, &proposal.result.markdown);
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
            return true;
        }
        let turn = self
            .doc
            .ai_panel
            .turns
            .iter()
            .find(|turn| turn.id == id)
            .expect("当前会话的工作稿");
        let mut text = if matches!(turn.state, TurnState::Proposed(_)) {
            self.doc
                .ai_proposal
                .as_ref()
                .map_or(turn.content.as_str(), |proposal| {
                    proposal.result.markdown.as_str()
                })
        } else {
            turn.content.as_str()
        };
        let highlighter = &mut self.doc.highlighter;
        let fonts = self.config.editor_fonts;
        let size = self.config.editor_font_size;
        let research = self.doc.draft.kind.is_research();
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
            .id_salt(("ai_workspace_scroll", self.doc.key, id))
            .auto_shrink([false; 2])
            .stick_to_bottom(turn.state.running())
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .id_salt(("ai_workspace_text", self.doc.key, id))
                        .desired_width(f32::INFINITY)
                        .desired_rows(20)
                        .layouter(&mut layouter),
                );
            });
        true
    }
}
