//! 智能体工作台的后台：工具 → 算子 → 技能三层，加上流程引擎与模型接口。
//!
//! 设计见 `docs/ai-agent-workbench.md` 第十六节。分层：
//! - `tools`：原子工具（读稿、工作稿读写、知识库与稿件库、词库、检查、计算、模型与选择题），
//!   每个工具有权限级别，技能只能用 `tools:` 白名单里的；
//! - `ops`：算子，内置的组合招式（澄清、预研、检索、生成、缺口循环、核验、出题）；
//! - `skill`：SKILL.md 技能文件（YAML 开头写流程与参数，正文是各步提示词），内置一份，
//!   用户可在配置目录覆盖或新增；
//! - `skill_files`：技能文件的启用停用、保存、导入导出；
//! - `api`：数据接口（内网 HTTP，只查询）的配置、组请求与返回映射；
//! - `engine`：按技能流程在黑板（`board`）上逐步执行，处理条件、逐项、挂起与续跑；
//! - `router`：按上下文、触发词与模型判断选技能；
//! - `backend`：模型调用接口，测试时换成按脚本回复的假模型；
//! - `evidence` / `gaps` / `clarify`：证据包与引用标记、缺口台账、选择题——全是纯逻辑。
//!
//! 三条红线在这里的落点：产物只是工作稿，经 `draft_page::tasks::reviewed_draft` 定稿成
//! 提案，由用户在侧栏接受才落入正文；工具与算子都不碰 `DraftInput`，文种切换由用户点选后在
//! 界面线程执行。

pub(crate) mod api;
pub(crate) mod backend;
pub(crate) mod board;
pub(crate) mod budget;
pub(crate) mod clarify;
pub(crate) mod engine;
pub(crate) mod evidence;
pub(crate) mod gaps;
pub(crate) mod ops;
pub(crate) mod references;
pub(crate) mod router;
pub(crate) mod skill;
pub(crate) mod skill_files;
pub(crate) mod style;
pub(crate) mod toolcall;
pub(crate) mod tools;

#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod builtin_tests;
#[cfg(test)]
mod context_tests;
#[cfg(test)]
pub(crate) mod testkit;
