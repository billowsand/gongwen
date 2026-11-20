//! 检索公共类型：一个查询、一种命中。
//!
//! F0 只定形状；F2 的 `manuscript_fts` 是第一个实现。知识库/维基的 FTS 先不改，
//! 以后换成同一个 `SearchIndex` 适配器。

use crate::models::{ManuscriptStatus, TemplateKind};
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Corpus {
    /// 稿件标题/文号/备注等元数据。
    ManuscriptMeta,
    /// 稿件正文。
    ManuscriptBody,
    /// 公文登记簿/制度清单。
    Registry,
    /// 标准词库。
    Vocabulary,
    /// 维基知识库。
    Wiki,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SearchFilters {
    pub security_level: Option<String>,
    pub year: Option<String>,
    pub unit: Option<String>,
    pub kind: Option<TemplateKind>,
    pub status: Option<ManuscriptStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchQuery {
    pub text: String,
    pub corpus: Corpus,
    pub filters: SearchFilters,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snippet {
    pub text: String,
    /// 命中片段在原文里的字符区间，供编辑器跳转/高亮用。
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchHit {
    pub manuscript_id: i64,
    pub score: f32,
    pub snippets: Vec<Snippet>,
}
