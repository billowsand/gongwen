//! 侧栏里的「智能写作（Pi 试用）」入口。
//!
//! 它和普通技能发送是两条独立操作：用当前输入的要求与正文开一轮 Pi Durable
//! 文件工作区写作，结果仍然只能交成提案，由用户接受。本阶段明确不支持选区、
//! `@` 引用与粘贴材料——遇到就直接说明，不静默丢掉。

use super::mention;
use crate::app::{DocJob, WorkerResult};
use crate::draft_page::DraftPage;
use crate::models::{LmStudioConfig, ReasoningEffort};
use crate::pi_agent::{PiEvent, PiJob, PiModel, PiRuntime, PiTask, PiWorkspace, base_dir, run};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// 一轮 Pi 写作的最长运行时间。到期由 Rust 与 host 两边同时止损。
const PI_TIMEOUT: Duration = Duration::from_secs(900);

impl DraftPage<'_> {
    /// 开始一轮 Pi 试写。`editor_has_selection` 是点按钮时编辑器里是否有选区：
    /// 本阶段不接选区，选中着就拒绝，而不是悄悄改全篇。
    pub(crate) fn start_pi_job(&mut self, editor_has_selection: bool) -> Result<(), String> {
        if self.doc.read_only() {
            return Err("这篇稿件已发布或归档，只读。".into());
        }
        if self.doc.busy || self.doc.ai_panel.running() {
            return Err("这篇稿件还有任务在跑，稍等一下。".into());
        }
        if self.doc.ai_panel.composer.selection.is_some() || editor_has_selection {
            return Err(
                "Pi 智能写作本阶段还不支持选区：请先取消编辑器里的选中，再按全文修改。".into(),
            );
        }
        let composer = &self.doc.ai_panel.composer;
        if !composer.refs.is_empty() {
            return Err(
                "Pi 智能写作本阶段还不支持 @ 引用文章：请先去掉引用，或用普通发送。".into(),
            );
        }
        if !composer.materials.is_empty() {
            return Err("Pi 智能写作本阶段还不支持粘贴材料：请先移除材料，或用普通发送。".into());
        }
        let request = mention::plain_request(composer.text.trim());
        if request.trim().is_empty() {
            return Err("请先写下这轮要做什么，再点「智能写作（Pi 试用）」。".into());
        }
        let chat = self
            .config
            .draft_chat()
            .map_err(|error| error.to_string())?;
        let model = pi_model(&chat)?;
        let kind = self.doc.draft.kind;
        let time = crate::prompt::TimeContext::now();
        let before = self.doc.generated_markdown.clone();
        let title = "智能写作（Pi 试用）".to_string();
        let context = vec![
            "Pi 文件工作区".to_string(),
            if before.trim().is_empty() {
                "空白起草".to_string()
            } else {
                "修改现稿".to_string()
            },
        ];
        self.doc.ai_panel.open = true;
        self.doc.ai_proposal = None;
        self.doc.ai_review_baseline = Some(before.clone());
        self.doc.ai_prompt_last_label = title.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.doc.ai_panel.cancel = Some(cancel.clone());
        let turn_id = self
            .doc
            .ai_panel
            .push_turn(title.clone(), request.clone(), context, None);
        if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) {
            turn.provenance = "Pi Durable · 文件工作区".into();
        }
        let (key, seq) = self.begin_job();
        if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) {
            turn.usage_job_seq = Some(seq);
        }
        *self.status = "正在启动 Pi 智能写作…".into();

        let sender = self.sender.clone();
        let system_date = time.today.clone();
        let kind_label = kind.label().to_string();
        let request_for_job = request.clone();
        std::thread::spawn(move || {
            let send = |job: DocJob| {
                let _ = sender.send(WorkerResult::Doc { key, seq, job });
            };
            let runtime = match PiRuntime::locate() {
                Ok(runtime) => runtime,
                Err(error) => {
                    send(DocJob::PiDone(Err(error)));
                    return;
                }
            };
            let base = match base_dir() {
                Ok(base) => base,
                Err(error) => {
                    send(DocJob::PiDone(Err(error)));
                    return;
                }
            };
            let workspace = match PiWorkspace::prepare(&base, key, seq, &before, &request_for_job) {
                Ok(workspace) => workspace,
                Err(error) => {
                    send(DocJob::PiDone(Err(error)));
                    return;
                }
            };
            let workspace_label = workspace.label.clone();
            let job = PiJob {
                runtime,
                workspace,
                model,
                task: PiTask {
                    request: request_for_job,
                    kind: kind_label,
                    system_date,
                },
                before,
                request_id: format!("gongwen-{key}-{seq}"),
                run_id: uuid::Uuid::new_v4().to_string(),
                timeout: PI_TIMEOUT,
            };
            let result = run(job, cancel, |event| match event {
                PiEvent::Progress(message) => send(DocJob::PiProgress(message)),
                PiEvent::Tool(line) => send(DocJob::PiTool(line)),
                PiEvent::Candidate { path, content } => send(DocJob::PiCandidate { path, content }),
            })
            .map_err(|error| {
                if error.contains(&workspace_label) {
                    error
                } else {
                    format!("{error}（工作区：{workspace_label}）")
                }
            });
            send(DocJob::PiDone(result));
        });
        Ok(())
    }
}

/// 应用现有 OpenAI 兼容模型设置映射成 host 的显式参数。
fn pi_model(chat: &LmStudioConfig) -> Result<PiModel, String> {
    let base_url = chat.base_url.trim().trim_end_matches('/').to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(format!(
            "起草模型的地址不是 http/https：{}。请到「AI 管理 → 模型服务」检查。",
            chat.base_url
        ));
    }
    let model_id = chat.model.trim().to_string();
    if model_id.is_empty() {
        return Err("还没有选择智能写作模型：请到「AI 管理 → 模型服务」选择。".into());
    }
    let context_window = if chat.context_window > 0 {
        chat.context_window
    } else {
        32_768
    };
    // 配置冲突时限制输出，不能虚构比服务端实际值更大的上下文窗口。
    let max_tokens = chat
        .max_tokens
        .max(1)
        .min(context_window.saturating_sub(1_024).max(1));
    let (reasoning, thinking_level) = match chat.reasoning_effort {
        ReasoningEffort::Default => (false, None),
        ReasoningEffort::Low => (true, Some("low".to_string())),
        ReasoningEffort::Medium => (true, Some("medium".to_string())),
        ReasoningEffort::High => (true, Some("high".to_string())),
        ReasoningEffort::Max => (true, Some("high".to_string())),
    };
    Ok(PiModel {
        base_url,
        api_key: chat.api_key.clone(),
        model_id,
        context_window,
        max_tokens,
        reasoning,
        thinking_level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat() -> LmStudioConfig {
        LmStudioConfig {
            base_url: "http://127.0.0.1:1234/v1/".into(),
            model: "local".into(),
            context_window: 8192,
            max_tokens: 32_000,
            reasoning_effort: ReasoningEffort::High,
            ..Default::default()
        }
    }

    #[test]
    fn model_limits_are_explicit_and_consistent() {
        let model = pi_model(&chat()).unwrap();
        assert_eq!(model.base_url, "http://127.0.0.1:1234/v1");
        assert_eq!(model.context_window, 8192);
        assert_eq!(model.max_tokens, 7168);
        assert!(model.max_tokens <= model.context_window);
        assert!(model.reasoning);
        assert_eq!(model.thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn non_http_and_empty_models_are_rejected() {
        let mut config = chat();
        config.base_url = "file:///c:/model".into();
        assert!(pi_model(&config).is_err());
        let mut config = chat();
        config.model.clear();
        assert!(pi_model(&config).is_err());
    }
}
