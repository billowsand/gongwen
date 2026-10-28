//! 右侧依据栏与交付前核对。

use super::*;

impl DraftPage<'_> {
    pub(crate) fn writing_evidence(&mut self, ui: &mut egui::Ui) {
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
        if self.doc.writing.view_stage == Stage::Extract {
            ui.separator();
            ui.strong("提炼覆盖");
            let coverage = self.doc.writing.coverage();
            ui.label(format!(
                "内容块 {} / {}（人工跳过 {}，失败 {}）",
                coverage.done, coverage.total, coverage.skipped, coverage.failed
            ));
            if coverage.failed > 0 || coverage.skipped > 0 {
                ui.weak("失败块与人工跳过在中央页面逐块列出，可重试、确认无关或撤销。");
            }
            let samples: Vec<(String, String)> = self
                .doc
                .writing
                .task
                .sources
                .iter()
                .filter(|source| source.role != SourceRole::Content)
                .map(|source| (source.id.clone(), source.title.clone()))
                .collect();
            for (id, title) in samples {
                let progress = self.doc.writing.source_coverage(&id);
                ui.weak(format!(
                    "{}《{}》分析 {}/{} 块",
                    if self
                        .doc
                        .writing
                        .task
                        .sources
                        .iter()
                        .find(|source| source.id == id)
                        .is_some_and(|source| source.role == SourceRole::Style)
                    {
                        "风格样稿"
                    } else {
                        "结构范文"
                    },
                    title,
                    progress.done,
                    progress.total
                ));
            }
        } else if matches!(self.doc.writing.view_stage, Stage::Outline | Stage::Collect) {
            ui.separator();
            ui.strong("各节素材");
            let task = &self.doc.writing.task;
            ui.weak(format!(
                "已提炼素材 {} 条，确认提纲后分给各节",
                task.current_topic_cards().len()
            ));
            for section in &task.sections {
                let state = if task.section_assigned(section) {
                    format!("素材 {} 条", task.current_section_cards(section).len())
                } else {
                    "待分配".to_string()
                };
                ui.label(&section.title);
                ui.weak(format!("约 {} 字 · {state}", section.target_chars));
            }
        } else if self.doc.writing.view_stage == Stage::Write {
            if let Some(section) = self.doc.writing.task.sections.get(self.doc.writing.section) {
                let section = section.clone();
                ui.separator();
                ui.strong("本节职责");
                ui.label(&section.points);
                let cards: Vec<_> = self
                    .doc
                    .writing
                    .task
                    .current_section_cards(&section)
                    .into_iter()
                    .cloned()
                    .collect();
                ui.weak(format!(
                    "本节约 {} 字；素材 {} 条",
                    section.target_chars,
                    cards.len()
                ));
                let stats = self
                    .doc
                    .writing
                    .task
                    .section_verification_stats(&section.id);
                self.writing_verification_summary(ui, stats);
                self.writing_cards(ui, &cards);
            }
        } else if self.doc.writing.view_stage == Stage::Review {
            ui.separator();
            ui.strong("交付前核对");
            let stats = self.doc.writing.task.verification_stats();
            self.writing_verification_summary(ui, stats);
            ui.weak("出处命中、人工核对与依据不足分开计数，不再显示笼统的全部通过。");
            self.writing_verification_issues(ui);
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

    fn writing_verification_summary(
        &self,
        ui: &mut egui::Ui,
        stats: crate::agent::writing::VerificationStats,
    ) {
        ui.label(format!(
            "已核对 {} · 存在冲突 {} · 依据不足 {} · 待核对 {}",
            stats.checked, stats.conflict, stats.insufficient, stats.unverified
        ));
    }

    fn writing_verification_issues(&mut self, ui: &mut egui::Ui) {
        let issues = self.doc.writing.task.verification_issue_cards();
        if issues.is_empty() {
            return;
        }
        ui.weak("软提醒：存在冲突或依据不足的条目不影响合并，但请核对原文与适用性。");
        for (title, card) in issues {
            ui.horizontal_wrapped(|ui| {
                let label = if card.verification == crate::agent::writing::Verification::Conflict {
                    "存在冲突"
                } else {
                    "依据不足"
                };
                ui.colored_label(theme::warn(), format!("{label}：{}", card.point));
                if !title.is_empty()
                    && ui.link(format!("查看《{title}》原文")).clicked()
                    && let Some(source) = self
                        .doc
                        .writing
                        .task
                        .sources
                        .iter()
                        .find(|source| source.id == card.source_id)
                {
                    self.doc.writing.source_viewer = Some(SourceViewer {
                        title: source.title.clone(),
                        text: source.text.clone(),
                    });
                }
            });
        }
    }

    pub(crate) fn writing_cards(&mut self, ui: &mut egui::Ui, cards: &[MaterialCard]) {
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
                ui.weak(format!(
                    "{} · 观点核验：{verification}",
                    if card.located {
                        "依据已在原文逐字定位"
                    } else {
                        "依据为模型概括，未逐字定位"
                    }
                ));
                ui.label(&card.quote);
                let mut set = None;
                ui.horizontal_wrapped(|ui| {
                    for (value, label) in [
                        (crate::agent::writing::Verification::Supported, "已核对"),
                        (crate::agent::writing::Verification::Conflict, "存在冲突"),
                        (
                            crate::agent::writing::Verification::Insufficient,
                            "依据不足",
                        ),
                    ] {
                        let selected = card.verification == value;
                        if ui
                            .add_enabled(
                                !self.doc.writing.running && !self.doc.read_only(),
                                egui::Button::new(label).selected(selected),
                            )
                            .clicked()
                        {
                            // 再点一次已选状态即撤回为待核对。
                            set = Some(if selected {
                                crate::agent::writing::Verification::Unverified
                            } else {
                                value
                            });
                        }
                    }
                });
                ui.weak("出处命中只说明原句存在；可标记三态，再点一次撤回待核对。");
                if let Some(verification) = set {
                    let rows = self.doc.writing.task.topic_cards.iter_mut().chain(
                        self.doc
                            .writing
                            .task
                            .sections
                            .iter_mut()
                            .flat_map(|section| section.cards.iter_mut()),
                    );
                    for row in rows.filter(|row| row.id == card.id) {
                        row.verification = verification;
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
}
