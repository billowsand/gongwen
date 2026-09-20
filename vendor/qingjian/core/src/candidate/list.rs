use serde::{Deserialize, Serialize};

use super::Candidate;

/// 排好序的候选列表，平台层按顺序绘制。
///
/// 与 [`Candidate`] 同理整个结构 `#[serde(default)]`：协议直接传它，缺字段不能炸旧 DLL。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CandidateList {
    /// 候选，索引 0 为首选。
    pub items: Vec<Candidate>,
}
