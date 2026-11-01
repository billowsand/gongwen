//! 与 host 的 JSONL 协议。一行一条 JSON，字段与 `pi-host/src/protocol.mjs` 一一对应。

use serde::{Deserialize, Serialize};

use super::materials::PiMaterialMeta;

pub(crate) const PROTOCOL_VERSION: u32 = 1;
/// 一条协议行的上限。host 侧同样按 16 MiB 卡住。
pub(crate) const MAX_HOST_LINE_BYTES: usize = 16 * 1024 * 1024;
/// 整个进程最多处理多少条消息。
pub(crate) const MAX_EVENTS: usize = 50_000;
/// 交付的 Markdown 上限。
pub(crate) const MAX_DELIVERY_BYTES: usize = 8 * 1024 * 1024;
/// 侧栏候选稿上限。超过这个数的候选不往前端送，避免界面被大文件拖住。
pub(crate) const MAX_CANDIDATE_BYTES: usize = 2 * 1024 * 1024;
/// 进度、错误这类短文本的显示上限。
pub(crate) const MAX_MESSAGE_CHARS: usize = 8_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitMessage {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) protocol: u32,
    pub(crate) request_id: String,
    pub(crate) run_id: String,
    pub(crate) workspace: InitWorkspace,
    pub(crate) model: InitModel,
    pub(crate) task: InitTask,
    pub(crate) limits: InitLimits,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitWorkspace {
    pub(crate) root: String,
    pub(crate) state: String,
    pub(crate) current: String,
    pub(crate) request: String,
    pub(crate) skills: String,
    pub(crate) pi_skills: String,
    pub(crate) materials: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitModel {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model_id: String,
    pub(crate) context_window: u32,
    pub(crate) max_tokens: u32,
    pub(crate) reasoning: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) thinking_level: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitTask {
    pub(crate) request: String,
    pub(crate) kind: String,
    pub(crate) system_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) selection: Option<InitSelection>,
    pub(crate) materials: Vec<PiMaterialMeta>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitSelection {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitLimits {
    pub(crate) max_file_bytes: u64,
    pub(crate) max_candidate_bytes: u64,
    pub(crate) timeout_seconds: u64,
    /// 一轮最多几批提问；超过由 host 拒绝。
    pub(crate) max_question_batches: u32,
    /// 一批最多几题。
    pub(crate) max_questions: u32,
}

/// host 发回来的事件。未知类型按协议错误处理，不顺从猜测。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum HostEvent {
    Ready {
        #[serde(default)]
        protocol: Option<u32>,
        #[serde(default, rename = "hostVersion")]
        host_version: Option<String>,
        #[serde(default, rename = "nodeVersion")]
        node_version: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Progress {
        message: String,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Tool {
        name: String,
        #[serde(default)]
        message: Option<String>,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Candidate {
        path: String,
        content: String,
        sha256: String,
        bytes: usize,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    /// 模型调用只读领域工具。Rust 校验工具名、参数与次数后按 `call_id` 回复。
    ToolRequest {
        #[serde(rename = "callId")]
        call_id: String,
        name: String,
        #[serde(default)]
        args: serde_json::Value,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    /// `ask_user` 出的题。答案带着同一个 `call_id` 回去；等待时间不计入执行超时。
    Questions {
        #[serde(rename = "callId")]
        call_id: String,
        questions: Vec<RawQuestion>,
        #[serde(default)]
        batch: u32,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    /// 审核 / 找资料交的结构化清单，只进侧栏，不替换正文。
    Report {
        #[serde(default)]
        summary: String,
        items: Vec<PiReportItem>,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Submitted {
        path: String,
        #[serde(default)]
        summary: String,
        content: String,
        sha256: String,
        bytes: usize,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Completed {
        submitted: bool,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Cancelled {
        #[serde(default)]
        message: String,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Error {
        #[serde(default)]
        code: Option<String>,
        message: String,
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
    Pong {
        #[serde(default, rename = "requestId")]
        request_id: Option<String>,
        #[serde(default, rename = "runId")]
        run_id: Option<String>,
    },
}

/// `ask_user` 的原始题目：题面 + 0–4 个选项。校验复用 `clarify::agent_questions`。
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RawQuestion {
    pub(crate) question: String,
    #[serde(default)]
    pub(crate) options: Vec<String>,
}

/// `finish_report` 的一条结构化清单项。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiReportItem {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) detail: String,
    /// `必改` / `建议` / `提示`；只在侧栏显示，不决定交付。
    #[serde(default)]
    pub(crate) severity: String,
}

impl HostEvent {
    /// 事件带的请求 / 运行标识必须与本次一致；host 发错就直接失败。
    pub(crate) fn ids(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Ready {
                request_id, run_id, ..
            }
            | Self::Progress {
                request_id, run_id, ..
            }
            | Self::Tool {
                request_id, run_id, ..
            }
            | Self::Candidate {
                request_id, run_id, ..
            }
            | Self::ToolRequest {
                request_id, run_id, ..
            }
            | Self::Questions {
                request_id, run_id, ..
            }
            | Self::Report {
                request_id, run_id, ..
            }
            | Self::Submitted {
                request_id, run_id, ..
            }
            | Self::Completed {
                request_id, run_id, ..
            }
            | Self::Cancelled {
                request_id, run_id, ..
            }
            | Self::Error {
                request_id, run_id, ..
            }
            | Self::Pong {
                request_id, run_id, ..
            } => (request_id.as_deref(), run_id.as_deref()),
        }
    }

    pub(crate) fn matches(&self, request_id: &str, run_id: &str) -> bool {
        let (request, run) = self.ids();
        request == Some(request_id) && run == Some(run_id)
    }
}
