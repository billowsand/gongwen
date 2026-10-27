//! 分节备料：每节重新读取原始材料。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_collection(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("围绕每一节，重新读原始材料");
        self.writing_section_tabs(ui);
        let Some(section) = self.doc.writing.task.sections.get(self.doc.writing.section) else {
            return;
        };
        let section = section.clone();
        ui.strong(&section.title);
        ui.label(&section.points);
        let progress: Vec<_> = self
            .doc
            .writing
            .task
            .chunks
            .iter()
            .filter(|chunk| chunk.section_id.as_deref() == Some(&section.id))
            .cloned()
            .collect();
        let coverage = self.doc.writing.section_coverage(&section.id);
        let done = coverage.done;
        ui.add(
            egui::ProgressBar::new(if coverage.total == 0 {
                0.0
            } else {
                done as f32 / coverage.total as f32
            })
            .text(format!(
                "已处理 {done} / {} 块原文（人工跳过 {}，失败 {}）",
                coverage.total, coverage.skipped, coverage.failed
            )),
        );
        for source in self
            .doc
            .writing
            .task
            .sources
            .iter()
            .filter(|source| source.role == SourceRole::Content)
        {
            let chunks: Vec<_> = progress
                .iter()
                .filter(|chunk| chunk.source_id == source.id)
                .collect();
            let count = chunks
                .iter()
                .filter(|chunk| {
                    matches!(
                        chunk.status,
                        ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                    )
                })
                .count();
            ui.label(format!("《{}》已处理 {count} 块", source.title));
        }
        if let Some(next) = self.writing_chunk_actions(ui, SourceRole::Content, Some(&section.id)) {
            *action = Some(next);
        }
        ui.separator();
        let cards: Vec<_> = self
            .doc
            .writing
            .task
            .current_section_cards(&section)
            .into_iter()
            .cloned()
            .collect();
        self.writing_cards(ui, &cards);
    }
}
