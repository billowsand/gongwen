use crate::task_assistant::extract::{MAX_SOURCE_BYTES, Proposal, parse_response, prompt};
use anyhow::{Result, bail};
use std::sync::mpsc::TryRecvError;

pub(super) struct Extraction {
    source_fingerprint: String,
    source_title: String,
    source_text: String,
    items: Vec<Proposal>,
    tasks: Vec<crate::task_assistant::Task>,
    source_ref: String,
    /// 同标题同正文的材料之前识别过：没调模型，`items` 为空，交给界面让人选查看还是重新识别。
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
    result: Result<crate::task_assistant::extract::Fill, String>,
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
            <= 8 * MAX_SOURCE_BYTES
}

impl super::GongwenApp {
    pub(crate) fn start_task_fill(&mut self, request: crate::task_ui::FillRequest) {
        if self.task_fill_rx.is_some() {
            self.task_ui.fill_error = Some("上一次识别还没结束，请等待后重试。".into());
            return;
        }
        // 单任务填表不给模型任务上下文与分类：它只填六个描述栏，负责人由界面按词库匹配。
        let setup = (|| -> Result<_> {
            if self.task_store.is_none() {
                bail!("任务数据库不可用");
            }
            let mut cfg = self.config.assist_chat()?;
            cfg.temperature = 0.0;
            cfg.max_tokens = cfg.max_tokens.clamp(1024, 4096);
            Ok(cfg)
        })();
        let cfg = match setup {
            Ok(v) => v,
            Err(e) => {
                self.task_ui.fill_error = Some(format!("无法开始填写：{e:#}"));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.task_fill_rx = Some(rx);
        self.task_ui.fill_busy = true;
        self.task_ui.fill_discard = false;
        self.task_ui.fill_error = None;
        std::thread::spawn(move || {
            let result = (|| -> Result<_> {
                if request.text.trim().is_empty() || request.text.len() > MAX_SOURCE_BYTES {
                    bail!("请粘贴非空且不超过 1 MB 的文字，系统不会截断原文");
                }
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                let (system, user) = crate::task_assistant::extract::prompt_fill(
                    &request.text,
                    &today,
                    &request.current,
                );
                let raw = crate::lmstudio::generate(&cfg, &system, &user)?;
                crate::task_assistant::extract::parse_fill(&raw, &request.text, &today)
            })()
            .map_err(|e| format!("识别失败，可重试：{e:#}"));
            let _ = tx.send(TaskFillOutput {
                draft_id: request.draft_id,
                text: request.text,
                result,
            });
        });
    }

    pub(crate) fn poll_task_fill(&mut self, ctx: &eframe::egui::Context) {
        let Some(rx) = self.task_fill_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(output) => {
                self.task_fill_rx = None;
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
                || bytes.saturating_add(material.1.len()) > 8 * MAX_SOURCE_BYTES
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
        let Some(rx) = self.task_extraction_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.task_extraction_rx = None;
                self.task_ui.extraction_busy = false;
                if std::mem::take(&mut self.task_ui.discard_extraction) {
                    crate::task_ui::receive_extraction(
                        &mut self.task_ui,
                        crate::task_ui::ExtractionOutcome::Discarded,
                    );
                } else {
                    let outcome = result.and_then(|data| {
                        use crate::task_ui::ExtractionOutcome;
                        let store = self
                            .task_store
                            .as_mut()
                            .ok_or_else(|| anyhow::anyhow!("任务数据库不可用"))?;
                        if let Some(source) = data.known {
                            // 识别过的材料不落库：选「重新识别」才调模型。
                            return Ok(ExtractionOutcome::Known {
                                source,
                                text: data.source_text,
                            });
                        }
                        if let Some(source_id) = data.rerun {
                            let result =
                                store.rerun_extraction(&source_id, &data.items, &data.tasks)?;
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
                            &data.tasks,
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
            let store = self
                .task_store
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("任务数据库不可用"))?;
            let tasks = store.list(false)?;
            let mut cfg = self.config.assist_chat()?;
            cfg.temperature = 0.0;
            cfg.max_tokens = cfg.max_tokens.clamp(1024, 8192);
            Ok((tasks, cfg, crate::storage::manuscript_db_path()?))
        })();
        let (tasks, cfg, path) = match setup {
            Ok(v) => v,
            Err(e) => {
                self.task_ui.extraction_error =
                    Some(format!("无法开始识别，请检查模型与任务库：{e:#}"));
                return;
            }
        };
        let total = tasks.len();
        let tasks = tasks.into_iter().take(50).collect::<Vec<_>>();
        if total > 50 {
            self.task_ui.notice =
                Some("仅向模型提供最近 50 项任务；较早任务可在待整理中手工匹配。".into());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.task_extraction_rx = Some(rx);
        self.task_ui.extraction_busy = true;
        self.task_ui.discard_extraction = false;
        self.task_ui.extraction_error = None;
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
                    bail!("材料须非空且不超过 1 MB，请分段整理；系统不会截断原文");
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
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    let (system, user) = prompt(&title, &source, &tasks, &today);
                    let raw = crate::lmstudio::generate(&cfg, &system, &user)?;
                    let items = parse_response(&raw, &source, &tasks)?;
                    crate::task_assistant::extract::check_proposal_dates(items, &source, &today)
                };
                Ok(Extraction {
                    source_fingerprint: fingerprint,
                    source_title: title,
                    source_text: source,
                    source_ref,
                    items,
                    tasks,
                    known,
                    rerun,
                })
            })();
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
        let full = vec![(String::new(), "x".repeat(MAX_SOURCE_BYTES), String::new()); 8];
        assert!(!can_add_material(&full, "正文"));
        assert!(can_add_material(&[], "正文"));
    }
}
