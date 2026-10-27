//! 分节写作、锁定、历史稿与统稿建议采纳。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_sections(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("一节一节写，随时可以修改");
        self.writing_section_tabs(ui);
        let index = self.doc.writing.section;
        let Some(section) = self.doc.writing.task.sections.get_mut(index) else {
            return;
        };
        let section_id = section.id.clone();
        let (chars, target_chars) = (section.markdown.chars().count(), section.target_chars);
        ui.strong(&section.title);
        ui.weak(section.status.label());
        let changed = ui
            .add_enabled(
                !section.locked,
                egui::TextEdit::multiline(&mut section.markdown)
                    .desired_width(f32::INFINITY)
                    .desired_rows(18),
            )
            .changed();
        if changed {
            section.manually_edited = true;
        }
        ui.weak(format!("当前 {chars} 字 / 目标 {target_chars} 字"));
        let confirm_manual = section.manually_edited
            && section.status == crate::agent::writing::SectionStatus::Writing
            && !section.markdown.trim().is_empty()
            && ui.button("确认本节人工稿已完成").clicked();
        if confirm_manual {
            section.status = crate::agent::writing::SectionStatus::ManualDone;
        }
        let lock_changed = ui
            .checkbox(&mut section.locked, "锁定本节，统稿只提建议")
            .changed();
        if changed || lock_changed || confirm_manual {
            self.doc.writing.task.drafts_changed();
            self.doc.writing.changed();
        }
        ui.label("本节修改要求");
        let mut request = self
            .doc
            .writing
            .rewrite_drafts
            .get(&section_id)
            .cloned()
            .unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::multiline(&mut request)
                    .desired_width(f32::INFINITY)
                    .desired_rows(2)
                    .hint_text("例如：保留第一段，第二段压缩到两句话"),
            )
            .changed()
        {
            // 按章节分别保存，切换章节不串。
            self.doc
                .writing
                .rewrite_drafts
                .insert(section_id.clone(), request.clone());
        }
        let locked = self.doc.writing.task.sections[index].locked;
        let has_request = !request.trim().is_empty();
        let response = ui.add_enabled(!locked && has_request, egui::Button::new("按要求重写本节"));
        let response = if !locked && !has_request {
            response.on_disabled_hover_text("先写本节修改要求")
        } else if locked {
            response.on_disabled_hover_text("本节已锁定，请先解锁")
        } else {
            response
        };
        if response.clicked() {
            *action = Some(Action::RewriteSection(index, request));
        }
        ui.weak("人工修改会保留；整批续写不会覆盖人工改过或已锁定的章节。重写只针对当前节。");
        if self
            .doc
            .writing
            .task
            .sections
            .get(index)
            .is_some_and(|section| {
                section.advisory_suggestion.is_some() || !section.advisory_parts.is_empty()
            })
        {
            self.writing_advisory(ui, index);
        }
        self.writing_section_history(ui);
    }

    pub(crate) fn writing_advisory(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(section) = self.doc.writing.task.sections.get(index) else {
            return;
        };
        let title = section.title.clone();
        let Some(advice) = section.advisory_suggestion.clone() else {
            return;
        };
        let mut adopt = false;
        let mut ignore = false;
        ui.push_id(("writing_advisory", index), |ui| {
            egui::Frame::new()
                .fill(theme::warn_soft())
                .corner_radius(6)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&title);
                        if ui.button("采纳到本节").clicked() {
                            adopt = true;
                        }
                        if ui.button("忽略").clicked() {
                            ignore = true;
                        }
                    });
                    egui::CollapsingHeader::new("原稿 / 统稿建议对照")
                        .id_salt(("writing_advisory_body", index))
                        .show(ui, |ui| {
                            ui.label("统稿建议：");
                            ui.label(&advice);
                        });
                    ui.weak("正文仍是原稿；采纳后原稿进入本节历史稿，正文换成建议并仍算人工稿。");
                });
        });
        if adopt {
            self.accept_advisory(index);
        } else if ignore {
            if let Some(section) = self.doc.writing.task.sections.get_mut(index) {
                section.advisory_suggestion = None;
                section.advisory_parts.clear();
            }
            self.doc.writing.changed();
        }
    }

    fn accept_advisory(&mut self, index: usize) {
        if self.doc.writing.task.accept_advisory(index) {
            self.doc.writing.changed();
        }
    }

    fn writing_section_history(&mut self, ui: &mut egui::Ui) {
        let index = self.doc.writing.section;
        let Some(section) = self.doc.writing.task.sections.get(index) else {
            return;
        };
        let history = section.revision_history.clone();
        let mut restore = None;
        if !history.is_empty() {
            ui.collapsing(format!("本节历史稿（{} 份）", history.len()), |ui| {
                for (number, text) in history.iter().enumerate().rev() {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "第 {} 份 · {} 字",
                            number + 1,
                            text.chars().count()
                        ));
                        if ui.button("查看").clicked() {
                            self.doc.writing.source_viewer = Some(SourceViewer {
                                title: "本节历史稿".into(),
                                text: text.clone(),
                            });
                        }
                        if ui.button("恢复到工作稿").clicked() {
                            restore = Some(text.clone());
                        }
                    });
                }
            });
        }
        if let Some(text) = restore {
            let section = &mut self.doc.writing.task.sections[index];
            section.revision_history.push(section.markdown.clone());
            section.markdown = text;
            section.manually_edited = true;
            section.locked = false;
            self.doc.writing.task.drafts_changed();
            self.doc.writing.changed();
        }
    }
}
