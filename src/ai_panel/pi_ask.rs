//! Pi 提问卡：模型用 `ask_user` 问必要问题时，在现有侧栏里显示、收集回答。
//!
//! 回答按问题 ID 与 `callId` 绑定本轮，不经过旧 `engine::apply_answers`，也不写
//! `DraftInput` 或正文。等待期间可以停止；进程被关掉时由结果卡显示中断。

use super::AiPanel;
use crate::pi_agent::{PiAnswer, PiBridge, PiQuestionBatch, PiReportItem};
use crate::theme;
use eframe::egui;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct PiAsk {
    pub(crate) call_id: String,
    pub(crate) questions: Vec<PiAskQuestion>,
    pub(crate) bridge: Arc<PiBridge>,
}

#[derive(Debug)]
pub(crate) struct PiAskQuestion {
    pub(crate) id: usize,
    pub(crate) text: String,
    pub(crate) options: Vec<String>,
    pub(crate) custom: bool,
    pub(crate) skippable: bool,
    pub(crate) choice: Option<usize>,
    pub(crate) free: String,
    pub(crate) skipped: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PiReportView {
    pub(crate) summary: String,
    pub(crate) items: Vec<PiReportItem>,
    pub(crate) workspace_label: String,
}

impl PiAsk {
    pub(crate) fn new(batch: PiQuestionBatch, bridge: Arc<PiBridge>) -> Self {
        Self {
            call_id: batch.call_id,
            questions: batch
                .questions
                .into_iter()
                .map(|question| PiAskQuestion {
                    id: question.id,
                    text: question.text,
                    options: question.options,
                    custom: question.custom,
                    skippable: question.skippable,
                    choice: None,
                    free: String::new(),
                    skipped: false,
                })
                .collect(),
            bridge,
        }
    }

    /// 收集回答。跳过写空值；既没选项也没自由输入且不能跳过的题先拦下。
    fn answers(&self) -> Result<Vec<PiAnswer>, String> {
        let mut answers = Vec::with_capacity(self.questions.len());
        for question in &self.questions {
            let value = if question.skipped {
                String::new()
            } else if !question.free.trim().is_empty() {
                question.free.trim().to_string()
            } else if let Some(index) = question.choice {
                question
                    .options
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("「{}」的选项不对", question.text))?
            } else {
                String::new()
            };
            if value.is_empty() && !question.skippable {
                return Err(format!("「{}」还没有回答", question.text));
            }
            answers.push(PiAnswer {
                id: question.id,
                question: question.text.clone(),
                value,
            });
        }
        Ok(answers)
    }
}

/// 侧栏里的 Pi 审核 / 资料清单。它不是正文提案，没有接受按钮。
pub(crate) fn pi_report_ui(ui: &mut egui::Ui, report: &PiReportView, running: bool) -> bool {
    let mut close = false;
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            theme::chip(ui, "Pi 审核清单", theme::accent(), theme::accent_soft());
            ui.weak(format!(
                "{} 项 · 仅侧栏报告，不替换正文",
                report.items.len()
            ));
            if !running && ui.small_button("收起").clicked() {
                close = true;
            }
        });
        if !report.summary.trim().is_empty() {
            ui.label(&report.summary);
        }
        for item in &report.items {
            ui.horizontal_wrapped(|ui| {
                if !item.severity.trim().is_empty() {
                    theme::chip(
                        ui,
                        &item.severity,
                        theme::text_muted(),
                        theme::accent_soft(),
                    );
                }
                ui.strong(&item.title);
                if !item.detail.trim().is_empty() {
                    ui.label(&item.detail);
                }
            });
        }
        ui.weak(format!("工作区：{}", report.workspace_label));
    });
    close
}

/// 侧栏顶部的提问卡。返回 `true` 表示已提交（卡已被收走）。
pub(crate) fn pi_ask_ui(ui: &mut egui::Ui, panel: &mut AiPanel, running: bool) -> bool {
    let Some(ask) = panel.pi_ask.as_mut() else {
        return false;
    };
    let mut submit = false;
    let mut cancel = false;
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            theme::chip(ui, "Pi 提问", theme::accent(), theme::accent_soft());
            ui.weak("回答只用于本轮 Pi 上下文，不写正文或表单；等待不计执行超时。");
        });
        for (index, question) in ask.questions.iter_mut().enumerate() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(&question.text).strong());
            ui.horizontal_wrapped(|ui| {
                for (option_index, option) in question.options.iter().enumerate() {
                    if ui
                        .selectable_label(question.choice == Some(option_index), option)
                        .clicked()
                    {
                        question.choice =
                            (question.choice != Some(option_index)).then_some(option_index);
                        if question.choice.is_some() {
                            question.free.clear();
                            question.skipped = false;
                        }
                    }
                }
            });
            if question.custom {
                ui.add(
                    egui::TextEdit::singleline(&mut question.free)
                        .hint_text("其他，自己写")
                        .id_salt(("pi_ask_free", index)),
                );
                if !question.free.trim().is_empty() {
                    question.choice = None;
                    question.skipped = false;
                }
            }
            if question.skippable {
                ui.checkbox(&mut question.skipped, "跳过这道（按现有信息处理）");
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if theme::primary_icon_button(ui, theme::Icon::Check, "提交回答").clicked() {
                submit = true;
            }
            if running && ui.button("停止本轮").clicked() {
                cancel = true;
            }
        });
    });
    if cancel {
        panel.pi_cancel = true;
    }
    if submit {
        let answers = match ask.answers() {
            Ok(answers) => answers,
            Err(error) => {
                panel.composer.error = Some(error);
                return false;
            }
        };
        match ask.bridge.answer(&ask.call_id, &answers) {
            Ok(()) => {
                let confirmed: Vec<String> = answers
                    .iter()
                    .filter(|answer| !answer.value.trim().is_empty())
                    .map(|answer| answer.question.trim().to_string())
                    .collect();
                panel.pi_ask = None;
                panel.composer.error = None;
                if let Some(turn) = panel.running_turn_mut() {
                    turn.notes
                        .push(format!("已确认 {} 个回答。", confirmed.len()));
                }
                panel.composer.pi_notes.extend(
                    answers
                        .iter()
                        .filter(|answer| !answer.value.trim().is_empty())
                        .map(|answer| {
                            format!("{}：{}", answer.question.trim(), answer.value.trim())
                        }),
                );
                if let Some(turn) = panel.running_turn_mut() {
                    turn.state = super::TurnState::Streaming;
                }
                return true;
            }
            Err(error) => {
                panel.composer.error = Some(error);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_agent::PiQuestion;

    fn ask() -> PiAsk {
        PiAsk::new(
            PiQuestionBatch {
                call_id: "q1".into(),
                batch: 1,
                questions: vec![
                    PiQuestion {
                        id: 1,
                        text: "想强调哪方面？".into(),
                        options: vec!["责任".into(), "时限".into()],
                        custom: true,
                        skippable: true,
                    },
                    PiQuestion {
                        id: 2,
                        text: "材料里的数字算不算保密？".into(),
                        options: Vec::new(),
                        custom: true,
                        skippable: false,
                    },
                ],
            },
            Arc::new(PiBridge::default()),
        )
    }

    #[test]
    fn answers_bind_ids_and_allow_skip_and_custom_text() {
        let mut ask = ask();
        ask.questions[0].choice = Some(1);
        ask.questions[1].free = "按材料原文".into();
        let answers = ask.answers().unwrap();
        assert_eq!(answers[0].id, 1);
        assert_eq!(answers[0].value, "时限");
        assert_eq!(answers[1].id, 2);
        assert_eq!(answers[1].value, "按材料原文");
        ask.questions[1].free.clear();
        assert!(ask.answers().is_err(), "不能跳过的题必须回答");
        ask.questions[1].free = "x".into();
        ask.questions[0].skipped = true;
        assert_eq!(ask.answers().unwrap()[0].value, "");
    }
}
