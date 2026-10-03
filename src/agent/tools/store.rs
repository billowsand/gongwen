//! C 组：知识库与稿件库（只读）。资料类结果默认并入证据包，带出处、可被引用、可核验。

use super::{
    Input, Permission, Tool, ToolCtx, ToolOutput, arg_i64, arg_str, arg_usize, kind_arg, optional,
    required, short,
};
use crate::agent::evidence::EvidenceDoc;
use crate::manuscript::{ManuscriptFilter, VersionRow};
use crate::models::{DraftInput, ManuscriptStatus, TemplateKind};
use serde_json::{Map, Value, json};

pub(super) const TOOLS: [&dyn Tool; 6] =
    [&KbSearch, &KbRead, &KbList, &MsSearch, &MsRead, &MsVersions];

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

fn status_from(text: &str) -> Option<ManuscriptStatus> {
    let text = text.trim();
    [
        ManuscriptStatus::New,
        ManuscriptStatus::Draft,
        ManuscriptStatus::Published,
        ManuscriptStatus::Archived,
    ]
    .into_iter()
    .find(|status| status.label() == text || format!("{status:?}").eq_ignore_ascii_case(text))
}

struct KbSearch;

impl Tool for KbSearch {
    fn id(&self) -> &'static str {
        "kb.search"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "检索本机知识库（与知识库页的检索、问答同一套），返回相关片段"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("query", "检索词：用一句话说要找什么"),
            optional("kind", "只要某个文种的片段"),
            optional("top", "最多返回几段"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        if !ctx.env.kb.enabled() {
            return Err("知识库未启用".into());
        }
        let query = arg_str(args, "query").unwrap_or_default();
        let (mut chunks, warnings) = ctx.env.kb.search(&query).map_err(|e| format!("{e:#}"))?;
        if let Some(kind) = kind_arg(args, "kind") {
            chunks.retain(|chunk| chunk.kind == kind);
        }
        if let Some(top) = arg_usize(args, "top") {
            chunks.truncate(top);
        }
        for warning in warnings {
            (ctx.emit)(crate::agent::engine::Event::Note(format!(
                "知识库：{warning}"
            )));
        }
        let value = Value::Array(
            chunks
                .iter()
                .map(|c| json!({"title": c.doc_title, "section": c.section, "text": c.text, "doc_id": c.doc_id}))
                .collect(),
        );
        let mut output = ToolOutput::new(
            value,
            format!("检索知识库「{}」→ {} 段", short(&query, 20), chunks.len()),
        );
        output.evidence = chunks.iter().map(EvidenceDoc::from_chunk).collect();
        output.evidence_by_default = true;
        Ok(output)
    }
}

struct KbRead;

impl Tool for KbRead {
    fn id(&self) -> &'static str {
        "kb.read"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "读知识库里某篇文档的全文"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required(
            "doc_id",
            "文档 id（kb.list 或 kb.search 的结果里有）",
        )];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let id = arg_i64(args, "doc_id").ok_or("doc_id 要是数字")?;
        let (title, text) = ctx
            .env
            .kb
            .read(id)
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("知识库里没有 id 为 {id} 的文档"))?;
        let chars = text.chars().count();
        let mut output = ToolOutput::new(
            json!({"id": id, "title": title, "text": text}),
            format!("读取知识库《{title}》（{chars} 字）"),
        );
        output.evidence = vec![EvidenceDoc {
            key: format!("kbdoc:{id}"),
            title,
            section: String::new(),
            kind_label: "知识库".into(),
            text,
        }];
        output.evidence_by_default = true;
        Ok(output)
    }
}

struct KbList;

impl Tool for KbList {
    fn id(&self) -> &'static str {
        "kb.list"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "列出知识库里的文档"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[optional("kind", "只列某个文种")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let docs = ctx
            .env
            .kb
            .list(kind_arg(args, "kind"))
            .map_err(|e| format!("{e:#}"))?;
        let count = docs.len();
        let value = Value::Array(
            docs.into_iter()
                .map(|(id, title, kind)| json!({"id": id, "title": title, "kind": kind.label()}))
                .collect(),
        );
        Ok(ToolOutput::new(value, format!("知识库共 {count} 篇")))
    }
}

struct MsSearch;

impl Tool for MsSearch {
    fn id(&self) -> &'static str {
        "ms.search"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "检索稿件库：按关键词（标题、文号、备注）、文种、状态、成文日期"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            optional("keyword", "关键词"),
            optional("kind", "文种"),
            optional("status", "状态：新建 / 草稿 / 已发布 / 已归档"),
            optional("date_from", "成文日期起，YYYY-MM-DD"),
            optional("date_to", "成文日期止，YYYY-MM-DD"),
            optional("limit", "最多几篇，默认 10"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let filter = ManuscriptFilter {
            keyword: arg_str(args, "keyword").unwrap_or_default(),
            status: arg_str(args, "status").and_then(|s| status_from(&s)),
            kind: kind_arg(args, "kind"),
            date_from: arg_str(args, "date_from").unwrap_or_default(),
            date_to: arg_str(args, "date_to").unwrap_or_default(),
        };
        let limit = arg_usize(args, "limit").unwrap_or(10).clamp(1, 50);
        let docs = ctx
            .env
            .manuscripts
            .search(&filter, limit)
            .map_err(|e| format!("{e:#}"))?;
        let count = docs.len();
        let value = Value::Array(
            docs.into_iter()
                .map(|doc| {
                    json!({
                        "id": doc.id,
                        "title": doc.title,
                        "kind": doc.kind.label(),
                        "status": doc.status.label(),
                        "doc_number": doc.doc_number,
                        "doc_date": doc.doc_date,
                    })
                })
                .collect(),
        );
        let label = if filter.keyword.is_empty() {
            "全部".to_string()
        } else {
            format!("「{}」", short(&filter.keyword, 16))
        };
        Ok(ToolOutput::new(
            value,
            format!("检索稿件库{label} → {count} 篇"),
        ))
    }
}

struct MsRead;

impl Tool for MsRead {
    fn id(&self) -> &'static str {
        "ms.read"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "读稿件库里一篇稿件的要素与正文，可指定历史版本"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("id", "稿件 id（ms.search 的结果里有）"),
            optional("version", "版本号；不给读当前版本"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let id = arg_i64(args, "id").ok_or("id 要是数字")?;
        let version = arg_i64(args, "version");
        let doc = ctx
            .env
            .manuscripts
            .read(id, version)
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("稿件库里没有 id 为 {id} 的稿件（或没有该版本）"))?;
        let version_label = doc
            .version
            .map_or_else(|| "当前版本".to_string(), |v| format!("v{v}"));
        let chars = doc.markdown.chars().filter(|c| !c.is_whitespace()).count();
        let mut output = ToolOutput::new(
            json!({
                "id": doc.id,
                "title": doc.title,
                "kind": doc.kind.label(),
                "status": doc.status.label(),
                "doc_number": doc.doc_number,
                "doc_date": doc.doc_date,
                "version": doc.version,
                "text": doc.markdown,
                "elements": serde_json::to_value(&doc.draft).unwrap_or(Value::Null),
            }),
            format!("读取稿件《{}》{version_label}（{chars} 字）", doc.title),
        );
        output.evidence = vec![EvidenceDoc {
            key: format!(
                "ms:{}:{}",
                doc.id,
                doc.version.map_or("latest".into(), |v| v.to_string())
            ),
            title: doc.title,
            section: version_label,
            kind_label: "稿件库".into(),
            text: doc.markdown,
        }];
        output.evidence_by_default = true;
        Ok(output)
    }
}

struct MsVersions;

impl Tool for MsVersions {
    fn id(&self) -> &'static str {
        "ms.versions"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "一篇稿件的历史版本列表"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required("id", "稿件 id")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let id = arg_i64(args, "id").ok_or("id 要是数字")?;
        let rows = ctx
            .env
            .manuscripts
            .versions(id)
            .map_err(|e| format!("{e:#}"))?;
        let count = rows.len();
        let value = Value::Array(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "version": row.version_number,
                        "name": row.name,
                        "title": row.title,
                        "doc_date": row.doc_date,
                        "created_at": row.created_at,
                    })
                })
                .collect(),
        );
        Ok(ToolOutput::new(
            value,
            format!("稿件 {id} 共 {count} 个版本"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use serde_json::json;

    #[test]
    fn knowledge_tools_return_evidence() {
        let mut fixture = Fixture::new("");
        let hits = fixture
            .call("kb.search", json!({"query": "防火责任"}))
            .unwrap();
        assert_eq!(hits.value[0]["title"], "森林防火条例");
        assert_eq!(hits.evidence.len(), 1);
        assert_eq!(hits.evidence[0].key, "kb:7");
        assert!(hits.evidence_by_default);
        let filtered = fixture
            .call("kb.search", json!({"query": "防火", "kind": "公函"}))
            .unwrap();
        assert_eq!(filtered.value.as_array().unwrap().len(), 0);

        let doc = fixture.call("kb.read", json!({"doc_id": 3})).unwrap();
        assert_eq!(doc.value["title"], "森林防火条例");
        assert_eq!(doc.evidence[0].key, "kbdoc:3");
        assert!(
            fixture
                .call("kb.read", json!({"doc_id": 99}))
                .unwrap_err()
                .contains("没有 id")
        );
        assert_eq!(
            fixture.call("kb.list", json!({})).unwrap().value[0]["id"],
            3
        );
    }

    #[test]
    fn manuscript_tools_search_read_and_list_versions() {
        let mut fixture = Fixture::new("");
        let found = fixture
            .call(
                "ms.search",
                json!({"keyword": "森林防火", "status": "已发布"}),
            )
            .unwrap();
        assert_eq!(found.value[0]["id"], 1);
        assert!(found.summary.contains("1 篇"));
        let none = fixture
            .call("ms.search", json!({"keyword": "防汛"}))
            .unwrap();
        assert_eq!(none.value.as_array().unwrap().len(), 0);

        let doc = fixture.call("ms.read", json!({"id": "1"})).unwrap();
        assert!(doc.value["text"].as_str().unwrap().contains("加强巡查"));
        assert_eq!(doc.evidence[0].key, "ms:1:latest");
        let old = fixture
            .call("ms.read", json!({"id": 1, "version": 1}))
            .unwrap();
        assert_eq!(old.evidence[0].key, "ms:1:1");
        assert_eq!(
            fixture.call("ms.versions", json!({"id": 1})).unwrap().value[0]["name"],
            "初稿"
        );
    }
}
