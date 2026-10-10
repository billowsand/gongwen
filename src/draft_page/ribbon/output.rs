//! 输出分区：同一组命令在锤子主题下按用途分组，动作仍走原有导出路径。

use crate::draft_page::{DraftPage, toolbar_separator};
use crate::models::ExportSelection;
use crate::theme;
use eframe::egui;

impl DraftPage<'_> {
    pub(crate) fn ribbon_output(&mut self, ui: &mut egui::Ui) {
        if theme::smartisan::active() {
            theme::smartisan::command_group(ui, "文件导出", |ui| {
                self.ribbon_export_formats(ui)
            });
            theme::divider_v(ui, 42.0);
            theme::smartisan::command_group(ui, "存放位置", |ui| {
                self.ribbon_output_directory(ui)
            });
            theme::divider_v(ui, 42.0);
            theme::smartisan::command_group(ui, "最近导出", |ui| self.export_open_buttons(ui));
        } else {
            self.ribbon_export_formats(ui);
            toolbar_separator(ui);
            self.ribbon_output_directory(ui);
            self.export_open_buttons(ui);
        }
    }

    fn ribbon_export_formats(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        let ready = !self.doc.busy && has_draft;
        let export_button = if theme::smartisan::active() {
            theme::primary_icon_button_enabled(ui, ready, theme::Icon::FileDown, "导出")
        } else {
            ui.add_enabled(
                ready,
                theme::secondary_icon_button(theme::Icon::FileDown, "导出"),
            )
        };
        if theme::smartisan::active() {
            theme::smartisan::bevel(
                ui.painter(),
                export_button.rect,
                export_button.is_pointer_button_down_on(),
                true,
            );
        }
        if export_button
            .on_hover_text("按设置里勾选的格式导出当前审校稿，可反复导出")
            .clicked()
        {
            self.start_export_current();
        }
        let overwrite = self.config.export.overwrite;
        let mut only: Option<(ExportSelection, &'static str)> = None;
        ui.add_enabled_ui(ready, |ui| {
            for (icon, label, selection, tip) in [
                (
                    theme::Icon::FileTypeDoc,
                    "仅 Word",
                    ExportSelection {
                        markdown: false,
                        docx: true,
                        pdf: false,
                        overwrite,
                    },
                    "这一次只出 docx，不改设置里勾好的常用格式",
                ),
                (
                    theme::Icon::FileTypePdf,
                    "仅 PDF",
                    ExportSelection {
                        markdown: false,
                        docx: false,
                        pdf: true,
                        overwrite,
                    },
                    "这一次只出 PDF，不改设置里勾好的常用格式",
                ),
                (
                    theme::Icon::PencilLine,
                    "仅 Markdown",
                    ExportSelection {
                        markdown: true,
                        docx: false,
                        pdf: false,
                        overwrite,
                    },
                    "这一次只出 Markdown 源码包：md 正文、稿中引用的图片，研究报告另含 references.bib",
                ),
            ] {
                if ui
                    .add(theme::icon_text_button(icon, label))
                    .on_hover_text(tip)
                    .clicked()
                {
                    only = Some((selection, label));
                }
            }
        });
        if let Some((selection, label)) = only {
            self.start_export_with(selection);
            *self.status = format!("正在按「{label}」导出…");
        }
    }

    fn ribbon_output_directory(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(theme::icon_text_button(theme::Icon::Folder, "输出目录"))
            .on_hover_text(self.config.output_dir.clone())
            .clicked()
        {
            self.open_output_dir();
        }
    }
}
