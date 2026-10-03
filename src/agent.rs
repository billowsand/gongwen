//! 智能体工作台的后台：技能、工具、模型接口与研究式起草流程。
//!
//! 设计见 `docs/ai-agent-workbench.md` 第十四节。分层：
//! - `skill`：SKILL.md 技能文件（提示词与流程参数），内置一份，用户可在配置目录覆盖；
//! - `backend`：模型调用接口，测试时换成按脚本回复的假模型；
//! - `tools`：知识库检索等工具与权限级别，每次调用都在任务流里留一行；
//! - `evidence` / `gaps` / `clarify`：证据包与引用标记、缺口台账、选择题——全是纯逻辑；
//! - `research`：把以上串成「预研 → 起草 → 缺口循环 → 核验 → 出题」的流程。
//!
//! 三条红线在这里的落点：产物只是工作稿，经 `draft_page::tasks::reviewed_draft` 定稿成
//! 提案，由用户在侧栏接受才落入正文；流程不碰 `DraftInput`，文种切换由用户点选后在
//! 界面线程执行。

pub(crate) mod backend;
pub(crate) mod clarify;
pub(crate) mod evidence;
pub(crate) mod gaps;
pub(crate) mod research;
pub(crate) mod skill;
pub(crate) mod tools;
