//! Pi Durable 智能写作（第一阶段）。
//!
//! 应用只负责收集本轮要求与正文快照，把工作区、模型参数交给随包的 Node host；
//! host 在专属工作区里用文件工具写作，最后通过 `submit_document` 交出**一份固定
//! 版本的 Markdown**。Rust 侧再验证哈希、来源与现有闸门，最后仍走
//! `install_ai_proposal` / `accept_ai_proposal`（红线 1）。
//!
//! 本模块不解析聊天正文，不按文件名拼章节，不注册 shell；未提交、超时、取消、
//! 进程崩溃都按失败处理。协议与工作区细节见 `docs/pi-implementation/phase-1-report.md`。

mod delivery;
mod materials;
mod process;
mod protocol;
mod runtime;
pub(crate) mod skills;
mod tools;
mod workspace;

#[cfg(test)]
pub(crate) use delivery::sha256_hex;
pub(crate) use delivery::{DeliveryCheck, PiSelection, check_delivery, verify_submission};
pub(crate) use materials::{MaterialRole, PiMaterialInput};
pub(crate) use process::{
    PiAnswer, PiBridge, PiEvent, PiJob, PiModel, PiOutcome, PiQuestionBatch, PiTask, run,
};
#[cfg(test)]
pub(crate) use process::{PiDelivery, PiQuestion, PiReport};
pub(crate) use protocol::PiReportItem;
pub(crate) use runtime::PiRuntime;
pub(crate) use skills as pi_skills;
pub(crate) use tools::PiToolContext;
pub(crate) use workspace::{PiWorkspace, base_dir};
