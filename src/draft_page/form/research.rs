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
                // 可手填也可选预置：下拉箭头画在输入框右端、跟输入框是同一个控件，
                // 不另挂一个 ComboBox——那样高度、圆角都和上下各行对不齐。
                let arrow_width = 22.0;
                let text = ui.add(
                    egui::TextEdit::singleline(&mut self.doc.draft.research.file_type)
                        .margin(egui::Margin {
                            left: 4,
                            right: arrow_width as i8,
                            top: 2,
                            bottom: 2,
                        })
                        .desired_width(field_width),
                );
                let arrow_rect = egui::Rect::from_min_max(
                    egui::pos2(text.rect.right() - arrow_width, text.rect.top()),
                    text.rect.right_bottom(),
                );
                let arrow = ui
                    .interact(
                        arrow_rect,
                        ui.id().with("research_file_type_presets"),
                        egui::Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                let icon = egui::Rect::from_center_size(arrow_rect.center(), egui::vec2(9.0, 6.0));
                ui.painter().add(egui::Shape::convex_polygon(
                    vec![icon.left_top(), icon.right_top(), icon.center_bottom()],
                    ui.style().interact(&arrow).fg_stroke.color,
                    egui::Stroke::NONE,
                ));
                egui::Popup::menu(&arrow)
                    .anchor(text.rect)
                    .align(egui::RectAlign::BOTTOM_START)
                    .width(field_width)
                    .show(|ui| {
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
        let mut reload_bibliography = None;
        egui::Grid::new("research_bibliography_grid")
            .num_columns(2)
            .min_row_height(FORM_CONTROL_HEIGHT)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                row_label_with_info(
                    ui,
                    "BibTeX",
                    "文件内容随稿件保存；移动或删除原始 .bib 文件不影响导出。                     在外部改了 .bib，点「重新读入」换成新内容。",
                );
                ui.horizontal(|ui| {
                    let label = if self.doc.draft.research.bibliography_name.is_empty() {
                        "选择 .bib 文件"
                    } else {
                        self.doc.draft.research.bibliography_name.as_str()
                    };
                    if ui
                        .add(theme::icon_text_button(theme::Icon::FileUp, label))
                        .on_hover_text("选一个 .bib 文件，替换现有的文献库")
                        .clicked()
                    {
                        pick_bibliography = true;
                    }
                    if let Some(path) = &self.doc.bibliography_source
                        && ui
                            .small_button("重新读入")
                            .on_hover_text(format!("从 {} 再读一遍", path.display()))
                            .clicked()
                    {
                        reload_bibliography = Some(path.clone());
                    }
                    if !self.doc.draft.research.bibliography_content.is_empty()
                        && ui.small_button("移除").clicked()
                    {
                        clear_bibliography = true;
                    }
                });
                ui.end_row();

                if !self.doc.draft.research.bibliography_content.trim().is_empty() {
                    row_label(ui, "文献");
                    self.bibliography_summary(ui);
                    ui.end_row();
                }
            });

        if clear_bibliography {
            self.doc.draft.research.bibliography_name.clear();
            self.doc.draft.research.bibliography_content.clear();
            self.doc.bibliography_source = None;
        }
        if pick_bibliography {
            self.import_bibliography(None);
        } else if let Some(path) = reload_bibliography {
            self.import_bibliography(Some(path));
        }

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(
                "正文层级：# 报告题名（至多一个，只上封面）；## 章、### 节、#### 小节；不要手工编号。最终版式以导出的 PDF 为准。",
            )
            .size(11.0)
            .color(theme::text_soft()),
        );
    }
}

impl DraftPage<'_> {
    /// 读入一个 .bib 作为本稿的文献库。`path` 为空时弹文件框（文献引用下拉里的
    /// 「导入 .bib…」也走这里）；给了路径就是「重新读入」。
    pub(crate) fn import_bibliography(&mut self, path: Option<std::path::PathBuf>) {
        let reload = path.is_some();
        let Some(path) = path.or_else(|| {
            rfd::FileDialog::new()
                .add_filter("BibTeX", &["bib"])
                .pick_file()
        }) else {
            return;
        };
        match crate::text_file::read_to_string(&path) {
            Ok(content) => {
                self.doc.draft.research.bibliography_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "references.bib".to_string());
                let changed = self.doc.draft.research.bibliography_content != content;
                self.doc.draft.research.bibliography_content = content;
                self.doc.bibliography_source = Some(path);
                let library = crate::export::bibliography::library(
                    &self.doc.draft.research.bibliography_content,
                );
                *self.status = match (&library.problem, reload, changed) {
                    (Some(problem), ..) => format!(
                        "参考文献已读入，但 BibTeX 第 {} 行有错：{}。改好前导不出 PDF。",
                        problem.line, problem.message
                    ),
                    (None, true, false) => "参考文献没有变化。".into(),
                    (None, true, true) => {
                        format!("已重新读入参考文献，共 {} 条。", library.entries.len())
                    }
                    (None, false, _) => {
                        format!("参考文献已随稿件保存，共 {} 条。", library.entries.len())
                    }
                };
            }
            Err(error) => {
                *self.status = format!("读取 BibTeX 失败：{error}");
            }
        }
    }

    /// 文献库摘要：共几条、正文引了几条；.bib 有错时橙色指出第几行。
    fn bibliography_summary(&self, ui: &mut egui::Ui) {
        let library =
            crate::export::bibliography::library(&self.doc.draft.research.bibliography_content);
        let cited = crate::preview::research_citations(ui.ctx(), &self.doc.generated_markdown);
        let total = library.entries.len();
        let used = cited
            .iter()
            .filter(|cited| library.contains(&cited.key))
            .count();
        let missing = cited.len() - used;
        ui.vertical(|ui| {
            let mut summary = format!(
                "共 {total} 条 · 已引 {used} · 未引 {}",
                total.saturating_sub(used)
            );
            if missing > 0 {
                summary.push_str(&format!(" · 缺 {missing} 个键"));
            }
            ui.label(egui::RichText::new(summary).color(theme::text_soft()))
                .on_hover_text(
                    "按正文引用统计；未引的文献不进参考文献表。                     缺的键是正文引了、文献库里没有的，PDF 导出会中止。",
                );
            if let Some(problem) = &library.problem {
                ui.label(
                    egui::RichText::new(format!(
                        "第 {} 行：{}。导出 PDF 会中止。",
                        problem.line, problem.message
                    ))
                    .size(theme::font_sizes::SMALL)
                    .color(theme::warn()),
                );
            }
        });
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
