//! 平台内核（F0/F1 骨架）。
//!
//! 这里先落通用数据和纯算法：单位三套代字、人员两个号码、编号方案与文号、
//! 检索/渲染/登记/检查的公共类型。F1 已把文号接到搜索和导出命名；其余类型
//! 等 F2/F3/F4 接入后，再逐步删掉 `src/main.rs` 上的 `allow(dead_code)`。
//!
//! 约定：本模块不依赖 UI，也不依赖 `ai-workbench` 特性；后台线程、非 AI 版都能用。

// 文号类型已有调用方；检索/渲染/台账/检查类型等 F2–F4 接入，先保留 allow。
#![allow(unused_imports)]

pub(crate) mod check;
pub(crate) mod db;
pub(crate) mod doc;
pub(crate) mod ledger;
pub(crate) mod render;
pub(crate) mod search;

pub(crate) use check::{Check, CheckLevel};
pub(crate) use doc::{
    DocNumber, DocumentMeta, NumberingScheme, PersonContact, SecurityMark, UnitCodes,
};
pub(crate) use ledger::{Direction, LedgerEntry, LedgerStatus, Pool};
pub(crate) use render::{
    Artifact, BatchPlan, OutputFormat, Protection, RenderOptions, RenderPlan, SourceRef,
};
pub(crate) use search::{Corpus, SearchFilters, SearchHit, SearchQuery, Snippet};
