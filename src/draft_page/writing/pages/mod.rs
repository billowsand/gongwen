//! 六阶段的成果编辑、就绪提示与交付操作。

mod artifacts;
mod collection;
mod evidence;
mod outline;
mod prepare;
mod review;
mod sections;

use super::Confirm;
use super::*;
use crate::agent::writing::{ChunkStatus, MaterialCard, Section};
use crate::app::GongwenApp;
use crate::theme;

impl DraftPage<'_> {
    pub(crate) fn writing_primary_action(
        &mut self,
        ui: &mut egui::Ui,
        editable: bool,
        action: &mut Option<Action>,
    ) {
        let stage = self.doc.writing.view_stage;
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
        } else if stage == Stage::Write && self.final_needs_confirm() {
            self.doc.writing.confirm = Some(Confirm::Repolish);
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

    /// 统稿页人工改过且还有内容时，重新统稿要先确认。
    fn final_needs_confirm(&self) -> bool {
        let task = &self.doc.writing.task;
        task.final_manual && !task.final_markdown.trim().is_empty()
    }

    pub(crate) fn writing_section_tabs(&mut self, ui: &mut egui::Ui) {
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
