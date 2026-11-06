//! 印制追溯设置与本地登记查询。
use super::GongwenApp;
use super::settings::{setting_continuation, setting_row};
use eframe::egui;

impl GongwenApp {
    pub(super) fn print_trace_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        ui.separator();
        ui.heading("印制追溯码");
        ui.checkbox(
            &mut self.config.export.print_trace.enabled,
            "起草页导出 PDF 时添加印制追溯码",
        );
        setting_row(
            ui,
            "印制单位",
            Some("填写本机登记的打印单位，与主送、抄送和接收单位无关"),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.config.export.print_trace.unit)
                        .char_limit(60)
                        .desired_width(300.0),
                );
            },
        );
        setting_continuation(ui, |ui| {
            ui.weak("每页右下角底部叠加长方形码和两行小字，不占正文空间。记录的是本次生成 PDF 登记的印制单位，不能验证文件转交后的实际打印单位。");
        });
        if let Err(error) = self.config.export.print_trace.validate() {
            ui.colored_label(super::warn(), error.to_string());
        }
        ui.add_space(8.0);
        egui::CollapsingHeader::new("按印制编号查询 / 最近登记")
            .id_salt("print_trace_history")
            .show(ui, |ui| {
                let key = egui::Id::new("print_trace_lookup_code");
                let mut query = ui
                    .ctx()
                    .data_mut(|data| data.get_temp::<String>(key))
                    .unwrap_or_default();
                ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .hint_text("输入完整编号，留空显示最近 20 条")
                        .char_limit(32)
                        .desired_width(310.0),
                );
                ui.ctx()
                    .data_mut(|data| data.insert_temp(key, query.clone()));
                if let Some(store) = &self.manuscript_store {
                    match store.find_print_traces(&query) {
                        Ok(records) => {
                            if records.is_empty() {
                                ui.weak("没有找到对应登记。");
                            }
                            for record in records {
                                ui.separator();
                                ui.label(format!(
                                    "{}  ·  {}",
                                    crate::export::print_trace::display_code(&record.code),
                                    record.unit
                                ));
                                ui.label(record.title);
                                ui.weak(record.created_at);
                                ui.label("原稿与 PDF 均已登记 SHA-256 校验值")
                                    .on_hover_text(format!(
                                        "原稿：{}\nPDF：{}",
                                        record.source_hash, record.pdf_hash
                                    ));
                            }
                        }
                        Err(error) => {
                            ui.colored_label(super::warn(), format!("查询失败：{error:#}"));
                        }
                    }
                } else {
                    ui.weak("稿件库尚未打开。");
                }
            });
    }
}
