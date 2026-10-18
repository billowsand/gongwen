//! 知识库检索（RAG）的向量、重排与切分配置。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::rag`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{EmbeddingConfig, ModelRef};
use serde::{Deserialize, Serialize};

/// 重排方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RerankMode {
    /// 不重排，直接按 RRF 融合分取前 N 条。
    None,
    /// 专用 rerank 端点（Jina / Cohere / TEI / Infinity / llama.cpp 等）。
    #[default]
    Api,
    /// 用对话大模型逐条打分重排。**LM Studio / Ollama 不提供 rerank 端点**，
    /// 这是不另起服务的替代方案：复用已配好的对话模型，代价是每次检索
    /// 多一次 LLM 调用（低温、短输出，通常几秒）。
    Llm,
}

impl RerankMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "不重排",
            Self::Api => "专用 rerank 端点",
            Self::Llm => "用对话大模型重排",
        }
    }

    pub const ALL: [Self; 3] = [Self::None, Self::Api, Self::Llm];
}

/// rerank 模型接入。rerank 不是 OpenAI 标准接口，各家（Jina / Cohere / TEI）的
/// 路径与响应字段不一，故路径与响应取值键做成可配，默认取 Jina/Cohere 风格。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RerankConfig {
    /// 重排方式。默认 `api`，保持既有配置的行为不变。
    pub mode: RerankMode,
    /// rerank 模型（专用端点模式），引用「模型服务商管理」里的提供商。
    pub model_ref: ModelRef,
    /// 旧字段：内联地址 / 模型 / 密钥，迁移后清空，仅作向后兼容读取。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    /// 端点路径，拼在提供商地址后（默认 "rerank"）。路径与响应取值键是各家
    /// 服务的方言，不属于连接身份，留在功能侧。
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// 请求里 top_n 字段名；空串表示不传 top_n。
    pub top_n_field: String,
    /// 响应里结果数组 / 原下标 / 相关分的取值键。
    pub response_results_key: String,
    pub response_index_key: String,
    pub response_score_key: String,
    pub timeout_seconds: u64,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            mode: RerankMode::default(),
            model_ref: ModelRef::default(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            path: "rerank".into(),
            model: String::new(),
            api_key: String::new(),
            top_n_field: "top_n".into(),
            response_results_key: "results".into(),
            response_index_key: "index".into(),
            response_score_key: "relevance_score".into(),
            timeout_seconds: 60,
        }
    }
}

/// 知识库检索增强（RAG）总配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RagConfig {
    /// 全局开关；起草页开关在此基础上临时覆盖。
    pub enabled: bool,
    pub embedding: EmbeddingConfig,
    pub rerank: RerankConfig,
    /// 向量路召回条数。
    pub recall_top_k: usize,
    /// 关键词（FTS5）路召回条数。
    pub fts_top_k: usize,
    /// 两路 RRF 融合后送 rerank 的条数。
    pub fused_top_m: usize,
    /// 最终注入提示词的参考块数。
    pub rerank_top_n: usize,
    /// 切块目标字数 / 上限 / 相邻块重叠。
    pub chunk_target_chars: usize,
    pub chunk_max_chars: usize,
    pub chunk_overlap_chars: usize,
    /// RRF 融合常数 k。
    pub rrf_k: f32,
    /// 相关性下限，按与首条结果的比例算（0 = 不过滤，0.5 = 砍掉不足首条一半分的）。
    /// 用相对比例而非绝对分数，是因为不同 rerank/embedding 模型的分值范围差异很大。
    pub min_score_ratio: f32,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding: EmbeddingConfig::default(),
            rerank: RerankConfig::default(),
            recall_top_k: 20,
            fts_top_k: 20,
            fused_top_m: 12,
            rerank_top_n: 5,
            chunk_target_chars: 400,
            chunk_max_chars: 600,
            chunk_overlap_chars: 80,
            rrf_k: 60.0,
            min_score_ratio: 0.5,
        }
    }
}

impl RagConfig {
    /// 实际生效的重排方式：选了「专用端点」却没填模型名时降级为不重排。
    /// 模型名看新引用，也认迁移前的旧内联字段。
    pub(crate) fn effective_rerank_mode(&self) -> RerankMode {
        match self.rerank.mode {
            RerankMode::Api
                if self.rerank.model_ref.is_empty() && self.rerank.model.trim().is_empty() =>
            {
                RerankMode::None
            }
            mode => mode,
        }
    }
}
