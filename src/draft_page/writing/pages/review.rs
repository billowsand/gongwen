//! 统稿审阅与统稿历史。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_review(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("通读统稿，再审阅合并");
        ui.weak("这里仍是工作稿。接受后进入正文，再使用现有预览、排版和导出。");
        let ready = self.doc.writing.ready(Stage::Review);
        ui.horizontal_wrapped(|ui| {
            let label = if self.doc.writing.task.final_markdown.trim().is_empty() {
                "开始统稿"
            } else {
                "重新统稿"
            };
            let response = ui.add_enabled(ready.is_ok(), egui::Button::new(label));
            let response = match &ready {
                Err(reason) => response.on_disabled_hover_text(reason.as_str()),
                Ok(()) => response,
            };
            if response.clicked() {
                if self.final_needs_confirm() {
                    self.doc.writing.confirm = Some(Confirm::Repolish);
                } else {
                    *action = Some(Action::Polish);
                }
            }
            if self.doc.writing.task.final_manual {
                ui.weak("统稿已人工修改，上游变化不会清掉它");
            }
        });
        if !self.doc.writing.task.review_notes.is_empty() {
            ui.collapsing("全篇结构与衔接检查", |ui| {
                ui.label(&self.doc.writing.task.review_notes);
            });
        }
        self.writing_advisories(ui);
        let problems = self.writing_delivery_issues();
        for problem in &problems {
            ui.colored_label(theme::warn(), problem);
        }
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.doc.writing.task.final_markdown)
                    .desired_width(f32::INFINITY)
                    .desired_rows(24),
            )
            .changed()
        {
            self.doc.writing.task.resume_action = None;
            self.doc.writing.task.final_manual = true;
            self.doc.writing.task.status = "统稿已人工修改，请重新核对后审阅".into();
            self.doc.writing.changed();
        }
        ui.weak("差异审阅支持逐块排除。正式正文已有改动时，接受入口会再次检查基线与关键事实。");
        self.writing_final_history(ui);
    }

    fn writing_final_history(&mut self, ui: &mut egui::Ui) {
        let history = self.doc.writing.task.final_history.clone();
        if history.is_empty() {
            return;
        }
        let mut restore = None;
        ui.collapsing(format!("统稿历史（{} 份）", history.len()), |ui| {
            for (number, text) in history.iter().enumerate().rev() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "第 {} 份 · {} 字",
                        number + 1,
                        text.chars().count()
                    ));
                    if ui.button("查看").clicked() {
                        self.doc.writing.source_viewer = Some(SourceViewer {
                            title: "统稿历史".into(),
                            text: text.clone(),
                        });
                    }
                    if ui.button("恢复到统稿页").clicked() {
                        restore = Some(number);
                    }
                });
            }
        });
        if let Some(index) = restore {
            self.doc.writing.task.restore_final(index);
            self.doc.writing.task.status = "已恢复历史统稿，请重新核对后审阅".into();
            self.doc.writing.changed();
        }
    }
}
