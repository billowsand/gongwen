//! 兼容适配：自主步骤的对话与工具调用格式已移到 `crate::ai_services::toolcall`。
//!
//! 接口用例与测试集也用它，实现只有一份；这里只做单向 re-export，第四阶段 F 连同旧编排删掉。

pub(crate) use crate::ai_services::toolcall::*;
