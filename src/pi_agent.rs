//! Pi Durable 智能写作：受限文件工作区、领域工具、恢复与随包运行时。
//!
//! 应用只负责收集本轮要求与正文快照，把工作区、模型参数交给随包的 Node host；
//! host 在专属工作区里用文件工具写作，最后通过 `submit_document` 交出**一份固定
//! 版本的 Markdown**。Rust 侧验证技术与权限保护，事实疑点只提示；交付成功由
//! 界面线程直接写入正文并保留撤销记录（2026-11-02 产品修订）。
//!
//! 本模块不解析聊天正文，不按文件名拼章节，不注册 shell；未提交、超时、取消、
//! 进程崩溃都不自动合并正文。协议、恢复与离线部署见 `docs/pi-runtime.md`。

mod delivery;
mod materials;
mod process;
mod protocol;
pub(crate) mod recovery;
mod runtime;
pub(crate) mod skills;
mod strategy;
mod tools;
mod workspace;

pub(crate) use delivery::sha256_hex;
pub(crate) use delivery::{
    DeliveryCheck, DeliveryReport, PiSelection, check_delivery, verify_submission,
};
pub(crate) use materials::{MaterialRole, PiMaterialInput};
pub(crate) use process::{
    PiAnswer, PiBridge, PiDelivery, PiEvent, PiJob, PiModel, PiOutcome, PiQuestionBatch, PiTask,
    run,
};
#[cfg(test)]
pub(crate) use process::{PiQuestion, PiReport};
pub(crate) use protocol::PiReportItem;
pub(crate) use recovery::{
    PiRunLog, PiRunRecord, PiRunRef, PiRunSelection, PiRunStatus, consume_run, read_sources,
    resolve_workspace,
};
pub(crate) use runtime::PiRuntime;
pub(crate) use skills as pi_skills;
pub(crate) use strategy::PiStrategyPreference;
pub(crate) use tools::PiToolContext;
pub(crate) use workspace::{PiWorkspace, base_dir};
