//! 与 host 的 JSONL 协议。一行一条 JSON，字段与 `pi-host/src/protocol.mjs` 一一对应。

use serde::{Deserialize, Serialize};

use super::materials::PiMaterialMeta;

/// 与 `pi-host/src/protocol.mjs` 一一对应。协议升版时必须两边同时改：旧 host 会
/// 直接拒绝新 init，恢复时也不会拿旧 host 去开原工作区。
pub(crate) const PROTOCOL_VERSION: u32 = 3;
/// host 版本；运行记录里存一份，恢复时不一致就拒绝在原工作区接着写。
pub(crate) const HOST_VERSION: &str = "0.3.6";
/// 宿主与 Rust 共同对账的领域工具额度：常规上限与分阶段绝对硬上限。
pub(crate) const MAX_REGULAR_TOOL_CALLS: usize = 60;
pub(crate) const MAX_TOTAL_TOOL_CALLS: usize = 84;
const _: () = assert!(MAX_REGULAR_TOOL_CALLS + 24 == MAX_TOTAL_TOOL_CALLS);
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
    /// 统一写作入口的策略倾向与已验证能力。旧 host 会忽略，新 host 缺省按自动。
    pub(crate) strategy: InitStrategy,
    pub(crate) task: InitTask,
    pub(crate) limits: InitLimits,
    /// 恢复模式：重新打开同一个 Durable 库，用同一份 requestId 找回原来的提交。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recovery: Option<InitRecovery>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitRecovery {
    /// 固定为 `resume`；将来若有别的模式，旧 host 会拒绝而不是猜。
    pub(crate) mode: &'static str,
    /// 原输入的 requestId，便于 host 核对。
    pub(crate) request_id: String,
    /// 用户本次点「接着写」的标识；允许新的有界补交，不复用上次已耗尽的请求。
    pub(crate) continuation_id: String,
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
    /// 交付回执与成品快照所在的运行目录（绝对路径，必须在工作区之外）。
    pub(crate) delivery: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitStrategy {
    /// `auto` / `phased`；host 按实际窗口、工作预算与任务规模决定最终策略。
    pub(crate) preference: &'static str,
    pub(crate) capability: InitCapability,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitCapability {
    /// 经验证的模型工作预算（token）；`None` 表示未验证，host 按保守默认规划。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) verified_work_budget: Option<u32>,
    /// 实测更适合短步骤的模型标记；未知能力不按模型名称猜。
    pub(crate) needs_short_steps: bool,
    /// 档案匹配/回退说明，供策略事件与侧栏运行记录回看。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) reason: String,
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
    pub(crate) writing_context: serde_json::Value,
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
    /// host 算出的策略决定：实际窗口与工作预算是两个字段，原因给人看。
    Strategy {
        mode: String,
        #[serde(default)]
        reasons: Vec<String>,
        #[serde(default)]
        window: u64,
        #[serde(default, rename = "workBudget")]
        work_budget: u64,
        #[serde(default, rename = "materialsBudget")]
        materials_budget: u64,
        #[serde(default)]
        verified: bool,
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
        /// 这是从上次已落盘的回执重放的，不是本轮重新生成。
        #[serde(default)]
        recovered: bool,
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
        /// 这是从上次已落盘的回执重放的，不是本轮重新生成。
        #[serde(default)]
        recovered: bool,
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

/// `ask_user` 的原始题目：题面 + 0 个或 2–4 个选项。校验走 `ai_services::questions` 共享闸门。
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
            | Self::Strategy {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strategy_event_keeps_the_two_window_fields_apart() {
        let event: HostEvent = serde_json::from_str(
            r#"{"type":"strategy","mode":"phased","reasons":["材料超过预算"],
                "window":262144,"workBudget":98304,"materialsBudget":61440,"verified":false,
                "requestId":"req-1","runId":"run-1"}"#,
        )
        .unwrap();
        assert!(event.matches("req-1", "run-1"));
        match event {
            HostEvent::Strategy {
                mode,
                reasons,
                window,
                work_budget,
                materials_budget,
                verified,
                ..
            } => {
                assert_eq!(mode, "phased");
                assert_eq!(reasons, vec!["材料超过预算"]);
                assert_eq!(window, 262_144);
                assert_ne!(window, work_budget, "实际窗口与工作预算必须是两个字段");
                assert_eq!(work_budget, 98_304);
                assert_eq!(materials_budget, 61_440);
                assert!(!verified);
            }
            other => panic!("解析成了别的类型：{other:?}"),
        }
    }

    #[test]
    fn strategy_init_is_serialized_with_camel_case_fields_and_no_unverified_budget() {
        let init = InitStrategy {
            preference: "phased",
            capability: InitCapability {
                verified_work_budget: None,
                needs_short_steps: false,
                reason: "无匹配档案".into(),
            },
        };
        let value = serde_json::to_value(&init).unwrap();
        assert_eq!(value["preference"], "phased");
        assert_eq!(value["capability"]["needsShortSteps"], false);
        assert!(value["capability"].get("verifiedWorkBudget").is_none());
    }
}
