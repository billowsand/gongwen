//! 兼容适配：模型调用接口已移到 `crate::ai_services::backend`。
//!
//! 共享实现只有一份；旧编排、旧测试暂时从这里取，产品调用方直接用
//! `crate::ai_services::backend`。等旧编排删除（第四阶段 F）时一并删掉。

pub(crate) use crate::ai_services::backend::*;
