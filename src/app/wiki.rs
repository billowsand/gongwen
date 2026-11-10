//! 维基知识库的作业接线：框架归纳的后台线程、进度、停止与结果投递。
//!
//! 与任务作业同一姿势：界面线程只登记状态，后台线程跑 Pi 作业，结果经
//! 有界通道回主线程；提案只是「模型建议」，由用户在知识库页确认后才写框架。

use std::sync::Arc;
use std::sync::mpsc::TryRecvError;

use super::task_assistant::{TaskJobUi, progress_sink};
use crate::app::GongwenApp;
use crate::pi_agent::wiki_jobs::FrameProposal;

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
        if self.wiki_frame_rx.is_some() {
            self.wiki.frame_error = Some("上一次归纳还没结束，请等待后重试。".into());
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
        let (ui, progress, cancel) = TaskJobUi::start();
        let (tx, rx) = std::sync::mpsc::channel();
        self.wiki_frame_rx = Some(rx);
        self.wiki_frame_job = Some(ui);
        self.wiki.frame_error = None;
        self.wiki.frame_progress = Some("正在准备工作区…".to_string());
        std::thread::spawn(move || {
            let result = crate::pi_agent::wiki_jobs::induce_frame(
                config,
                path,
                cancel,
                progress_sink(&progress),
            )
            .map_err(|error| format!("归纳失败，可重试：{error}"));
            let _ = tx.send(WikiFrameOutput { result });
        });
    }

    /// 每帧轮询：推进进度行、响应停止、收取提案或失败原因。
    pub(crate) fn poll_wiki_frame(&mut self, ctx: &eframe::egui::Context) {
        if let Some(job) = &self.wiki_frame_job {
            if self.wiki.request_stop_frame {
                job.stop();
            } else {
                self.wiki.frame_progress = Some(job.line());
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let Some(rx) = self.wiki_frame_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(output) => {
                self.wiki_frame_rx = None;
                self.wiki_frame_job = None;
                self.wiki.frame_progress = None;
                if std::mem::take(&mut self.wiki.request_stop_frame) {
                    // 用户主动停止：不采纳本轮结果，也不把「已停止」当错误吓人。
                    self.wiki.frame_error = None;
                } else {
                    match output.result {
                        Ok(proposal) => {
                            self.wiki.frame_error = None;
                            self.wiki.frame_proposal = Some(proposal);
                        }
                        Err(error) => self.wiki.frame_error = Some(error),
                    }
                }
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.wiki_frame_rx = None;
                self.wiki_frame_job = None;
                let was_running = self.wiki.frame_progress.is_some();
                self.wiki.frame_progress = None;
                if std::mem::take(&mut self.wiki.request_stop_frame) {
                    self.wiki.frame_error = None;
                } else if was_running {
                    self.wiki.frame_error = Some("归纳线程意外退出，请重试。".into());
                }
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => {}
        }
    }
}
