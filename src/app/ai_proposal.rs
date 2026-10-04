//! AI 修改提案：审阅窗与装入 / 接受 / 放弃。
//!
//! 所有 AI 产物（侧栏的每个技能）都先装成提案，用户在侧栏结果卡或审阅窗里接受才落入正文
//! ——这是正文被 AI 产物改写的唯一入口（红线 1）。原来的「AI 起草工作台」已由侧栏技能取代，
//! 只留下这一部分。

use crate::app::GongwenApp;
use crate::diff::{ContentSnapshot, DiffBlock, ManuscriptDiff, manuscript_diff};
use crate::diff_view::{DiffViewConfig, manuscript_diff_ui};
use crate::draft_page::DraftSession;
use crate::theme;
use eframe::egui;

/// 提案正文里 `text`（缺口所在句，找不到就退一步找它的前半句）落在第几处改动。
fn change_containing(report: &ManuscriptDiff, markdown: &str, text: &str) -> Option<usize> {
    let head: String = text.chars().take(12).collect();
    let pos = markdown
        .find(text)
        .or_else(|| (!head.is_empty()).then(|| markdown.find(&head)).flatten())?;
    report
        .body
        .blocks
        .iter()
        .filter_map(|block| match block {
            DiffBlock::Changed(change) => Some(change),
            DiffBlock::Unchanged(_) => None,
        })
        .position(|change| {
            change
                .new_range
                .as_ref()
                .is_some_and(|range| range.contains(&pos))
        })
}

impl GongwenApp {
    pub(crate) fn ai_proposal_window(&mut self, ctx: &egui::Context) {
        if self.docs.is_empty() || !self.showing_doc() {
            return;
        }
        let index = self.active_doc;
        let Some(mut proposal) = self.docs[index].ai_proposal.take() else {
            theme::reset_window_anim(ctx, egui::Id::new("ai_proposal_anim"));
            return;
        };
        if !proposal.open {
            self.docs[index].ai_proposal = Some(proposal);
            return;
        }
        let mut keep = true;
        let mut accept = false;
        let mut discard = false;
        let old = ContentSnapshot::new(
            self.docs[index].draft.clone(),
            proposal.before.clone(),
            String::new(),
        );
        let new = ContentSnapshot::new(
            self.docs[index].draft.clone(),
            proposal.result.markdown.clone(),
            String::new(),
        );
        let report = manuscript_diff(&old, &new);
        if let Some(text) = proposal.locate.take()
            && let Some(index) = change_containing(&report, &proposal.result.markdown, &text)
        {
            proposal.view.set_focus(index, true);
        }
        let win = egui::Window::new("审阅 AI 修改提案")
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(1020.0)
            .default_height(720.0)
            .min_width(760.0)
            .min_height(560.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(&proposal.label);
                    theme::chip(
                        ui,
                        &format!("{} 处正文变化", report.body.changed_count),
                        theme::accent(),
                        theme::accent_soft(),
                    );
                });
                ui.weak("左侧为当前审校稿，右侧为 AI 提案；接受前不会覆盖正文或自动导出。 ");
                if !proposal.fact_changes.is_empty() {
                    ui.add_space(8.0);
                    theme::card().fill(theme::danger_soft()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            theme::chip(ui, "关键事实变化", theme::danger(), theme::danger_soft());
                            ui.label(format!(
                                "检测到 {} 项单位、人员、日期、数字或文件依据变化。",
                                proposal.fact_changes.len()
                            ));
                        });
                        egui::Grid::new("ai_fact_changes")
                            .striped(true)
                            .num_columns(3)
                            .show(ui, |ui| {
                                ui.strong("类型");
                                ui.strong("变化");
                                ui.strong("内容");
                                ui.end_row();
                                for change in &proposal.fact_changes {
                                    ui.label(change.kind.label());
                                    ui.colored_label(
                                        if change.change == crate::ai_guard::FactChangeKind::Added {
                                            theme::success()
                                        } else {
                                            theme::danger()
                                        },
                                        change.change.label(),
                                    );
                                    ui.label(&change.value);
                                    ui.end_row();
                                }
                            });
                        ui.checkbox(
                            &mut proposal.fact_changes_confirmed,
                            "我已逐项核对，上述事实变化符合本次修改要求",
                        );
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let can_accept =
                        proposal.fact_changes.is_empty() || proposal.fact_changes_confirmed;
                    if theme::primary_icon_button_enabled(
                        ui,
                        can_accept,
                        theme::Icon::SquareCheck,
                        "接受提案",
                    )
                    .on_disabled_hover_text("请先确认关键事实变化")
                    .clicked()
                    {
                        accept = true;
                    }
                    if ui
                        .add(theme::secondary_icon_button(theme::Icon::Trash, "放弃提案"))
                        .clicked()
                    {
                        discard = true;
                    }
                    if ui.button("稍后处理").clicked() {
                        proposal.open = false;
                    }
                });
                ui.separator();
                manuscript_diff_ui(
                    ui,
                    &report,
                    &mut proposal.view,
                    &DiffViewConfig {
                        old_label: "当前审校稿",
                        new_label: "AI 修改提案",
                    },
                );
            });
        if let Some(window) = win {
            theme::window_enter_anim(ctx, egui::Id::new("ai_proposal_anim"), &window.response);
        }
        if !keep {
            proposal.open = false;
        }
        self.docs[index].ai_proposal = Some(proposal);
        if accept {
            Self::accept_ai_proposal(&mut self.docs[index], &mut self.status);
        } else if discard {
            Self::discard_ai_proposal(&mut self.docs[index], &mut self.status);
        }
    }

    /// 把一份 AI 产物装成待审提案：对照 `before` 比关键事实，算好结果卡上的摘要。
    /// 侧栏开着时结果卡就地给出采用 / 对照 / 放弃，不再自动弹审阅窗。
    pub(crate) fn install_ai_proposal(
        doc: &mut DraftSession,
        before: String,
        result: crate::models::GeneratedDraft,
        label: String,
        vocabulary: &[crate::models::VocabularyEntry],
    ) -> crate::ai_panel::ProposalSummary {
        let fact_changes =
            crate::ai_guard::compare_key_facts(&before, &result.markdown, vocabulary);
        let summary = crate::ai_panel::ProposalSummary {
            chars: result.markdown.chars().count(),
            was_empty: before.trim().is_empty(),
            fact_changes: fact_changes.len(),
            warnings: result.warnings.len(),
            truncated: result
                .warnings
                .iter()
                .any(|note| note.message.starts_with(crate::ai_panel::TRUNCATED_NOTE)),
        };
        doc.ai_proposal = Some(crate::draft_page::AiProposal {
            before,
            result,
            label,
            fact_changes,
            fact_changes_confirmed: false,
            view: crate::diff_view::DiffViewState::default(),
            open: !doc.ai_panel.open,
            locate: None,
        });
        summary
    }

    /// 接受 AI 提案：落入正文，并把侧栏里对应的结果卡标为已写入。
    ///
    /// 这是正文被 AI 产物改写的唯一入口（红线 1），审阅窗与侧栏结果卡共用。
    /// 关键事实变化没勾确认、或提案丢了正文里的公文引用时拒绝，提案原样留着。
    pub(crate) fn accept_ai_proposal(doc: &mut DraftSession, status: &mut String) -> bool {
        let Some(proposal) = doc.ai_proposal.take() else {
            return false;
        };
        if !proposal.fact_changes.is_empty() && !proposal.fact_changes_confirmed {
            *status = "请先逐项核对关键事实变化，再接受提案。".into();
            doc.ai_proposal = Some(proposal);
            return false;
        }
        // 提案是对着 `before` 生成的。AI 跑着、提案搁着的时候正文都能继续改，
        // 之后再采用会把这些改动整篇冲掉。自动保存会规整有序列表标点，比较前两边一起规整。
        let normalize = crate::export::normalize_ordered_list_punctuation;
        if normalize(&proposal.before) != normalize(&doc.generated_markdown) {
            *status = "提案生成后正文又改过，直接采用会覆盖这些改动。请让 AI 按当前正文重做，或放弃提案。".into();
            doc.ai_proposal = Some(proposal);
            return false;
        }
        if let Err(error) = crate::document_reference::ensure_preserved(
            &doc.generated_markdown,
            &proposal.result.markdown,
        ) {
            *status = error.to_string();
            doc.ai_proposal = Some(proposal);
            return false;
        }
        let label = proposal.label.clone();
        GongwenApp::take_generated(doc, proposal.result);
        doc.ai_panel.resolve_proposal(true);
        *status = format!("已接受“{label}”修改提案。");
        true
    }

    /// 放弃 AI 提案，正文不变。
    pub(crate) fn discard_ai_proposal(doc: &mut DraftSession, status: &mut String) {
        if let Some(proposal) = doc.ai_proposal.take() {
            doc.ai_panel.resolve_proposal(false);
            *status = format!("已放弃“{}”修改提案，当前审校稿未改变。", proposal.label);
        }
    }
}
