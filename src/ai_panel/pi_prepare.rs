//! Pi 后台准备完成后的交接元数据。
//!
//! 后台线程准备完工作区与运行记录后，只把稳定的 [`crate::pi_agent::PiRunRef`] 与
//! 这个确认通道交回界面线程；界面核对轮次、保存会话成功后才允许 host 启动。
//! 带密钥、带整篇正文的启动结构不经过界面。

use std::sync::mpsc::{Receiver, SyncSender};

/// 界面 → 后台的确认通道。容量 1，界面只回一次。
pub(crate) struct PiPrepareAck {
    tx: SyncSender<Result<(), String>>,
}

impl PiPrepareAck {
    pub(crate) fn channel() -> (Self, Receiver<Result<(), String>>) {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        (Self { tx }, rx)
    }

    /// 界面确认（`Ok`）或拒绝（`Err`）。界面已经不收这一轮时安静丢弃。
    pub(crate) fn confirm(&self, result: Result<(), String>) {
        let _ = self.tx.try_send(result);
    }
}

impl std::fmt::Debug for PiPrepareAck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PiPrepareAck")
    }
}
