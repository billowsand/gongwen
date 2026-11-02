//! Pi 只读领域工具与事实台账。
//!
//! host 发 `tool_request`，Rust 在这里校验工具名与参数并执行，再把结果按调用 ID 回去。
//! 这里**不**复用旧的 `Board` / `ToolCtx` / 流程引擎：只调用现有的知识库检索、
//! 稿件库、词库与确定性检查。每个真正返回给模型的资料都登记一条证据（编号、来源、
//! 哈希、片段、角色）；提交时的来源闸门只认这份台账与用户材料，不采信模型自报的引用。

use crate::ai_services::knowledge::{KnowledgeSearch, RagSearch};
use crate::ai_services::manuscripts::ManuscriptSource;
use crate::ai_services::vocabulary as vocab;
use crate::manuscript::ManuscriptFilter;
use crate::models::{AppConfig, DraftInput, TemplateKind, VocabularyCategory};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 一次工具调用最多返回多少资料。
const MAX_SEARCH_RESULTS: usize = 10;
/// 读全文的字符上限。
const MAX_READ_CHARS: usize = 200_000;
/// 一条证据交给模型的片段上限。
const MAX_SNIPPET_CHARS: usize = 600;

/// 知识库检索结果的窄接口（生产用现有 RAG，测试可换假库）。
pub(crate) trait PiKnowledge: Send + Sync {
    fn enabled(&self) -> bool;
    fn search(&self, query: &str, top: usize) -> Result<Vec<PiChunk>, String>;
    fn read(&self, doc_id: i64) -> Result<Option<(String, String)>, String>;
}

#[derive(Debug, Clone)]
pub(crate) struct PiChunk {
    pub(crate) doc_id: i64,
    pub(crate) title: String,
    pub(crate) section: String,
    pub(crate) text: String,
}

/// 稿件库的窄接口。
pub(crate) trait PiManuscripts: Send + Sync {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<PiManuscriptRow>, String>;
    fn read(&self, id: i64, version: Option<i64>) -> Result<Option<PiManuscriptRow>, String>;
}

#[derive(Debug, Clone)]
pub(crate) struct PiManuscriptRow {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) kind: TemplateKind,
    pub(crate) status: String,
    pub(crate) doc_number: String,
    pub(crate) doc_date: String,
    pub(crate) version: Option<i64>,
    pub(crate) markdown: String,
}

impl PiKnowledge for RagSearch {
    fn enabled(&self) -> bool {
        KnowledgeSearch::enabled(self)
    }

    fn search(&self, query: &str, top: usize) -> Result<Vec<PiChunk>, String> {
        let (chunks, _warnings) = KnowledgeSearch::search(self, query)
            .map_err(|error| format!("知识库检索失败：{error:#}"))?;
        Ok(chunks
            .into_iter()
            .take(top)
            .map(|chunk| PiChunk {
                doc_id: chunk.doc_id,
                title: chunk.doc_title,
                section: chunk.section,
                text: chunk.text,
            })
            .collect())
    }

    fn read(&self, doc_id: i64) -> Result<Option<(String, String)>, String> {
        KnowledgeSearch::read(self, doc_id).map_err(|error| format!("读知识库文档失败：{error:#}"))
    }
}

impl PiManuscripts for crate::ai_services::manuscripts::SqliteManuscripts {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<PiManuscriptRow>, String> {
        let filter = ManuscriptFilter {
            keyword: query.to_string(),
            ..Default::default()
        };
        let rows = ManuscriptSource::search(self, &filter, limit)
            .map_err(|error| format!("稿件库检索失败：{error:#}"))?;
        Ok(rows
            .into_iter()
            .map(|row| PiManuscriptRow {
                id: row.id,
                title: row.title,
                kind: row.kind,
                status: format!("{:?}", row.status),
                doc_number: row.doc_number,
                doc_date: row.doc_date,
                version: row.version,
                markdown: row.markdown,
            })
            .collect())
    }

    fn read(&self, id: i64, version: Option<i64>) -> Result<Option<PiManuscriptRow>, String> {
        let row = ManuscriptSource::read(self, id, version)
            .map_err(|error| format!("读稿件失败：{error:#}"))?;
        Ok(row.map(|row| PiManuscriptRow {
            id: row.id,
            title: row.title,
            kind: row.kind,
            status: format!("{:?}", row.status),
            doc_number: row.doc_number,
            doc_date: row.doc_date,
            version: row.version,
            markdown: row.markdown,
        }))
    }
}

/// 一条已返回给模型的证据。
#[derive(Debug, Clone)]
pub(crate) struct PiEvidence {
    pub(crate) id: String,
    pub(crate) tool: String,
    pub(crate) title: String,
    pub(crate) source: String,
    pub(crate) role: String,
    pub(crate) snippet: String,
    /// 全文，只在本进程内用于事实比对；不发给模型、不写日志。
    pub(crate) text: String,
    pub(crate) sha256: String,
    pub(crate) bytes: usize,
    /// 能否作为本次事项的事实来源（样稿类不能）。
    pub(crate) fact: bool,
}

impl PiEvidence {
    /// 给侧栏 / 事件看的短说明。
    pub(crate) fn line(&self) -> String {
        let first = self.snippet.lines().next().unwrap_or("").trim();
        format!(
            "证据 {} · {}《{}》· {} · {} 字节 · sha256 {}…（{}）：{}",
            self.id,
            self.source,
            self.title,
            self.role,
            self.bytes,
            &self.sha256[..12.min(self.sha256.len())],
            self.tool,
            first
        )
    }
}

/// 一轮 Pi 的工具上下文。后台线程持有一份，工具结果与证据都只活在这一轮。
pub(crate) struct PiToolContext {
    config: Arc<AppConfig>,
    draft: DraftInput,
    current: String,
    request: String,
    /// 可作为事实来源的材料全文（只有事实材料 / 来函）。
    material_facts: Vec<String>,
    /// 表单里用户已填的描述类要素：名称是用户确认的，可作为事实来源；
    /// 授权类要素不在此列，词库查询结果也不在此列。
    form_facts: Vec<String>,
    kb: Arc<dyn PiKnowledge>,
    manuscripts: Arc<dyn PiManuscripts>,
    evidence: Arc<Mutex<Vec<PiEvidence>>>,
    seq: Arc<AtomicUsize>,
}

impl PiToolContext {
    /// 生产环境：沿用现有知识库设置与检索链，不改变 embedding / 索引。
    pub(crate) fn new(
        config: Arc<AppConfig>,
        draft: DraftInput,
        current: String,
        request: String,
        material_facts: Vec<String>,
    ) -> Self {
        let kb: Arc<dyn PiKnowledge> = match config.assist_chat() {
            Ok(chat) => Arc::new(RagSearch {
                enabled: config.rag.enabled,
                rag: config.resolved_rag(),
                chat,
                kind_filter: None,
            }),
            Err(_) => match config.pi_chat() {
                Ok(chat) => Arc::new(RagSearch {
                    enabled: config.rag.enabled,
                    rag: config.resolved_rag(),
                    chat,
                    kind_filter: None,
                }),
                Err(_) => Arc::new(DisabledKnowledge),
            },
        };
        Self::with_sources(
            config,
            draft,
            current,
            request,
            material_facts,
            kb,
            Arc::new(crate::ai_services::manuscripts::SqliteManuscripts),
        )
    }

    /// 测试替身入口。
    pub(crate) fn with_sources(
        config: Arc<AppConfig>,
        draft: DraftInput,
        current: String,
        request: String,
        material_facts: Vec<String>,
        kb: Arc<dyn PiKnowledge>,
        manuscripts: Arc<dyn PiManuscripts>,
    ) -> Self {
        Self {
            form_facts: form_facts(&draft, &config.vocabulary),
            config,
            draft,
            current,
            request,
            material_facts,
            kb,
            manuscripts,
            evidence: Arc::new(Mutex::new(Vec::new())),
            seq: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn evidence(&self) -> Vec<PiEvidence> {
        self.evidence
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// 可作为事实来源的全文（用户材料 + 工具实际返回的资料）。
    pub(crate) fn fact_sources(&self) -> Vec<String> {
        let mut out = self.material_facts.clone();
        out.extend(self.form_facts.iter().cloned());
        let ledger = self
            .evidence
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for item in ledger.iter().filter(|item| item.fact) {
            out.push(item.text.clone());
        }
        out
    }

    /// 只登记用户实际提交的答案，不把模型问题中的假设当成已确认事实。
    pub(crate) fn confirm_answers(&self, values: &[String]) {
        for value in values {
            self.record("ask_user", "用户回答", "本轮用户", "事实", value, true);
        }
    }

    /// 允许的工具名（协议层先按它拒绝未知工具）。
    pub(crate) fn known(name: &str) -> bool {
        matches!(
            name,
            "kb_search"
                | "kb_read"
                | "ms_search"
                | "ms_read"
                | "vocab_search"
                | "check_document"
                | "doc_elements"
        )
    }

    fn next_id(&self) -> String {
        let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        format!("E{n}")
    }

    fn record(
        &self,
        tool: &str,
        title: &str,
        source: &str,
        role: &str,
        text: &str,
        fact: bool,
    ) -> String {
        let id = self.next_id();
        let snippet: String = text.chars().take(MAX_SNIPPET_CHARS).collect();
        let item = PiEvidence {
            id: id.clone(),
            tool: tool.into(),
            title: title.into(),
            source: source.into(),
            role: role.into(),
            snippet,
            text: text.to_string(),
            sha256: sha256_hex(text),
            bytes: text.len(),
            fact,
        };
        self.evidence
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(item);
        id
    }

    /// 读一条用户 `@` 引用的全文，作为本轮材料快照。失败明确返回，不静默丢。
    pub(crate) fn read_reference(
        &self,
        source: crate::ai_services::references::RefSource,
        id: i64,
    ) -> Result<(String, String), String> {
        match source {
            crate::ai_services::references::RefSource::Knowledge => self
                .kb
                .read(id)?
                .ok_or_else(|| format!("知识库里找不到编号为 {id} 的引用文章")),
            crate::ai_services::references::RefSource::Manuscript => {
                let row = self
                    .manuscripts
                    .read(id, None)?
                    .ok_or_else(|| format!("稿件库里找不到编号为 {id} 的引用稿件"))?;
                if row.markdown.trim().is_empty() {
                    return Err(format!("引用的稿件《{}》没有正文", row.title));
                }
                Ok((row.title, row.markdown))
            }
        }
    }

    /// 执行一次工具调用。参数校验失败与业务失败都返回结构化错误文本。
    pub(crate) fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        let args = args
            .as_object()
            .ok_or_else(|| "工具参数必须是 JSON 对象".to_string())?;
        match name {
            "kb_search" => self.kb_search(args),
            "kb_read" => self.kb_read(args),
            "ms_search" => self.ms_search(args),
            "ms_read" => self.ms_read(args),
            "vocab_search" => self.vocab_search(args),
            "check_document" => self.check_document(args),
            "doc_elements" => Ok(self.doc_elements()),
            other => Err(format!("未知工具：{other}")),
        }
    }

    fn kb_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        if !self.kb.enabled() {
            return Err("知识库未启用：请到「知识库」页启用并配置检索后再试。".into());
        }
        let query = required_str(args, "query")?;
        let top = usize_arg(args, "top", MAX_SEARCH_RESULTS);
        let chunks = self.kb.search(&query, top)?;
        if chunks.is_empty() {
            return Ok(json!({"results": [], "note": "知识库没有找到相关片段"}));
        }
        let mut results = Vec::new();
        for chunk in chunks {
            let id = self.record(
                "kb_search",
                &chunk.title,
                "知识库",
                "资料",
                &chunk.text,
                true,
            );
            results.push(json!({
                "evidence": id,
                "doc_id": chunk.doc_id,
                "title": chunk.title,
                "section": chunk.section,
                "text": chunk.text,
            }));
        }
        Ok(json!({"results": results}))
    }

    fn kb_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let doc_id = int_arg(args, "doc_id")?;
        let Some((title, text)) = self.kb.read(doc_id)? else {
            return Err(format!("知识库没有编号为 {doc_id} 的文档"));
        };
        let text = limit_chars(&text, MAX_READ_CHARS);
        let id = self.record("kb_read", &title, "知识库", "全文", &text, true);
        Ok(json!({"evidence": id, "doc_id": doc_id, "title": title, "text": text}))
    }

    fn ms_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let query = required_str(args, "query")?;
        let limit = usize_arg(args, "limit", 5);
        let rows = self.manuscripts.search(&query, limit)?;
        let mut results = Vec::new();
        for row in rows {
            // 列表项本身不足以当事实依据：把标题与文号登记为摘要，正文要靠 ms_read。
            let summary = format!(
                "《{}》{} {} {}",
                row.title, row.doc_number, row.doc_date, row.status
            );
            let id = self.record("ms_search", &row.title, "稿件库", "摘要", &summary, true);
            results.push(json!({
                "evidence": id,
                "id": row.id,
                "title": row.title,
                "kind": row.kind.label(),
                "status": row.status,
                "doc_number": row.doc_number,
                "doc_date": row.doc_date,
            }));
        }
        Ok(json!({"results": results}))
    }

    fn ms_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = int_arg(args, "id")?;
        let version = args.get("version").and_then(Value::as_i64);
        let Some(row) = self.manuscripts.read(id, version)? else {
            return Err(format!("稿件库没有编号为 {id} 的稿件"));
        };
        let text = limit_chars(&row.markdown, MAX_READ_CHARS);
        if text.trim().is_empty() {
            return Err(format!("稿件《{}》没有正文", row.title));
        }
        let label = match row.version {
            Some(version) => format!("《{}》v{version}", row.title),
            None => format!("《{}》", row.title),
        };
        let evidence = self.record("ms_read", &label, "稿件库", "全文", &text, true);
        Ok(json!({
            "evidence": evidence,
            "id": row.id,
            "version": row.version,
            "title": row.title,
            "kind": row.kind.label(),
            "status": row.status,
            "markdown": text,
        }))
    }

    fn vocab_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let query = required_str(args, "query")?;
        let category = match args.get("category").and_then(Value::as_str) {
            Some("unit" | "单位") => Some(VocabularyCategory::Unit),
            Some("person" | "人员") => Some(VocabularyCategory::Person),
            Some(other) => return Err(format!("不认识的词库类别：{other}")),
            None => None,
        };
        let mut rows = Vec::new();
        for entry in &self.config.vocabulary {
            if category.is_some_and(|category| entry.category != category) {
                continue;
            }
            if !vocab::matches(entry, &query) {
                continue;
            }
            rows.push(json!({
                "name": entry.canonical,
                "category": entry.category.label(),
                "code": entry.code,
                "position": entry.position,
                "phone": entry.phone,
                "aliases": entry.aliases,
                "note": entry.note,
            }));
            if rows.len() >= 20 {
                break;
            }
        }
        Ok(json!({"results": rows, "note": "词库只提供标准写法，不产生事实证据"}))
    }

    fn check_document(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| self.current.clone());
        if text.trim().is_empty() {
            return Err("没有可检查的正文".into());
        }
        let mut hard = Vec::new();
        let mut soft = Vec::new();
        if let Err(error) = crate::document_reference::ensure_preserved(&self.current, &text) {
            hard.push(format!("公文引用检查未通过：{error}"));
        }
        let stripped = crate::ai_services::citations::strip_citations(&text);
        let mut sources = vec![self.current.clone(), self.request.clone()];
        sources.extend(self.fact_sources());
        let joined = sources.join("\n");
        for fact in crate::ai_guard::extract_key_facts(&stripped, &self.config.vocabulary) {
            if !joined.contains(&fact.value) {
                hard.push(format!(
                    "{}「{}」找不到出处：补充依据，或改成【待核实】",
                    fact.kind.label(),
                    fact.value
                ));
            }
        }
        let lexicon = crate::proofread::Lexicon::resolved(&self.config.proofread);
        for note in lexicon.check(&text) {
            soft.push(format!("［{}］{}", note.level.label(), note.message));
        }
        for note in
            crate::proofread_rules::check_with_config(&self.draft, &text, &self.config.proofread)
        {
            soft.push(format!("［{}］{}", note.level.label(), note.message));
        }
        for warning in crate::validator::validate(
            &self.draft,
            &text,
            &self.config.vocabulary,
            &self.config.security_rules,
        ) {
            soft.push(warning);
        }
        hard.truncate(20);
        soft.truncate(40);
        Ok(json!({
            "hard": hard,
            "soft": soft,
            "note": "硬问题必须修订后才能提交；软问题程序会附在提案上供人核对",
        }))
    }

    /// 只读当前表单要素：描述类给值，授权类只列只读值，任何情况下都不写表单。
    /// 值先换成名称再给模型——身份引用（`@unit:` / `@person:`）进模型会被照抄进正文。
    fn doc_elements(&self) -> Value {
        let readable = crate::units::identity::readable(&self.draft, &self.config.vocabulary);
        let mut description = Vec::new();
        for field in crate::element_fields::FieldId::ALL {
            if !field.applies(&readable) {
                continue;
            }
            description.push(json!({
                "field": field.label(),
                "value": field.read(&readable),
                "required": field.required(readable.kind),
            }));
        }
        let profile = &readable.profile;
        let mut authorized = Vec::new();
        for (label, value) in [
            ("密级", profile.security_level.as_str()),
            ("保密期限", profile.security_period.as_str()),
            ("发文字号中的年份", profile.document_year.as_str()),
            ("发文字号序号", profile.document_number.as_str()),
        ] {
            if !value.trim().is_empty() {
                authorized.push(json!({"field": label, "value": value}));
            }
        }
        if !self.draft.date.trim().is_empty() {
            authorized.push(json!({"field": "成文日期", "value": self.draft.date}));
        }
        if !readable.minutes.printing_unit.trim().is_empty() {
            authorized.push(json!({
                "field": "版记印发机关",
                "value": readable.minutes.printing_unit
            }));
        }
        json!({
            "kind": self.draft.kind.label(),
            "title": self.draft.title_hint,
            "description": description,
            "authorized": authorized,
            "note": "表单已填的值可以直接使用；描述类空字段不要编造，可以问用户。授权类只读：不要编造、不要建议、不要出题、不要写入正文。",
        })
    }
}

struct DisabledKnowledge;

/// 表单里用户已填的描述类要素与标题：**不是**授权类，也不包含词库查询结果。
/// 事实比对同样要名称而不是身份引用，否则模型写的名称对不上、还可能照抄引用。
fn form_facts(draft: &DraftInput, vocabulary: &[crate::models::VocabularyEntry]) -> Vec<String> {
    let readable = crate::units::identity::readable(draft, vocabulary);
    let mut out = Vec::new();
    if !readable.title_hint.trim().is_empty() {
        out.push(readable.title_hint.clone());
    }
    for field in crate::element_fields::FieldId::ALL {
        if !field.applies(&readable) {
            continue;
        }
        let value = field.read(&readable);
        if !value.trim().is_empty() {
            out.push(value);
        }
    }
    out
}

impl PiKnowledge for DisabledKnowledge {
    fn enabled(&self) -> bool {
        false
    }

    fn search(&self, _query: &str, _top: usize) -> Result<Vec<PiChunk>, String> {
        Err("知识库未启用".into())
    }

    fn read(&self, _doc_id: i64) -> Result<Option<(String, String)>, String> {
        Err("知识库未启用".into())
    }
}

fn required_str(args: &Map<String, Value>, name: &str) -> Result<String, String> {
    let value = args
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("缺少参数 {name}"))?
        .trim()
        .to_string();
    if value.is_empty() {
        return Err(format!("参数 {name} 不能为空"));
    }
    if value.len() > 500 {
        return Err(format!("参数 {name} 太长"));
    }
    Ok(value)
}

fn int_arg(args: &Map<String, Value>, name: &str) -> Result<i64, String> {
    args.get(name)
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("参数 {name} 必须是正整数"))
}

fn usize_arg(args: &Map<String, Value>, name: &str, max: usize) -> usize {
    args.get(name)
        .and_then(Value::as_u64)
        .map(|value| (value as usize).clamp(1, max))
        .unwrap_or(max.min(5))
}

fn limit_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("\n\n【资料过长，只读到上限，其余内容请按标题分段查询】");
    out
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeKb {
        text: String,
    }

    impl PiKnowledge for FakeKb {
        fn enabled(&self) -> bool {
            true
        }

        fn search(&self, _query: &str, _top: usize) -> Result<Vec<PiChunk>, String> {
            Ok(vec![PiChunk {
                doc_id: 7,
                title: "森林防火规定".into(),
                section: "第三条".into(),
                text: self.text.clone(),
            }])
        }

        fn read(&self, doc_id: i64) -> Result<Option<(String, String)>, String> {
            Ok((doc_id == 7).then(|| ("森林防火规定".into(), self.text.clone())))
        }
    }

    struct FakeManuscripts;

    impl PiManuscripts for FakeManuscripts {
        fn search(&self, _query: &str, _limit: usize) -> Result<Vec<PiManuscriptRow>, String> {
            Ok(vec![PiManuscriptRow {
                id: 3,
                title: "去年防火通知".into(),
                kind: TemplateKind::OfficialLetter,
                status: "Published".into(),
                doc_number: "新政发〔2025〕3号".into(),
                doc_date: "2025-10-01".into(),
                version: None,
                markdown: String::new(),
            }])
        }

        fn read(&self, id: i64, _version: Option<i64>) -> Result<Option<PiManuscriptRow>, String> {
            Ok((id == 3).then(|| PiManuscriptRow {
                id: 3,
                title: "去年防火通知".into(),
                kind: TemplateKind::OfficialLetter,
                status: "Published".into(),
                doc_number: "新政发〔2025〕3号".into(),
                doc_date: "2025-10-01".into(),
                version: None,
                markdown: "# 通知\n\n去年10月9日开会。\n".into(),
            }))
        }
    }

    fn context(kb_text: &str) -> PiToolContext {
        PiToolContext::with_sources(
            Arc::new(AppConfig::default()),
            DraftInput::default(),
            "# 通知\n\n原文。\n".into(),
            "改一下。".into(),
            vec![],
            Arc::new(FakeKb {
                text: kb_text.into(),
            }),
            Arc::new(FakeManuscripts),
        )
    }

    #[test]
    fn user_answer_becomes_a_fact_source_for_checking() {
        let ctx = context("");
        let candidate = "# 通知\n\n请于2026年11月8日完成工作。";
        assert!(
            !ctx.call("check_document", &json!({"text":candidate}))
                .unwrap()["hard"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        ctx.confirm_answers(&["2026年11月8日".into()]);
        assert!(
            ctx.call("check_document", &json!({"text":candidate}))
                .unwrap()["hard"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            ctx.fact_sources()
                .iter()
                .any(|text| text == "2026年11月8日")
        );
    }

    #[test]
    fn kb_results_become_authorised_evidence_with_ids() {
        let ctx = context("各单位应于每年十月开展一次检查。");
        let value = ctx
            .call("kb_search", &json!({"query": "防火检查"}))
            .unwrap();
        let id = value["results"][0]["evidence"].as_str().unwrap();
        assert_eq!(id, "E1");
        let ledger = ctx.evidence();
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger[0].id, "E1");
        assert!(ledger[0].fact);
        assert_eq!(ledger[0].source, "知识库");
        assert!(
            ctx.fact_sources()
                .iter()
                .any(|text| text.contains("每年十月"))
        );
    }

    #[test]
    fn missing_references_fail_explicitly() {
        let ctx = context("x");
        assert!(
            ctx.read_reference(crate::ai_services::references::RefSource::Knowledge, 99)
                .is_err()
        );
        assert!(
            ctx.read_reference(crate::ai_services::references::RefSource::Manuscript, 99)
                .is_err()
        );
    }

    #[test]
    fn read_tools_keep_source_and_version_and_limit_results() {
        let ctx = context("x");
        let read = ctx.call("kb_read", &json!({"doc_id": 7})).unwrap();
        assert_eq!(read["title"], "森林防火规定");
        let ms = ctx.call("ms_read", &json!({"id": 3})).unwrap();
        assert!(ms["markdown"].as_str().unwrap().contains("去年10月9日"));
        assert!(ctx.call("kb_read", &json!({"doc_id": 9})).is_err());
        assert!(ctx.call("kb_read", &json!({"doc_id": -1})).is_err());
        assert!(ctx.call("kb_search", &json!({})).is_err());
        assert!(ctx.call("unknown", &json!({})).is_err());
    }

    #[test]
    fn check_document_rejects_untraced_facts_and_accepts_evidence() {
        let ctx = context("各单位应于每年十月开展一次检查。");
        let bad = ctx
            .call(
                "check_document",
                &json!({"text": "# 通知\n\n请于2026年10月8日报送。\n"}),
            )
            .unwrap();
        assert!(!bad["hard"].as_array().unwrap().is_empty());
        let good = ctx
            .call(
                "check_document",
                &json!({"text": "# 通知\n\n请各单位于每年十月开展一次检查。\n"}),
            )
            .unwrap();
        assert!(good["hard"].as_array().unwrap().is_empty(), "{good}");
    }

    #[test]
    fn doc_elements_is_read_only_and_separates_authorized_fields() {
        let ctx = context("x");
        let value = ctx.call("doc_elements", &json!({})).unwrap();
        assert_eq!(value["kind"], "公函");
        let authorized = value["authorized"].as_array().unwrap();
        assert!(
            authorized
                .iter()
                .any(|item| item["field"] == "密级" && item["value"] == "机密")
        );
        let note = value["note"].as_str().unwrap();
        assert!(note.contains("只读"));
        assert!(note.contains("不要出题"));
    }

    #[test]
    fn vocab_search_returns_standard_entries_only() {
        let mut config = AppConfig::default();
        config.vocabulary.push(crate::models::VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "市应急管理局".into(),
            ..Default::default()
        });
        let ctx = PiToolContext::with_sources(
            Arc::new(config),
            DraftInput::default(),
            String::new(),
            String::new(),
            vec![],
            Arc::new(FakeKb { text: "x".into() }),
            Arc::new(FakeManuscripts),
        );
        let value = ctx
            .call(
                "vocab_search",
                &json!({"query": "应急", "category": "unit"}),
            )
            .unwrap();
        assert_eq!(value["results"][0]["name"], "市应急管理局");
        assert!(ctx.evidence().is_empty(), "词库查询不产生事实证据");
        assert!(
            ctx.fact_sources().is_empty(),
            "词库证明规范名称存在，不证明该单位参与本次事项"
        );
    }
}
