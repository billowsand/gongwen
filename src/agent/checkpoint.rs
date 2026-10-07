//! 检查点：一步成功之后落盘的现场（整块黑板 + 下一步的位置）。
//!
//! 挂起就是 `reason == Ask` 的检查点；崩溃、关窗、停止、出错之后从最近一份接着跑，
//! 与「答完题接着跑」走同一个入口（`docs/agent-kernel-hardening.md` 第六节）。
//! 检查点只存黑板与位置，不含也不写正文；恢复后照旧走定稿、闸门与提案（红线 1、3）。

use super::board::Board;

/// 从哪一步接着跑。`[i]` 表示从顶层第 i 步开始；`[i, k]` 表示顶层第 i 步是
/// `for_each`，它的前 k 项已做完、从第 k 项（0 基）接着跑。空路径表示全新开跑。
///
/// 第一天就按路径设计而不是单个下标：第 5 期的预算与子图都建在它上面，不能简化。
pub(crate) type StepPath = Vec<usize>;

/// 为什么停在这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Reason {
    /// 全新开跑（保留给第 5 期预算起点）。
    #[allow(dead_code)]
    Start,
    /// 一步成功之后。
    Step,
    /// 挂起问用户。
    Ask,
    /// 到预算上限停下（第 5 期）。
    #[allow(dead_code)]
    Limit,
    /// 出错（保留；出错时恢复用最近一份成功之后的检查点）。
    #[allow(dead_code)]
    Error,
}

/// 一次可恢复的现场。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Checkpoint {
    /// 下一个要跑的步骤。
    pub(crate) at: StepPath,
    pub(crate) reason: Reason,
    /// 卡片上给人看的一行，如「已完成 算子 gap_loop」。
    pub(crate) label: String,
    /// 整块黑板（含 AI 工作稿与证据包）。只读区（要素、正文快照、系统提示、日期）
    /// 恢复时用界面当前值重灌（`Board::refresh_from_ui`），存的值不生效（红线 2）。
    pub(crate) board: Board,
    /// 超过尺寸上限时证据包被截过（落盘时标），恢复后较早的证据需重新检索。
    #[serde(default)]
    pub(crate) partial: bool,
}
