//! 提炼成果、失败块处置与卡片核对。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_artifacts(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("查看提炼成果");
        let coverage = self.doc.writing.coverage();
        let query = self.doc.writing.task.topic_query();
        let failed = self
            .doc
            .writing
            .task
            .chunks
            .iter()
            .filter(|chunk| {
                chunk.role == SourceRole::Content
                    && chunk.section_id.is_none()
                    && chunk.query == query
                    && chunk.status == ChunkStatus::Failed
            })
            .count();
        ui.weak(format!(
            "内容块处理：{} / {}（人工跳过 {}，失败 {}）",
            coverage.done, coverage.total, coverage.skipped, failed
        ));
        ui.horizontal_wrapped(|ui| {
            for (index, label) in ["内容素材", "写作风格", "篇章框架"].into_iter().enumerate()
            {
                ui.selectable_value(&mut self.doc.writing.artifact_tab, index, label);
            }
        });
        ui.separator();
        match self.doc.writing.artifact_tab {
            1 | 2 => {
                let style = self.doc.writing.artifact_tab == 1;
                ui.label(if style {
                    "可直接编辑，拟纲、写作与统稿均使用此写法要求。"
                } else {
                    "可直接编辑；这里只规定篇章结构，实际版式沿用本地模板。"
                });
                let text = if style {
                    &mut self.doc.writing.task.style_md
                } else {
                    &mut self.doc.writing.task.framework_md
                };
                if ui
                    .add(
                        egui::TextEdit::multiline(text)
                            .desired_width(f32::INFINITY)
                            .desired_rows(18),
                    )
                    .changed()
                {
                    self.doc.writing.task.style_changed(style);
                    self.doc.writing.changed();
                }
                ui.horizontal(|ui| {
                    let text = if style {
                        &self.doc.writing.task.style_md
                    } else {
                        &self.doc.writing.task.framework_md
                    };
                    if ui.button("复制内容").clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                    if ui.button("导出 Markdown…").clicked() {
                        self.actions
                            .push(crate::app::DraftAction::ExportWritingArtifact {
                                name: if style { "style.md" } else { "framework.md" }.into(),
                                text: text.clone(),
                            });
                    }
                });
                let role = if style {
                    SourceRole::Style
                } else {
                    SourceRole::Framework
                };
                if let Some(next) = self.writing_chunk_actions(ui, role, None) {
                    *action = Some(next);
                }
            }
            _ => {
                let cards: Vec<_> = self
                    .doc
                    .writing
                    .task
                    .current_topic_cards()
                    .into_iter()
                    .cloned()
                    .collect();
                if cards.is_empty() {
                    ui.label("尚无相关观点。检查材料与主题，或先开始提炼。");
                }
                self.writing_cards(ui, &cards);
                if let Some(next) = self.writing_chunk_actions(ui, SourceRole::Content, None) {
                    *action = Some(next);
                }
            }
        }
    }

    /// 失败块与人工跳过的原地处置：材料名、原文位置、原因、操作按钮。
    pub(crate) fn writing_chunk_actions(
        &mut self,
        ui: &mut egui::Ui,
        role: SourceRole,
        section_id: Option<&str>,
    ) -> Option<Action> {
        let task = &self.doc.writing.task;
        let query = match section_id {
            Some(id) => {
                let section = task.sections.iter().find(|section| section.id == id)?;
                task.section_query(section)
            }
            None => {
                if role != SourceRole::Content {
                    String::new()
                } else {
                    task.topic_query()
                }
            }
        };
        let rows: Vec<_> = task
            .chunks
            .iter()
            .filter(|chunk| {
                chunk.role == role
                    && chunk.section_id.as_deref() == section_id
                    && chunk.query == query
                    && matches!(
                        chunk.status,
                        ChunkStatus::Failed | ChunkStatus::SkippedByUser
                    )
            })
            .cloned()
            .collect();
        if rows.is_empty() {
            return None;
        }
        ui.add_space(6.0);
        ui.strong("失败与人工跳过");
        let mut action = None;
        let mut skip = None;
        let mut unskip = None;
        for chunk in rows {
            let Some(source) = task
                .sources
                .iter()
                .find(|source| source.id == chunk.source_id)
                .cloned()
            else {
                ui.weak(format!(
                    "原文块 {}..{}：材料已移除，记录不再处理",
                    chunk.start, chunk.end
                ));
                continue;
            };
            ui.push_id(
                ("writing_chunk", &chunk.source_id, chunk.start, chunk.end),
                |ui| match chunk.status {
                    ChunkStatus::Failed => {
                        ui.colored_label(
                            theme::warn(),
                            format!(
                                "《{}》原文 {}..{}：{}",
                                source.title,
                                chunk.start,
                                chunk.end,
                                chunk.error.as_deref().unwrap_or("此块未能成功处理，可重试")
                            ),
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("重试此块").clicked() {
                                action = Some(Action::RetryChunk(writing::ChunkRef {
                                    source_id: chunk.source_id.clone(),
                                    section_id: chunk.section_id.clone(),
                                    role,
                                    start: chunk.start,
                                    end: chunk.end,
                                }));
                            }
                            if role == SourceRole::Content
                                && ui.button("人工确认无相关内容").clicked()
                            {
                                skip = Some((source.clone(), chunk.clone()));
                            }
                        });
                    }
                    ChunkStatus::SkippedByUser => {
                        ui.label(format!(
                            "《{}》原文 {}..{}：人工跳过（计入覆盖，不是模型判定）",
                            source.title, chunk.start, chunk.end
                        ));
                        if ui.button("撤销人工跳过").clicked() {
                            unskip = Some((source.clone(), chunk.clone()));
                        }
                    }
                    _ => {}
                },
            );
        }
        if let Some((source, chunk)) = skip {
            self.doc.writing.task.mark_chunk_skipped(
                &source,
                chunk.section_id.as_deref(),
                &chunk.query,
                chunk.start,
                chunk.end,
            );
            self.doc.writing.changed();
        }
        if let Some((source, chunk)) = unskip {
            self.doc.writing.task.unskip_chunk(
                &source,
                chunk.section_id.as_deref(),
                &chunk.query,
                chunk.start,
                chunk.end,
            );
            self.doc.writing.changed();
        }
        action
    }
}
