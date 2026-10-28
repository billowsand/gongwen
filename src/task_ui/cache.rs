//! 页面读到的任务数据，按 `Store::data_key` 失效。
//!
//! egui 每次鼠标移动都会重绘；不缓存时任务、候选、分类、历史每帧各查一遍，
//! 搜索还要逐任务查一次材料来源。改成代号不变就不查库，写入后下一帧整体重取。
use crate::task_assistant::{CandidateRow, HistoryEntry, Store, Task};
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct Cache {
    key: Option<(u64, u64)>,
    pub tasks: Vec<Task>,
    pub categories: Vec<String>,
    pub candidates: Vec<CandidateRow>,
    pub auto: bool,
    hits: Option<(String, HashSet<String>)>,
    history: Option<(String, Vec<HistoryEntry>)>,
}

impl Cache {
    /// 代号变了才重取；读失败也记下代号，免得每帧重试同一个错误。
    pub fn refresh(&mut self, store: &Store) -> Result<(), String> {
        let key = store.data_key();
        if self.key == Some(key) {
            return Ok(());
        }
        self.key = Some(key);
        self.hits = None;
        self.history = None;
        let mut errors = Vec::new();
        match store.list(true) {
            Ok(v) => self.tasks = v,
            Err(e) => errors.push(format!("读取任务失败：{e:#}")),
        }
        match store.categories() {
            Ok(v) => self.categories = v,
            Err(e) => errors.push(format!("读取分类失败：{e:#}")),
        }
        match store.candidates() {
            Ok(v) => self.candidates = v,
            Err(e) => errors.push(format!("读取待整理建议失败：{e:#}")),
        }
        match store.auto() {
            Ok(v) => self.auto = v,
            Err(e) => errors.push(format!("读取自动整理设置失败：{e:#}")),
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    /// 材料来源或历史说明命中关键词的任务；关键词或数据变了才重查。
    pub fn hits(&mut self, store: &Store, query: &str) -> Result<&HashSet<String>, String> {
        if self.hits.as_ref().is_none_or(|(q, _)| q != query) {
            let found = store
                .source_hits(query)
                .map_err(|e| format!("搜索材料来源失败：{e:#}"))?;
            self.hits = Some((query.to_owned(), found));
        }
        Ok(&self.hits.as_ref().expect("刚写入").1)
    }

    pub fn history(&mut self, store: &Store, task_id: &str) -> Result<&[HistoryEntry], String> {
        if self.history.as_ref().is_none_or(|(id, _)| id != task_id) {
            let entries = store
                .history_entries(task_id)
                .map_err(|e| format!("读取历史失败：{e:#}"))?;
            self.history = Some((task_id.to_owned(), entries));
        }
        Ok(&self.history.as_ref().expect("刚写入").1)
    }
}
