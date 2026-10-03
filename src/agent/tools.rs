//! 工具层：流程能调用的工具、它们的权限级别，以及每次调用留在任务流里的那一行。
//!
//! 权限分四级（`docs/ai-agent-workbench.md` 4.1）：只读、检查、写工作稿、问用户。
//! **没有任何工具能写正文或改要素**——正文只能由用户在侧栏接受提案时改写。
//!
//! 本期只有进程内的调用方（研究式起草流程）。第 ③ 期接 opencode 时，同一批工具再经最小
//! MCP 服务暴露出去，权限检查仍放在这一层。

use crate::models::{LmStudioConfig, RagConfig, TemplateKind};
use crate::rag::RetrievedChunk;

/// 工具的权限级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Permission {
    /// 只读：检索知识库、读稿件。
    Read,
    /// 检查：校对、要素校验、事实比对。确定性，不改任何东西。
    Check,
    /// 写 AI 工作稿。工作稿不是正文，落地要用户合并。
    WriteWorkspace,
    /// 问用户：出选择题，流程停下等回答。
    AskUser,
}

impl Permission {
    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Read => "⌕",
            Self::Check => "✓",
            Self::WriteWorkspace => "✎",
            Self::AskUser => "?",
        }
    }
}

/// 一次工具调用留下的记录，任务流里显示成一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolUse {
    pub(crate) tool: &'static str,
    pub(crate) permission: Permission,
    /// 「检索「冬季防火依据」→ 5 段（新增 3）」这样一句人话。
    pub(crate) summary: String,
}

impl ToolUse {
    pub(crate) fn new(tool: &'static str, permission: Permission, summary: String) -> Self {
        Self {
            tool,
            permission,
            summary,
        }
    }

    pub(crate) fn line(&self) -> String {
        format!("{} {}", self.permission.icon(), self.summary)
    }
}

/// 知识库检索（只读工具）。测试时换成按脚本返回的假知识库。
pub(crate) trait KnowledgeSearch {
    /// 知识库没启用时不检索，可检索的缺口直接记为「知识库无答案」。
    fn enabled(&self) -> bool;
    /// 返回 (片段, 降级说明)。
    fn search(&self, query: &str) -> anyhow::Result<(Vec<RetrievedChunk>, Vec<String>)>;
}

/// 用本机知识库检索，与知识库页的检索、问答走同一个 `rag::retrieve`。
pub(crate) struct RagSearch {
    pub(crate) enabled: bool,
    pub(crate) rag: RagConfig,
    pub(crate) chat: LmStudioConfig,
    pub(crate) kind_filter: Option<TemplateKind>,
}

impl KnowledgeSearch for RagSearch {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn search(&self, query: &str) -> anyhow::Result<(Vec<RetrievedChunk>, Vec<String>)> {
        let db_path = crate::storage::manuscript_db_path()?;
        let outcome =
            crate::rag::retrieve(&self.rag, &self.chat, &db_path, query, self.kind_filter)?;
        Ok((outcome.chunks, outcome.warnings))
    }
}
