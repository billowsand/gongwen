//! 写作任务的结果泵独立于可见标签，离开页面后仍保存每个完成单元。

use super::GongwenApp;
use eframe::egui;

impl GongwenApp {
    pub(super) fn poll_writing_tasks(&mut self, ctx: &egui::Context) {
        for doc in &mut self.docs {
            let was_running = doc.writing.running;
            doc.writing.sync(doc.manuscript_id, false);
            if let Some(id) = doc.writing.proposal_turn
                && let Some(turn) = doc.ai_panel.turn_mut(id)
            {
                if matches!(turn.state, crate::ai_panel::TurnState::Accepted) {
                    doc.writing.open = false;
                    doc.writing.proposal_turn = None;
                } else if matches!(turn.state, crate::ai_panel::TurnState::Discarded) {
                    doc.writing.proposal_turn = None;
                }
            }
            if was_running && !doc.writing.running {
                doc.busy = false;
            }
            if doc.writing.running || doc.writing.waiting_to_save() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }
}
