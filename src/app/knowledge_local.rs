//! 知识库本地全文检索与旧库补索引的后台调度。

use super::jobs::KnowledgeJob;
use super::{GongwenApp, WorkerResult};
use crate::{knowledge, storage};
use std::thread;

impl GongwenApp {
    /// 无模型本地全文检索；首次搜索同时在后台补建旧文档全文索引。
    pub(crate) fn knowledge_local_search(&mut self) {
        if self.knowledge_busy {
            return;
        }
        let query = self.knowledge_test_query.trim().to_string();
        if query.chars().count() > 256 {
            self.knowledge_error = Some("检索词不能超过256个字符。".into());
            return;
        }
        if query.is_empty() {
            return;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            return;
        };
        self.knowledge_busy = true;
        self.knowledge_local_results.clear();
        self.knowledge_test_results.clear();
        self.knowledge_search_warnings.clear();
        self.knowledge_error = None;
        let kind = self.knowledge_filter_kind;
        let tx = self.sender.clone();
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<Vec<crate::knowledge::LocalSearchHit>> {
                let mut store = crate::knowledge::KnowledgeStore::open(&db_path)?;
                for id in store.unindexed_local_doc_ids()? {
                    store.index_local_document(id)?;
                }
                store.local_search(&query, kind, 50)
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::LocalSearchDone {
                query,
                kind,
                result,
            }));
        });
    }

    /// 打开知识库页面时后台补建旧数据的全文索引。
    pub(crate) fn knowledge_backfill_local_index(&mut self) {
        if self.knowledge_local_backfill_started || self.knowledge_busy {
            return;
        }
        let Some(db_path) = storage::manuscript_db_path().ok() else {
            return;
        };
        self.knowledge_local_backfill_started = true;
        self.knowledge_busy = true;
        self.knowledge_progress_verb = "正在建立本地全文索引";
        self.knowledge_index_progress = Some((0, 0, String::new()));
        let tx = self.sender.clone();
        thread::spawn(move || {
            let mut failed = Vec::new();
            let mut ok = 0;
            match knowledge::KnowledgeStore::open(&db_path) {
                Ok(mut store) => {
                    let ids = match store.unindexed_local_doc_ids() {
                        Ok(ids) => ids,
                        Err(error) => {
                            let _ =
                                tx.send(WorkerResult::Knowledge(KnowledgeJob::LocalBackfillDone {
                                    ok,
                                    failed: vec![("读取索引状态".into(), format!("{error:#}"))],
                                }));
                            return;
                        }
                    };
                    for (index, id) in ids.iter().enumerate() {
                        let title = store
                            .get_doc_content(*id)
                            .ok()
                            .flatten()
                            .map(|(title, _)| title)
                            .unwrap_or_default();
                        let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::IndexProgress {
                            done: index,
                            total: ids.len(),
                            current_title: title.clone(),
                        }));
                        match store.index_local_document(*id) {
                            Ok(true) => ok += 1,
                            Ok(false) => {}
                            Err(error) => failed.push((title, format!("{error:#}"))),
                        }
                    }
                }
                Err(error) => failed.push(("知识库".into(), format!("{error:#}"))),
            }
            let _ = tx.send(WorkerResult::Knowledge(KnowledgeJob::LocalBackfillDone {
                ok,
                failed,
            }));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::LocalSearchHit;
    use crate::models::TemplateKind;

    fn hit() -> LocalSearchHit {
        LocalSearchHit {
            doc_id: 1,
            title: "测试材料".into(),
            kind: TemplateKind::OfficialLetter,
            excerpt: "正文".into(),
            highlights: std::iter::once(0..6).collect(),
            source_offset: 0,
        }
    }

    #[test]
    fn local_fulltext_background_results_follow_current_query_and_kind() {
        let mut app = crate::app::management_samples::fixture();
        app.knowledge_test_query = "当前查询".into();
        app.knowledge_busy = true;
        app.apply_knowledge_job(KnowledgeJob::LocalSearchDone {
            query: "旧查询".into(),
            kind: None,
            result: Ok(vec![hit()]),
        });
        assert!(app.knowledge_local_results.is_empty());
        assert!(!app.knowledge_busy);
        app.apply_knowledge_job(KnowledgeJob::LocalSearchDone {
            query: "当前查询".into(),
            kind: Some(TemplateKind::WhitePaper),
            result: Ok(vec![hit()]),
        });
        assert!(app.knowledge_local_results.is_empty());
        app.apply_knowledge_job(KnowledgeJob::LocalSearchDone {
            query: "当前查询".into(),
            kind: None,
            result: Ok(vec![hit()]),
        });
        assert_eq!(app.knowledge_local_results.len(), 1);
        app.apply_knowledge_job(KnowledgeJob::LocalSearchDone {
            query: "当前查询".into(),
            kind: None,
            result: Err("读取失败".into()),
        });
        assert!(app.knowledge_local_results.is_empty());
        assert!(app.knowledge_error.as_deref().unwrap().contains("读取失败"));
    }
}
