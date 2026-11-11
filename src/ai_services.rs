//! 共享 AI 服务：分阶段写作、Pi、API 工作台与旧兼容适配器共用的一份实现。
//!
//! 这一层**不依赖**旧编排（`Board`、`Skill`、`engine::Event`、`ToolCtx` 与算子），只放窄接口
//! 与纯函数。旧 `agent` 模块单向 re-export 这里的类型与函数以兼容旧测试与旧流程，等旧流程
//! 删除时一并去掉（`docs/pi-implementation/phase-4b.md`）。
//!
//! 分组：
//! - `backend`：模型调用接口与用量记录（含辅助步骤的通用系统提示）；
//! - `budget`：单次请求的输入预算与长文切块；
//! - `toolcall`：自主步骤的对话与工具调用格式；
//! - `style`：风格档案的统计、学习、排序与渲染；
//! - `knowledge` / `manuscripts`：知识库检索与稿件库读取的窄接口及本机实现；
//! - `vocabulary`：标准词库的匹配、职能建议与所属单位；
//! - `text` / `citations`：截断、结构化结果转文字、句切与引用标记清理；
//! - `references`：`@` 引用的文章 DTO；
//! - `persistence`：配置目录下 JSON 文件的读写。

pub(crate) mod backend;
pub(crate) mod budget;
pub(crate) mod citations;
pub(crate) mod knowledge;
pub(crate) mod manuscripts;
pub(crate) use crate::sealed_json as persistence;
pub(crate) mod questions;
pub(crate) mod references;
pub(crate) mod skill_package;
pub(crate) mod style;
pub(crate) mod text;
pub(crate) mod toolcall;
pub(crate) mod vocabulary;
