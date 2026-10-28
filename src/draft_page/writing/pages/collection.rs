//! 各节素材：从已提炼的主题素材里分给各节，可手动移出或补入，不再逐节重读原文。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_collection(&mut self, ui: &mut egui::Ui, _action: &mut Option<Action>) {
        ui.heading("核对各节素材");
        ui.weak("素材来自第二步的提炼结果，按提纲分给各节；可以把不合适的移出，或从未分配的素材里补入。");
        self.writing_section_tabs(ui);
        let index = self.doc.writing.section;
        let Some(section) = self.doc.writing.task.sections.get(index).cloned() else {
            return;
        };
        ui.strong(&section.title);
        ui.label(&section.points);
        let task = &self.doc.writing.task;
        if !task.section_assigned(&section) {
            ui.colored_label(
                theme::warn(),
                "本节还没按当前提纲分配素材：点「确认提纲，分配各节素材」重新分配。",
            );
        }
        let assigned: Vec<MaterialCard> = task
            .current_section_cards(&section)
            .into_iter()
            .cloned()
            .collect();
        ui.weak(format!("本节素材 {} 条", assigned.len()));
        let mut remove = None;
        for card in &assigned {
            ui.push_id(("writing_section_card", &card.id), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("移出本节").clicked() {
                        remove = Some(card.id.clone());
                    }
                    ui.label(&card.point);
                });
            });
        }
        let used: std::collections::HashSet<&str> = task
            .sections
            .iter()
            .flat_map(|section| task.current_section_cards(section))
            .map(|card| card.id.as_str())
            .collect();
        let spare: Vec<MaterialCard> = task
            .current_topic_cards()
            .into_iter()
            .filter(|card| !assigned.iter().any(|mine| mine.id == card.id))
            .cloned()
            .collect();
        let unassigned = spare
            .iter()
            .filter(|card| !used.contains(card.id.as_str()))
            .count();
        let mut add = None;
        egui::CollapsingHeader::new(format!(
            "其他素材（{} 条，其中 {} 条尚未分给任何章节）",
            spare.len(),
            unassigned
        ))
        .id_salt(("writing_spare_cards", index))
        .show(ui, |ui| {
            for card in &spare {
                ui.push_id(("writing_spare_card", &card.id), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if ui.small_button("加入本节").clicked() {
                            add = Some(card.clone());
                        }
                        if !used.contains(card.id.as_str()) {
                            ui.weak("未分配");
                        }
                        ui.label(&card.point);
                    });
                });
            }
        });
        if remove.is_some() || add.is_some() {
            let query = self.doc.writing.task.section_query(&section);
            let target = &mut self.doc.writing.task.sections[index];
            if let Some(id) = remove {
                target.cards.retain(|card| card.id != id);
            }
            if let Some(card) = add {
                target.cards.push(MaterialCard {
                    section_id: Some(section.id.clone()),
                    query,
                    ..card
                });
            }
            self.doc.writing.task.drafts_changed();
            self.doc.writing.changed();
        }
        ui.separator();
        self.writing_cards(ui, &assigned);
    }
}
