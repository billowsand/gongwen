//! 六阶段的成果编辑、就绪提示与交付操作。

use super::*;
use crate::agent::writing::{ChunkStatus, MaterialCard, Section};
use crate::app::GongwenApp;
use crate::theme;

impl DraftPage<'_> {
    pub(super) fn writing_primary_action(
        &mut self,
        ui: &mut egui::Ui,
        editable: bool,
        action: &mut Option<Action>,
    ) {
        let stage = self.doc.writing.task.stage;
        let (label, next, required) = match stage {
            Stage::Prepare => ("开始提炼", Some(Action::Extract), Stage::Extract),
            Stage::Extract => ("生成提纲", Some(Action::Plan), Stage::Outline),
            Stage::Outline => (
                "确认提纲，准备各节素材",
                Some(Action::Collect),
                Stage::Outline,
            ),
            Stage::Collect => ("开始分节写作", Some(Action::Write), Stage::Write),
            Stage::Write => ("合并全文，开始统稿", Some(Action::Polish), Stage::Review),
            Stage::Review => ("审阅并合并到正文…", None, Stage::Review),
        };
        let ready = self.doc.writing.ready(required);
        let response = theme::primary_icon_button_enabled(
            ui,
            editable && ready.is_ok(),
            theme::Icon::Sparkles,
            label,
        );
        if let Err(error) = ready {
            response.on_disabled_hover_text(error.to_string());
            return;
        }
        if !response.clicked() {
            return;
        }
        if stage == Stage::Review {
            self.deliver_writing_proposal();
        } else if stage == Stage::Outline {
            match self.doc.writing.task.confirm_outline() {
                Ok(()) => {
                    self.doc.writing.changed();
                    *action = next;
                }
                Err(error) => self.doc.writing.task.error = Some(error.to_string()),
            }
        } else {
            *action = next;
        }
    }

    pub(super) fn writing_prepare(&mut self, ui: &mut egui::Ui) {
        ui.heading("先备好材料，再开始写");
        ui.label("内容材料提供事实，风格样稿学习表达，结构范文学习篇章组织。");
        ui.add_space(12.0);
        ui.strong("这篇公文要写什么");
        let topic_changed = ui
            .add(
                egui::TextEdit::multiline(&mut self.doc.writing.task.topic)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("写明主题、受文对象、写作目的与重点"),
            )
            .changed();
        let mut length_changed = false;
        ui.horizontal(|ui| {
            ui.label("目标篇幅");
            length_changed = ui
                .add(
                    egui::DragValue::new(&mut self.doc.writing.task.target_chars)
                        .range(200..=30_000)
                        .speed(50)
                        .suffix(" 字"),
                )
                .changed();
            ui.weak(format!("文种：{}", self.doc.draft.kind.label()));
        });
        if topic_changed {
            self.doc.writing.task.invalidate_from(Stage::Extract);
            self.doc.writing.changed();
        } else if length_changed {
            self.doc.writing.task.invalidate_from(Stage::Outline);
            self.doc.writing.changed();
        }
        ui.add_space(14.0);
        if ui.available_width() >= 680.0 {
            ui.columns(3, |columns| {
                for (column, role) in columns.iter_mut().zip(SourceRole::ALL) {
                    self.writing_source_group(column, role);
                }
            });
        } else {
            for role in SourceRole::ALL {
                self.writing_source_group(ui, role);
                ui.add_space(12.0);
            }
        }
        for error in &self.doc.writing.import_errors {
            ui.colored_label(theme::warn(), error);
        }
        ui.add_space(10.0);
        ui.weak(
            "无风格或结构范文时采用文种默认要求。文件导入在后台处理，解析失败的材料不会计为可用。",
        );
    }

    pub(super) fn writing_artifacts(&mut self, ui: &mut egui::Ui) {
        ui.heading("查看提炼成果");
        let coverage = self.doc.writing.task.coverage();
        let failed = self
            .doc
            .writing
            .task
            .chunks
            .iter()
            .filter(|chunk| chunk.status == ChunkStatus::Failed)
            .count();
        ui.weak(format!(
            "内容块处理：{} / {}，失败条目 {} 块",
            coverage.done, coverage.total, failed
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
                    if style {
                        self.doc.writing.task.style_manual = true;
                    } else {
                        self.doc.writing.task.framework_manual = true;
                    }
                    self.doc.writing.task.invalidate_from(Stage::Outline);
                    self.doc.writing.task.stage = Stage::Extract;
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
            }
            _ => {
                let cards = self.doc.writing.task.topic_cards.clone();
                if cards.is_empty() {
                    ui.label("尚无相关观点。检查材料与主题，或先开始提炼。");
                }
                self.writing_cards(ui, &cards);
            }
        }
    }

    pub(super) fn writing_outline(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("先把提纲定下来");
        ui.weak("直接编辑标题、要点与篇幅。确认后再按各节主题重新读取全部内容材料。");
        let mut changed = false;
        let mut move_up = None;
        let mut remove = None;
        let count = self.doc.writing.task.sections.len();
        for (index, section) in self.doc.writing.task.sections.iter_mut().enumerate() {
            ui.push_id(&section.id, |ui| {
                ui.separator();
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut section.title)
                            .desired_width(f32::INFINITY)
                            .hint_text("本节标题"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::TextEdit::multiline(&mut section.points)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("本节应回答什么问题、有哪些必写要点"),
                    )
                    .changed();
                ui.horizontal_wrapped(|ui| {
                    ui.label("本节约");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut section.target_chars)
                                .range(100..=10_000)
                                .speed(50)
                                .suffix(" 字"),
                        )
                        .changed();
                    if ui
                        .add_enabled(index > 0, egui::Button::new("↑ 上移"))
                        .clicked()
                    {
                        move_up = Some(index);
                    }
                    if ui
                        .add_enabled(count > 1, egui::Button::new("删除本节"))
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
            });
        }
        if let Some(index) = move_up {
            self.doc.writing.task.sections.swap(index, index - 1);
            changed = true;
        }
        if let Some(index) = remove {
            let removed = self.doc.writing.task.sections.remove(index);
            self.doc.writing.task.section_history.push(removed);
            changed = true;
        }
        if ui.button("＋ 增加一节").clicked() {
            self.doc.writing.task.sections.push(Section {
                id: uuid::Uuid::new_v4().to_string(),
                title: "新增章节".into(),
                target_chars: 300,
                ..Default::default()
            });
            changed = true;
        }
        if !self.doc.writing.task.section_history.is_empty() {
            let mut restore = None;
            ui.collapsing("历史提纲中的章节", |ui| {
                for section in self.doc.writing.task.section_history.clone().iter().rev() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&section.title);
                        if ui.button("查看原稿").clicked() {
                            self.doc.writing.source_viewer = Some(SourceViewer {
                                title: section.title.clone(),
                                text: section.markdown.clone(),
                            });
                        }
                        if ui.button("恢复为一节").clicked() {
                            restore = Some(section.clone());
                        }
                    });
                }
            });
            if let Some(mut section) = restore {
                section.id = uuid::Uuid::new_v4().to_string();
                section.manually_edited = true;
                section.rewrite_in_progress = false;
                section.merge_state = None;
                self.doc.writing.task.sections.push(section);
                changed = true;
            }
        }
        if changed {
            self.doc.writing.task.invalidate_from(Stage::Collect);
            self.doc.writing.task.outline_confirmed = false;
            self.doc.writing.task.stage = Stage::Outline;
            self.doc.writing.changed();
        }
        ui.add_space(14.0);
        ui.strong("让 AI 按要求调整提纲");
        ui.add(
            egui::TextEdit::multiline(&mut self.doc.writing.revise)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("例如：合并相近的两节，补充责任分工，调整先后顺序"),
        );
        if ui
            .add_enabled(
                !self.doc.writing.revise.trim().is_empty(),
                egui::Button::new("调整提纲"),
            )
            .clicked()
        {
            *action = Some(Action::ReviseOutline(self.doc.writing.revise.clone()));
        }
        ui.weak("提纲未确认前不会开始写正文。重新生成提纲后仍需确认。");
    }

    fn writing_section_tabs(&mut self, ui: &mut egui::Ui) {
        self.doc.writing.section = self
            .doc
            .writing
            .section
            .min(self.doc.writing.task.sections.len().saturating_sub(1));
        ui.horizontal_wrapped(|ui| {
            for (index, section) in self.doc.writing.task.sections.iter().enumerate() {
                ui.selectable_value(&mut self.doc.writing.section, index, &section.title);
            }
        });
        ui.separator();
    }

    pub(super) fn writing_collection(&mut self, ui: &mut egui::Ui) {
        ui.heading("围绕每一节，重新读原始材料");
        self.writing_section_tabs(ui);
        let Some(section) = self.doc.writing.task.sections.get(self.doc.writing.section) else {
            return;
        };
        let section = section.clone();
        ui.strong(&section.title);
        ui.label(&section.points);
        let progress: Vec<_> = self
            .doc
            .writing
            .task
            .chunks
            .iter()
            .filter(|chunk| chunk.section_id.as_deref() == Some(&section.id))
            .collect();
        let coverage = self.doc.writing.task.section_coverage(&section.id);
        let done = coverage.done;
        ui.add(
            egui::ProgressBar::new(if coverage.total == 0 {
                0.0
            } else {
                done as f32 / coverage.total as f32
            })
            .text(format!("已处理 {done} / {} 块原文", coverage.total)),
        );
        for source in self
            .doc
            .writing
            .task
            .sources
            .iter()
            .filter(|source| source.role == SourceRole::Content)
        {
            let chunks: Vec<_> = progress
                .iter()
                .filter(|chunk| chunk.source_id == source.id)
                .collect();
            let count = chunks
                .iter()
                .filter(|chunk| {
                    matches!(
                        chunk.status,
                        ChunkStatus::Extracted | ChunkStatus::NoRelevantContent
                    )
                })
                .count();
            ui.label(format!("《{}》已处理 {count} 块", source.title));
        }
        for chunk in progress
            .iter()
            .filter(|chunk| chunk.status == ChunkStatus::Failed)
        {
            ui.colored_label(
                theme::warn(),
                chunk
                    .error
                    .as_deref()
                    .unwrap_or("此块未能成功提取，接着跑可重试"),
            );
        }
        ui.separator();
        self.writing_cards(ui, &section.cards);
    }

    pub(super) fn writing_sections(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        ui.heading("一节一节写，随时可以修改");
        self.writing_section_tabs(ui);
        let Some(section) = self
            .doc
            .writing
            .task
            .sections
            .get_mut(self.doc.writing.section)
        else {
            return;
        };
        ui.strong(&section.title);
        ui.weak(&section.status);
        let changed = ui
            .add_enabled(
                !section.locked,
                egui::TextEdit::multiline(&mut section.markdown)
                    .desired_width(f32::INFINITY)
                    .desired_rows(18),
            )
            .changed();
        if changed {
            section.manually_edited = true;
        }
        let confirm_manual = section.manually_edited
            && section.status == "写作中"
            && !section.markdown.trim().is_empty()
            && ui.button("确认本节人工稿已完成").clicked();
        if confirm_manual {
            section.status = "人工已完成".into();
        }
        let lock_changed = ui
            .checkbox(&mut section.locked, "锁定本节，统稿只提建议")
            .changed();
        if changed || lock_changed || confirm_manual {
            self.doc.writing.task.invalidate_from(Stage::Review);
            self.doc.writing.changed();
        }
        ui.label("本节修改要求");
        ui.add(
            egui::TextEdit::multiline(&mut self.doc.writing.rewrite)
                .desired_width(f32::INFINITY)
                .desired_rows(2)
                .hint_text("例如：保留第一段，第二段压缩到两句话"),
        );
        let locked = self.doc.writing.task.sections[self.doc.writing.section].locked;
        if ui
            .add_enabled(!locked, egui::Button::new("按要求重写本节"))
            .clicked()
        {
            *action = Some(Action::RewriteSection(
                self.doc.writing.section,
                self.doc.writing.rewrite.clone(),
            ));
        }
        ui.weak("人工修改会保留；整批续写不会覆盖人工改过或已锁定的章节。重写只针对当前节。");
        self.writing_section_history(ui);
    }

    pub(super) fn writing_review(&mut self, ui: &mut egui::Ui) {
        ui.heading("通读统稿，再审阅合并");
        ui.weak("这里仍是工作稿。接受后进入正文，再使用现有预览、排版和导出。");
        if !self.doc.writing.task.review_notes.is_empty() {
            ui.collapsing("全篇结构与衔接检查", |ui| {
                ui.label(&self.doc.writing.task.review_notes);
            });
        }
        let problems = self.writing_delivery_issues();
        for problem in &problems {
            ui.colored_label(theme::warn(), problem);
        }
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.doc.writing.task.final_markdown)
                    .desired_width(f32::INFINITY)
                    .desired_rows(24),
            )
            .changed()
        {
            self.doc.writing.task.resume_action = None;
            self.doc.writing.task.status = "统稿已人工修改，请重新核对后审阅".into();
            self.doc.writing.changed();
        }
        ui.weak("差异审阅支持逐块排除。正式正文已有改动时，接受入口会再次检查基线与关键事实。");
    }

    pub(super) fn writing_evidence(&mut self, ui: &mut egui::Ui) {
        ui.strong("本次写作");
        ui.label(&self.doc.writing.task.topic);
        ui.weak(format!(
            "文种：{} · 约 {} 字",
            self.doc.writing.task.kind.label(),
            self.doc.writing.task.target_chars
        ));
        ui.separator();
        ui.strong("写作约束");
        ui.collapsing("写作风格", |ui| {
            ui.label(&self.doc.writing.task.style_md);
        });
        ui.collapsing("篇章框架", |ui| {
            ui.label(&self.doc.writing.task.framework_md);
        });
        if self.doc.writing.task.stage == Stage::Collect {
            ui.separator();
            ui.strong("分节备料进度");
            for section in &self.doc.writing.task.sections {
                let coverage = self.doc.writing.task.section_coverage(&section.id);
                ui.label(&section.title);
                ui.weak(format!("已处理 {} / {} 块", coverage.done, coverage.total));
            }
        } else if self.doc.writing.task.stage == Stage::Write {
            if let Some(section) = self.doc.writing.task.sections.get(self.doc.writing.section) {
                let section = section.clone();
                ui.separator();
                ui.strong("本节职责");
                ui.label(&section.points);
                ui.weak(format!(
                    "本节约 {} 字；素材 {} 条",
                    section.target_chars,
                    section.cards.len()
                ));
                self.writing_cards(ui, &section.cards);
            }
        } else if self.doc.writing.task.stage == Stage::Review {
            ui.separator();
            ui.strong("交付前核对");
            let issues = self.writing_delivery_issues();
            if issues.is_empty() {
                ui.label("原文定位与已识别的关键事实来源检查未发现新增问题。");
                ui.weak("这些检查不能代替人工判断适用性和观点是否准确，请在审阅窗逐项核对。");
            } else {
                for issue in issues {
                    ui.colored_label(theme::warn(), issue);
                }
            }
        } else {
            ui.separator();
            ui.label("材料用于取证，样稿用于学习表达。模型提炼的观点始终保留原文供核对。");
        }
    }

    fn writing_cards(&mut self, ui: &mut egui::Ui, cards: &[MaterialCard]) {
        for card in cards {
            ui.push_id(&card.id, |ui| {
                ui.separator();
                ui.strong(&card.point);
                let verification = match card.verification {
                    crate::agent::writing::Verification::Unverified => "待人工核对",
                    crate::agent::writing::Verification::Supported => "支持",
                    crate::agent::writing::Verification::Conflict => "存在冲突",
                    crate::agent::writing::Verification::Insufficient => "依据不足",
                };
                ui.weak(format!("原句定位已命中 · 观点核验：{verification}"));
                ui.label(&card.quote);
                let mut checked =
                    card.verification == crate::agent::writing::Verification::Supported;
                if ui
                    .add_enabled(
                        !self.doc.writing.running && !self.doc.read_only(),
                        egui::Checkbox::new(&mut checked, "我已核对这条观点及其适用性"),
                    )
                    .changed()
                {
                    let rows = self.doc.writing.task.topic_cards.iter_mut().chain(
                        self.doc
                            .writing
                            .task
                            .sections
                            .iter_mut()
                            .flat_map(|section| section.cards.iter_mut()),
                    );
                    for row in rows.filter(|row| row.id == card.id) {
                        row.verification = if checked {
                            crate::agent::writing::Verification::Supported
                        } else {
                            crate::agent::writing::Verification::Unverified
                        };
                    }
                    self.doc.writing.changed();
                }
                if let Some(source) = self
                    .doc
                    .writing
                    .task
                    .sources
                    .iter()
                    .find(|source| source.id == card.source_id)
                    && ui.link(format!("查看《{}》原文", source.title)).clicked()
                {
                    self.doc.writing.source_viewer = Some(SourceViewer {
                        title: source.title.clone(),
                        text: source.text.clone(),
                    });
                }
            });
        }
    }

    fn writing_section_history(&mut self, ui: &mut egui::Ui) {
        let index = self.doc.writing.section;
        let Some(section) = self.doc.writing.task.sections.get(index) else {
            return;
        };
        let history = section.revision_history.clone();
        let mut restore = None;
        if !history.is_empty() {
            ui.collapsing(format!("本节历史稿（{} 份）", history.len()), |ui| {
                for (number, text) in history.iter().enumerate().rev() {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "第 {} 份 · {} 字",
                            number + 1,
                            text.chars().count()
                        ));
                        if ui.button("查看").clicked() {
                            self.doc.writing.source_viewer = Some(SourceViewer {
                                title: "本节历史稿".into(),
                                text: text.clone(),
                            });
                        }
                        if ui.button("恢复到工作稿").clicked() {
                            restore = Some(text.clone());
                        }
                    });
                }
            });
        }
        if let Some(text) = restore {
            let section = &mut self.doc.writing.task.sections[index];
            section.revision_history.push(section.markdown.clone());
            section.markdown = text;
            section.manually_edited = true;
            section.locked = false;
            self.doc.writing.task.invalidate_from(Stage::Review);
            self.doc.writing.changed();
        }
    }

    fn writing_delivery_issues(&mut self) -> Vec<String> {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for entry in &self.config.vocabulary {
            entry.canonical.hash(&mut hash);
            std::mem::discriminant(&entry.category).hash(&mut hash);
        }
        let key = hash.finish();
        if let Some((old, issues)) = &self.doc.writing.delivery_cache
            && *old == key
        {
            return issues.clone();
        }
        let issues =
            super::validation::delivery_issues(&self.doc.writing.task, &self.config.vocabulary);
        self.doc.writing.delivery_cache = Some((key, issues.clone()));
        issues
    }

    fn deliver_writing_proposal(&mut self) {
        if let Some(proposal) = self.doc.ai_proposal.as_mut() {
            proposal.open = true;
            self.doc.writing.task.error =
                Some("已有待审提案，请先接受或放弃，再提交本次统稿".into());
            return;
        }
        let issues = self.writing_delivery_issues();
        if !issues.is_empty() {
            self.doc.writing.task.error = Some(issues.join("；"));
            return;
        }
        let mut result = crate::draft_page::reviewed_draft(
            &self.doc.draft,
            self.config,
            &self.doc.writing.task.final_markdown,
            false,
        );
        let mut notes =
            crate::proofread::Lexicon::resolved(&self.config.proofread).check(&result.markdown);
        notes.extend(crate::proofread_rules::check_with_config(
            &self.doc.draft,
            &result.markdown,
            &self.config.proofread,
        ));
        result
            .warnings
            .extend(notes.into_iter().map(crate::models::ReviewNote::from));
        let before = self.doc.generated_markdown.clone();
        self.doc.ai_panel.open = false;
        let task = &self.doc.writing.task;
        let provenance = format!(
            "分阶段写作任务 {}；{} 份原文快照；风格与框架随任务保存",
            task.id,
            task.sources.len()
        );
        let usage = task.usage.clone();
        let turn = self.doc.ai_panel.push_turn(
            "分阶段写作统稿".into(),
            task.topic.clone(),
            vec!["固定六阶段流程".into()],
            None,
        );
        let summary = GongwenApp::install_ai_proposal(
            self.doc,
            before,
            result,
            "分阶段写作统稿".into(),
            &self.config.vocabulary,
        );
        if let Some(turn) = self.doc.ai_panel.turn_mut(turn) {
            turn.provenance = provenance;
            turn.elapsed = Some(std::time::Duration::from_millis(
                usage
                    .calls
                    .iter()
                    .fold(0u64, |sum, call| sum.saturating_add(call.elapsed_ms)),
            ));
            turn.usage = usage;
            turn.notes.push(format!(
                "写作风格快照：\n{}\n\n篇章框架快照：\n{}",
                self.doc.writing.task.style_md, self.doc.writing.task.framework_md
            ));
        }
        self.doc.writing.proposal_turn = Some(turn);
        self.doc
            .ai_panel
            .finish(crate::ai_panel::TurnState::Proposed(summary));
        if let Some(proposal) = self.doc.ai_proposal.as_mut() {
            proposal.open = true;
        }
        self.doc.writing.sync(self.doc.manuscript_id, true);
    }
}
