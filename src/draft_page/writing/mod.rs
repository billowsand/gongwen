//! 分阶段写作界面与后台任务桥接；正式正文只通过既有提案入口写入。

mod materials;
mod pages;
#[cfg(test)]
mod tests;
mod validation;

use crate::agent::backend::LmBackend;
use crate::agent::writing::{self, Action, SourceRole, SourceSnapshot, Stage, WritingTask};
use crate::draft_page::DraftPage;
use crate::models::{AppConfig, TemplateKind};
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use materials::{MaterialPicker, SourceViewer};

enum Update {
    Progress(Box<WritingTask>),
    Finished(Box<WritingTask>, Option<String>),
    Imported(Vec<SourceSnapshot>, Vec<String>),
}

/// 每篇稿件独立持有，后台执行时编辑缓冲冻结，避免旧结果覆盖人工输入。
pub(crate) struct WritingPanel {
    pub(crate) open: bool,
    pub(crate) task: WritingTask,
    pub(crate) running: bool,
    pub(crate) proposal_turn: Option<u64>,
    loaded_for: Option<i64>,
    dirty: bool,
    last_edit: Option<Instant>,
    pub(crate) save_error: Option<String>,
    load_failed: bool,
    receiver: Option<Receiver<Update>>,
    cancel: Arc<AtomicBool>,
    pub(crate) section: usize,
    artifact_tab: usize,
    revise: String,
    rewrite: String,
    picker: Option<MaterialPicker>,
    source_viewer: Option<SourceViewer>,
    evidence_open: bool,
    import_errors: Vec<String>,
    readiness: [Option<String>; 6],
    readiness_valid: bool,
    delivery_cache: Option<(u64, Vec<String>)>,
}

impl WritingPanel {
    pub(crate) fn new(kind: TemplateKind) -> Self {
        Self {
            open: false,
            task: WritingTask::new(kind),
            running: false,
            proposal_turn: None,
            loaded_for: None,
            dirty: false,
            last_edit: None,
            save_error: None,
            load_failed: false,
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            section: 0,
            artifact_tab: 0,
            revise: String::new(),
            rewrite: String::new(),
            picker: None,
            source_viewer: None,
            evidence_open: true,
            import_errors: Vec::new(),
            readiness: std::array::from_fn(|_| None),
            readiness_valid: false,
            delivery_cache: None,
        }
    }

    fn changed(&mut self) {
        self.readiness_valid = false;
        self.delivery_cache = None;
        self.dirty = true;
        self.last_edit = Some(Instant::now());
        self.task.error = None;
    }

    fn ready(&mut self, stage: Stage) -> Result<(), String> {
        if !self.readiness_valid {
            self.readiness = std::array::from_fn(|index| {
                self.task
                    .ready_for(Stage::ALL[index])
                    .err()
                    .map(|error| error.to_string())
            });
            self.readiness_valid = true;
        }
        self.readiness[stage.index()].clone().map_or(Ok(()), Err)
    }

    pub(crate) fn pause(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub(crate) fn waiting_to_save(&self) -> bool {
        self.dirty
    }

    /// 不依赖当前标签是否可见，应用层每帧收取结果并保存编辑。
    pub(crate) fn sync(&mut self, id: Option<i64>, force: bool) {
        if let Some(id) = id
            && self.loaded_for != Some(id)
        {
            let adopt = self.loaded_for.is_none() && self.dirty;
            self.loaded_for = Some(id);
            if !adopt {
                match writing::store::load(id) {
                    Ok(Some(task)) => {
                        self.task = task;
                        self.readiness_valid = false;
                        self.delivery_cache = None;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.load_failed = true;
                        self.save_error = Some(format!("读取写作任务失败：{error:#}"));
                    }
                }
            }
        }
        let mut updates = Vec::new();
        let mut disconnected = false;
        if let Some(receiver) = &self.receiver {
            loop {
                match receiver.try_recv() {
                    Ok(update) => updates.push(update),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for update in updates {
            self.readiness_valid = false;
            self.delivery_cache = None;
            match update {
                Update::Progress(task) => self.task = *task,
                Update::Finished(task, error) => {
                    self.task = *task;
                    self.task.error = error;
                    self.running = false;
                    self.receiver = None;
                    self.dirty = true;
                    self.last_edit = None;
                }
                Update::Imported(sources, errors) => {
                    for source in sources {
                        self.task.add_source(source);
                    }
                    self.import_errors = errors;
                    self.running = false;
                    self.receiver = None;
                    self.changed();
                    self.last_edit = None;
                }
            }
        }
        if disconnected && self.running {
            self.running = false;
            self.receiver = None;
            self.task.error =
                Some("后台任务提前结束；已完成的处理单元保留，可以接着跑或重试导入".into());
        }
        let Some(id) = id else {
            if force && self.dirty {
                self.save_error = Some("写作任务尚未关联稿件，请先保存本篇稿件后再离开".into());
            }
            return;
        };
        if self.load_failed || self.running || !self.dirty {
            return;
        }
        if !force
            && self
                .last_edit
                .is_some_and(|time| time.elapsed() < Duration::from_millis(600))
        {
            return;
        }
        match writing::store::save(id, &self.task) {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
            }
            Err(error) => {
                self.save_error = Some(format!("保存写作任务失败：{error:#}"));
                self.last_edit = Some(Instant::now());
            }
        }
    }

    pub(crate) fn import_files(
        &mut self,
        paths: Vec<PathBuf>,
        role: SourceRole,
        ctx: egui::Context,
    ) {
        if self.running {
            return;
        }
        self.running = true;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.task.status = "正在导入材料…".into();
        std::thread::spawn(move || {
            let mut sources = Vec::new();
            let mut errors = Vec::new();
            for path in paths {
                if cancel.load(Ordering::Relaxed) {
                    errors.push("导入已停止，未完成的文件需要重新导入".into());
                    break;
                }
                let title = crate::doc_import::file_label(&path);
                let result = crate::doc_import::to_knowledge_markdown(&path)
                    .and_then(|text| SourceSnapshot::new(title.clone(), role, text));
                match result {
                    Ok(source) => sources.push(source),
                    Err(error) => errors.push(format!("《{title}》：{error:#}")),
                }
            }
            let _ = sender.send(Update::Imported(sources, errors));
            ctx.request_repaint();
        });
    }

    fn start(
        &mut self,
        id: i64,
        config: AppConfig,
        action: Action,
        ctx: egui::Context,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(!self.running && !self.load_failed, "当前任务尚不可运行");
        writing::store::save(id, &self.task)?;
        self.dirty = false;
        self.save_error = None;
        self.picker = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let mut task = self.task.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.running = true;
        self.task.error = None;
        std::thread::spawn(move || {
            let model = LmBackend::new(&config, cancel.clone());
            let result = writing::run(&mut task, action, &model, &mut |task| {
                writing::store::save(id, task)?;
                sender
                    .send(Update::Progress(Box::new(task.clone())))
                    .map_err(|_| anyhow::anyhow!("写作界面已关闭，进度已保存"))?;
                ctx.request_repaint();
                Ok(())
            });
            let mut error = result.err().map(|error| format!("{error:#}"));
            if cancel.load(Ordering::Relaxed) {
                task.status = "已暂停，可从已保存的位置继续".into();
            }
            task.error.clone_from(&error);
            if let Err(save) = writing::store::save(id, &task) {
                error = Some(format!("{}；保存失败：{save:#}", error.unwrap_or_default()));
            }
            let _ = sender.send(Update::Finished(Box::new(task), error));
            ctx.request_repaint();
        });
        Ok(())
    }
}

impl Drop for WritingPanel {
    fn drop(&mut self) {
        self.pause();
    }
}

impl DraftPage<'_> {
    pub(crate) fn writing_mode_switch(&mut self, ui: &mut egui::Ui) {
        let was_open = self.doc.writing.open;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.doc.writing.open, false, "正文");
            ui.selectable_value(&mut self.doc.writing.open, true, "分阶段写作");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.doc.writing.open {
                    if ui.button("公文要素").clicked() {
                        self.doc.form_collapsed = !self.doc.form_collapsed;
                    }
                    ui.checkbox(&mut self.doc.writing.evidence_open, "写作依据");
                    if let Some(error) = &self.doc.writing.save_error {
                        ui.colored_label(crate::theme::warn(), error);
                    } else if self.doc.writing.running {
                        ui.weak("任务进行中 · 每个处理单元保存进度");
                    } else if self.doc.writing.dirty {
                        ui.weak("保存中…");
                    } else {
                        ui.weak("已保存");
                    }
                }
            });
        });
        if self.doc.writing.open && !was_open {
            self.doc.form_collapsed = true;
            self.doc.ai_panel.open = false;
            self.doc.result_drawer_open = false;
            if self.doc.manuscript_id.is_none() {
                self.actions.push(crate::app::DraftAction::SaveToLibrary);
            }
        }
        if was_open && !self.doc.writing.open {
            self.doc.writing.sync(self.doc.manuscript_id, true);
        }
    }

    pub(crate) fn writing_ui(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let editable = !self.doc.read_only() && !self.doc.writing.running;
        let stage = self.doc.writing.task.stage;
        ui.horizontal_wrapped(|ui| {
            for next in Stage::ALL {
                let available = next == Stage::Prepare || self.doc.writing.ready(next).is_ok();
                if ui
                    .add_enabled(
                        available,
                        egui::Button::new(format!("{} {}", next.index() + 1, next.label()))
                            .selected(stage == next),
                    )
                    .clicked()
                {
                    self.doc.writing.task.stage = next;
                    self.doc.writing.changed();
                }
            }
        });
        ui.separator();
        if let Some(reason) = &self.doc.writing.task.stale_reason {
            ui.colored_label(crate::theme::warn(), reason);
        }
        if let Some(error) = &self.doc.writing.task.error {
            ui.colored_label(crate::theme::warn(), error);
        }
        if self.doc.writing.load_failed {
            if ui.button("重试读取写作任务").clicked() {
                self.doc.writing.loaded_for = None;
                self.doc.writing.load_failed = false;
            }
            return;
        }
        egui::Panel::bottom(egui::Id::new(("writing_footer", self.doc.key))).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(&self.doc.writing.task.status);
                if self.doc.writing.running {
                    ui.spinner();
                    if ui.button("暂停").clicked() {
                        self.doc.writing.pause();
                    }
                } else {
                    if let Some(resume) = &self.doc.writing.task.resume_action
                        && ui
                            .add_enabled(editable, egui::Button::new("接着跑"))
                            .clicked()
                    {
                        action = Some(resume.clone());
                    }
                    self.writing_primary_action(ui, editable, &mut action);
                }
            });
        });
        let inline_evidence = self.doc.writing.evidence_open && ui.available_width() <= 850.0;
        if self.doc.writing.evidence_open && !inline_evidence {
            egui::Panel::right(egui::Id::new(("writing_evidence", self.doc.key)))
                .default_size(280.0)
                .size_range(240.0..=360.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("writing_evidence_scroll")
                        .show(ui, |ui| self.writing_evidence(ui));
                });
        }
        egui::ScrollArea::vertical()
            .id_salt(("writing_stage", self.doc.key, stage.index()))
            .show(ui, |ui| {
                ui.add_enabled_ui(editable, |ui| match stage {
                    Stage::Prepare => self.writing_prepare(ui),
                    Stage::Extract => self.writing_artifacts(ui),
                    Stage::Outline => self.writing_outline(ui, &mut action),
                    Stage::Collect => self.writing_collection(ui),
                    Stage::Write => self.writing_sections(ui, &mut action),
                    Stage::Review => self.writing_review(ui),
                });
                if inline_evidence {
                    ui.collapsing("写作依据与待处理事项", |ui| {
                        self.writing_evidence(ui)
                    });
                }
            });
        self.writing_material_modal(ui.ctx());
        if let Some(action) = action {
            self.start_writing(action, ui.ctx());
        }
    }

    fn start_writing(&mut self, action: Action, ctx: &egui::Context) {
        if self.doc.busy || self.doc.ai_panel.running() || self.doc.read_only() {
            self.doc.writing.task.error = Some("请先完成其他任务，且仅在草稿中运行写作".into());
            return;
        }
        let Some(id) = self.doc.manuscript_id else {
            self.actions.push(crate::app::DraftAction::SaveToLibrary);
            self.doc.writing.task.error = Some("正在为本篇稿件保存身份，请稍后继续".into());
            return;
        };
        if self.doc.writing.task.kind != self.doc.draft.kind {
            self.doc.writing.task.kind = self.doc.draft.kind;
            self.doc.writing.task.invalidate_from(Stage::Outline);
        }
        match self
            .doc
            .writing
            .start(id, self.config.clone(), action, ctx.clone())
        {
            Ok(()) => self.doc.busy = true,
            Err(error) => self.doc.writing.task.error = Some(format!("{error:#}")),
        }
    }
}
