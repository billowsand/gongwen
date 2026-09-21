//! 研究报告专用的封面元数据与 BibTeX 表单。

use super::*;

impl DraftPage<'_> {
    pub(super) fn research_form_ui(&mut self, ui: &mut egui::Ui, available_width: f32) {
        let content_width = available_width.clamp(*CONTENT_WIDTH.start(), *CONTENT_WIDTH.end());
        let field_width =
            (content_width - LABEL_WIDTH - FORM_LAYOUT_GUTTER).max(FORM_FIELD_MIN_WIDTH);
        let mut pick_bibliography = false;
        let mut clear_bibliography = false;

        ui.strong("封面信息");
        ui.add_space(4.0);
        egui::Grid::new("research_cover_grid")
            .num_columns(2)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                row_label_with_info(
                    ui,
                    "密级",
                    "默认公开；涉密报告同时填写保密期限，排版时以五角星连接。",
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.security)
                        .desired_width(field_width),
                );
                ui.end_row();

                row_label(ui, "保密期限");
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.security_years)
                        .hint_text("公开报告留空；例如：5年、长期")
                        .desired_width(field_width),
                );
                ui.end_row();

                row_label(ui, "文件类型");
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.file_type)
                        .desired_width(field_width),
                );
                ui.end_row();

                row_label(ui, "文件编号");
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.file_number)
                        .hint_text("可留空")
                        .desired_width(field_width),
                );
                ui.end_row();

                row_label(ui, "版本号");
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.version)
                        .hint_text("例如：V1.0")
                        .desired_width(field_width),
                );
                ui.end_row();
            });

        ui.add_space(12.0);
        ui.strong("题名与编制");
        ui.add_space(4.0);
        egui::Grid::new("research_author_grid")
            .num_columns(2)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                required_row_label(ui, "文件名称", None);
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.title_hint)
                        .hint_text("研究报告标题")
                        .desired_width(field_width),
                );
                ui.end_row();

                required_row_label(ui, "撰写单位", None);
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.institution)
                        .hint_text("编制或撰写单位")
                        .desired_width(field_width),
                );
                ui.end_row();

                required_row_label(ui, "撰写时间", Some("研究报告封面按年月显示。"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.date)
                        .hint_text("例如：2026年9月")
                        .desired_width(field_width),
                );
                ui.end_row();
            });

        ui.add_space(12.0);
        ui.strong("参考文献");
        ui.add_space(4.0);
        egui::Grid::new("research_bibliography_grid")
            .num_columns(2)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                row_label_with_info(
                    ui,
                    "BibTeX",
                    "文件内容随稿件保存；移动或删除原始 .bib 文件不影响导出。",
                );
                ui.horizontal(|ui| {
                    let label = if self.doc.draft.research.bibliography_name.is_empty() {
                        "选择 .bib 文件"
                    } else {
                        self.doc.draft.research.bibliography_name.as_str()
                    };
                    if ui
                        .add(theme::icon_text_button(theme::Icon::FileUp, label))
                        .clicked()
                    {
                        pick_bibliography = true;
                    }
                    if !self.doc.draft.research.bibliography_content.is_empty()
                        && ui.small_button("移除").clicked()
                    {
                        clear_bibliography = true;
                    }
                });
                ui.end_row();
            });

        if clear_bibliography {
            self.doc.draft.research.bibliography_name.clear();
            self.doc.draft.research.bibliography_content.clear();
        }
        if pick_bibliography
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("BibTeX", &["bib"])
                .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    self.doc.draft.research.bibliography_name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| "references.bib".to_string());
                    self.doc.draft.research.bibliography_content = content;
                    *self.status = "参考文献已随稿件保存。".into();
                }
                Err(error) => {
                    *self.status = format!("读取 BibTeX 失败：{error}");
                }
            }
        }

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(
                "正文层级：# 报告题名（至多一个，只上封面）；## 章、### 节、#### 小节；不要手工编号。最终版式以 TeX 编译 PDF 为准。",
            )
            .size(11.0)
            .color(theme::text_soft()),
        );
    }
}
