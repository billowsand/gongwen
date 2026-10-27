//! 三类材料的添加、库内选择、粘贴与原文查看。

use super::*;
use crate::manuscript::ManuscriptFilter;
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickerKind {
    Paste,
    Manuscript,
    Knowledge,
}

pub(super) struct MaterialPicker {
    role: SourceRole,
    kind: PickerKind,
    title: String,
    text: String,
    keyword: String,
    rows: Vec<(i64, String)>,
    error: Option<String>,
}

pub(super) struct SourceViewer {
    pub title: String,
    pub text: String,
}

impl DraftPage<'_> {
    pub(super) fn writing_source_group(&mut self, ui: &mut egui::Ui, role: SourceRole) {
        ui.group(|ui| {
            ui.set_min_width(120.0);
            ui.strong(role.label());
            ui.weak(match role {
                SourceRole::Content => "提供本文事实与观点",
                SourceRole::Style => "学习怎么表达",
                SourceRole::Framework => "学习如何组织篇章",
            });
            let sources: Vec<_> = self
                .doc
                .writing
                .task
                .sources
                .iter()
                .filter(|source| source.role == role)
                .map(|source| {
                    (
                        source.id.clone(),
                        source.title.clone(),
                        source.text.chars().count(),
                    )
                })
                .collect();
            let mut remove = None;
            for (id, title, chars) in sources {
                ui.separator();
                if ui.link(&title).clicked()
                    && let Some(text) = self.doc.writing.task.source_text(&id)
                {
                    self.doc.writing.source_viewer = Some(SourceViewer {
                        title: title.clone(),
                        text: text.to_string(),
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.weak(format!("{chars} 字"));
                    if ui.small_button("移除").clicked() {
                        remove = Some(id);
                    }
                });
            }
            if let Some(id) = remove {
                // 已有处理记录的材料移除后不能恢复，先确认。
                if self.doc.writing.task.source_impact(&id) == (0, 0) {
                    self.doc.writing.task.remove_source(&id);
                    self.doc.writing.changed();
                } else {
                    self.doc.writing.confirm = Some(super::Confirm::RemoveSource(id));
                }
            }
            ui.menu_button("＋ 添加材料", |ui| {
                if ui.button("从文件导入…").clicked() {
                    self.actions
                        .push(crate::app::DraftAction::ImportWritingSources {
                            key: self.doc.key,
                            role,
                        });
                    ui.close();
                }
                for (kind, label) in [
                    (PickerKind::Manuscript, "从稿件库选择…"),
                    (PickerKind::Knowledge, "从知识库选择…"),
                    (PickerKind::Paste, "粘贴文本…"),
                ] {
                    if ui.button(label).clicked() {
                        self.open_writing_picker(role, kind);
                        ui.close();
                    }
                }
            });
        });
    }

    fn open_writing_picker(&mut self, role: SourceRole, kind: PickerKind) {
        let result = match kind {
            PickerKind::Manuscript => self
                .store
                .as_deref_mut()
                .ok_or_else(|| anyhow::anyhow!("稿件库尚不可用"))
                .and_then(|store| store.list(&ManuscriptFilter::default()))
                .map(|rows| rows.into_iter().map(|row| (row.id, row.title)).collect()),
            PickerKind::Knowledge => crate::storage::manuscript_db_path()
                .and_then(|path| crate::knowledge::KnowledgeStore::open(&path)?.list_docs(None))
                .map(|rows| rows.into_iter().map(|row| (row.id, row.title)).collect()),
            PickerKind::Paste => Ok(Vec::new()),
        };
        let (rows, error) = match result {
            Ok(rows) => (rows, None),
            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
        };
        self.doc.writing.picker = Some(MaterialPicker {
            role,
            kind,
            title: String::new(),
            text: String::new(),
            keyword: String::new(),
            rows,
            error,
        });
    }

    pub(super) fn writing_material_modal(&mut self, ctx: &egui::Context) {
        if let Some(mut picker) = self.doc.writing.picker.take() {
            let mut keep = true;
            let mut added = false;
            let title = format!("添加{}", picker.role.label());
            egui::Window::new(title)
                .id(egui::Id::new(("writing_material_picker", self.doc.key)))
                .open(&mut keep)
                .default_width(580.0)
                .collapsible(false)
                .show(ctx, |ui| {
                    if let Some(error) = &picker.error {
                        ui.colored_label(theme::warn(), error);
                    }
                    if picker.kind == PickerKind::Paste {
                        ui.label("材料名称");
                        ui.add(
                            egui::TextEdit::singleline(&mut picker.title)
                                .desired_width(f32::INFINITY),
                        );
                        ui.label("原文内容");
                        ui.add(
                            egui::TextEdit::multiline(&mut picker.text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(14),
                        );
                        if ui
                            .add_enabled(
                                !picker.title.trim().is_empty() && !picker.text.trim().is_empty(),
                                egui::Button::new("加入本次写作"),
                            )
                            .clicked()
                        {
                            match SourceSnapshot::new(
                                picker.title.clone(),
                                picker.role,
                                picker.text.clone(),
                            ) {
                                Ok(source) => {
                                    self.doc.writing.task.add_source(source);
                                    self.doc.writing.changed();
                                    added = true;
                                }
                                Err(error) => picker.error = Some(error.to_string()),
                            }
                        }
                    } else {
                        ui.add(
                            egui::TextEdit::singleline(&mut picker.keyword)
                                .hint_text("按标题筛选")
                                .desired_width(f32::INFINITY),
                        );
                        let mut choose = None;
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for (id, title) in picker
                                    .rows
                                    .iter()
                                    .filter(|(_, title)| title.contains(picker.keyword.trim()))
                                {
                                    if ui.button(title).clicked() {
                                        choose = Some(*id);
                                    }
                                }
                            });
                        if let Some(id) = choose {
                            let result = match picker.kind {
                                PickerKind::Manuscript => self
                                    .store
                                    .as_deref_mut()
                                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))
                                    .and_then(|store| store.get(id))
                                    .and_then(|record| {
                                        record
                                            .map(|record| (record.title, record.content_markdown))
                                            .ok_or_else(|| anyhow::anyhow!("稿件已不存在"))
                                    }),
                                PickerKind::Knowledge => crate::storage::manuscript_db_path()
                                    .and_then(|path| {
                                        crate::knowledge::KnowledgeStore::open(&path)?
                                            .get_doc_content(id)
                                    })
                                    .and_then(|record| {
                                        record.ok_or_else(|| anyhow::anyhow!("知识库资料已不存在"))
                                    }),
                                PickerKind::Paste => Err(anyhow::anyhow!("粘贴材料不使用库内选择")),
                            }
                            .and_then(|(title, text)| {
                                SourceSnapshot::new(title, picker.role, text)
                            });
                            match result {
                                Ok(source) => {
                                    self.doc.writing.task.add_source(source);
                                    self.doc.writing.changed();
                                    added = true;
                                }
                                Err(error) => picker.error = Some(format!("{error:#}")),
                            }
                        }
                        ui.weak("添加时保存原文快照，资料库中的后续修改不会悄悄改变本次依据。");
                    }
                });
            if keep && !added {
                self.doc.writing.picker = Some(picker);
            }
        }
        if let Some(viewer) = self.doc.writing.source_viewer.take() {
            let mut keep = true;
            egui::Window::new(&viewer.title)
                .id(egui::Id::new(("writing_source_viewer", self.doc.key)))
                .open(&mut keep)
                .default_width(700.0)
                .default_height(520.0)
                .show(ctx, |ui| {
                    ui.weak("本次写作使用的固定原文快照");
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(&viewer.text);
                    });
                });
            if keep {
                self.doc.writing.source_viewer = Some(viewer);
            }
        }
    }
}
