//! 提纲编辑与人工确认。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_outline(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("先把提纲定下来");
        ui.weak("直接编辑标题、要点与篇幅，标题不写编号。确认后按各节要求读取内容材料；只调顺序或未改动的章节沿用已备素材。");
        let mut changed = false;
        let mut move_up = None;
        let mut move_down = None;
        let mut remove = None;
        let count = self.doc.writing.task.sections.len();
        for (index, section) in self.doc.writing.task.sections.iter_mut().enumerate() {
            ui.push_id(&section.id, |ui| {
                ui.separator();
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut section.title)
                            .desired_width(f32::INFINITY)
                            .hint_text("本节标题"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::TextEdit::multiline(&mut section.points)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("本节应回答什么问题、有哪些必写要点"),
                    )
                    .changed();
                ui.horizontal_wrapped(|ui| {
                    ui.label("本节约");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut section.target_chars)
                                .range(100..=10_000)
                                .speed(50)
                                .suffix(" 字"),
                        )
                        .changed();
                    if ui
                        .add_enabled(index > 0, egui::Button::new("↑ 上移"))
                        .clicked()
                    {
                        move_up = Some(index);
                    }
                    if ui
                        .add_enabled(index + 1 < count, egui::Button::new("↓ 下移"))
                        .clicked()
                    {
                        move_down = Some(index);
                    }
                    if ui
                        .add_enabled(count > 1, egui::Button::new("删除本节"))
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
            });
        }
        if let Some(index) = move_up {
            self.doc.writing.task.sections.swap(index, index - 1);
            changed = true;
        }
        if let Some(index) = move_down {
            self.doc.writing.task.sections.swap(index, index + 1);
            changed = true;
        }
        if let Some(index) = remove {
            let section = &self.doc.writing.task.sections[index];
            let id = section.id.clone();
            if section.markdown.trim().is_empty() && section.cards.is_empty() {
                self.doc.writing.task.remove_section(&id);
                changed = true;
            } else {
                self.doc.writing.confirm = Some(Confirm::RemoveSection(id));
            }
        }
        if ui.button("＋ 增加一节").clicked() {
            self.doc.writing.task.sections.push(Section {
                id: uuid::Uuid::new_v4().to_string(),
                title: "新增章节".into(),
                target_chars: 300,
                ..Default::default()
            });
            changed = true;
        }
        if !self.doc.writing.task.section_history.is_empty() {
            let mut restore = None;
            ui.collapsing("历史提纲中的章节", |ui| {
                for section in self.doc.writing.task.section_history.clone().iter().rev() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&section.title);
                        if ui.button("查看原稿").clicked() {
                            self.doc.writing.source_viewer = Some(SourceViewer {
                                title: section.title.clone(),
                                text: section.markdown.clone(),
                            });
                        }
                        if ui.button("恢复为一节").clicked() {
                            restore = Some(section.clone());
                        }
                    });
                }
            });
            if let Some(mut section) = restore {
                section.id = uuid::Uuid::new_v4().to_string();
                section.manually_edited = true;
                section.rewrite_in_progress = false;
                section.merge_state = None;
                self.doc.writing.task.sections.push(section);
                changed = true;
            }
        }
        if changed {
            // 各节备料按本节指纹判断：只调顺序全部保留，改了哪一节只有那一节待重新备料。
            self.doc.writing.task.outline_changed();
            self.doc.writing.task.stage = Stage::Outline;
            self.doc.writing.changed();
        }
        let total: usize = self
            .doc
            .writing
            .task
            .sections
            .iter()
            .map(|section| section.target_chars)
            .sum();
        let target = self.doc.writing.task.target_chars.max(1);
        let deviation = (total as f64 - target as f64).abs() / target as f64;
        let text = format!(
            "各节字数合计 {total} 字 / 目标 {target} 字（{}）",
            if total > target { "超出" } else { "不足" }
        );
        if deviation > 0.15 {
            ui.colored_label(
                theme::warn(),
                format!("{text} 偏差超过 15%，建议调整各节篇幅或目标篇幅"),
            );
        } else {
            ui.weak(text);
        }
        ui.add_space(14.0);
        ui.strong("让 AI 按要求调整提纲");
        ui.add(
            egui::TextEdit::multiline(&mut self.doc.writing.revise)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("例如：合并相近的两节，补充责任分工，调整先后顺序"),
        );
        if ui
            .add_enabled(
                !self.doc.writing.revise.trim().is_empty(),
                egui::Button::new("调整提纲"),
            )
            .clicked()
        {
            *action = Some(Action::ReviseOutline(self.doc.writing.revise.clone()));
        }
        ui.weak("提纲未确认前不会开始写正文。重新生成提纲后仍需确认。");
    }
}
