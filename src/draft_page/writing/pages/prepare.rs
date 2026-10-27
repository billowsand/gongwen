//! 素材准备：主题、篇幅与三类材料的添加。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_prepare(&mut self, ui: &mut egui::Ui) {
        ui.heading("先备好材料，再开始写");
        ui.label("内容材料提供事实，风格样稿学习表达，结构范文学习篇章组织。");
        ui.add_space(12.0);
        ui.strong("这篇公文要写什么");
        self.writing_topic(ui);
        let mut length_changed = false;
        ui.horizontal(|ui| {
            ui.label("目标篇幅");
            length_changed = ui
                .add(
                    egui::DragValue::new(&mut self.doc.writing.task.target_chars)
                        .range(200..=30_000)
                        .speed(50)
                        .suffix(" 字"),
                )
                .changed();
            ui.weak(format!("文种：{}", self.doc.draft.kind.label()));
        });
        if length_changed {
            self.doc.writing.task.target_changed();
            self.doc.writing.changed();
        }
        ui.add_space(14.0);
        if ui.available_width() >= 680.0 {
            ui.columns(3, |columns| {
                for (column, role) in columns.iter_mut().zip(SourceRole::ALL) {
                    self.writing_source_group(column, role);
                }
            });
        } else {
            for role in SourceRole::ALL {
                self.writing_source_group(ui, role);
                ui.add_space(12.0);
            }
        }
        for error in &self.doc.writing.import_errors {
            ui.colored_label(theme::warn(), error);
        }
        ui.add_space(10.0);
        ui.weak(
            "无风格或结构范文时采用文种默认要求。文件导入在后台处理，解析失败的材料不会计为可用。",
        );
    }

    /// 还没有成果时直接改主题；已有成果时先改草稿，说明影响后由用户点「应用」。
    fn writing_topic(&mut self, ui: &mut egui::Ui) {
        fn editor(text: &mut String) -> egui::TextEdit<'_> {
            egui::TextEdit::multiline(text)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("写明主题、受文对象、写作目的与重点")
        }
        let writing = &mut self.doc.writing;
        if !writing.task.has_results() {
            writing.topic_draft = None;
            if ui.add(editor(&mut writing.task.topic)).changed() {
                writing.changed();
            }
            return;
        }
        let draft = writing
            .topic_draft
            .get_or_insert_with(|| writing.task.topic.clone());
        ui.add(editor(draft));
        if *draft == writing.task.topic {
            return;
        }
        let cards = writing.task.current_topic_cards().len();
        let done = writing
            .task
            .chunks
            .iter()
            .filter(|chunk| {
                chunk.role == SourceRole::Content
                    && matches!(
                        chunk.status,
                        ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                    )
            })
            .count();
        let mut apply = false;
        let mut discard = false;
        egui::Frame::new()
            .fill(theme::warn_soft())
            .corner_radius(6)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.colored_label(
                    theme::warn(),
                    format!(
                        "改主题后，已提炼的 {cards} 张主题素材卡与各节备料（已处理 {done} 块）不再算作当前成果，需按新主题重新提炼、备料，提纲也要重新确认。重新提炼完成前改回原主题即可恢复。"
                    ),
                );
                ui.horizontal(|ui| {
                    apply = ui.button("应用新主题").clicked();
                    discard = ui.button("放弃修改").clicked();
                });
            });
        if apply && let Some(topic) = writing.topic_draft.take() {
            writing.task.set_topic(topic);
            writing.changed();
        } else if discard {
            writing.topic_draft = None;
        }
    }
}
