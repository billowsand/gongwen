//! 导入页的「接入助手」：识别之后自动接着干——后台线程跑接入循环
//! （`agent::api_import::onboard`），一直试到候选接口调通，缺什么问人，要对照资料改的交模型改。
//!
//! 界面这边：过程记录、问题卡（粘贴 Key、填文字、选一项，可以跳过）、停止、结论。助手运行时
//! 候选接口由它改，界面锁住编辑；停下以后交回给人，人还可以改完再让它接着试。

use super::key_field;
use crate::agent::api::ApiSecrets;
use crate::agent::api_import::Material;
use crate::agent::api_import::onboard::{
    Answer, Answers, Ask, Desk, Event, Item, Question, Report, Run,
};
use crate::agent::backend::{LmBackend, ModelBackend};
use crate::models::AppConfig;
use crate::theme;
use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

/// 后台线程报回来的。
enum Msg {
    Event(Event),
    Ask(Vec<Question>),
    Done(Report, ApiSecrets),
}

/// 一次接入助手的状态。
pub(super) struct Assist {
    rx: Receiver<Msg>,
    answers: Sender<Answers>,
    cancel: Arc<AtomicBool>,
    steps: Vec<String>,
    pending: Option<Pending>,
    report: Option<Report>,
}

/// 放弃导入、离开页面时让后台停下（正在等的模型调用也会停）。
impl Drop for Assist {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 等用户回答的一批问题与填到一半的回答。
struct Pending {
    questions: Vec<Question>,
    /// 每题填的文字（密钥、文字题，或选择题「自己写」的那一项）。
    values: Vec<String>,
    /// 选择题选了第几项；等于选项数表示「自己写」。
    choices: Vec<Option<usize>>,
    skip: Vec<bool>,
}

impl Pending {
    fn new(questions: Vec<Question>) -> Self {
        let n = questions.len();
        Self {
            questions,
            values: vec![String::new(); n],
            choices: vec![None; n],
            skip: vec![false; n],
        }
    }

    fn answered(&self, index: usize) -> bool {
        if self.skip[index] {
            return true;
        }
        match &self.questions[index].ask {
            Ask::Choice { options, .. } => match self.choices[index] {
                Some(choice) if choice < options.len() => true,
                Some(_) => !self.values[index].trim().is_empty(),
                None => false,
            },
            _ => !self.values[index].trim().is_empty(),
        }
    }

    fn answers(&self) -> Answers {
        self.questions
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let answer = if self.skip[index] {
                    Answer::Skip
                } else {
                    match (&question.ask, self.choices[index]) {
                        (Ask::Choice { options, .. }, Some(choice)) if choice < options.len() => {
                            Answer::Choice(choice)
                        }
                        _ if self.values[index].trim().is_empty() => Answer::Skip,
                        _ => Answer::Text(self.values[index].trim().to_string()),
                    }
                };
                (question.id.clone(), answer)
            })
            .collect()
    }
}

/// 后台线程那一侧：报进度、等回答。
struct ChannelDesk {
    tx: Sender<Msg>,
    answers: Receiver<Answers>,
    cancel: Arc<AtomicBool>,
}

impl Desk for ChannelDesk {
    fn emit(&mut self, event: Event) {
        let _ = self.tx.send(Msg::Event(event));
    }

    fn ask(&mut self, questions: &[Question]) -> Option<Answers> {
        self.tx.send(Msg::Ask(questions.to_vec())).ok()?;
        loop {
            match self.answers.recv_timeout(Duration::from_millis(200)) {
                Ok(answers) => return Some(answers),
                Err(RecvTimeoutError::Timeout) if !self.cancelled() => {}
                Err(_) => return None,
            }
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// 界面要处理的变化。
pub(super) enum Update {
    Item(usize, Box<Item>),
    /// 用户填了密钥：界面上的密钥表跟着改。
    Secret(String, String),
    /// 助手停了：它手里的密钥表（含用户填的）。
    Done(ApiSecrets),
}

impl Assist {
    /// 开跑。`items` 按候选接口的顺序。
    pub(super) fn start(
        items: Vec<Item>,
        secrets: ApiSecrets,
        material: Material,
        config: &AppConfig,
        use_model: bool,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (answers_tx, answers_rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = config.clone();
        let stop = cancel.clone();
        std::thread::spawn(move || {
            let backend = use_model.then(|| LmBackend::new(&config, stop.clone()));
            let mut desk = ChannelDesk {
                tx: tx.clone(),
                answers: answers_rx,
                cancel: stop,
            };
            let mut run = Run::new(
                items,
                secrets,
                &material,
                backend.as_ref().map(|b| b as &dyn ModelBackend),
            );
            let report = run.run(&mut desk);
            let _ = tx.send(Msg::Done(report, run.secrets));
        });
        Self {
            rx,
            answers: answers_tx,
            cancel,
            steps: vec![if use_model {
                "接入助手开始：逐个试调，缺什么问你，要对照资料改的交模型改".into()
            } else {
                "接入助手开始（没配起草模型：只能试调、问你补的东西，配置要自己改）".into()
            }],
            pending: None,
            report: None,
        }
    }

    pub(super) fn running(&self) -> bool {
        self.report.is_none()
    }

    /// 在等用户回答。
    #[cfg(test)]
    pub(super) fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// 取回后台的消息，返回界面要处理的变化。
    pub(super) fn poll(&mut self, ctx: &egui::Context) -> Vec<Update> {
        let mut updates = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(Msg::Event(Event::Step(line))) => self.steps.push(line),
                Ok(Msg::Event(Event::Item(index, item))) => updates.push(Update::Item(index, item)),
                Ok(Msg::Ask(questions)) => self.pending = Some(Pending::new(questions)),
                Ok(Msg::Done(report, secrets)) => {
                    self.report = Some(report);
                    self.pending = None;
                    updates.push(Update::Done(secrets));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.report.is_none() {
                        self.report = Some(Report {
                            stopped: true,
                            ..Report::default()
                        });
                        self.steps.push("接入助手意外中断了。".into());
                    }
                    break;
                }
            }
        }
        if self.running() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        updates
    }

    pub(super) fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 过程记录、问题卡、结论。返回界面要处理的变化（用户填的密钥）。
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Update> {
        let mut updates = Vec::new();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("接入助手").strong());
            if self.running() {
                theme::spinner(ui, 14.0, theme::accent());
                if self.pending.is_some() {
                    ui.weak("等你补充…");
                } else {
                    ui.weak("正在试调、修配置…");
                }
                if ui.button("停止").clicked() {
                    self.stop();
                }
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("api_assist_steps")
            .max_height(160.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.steps {
                    theme::caption(ui, &format!("· {line}"));
                }
            });
        if let Some(pending) = &mut self.pending {
            ui.add_space(6.0);
            if let Some(answers) = questions_ui(ui, pending, &mut updates) {
                if self.answers.send(answers).is_err() {
                    self.steps.push("接入助手已经停了。".into());
                }
                self.pending = None;
            }
        }
        if let Some(report) = &self.report {
            ui.add_space(4.0);
            let (icon, color, soft) = if report.open.is_empty() && !report.stopped {
                (theme::Icon::Check, theme::success(), theme::success_soft())
            } else {
                (
                    theme::Icon::TriangleAlert,
                    theme::warn(),
                    theme::warn_soft(),
                )
            };
            let mut text = report.summary();
            for line in &report.open {
                text.push_str(&format!("\n· {line}"));
            }
            theme::notice(ui, icon, color, soft, text);
        }
        updates
    }
}

/// 问题卡；点「继续」时返回回答。
fn questions_ui(
    ui: &mut egui::Ui,
    pending: &mut Pending,
    updates: &mut Vec<Update>,
) -> Option<Answers> {
    let mut submit = None;
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            egui::RichText::new(format!("需要你补的（{} 件）", pending.questions.len())).strong(),
        );
        for index in 0..pending.questions.len() {
            ui.add_space(6.0);
            let question = pending.questions[index].clone();
            ui.label(&question.text);
            if !question.basis.is_empty() {
                theme::caption(ui, &question.basis);
            }
            ui.add_enabled_ui(!pending.skip[index], |ui| match &question.ask {
                Ask::Secret { .. } => {
                    key_field(ui, &mut pending.values[index]);
                }
                Ask::Text { hint } => {
                    ui.add(
                        egui::TextEdit::singleline(&mut pending.values[index])
                            .hint_text(hint.as_str())
                            .desired_width(320.0),
                    );
                }
                Ask::Choice { options, other } => {
                    for (choice, option) in options.iter().enumerate() {
                        ui.radio_value(&mut pending.choices[index], Some(choice), option.as_str());
                    }
                    if let Some(other) = other {
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut pending.choices[index],
                                Some(options.len()),
                                other.as_str(),
                            );
                            if pending.choices[index] == Some(options.len()) {
                                ui.add(
                                    egui::TextEdit::singleline(&mut pending.values[index])
                                        .desired_width(220.0),
                                );
                            }
                        });
                    }
                }
            });
            ui.checkbox(&mut pending.skip[index], "这个先跳过");
        }
        ui.add_space(6.0);
        let ready = (0..pending.questions.len()).all(|i| pending.answered(i));
        ui.horizontal(|ui| {
            if theme::primary_icon_button_enabled(ui, ready, theme::Icon::Check, "继续").clicked()
            {
                let answers = pending.answers();
                for question in &pending.questions {
                    if let (Ask::Secret { name }, Some(Answer::Text(value))) =
                        (&question.ask, answers.get(&question.id))
                    {
                        updates.push(Update::Secret(name.clone(), value.clone()));
                    }
                }
                submit = Some(answers);
            }
            if !ready {
                ui.weak("每件都填好或勾「先跳过」");
            }
        });
    });
    submit
}
