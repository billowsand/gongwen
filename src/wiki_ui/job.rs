//! 维基作业活动。后台记录 Pi 已脱敏的事件，界面线程只读取摘要。

use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui;

use crate::app::GongwenApp;
use crate::pi_agent::PiEvent;
use crate::theme;

#[derive(Clone)]
pub(crate) enum WikiRequest {
    Frame,
    Write {
        node: i64,
        docs: Vec<i64>,
    },
    Update {
        pages: Vec<(String, String)>,
        source: Option<i64>,
    },
}

impl WikiRequest {
    fn title(&self) -> &'static str {
        match self {
            Self::Frame => "归纳分类框架",
            Self::Write { docs, .. } if !docs.is_empty() => "从圈选资料整编页面",
            Self::Write { .. } => "整编维基页面",
            Self::Update { .. } => "更新维基页面",
        }
    }

    fn retry(self, app: &mut GongwenApp) {
        match self {
            Self::Frame => app.start_wiki_frame(),
            Self::Write { node, docs } => {
                app.start_wiki_write(node, docs);
            }
            Self::Update { pages, source } => {
                app.start_wiki_update(pages, source);
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct WikiRun {
    request: WikiRequest,
    scope: String,
    started: Instant,
    log: Arc<Mutex<Vec<Activity>>>,
    finished: Option<(Instant, Outcome)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Delivered,
    Failed,
    Stopped,
}

#[derive(Clone)]
struct Activity {
    id: Option<String>,
    title: String,
    result: Option<String>,
    at: u64,
}

impl WikiRun {
    pub(crate) fn new(request: WikiRequest, scope: String) -> Self {
        Self {
            request,
            scope,
            started: Instant::now(),
            log: Arc::new(Mutex::new(Vec::new())),
            finished: None,
        }
    }

    pub(crate) fn sink(&self) -> impl FnMut(PiEvent) + use<> {
        let log = Arc::clone(&self.log);
        let started = self.started;
        move |event| {
            let (id, title, result) = match event {
                PiEvent::Activity(activity) => {
                    (Some(activity.id), activity.action, activity.outcome)
                }
                PiEvent::Progress(line) | PiEvent::Tool(line) => (None, line, None),
                PiEvent::Strategy(info) => (None, info.line(), None),
                _ => return,
            };
            let Ok(mut log) = log.lock() else { return };
            if let Some(id) = &id
                && let Some(item) = log
                    .iter_mut()
                    .rev()
                    .find(|item| item.id.as_ref() == Some(id))
            {
                item.result = result;
                return;
            }
            if title.trim().is_empty()
                || log
                    .last()
                    .is_some_and(|item| item.id.is_none() && item.title == title)
            {
                return;
            }
            log.push(Activity {
                id,
                title,
                result,
                at: started.elapsed().as_secs(),
            });
        }
    }

    pub(crate) fn finish(&mut self, outcome: Outcome) {
        self.finished = Some((Instant::now(), outcome));
    }

    pub(crate) fn active(&self) -> bool {
        self.finished.is_none()
    }
}

impl super::WikiUiState {
    pub(crate) fn busy(&self) -> bool {
        self.frame_run.as_ref().is_some_and(WikiRun::active)
            || self.page_run.as_ref().is_some_and(WikiRun::active)
    }

    pub(crate) fn has_run(&self) -> bool {
        self.frame_run.is_some() || self.page_run.is_some()
    }

    pub(crate) fn show_job(&mut self) {
        self.rail_hidden = false;
        self.job_tab = true;
    }

    pub(crate) fn rail_job_visible(&self) -> bool {
        !self.rail_hidden && self.job_tab && self.has_run()
    }

    pub(crate) fn toggle_job(&mut self) {
        if self.rail_job_visible() {
            self.rail_hidden = true;
        } else {
            self.show_job();
        }
    }

    fn run_for_rail(&self, prefer_frame: bool) -> Option<&WikiRun> {
        let (first, second) = if prefer_frame {
            (&self.frame_run, &self.page_run)
        } else {
            (&self.page_run, &self.frame_run)
        };
        first
            .as_ref()
            .filter(|run| run.active())
            .or_else(|| second.as_ref().filter(|run| run.active()))
            .or_else(|| {
                [first.as_ref(), second.as_ref()]
                    .into_iter()
                    .flatten()
                    .max_by_key(|run| {
                        (
                            if prefer_frame {
                                matches!(run.request, WikiRequest::Frame)
                            } else {
                                false
                            },
                            run.started,
                        )
                    })
            })
    }
}

/// 与智能写作一样常显当前动作，工具结果随过程条目展开。
pub(crate) fn job_contents(app: &mut GongwenApp, ui: &mut egui::Ui, prefer_frame: bool) {
    let run = app.wiki.run_for_rail(prefer_frame).cloned();
    render_run(app, ui, run);
}

pub(super) fn page_job_contents(app: &mut GongwenApp, ui: &mut egui::Ui) {
    render_run(app, ui, app.wiki.page_run.clone());
}

fn render_run(app: &mut GongwenApp, ui: &mut egui::Ui, run: Option<WikiRun>) {
    let Some(run) = run else {
        ui.weak("尚未发起维基作业");
        return;
    };
    let frame = matches!(run.request, WikiRequest::Frame);
    let stopping = if frame {
        app.wiki.request_stop_frame
    } else {
        app.wiki.request_stop_write
    };
    let error = if frame {
        app.wiki.frame_error.as_deref()
    } else {
        app.wiki.write_error.as_deref()
    };
    show_run(ui, &run, stopping, error);
    match run.finished.map(|(_, outcome)| outcome) {
        None => {
            if ui
                .add_enabled(!stopping, egui::Button::new("停止"))
                .clicked()
            {
                if frame {
                    app.wiki.request_stop_frame = true;
                } else {
                    app.wiki.request_stop_write = true;
                }
            }
        }
        Some(Outcome::Delivered) if frame && app.wiki.frame_proposal.is_some() => {
            if ui.button("查看框架提案").clicked() {
                app.wiki.open_frame_proposal();
                app.open_page(crate::app::NavPage::Wiki);
            }
        }
        Some(Outcome::Delivered) if !frame && app.wiki.review.is_some() => {
            if ui.button("打开待审批次").clicked() {
                app.wiki.open_inbox();
                app.open_page(crate::app::NavPage::Wiki);
            }
        }
        Some(Outcome::Failed | Outcome::Stopped)
            if !app.wiki.busy() && ui.button("重新发起").clicked() =>
        {
            run.request.retry(app);
        }
        _ => {}
    }
}

pub(super) fn show_run(ui: &mut egui::Ui, run: &WikiRun, stopping: bool, error: Option<&str>) {
    let log = run.log.lock().map(|log| log.clone()).unwrap_or_default();
    let elapsed = run
        .finished
        .map_or_else(Instant::now, |(at, _)| at)
        .duration_since(run.started)
        .as_secs();
    let status = match run.finished.map(|(_, outcome)| outcome) {
        Some(Outcome::Delivered) => "已交付",
        Some(Outcome::Stopped) => "已停止",
        Some(Outcome::Failed) => "失败",
        None if stopping => "正在停止",
        None if log.is_empty() => "准备中",
        None => "运行中",
    };
    ui.strong(run.request.title());
    ui.horizontal(|ui| {
        if run.active() {
            ui.spinner();
        }
        ui.label(status);
        ui.weak(format!("{:02}:{:02}", elapsed / 60, elapsed % 60));
    });
    if !run.scope.is_empty() {
        ui.weak(&run.scope);
    }
    if let Some(error) = error {
        ui.colored_label(theme::warn(), error);
    }
    if let Some(item) = log.last() {
        ui.separator();
        crate::ai_panel::phase_label(ui, &item.title);
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt(("wiki_activity", run.started))
        .max_height((ui.available_height() - 40.0).max(32.0))
        .stick_to_bottom(run.active())
        .show(ui, |ui| {
            for (index, item) in log.iter().enumerate() {
                ui.push_id(index, |ui| {
                    let label = format!("{:02}:{:02}  {}", item.at / 60, item.at % 60, item.title);
                    if let Some(result) = item.result.as_deref().filter(|result| !result.is_empty())
                    {
                        egui::CollapsingHeader::new(label).show(ui, |ui| {
                            ui.weak(result);
                        });
                    } else {
                        ui.label(label);
                    }
                });
            }
        });
}

pub(crate) fn source_rail(app: &mut GongwenApp, ui: &mut egui::Ui) {
    if app.wiki.rail_job_visible() {
        super::workspaces::right_panel(ui, "wiki_source_rail", |ui| {
            ui.heading("AI 工作状态");
            ui.separator();
            job_contents(app, ui, false);
        });
    }
}
