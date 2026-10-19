//! 输出分区：一行紧凑命令，动作仍走原有导出路径。

use crate::draft_page::{DraftPage, toolbar_separator};
use crate::models::ExportSelection;
use crate::theme;
use eframe::egui;

impl DraftPage<'_> {
    pub(crate) fn ribbon_output(&mut self, ui: &mut egui::Ui) {
        self.ribbon_export_formats(ui);
        toolbar_separator(ui);
        self.ribbon_output_directory(ui);
        if theme::smartisan::active() {
            toolbar_separator(ui);
        }
        self.export_open_buttons(ui);
    }

    fn ribbon_export_formats(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        let ready = !self.doc.busy && has_draft;
        let export_button = ui.add_enabled(
            ready,
            theme::secondary_icon_button(theme::Icon::FileDown, "导出"),
        );
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
                        typst: false,
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
                        typst: false,
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
                        typst: false,
                        overwrite,
                    },
                    "这一次只出 Markdown 源码包：md 正文、稿中引用的图片，研究报告另含 references.bib",
                ),
                (
                    theme::Icon::Package,
                    "仅 Typst 包",
                    ExportSelection {
                        markdown: false,
                        docx: false,
                        pdf: false,
                        typst: true,
                        overwrite,
                    },
                    "这一次只出 Typst 包：模板、数据与插图打成 zip，装有常用中文字体的机器可用 typst 命令行编译",
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
            if self.doc.busy {
                *self.status = format!("正在按「{label}」导出…");
            }
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
