//! 兼容适配：预算与切块已移到 `crate::ai_services::budget`。
//!
//! 旧编排与分阶段写作共用同一份实现，这里只做单向 re-export；第四阶段 F 删除旧编排后
//! 连同本文件一起去掉。

pub(crate) use crate::ai_services::budget::*;
