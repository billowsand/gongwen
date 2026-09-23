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

                row_label_with_info(
                    ui,
                    "文件类型",
                    "决定封面样式：立项论证、建设实施、技术实现、项目总结为项目类，封面印阶段条；其余为研究类，封面印署名行。可从右侧下拉选，也可手填。",
                );
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add(
                        egui::TextEdit::singleline(&mut self.doc.draft.research.file_type)
                            .desired_width((field_width - 32.0).max(80.0)),
                    );
                    egui::ComboBox::from_id_salt("research_file_type_presets")
                        .selected_text("")
                        .width(20.0)
                        .show_ui(ui, |ui| {
                            for (index, preset) in mdx::cover::DOC_TYPE_PRESETS.iter().enumerate() {
                                if index == 4 {
                                    ui.separator();
                                }
                                ui.selectable_value(
                                    &mut self.doc.draft.research.file_type,
                                    preset.to_string(),
                                    *preset,
                                );
                            }
                        });
                });
                ui.end_row();

                row_label(ui, "文件编号");
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.file_number)
                        .hint_text("可留空")
                        .desired_width(field_width),
                );
                ui.end_row();

                row_label(ui, "版本稿次");
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.version)
                        .hint_text("例如：V1.0、送审稿；封面上加括号印在题名下方")
                        .desired_width(field_width),
                );
                ui.end_row();

                let family = mdx::cover::Family::of(&self.doc.draft.research.file_type);
                let hints = cover_hints(family);
                row_label_with_info(
                    ui,
                    "标识行",
                    "印在封面文种下方，按原样排。可留空。",
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.ident)
                        .hint_text(hints.ident)
                        .desired_width(field_width),
                );
                ui.end_row();

                if !family.is_project() {
                    row_label_with_info(
                        ui,
                        "署名行",
                        "研究类封面印在落款上方，按原样排。项目类封面这里是阶段条，不印署名行。",
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.doc.draft.research.byline)
                            .hint_text(hints.byline)
                            .desired_width(field_width),
                    );
                    ui.end_row();
                }

                if family == mdx::cover::Family::Research(mdx::cover::ResearchKind::Translation)
                {
                    row_label_with_info(ui, "外文原题", "以西文斜体印在中文题名下方。");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.doc.draft.research.original_title)
                            .hint_text("例如：Artificial Intelligence Risk Management Framework")
                            .desired_width(field_width),
                    );
                    ui.end_row();
                }
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

                required_row_label(
                    ui,
                    "撰写时间",
                    Some("封面按年月排，自动写成汉字数字，如二〇二六年九月。"),
                );
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

/// 封面可选行的输入提示，按文件类型给出本文种的写法示例。
struct CoverHints {
    ident: &'static str,
    byline: &'static str,
}

fn cover_hints(family: mdx::cover::Family) -> CoverHints {
    use mdx::cover::{Family, ResearchKind};
    match family {
        Family::Project { .. } => CoverHints {
            ident: "例如：项目编号：XM-2026-014",
            byline: "",
        },
        Family::Research(ResearchKind::Consulting) => CoverHints {
            ident: "例如：二〇二六年第3期（总第27期）",
            byline: "例如：供领导决策参考",
        },
        Family::Research(ResearchKind::Topic) => CoverHints {
            ident: "例如：课题编号：ZT-2026-07",
            byline: "例如：政务智能化专题课题组",
        },
        Family::Research(ResearchKind::Translation) => CoverHints {
            ident: "例如：原文：美国国家标准与技术研究院，2023年1月",
            byline: "例如：编译：信息资源处　审校：研究室",
        },
        Family::Research(ResearchKind::Other) => CoverHints {
            ident: "可留空",
            byline: "可留空",
        },
    }
}
