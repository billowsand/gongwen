//! 维基知识库的作业接线：框架归纳的后台线程、进度、停止与结果投递。
//!
//! 与任务作业同一姿势：界面线程只登记状态，后台线程跑 Pi 作业，结果经
//! 有界通道回主线程；提案只是「模型建议」，由用户在知识库页确认后才写框架。

use std::sync::Arc;
use std::sync::mpsc::TryRecvError;

use super::task_assistant::TaskJobUi;
use crate::app::GongwenApp;
use crate::pi_agent::wiki_jobs::FrameProposal;
use crate::wiki_ui::{Outcome, WikiRequest, WikiRun};

/// 框架归纳作业的输出：通过确定性复核的提案，或失败原因。
pub(crate) struct WikiFrameOutput {
    pub(crate) result: Result<FrameProposal, String>,
}

impl GongwenApp {
    /// 发起「从资料归纳框架」：Pi 浏览资料库，归纳两级框架提案。
    pub(crate) fn start_wiki_frame(&mut self) {
        if self.lock_requested {
            return;
        }
        if self.wiki_frame_rx.is_some()
            || self.wiki_write_rx.is_some()
            || self.wiki.frame_proposal.is_some()
        {
            self.wiki.frame_error = Some("请先结束当前维基作业或处理待确认的框架提案。".into());
            return;
        }
        let setup: anyhow::Result<(Arc<crate::models::AppConfig>, std::path::PathBuf)> = (|| {
            Ok((
                Arc::new(self.config.clone()),
                crate::storage::manuscript_db_path()?,
            ))
        })(
        );
        let (config, path) = match setup {
            Ok(value) => value,
            Err(error) => {
                self.wiki.frame_error = Some(format!("无法开始归纳：{error:#}"));
                return;
            }
        };
        let (ui, _, cancel) = TaskJobUi::start();
        let (tx, rx) = std::sync::mpsc::channel();
        self.wiki_frame_rx = Some(rx);
        self.wiki_frame_job = Some(ui);
        self.wiki.frame_error = None;
        let run = WikiRun::new(WikiRequest::Frame, "资料库".into());
        let events = run.sink();
        self.wiki.frame_run = Some(run);
        self.wiki.open_frame_proposal();
        self.wiki.show_job();
        std::thread::spawn(move || {
            let result = crate::pi_agent::wiki_jobs::induce_frame(config, path, cancel, events)
                .map_err(|error| format!("归纳失败，可重试：{error}"));
            let _ = tx.send(WikiFrameOutput { result });
        });
    }

    /// 每帧轮询：推进进度行、响应停止、收取提案或失败原因。
    pub(crate) fn poll_wiki_frame(&mut self, ctx: &eframe::egui::Context) {
        if let Some(job) = &self.wiki_frame_job {
            if self.wiki.request_stop_frame {
                job.stop();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let Some(rx) = self.wiki_frame_rx.as_ref() else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(output) => output.result,
            Err(TryRecvError::Disconnected) => Err("归纳线程意外退出，请重试。".into()),
            Err(TryRecvError::Empty) => return,
        };
        self.wiki_frame_rx = None;
        self.wiki_frame_job = None;
        let outcome = if std::mem::take(&mut self.wiki.request_stop_frame) {
            Outcome::Stopped
        } else {
            match result {
                Ok(value) => {
                    self.wiki.frame_proposal = Some(value);
                    Outcome::Delivered
                }
                Err(error) => {
                    self.wiki.frame_error = Some(error);
                    Outcome::Failed
                }
            }
        };
        if let Some(run) = &mut self.wiki.frame_run {
            run.finish(outcome);
        }
        ctx.request_repaint();
    }
}

/// 「圈选未整编资料反向整编」对话框状态。
pub(crate) struct WikiCircleState {
    /// 圈选的资料（doc_id, 标题）。
    pub(crate) docs: Vec<(i64, String)>,
    /// 目标分类节点（知识库框架里选）。
    pub(crate) category: Option<i64>,
    pub(crate) options: Vec<(i64, String)>,
    /// 发起失败的即时提示。
    pub(crate) error: Option<String>,
}

/// 新资料关联提示：一篇新入库资料 + 维基检索命中的候选页。
#[derive(Debug, Clone)]
pub(crate) struct WikiDocTip {
    pub(crate) doc_title: String,
    pub(crate) doc_id: i64,
    /// (页面 uuid, 标题)，至多 3 条。
    pub(crate) hits: Vec<(String, String)>,
}

impl GongwenApp {
    pub(crate) fn open_wiki_source_scope(&mut self, docs: Vec<(i64, String)>) {
        let options = self
            .wiki_store
            .as_ref()
            .and_then(|store| store.list_tree().ok())
            .unwrap_or_default()
            .into_iter()
            .flat_map(|theme| {
                theme.categories.into_iter().map(move |category| {
                    (
                        category.id,
                        format!("{} / {}", theme.theme.name, category.name),
                    )
                })
            })
            .collect();
        self.wiki_circle = Some(WikiCircleState {
            docs,
            category: None,
            options,
            error: None,
        });
    }

    /// 刷新资料文档的维基状态（「维基」列：已用于维基 N 页 / 未整编）。
    pub(crate) fn refresh_wiki_doc_status(&mut self) {
        self.wiki_doc_status = self
            .wiki_store
            .as_ref()
            .and_then(|store| store.doc_wiki_status().ok())
            .unwrap_or_default();
        // 资料增删会影响「需复核」与「未整编」派生状态，让知识库页下一帧重读缓存。
        self.wiki.source_pages = self
            .wiki_store
            .as_ref()
            .and_then(|store| store.source_pages().ok())
            .unwrap_or_default();
        self.wiki.invalidate();
    }

    /// 只在资料列表刷新时检索。未轮到的资料保留到下一次提示。
    pub(crate) fn detect_new_docs_for_wiki(&mut self) {
        let Some(wiki) = self.wiki_store.as_ref() else {
            return;
        };
        let unfiltered = if self.knowledge_filter_kind.is_some() {
            let Some(store) = self.knowledge_store.as_mut() else {
                return;
            };
            let Ok(docs) = store.list_docs(None) else {
                return;
            };
            Some(docs)
        } else {
            None
        };
        let docs = unfiltered.as_deref().unwrap_or(&self.knowledge_docs);
        let current = docs
            .iter()
            .map(|doc| doc.id)
            .collect::<std::collections::HashSet<_>>();
        if let Some(tip) = &self.wiki_new_doc_tip
            && !current.contains(&tip.doc_id)
        {
            self.wiki_new_doc_tip = None;
        }
        let Some(seen) = self.wiki_seen_doc_ids.as_mut() else {
            self.wiki_seen_doc_ids = Some(current);
            return;
        };
        seen.retain(|id| current.contains(id));
        if self.wiki_new_doc_tip.is_some() {
            return;
        }
        for doc in docs {
            if seen.contains(&doc.id) {
                continue;
            }
            let Ok(hits) = wiki.search(&doc.title, 3) else {
                continue;
            };
            seen.insert(doc.id);
            if !hits.is_empty() {
                self.wiki_new_doc_tip = Some(WikiDocTip {
                    doc_title: doc.title.clone(),
                    doc_id: doc.id,
                    hits: hits.into_iter().map(|hit| (hit.uuid, hit.title)).collect(),
                });
                break;
            }
        }
    }
}

/// 页面整编作业的输出：通过确定性复核的页面提案集，或失败原因。
pub(crate) struct WikiWriteOutput {
    pub(crate) result: Result<crate::pi_agent::wiki_jobs::PageProposalSet, String>,
}

impl GongwenApp {
    /// 发起节点整编，圈选资料可作为优先读物。
    pub(crate) fn start_wiki_write(&mut self, node_id: i64, doc_hints: Vec<i64>) -> bool {
        if self.lock_requested {
            return false;
        }
        if let Err(error) = self.wiki_can_start_page_job() {
            self.wiki.write_error = Some(error.clone());
            if let Some(state) = self.wiki_circle.as_mut() {
                state.error = Some(error);
            }
            return false;
        }
        let path = match crate::storage::manuscript_db_path() {
            Ok(path) => path,
            Err(error) => {
                let error = format!("无法开始整编：{error:#}");
                self.wiki.write_error = Some(error.clone());
                if let Some(state) = self.wiki_circle.as_mut() {
                    state.error = Some(error);
                }
                return false;
            }
        };
        let config = Arc::new(self.config.clone());
        let (ui, _, cancel) = TaskJobUi::start();
        let (tx, rx) = std::sync::mpsc::channel();
        self.wiki_write_rx = Some(rx);
        self.wiki_write_job = Some(ui);
        self.wiki.write_error = None;
        let scope = self
            .wiki_store
            .as_ref()
            .and_then(|store| store.list_tree().ok())
            .and_then(|tree| {
                tree.into_iter().find_map(|theme| {
                    if theme.theme.id == node_id {
                        return Some(theme.theme.name);
                    }
                    theme
                        .categories
                        .into_iter()
                        .find(|node| node.id == node_id)
                        .map(|node| format!("{} / {}", theme.theme.name, node.name))
                })
            })
            .unwrap_or_default();
        let scope = if doc_hints.is_empty() {
            scope
        } else {
            format!("{scope} · 已选 {} 篇", doc_hints.len())
        };
        let run = WikiRun::new(
            WikiRequest::Write {
                node: node_id,
                docs: doc_hints.clone(),
            },
            scope,
        );
        let events = run.sink();
        self.wiki.page_run = Some(run);
        self.wiki.show_job();
        std::thread::spawn(move || {
            let result = crate::pi_agent::wiki_jobs::write_pages(
                config, path, node_id, doc_hints, cancel, events,
            )
            .map_err(|error| format!("整编失败，可重试：{error}"));
            let _ = tx.send(WikiWriteOutput { result });
        });
        true
    }

    fn wiki_can_start_page_job(&self) -> Result<(), String> {
        if self.wiki_write_rx.is_some() || self.wiki_frame_rx.is_some() {
            return Err("已有维基作业在跑，请等待结束后再发起。".into());
        }
        if self.wiki.review.is_some() {
            return Err("请先处理审核台中的待审页面。".into());
        }
        Ok(())
    }

    /// 发起「按新资料更新」：给定目标页（uuid + 标题），Pi 读新资料交回
    /// 整页更新提案；与整编共用进度条、停止与审核台（提案带 uuid，审核台按 diff 展示）。
    pub(crate) fn start_wiki_update(
        &mut self,
        targets: Vec<(String, String)>,
        source: Option<i64>,
    ) -> bool {
        if self.lock_requested {
            return false;
        }
        if targets.is_empty() {
            return false;
        }
        if let Err(error) = self.wiki_can_start_page_job() {
            self.wiki.write_error = Some(error);
            return false;
        }
        let setup: anyhow::Result<(Arc<crate::models::AppConfig>, std::path::PathBuf)> = (|| {
            Ok((
                Arc::new(self.config.clone()),
                crate::storage::manuscript_db_path()?,
            ))
        })(
        );
        let (config, path) = match setup {
            Ok(value) => value,
            Err(error) => {
                self.wiki.write_error = Some(format!("无法开始更新：{error:#}"));
                return false;
            }
        };
        let (ui, _, cancel) = TaskJobUi::start();
        let (tx, rx) = std::sync::mpsc::channel();
        self.wiki_write_rx = Some(rx);
        self.wiki_write_job = Some(ui);
        self.wiki.write_error = None;
        let scope = targets
            .iter()
            .map(|(_, title)| title.as_str())
            .collect::<Vec<_>>()
            .join("、");
        let run = WikiRun::new(
            WikiRequest::Update {
                pages: targets.clone(),
                source,
            },
            scope,
        );
        let events = run.sink();
        self.wiki.page_run = Some(run);
        self.wiki.show_job();
        std::thread::spawn(move || {
            let result = crate::pi_agent::wiki_jobs::update_pages(
                config, path, targets, source, cancel, events,
            )
            .map_err(|error| format!("更新失败，可重试：{error}"));
            let _ = tx.send(WikiWriteOutput { result });
        });
        true
    }

    /// 每帧轮询页面整编作业：进度、停止、提案进审核台或失败原因。
    pub(crate) fn poll_wiki_write(&mut self, ctx: &eframe::egui::Context) {
        if let Some(job) = &self.wiki_write_job {
            if self.wiki.request_stop_write {
                job.stop();
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let Some(rx) = self.wiki_write_rx.as_ref() else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(output) => output.result,
            Err(TryRecvError::Disconnected) => Err("整编线程意外退出，请重试。".into()),
            Err(TryRecvError::Empty) => return,
        };
        self.wiki_write_rx = None;
        self.wiki_write_job = None;
        let outcome = if std::mem::take(&mut self.wiki.request_stop_write) {
            Outcome::Stopped
        } else {
            match result {
                Ok(value) => {
                    self.wiki.review = Some(crate::wiki_ui::ReviewState::new(value));
                    Outcome::Delivered
                }
                Err(error) => {
                    self.wiki.write_error = Some(error);
                    Outcome::Failed
                }
            }
        };
        if let Some(run) = &mut self.wiki.page_run {
            run.finish(outcome);
        }
        ctx.request_repaint();
    }
}
