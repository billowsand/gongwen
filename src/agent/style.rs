//! 兼容适配：风格档案已移到 `crate::ai_services::style`。
//!
//! 档案格式（`styles.json`）、提示词与截断顺序都在共享模块里，只有一份实现；
//! 这里只做单向 re-export，第四阶段 F 连同旧编排删掉。

pub(crate) use crate::ai_services::style::*;
