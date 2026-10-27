//! 统稿审阅：逐节原稿 / 统稿建议对照，直接改全文作为次级入口。

use super::*;
use crate::agent::writing::Section;

impl DraftPage<'_> {
    pub(crate) fn writing_review(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("逐节对照，再审阅合并");
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
        let problems = self.writing_delivery_issues();
        for problem in &problems {
            ui.colored_label(theme::warn(), problem);
        }
        self.writing_review_sections(ui);
        self.writing_review_editor(ui);
        ui.weak("差异审阅支持逐块排除。正式正文已有改动时，接受入口会再次检查基线与关键事实。");
        self.writing_final_history(ui);
    }

    /// 默认视图：每节左边原稿、右边统稿建议，由用户决定拼进全文的是哪一份。
    fn writing_review_sections(&mut self, ui: &mut egui::Ui) {
        let sections: Vec<Section> = self.doc.writing.task.sections.clone();
        if sections.is_empty() {
            ui.weak("还没有章节，先在提纲页确认提纲。");
            return;
        }
        let final_manual = self.doc.writing.task.final_manual;
        if final_manual {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(theme::warn(), "统稿已人工修改，下面逐节选择不会改动它。");
                if ui
                    .button("按逐节选择重新拼全文")
                    .on_hover_text("当前人工统稿先存入统稿历史，可随时恢复")
                    .clicked()
                {
                    self.doc.writing.task.reassemble_final();
                    self.doc.writing.changed();
                }
            });
        }
        let mut choice = None;
        let mut adopt_advisory = None;
        let mut ignore_advisory = None;
        for (index, section) in sections.iter().enumerate() {
            ui.push_id(("writing_review_section", index), |ui| {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&section.title);
                    if section.locked {
                        ui.weak("已锁定");
                    } else if section.manually_edited {
                        ui.weak("人工稿");
                    }
                    ui.weak(section.status.label());
                });
                if let Some(suggestion) = &section.review_suggestion {
                    self.writing_comparison(ui, &section.markdown, suggestion, index);
                    let kept = section.review_kept_draft;
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(if kept {
                            "全文用：原稿"
                        } else {
                            "全文用：统稿建议"
                        });
                        // 两个选项互斥且可来回切换，建议不会因为选了原稿而丢失。
                        if ui
                            .add(egui::Button::new("采纳建议").selected(!kept))
                            .clicked()
                            && kept
                        {
                            choice = Some((index, false));
                        }
                        if ui
                            .add(egui::Button::new("保留原稿").selected(kept))
                            .clicked()
                            && !kept
                        {
                            choice = Some((index, true));
                        }
                    });
                } else if let Some(advice) = &section.advisory_suggestion {
                    ui.weak("锁定 / 人工稿只给建议，采纳后写入本节并仍算人工稿。");
                    let mut adopt = false;
                    let mut ignore = false;
                    self.writing_comparison(ui, &section.markdown, advice, index);
                    ui.horizontal_wrapped(|ui| {
                        adopt = ui.button("采纳到本节").clicked();
                        ignore = ui.button("忽略").clicked();
                    });
                    if adopt {
                        adopt_advisory = Some(index);
                    }
                    if ignore {
                        ignore_advisory = Some(index);
                    }
                } else if !section.markdown.trim().is_empty() {
                    ui.weak("本节暂未生成统稿建议，拼全文时使用本节正文。");
                }
            });
        }
        if let Some((index, keep_draft)) = choice
            && self.doc.writing.task.choose_review(index, keep_draft)
        {
            self.doc.writing.changed();
        }
        if let Some(index) = adopt_advisory
            && self.doc.writing.task.accept_advisory(index)
        {
            self.doc.writing.changed();
        }
        if let Some(index) = ignore_advisory {
            if let Some(section) = self.doc.writing.task.sections.get_mut(index) {
                section.advisory_suggestion = None;
                section.advisory_parts.clear();
            }
            self.doc.writing.changed();
        }
    }

    fn writing_comparison(
        &mut self,
        ui: &mut egui::Ui,
        original: &str,
        suggestion: &str,
        index: usize,
    ) {
        if ui.available_width() >= 760.0 {
            ui.columns(2, |columns| {
                columns[0].strong("原稿");
                egui::ScrollArea::vertical()
                    .id_salt(("writing_review_original", index))
                    .max_height(220.0)
                    .show(&mut columns[0], |ui| ui.label(original));
                columns[1].strong("统稿建议");
                egui::ScrollArea::vertical()
                    .id_salt(("writing_review_suggestion", index))
                    .max_height(220.0)
                    .show(&mut columns[1], |ui| ui.label(suggestion));
            });
        } else {
            egui::CollapsingHeader::new("原稿")
                .id_salt(("writing_review_original", index))
                .show(ui, |ui| ui.label(original));
            egui::CollapsingHeader::new("统稿建议")
                .id_salt(("writing_review_suggestion", index))
                .show(ui, |ui| ui.label(suggestion));
        }
    }

    /// 次级入口：直接改整篇统稿，改过之后按人工统稿保留。
    fn writing_review_editor(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("直接编辑全文")
            .id_salt("writing_review_editor")
            .show(ui, |ui| {
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut self.doc.writing.task.final_markdown)
                            .desired_width(f32::INFINITY)
                            .desired_rows(20),
                    )
                    .changed()
                {
                    self.doc.writing.task.resume_action = None;
                    self.doc.writing.task.final_manual = true;
                    self.doc.writing.task.status = "统稿已人工修改，请重新核对后审阅".into();
                    self.doc.writing.changed();
                }
            });
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
