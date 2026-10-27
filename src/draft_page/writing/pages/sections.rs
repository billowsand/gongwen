//! 分节写作、锁定、历史稿与统稿建议采纳。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_sections(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("一节一节写，随时可以修改");
        self.writing_section_tabs(ui);
        let Some(section) = self
            .doc
            .writing
            .task
            .sections
            .get_mut(self.doc.writing.section)
        else {
            return;
        };
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
        ui.add(
            egui::TextEdit::multiline(&mut self.doc.writing.rewrite)
                .desired_width(f32::INFINITY)
                .desired_rows(2)
                .hint_text("例如：保留第一段，第二段压缩到两句话"),
        );
        let locked = self.doc.writing.task.sections[self.doc.writing.section].locked;
        if ui
            .add_enabled(!locked, egui::Button::new("按要求重写本节"))
            .clicked()
        {
            *action = Some(Action::RewriteSection(
                self.doc.writing.section,
                self.doc.writing.rewrite.clone(),
            ));
        }
        ui.weak("人工修改会保留；整批续写不会覆盖人工改过或已锁定的章节。重写只针对当前节。");
        let index = self.doc.writing.section;
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

    /// 锁定或人工稿的统稿建议：单独展示，采纳与否由用户决定，正文在采纳前不动。
    pub(crate) fn writing_advisories(&mut self, ui: &mut egui::Ui) {
        let indexes: Vec<usize> = self
            .doc
            .writing
            .task
            .sections
            .iter()
            .enumerate()
            .filter(|(_, section)| section.advisory_suggestion.is_some())
            .map(|(index, _)| index)
            .collect();
        if indexes.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.strong("锁定与人工稿的统稿建议（不覆盖正文，采纳由你决定）");
        for index in indexes {
            self.writing_advisory(ui, index);
        }
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
