//! Pi Durable 智能写作：受限文件工作区、领域工具、恢复与随包运行时。
//!
//! 应用只负责收集本轮要求与正文快照，把工作区、模型参数交给随包的 Node host；
//! host 在专属工作区里用文件工具写作，最后通过 `submit_document` 交出**一份固定
//! 版本的 Markdown**。Rust 侧验证技术与权限保护，不执行事实核查；交付成功由
//! 界面线程直接写入正文并保留撤销记录（2026-11-02 产品修订）。
//!
//! 本模块不解析聊天正文，不按文件名拼章节，不注册 shell；未提交、超时、取消、
//! 进程崩溃都不自动合并正文。协议、恢复与离线部署见 `docs/pi-runtime.md`。

mod activity;
pub(crate) mod capability;
mod delivery;
mod materials;
mod process;
mod protocol;
pub(crate) mod recovery;
mod runtime;
pub(crate) mod sealed_tree;
pub(crate) mod skills;
mod strategy;
pub(crate) mod style_learning;
pub(crate) mod task_jobs;
mod tools;
#[cfg(test)]
mod wiki_eval;
pub(crate) mod wiki_jobs;
mod workspace;
mod writing_context;

pub(crate) use activity::PiActivity;
pub(crate) use delivery::{
    DeliveryCheck, DeliveryReport, PiSelection, check_delivery, restore_outside_selection,
    sha256_hex, verify_submission,
};
pub(crate) use materials::{MaterialRole, PiMaterialInput};
pub(crate) use process::{
    PiAnswer, PiBridge, PiDelivery, PiEvent, PiJob, PiModel, PiOutcome, PiQuestionBatch, PiTask,
    PiWritingGuard, begin_writing, run, writing_active,
};
#[cfg(test)]
pub(crate) use process::{PiQuestion, PiReport};
pub(crate) use protocol::PiReportItem;
pub(crate) use recovery::{
    PiRunLog, PiRunRecord, PiRunRef, PiRunSelection, PiRunStatus, consume_run, read_sources,
    reject_run, resolve_workspace, run_delivery_rejected, run_status,
};
pub(crate) use runtime::PiRuntime;
pub(crate) use skills as pi_skills;
pub(crate) use strategy::PiCapabilitySnapshot;
pub(crate) use strategy::PiStrategyPreference;
pub(crate) use tools::PiToolContext;
pub(crate) use workspace::{PiWorkspace, base_dir, prune_workspaces, workspace_usage};
