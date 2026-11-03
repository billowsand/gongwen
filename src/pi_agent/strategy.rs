//! 统一写作入口的策略倾向。
//!
//! 用户只选「自动 / 倾向分阶段」，不配置输出长度或分块阈值。真正的策略由 host 按
//! 实际窗口、工作预算与任务规模决定；这里只把倾向与已验证的能力元数据传过去。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PiStrategyPreference {
    /// 由 host 按任务规模与预算自动决定。
    #[default]
    Auto,
    /// 用户希望长任务尽量分阶段；短任务仍可常规完成。
    Phased,
}

impl PiStrategyPreference {
    pub(crate) const ALL: [Self; 2] = [Self::Auto, Self::Phased];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Phased => "phased",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::Phased => "倾向分阶段",
        }
    }
}
