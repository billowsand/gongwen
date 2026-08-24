//! 办理进度面板：现在卡在哪，下一步该做什么。
//!
//! 这一页是阶段 4 的全部界面。它刻意**不是向导**——不强制顺序、不禁用任何操作，
//! 只把「哪一步过了、哪一步差什么」摆出来，并给一个直接跳到处理入口的按钮。
//!
//! 真实写公文并不线性：先润色后填要素是常事，等排版出了问题再回头改正文也是
//! 常事。强行排序只会让工具变难用，而用户真正缺的从来不是流程约束，是**一眼
//! 看清还差什么**。

use crate::app::GongwenApp;
use crate::sop::{self, DocSnapshot, Stage, StageState};
use crate::theme;
use eframe::egui;

/// 面板里点出的跳转。
enum SopAction {
    OpenWorkbench,
    OpenElements,
    OpenReviewDrawer,
    RunModelReview,
    Export,
    Save,
    Close,
}

impl GongwenApp {
    /// 从当前会话造判定所需的快照。
    ///
    /// 判定逻辑住在 `sop` 里且只吃这个快照，所以它能脱离整个应用单测——
    /// `DraftSession` 拖着一堆 egui 视图状态，测试里造不出来。
    fn sop_snapshot(&self) -> DocSnapshot {
        let doc = &self.docs[self.active_doc];
        let blocking = crate::validator::blocking_issues(
            &doc.draft,
            &doc.generated_markdown,
            &self.config.vocabulary,
            &self.config.security_rules,
        )
        .len();
        let mut mustfix = 0usize;
        let mut advisory = 0usize;
        let mut model = 0usize;
        for item in doc.revisions.items() {
            if item.state != crate::revision::RevisionState::Pending {
                continue;
            }
            match item.severity {
                crate::proofread::Level::MustFix => mustfix += 1,
                _ => advisory += 1,
            }
            if matches!(item.source, crate::revision::RevisionSource::Model { .. }) {
                model += 1;
            }
        }
        DocSnapshot {
            empty: doc.generated_markdown.trim().is_empty(),
            blocking_issues: blocking,
            mustfix_pending: mustfix,
            advisory_pending: advisory,
            model_pending: model,
            // 句子结论缓存非空即说明这一篇跑过复核。换稿时缓存会清，正合适。
            model_review_ran: !doc.revise_cache.is_empty(),
            model_configured: self.config.revise_model.enabled
                && !self
                    .config
                    .revise_model
                    .resolve(&self.config.lm_studio)
                    .model
                    .trim()
                    .is_empty(),
            open_questions: sop::count_open_questions(&doc.generated_markdown),
            exported: !doc.output_files.is_empty(),
            export_failed: doc.export_error.is_some(),
            saved: doc.manuscript_id.is_some(),
            committed: doc.committed_baseline.is_some(),
            dirty: doc.is_dirty(),
        }
    }

    pub(crate) fn sop_window(&mut self, ctx: &egui::Context) {
        if !self.sop_open || self.docs.is_empty() || !self.showing_doc() {
            return;
        }
        let statuses = sop::evaluate(&self.sop_snapshot());
        let (passed, total) = sop::progress(&statuses);
        let next = sop::next_todo(&statuses).map(|item| item.stage);
        let mut keep = true;
        let mut action = None;

        egui::Window::new("办理进度")
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(620.0)
            .min_width(460.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.heading("办理进度");
                    let (fg, bg) = if passed == total {
                        (theme::success(), theme::success_soft())
                    } else {
                        (theme::accent(), theme::accent_soft())
                    };
                    theme::chip(ui, &format!("{passed}/{total}"), fg, bg);
                });
                match next {
                    Some(stage) => {
                        ui.weak(format!("下一步：{}", stage.label()));
                    }
                    None => {
                        ui.colored_label(theme::success(), "各步均已达标，可以签发。");
                    }
                }
                ui.weak("这里只报状态，不挡操作——先做哪一步由你决定。");
                ui.add_space(8.0);

                for status in &statuses {
                    let is_next = next == Some(status.stage);
                    let frame = if is_next {
                        theme::card().fill(theme::accent_soft())
                    } else {
                        theme::card()
                    };
                    frame.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            let (mark, color) = match status.state {
                                StageState::Passed => ("✓", theme::success()),
                                StageState::Todo => ("•", theme::warn()),
                                StageState::NotApplicable => ("—", theme::text_muted()),
                            };
                            ui.colored_label(color, mark);
                            ui.strong(status.stage.label());
                            if is_next {
                                theme::chip(ui, "下一步", theme::accent(), theme::surface());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if status.state == StageState::Todo
                                        && let Some(picked) = jump_button(ui, status.stage)
                                    {
                                        action = Some(picked);
                                    }
                                },
                            );
                        });
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&status.detail).color(theme::text_soft()),
                            )
                            .wrap_mode(egui::TextWrapMode::Wrap),
                        );
                        ui.weak(status.stage.purpose());
                    });
                    ui.add_space(6.0);
                }
            });
        if !keep {
            action = Some(SopAction::Close);
        }
        if let Some(action) = action {
            self.apply_sop_action(action);
        }
    }

    fn apply_sop_action(&mut self, action: SopAction) {
        match action {
            SopAction::OpenWorkbench => {
                self.sop_open = false;
                self.open_ai_workbench(None);
            }
            SopAction::OpenElements => {
                self.sop_open = false;
                let doc = &mut self.docs[self.active_doc];
                doc.form_collapsed = false;
            }
            SopAction::OpenReviewDrawer => {
                self.sop_open = false;
                self.docs[self.active_doc].result_drawer_open = true;
            }
            SopAction::RunModelReview => {
                self.sop_open = false;
                self.draft_page().start_model_review();
            }
            SopAction::Export => {
                self.sop_open = false;
                self.draft_page().start_export_current();
            }
            SopAction::Save => {
                self.sop_open = false;
                // 走功能区那条同一个入口，不另开一套保存逻辑。
                self.draft_actions
                    .push(crate::app::DraftAction::SaveToLibrary);
            }
            SopAction::Close => self.sop_open = false,
        }
    }
}

/// 未达标的那一步给一个直达按钮。跳到处理入口，而不是把处理搬进这个面板——
/// 各步的操作界面本来就在该在的地方，搬过来会变成两套。
fn jump_button(ui: &mut egui::Ui, stage: Stage) -> Option<SopAction> {
    let (label, action) = match stage {
        Stage::Draft => ("打开 AI 工作台", SopAction::OpenWorkbench),
        Stage::Elements => ("展开要素区", SopAction::OpenElements),
        // 存疑占位也在正文里，跟校对同一个抽屉处理。
        Stage::Proofread | Stage::OpenQuestions => ("打开审校抽屉", SopAction::OpenReviewDrawer),
        Stage::ModelReview => ("跑文字复核", SopAction::RunModelReview),
        Stage::Layout => ("导出并编译", SopAction::Export),
        Stage::Submit => ("存入稿件库", SopAction::Save),
    };
    ui.add(theme::icon_text_button(theme::Icon::ChevronRight, label))
        .clicked()
        .then_some(action)
}
