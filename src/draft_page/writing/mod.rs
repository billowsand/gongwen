//! 分阶段写作界面与后台任务桥接；正式正文只通过既有提案入口写入。

mod materials;
mod pages;
#[cfg(test)]
mod tests;
mod validation;

use crate::agent::backend::LmBackend;
use crate::agent::writing::{
    self, Action, Coverage, SourceRole, SourceSnapshot, Stage, WritingTask,
};
use crate::draft_page::DraftPage;
use crate::models::{AppConfig, TemplateKind};
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use materials::{MaterialPicker, SourceViewer};

/// 会丢掉成果或替换人工修改的操作，先弹框确认再做。
enum Confirm {
    RemoveSource(String),
    RemoveSection(String),
    Repolish,
}

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
    /// 每节各自的修改要求，切换章节不串。
    rewrite_drafts: HashMap<String, String>,
    picker: Option<MaterialPicker>,
    source_viewer: Option<SourceViewer>,
    evidence_open: bool,
    import_errors: Vec<String>,
    readiness: [Option<String>; 6],
    /// 已有成果时主题先改在这里，用户点「应用」才写进任务。
    topic_draft: Option<String>,
    confirm: Option<Confirm>,
    readiness_valid: bool,
    delivery_cache: Option<(u64, Vec<String>)>,
    /// 正在看的阶段；运行中进度更新只改任务阶段，不把用户拉回去。
    pub(crate) view_stage: Stage,
    view_manual: bool,
    /// 覆盖率按任务修订号缓存：每帧重算 MB 级材料的切块与哈希会卡帧。
    coverage_cache: Option<CoverageCache>,
    revision: u64,
    #[cfg(test)]
    coverage_computations: usize,
}

struct CoverageCache {
    revision: u64,
    topic: Coverage,
    sections: HashMap<String, Coverage>,
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
            rewrite_drafts: HashMap::new(),
            picker: None,
            source_viewer: None,
            evidence_open: true,
            import_errors: Vec::new(),
            readiness: std::array::from_fn(|_| None),
            readiness_valid: false,
            topic_draft: None,
            confirm: None,
            delivery_cache: None,
            view_stage: Stage::Prepare,
            view_manual: false,
            coverage_cache: None,
            revision: 0,
            #[cfg(test)]
            coverage_computations: 0,
        }
    }

    fn invalidate_caches(&mut self) {
        self.readiness_valid = false;
        self.delivery_cache = None;
        self.coverage_cache = None;
        self.revision = self.revision.wrapping_add(1);
    }

    fn changed(&mut self) {
        self.invalidate_caches();
        self.dirty = true;
        self.last_edit = Some(Instant::now());
        self.task.error = None;
    }

    /// 覆盖率缓存命中时不重切材料、不算哈希；任务变化后自动重算。
    pub(crate) fn coverage(&mut self) -> Coverage {
        self.ensure_coverage_cache();
        self.coverage_cache
            .as_ref()
            .map(|cache| cache.topic)
            .unwrap_or_default()
    }

    pub(crate) fn section_coverage(&mut self, section_id: &str) -> Coverage {
        self.ensure_coverage_cache();
        if let Some(coverage) = self
            .coverage_cache
            .as_ref()
            .and_then(|cache| cache.sections.get(section_id))
        {
            return *coverage;
        }
        let coverage = self.task.section_coverage(section_id);
        #[cfg(test)]
        {
            self.coverage_computations += 1;
        }
        if let Some(cache) = self.coverage_cache.as_mut() {
            cache.sections.insert(section_id.to_string(), coverage);
        }
        coverage
    }

    /// 阶段是否已经完成（用于阶段导航的勾）。
    pub(crate) fn stage_complete(&mut self, stage: Stage) -> bool {
        match stage {
            Stage::Prepare => {
                !self.task.topic.trim().is_empty()
                    && self
                        .task
                        .sources
                        .iter()
                        .any(|source| source.role == SourceRole::Content)
            }
            Stage::Extract => {
                let coverage = self.coverage();
                if coverage.total == 0 || coverage.pending > 0 || coverage.failed > 0 {
                    return false;
                }
                let sources: Vec<String> = self
                    .task
                    .sources
                    .iter()
                    .filter(|source| source.role != SourceRole::Content)
                    .map(|source| source.id.clone())
                    .collect();
                sources.iter().all(|id| {
                    let progress = self.task.source_coverage(id);
                    progress.total == 0 || (progress.pending == 0 && progress.failed == 0)
                })
            }
            Stage::Outline => !self.task.sections.is_empty() && self.task.outline_confirmed,
            Stage::Collect => {
                let ids: Vec<String> = self
                    .task
                    .sections
                    .iter()
                    .map(|section| section.id.clone())
                    .collect();
                !ids.is_empty()
                    && ids.iter().all(|id| {
                        let coverage = self.section_coverage(id);
                        coverage.total > 0 && coverage.pending == 0 && coverage.failed == 0
                    })
            }
            Stage::Write => {
                !self.task.sections.is_empty()
                    && self.task.sections.iter().all(|section| {
                        !section.markdown.trim().is_empty()
                            && matches!(
                                section.status,
                                crate::agent::writing::SectionStatus::Done
                                    | crate::agent::writing::SectionStatus::ManualDone
                                    | crate::agent::writing::SectionStatus::ReviewReady
                            )
                    })
            }
            Stage::Review => !self.task.final_markdown.trim().is_empty(),
        }
    }

    /// 这一阶段有失败块或失效的成果，需要用户回来看一眼。
    pub(crate) fn stage_attention(&mut self, stage: Stage) -> bool {
        match stage {
            Stage::Extract => {
                let coverage = self.coverage();
                coverage.failed > 0 || (coverage.done > 0 && coverage.pending > 0)
            }
            Stage::Collect => {
                let ids: Vec<String> = self
                    .task
                    .sections
                    .iter()
                    .map(|section| section.id.clone())
                    .collect();
                ids.iter().any(|id| {
                    let coverage = self.section_coverage(id);
                    coverage.failed > 0 || (coverage.done > 0 && coverage.pending > 0)
                })
            }
            Stage::Outline => !self.task.sections.is_empty() && !self.task.outline_confirmed,
            Stage::Write => {
                self.task.stale_reason.is_some()
                    && self
                        .task
                        .sections
                        .iter()
                        .any(|section| !section.markdown.trim().is_empty())
            }
            Stage::Prepare | Stage::Review => false,
        }
    }

    /// 长任务的总进度：主题提炼 + 各节备料 + 各节正文。
    pub(crate) fn overall_progress(&mut self) -> (usize, usize) {
        let topic = self.coverage();
        let mut total = topic.total;
        let mut done = topic.done;
        let ids: Vec<String> = self
            .task
            .sections
            .iter()
            .map(|section| section.id.clone())
            .collect();
        for id in &ids {
            let coverage = self.section_coverage(id);
            total += coverage.total;
            done += coverage.done;
        }
        total += ids.len();
        done += self
            .task
            .sections
            .iter()
            .filter(|section| {
                use crate::agent::writing::SectionStatus;
                !section.markdown.trim().is_empty()
                    && matches!(
                        section.status,
                        SectionStatus::Done
                            | SectionStatus::ManualDone
                            | SectionStatus::ReviewReady
                    )
            })
            .count();
        (done, total)
    }

    /// 主操作旁的预计模型调用次数；口径写在悬停说明里。
    pub(crate) fn estimated_calls(&mut self) -> Option<(String, String)> {
        let basis =
            "按当前材料块数、章节数和素材组数粗估，实际会受窗口大小、已完成块与失败重试影响";
        match self.task.stage {
            Stage::Prepare => {
                let coverage = self.coverage();
                if coverage.total == 0 {
                    return None;
                }
                let mut samples = 0usize;
                let sample_ids: Vec<String> = self
                    .task
                    .sources
                    .iter()
                    .filter(|source| source.role != SourceRole::Content)
                    .map(|source| source.id.clone())
                    .collect();
                for id in sample_ids {
                    samples += self.task.source_coverage(&id).total;
                }
                let summarize = if samples > 0 { 2 } else { 0 };
                Some((
                    format!(
                        "开始提炼前：预计约 {} 次模型调用（内容 {} 块 + 样稿 {} 块 + 归纳 {} 次）",
                        coverage.total + samples + summarize,
                        coverage.total,
                        samples,
                        summarize
                    ),
                    basis.to_string(),
                ))
            }
            Stage::Outline => {
                let coverage = self.coverage();
                let sections = self.task.sections.len().max(1);
                Some((
                    format!(
                        "准备各节素材前：预计约 {} 次模型调用（内容 {} 块 × {} 节）",
                        coverage.total * sections,
                        coverage.total,
                        sections
                    ),
                    basis.to_string(),
                ))
            }
            Stage::Collect => {
                let sections: Vec<usize> = self
                    .task
                    .sections
                    .iter()
                    .map(|section| {
                        self.task
                            .current_section_cards(section)
                            .len()
                            .div_ceil(5)
                            .max(1)
                    })
                    .collect();
                let calls: usize = sections.iter().sum();
                Some((
                    format!(
                        "开始分节写作前：预计约 {} 次模型调用（{} 节、按素材组粗估）",
                        calls,
                        sections.len()
                    ),
                    basis.to_string(),
                ))
            }
            Stage::Extract | Stage::Write | Stage::Review => None,
        }
    }

    fn ensure_coverage_cache(&mut self) {
        if self
            .coverage_cache
            .as_ref()
            .is_some_and(|cache| cache.revision == self.revision)
        {
            return;
        }
        let topic = self.task.coverage();
        #[cfg(test)]
        {
            self.coverage_computations += 1;
        }
        self.coverage_cache = Some(CoverageCache {
            revision: self.revision,
            topic,
            sections: HashMap::new(),
        });
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
                        self.view_stage = self.task.stage;
                        self.view_manual = false;
                        self.topic_draft = None;
                        self.invalidate_caches();
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
            self.invalidate_caches();
            match update {
                Update::Progress(task) => {
                    let old_stage = self.task.stage;
                    self.task = *task;
                    if !self.view_manual || self.view_stage == old_stage {
                        self.view_stage = self.task.stage;
                        self.view_manual = false;
                    }
                }
                Update::Finished(task, error) => {
                    let old_stage = self.task.stage;
                    self.task = *task;
                    self.task.error = error;
                    if !self.view_manual || self.view_stage == old_stage {
                        self.view_stage = self.task.stage;
                        self.view_manual = false;
                    }
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

impl DraftPage<'_> {
    fn writing_confirm_modal(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let Some(confirm) = &self.doc.writing.confirm else {
            return;
        };
        let task = &self.doc.writing.task;
        let (title, message, button, icon) = match confirm {
            Confirm::RemoveSource(id) => {
                let Some(source) = task.sources.iter().find(|s| &s.id == id) else {
                    self.doc.writing.confirm = None;
                    return;
                };
                let (cards, chunks) = task.source_impact(id);
                let after = if source.role == SourceRole::Content {
                    "提纲需要重新确认；其他材料已完成的提炼与备料照常保留。"
                } else {
                    "下次提炼时按剩余样稿重新归纳（人工改过的风格与框架保留）。"
                };
                (
                    "移除材料",
                    format!(
                        "移除《{}》后，它的 {cards} 张素材卡与 {chunks} 块处理记录一并删除，不能恢复。{after}",
                        source.title
                    ),
                    "移除",
                    crate::theme::Icon::Trash,
                )
            }
            Confirm::RemoveSection(id) => {
                let Some(section) = task.sections.iter().find(|s| &s.id == id) else {
                    self.doc.writing.confirm = None;
                    return;
                };
                (
                    "删除章节",
                    format!(
                        "删除「{}」后，本节正文移入「历史提纲中的章节」，可以恢复为一节，但恢复后需要重新备料。提纲需要重新确认。",
                        section.title
                    ),
                    "删除本节",
                    crate::theme::Icon::Trash,
                )
            }
            Confirm::Repolish => (
                "重新统稿",
                "重新统稿会用新的统稿结果替换你在统稿页上的修改。当前稿会存入「统稿历史」，可以随时恢复。"
                    .to_string(),
                "重新统稿",
                crate::theme::Icon::Sparkles,
            ),
        };
        let dialog = crate::modal::dialog(
            ctx,
            egui::Id::new(("writing_confirm", self.doc.key)),
            title,
            400.0,
            crate::modal::Dismiss::EscOrBackdrop,
            |ui| {
                ui.label(message);
                ui.add_space(10.0);
                let mut confirmed = false;
                ui.horizontal(|ui| {
                    if ui
                        .add(crate::theme::warning_icon_button(icon, button))
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("取消").clicked() {
                        ui.close();
                    }
                });
                confirmed
            },
        );
        if dialog.inner {
            let task = &mut self.doc.writing.task;
            match self.doc.writing.confirm.take() {
                Some(Confirm::RemoveSource(id)) => {
                    task.remove_source(&id);
                }
                Some(Confirm::RemoveSection(id)) => task.remove_section(&id),
                Some(Confirm::Repolish) => *action = Some(Action::Polish),
                None => {}
            }
            self.doc.writing.changed();
        } else if dialog.dismissed {
            self.doc.writing.confirm = None;
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

fn format_elapsed(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl Drop for WritingPanel {
    fn drop(&mut self) {
        self.pause();
    }
}

impl DraftPage<'_> {
    pub(crate) fn writing_header(&mut self, ui: &mut egui::Ui) {
        let was_open = self.doc.writing.open;
        ui.horizontal(|ui| {
            if ui.button("← 返回正文").clicked() {
                self.doc.writing.open = false;
            }
            ui.strong("分阶段写作");
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
        if was_open && !self.doc.writing.open {
            self.doc.writing.sync(self.doc.manuscript_id, true);
        }
    }

    pub(crate) fn writing_ui(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let editable = !self.doc.read_only() && !self.doc.writing.running;
        // 文种在外部表单里改了：立刻按新文种收窄失效、更新默认写法，不必等用户点运行。
        if !self.doc.writing.running && self.doc.writing.task.kind != self.doc.draft.kind {
            self.doc.writing.task.sync_kind(self.doc.draft.kind);
            self.doc.writing.changed();
        }
        if !self.doc.writing.running
            && !self.doc.writing.view_manual
            && self.doc.writing.view_stage != self.doc.writing.task.stage
        {
            self.doc.writing.view_stage = self.doc.writing.task.stage;
        }
        let stage = self.doc.writing.view_stage;
        let running = self.doc.writing.running;
        ui.horizontal_wrapped(|ui| {
            for next in Stage::ALL {
                let ready = self.doc.writing.ready(next);
                // 运行中允许翻看其他阶段（只读），不被就绪条件挡在门外。
                let available = running || next == Stage::Prepare || ready.is_ok();
                let done = self.doc.writing.stage_complete(next);
                let attention = !done && self.doc.writing.stage_attention(next);
                let mark = if done {
                    " √"
                } else if attention {
                    " ⚠"
                } else {
                    ""
                };
                let response = ui.add_enabled(
                    available,
                    egui::Button::new(format!("{} {}{}", next.index() + 1, next.label(), mark))
                        .selected(stage == next),
                );
                let response = match &ready {
                    Err(reason) if !running => response.on_disabled_hover_text(reason.as_str()),
                    _ => response,
                };
                if response.clicked() {
                    self.doc.writing.view_stage = next;
                    if running {
                        self.doc.writing.view_manual = next != self.doc.writing.task.stage;
                    } else {
                        self.doc.writing.task.stage = next;
                        self.doc.writing.view_manual = false;
                        self.doc.writing.changed();
                    }
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
                if let Some((estimate, basis)) = self.doc.writing.estimated_calls() {
                    ui.weak(estimate).on_hover_text(basis);
                }
            });
            if self.doc.writing.running
                || (self.doc.writing.task.started_at_unix.is_some()
                    && self.doc.writing.task.resume_action.is_some())
            {
                let (done, total) = self.doc.writing.overall_progress();
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::ProgressBar::new(if total == 0 {
                            0.0
                        } else {
                            done as f32 / total as f32
                        })
                        .desired_width(240.0)
                        .text(format!("总进度 {done} / {total}")),
                    );
                    if self.doc.writing.running
                        && let Some(started) = self.doc.writing.task.started_at_unix
                    {
                        ui.weak(format!("已用 {}", format_elapsed(unix_now() - started)));
                    }
                });
            }
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
                if !writing::supports(self.doc.draft.kind) {
                    ui.colored_label(
                        crate::theme::warn(),
                        format!(
                            "{}有固定骨架或如实记录要求，不适合分阶段写作；已有成果可以查看，不能继续运行。",
                            self.doc.draft.kind.label()
                        ),
                    );
                }
                ui.add_enabled_ui(editable, |ui| match stage {
                    Stage::Prepare => self.writing_prepare(ui),
                    Stage::Extract => self.writing_artifacts(ui, &mut action),
                    Stage::Outline => self.writing_outline(ui, &mut action),
                    Stage::Collect => self.writing_collection(ui, &mut action),
                    Stage::Write => self.writing_sections(ui, &mut action),
                    Stage::Review => self.writing_review(ui, &mut action),
                });
                if inline_evidence {
                    ui.collapsing("写作依据与待处理事项", |ui| {
                        self.writing_evidence(ui)
                    });
                }
            });
        self.writing_material_modal(ui.ctx());
        self.writing_confirm_modal(ui.ctx(), &mut action);
        if let Some(action) = action {
            self.start_writing(action, ui.ctx());
        }
    }

    fn start_writing(&mut self, action: Action, ctx: &egui::Context) {
        if self.doc.busy || self.doc.ai_panel.running() || self.doc.read_only() {
            self.doc.writing.task.error = Some("请先完成其他任务，且仅在草稿中运行写作".into());
            return;
        }
        if !writing::supports(self.doc.draft.kind) {
            self.doc.writing.task.error = Some(format!(
                "{}不适合分阶段写作，请改用 AI 侧栏起草",
                self.doc.draft.kind.label()
            ));
            return;
        }
        let Some(id) = self.doc.manuscript_id else {
            self.actions.push(crate::app::DraftAction::SaveToLibrary);
            self.doc.writing.task.error = Some("正在为本篇稿件保存身份，请稍后继续".into());
            return;
        };
        // 入口在界面上每帧也会同步文种；这里再收敛一次，防止启动瞬间外部表单刚改过。
        self.doc.writing.task.sync_kind(self.doc.draft.kind);
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
