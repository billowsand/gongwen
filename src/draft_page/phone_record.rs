//! 电话记录单要素，与发文表单分开维护。
use super::DraftPage;
use super::form::{check_form, field_error};
use crate::app::widgets::{contact_pair, layout_options, plain_options, single_select};
use crate::models::SecurityLevel;
use eframe::egui;

impl DraftPage<'_> {
    pub(crate) fn phone_record_form_ui(&mut self, ui: &mut egui::Ui, width: f32) {
        ui.weak("记录收到的电话通知；首长批示栏导出时留空供人工填写。");
        let units = layout_options(&self.unit_pool(false), None);
        let caller_contacts = if self.doc.draft.phone_record.caller_unit.trim().is_empty() {
            self.contacts()
        } else {
            crate::units::UnitDisplay::new(&self.config.vocabulary)
                .people_of(&self.doc.draft.phone_record.caller_unit)
        };
        let caller_contacts: Vec<_> = caller_contacts
            .into_iter()
            .map(|(name, phone)| {
                let position = self
                    .config
                    .vocabulary
                    .iter()
                    .find(|entry| {
                        entry.category == crate::models::VocabularyCategory::Person
                            && entry.canonical.trim() == name
                    })
                    .map(|entry| entry.position.trim())
                    .unwrap_or("");
                (format!("{name}{position}"), phone)
            })
            .collect();
        let rules = self.config.security_rules.clone();
        let check = check_form(&self.doc.draft);
        let field_width = (width - 110.0).max(80.0);
        let r = &mut self.doc.draft.phone_record;
        egui::Grid::new("phone_record_form")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label("本单位名称 *");
                single_select(
                    ui,
                    "record_institution",
                    &mut r.institution,
                    &units,
                    &mut self.doc.manual_fields,
                    self.config.allow_free_text,
                    field_width,
                    "记录单所属单位",
                );
                ui.end_row();
                field_error(ui, &check, "record_institution");
                ui.label("来电单位 *");
                if single_select(
                    ui,
                    "record_unit",
                    &mut r.caller_unit,
                    &units,
                    &mut self.doc.manual_fields,
                    true,
                    field_width,
                    "来电单位标准名称",
                ) {
                    r.caller_person.clear();
                    r.caller_phone.clear();
                }
                ui.end_row();
                field_error(ui, &check, "record_unit");
                ui.label("谈话人及职务 *");
                let names = plain_options(
                    &caller_contacts
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect::<Vec<_>>(),
                );
                let manual = self.doc.manual_fields.contains("record_person") || names.is_empty();
                if single_select(
                    ui,
                    "record_person",
                    &mut r.caller_person,
                    &names,
                    &mut self.doc.manual_fields,
                    true,
                    field_width,
                    "姓名及词库职务",
                ) && !manual
                {
                    r.caller_phone = caller_contacts
                        .iter()
                        .find(|(name, _)| name == &r.caller_person)
                        .map(|(_, phone)| phone.clone())
                        .unwrap_or_default();
                }
                ui.end_row();
                field_error(ui, &check, "record_person");
                ui.label("来电电话");
                let phones = plain_options(
                    &caller_contacts
                        .iter()
                        .map(|(_, phone)| phone.clone())
                        .filter(|phone| !phone.is_empty())
                        .collect::<Vec<_>>(),
                );
                single_select(
                    ui,
                    "record_phone",
                    &mut r.caller_phone,
                    &phones,
                    &mut self.doc.manual_fields,
                    true,
                    field_width,
                    "来电电话，可手填",
                );
                ui.end_row();
                ui.label("通话时间 *");
                ui.add(egui::TextEdit::singleline(&mut r.call_time).desired_width(field_width));
                ui.end_row();
                field_error(ui, &check, "record_time");
                ui.label("密级 *");
                let mut level = SecurityLevel::from_marking(&self.doc.draft.profile.security_level);
                let previous = level;
                egui::ComboBox::from_id_salt("record_security")
                    .selected_text(level.label())
                    .show_ui(ui, |ui| {
                        for option in SecurityLevel::ALL {
                            if option != SecurityLevel::Unmarked {
                                ui.selectable_value(&mut level, option, option.label());
                            }
                        }
                    });
                if previous != level {
                    self.doc.draft.profile.security_level = level.marking().into();
                    self.doc.draft.profile.security_period = rules.default_period(level);
                }
                ui.end_row();
                field_error(ui, &check, "security_level");
                ui.label("保密期限");
                let options = rules.period_options(level);
                if options.is_empty() {
                    self.doc.draft.profile.security_period.clear();
                }
                egui::ComboBox::from_id_salt("record_security_period")
                    .selected_text(&self.doc.draft.profile.security_period)
                    .show_ui(ui, |ui| {
                        for option in options {
                            ui.selectable_value(
                                &mut self.doc.draft.profile.security_period,
                                option.clone(),
                                option,
                            );
                        }
                    });
                ui.end_row();
                ui.label("承办单位");
                if single_select(
                    ui,
                    "record_responsible",
                    &mut self.doc.draft.profile.responsible_unit,
                    &units,
                    &mut self.doc.manual_fields,
                    self.config.allow_free_text,
                    field_width,
                    "承办单位",
                ) {
                    self.doc.draft.profile.contact_person.clear();
                    self.doc.draft.profile.contact_phone.clear();
                }
                ui.end_row();
                ui.label("联系人及电话");
                let contacts = crate::units::UnitDisplay::new(&self.config.vocabulary)
                    .responsible_people_of(&self.doc.draft.profile.responsible_unit);
                contact_pair(
                    ui,
                    &mut self.doc.draft.profile.contact_person,
                    &mut self.doc.draft.profile.contact_phone,
                    &contacts,
                    &mut self.doc.manual_fields,
                    self.config.allow_free_text,
                    field_width,
                );
                ui.end_row();
            });
        if let Some(message) = rules.check(
            SecurityLevel::from_marking(&self.doc.draft.profile.security_level),
            &self.doc.draft.profile.security_period,
        ) {
            ui.colored_label(crate::theme::warn(), message);
        }
        ui.label("拟办建议（人工填写）");
        ui.add(
            egui::TextEdit::multiline(&mut r.suggestion)
                .desired_width(width)
                .desired_rows(3),
        );
        ui.weak("正文编辑区填写来电内容。通话时间按实际接听时间填写。");
    }
}
