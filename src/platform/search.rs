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
    /// 命中片段在原文里的字节区间，供编辑器跳转/高亮用。
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchHit {
    pub manuscript_id: i64,
    pub title: String,
    pub kind: TemplateKind,
    pub doc_date: String,
    pub score: f32,
    pub snippets: Vec<Snippet>,
}

/// 索引文本：jieba 词 + 连续汉字二元组。二元组防专名被切碎后漏召回；
/// 分词器用 `lexicon::segmenter`，已挂公文词表。
pub(crate) fn index_text(title: &str, content: &str) -> String {
    tokenize(&format!("{title}\n{content}"))
}

pub(crate) fn tokenize(text: &str) -> String {
    let mut tokens = crate::lexicon::segmenter::tokenize(text)
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut run = Vec::new();
    for ch in text.chars() {
        if is_han(ch) {
            run.push(ch);
        } else {
            push_bigrams(&run, &mut tokens);
            run.clear();
        }
    }
    push_bigrams(&run, &mut tokens);
    tokens.sort();
    tokens.dedup();
    tokens.join(" ")
}

/// 查询词：沿用同一分词器；去掉空白与纯标点，最多 32 组。
pub(crate) fn query_tokens(query: &str) -> Vec<String> {
    let mut tokens = crate::lexicon::segmenter::tokenize(query)
        .split_whitespace()
        .filter(|token| token.chars().any(|ch| ch.is_alphanumeric() || is_han(ch)))
        .map(str::to_string)
        .collect::<Vec<_>>();
    tokens.truncate(32);
    tokens.dedup();
    tokens
}

/// 拼 FTS5 查询：每个词加引号、组间 AND，用户输入不会被当成运算符。
pub(crate) fn fts_query(query: &str) -> Option<String> {
    let tokens = query_tokens(query);
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .iter()
            .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

fn push_bigrams(run: &[char], out: &mut Vec<String>) {
    if run.len() < 2 {
        return;
    }
    for pair in run.windows(2) {
        out.push(pair.iter().collect());
    }
}

fn is_han(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}
