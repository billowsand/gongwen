//! 知识库（只读）的窄接口与本机实现。
//!
//! 从旧 `agent::tools` 搬来，语义与降级说明不变：知识库没启用就不检索，检索与知识库页的
//! 检索、问答走同一个 `rag::retrieve`。旧工具层只做 JSON / 证据包装。

use crate::models::{LmStudioConfig, RagConfig, TemplateKind};
use crate::rag::RetrievedChunk;

/// 知识库（只读）。测试时换成假知识库。
pub(crate) trait KnowledgeSearch {
    /// 知识库没启用时不检索，可检索的缺口直接记为「知识库无答案」。
    fn enabled(&self) -> bool;
    /// 返回 (片段, 降级说明)。
    fn search(&self, query: &str) -> anyhow::Result<(Vec<RetrievedChunk>, Vec<String>)>;
    /// 一篇文档的 (标题, 全文)。
    fn read(&self, _doc_id: i64) -> anyhow::Result<Option<(String, String)>> {
        Ok(None)
    }
    /// 文档列表：(id, 标题, 文种)。
    fn list(
        &self,
        _kind: Option<TemplateKind>,
    ) -> anyhow::Result<Vec<(i64, String, TemplateKind)>> {
        Ok(Vec::new())
    }
}

/// 本机知识库，检索与知识库页的检索、问答走同一个 `rag::retrieve`。
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

    fn read(&self, doc_id: i64) -> anyhow::Result<Option<(String, String)>> {
        let db_path = crate::storage::manuscript_db_path()?;
        crate::knowledge::KnowledgeStore::open(&db_path)?.get_doc_content(doc_id)
    }

    fn list(&self, kind: Option<TemplateKind>) -> anyhow::Result<Vec<(i64, String, TemplateKind)>> {
        let db_path = crate::storage::manuscript_db_path()?;
        Ok(crate::knowledge::KnowledgeStore::open(&db_path)?
            .list_docs(kind)?
            .into_iter()
            .map(|row| (row.id, row.title, row.kind))
            .collect())
    }
}
