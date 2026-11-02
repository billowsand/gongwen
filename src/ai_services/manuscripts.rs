//! 稿件库（只读）的窄接口与本机实现。
//!
//! 从旧 `agent::tools::store` 搬来：版本读取、状态字段与限额都不变；旧工具层只做
//! JSON / 证据包装。每次调用在当前线程里打开 SQLite，与知识库检索的做法一致。

use crate::manuscript::{ManuscriptFilter, VersionRow};
use crate::models::{DraftInput, ManuscriptStatus, TemplateKind};

/// 一篇稿件（某个版本）。
#[derive(Debug, Clone)]
pub(crate) struct ManuscriptDoc {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) kind: TemplateKind,
    pub(crate) status: ManuscriptStatus,
    pub(crate) doc_number: String,
    pub(crate) doc_date: String,
    pub(crate) draft: DraftInput,
    pub(crate) markdown: String,
    /// None 表示当前版本。
    pub(crate) version: Option<i64>,
}

/// 稿件库（只读）。测试时换成假稿件库。
pub(crate) trait ManuscriptSource {
    /// 列表检索；返回的 `markdown` 可以为空（列表用不到正文）。
    fn search(&self, filter: &ManuscriptFilter, limit: usize)
    -> anyhow::Result<Vec<ManuscriptDoc>>;
    fn read(&self, id: i64, version: Option<i64>) -> anyhow::Result<Option<ManuscriptDoc>>;
    fn versions(&self, id: i64) -> anyhow::Result<Vec<VersionRow>>;
}

/// 本机稿件库：每次调用在当前线程里打开 SQLite，与知识库检索的做法一致。
pub(crate) struct SqliteManuscripts;

impl SqliteManuscripts {
    fn open() -> anyhow::Result<crate::manuscript::ManuscriptStore> {
        crate::manuscript::ManuscriptStore::open(&crate::storage::manuscript_db_path()?)
    }
}

impl ManuscriptSource for SqliteManuscripts {
    fn search(
        &self,
        filter: &ManuscriptFilter,
        limit: usize,
    ) -> anyhow::Result<Vec<ManuscriptDoc>> {
        Ok(Self::open()?
            .list(filter)?
            .into_iter()
            .take(limit)
            .map(|row| ManuscriptDoc {
                id: row.id,
                title: row.title,
                kind: row.kind,
                status: row.status,
                doc_number: row.doc_number,
                doc_date: row.doc_date,
                draft: DraftInput::default(),
                markdown: String::new(),
                version: None,
            })
            .collect())
    }

    fn read(&self, id: i64, version: Option<i64>) -> anyhow::Result<Option<ManuscriptDoc>> {
        let mut store = Self::open()?;
        let Some(record) = store.get(id)? else {
            return Ok(None);
        };
        let mut doc = ManuscriptDoc {
            id: record.id,
            title: record.title,
            kind: record.kind,
            status: record.status,
            doc_number: record.doc_number,
            doc_date: record.doc_date,
            draft: record.snapshot,
            markdown: record.content_markdown,
            version: None,
        };
        if let Some(number) = version {
            let Some(old) = store.get_manuscript_version(id, number)? else {
                return Ok(None);
            };
            doc.draft = old.snapshot;
            doc.markdown = old.content_markdown;
            doc.version = Some(number);
        }
        Ok(Some(doc))
    }

    fn versions(&self, id: i64) -> anyhow::Result<Vec<VersionRow>> {
        Self::open()?.list_manuscript_versions(id)
    }
}
