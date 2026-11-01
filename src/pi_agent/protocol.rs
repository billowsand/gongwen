//! 与 host 的 JSONL 协议。一行一条 JSON，字段与 `pi-host/src/protocol.mjs` 一一对应。

use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitLimits {
    pub(crate) max_file_bytes: u64,
    pub(crate) max_candidate_bytes: u64,
    pub(crate) timeout_seconds: u64,
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
