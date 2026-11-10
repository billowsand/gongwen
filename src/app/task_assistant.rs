//! 任务助手的后台作业：材料整理与单任务填写都交给 Pi 智能体。
//!
//! 界面线程只做内存校验、组装请求与状态切换；模型运行时定位、材料快照、host 启动、
//! 交付 JSON 的确定性复核都在后台线程。进度与停止信号通过 [`TaskJobUi`] 与界面共享，
//! 结果仍走原来的 mpsc 通道交回，采纳与否由界面决定——这里不写数据库。
//!
//! 与旧的一次性调用相比，材料不再受单次请求窗口限制：整份原文按 Pi 的材料上限分块，
//! 模型用 `task_search` / `task_read` 核对现有任务，不再只看到最近 50 项。

use crate::task_assistant::extract::{Fill, MAX_SOURCE_BYTES, Proposal};
use anyhow::{Result, bail};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

pub(super) struct Extraction {
    source_fingerprint: String,
    source_title: String,
    source_text: String,
    items: Vec<Proposal>,
    source_ref: String,
    /// 同标题同正文的材料之前识别过：没启动 Pi，`items` 为空，交给界面让人选查看还是重新识别。
    known: Option<crate::task_assistant::SourceRow>,
    /// 重新识别：结果替换这份已存材料的未处理建议，不另存一份。
    rerun: Option<String>,
}

enum Material {
    Text(String, String, String),
    Knowledge(i64),
    /// 按材料库里存的快照重新识别。
    Rerun(String),
}

pub(super) struct TaskFillOutput {
    draft_id: String,
    text: String,
    result: Result<Fill, String>,
}

/// 一轮后台作业的进度与停止信号。界面每帧读一次进度，停止只置标志。
pub(super) struct TaskJobUi {
    progress: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
}

impl TaskJobUi {
    fn start() -> (Self, Arc<Mutex<String>>, Arc<AtomicBool>) {
        let progress = Arc::new(Mutex::new(String::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let ui = Self {
            progress: Arc::clone(&progress),
            cancel: Arc::clone(&cancel),
        };
        (ui, progress, cancel)
    }

    /// 最近一条进度；还没收到任何事件时为空串。
    pub(super) fn line(&self) -> String {
        self.progress
            .lock()
            .map(|line| line.clone())
            .unwrap_or_default()
    }

    /// 请求停止：在途的工具与模型调用在阶段边界收手，迟到结果不采纳。
    pub(super) fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// 进度写入：后台线程只改这一格字符串，界面每帧读。
fn progress_sink(progress: &Arc<Mutex<String>>) -> impl FnMut(String) + 'static {
    let slot = Arc::clone(progress);
    move |line: String| {
        if let Ok(mut current) = slot.lock() {
            *current = line;
        }
    }
}

/// 导入线程先限量，再克隆正文，避免超大批次在交给队列之前耗尽内存。
pub(super) fn can_add_material(items: &[(String, String, String)], text: &str) -> bool {
    text.len() <= MAX_SOURCE_BYTES
        && items.len() < 32
        && items
            .iter()
            .map(|(_, body, _)| body.len())
            .sum::<usize>()
            .saturating_add(text.len())
            <= 2 * MAX_SOURCE_BYTES
}

impl super::GongwenApp {
    pub(crate) fn start_task_fill(&mut self, request: crate::task_ui::FillRequest) {
        if self.task_fill_rx.is_some() {
            self.task_ui.fill_error = Some("上一次识别还没结束，请等待后重试。".into());
            return;
        }
        let setup = (|| -> Result<_> {
            if self.task_store.is_none() {
                bail!("任务数据库不可用");
            }
            Ok((
                Arc::new(self.config.clone()),
                crate::storage::manuscript_db_path()?,
            ))
        })();
        let (config, path) = match setup {
            Ok(value) => value,
            Err(error) => {
                self.task_ui.fill_error = Some(format!("无法开始填写：{error:#}"));
                return;
            }
        };
        let (ui, progress, cancel) = TaskJobUi::start();
        let (tx, rx) = std::sync::mpsc::channel();
        self.task_fill_rx = Some(rx);
        self.task_fill_job = Some(ui);
        self.task_ui.fill_busy = true;
        self.task_ui.fill_discard = false;
        self.task_ui.fill_error = None;
        self.task_ui.fill_progress = None;
        let text = request.text.clone();
        let current = request.current.clone();
        std::thread::spawn(move || {
            let result = crate::pi_agent::task_jobs::fill(
                config,
                path,
                text.clone(),
                current,
                cancel,
                progress_sink(&progress),
            )
            .map(|done| done.fill)
            .map_err(|error| format!("识别失败，可重试：{error}"));
            let _ = tx.send(TaskFillOutput {
                draft_id: request.draft_id,
                text,
                result,
            });
        });
    }

    pub(crate) fn poll_task_fill(&mut self, ctx: &eframe::egui::Context) {
        if let Some(job) = &self.task_fill_job {
            self.task_ui.fill_progress = Some(job.line());
            if std::mem::take(&mut self.task_ui.request_stop_fill) {
                job.stop();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let Some(rx) = self.task_fill_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(output) => {
                self.task_fill_rx = None;
                self.task_fill_job = None;
                self.task_ui.fill_progress = None;
                crate::task_ui::receive_fill(
                    &mut self.task_ui,
                    &output.draft_id,
                    &output.text,
                    output.result,
                    &self.config.vocabulary,
                );
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.task_fill_rx = None;
                self.task_fill_job = None;
                self.task_ui.fill_progress = None;
                if self.task_ui.fill_busy {
                    self.task_ui.fill_busy = false;
                    self.task_ui.fill_discard = false;
                    self.task_ui.fill_error = Some("识别线程意外退出，请重试。".into());
                }
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    pub(crate) fn enqueue_task_materials(&mut self, materials: Vec<(String, String, String)>) {
        let mut bytes: usize = self
            .task_auto_queue
            .iter()
            .map(|(_, text, _)| text.len())
            .sum();
        let mut skipped = 0;
        for material in materials {
            if material.1.len() > MAX_SOURCE_BYTES
                || self.task_auto_queue.len() >= 32
                || bytes.saturating_add(material.1.len()) > 2 * MAX_SOURCE_BYTES
            {
                skipped += 1;
                continue;
            }
            bytes += material.1.len();
            self.task_auto_queue.push_back(material);
        }
        if skipped > 0 {
            self.task_ui.extraction_error = Some(format!(
                "{skipped} 篇材料超过自动队列的大小或数量上限，仍保存在知识库，可分批手工整理。"
            ));
        }
    }

    pub(crate) fn poll_task_extraction(&mut self, ctx: &eframe::egui::Context) {
        if let Some(job) = &self.task_extraction_job {
            self.task_ui.extraction_progress = Some(job.line());
            if std::mem::take(&mut self.task_ui.request_stop_extraction) {
                job.stop();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let Some(rx) = self.task_extraction_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.task_extraction_rx = None;
                self.task_extraction_job = None;
                self.task_ui.extraction_progress = None;
                self.task_ui.extraction_busy = false;
                if std::mem::take(&mut self.task_ui.discard_extraction) {
                    crate::task_ui::receive_extraction(
                        &mut self.task_ui,
                        crate::task_ui::ExtractionOutcome::Discarded,
                    );
                } else {
                    let outcome = result.and_then(|data| {
                        use crate::task_ui::ExtractionOutcome;
                        if let Some(source) = data.known {
                            // 识别过的材料不重复调用模型：选「重新识别」才启动新的作业。
                            return Ok(ExtractionOutcome::Known {
                                source,
                                text: data.source_text,
                            });
                        }
                        let store = self
                            .task_store
                            .as_mut()
                            .ok_or_else(|| anyhow::anyhow!("任务数据库不可用"))?;
                        let tasks = store.list(true)?;
                        if let Some(source_id) = data.rerun {
                            let result = store.rerun_extraction(&source_id, &data.items, &tasks)?;
                            return Ok(ExtractionOutcome::Rerun {
                                source_id,
                                title: data.source_title,
                                result,
                            });
                        }
                        let added = store.commit_extraction(
                            &data.source_title,
                            &data.source_text,
                            &data.source_fingerprint,
                            &data.source_ref,
                            &data.items,
                            &tasks,
                        )?;
                        let source_id = store
                            .source_by_fingerprint(&data.source_fingerprint)?
                            .map(|row| row.id)
                            .unwrap_or_default();
                        Ok(ExtractionOutcome::Saved {
                            source_id,
                            title: data.source_title,
                            text: data.source_text,
                            added,
                        })
                    });
                    match outcome {
                        Ok(outcome) => {
                            crate::task_ui::receive_extraction(&mut self.task_ui, outcome)
                        }
                        Err(e) => {
                            self.task_ui.extraction_error =
                                Some(format!("材料整理失败，可重试：{e:#}"))
                        }
                    }
                }
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.task_extraction_rx = None;
                self.task_extraction_job = None;
                self.task_ui.extraction_progress = None;
                self.task_ui.extraction_busy = false;
                self.task_ui.discard_extraction = false;
                self.task_ui.extraction_error = Some("识别线程意外退出，可重试。".into());
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    pub(crate) fn start_task_extraction(
        &mut self,
        title: String,
        source: String,
        source_ref: String,
    ) {
        self.start_task_material(Material::Text(title, source, source_ref));
    }

    pub(crate) fn start_task_knowledge_extraction(&mut self, id: i64) {
        self.start_task_material(Material::Knowledge(id));
    }

    pub(crate) fn start_task_rerun(&mut self, source_id: String) {
        self.start_task_material(Material::Rerun(source_id));
    }

    fn start_task_material(&mut self, material: Material) {
        if self.task_extraction_rx.is_some() {
            self.task_ui.extraction_error = Some("已有材料正在识别，请等待结束后再整理。".into());
            return;
        }
        let setup = (|| -> Result<_> {
            if self.task_store.is_none() {
                bail!("任务数据库不可用");
            }
            Ok((
                Arc::new(self.config.clone()),
                crate::storage::manuscript_db_path()?,
            ))
        })();
        let (config, path) = match setup {
            Ok(value) => value,
            Err(error) => {
                self.task_ui.extraction_error =
                    Some(format!("无法开始识别，请检查任务库：{error:#}"));
                return;
            }
        };
        let (ui, progress, cancel) = TaskJobUi::start();
        let (tx, rx) = std::sync::mpsc::channel();
        self.task_extraction_rx = Some(rx);
        self.task_extraction_job = Some(ui);
        self.task_ui.extraction_busy = true;
        self.task_ui.discard_extraction = false;
        self.task_ui.extraction_error = None;
        self.task_ui.extraction_progress = None;
        std::thread::spawn(move || {
            let result = (|| -> Result<Extraction> {
                let mut rerun = None;
                let (title, source, source_ref) = match material {
                    Material::Text(title, text, source_ref) => (title, text, source_ref),
                    Material::Rerun(id) => {
                        let (title, text) = crate::task_assistant::Store::open(&path)?
                            .source(&id)
                            .map_err(|_| anyhow::anyhow!("这份材料已不存在"))?;
                        rerun = Some(id);
                        (title, text, String::new())
                    }
                    Material::Knowledge(id) => {
                        let mut store = crate::knowledge::KnowledgeStore::open(&path)?;
                        let (title, text) = store
                            .get_doc_content(id)?
                            .ok_or_else(|| anyhow::anyhow!("所选资料已不存在"))?;
                        (title, text, format!("knowledge:{id}"))
                    }
                };
                if title.trim().is_empty() || title.chars().count() > 200 {
                    bail!("材料标题不能为空且最多 200 字");
                }
                if source.trim().is_empty() || source.len() > MAX_SOURCE_BYTES {
                    bail!(
                        "材料须非空且不超过 {} MiB，请拆分后再整理；系统不会截断原文",
                        MAX_SOURCE_BYTES / (1024 * 1024)
                    );
                }
                use sha2::{Digest, Sha256};
                let fingerprint = Sha256::digest(format!("{title}\0{source}").as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                let known = if rerun.is_some() {
                    None
                } else {
                    crate::task_assistant::Store::open(&path)?
                        .source_by_fingerprint(&fingerprint)?
                };
                let items = if known.is_some() {
                    Vec::new()
                } else {
                    let done = crate::pi_agent::task_jobs::extract(
                        Arc::clone(&config),
                        path.clone(),
                        title.clone(),
                        source.clone(),
                        cancel,
                        progress_sink(&progress),
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                    done.proposals
                };
                Ok(Extraction {
                    source_fingerprint: fingerprint,
                    source_title: title,
                    source_text: source,
                    items,
                    source_ref,
                    known,
                    rerun,
                })
            })()
            .map_err(|error| anyhow::anyhow!("{error:#}"));
            let _ = tx.send(result);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_assistant_import_queue_limits_before_cloning_materials() {
        assert!(!can_add_material(&[], &"x".repeat(MAX_SOURCE_BYTES + 1)));
        let many = vec![(String::new(), String::new(), String::new()); 32];
        assert!(!can_add_material(&many, "正文"));
        let full = vec![(String::new(), "x".repeat(MAX_SOURCE_BYTES), String::new()); 2];
        assert!(!can_add_material(&full, "正文"));
        assert!(can_add_material(&[], "正文"));
    }
}
