//! Pi 只读领域工具与事实台账。
//!
//! host 发 `tool_request`，Rust 在这里校验工具名与参数并执行，再把结果按调用 ID 回去。
//! 这里**不**复用旧的 `Board` / `ToolCtx` / 流程引擎：只调用现有的知识库检索、
//! 稿件库、词库与确定性检查。每个真正返回给模型的资料都登记一条证据（编号、来源、
//! 哈希、片段、角色），用于回查和恢复；不据此判断正文事实真伪。
//!
//! 工具在独立线程执行，界面一停 host 就被回收，但已发出的 HTTP / 检索 / LLM 重排无法
//! 从中途 abort：这一层只做取消边界（阶段之间尽快收手、迟到结果不写证据、不算成功），
//! 并用在途槽位（`ToolSlot`）限制同时最多几个慢工具，避免反复停止重开无限叠加资源。
//! 不要把这里当成「已经强杀请求」——请求本身仍会跑完，只是结果被丢弃。

use crate::agent::api::{ApiEndpoint, ApiSecrets, ApiStore};
use crate::agent::apidef::tooling;
use crate::ai_services::knowledge::{KnowledgeSearch, RagSearch};
use crate::ai_services::manuscripts::ManuscriptSource;
use crate::ai_services::style::{self, StyleBook};
use crate::manuscript::ManuscriptFilter;
use crate::models::{AppConfig, DraftInput, TemplateKind};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 一次工具调用最多返回多少资料。
const MAX_SEARCH_RESULTS: usize = 10;
/// 读全文的字符上限；超限时按 offset 分页继续，不静默截断。
const MAX_READ_CHARS: usize = 160_000;
/// 一条证据交给模型的片段上限。
const MAX_SNIPPET_CHARS: usize = 600;
/// `api_search` 一次最多返回几个接口。
const MAX_API_RESULTS: usize = 5;
/// 维基工具独立小额度（设计 §6.2）：一轮写作作业里四只维基工具合计 12 次，
/// 防语义下钻遍历挤占写作工具预算。
pub(super) const WIKI_TOOL_BUDGET: usize = 12;
/// `api_call` 一次交给模型的条目上限（接口返回可能很多，先给前几条）。
const MAX_API_ITEMS: usize = 20;
/// 每个接口条目交给模型的字符上限；证据台账同样截到这里，防超长返回撑爆预算。
/// 完整内容留在本轮结果缓存里，模型可用 `api_read` 按位置续读。
const MAX_API_ITEM_CHARS: usize = 2_000;
/// 本轮接口结果缓存：最多保留几份、总字节上限（内存与落盘各自按同一上限裁剪）。
const MAX_CACHED_API_RESULTS: usize = 8;
const MAX_CACHED_API_BYTES: usize = 2 * 1024 * 1024;
/// 落盘目录名（运行目录下）。恢复时从这里读回，避免重新调用接口。
const API_CACHE_DIR: &str = "api-results";
/// 一次最多列几份风格档案。
const MAX_STYLE_ROWS: usize = 20;
/// 读一份风格时交给模型的 token 预算与字符上限。
const MAX_STYLE_TOKENS: usize = 1_200;
const MAX_STYLE_CHARS: usize = 8_000;
/// 风格结果固定带的一句说明：写法不是事实。
const STYLE_NOTE: &str = "风格档案与范例是写法样稿（style_sample），只给结构与表达；不是本次事项的事实来源，\
其中的单位、人员、日期、数字与结论不得写进正文；这个结果不登记为事实证据。";

/// 同时在途的域工具上限。界面停止后旧工具可能还在收尾（HTTP 请求本身无法中途 abort），
/// 反复停止重开不能无限叠加线程、连接与模型额度，这里做资源兜底。
const MAX_IN_FLIGHT_TOOLS: usize = 4;
/// 槽位满了最多等多久：等不到就明确回错，而不是让本轮工具无限排队。
const TOOL_SLOT_WAIT: Duration = Duration::from_secs(10);
/// 等槽位时多久看一次取消信号；停止后排队线程要尽快退出，不能陪旧任务耗满 10 秒。
const TOOL_SLOT_POLL: Duration = Duration::from_millis(20);
/// 工具在途计数：`ToolSlot` 用 CAS 申请、Drop 释放。
static IN_FLIGHT_TOOLS: AtomicUsize = AtomicUsize::new(0);
/// 取消时统一回给模型 / 宿主的文案；只说结果不采用，不暗示底层请求已被强杀。
const CANCELLED_TOOL: &str = "已停止 Pi 智能写作，本次工具结果不采用。";

/// 知识库检索结果的窄接口（生产用现有 RAG，测试可换假库）。
pub(crate) trait PiKnowledge: Send + Sync {
    fn enabled(&self) -> bool;
    fn search(&self, query: &str, top: usize) -> Result<PiSearchOutcome, String>;
    fn read(&self, doc_id: i64) -> Result<Option<(String, String)>, String>;
}

/// 维基检索命中：标题 + 所在分类路径 + 摘要（路径与摘要帮助模型判断要不要读全文）。
#[derive(Debug, Clone)]
pub(crate) struct PiWikiHit {
    pub(crate) page_id: String,
    pub(crate) title: String,
    pub(crate) path: String,
    pub(crate) snippet: String,
}

/// 出链候选：显示文字 + 目标标题 + 是否已建页 + 一句话摘要。
/// 只是下钻判断的候选方向，不是必读清单（设计 §6.1）。
#[derive(Debug, Clone)]
pub(crate) struct PiWikiLinkInfo {
    pub(crate) text: String,
    pub(crate) to_title: String,
    pub(crate) to_page: Option<String>,
    pub(crate) resolved: bool,
    pub(crate) summary: String,
}

/// 维基知识库只读来源（生产每轮作业独立连接，测试可换假库）。
pub(crate) trait PiWiki: Send + Sync {
    fn enabled(&self) -> bool;
    fn search(&self, query: &str, top: usize) -> Result<Vec<PiWikiHit>, String>;
    /// 读页面全文：(标题, 正文)。
    fn read(&self, uuid: &str) -> Result<Option<(String, String)>, String>;
    fn links(&self, uuid: &str) -> Result<Vec<PiWikiLinkInfo>, String>;
    /// 该页当前版本的依据清单：(资料库 doc_id, 支撑说明)。
    fn doc_refs(&self, uuid: &str) -> Result<Vec<(i64, String)>, String>;
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PiSearchOutcome {
    pub(crate) chunks: Vec<PiChunk>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PiChunk {
    pub(crate) doc_id: i64,
    pub(crate) title: String,
    pub(crate) section: String,
    pub(crate) text: String,
}

/// 一份可用风格档案的摘要（列表用）。
#[derive(Debug, Clone)]
pub(crate) struct PiStyleRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) note: String,
    pub(crate) kinds: Vec<String>,
    pub(crate) occasions: Vec<String>,
    pub(crate) examples: usize,
    pub(crate) description_chars: usize,
}

/// 按 id 读一份风格的结果。找不到 / 停用 / 不适用都明确返回，不偷偷换成别的。
#[derive(Debug, Clone)]
pub(crate) enum PiStyleLookup {
    Found {
        id: String,
        name: String,
        text: String,
    },
    Unavailable {
        reason: &'static str,
        message: String,
    },
}

/// 风格列表：最多 `MAX_STYLE_ROWS` 份，多出来的明确标记，不默默截断。
#[derive(Debug, Clone)]
pub(crate) struct PiStyleList {
    pub(crate) rows: Vec<PiStyleRow>,
    pub(crate) truncated: bool,
}

/// 风格档案的窄接口（生产用 `ai_services::style`，测试注入假数据；不写库、不加使用次数）。
pub(crate) trait PiStyles: Send + Sync {
    fn list(&self, kind: TemplateKind) -> Result<PiStyleList, String>;
    fn read(&self, id: &str, kind: TemplateKind) -> Result<PiStyleLookup, String>;
}

/// 只读资料来源；生产与测试共用明确的注入边界。
pub(crate) struct PiSources {
    pub(crate) knowledge: Arc<dyn PiKnowledge>,
    pub(crate) manuscripts: Arc<dyn PiManuscripts>,
    pub(crate) styles: Arc<dyn PiStyles>,
}

/// 生产实现：只读 `styles.json`，不调其它模型、不保存、不记使用次数。
struct StyleArchives;

impl PiStyles for StyleArchives {
    fn list(&self, kind: TemplateKind) -> Result<PiStyleList, String> {
        let book = StyleBook::load().map_err(|error| format!("读不到风格档案：{error:#}"))?;
        let applicable = book
            .styles
            .iter()
            .filter(|profile| style_applies(profile, kind))
            .collect::<Vec<_>>();
        let truncated = applicable.len() > MAX_STYLE_ROWS;
        let rows = applicable
            .into_iter()
            .take(MAX_STYLE_ROWS)
            .map(|profile| PiStyleRow {
                id: profile.id.clone(),
                name: profile.name.clone(),
                note: profile.note.clone(),
                kinds: profile
                    .kinds
                    .iter()
                    .map(|kind| kind.label().to_string())
                    .collect(),
                occasions: profile.occasions.clone(),
                examples: profile.examples.len(),
                description_chars: profile.description.chars().count(),
            })
            .collect();
        Ok(PiStyleList { rows, truncated })
    }

    fn read(&self, id: &str, kind: TemplateKind) -> Result<PiStyleLookup, String> {
        let book = StyleBook::load().map_err(|error| format!("读不到风格档案：{error:#}"))?;
        let Some(profile) = book.get(id) else {
            return Ok(PiStyleLookup::Unavailable {
                reason: "unknown",
                message: format!(
                    "没有编号为「{id}」的风格档案。用 style_list 看当前文种可用的档案，不要自己换另一份。"
                ),
            });
        };
        if !profile.enabled {
            return Ok(PiStyleLookup::Unavailable {
                reason: "disabled",
                message: format!(
                    "风格「{}」已停用，不能使用。需要时请用户到「AI 管理 → 风格」里启用。",
                    profile.name
                ),
            });
        }
        if !style_applies(profile, kind) {
            return Ok(PiStyleLookup::Unavailable {
                reason: "not_applicable",
                message: format!(
                    "风格「{}」不适用于当前文种（{}）。用 style_list 看当前文种的档案。",
                    profile.name,
                    kind.label()
                ),
            });
        }
        let text = style_excerpt(&style::render(profile, MAX_STYLE_TOKENS), MAX_STYLE_CHARS);
        Ok(PiStyleLookup::Found {
            id: profile.id.clone(),
            name: profile.name.clone(),
            text,
        })
    }
}

fn style_applies(profile: &crate::ai_services::style::StyleProfile, kind: TemplateKind) -> bool {
    profile.enabled && (profile.kinds.is_empty() || profile.kinds.contains(&kind))
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

    fn search(&self, query: &str, top: usize) -> Result<PiSearchOutcome, String> {
        let (chunks, warnings) = KnowledgeSearch::search(self, query)
            .map_err(|error| format!("知识库检索失败：{error:#}"))?;
        Ok(PiSearchOutcome {
            chunks: chunks
                .into_iter()
                .take(top)
                .map(|chunk| PiChunk {
                    doc_id: chunk.doc_id,
                    title: chunk.doc_title,
                    section: chunk.section,
                    text: chunk.text,
                })
                .collect(),
            warnings,
        })
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

/// 内网数据接口的窄接口：生产从配置目录读 OpenAPI 与密钥，测试可换假数据。
pub(crate) trait PiApis: Send + Sync {
    /// 按用途搜「只查询且开放给 AI」的接口，返回接口 id、说明与参数签名。
    fn search(&self, query: &str, limit: usize) -> Result<Vec<PiApiHit>, String>;
    /// 调用一个接口；参数先按 schema 校验，不合格不发请求。
    fn call(&self, api: &str, args: &Map<String, Value>) -> Result<PiApiCall, String>;
}

/// 搜到的一个接口。
#[derive(Debug, Clone)]
pub(crate) struct PiApiHit {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) signature: String,
}

/// 接口返回的一条资料。
#[derive(Debug, Clone)]
pub(crate) struct PiApiItem {
    pub(crate) title: String,
    pub(crate) source: String,
    pub(crate) text: String,
}

/// 一次接口调用的结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct PiApiCall {
    pub(crate) name: String,
    pub(crate) items: Vec<PiApiItem>,
    pub(crate) total: usize,
}

/// 生产实现：只暴露只查询、开放给 AI、配置没问题的接口（红线不放宽）。
struct ApiCatalog;

impl PiApis for ApiCatalog {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<PiApiHit>, String> {
        let store = ApiStore::load().map_err(|error| format!("读数据接口配置失败：{error:#}"))?;
        Ok(search_apis(&store, query, limit))
    }

    fn call(&self, api: &str, args: &Map<String, Value>) -> Result<PiApiCall, String> {
        let store = ApiStore::load().map_err(|error| format!("读数据接口配置失败：{error:#}"))?;
        let endpoint = store.get(api).ok_or_else(|| {
            format!("没有配置 id 为「{api}」的数据接口：先用 api_search 搜接口 id")
        })?;
        if !endpoint.readonly {
            return Err(format!("接口「{}」会改数据，不给 AI 调", endpoint.name));
        }
        if !endpoint.ai {
            return Err(format!("接口「{}」没有开放给 AI", endpoint.name));
        }
        if !endpoint.unsupported.trim().is_empty() {
            return Err(format!(
                "接口「{}」当前配置不受支持：{}",
                endpoint.name,
                endpoint.unsupported.trim()
            ));
        }
        tooling::validate(endpoint, args)?;
        let secrets =
            ApiSecrets::load().map_err(|error| format!("读数据接口密钥失败：{error:#}"))?;
        let called = crate::agent::api::call(endpoint, args, &secrets)?;
        Ok(PiApiCall {
            name: endpoint.name.clone(),
            total: called.total,
            items: called
                .items
                .into_iter()
                .map(|item| PiApiItem {
                    title: if item.title.is_empty() {
                        endpoint.name.clone()
                    } else {
                        item.title
                    },
                    source: item.source,
                    text: item.text,
                })
                .collect(),
        })
    }
}

/// 搜接口的纯函数，测试直接传内存里的 `ApiStore`。
fn search_apis(store: &ApiStore, query: &str, limit: usize) -> Vec<PiApiHit> {
    let words = tooling::query_words(query);
    let mut scored: Vec<(usize, &ApiEndpoint)> = store
        .endpoints
        .iter()
        .filter(|endpoint| tooling::usable(endpoint))
        .map(|endpoint| {
            let (name, rest) = tooling::search_text(endpoint);
            (tooling::match_score(&words, &name, &rest), endpoint)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, endpoint)| PiApiHit {
            id: endpoint.id.clone(),
            name: endpoint.name.clone(),
            description: endpoint.description.trim().to_string(),
            signature: tooling::signatures(endpoint),
        })
        .collect()
}

/// 一份可续读的接口结果：完整条目留在缓存里，模型按 `result_id` 分页读取。
#[derive(Debug, Clone)]
struct CachedApiResult {
    id: String,
    api: String,
    name: String,
    total: usize,
    items: Vec<PiApiItem>,
}

impl CachedApiResult {
    fn bytes(&self) -> usize {
        self.items
            .iter()
            .map(|item| item.title.len() + item.source.len() + item.text.len())
            .sum()
    }

    fn json(&self) -> Value {
        json!({
            "version": 1,
            "id": self.id,
            "api": self.api,
            "name": self.name,
            "total": self.total,
            "items": self.items.iter().map(|item| json!({
                "title": item.title,
                "source": item.source,
                "text": item.text,
            })).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: Value) -> Option<Self> {
        let items = value.get("items")?.as_array()?;
        let mut parsed = Vec::new();
        for item in items {
            parsed.push(PiApiItem {
                title: item.get("title")?.as_str()?.to_string(),
                source: item
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                text: item.get("text")?.as_str()?.to_string(),
            });
        }
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            api: value.get("api")?.as_str()?.to_string(),
            name: value.get("name")?.as_str()?.to_string(),
            total: value
                .get("total")
                .and_then(Value::as_u64)
                .unwrap_or(parsed.len() as u64) as usize,
            items: parsed,
        })
    }
}

/// 本轮接口结果缓存：内存有上限；挂了运行目录时同时落盘，恢复后仍能续读。
#[derive(Default)]
struct ApiResultCache {
    dir: Option<PathBuf>,
    results: VecDeque<CachedApiResult>,
}

impl ApiResultCache {
    fn set_dir(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
    }

    fn store(&mut self, mut result: CachedApiResult) -> String {
        if result.items.is_empty() {
            return String::new();
        }
        result.id = format!(
            "api-{}",
            &sha256_hex(&serde_json::to_string(&result.json()).unwrap_or_default())[..16]
        );
        let id = result.id.clone();
        if let Some(dir) = &self.dir {
            // 落盘失败不影响本轮：内存里还能读；恢复时才需要它。
            let path = dir.join(API_CACHE_DIR);
            if std::fs::create_dir_all(&path).is_ok() {
                let file = path.join(format!("{id}.json"));
                let _ = crate::vault::write_path(
                    &file,
                    "pi",
                    &serde_json::to_vec(&result.json()).unwrap_or_default(),
                );
            }
            prune_api_cache_dir(&path);
        }
        self.results.retain(|cached| cached.id != id);
        self.results.push_back(result);
        while self.results.len() > MAX_CACHED_API_RESULTS
            || self.total_bytes() > MAX_CACHED_API_BYTES
        {
            if self.results.len() <= 1 {
                break;
            }
            self.results.pop_front();
        }
        id
    }

    fn total_bytes(&self) -> usize {
        self.results.iter().map(CachedApiResult::bytes).sum()
    }

    fn load(&mut self, id: &str) -> Option<CachedApiResult> {
        if !valid_api_result_id(id) {
            return None;
        }
        if let Some(found) = self.results.iter().find(|cached| cached.id == id) {
            return Some(found.clone());
        }
        let dir = self.dir.as_ref()?;
        let raw = crate::vault::read_path(
            &dir.join(API_CACHE_DIR).join(format!("{id}.json")),
            "pi",
            MAX_CACHED_API_BYTES as u64,
        )
        .ok()?;
        let value: Value = serde_json::from_slice(&raw).ok()?;
        let result = CachedApiResult::from_json(value)?;
        if result.id != id {
            return None;
        }
        Some(result)
    }
}

fn valid_api_result_id(id: &str) -> bool {
    id.strip_prefix("api-")
        .is_some_and(|hash| hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// 落盘缓存按文件修改时间裁剪到总字节上限，避免长任务把磁盘撑大。
fn prune_api_cache_dir(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| {
                (
                    meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                    meta.len(),
                    entry.path(),
                )
            })
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, bytes, _)| bytes).sum();
    if total <= MAX_CACHED_API_BYTES as u64 {
        return;
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, bytes, path) in files {
        if total <= MAX_CACHED_API_BYTES as u64 {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(bytes);
        }
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
    /// 维基知识库只读来源：默认 DisabledWiki，只有写作作业显式挂入（口径 7.3）。
    wiki: Arc<dyn PiWiki>,
    /// 维基工具本轮已用次数（独立小额度，防语义下钻遍历挤占写作预算）。
    wiki_used: Arc<AtomicUsize>,
    apis: Arc<dyn PiApis>,
    api_cache: Mutex<ApiResultCache>,
    manuscripts: Arc<dyn PiManuscripts>,
    styles: Arc<dyn PiStyles>,
    evidence: Arc<Mutex<Vec<PiEvidence>>>,
    seq: Arc<AtomicUsize>,
    /// 界面「停止」信号，`attach_cancel` 只挂一次。工具在独立线程执行，HTTP 请求本身
    /// 无法中途 abort，这里只按取消边界处理：阶段之间尽快收手，迟到结果不写证据、
    /// 不算成功；资源叠加由 `ToolSlot` 兜底。
    cancel: OnceLock<Arc<AtomicBool>>,
    /// 任务助手作业：只开放只读任务工具与词库/日期，拒绝知识库、稿件库、接口与正文工具。
    task_only: bool,
    wiki_job: bool,
    wiki_doc_hits: Mutex<std::collections::HashSet<i64>>,
    /// 任务数据库路径；只有任务作业才有。
    task_db: Option<std::path::PathBuf>,
}

/// 一个在途工具槽位。停止后的慢工具要跑完当前请求才会返回，反复停止重开时用槽位
/// 限制同时最多 `MAX_IN_FLIGHT_TOOLS` 个，避免线程、连接与模型额度无限叠加。
#[derive(Debug)]
struct ToolSlot;

impl ToolSlot {
    /// 申请一个槽位；满了最多等 `TOOL_SLOT_WAIT`，等待期间定期检查取消。
    fn acquire(ctx: &PiToolContext) -> Result<Self, String> {
        Self::acquire_within(ctx, TOOL_SLOT_WAIT)
    }

    /// 带等待上限的申请（测试用零等待验证「满了就明确失败」）。
    fn acquire_within(ctx: &PiToolContext, wait: Duration) -> Result<Self, String> {
        let started = Instant::now();
        loop {
            // CAS 申请：先看再加会并发越过上限。
            let mut current = IN_FLIGHT_TOOLS.load(Ordering::Acquire);
            while current < MAX_IN_FLIGHT_TOOLS {
                match IN_FLIGHT_TOOLS.compare_exchange_weak(
                    current,
                    current + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return Ok(ToolSlot),
                    Err(updated) => current = updated,
                }
            }
            if wait.is_zero() || started.elapsed() >= wait {
                return Err(format!(
                    "上一个已停止的任务仍在收尾（域工具最多同时 {MAX_IN_FLIGHT_TOOLS} 个），\
                     本次工具未执行；稍后再试或停止本轮。"
                ));
            }
            // 排队期间用户可能又点了停止：先承认取消，别让这一轮的工具继续排队。
            ctx.check_cancel()?;
            std::thread::sleep(TOOL_SLOT_POLL);
        }
    }
}

impl Drop for ToolSlot {
    fn drop(&mut self) {
        IN_FLIGHT_TOOLS.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
fn in_flight_tools() -> usize {
    IN_FLIGHT_TOOLS.load(Ordering::Acquire)
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
        // 检索里的对话模型只用于 `rerank.mode=Llm` 的 LLM 重排，统一用写作模型：
        // 不再隐式借用「辅助模型」或文字复核的小模型（2026-11-05 模型通道收敛）。
        let kb: Arc<dyn PiKnowledge> = match config.pi_chat() {
            Ok(chat) => Arc::new(RagSearch {
                enabled: config.rag.enabled,
                rag: config.resolved_rag(),
                chat,
                kind_filter: None,
            }),
            Err(_) => Arc::new(DisabledKnowledge),
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

    /// 测试替身入口。风格工具默认读本机档案；需要隔离时用 `with_all_sources` 注入。
    pub(crate) fn with_sources(
        config: Arc<AppConfig>,
        draft: DraftInput,
        current: String,
        request: String,
        material_facts: Vec<String>,
        kb: Arc<dyn PiKnowledge>,
        manuscripts: Arc<dyn PiManuscripts>,
    ) -> Self {
        Self::with_all_sources(
            config,
            draft,
            current,
            request,
            material_facts,
            PiSources {
                knowledge: kb,
                manuscripts,
                styles: Arc::new(StyleArchives),
            },
        )
    }

    /// 全部资料源的注入入口（风格显式给，测试不读真实风格库）。
    pub(crate) fn with_all_sources(
        config: Arc<AppConfig>,
        draft: DraftInput,
        current: String,
        request: String,
        material_facts: Vec<String>,
        sources: PiSources,
    ) -> Self {
        Self {
            form_facts: form_facts(&draft, &config.vocabulary),
            config,
            draft,
            current,
            request,
            material_facts,
            kb: sources.knowledge,
            wiki: Arc::new(DisabledWiki),
            wiki_used: Arc::new(AtomicUsize::new(0)),
            apis: Arc::new(ApiCatalog),
            api_cache: Mutex::new(ApiResultCache::default()),
            manuscripts: sources.manuscripts,
            styles: sources.styles,
            evidence: Arc::new(Mutex::new(Vec::new())),
            seq: Arc::new(AtomicUsize::new(0)),
            task_only: false,
            wiki_job: false,
            wiki_doc_hits: Mutex::new(std::collections::HashSet::new()),
            task_db: None,
            cancel: OnceLock::new(),
        }
    }

    /// 测试替身：换掉内网数据接口来源，不碰真实配置目录。
    #[cfg(test)]
    pub(crate) fn with_apis(mut self, apis: Arc<dyn PiApis>) -> Self {
        self.apis = apis;
        self
    }

    /// 测试替身：换掉维基来源，不碰真实稿件库。
    #[cfg(test)]
    pub(crate) fn with_wiki(mut self, wiki: Arc<dyn PiWiki>) -> Self {
        self.wiki = wiki;
        self
    }

    /// 给本轮挂上接口结果缓存目录（运行目录）。恢复后仍能按 `result_id` 续读。
    pub(crate) fn with_api_cache_dir(self, dir: PathBuf) -> Self {
        if let Ok(mut cache) = self.api_cache.lock() {
            cache.set_dir(dir);
        }
        self
    }

    /// 任务助手作业的工具面：只留任务检索、词库与日期，其余工具一律拒绝。
    ///
    /// 任务材料已经在工作区里，不需要也不允许读知识库、稿件库或内网接口，
    /// 也不碰公文正文与表单要素。
    pub(crate) fn task_only(mut self, db: std::path::PathBuf) -> Self {
        self.task_only = true;
        self.task_db = Some(db);
        self
    }

    pub(super) fn wiki_job(mut self) -> Self {
        self.wiki_job = true;
        self
    }

    pub(super) fn wiki_doc_hits(&self) -> std::collections::HashSet<i64> {
        self.wiki_doc_hits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 写作作业挂入维基来源。库路径不可用时按未接入处理，工具给出明确指引。
    pub(crate) fn with_wiki_autoload(mut self) -> Self {
        self.wiki = match crate::storage::manuscript_db_path() {
            Ok(path) => Arc::new(WikiArchives { db: path }),
            Err(_) => Arc::new(DisabledWiki),
        };
        self
    }

    /// 挂上本轮停止信号。只设置一次：同一上下文只认第一个信号，
    /// 重复挂说明调用方接错了（覆盖会让已在排队的工具看不到真正的停止）。
    pub(crate) fn attach_cancel(&self, cancel: Arc<AtomicBool>) {
        let _ = self.cancel.set(cancel);
    }

    /// 本轮是否已被停止。
    fn cancel_requested(&self) -> bool {
        self.cancel
            .get()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// 取消边界：统一返回可读的停止错误，供工具各阶段边界复检。
    fn check_cancel(&self) -> Result<(), String> {
        if self.cancel_requested() {
            Err(CANCELLED_TOOL.to_string())
        } else {
            Ok(())
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

    pub(super) fn is_wiki_tool(name: &str) -> bool {
        matches!(
            name,
            "wiki_search" | "wiki_read" | "wiki_links" | "wiki_doc_read"
        )
    }

    /// 允许的工具名（协议层先按它拒绝未知工具）。
    pub(crate) fn known(name: &str) -> bool {
        matches!(
            name,
            "kb_search"
                | "kb_read"
                | "wiki_search"
                | "wiki_read"
                | "wiki_links"
                | "wiki_doc_read"
                | "ms_search"
                | "ms_read"
                | "api_search"
                | "api_call"
                | "api_read"
                | "vocab_search"
                | "current_date"
                | "check_document"
                | "doc_elements"
                | "style_list"
                | "style_read"
                | "task_search"
                | "task_read"
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
        // 入口先看停止：停止后 host 可能还排着旧工具请求，检索 / 模型 / 网络一律不再发起。
        self.check_cancel()?;
        let args = args
            .as_object()
            .ok_or_else(|| "工具参数必须是 JSON 对象".to_string())?;
        if self.task_only
            && !matches!(
                name,
                "task_search" | "task_read" | "vocab_search" | "current_date"
            )
            // 任务作业挂了维基库时，开放四个只读维基工具（三期：任务助手接入）。
            && !(self.wiki.enabled() && Self::is_wiki_tool(name))
        {
            return Err(format!("任务助手作业不开放工具：{name}"));
        }
        if self.wiki_job
            && !matches!(name, "kb_search" | "kb_read" | "current_date")
            && !(self.wiki.enabled() && Self::is_wiki_tool(name))
        {
            return Err(format!("维基制作作业不开放工具：{name}"));
        }
        // 在途槽位是资源兜底：HTTP 请求无法从外部 abort，停掉的旧任务仍可能占着线程、
        // 连接和模型额度；限流后反复停止重开不会无限叠加。
        let _slot = ToolSlot::acquire(self)?;
        let result = match name {
            "kb_search" => self.kb_search(args),
            "kb_read" => self.kb_read(args),
            "wiki_search" => self.wiki_search(args),
            "wiki_read" => self.wiki_read(args),
            "wiki_links" => self.wiki_links(args),
            "wiki_doc_read" => self.wiki_doc_read(args),
            "ms_search" => self.ms_search(args),
            "ms_read" => self.ms_read(args),
            "api_search" => self.api_search(args),
            "api_call" => self.api_call(args),
            "api_read" => self.api_read(args),
            "vocab_search" => self.vocab_search(args),
            "current_date" => Ok(super::writing_context::date_context()),
            "check_document" => self.check_document(args),
            "doc_elements" => Ok(self.doc_elements()),
            "style_list" => self.style_list(),
            "style_read" => self.style_read(args),
            "task_search" => self.task_search(args),
            "task_read" => self.task_read(args),
            other => Err(format!("未知工具：{other}")),
        };
        // dispatch 返回后再看一次：慢工具可能在停止之后才返回，即使成功也不能当成
        // 这一轮的工具结果交回去（迟到结果不采用）。
        self.check_cancel()?;
        if self.wiki_job
            && let Ok(value) = &result
        {
            let mut hits = self
                .wiki_doc_hits
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match name {
                "kb_search" => {
                    if let Some(results) = value["results"].as_array() {
                        hits.extend(results.iter().filter_map(|row| row["doc_id"].as_i64()));
                    }
                }
                "kb_read" | "wiki_doc_read" => hits.extend(value["doc_id"].as_i64()),
                _ => {}
            }
        }
        result
    }

    fn kb_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        if !self.kb.enabled() {
            return Err("知识库未启用：请到「知识库」页启用并配置检索后再试。".into());
        }
        let query = required_str(args, "query")?;
        let top = usize_arg(args, "top", MAX_SEARCH_RESULTS);
        // 检索里可能带 embedding 与 LLM 重排，慢在这一步；调用前后各看一眼停止信号。
        self.check_cancel()?;
        let outcome = self.kb.search(&query, top)?;
        self.check_cancel()?;
        let warnings = outcome.warnings;
        if outcome.chunks.is_empty() {
            return Ok(json!({
                "results": [],
                "zero_hits": true,
                "note": "知识库没有找到相关片段。可缩短查询词或换用材料中的关键术语后重试。",
                "warnings": warnings,
            }));
        }
        let mut results = Vec::new();
        for chunk in outcome.chunks {
            // 逐条登记前复检：停止后不往证据台账里写资料。
            self.check_cancel()?;
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
        Ok(json!({"results": results, "zero_hits": false, "warnings": warnings}))
    }

    fn kb_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let doc_id = int_arg(args, "doc_id")?;
        // 读全文可能要从库 / 索引取大段内容；调用前后各查一次取消。
        self.check_cancel()?;
        let Some((title, full)) = self.kb.read(doc_id)? else {
            return Err(format!("知识库没有编号为 {doc_id} 的文档"));
        };
        self.check_cancel()?;
        let offset = optional_offset_arg(args);
        let page = read_page(&full, offset, read_limit_arg(args))?;
        let id = self.record(
            "kb_read",
            &title,
            "知识库",
            &page.location,
            &page.text,
            true,
        );
        Ok(json!({
            "evidence": id,
            "doc_id": doc_id,
            "title": title,
            "text": page.text,
            "offset": page.offset,
            "total_chars": page.total_chars,
            "next_offset": page.next_offset,
            "unread_chars": page.unread_chars,
            "truncated": page.next_offset.is_some(),
            "note": READ_PAGE_NOTE,
        }))
    }

    /// 维基工具本轮额度（设计 §6.2：独立小额度，防遍历挤占写作预算）。
    fn wiki_budget(&self) -> Result<(), String> {
        let used = self.wiki_used.fetch_add(1, Ordering::Relaxed);
        if used >= WIKI_TOOL_BUDGET {
            return Err(format!(
                "维基工具本轮额度已用完（{WIKI_TOOL_BUDGET} 次）。请基于已读到的维基内容写作；\
                 还不够就回落 kb_search 检索资料库，不要再调维基工具。"
            ));
        }
        Ok(())
    }

    /// 未挂维基来源时的统一指引（任务/风格作业、或库路径不可用）。
    fn wiki_enabled(&self) -> Result<(), String> {
        if self.wiki.enabled() {
            return Ok(());
        }
        Err("维基知识库未接入本轮作业，或库打开失败。请改用 kb_search 检索资料库。".into())
    }

    /// 语义检索维基页面（写作取料第一站）：标题 + 分类路径 + 摘要，逐条登记证据。
    fn wiki_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        self.wiki_budget()?;
        self.wiki_enabled()?;
        let query = required_str(args, "query")?;
        let top = usize_arg(args, "top", MAX_SEARCH_RESULTS);
        self.check_cancel()?;
        let hits = self.wiki.search(&query, top)?;
        self.check_cancel()?;
        if hits.is_empty() {
            return Ok(json!({
                "results": [],
                "zero_hits": true,
                "note": "维基没有找到相关页面。可换用材料里的关键术语再试，或回落 kb_search 检索资料库。",
            }));
        }
        let mut results = Vec::new();
        for hit in hits {
            // 逐条登记前复检：停止后不往证据台账里写资料。
            self.check_cancel()?;
            let id = self.record(
                "wiki_search",
                &hit.title,
                "维基",
                &hit.path,
                &hit.snippet,
                true,
            );
            results.push(json!({
                "evidence": id,
                "page_id": hit.page_id,
                "title": hit.title,
                "path": hit.path,
                "snippet": hit.snippet,
            }));
        }
        Ok(json!({
            "results": results,
            "zero_hits": false,
            "note": "维基页是已整编的知识综述。信息不够细时用 wiki_read 读全文，再按其依据清单用 wiki_doc_read 溯源原文。",
        }))
    }

    /// 读维基页面全文（按 offset 分页，与 kb_read 同规矩）。
    fn wiki_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        self.wiki_budget()?;
        self.wiki_enabled()?;
        let uuid = required_str(args, "page_id")?;
        self.check_cancel()?;
        let Some((title, full)) = self.wiki.read(&uuid)? else {
            return Err(format!("维基里没有这个页面（page_id={uuid}）"));
        };
        self.check_cancel()?;
        let refs = self.wiki.doc_refs(&uuid)?;
        let offset = optional_offset_arg(args);
        let page = read_page(&full, offset, read_limit_arg(args))?;
        let id = self.record(
            "wiki_read",
            &title,
            "维基",
            &page.location,
            &page.text,
            true,
        );
        Ok(json!({
            "evidence": id,
            "page_id": uuid,
            "doc_refs": refs.iter().map(|(id, note)| json!({"doc_id": id, "note": note})).collect::<Vec<_>>(),
            "title": title,
            "text": page.text,
            "offset": page.offset,
            "total_chars": page.total_chars,
            "next_offset": page.next_offset,
            "unread_chars": page.unread_chars,
            "truncated": page.next_offset.is_some(),
            "note": READ_PAGE_NOTE,
        }))
    }

    /// 列出页面的出链候选（标题 + 一句话摘要），供语义下钻判断。
    /// 候选方向不登记为事实证据——读没读、怎么用由模型结合写作任务决定。
    fn wiki_links(&self, args: &Map<String, Value>) -> Result<Value, String> {
        self.wiki_budget()?;
        self.wiki_enabled()?;
        let uuid = required_str(args, "page_id")?;
        self.check_cancel()?;
        let links = self.wiki.links(&uuid)?;
        self.check_cancel()?;
        let results: Vec<Value> = links
            .into_iter()
            .map(|link| {
                json!({
                    "text": link.text,
                    "to_title": link.to_title,
                    "to_page": link.to_page,
                    "resolved": link.resolved,
                    "summary": link.summary,
                })
            })
            .collect();
        Ok(json!({
            "links": results,
            "note": "出链只是候选方向，不是必读清单：结合写作任务自行判断要不要用 wiki_read 下钻；\
                     resolved=false 是待建页，没有内容。",
        }))
    }

    /// 按页的依据清单读原始资料文档（维基信息不够细时溯源原文）。
    /// 文档必须在页面当前版本的依据清单里，防止借维基名义乱翻资料库。
    fn wiki_doc_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        self.wiki_budget()?;
        self.wiki_enabled()?;
        let uuid = required_str(args, "page_id")?;
        let doc_id = int_arg(args, "doc_id")?;
        let refs = self.wiki.doc_refs(&uuid)?;
        let Some((_, note)) = refs.iter().find(|(id, _)| *id == doc_id) else {
            return Err(format!(
                "页面 {uuid} 当前版本的依据清单里没有文档 {doc_id}；\
                 先用 wiki_read 读该页，按它给出的依据清单选文档。"
            ));
        };
        self.check_cancel()?;
        let Some((title, full)) = self.kb.read(doc_id)? else {
            return Err(format!("资料库没有编号为 {doc_id} 的文档"));
        };
        self.check_cancel()?;
        let offset = optional_offset_arg(args);
        let page = read_page(&full, offset, read_limit_arg(args))?;
        let role = if note.trim().is_empty() {
            "维基依据".to_string()
        } else {
            format!("维基依据：{note}")
        };
        let id = self.record(
            "wiki_doc_read",
            &title,
            "资料库原文",
            &role,
            &page.text,
            true,
        );
        Ok(json!({
            "evidence": id,
            "page_id": uuid,
            "doc_id": doc_id,
            "support": note,
            "title": title,
            "text": page.text,
            "offset": page.offset,
            "total_chars": page.total_chars,
            "next_offset": page.next_offset,
            "unread_chars": page.unread_chars,
            "truncated": page.next_offset.is_some(),
            "note": READ_PAGE_NOTE,
        }))
    }

    fn ms_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let query = required_str(args, "query")?;
        let limit = usize_arg(args, "limit", 5);
        // 稿件库是 SQLite 查询，慢在磁盘/加密库上；调用前后各查一次取消。
        self.check_cancel()?;
        let rows = self.manuscripts.search(&query, limit)?;
        self.check_cancel()?;
        let mut results = Vec::new();
        for row in rows {
            // 逐条登记前复检：停止后不往证据台账里写摘要。
            self.check_cancel()?;
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
        // 读稿件全文可能要做版本解析与大段取文；调用前后各查一次取消。
        self.check_cancel()?;
        let Some(row) = self.manuscripts.read(id, version)? else {
            return Err(format!("稿件库没有编号为 {id} 的稿件"));
        };
        self.check_cancel()?;
        let full = row.markdown;
        if full.trim().is_empty() {
            return Err(format!("稿件《{}》没有正文", row.title));
        }
        let offset = optional_offset_arg(args);
        let page = read_page(&full, offset, read_limit_arg(args))?;
        let label = match row.version {
            Some(version) => format!("《{}》v{version}", row.title),
            None => format!("《{}》", row.title),
        };
        let evidence = self.record(
            "ms_read",
            &label,
            "稿件库",
            &page.location,
            &page.text,
            true,
        );
        Ok(json!({
            "evidence": evidence,
            "id": row.id,
            "version": row.version,
            "title": row.title,
            "kind": row.kind.label(),
            "status": row.status,
            "markdown": page.text,
            "offset": page.offset,
            "total_chars": page.total_chars,
            "next_offset": page.next_offset,
            "unread_chars": page.unread_chars,
            "truncated": page.next_offset.is_some(),
            "note": READ_PAGE_NOTE,
        }))
    }

    /// 只读接口搜索：只列「只查询且开放给 AI」的接口。
    fn api_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let query = required_str(args, "query")?;
        let limit = usize_arg(args, "limit", MAX_API_RESULTS).clamp(1, MAX_API_RESULTS);
        // 搜接口要读配置目录；调用前后各查一次取消。
        self.check_cancel()?;
        let hits = self.apis.search(&query, limit)?;
        self.check_cancel()?;
        if hits.is_empty() {
            return Ok(json!({
                "query": query,
                "results": [],
                "zero_hits": true,
                "note": "没有找到相关的只读接口。换更通用的关键词再搜；仍找不到就按材料写，不要猜数据。",
            }));
        }
        let results: Vec<Value> = hits
            .into_iter()
            .map(|hit| {
                json!({
                    "id": hit.id,
                    "name": hit.name,
                    "description": crate::ai_services::text::short(&hit.description, 300),
                    "params": hit.signature,
                })
            })
            .collect();
        Ok(json!({
            "query": query,
            "results": results,
            "zero_hits": false,
            "note": "用 api_call 调用：api 填 id，args 只放接口签名里的参数；返回是资料，不是结论。",
        }))
    }

    /// 调一个只读接口。参数先按 schema 校验，不合格不发请求；返回条目进证据台账。
    fn api_call(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let api = required_str(args, "api")?;
        let inputs = match args.get("args") {
            Some(Value::Object(map)) => map.clone(),
            Some(Value::Null) | None => Map::new(),
            Some(_) => return Err("参数 args 必须是对象".into()),
        };
        // 内网 HTTP 请求一旦发出就无法中途 abort，慢在 `apis.call`；调用前后各查一次取消，
        // 停止后即使请求已完成，也不登记证据、不把结果当成功交回去（由 call 统一兜底）。
        self.check_cancel()?;
        let called = self.apis.call(&api, &inputs)?;
        self.check_cancel()?;
        if called.items.is_empty() {
            return Ok(json!({
                "api": api,
                "name": called.name,
                "items": [],
                "total": called.total,
                "note": "接口调用成功但没有返回条目；换参数再查，或按材料写，不要编数据。",
            }));
        }
        // 完整条目先留在本轮结果缓存（内存 + 运行目录）：返回给模型的只是前几条 / 截短文本，
        // 长资料用 api_read 按 result_id 继续读，恢复后也能从运行目录读回。
        let result_id = {
            let mut cache = self
                .api_cache
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            cache.store(CachedApiResult {
                id: String::new(),
                api: api.clone(),
                name: called.name.clone(),
                total: called.total,
                items: called.items.clone(),
            })
        };
        let total_items = called.items.len();
        let shown = total_items.min(MAX_API_ITEMS);
        let mut truncated = called.total > shown || total_items > shown;
        let mut results = Vec::new();
        for item in called.items.into_iter().take(MAX_API_ITEMS) {
            // 逐条登记前复检：停止后不把接口资料写进证据台账。
            self.check_cancel()?;
            let text_truncated = item.text.chars().count() > MAX_API_ITEM_CHARS;
            truncated |= text_truncated;
            let text: String = item.text.chars().take(MAX_API_ITEM_CHARS).collect();
            let evidence = self.record("api_call", &item.title, "数据接口", "资料", &text, true);
            results.push(json!({
                "evidence": evidence,
                "title": item.title,
                "source": item.source,
                "text": text,
                "text_truncated": text_truncated,
            }));
        }
        Ok(json!({
            "api": api,
            "name": called.name,
            "result_id": result_id,
            "items": results,
            "total": called.total,
            "truncated": truncated,
            "note": if truncated {
                "返回条目或正文已截断，不能视为完整结果。用 api_read（result_id 填上面的值，item 从 0 开始）读取完整条目与全文；仍不够时再缩小查询条件。无法取得全文时说明资料不完整，不要据片段作完整统计或结论。"
            } else {
                "返回条目是资料，可据以写正文；不要写返回里没有的字段。需要看单条完整原文时用 api_read。"
            },
        }))
    }

    /// 续读一次接口调用留下的完整结果：不带 item 列条目；带 item 按字符分页读该条全文。
    fn api_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        self.check_cancel()?;
        let result_id = required_str(args, "result_id")?;
        let cached = {
            let mut cache = self
                .api_cache
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            cache.load(&result_id)
        }
        .ok_or_else(|| {
            "接口结果缓存已失效（可能来自更早的会话或已被清理）：请重新调用接口，或缩小查询条件。"
                .to_string()
        })?;
        let Some(item) = args.get("item").and_then(Value::as_u64) else {
            let items: Vec<Value> = cached
                .items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    json!({
                        "item": index,
                        "title": item.title,
                        "source": item.source,
                        "chars": item.text.chars().count(),
                    })
                })
                .collect();
            return Ok(json!({
                "result_id": result_id,
                "api": cached.api,
                "name": cached.name,
                "total": cached.total,
                "items": items,
                "note": "带 item 与 offset 读取某条完整原文；条目过多时先按标题挑需要的。",
            }));
        };
        let index = item as usize;
        let Some(found) = cached.items.get(index) else {
            return Err(format!(
                "没有第 {index} 条（这份结果共 {} 条，从 0 开始编号）",
                cached.items.len()
            ));
        };
        let offset = optional_offset_arg(args);
        let page = read_page(&found.text, offset, read_limit_arg(args))?;
        self.check_cancel()?;
        let evidence = self.record(
            "api_read",
            &found.title,
            "数据接口",
            "资料",
            &page.text,
            true,
        );
        Ok(json!({
            "evidence": evidence,
            "result_id": result_id,
            "api": cached.api,
            "item": index,
            "title": found.title,
            "source": found.source,
            "text": page.text,
            "offset": page.offset,
            "total_chars": page.total_chars,
            "next_offset": page.next_offset,
            "unread_chars": page.unread_chars,
            "truncated": page.next_offset.is_some(),
            "note": READ_PAGE_NOTE,
        }))
    }

    /// 任务作业的任务库连接：每次调用新开一个短连接，不跨线程共享 rusqlite 连接。
    fn task_store(&self) -> Result<crate::task_assistant::Store, String> {
        let path = self
            .task_db
            .as_ref()
            .ok_or_else(|| "本作业没有任务库".to_string())?;
        crate::task_assistant::Store::open_read_only(path)
            .map_err(|e| format!("打开任务库失败：{e:#}"))
    }

    /// 检索现有任务：模型据此判断材料讲的是已有任务还是新工作。
    fn task_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let limit = usize_arg(args, "limit", 20);
        let store = self.task_store()?;
        let tasks = store
            .search(query, limit)
            .map_err(|e| format!("检索任务失败：{e:#}"))?;
        let items: Vec<Value> = tasks.iter().map(task_brief).collect();
        let total = tasks.len();
        Ok(serde_json::json!({
            "items": items,
            "returned": total,
            "note": "现有任务摘要。task_id 只能填这里或 task_read 返回的 id；查不到就按新任务处理。",
        }))
    }

    /// 读一项任务的完整记录，供模型核对进展、下一步与风险。
    fn task_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = required_str(args, "id")?;
        let store = self.task_store()?;
        let task = store
            .get(&id)
            .map_err(|e| format!("读任务失败：{e:#}"))?
            .ok_or_else(|| format!("找不到任务 {id}"))?;
        Ok(serde_json::json!({
            "id": task.id,
            "name": task.name,
            "category": task.category,
            "status": task.status.as_str(),
            "owner": task.owner_name,
            "deadline": task.deadline,
            "progress": task.progress,
            "next_step": task.next_step,
            "risks": task.risks,
            "needs_review": task.needs_review,
            "parent_id": task.parent_id,
            "updated_at": task.updated_at,
        }))
    }

    fn vocab_search(&self, args: &Map<String, Value>) -> Result<Value, String> {
        // 词库查询在内存里，但同样走取消边界：停止后不再继续回结果。
        self.check_cancel()?;
        let value = super::writing_context::search(&self.config.vocabulary, args)?;
        self.check_cancel()?;
        Ok(value)
    }

    pub(crate) fn writing_context(&self) -> Value {
        json!({
            "date": super::writing_context::date_context(),
            "organization": super::writing_context::organization_context(&self.config.vocabulary, &self.request),
        })
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
        // 文档检查要跑词表、规则与要素校验，慢在下面这一段；前后各查一次取消。
        self.check_cancel()?;
        let mut hard = Vec::new();
        let mut soft = Vec::new();
        if let Err(error) = crate::document_reference::ensure_preserved(&self.current, &text) {
            hard.push(format!("公文引用检查未通过：{error}"));
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
        self.check_cancel()?;
        Ok(json!({
            "hard": hard,
            "soft": soft,
            "note": "只检查文稿格式、引用保护与文字规则，不判断事实真伪；成品由程序直接写入并保留撤销。",
        }))
    }

    /// 当前文种可用的风格档案。只读；档案例子与写法都不是本次事项的事实。
    fn style_list(&self) -> Result<Value, String> {
        // 读风格库要碰磁盘；调用前后各查一次取消。
        self.check_cancel()?;
        let list = self.styles.list(self.draft.kind)?;
        self.check_cancel()?;
        let truncated = list.truncated || list.rows.len() > MAX_STYLE_ROWS;
        let results = list
            .rows
            .iter()
            .take(MAX_STYLE_ROWS)
            .map(|row| {
                json!({
                    "id": style_excerpt(&row.id, 500),
                    "name": style_excerpt(&row.name, 100),
                    "note": style_excerpt(&row.note, 500),
                    "kinds": row.kinds.iter().take(20).map(|value| style_excerpt(value, 100)).collect::<Vec<_>>(),
                    "occasions": row.occasions.iter().take(10).map(|value| style_excerpt(value, 100)).collect::<Vec<_>>(),
                    "examples": row.examples,
                    "description_chars": row.description_chars,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "kind": self.draft.kind.label(),
            "results": results,
            "count": results.len(),
            "truncated": truncated,
            "note": STYLE_NOTE,
        }))
    }

    /// 按 id 读一份风格。未知 / 停用 / 不适用明确告知，不静默换成另一份。
    fn style_read(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = required_str(args, "id")?;
        // 读风格档案要碰磁盘；调用前后各查一次取消。
        self.check_cancel()?;
        let lookup = self.styles.read(&id, self.draft.kind)?;
        self.check_cancel()?;
        match lookup {
            PiStyleLookup::Found { id, name, text } => Ok(json!({
                "id": style_excerpt(&id, 500),
                "name": style_excerpt(&name, 100),
                "available": true,
                "text": style_excerpt(&text, MAX_STYLE_CHARS),
                "note": STYLE_NOTE,
            })),
            PiStyleLookup::Unavailable { reason, message } => Ok(json!({
                "id": id,
                "available": false,
                "reason": reason,
                "message": style_excerpt(&message, 1_000),
                "note": "没有换成别的风格；用 style_list 看当前文种可用的档案。",
            })),
        }
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

    fn search(&self, _query: &str, _top: usize) -> Result<PiSearchOutcome, String> {
        Err("知识库未启用".into())
    }

    fn read(&self, _doc_id: i64) -> Result<Option<(String, String)>, String> {
        Err("知识库未启用".into())
    }
}

/// 未接维基的占位（非写作作业、或库路径不可用）：工具给出明确指引，不静默空结果。
struct DisabledWiki;

impl PiWiki for DisabledWiki {
    fn enabled(&self) -> bool {
        false
    }

    fn search(&self, _query: &str, _top: usize) -> Result<Vec<PiWikiHit>, String> {
        Err("维基知识库未接入本轮作业".into())
    }

    fn read(&self, _uuid: &str) -> Result<Option<(String, String)>, String> {
        Err("维基知识库未接入本轮作业".into())
    }

    fn links(&self, _uuid: &str) -> Result<Vec<PiWikiLinkInfo>, String> {
        Err("维基知识库未接入本轮作业".into())
    }

    fn doc_refs(&self, _uuid: &str) -> Result<Vec<(i64, String)>, String> {
        Err("维基知识库未接入本轮作业".into())
    }
}

/// 生产实现：每轮写作作业按次打开维基库连接（rusqlite 连接非 Sync，不能跨线程
/// 缓存；与知识库检索链同姿势），工具调用频次低（独立小额度 12 次/轮），代价可接受。
struct WikiArchives {
    db: PathBuf,
}

impl WikiArchives {
    fn open(&self) -> Result<crate::wiki::WikiStore, String> {
        crate::wiki::WikiStore::open(&self.db).map_err(|error| format!("{error:#}"))
    }
}

/// 框架里 node_id 所在的「主题 / 分类」路径；找不到时给空串。
fn wiki_node_path(tree: &[crate::wiki::WikiTree], node_id: i64) -> String {
    for t in tree {
        if t.theme.id == node_id {
            return t.theme.name.clone();
        }
        if let Some(c) = t.categories.iter().find(|c| c.id == node_id) {
            return format!("{} / {}", t.theme.name, c.name);
        }
    }
    String::new()
}

/// 压平空白后取前 n 字做摘要/导语。
fn wiki_compact(text: &str, max_chars: usize) -> String {
    let flat: String = text.split_whitespace().collect();
    flat.chars().take(max_chars).collect()
}

impl PiWiki for WikiArchives {
    fn enabled(&self) -> bool {
        true
    }

    fn search(&self, query: &str, top: usize) -> Result<Vec<PiWikiHit>, String> {
        let store = self.open()?;
        let hits = store
            .search_for_evidence(query, top)
            .map_err(|error| format!("{error:#}"))?;
        let tree = store.list_tree().map_err(|error| format!("{error:#}"))?;
        let mut out = Vec::new();
        for hit in hits {
            let snippet = store
                .get_page(&hit.uuid)
                .ok()
                .flatten()
                .map(|page| wiki_compact(&page.body, 120))
                .unwrap_or_default();
            out.push(PiWikiHit {
                path: wiki_node_path(&tree, hit.node_id),
                page_id: hit.uuid,
                title: hit.title,
                snippet,
            });
        }
        Ok(out)
    }

    fn read(&self, uuid: &str) -> Result<Option<(String, String)>, String> {
        let store = self.open()?;
        if store
            .page_has_missing_sources(uuid)
            .map_err(|error| format!("{error:#}"))?
        {
            return Err("此页的依据已删除或替换，待人工复核；请检索资料库原文。".into());
        }
        store
            .get_page(uuid)
            .map(|page| page.map(|page| (page.meta.title, page.body)))
            .map_err(|error| format!("{error:#}"))
    }

    fn links(&self, uuid: &str) -> Result<Vec<PiWikiLinkInfo>, String> {
        let store = self.open()?;
        let tree = store.list_tree().map_err(|error| format!("{error:#}"))?;
        let links = store
            .outgoing_links(uuid)
            .map_err(|error| format!("{error:#}"))?;
        let mut out = Vec::new();
        for link in links {
            let info = if let Some(target) = &link.to_page {
                let (to_title, summary) = store
                    .get_page(target)
                    .ok()
                    .flatten()
                    .map(|page| (page.meta.title.clone(), wiki_compact(&page.body, 80)))
                    .unwrap_or_else(|| (link.to_title.clone(), String::new()));
                PiWikiLinkInfo {
                    text: link.text,
                    to_title,
                    to_page: Some(target.clone()),
                    resolved: true,
                    summary,
                }
            } else if let Some(node) = link.to_node {
                let guide = tree.iter().find_map(|branch| {
                    if branch.theme.id == node {
                        branch.theme_guide.as_ref()
                    } else {
                        branch.category_guides.get(&node)
                    }
                });
                let page = guide.and_then(|uuid| store.get_page(uuid).ok().flatten());
                PiWikiLinkInfo {
                    text: link.text.clone(),
                    to_title: page
                        .as_ref()
                        .map(|p| p.meta.title.clone())
                        .unwrap_or_else(|| {
                            if link.to_title.is_empty() {
                                link.text.clone()
                            } else {
                                link.to_title
                            }
                        }),
                    to_page: guide.cloned(),
                    resolved: guide.is_some(),
                    summary: page
                        .map(|p| wiki_compact(&p.body, 80))
                        .unwrap_or_else(|| "导览页待整编".into()),
                }
            } else {
                PiWikiLinkInfo {
                    text: link.text,
                    to_title: link.to_title.clone(),
                    to_page: None,
                    resolved: false,
                    summary: "待建页，还没有内容".into(),
                }
            };
            out.push(info);
        }
        Ok(out)
    }

    fn doc_refs(&self, uuid: &str) -> Result<Vec<(i64, String)>, String> {
        self.open().and_then(|store| {
            store
                .page_doc_refs(uuid)
                .map_err(|error| format!("{error:#}"))
        })
    }
}

/// 任务检索结果的一行：只带模型判断目标所需的最短信息。
fn task_brief(task: &crate::task_assistant::Task) -> Value {
    serde_json::json!({
        "id": task.id,
        "name": task.name,
        "status": task.status.as_str(),
        "owner": task.owner_name,
        "deadline": task.deadline,
        "next_step": task.next_step,
    })
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

/// 风格返回值的总字符上限包括省略号，不能再附加超出限额的提示。
fn style_excerpt(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() && max > 0 {
        out.pop();
        out.push('…');
    }
    out
}

/// 读一页原文：offset 是字符偏移，返回本页、总量与下一段偏移。
struct ReadPage {
    text: String,
    offset: usize,
    total_chars: usize,
    next_offset: Option<usize>,
    unread_chars: usize,
    location: String,
}

const READ_PAGE_NOTE: &str = "这是分页读到的一部分；next_offset 非空时还有未读内容，用同一个工具带 offset=next_offset 继续读，不要把本页当全文。";

fn optional_offset_arg(args: &Map<String, Value>) -> usize {
    args.get("offset")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(0)
}

/// 本次读多少字：host 会把隐藏参数 `limit_chars` 换成本轮的工具结果预算
/// （`readBudgetChars`）。模型看不到这个字段，也不能靠它要得比上限多：
/// 一律 clamp 到 `1..=MAX_READ_CHARS`；没有这个参数时按固定上限（旧行为）。
fn read_limit_arg(args: &Map<String, Value>) -> usize {
    args.get("limit_chars")
        .and_then(Value::as_u64)
        .map(|value| (value as usize).clamp(1, MAX_READ_CHARS))
        .unwrap_or(MAX_READ_CHARS)
}

fn read_page(full: &str, offset: usize, max: usize) -> Result<ReadPage, String> {
    let total_chars = full.chars().count();
    if total_chars == 0 {
        return Err("文档没有内容".into());
    }
    if offset >= total_chars {
        return Err(format!(
            "offset {offset} 超过文档长度（共 {total_chars} 字）；从 0 开始读"
        ));
    }
    let text: String = full.chars().skip(offset).take(max).collect();
    let end = offset + text.chars().count();
    let next_offset = (end < total_chars).then_some(end);
    Ok(ReadPage {
        text,
        offset,
        total_chars,
        next_offset,
        unread_chars: total_chars - end,
        location: format!("第 {offset}–{end} 字（共 {total_chars} 字）"),
    })
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::VocabularyCategory;

    #[test]
    fn long_documents_page_with_an_explicit_unread_remainder() {
        let text = "零一二三四五六七八九十";
        let page = read_page(text, 0, 4).unwrap();
        assert_eq!(page.text, "零一二三");
        assert_eq!(page.total_chars, 11);
        assert_eq!(page.next_offset, Some(4));
        assert_eq!(page.unread_chars, 7);
        assert!(page.location.contains("第 0–4 字"));
        let last = read_page(text, page.next_offset.unwrap(), 100).unwrap();
        assert_eq!(last.text, "四五六七八九十");
        assert_eq!(last.next_offset, None);
        assert_eq!(last.unread_chars, 0);
        assert!(read_page(text, 11, 4).is_err(), "越过末尾要明确失败");
    }

    fn args_of(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn the_read_budget_cannot_exceed_the_hard_limit() {
        let hard = super::MAX_READ_CHARS;
        assert_eq!(
            super::read_limit_arg(&Map::new()),
            hard,
            "缺省允许整篇大材料，调用时仍受 host 预算约束"
        );
        assert_eq!(
            super::read_limit_arg(&args_of(json!({"limit_chars": 3200}))),
            3200
        );
        assert_eq!(
            super::read_limit_arg(&args_of(json!({"limit_chars": 10_000_000}))),
            hard,
            "预算给得再大也不能突破上限"
        );
        assert_eq!(
            super::read_limit_arg(&args_of(json!({"limit_chars": 0}))),
            1,
            "0 也要能读一个字，不能返回空页"
        );
    }

    #[test]
    fn a_small_limit_pages_and_still_reports_the_unread_remainder() {
        let ctx = context("x");
        let first = ctx
            .call("ms_read", &json!({"id": 3, "limit_chars": 4, "offset": 0}))
            .unwrap();
        let text = first["markdown"].as_str().unwrap();
        assert_eq!(text.chars().count(), 4);
        assert_eq!(first["truncated"], true);
        assert_eq!(first["next_offset"], 4);
        let total = first["total_chars"].as_u64().unwrap() as usize;
        assert_eq!(first["unread_chars"], (total - 4) as u64);
        let second = ctx
            .call("ms_read", &json!({"id": 3, "limit_chars": 4, "offset": 4}))
            .unwrap();
        assert_eq!(second["offset"], 4);
        assert_ne!(second["markdown"], first["markdown"], "分页不能重复同一段");
    }

    #[test]
    fn kb_and_ms_read_accept_an_offset_and_report_what_is_unread() {
        let ctx = context("x");
        let first = ctx
            .call("kb_read", &json!({"doc_id": 7, "offset": 0}))
            .unwrap();
        assert_eq!(first["truncated"], false);
        assert!(first["next_offset"].is_null());
        let second = ctx.call("ms_read", &json!({"id": 3, "offset": 0})).unwrap();
        assert!(second["markdown"].as_str().unwrap().contains("去年10月9日"));
        assert!(
            ctx.call("ms_read", &json!({"id": 3, "offset": 99999}))
                .is_err()
        );
    }

    struct FakeKb {
        text: String,
        warnings: Vec<String>,
    }

    impl PiKnowledge for FakeKb {
        fn enabled(&self) -> bool {
            true
        }

        fn search(&self, _query: &str, _top: usize) -> Result<PiSearchOutcome, String> {
            Ok(PiSearchOutcome {
                chunks: vec![PiChunk {
                    doc_id: 7,
                    title: "森林防火规定".into(),
                    section: "第三条".into(),
                    text: self.text.clone(),
                }],
                warnings: self.warnings.clone(),
            })
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
                warnings: Vec::new(),
            }),
            Arc::new(FakeManuscripts),
        )
    }

    /// 假风格库：只验证工具行为，不读真实 `styles.json`。
    struct FakeStyles;

    impl PiStyles for FakeStyles {
        fn list(&self, kind: TemplateKind) -> Result<PiStyleList, String> {
            let all = vec![
                PiStyleRow {
                    id: "literal-style".into(),
                    name: "公函写法".into(),
                    note: "商洽有礼".into(),
                    kinds: vec![kind.label().into()],
                    occasions: vec!["商洽".into()],
                    examples: 2,
                    description_chars: 30,
                },
                PiStyleRow {
                    id: "notice-style".into(),
                    name: "通知写法".into(),
                    note: "部署明确".into(),
                    kinds: vec!["通知".into()],
                    occasions: vec![],
                    examples: 1,
                    description_chars: 20,
                },
                PiStyleRow {
                    id: "disabled-style".into(),
                    name: "旧写法".into(),
                    note: String::new(),
                    kinds: vec![],
                    occasions: vec![],
                    examples: 0,
                    description_chars: 0,
                },
            ];
            let cutoff = 12usize;
            Ok(PiStyleList {
                truncated: all.len() > cutoff,
                rows: all,
            })
        }

        fn read(&self, id: &str, kind: TemplateKind) -> Result<PiStyleLookup, String> {
            Ok(match id {
                "oversized" => PiStyleLookup::Found {
                    id: id.into(),
                    name: "名".repeat(10_000),
                    text: "例".repeat(100_000),
                },
                "literal-style" => PiStyleLookup::Found {
                    id: id.into(),
                    name: "公函写法".into(),
                    text: "【写法风格：公函写法】商洽有礼，用「请予支持为荷」。".into(),
                },
                "disabled-style" => PiStyleLookup::Unavailable {
                    reason: "disabled",
                    message: "风格「旧写法」已停用，不能使用。".into(),
                },
                "notice-style" => PiStyleLookup::Unavailable {
                    reason: "not_applicable",
                    message: format!("风格「通知写法」不适用于当前文种（{}）。", kind.label()),
                },
                other => PiStyleLookup::Unavailable {
                    reason: "unknown",
                    message: format!("没有编号为「{other}」的风格档案。"),
                },
            })
        }
    }

    fn style_context() -> PiToolContext {
        PiToolContext::with_all_sources(
            Arc::new(AppConfig::default()),
            DraftInput::default(),
            "# 当前\n".into(),
            "按风格改。".into(),
            vec![],
            PiSources {
                knowledge: Arc::new(FakeKb {
                    text: "x".into(),
                    warnings: Vec::new(),
                }),
                manuscripts: Arc::new(FakeManuscripts),
                styles: Arc::new(FakeStyles),
            },
        )
    }

    #[test]
    fn style_tools_are_read_only_and_never_become_fact_sources() {
        let ctx = style_context();
        let list = ctx.call("style_list", &json!({})).unwrap();
        assert_eq!(list["kind"], "公函");
        assert_eq!(list["count"], 3);
        assert_eq!(list["truncated"], false);
        assert_eq!(list["results"][0]["id"], "literal-style");
        assert!(list["note"].as_str().unwrap().contains("style_sample"));
        assert!(list["note"].as_str().unwrap().contains("不得写进正文"));
        let read = ctx
            .call("style_read", &json!({"id": "literal-style"}))
            .unwrap();
        assert_eq!(read["available"], true);
        assert!(read["text"].as_str().unwrap().contains("请予支持为荷"));
        assert!(
            read["note"]
                .as_str()
                .unwrap()
                .contains("不是本次事项的事实来源")
        );
        // 风格结果是写法样稿：不登记证据，也不进事实来源。
        assert!(ctx.evidence().is_empty());
        assert!(ctx.fact_sources().is_empty());
    }

    #[test]
    fn unknown_disabled_or_inapplicable_styles_are_reported_not_substituted() {
        let ctx = style_context();
        for (id, reason) in [
            ("missing", "unknown"),
            ("disabled-style", "disabled"),
            ("notice-style", "not_applicable"),
        ] {
            let value = ctx.call("style_read", &json!({"id": id})).unwrap();
            assert_eq!(value["available"], false, "{value}");
            assert_eq!(value["reason"], reason, "{value}");
            assert!(value.get("text").is_none(), "不能给出替代风格：{value}");
            assert!(
                value["message"]
                    .as_str()
                    .unwrap()
                    .contains(&"风格".to_string())
            );
        }
        assert!(ctx.call("style_read", &json!({})).is_err());
    }

    #[test]
    fn style_read_bounds_all_profile_text_at_the_tool_boundary() {
        let value = style_context()
            .call("style_read", &json!({"id": "oversized"}))
            .unwrap();
        assert!(value["name"].as_str().unwrap().chars().count() <= 100);
        assert!(value["text"].as_str().unwrap().chars().count() <= MAX_STYLE_CHARS);
    }

    /// 假维基库：两页 + 出链候选 + 依据清单，只验证工具行为。
    struct FakeWiki;

    impl PiWiki for FakeWiki {
        fn enabled(&self) -> bool {
            true
        }

        fn search(&self, _query: &str, _top: usize) -> Result<Vec<PiWikiHit>, String> {
            Ok(vec![
                PiWikiHit {
                    page_id: "uuid-a".into(),
                    title: "监督检查".into(),
                    path: "安全生产 / 监督检查".into(),
                    snippet: "四不两直与闭环整改。".into(),
                },
                PiWikiHit {
                    page_id: "uuid-b".into(),
                    title: "责任体系".into(),
                    path: "安全生产 / 监督检查".into(),
                    snippet: "职责分工与考核。".into(),
                },
            ])
        }

        fn read(&self, uuid: &str) -> Result<Option<(String, String)>, String> {
            Ok(match uuid {
                "uuid-a" => Some((
                    "监督检查".into(),
                    "监督检查要闭环整改。见 [[责任体系]]。".into(),
                )),
                _ => None,
            })
        }

        fn links(&self, _uuid: &str) -> Result<Vec<PiWikiLinkInfo>, String> {
            Ok(vec![
                PiWikiLinkInfo {
                    text: "责任体系".into(),
                    to_title: "责任体系".into(),
                    to_page: Some("uuid-b".into()),
                    resolved: true,
                    summary: "职责分工与考核".into(),
                },
                PiWikiLinkInfo {
                    text: "安全文化".into(),
                    to_title: "安全文化".into(),
                    to_page: None,
                    resolved: false,
                    summary: "待建页，还没有内容".into(),
                },
            ])
        }

        fn doc_refs(&self, uuid: &str) -> Result<Vec<(i64, String)>, String> {
            Ok(if uuid == "uuid-a" {
                vec![(7, "闭环整改要求".into())]
            } else {
                Vec::new()
            })
        }
    }

    fn wiki_context() -> PiToolContext {
        context("资料原文。").with_wiki(Arc::new(FakeWiki))
    }

    #[test]
    fn wiki_search_records_each_hit_as_fact_evidence() {
        let ctx = wiki_context();
        let hits = ctx
            .call("wiki_search", &json!({"query": "监督检查"}))
            .unwrap();
        assert_eq!(hits["zero_hits"], false);
        let results = hits["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["page_id"], "uuid-a");
        assert_eq!(results[0]["path"], "安全生产 / 监督检查");
        assert_eq!(results[0]["evidence"], "E1");
        // 命中进证据台账，也进事实来源。
        assert_eq!(ctx.evidence().len(), 2);
        assert_eq!(ctx.fact_sources().len(), 2);
    }

    #[test]
    fn wiki_read_pages_like_kb_and_reports_missing_pages() {
        let ctx = wiki_context();
        let page = ctx
            .call("wiki_read", &json!({"page_id": "uuid-a", "limit_chars": 6}))
            .unwrap();
        assert_eq!(page["title"], "监督检查");
        assert_eq!(page["text"].as_str().unwrap().chars().count(), 6);
        assert_eq!(page["next_offset"], 6);
        assert_eq!(page["truncated"], true);
        assert!(
            ctx.call("wiki_read", &json!({"page_id": "uuid-x"}))
                .is_err()
        );
        assert!(ctx.call("wiki_read", &json!({})).is_err());
    }

    #[test]
    fn wiki_links_are_candidates_never_fact_sources() {
        let ctx = wiki_context();
        let links = ctx
            .call("wiki_links", &json!({"page_id": "uuid-a"}))
            .unwrap();
        let items = links["links"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["resolved"], true);
        assert_eq!(items[0]["to_page"], "uuid-b");
        assert_eq!(items[1]["resolved"], false, "待建页要明确标出");
        // 候选方向不登记证据，更不是事实来源。
        assert!(ctx.evidence().is_empty());
        assert!(ctx.fact_sources().is_empty());
    }

    #[test]
    fn wiki_doc_read_requires_doc_in_the_pages_evidence_list() {
        let ctx = wiki_context();
        let read = ctx
            .call("wiki_doc_read", &json!({"page_id": "uuid-a", "doc_id": 7}))
            .unwrap();
        assert_eq!(read["title"], "森林防火规定");
        assert_eq!(read["support"], "闭环整改要求");
        assert_eq!(read["evidence"], "E1");
        // 不在依据清单里的文档一律拒绝。
        assert!(
            ctx.call("wiki_doc_read", &json!({"page_id": "uuid-a", "doc_id": 9}))
                .is_err()
        );
        // 页面没有依据清单时同样拒绝。
        assert!(
            ctx.call("wiki_doc_read", &json!({"page_id": "uuid-b", "doc_id": 7}))
                .is_err()
        );
    }

    #[test]
    fn wiki_tools_stop_at_the_independent_round_budget() {
        let ctx = wiki_context();
        for i in 0..super::WIKI_TOOL_BUDGET {
            assert!(
                ctx.call("wiki_links", &json!({"page_id": "uuid-a"}))
                    .is_ok(),
                "第 {} 次应在额度内",
                i + 1
            );
        }
        let err = ctx
            .call("wiki_links", &json!({"page_id": "uuid-a"}))
            .unwrap_err();
        assert!(err.contains("额度已用完"), "{err}");
    }

    #[test]
    fn wiki_tools_are_closed_for_task_jobs_and_unmounted_rounds() {
        // 任务助手作业不挂维基工具（也不挂知识库）。
        let task = context("x").task_only(PathBuf::from(":memory:"));
        assert!(task.call("wiki_search", &json!({"query": "甲"})).is_err());
        assert!(task.call("wiki_links", &json!({"page_id": "u"})).is_err());
        // 未挂维基来源的写作上下文给出明确指引，不静默空结果。
        let bare = context("x");
        let err = bare
            .call("wiki_search", &json!({"query": "甲"}))
            .unwrap_err();
        assert!(err.contains("未接入"), "{err}");
    }

    #[test]
    fn user_answers_are_retained_without_requiring_fact_confirmation() {
        let ctx = context("");
        let candidate = "# 通知\n\n请于2026年11月8日完成工作。";
        assert!(
            ctx.call("check_document", &json!({"text":candidate}))
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
        assert_eq!(value["zero_hits"], false);
        assert_eq!(value["warnings"], json!([]));
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
    fn kb_warnings_survive_with_hits_and_zero_hits_remain_distinct() {
        let mut config = AppConfig::default();
        config.rag.enabled = true;
        let kb: Arc<dyn PiKnowledge> = Arc::new(FakeKb {
            text: "检索命中内容".into(),
            warnings: vec!["embedding不可用，本次只用关键词检索".into()],
        });
        let ctx = PiToolContext::with_sources(
            Arc::new(config.clone()),
            DraftInput::default(),
            String::new(),
            String::new(),
            Vec::new(),
            kb,
            Arc::new(FakeManuscripts),
        );
        let hit = ctx.call("kb_search", &json!({"query":"检索"})).unwrap();
        assert_eq!(hit["zero_hits"], false);
        assert_eq!(hit["warnings"][0], "embedding不可用，本次只用关键词检索");
        assert!(hit["results"][0]["evidence"].is_string());

        let empty_kb: Arc<dyn PiKnowledge> = Arc::new(EmptyWarningKb);
        let empty = PiToolContext::with_sources(
            Arc::new(config),
            DraftInput::default(),
            String::new(),
            String::new(),
            Vec::new(),
            empty_kb,
            Arc::new(FakeManuscripts),
        );
        let miss = empty.call("kb_search", &json!({"query":"无匹配"})).unwrap();
        assert_eq!(miss["zero_hits"], true);
        assert_eq!(miss["results"], json!([]));
        assert_eq!(miss["warnings"][0], "rerank不可用，已保留关键词结果");
    }

    #[test]
    fn pi_tool_context_uses_real_temporary_sqlite_knowledge_chain() {
        struct ConfigReset;
        impl Drop for ConfigReset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let directory = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(directory.path().to_path_buf()));
        let _reset = ConfigReset;
        let manuscript_db = crate::storage::manuscript_db_path().unwrap();
        let mut manuscripts = crate::manuscript::ManuscriptStore::open(&manuscript_db).unwrap();
        let manuscript_id = manuscripts
            .create(
                &crate::manuscript::NewManuscript {
                    snapshot: DraftInput {
                        title_hint: "森林防火通知".into(),
                        ..Default::default()
                    },
                    content_markdown: "# 森林防火通知\n\n咨询电话010-12345678。\n".into(),
                    status: crate::models::ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        drop(manuscripts);
        let db = crate::storage::manuscript_db_path().unwrap();
        let mut store = crate::knowledge::KnowledgeStore::open(&db).unwrap();
        let doc_id = store
            .import_document(&crate::knowledge::KnowledgeImportItem {
                source: crate::knowledge::KnowledgeSource::Markdown,
                source_manuscript_id: None,
                source_path: "fixture.md".into(),
                kind: TemplateKind::OfficialLetter,
                title: "森林防火通知".into(),
                content_markdown: format!(
                    "# 森林防火通知\n\n## 工作要求\n\n{}\n",
                    "各单位应于十月开展森林防火检查，并记录隐患整改情况。".repeat(20)
                ),
            })
            .unwrap();
        let text = "各单位应于十月开展森林防火检查，并记录隐患整改情况。";
        store
            .replace_chunks(
                doc_id,
                &[crate::knowledge::NewKnowledgeChunk {
                    ord: 0,
                    section: "工作要求".into(),
                    text: text.into(),
                    tokens: crate::rag::tokenize(text),
                    embedding: None,
                    dims: 0,
                }],
                "isolated-test-embedding",
            )
            .unwrap();
        drop(store);

        let rag = crate::models::RagConfig {
            enabled: true,
            embedding: crate::models::EmbeddingConfig {
                model: "isolated-test-embedding".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                timeout_seconds: 1,
                ..Default::default()
            },
            rerank: crate::models::RerankConfig {
                mode: crate::models::RerankMode::None,
                ..Default::default()
            },
            ..Default::default()
        };
        let knowledge: Arc<dyn PiKnowledge> = Arc::new(RagSearch {
            enabled: true,
            rag,
            chat: crate::models::LmStudioConfig::default(),
            kind_filter: None,
        });
        let ctx = PiToolContext::with_all_sources(
            Arc::new(AppConfig::default()),
            DraftInput::default(),
            String::new(),
            "真实资料检索评测".into(),
            Vec::new(),
            PiSources {
                knowledge,
                manuscripts: Arc::new(crate::ai_services::manuscripts::SqliteManuscripts),
                styles: Arc::new(FakeStyles),
            },
        );
        let hit = ctx
            .call("kb_search", &json!({"query":"森林防火通知", "top":1}))
            .unwrap();
        assert_eq!(
            hit["zero_hits"], false,
            "本地 SQLite FTS 应有真实命中：{hit}"
        );
        assert_eq!(hit["results"].as_array().unwrap().len(), 1);
        assert!(
            !hit["warnings"].as_array().unwrap().is_empty(),
            "embed loopback 失败应明确作为降级警告：{hit}"
        );
        let evidence = hit["results"][0]["evidence"].as_str().unwrap();
        assert_eq!(evidence, "E1");
        let read = ctx
            .call(
                "kb_read",
                &json!({"doc_id":doc_id, "offset":0, "limit_chars":100}),
            )
            .unwrap();
        assert_eq!(read["offset"], 0);
        assert!(read["next_offset"].is_number());
        assert!(
            ctx.evidence()
                .iter()
                .any(|item| item.id == evidence && item.fact)
        );
        let manuscript_hit = ctx
            .call("ms_search", &json!({"query":"森林防火通知", "limit":3}))
            .unwrap();
        assert!(
            manuscript_hit["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == manuscript_id)
        );
        let manuscript_read = ctx
            .call("ms_read", &json!({"id": manuscript_id, "limit_chars": 100}))
            .unwrap();
        assert!(
            manuscript_read["markdown"]
                .as_str()
                .unwrap()
                .contains("010-12345678")
        );
        assert!(
            ctx.evidence()
                .iter()
                .any(|item| item.tool == "ms_read" && item.text.contains("010-12345678"))
        );
    }

    struct EmptyWarningKb;
    impl PiKnowledge for EmptyWarningKb {
        fn enabled(&self) -> bool {
            true
        }
        fn search(&self, _query: &str, _top: usize) -> Result<PiSearchOutcome, String> {
            Ok(PiSearchOutcome {
                chunks: Vec::new(),
                warnings: vec!["rerank不可用，已保留关键词结果".into()],
            })
        }
        fn read(&self, _doc_id: i64) -> Result<Option<(String, String)>, String> {
            Ok(None)
        }
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
    fn check_document_does_not_run_fact_verification() {
        let ctx = context("各单位应于每年十月开展一次检查。");
        let bad = ctx
            .call(
                "check_document",
                &json!({"text": "# 通知\n\n请于2026年10月8日报送。\n"}),
            )
            .unwrap();
        assert!(bad["hard"].as_array().unwrap().is_empty());
        let good = ctx
            .call(
                "check_document",
                &json!({"text": "# 通知\n\n请各单位于每年十月开展一次检查。\n"}),
            )
            .unwrap();
        assert!(good["hard"].as_array().unwrap().is_empty(), "{good}");
    }

    #[test]
    fn current_date_is_a_read_only_tool_and_writing_context_is_provided() {
        let ctx = context("原文");
        let before = ctx.current.clone();
        assert!(PiToolContext::known("current_date"));
        let date = ctx.call("current_date", &json!({})).unwrap();
        assert!(date["now"].as_str().unwrap().contains("星期"));
        assert!(!date["timezone"].as_str().unwrap().is_empty());
        assert!(ctx.evidence().is_empty());
        let snapshot = ctx.writing_context();
        assert_eq!(snapshot["date"]["today"], date["today"]);
        assert!(snapshot["organization"]["results"].is_array());
        assert_eq!(ctx.current, before);
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
            Arc::new(FakeKb {
                text: "x".into(),
                warnings: Vec::new(),
            }),
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

    /// 假内网接口：只验证工具行为、参数校验与证据登记。
    struct FakeApis;

    impl PiApis for FakeApis {
        fn search(&self, query: &str, limit: usize) -> Result<Vec<PiApiHit>, String> {
            if query != "火灾" {
                return Ok(Vec::new());
            }
            Ok(vec![PiApiHit {
                id: "stat.fire".into(),
                name: "火灾统计".into(),
                description: "按地区统计森林火灾".into(),
                signature: "region: 文字（必填）".into(),
            }]
            .into_iter()
            .take(limit)
            .collect())
        }

        fn call(&self, api: &str, args: &Map<String, Value>) -> Result<PiApiCall, String> {
            if api != "stat.fire" {
                return Err(format!("没有配置 id 为「{api}」的数据接口"));
            }
            if args.get("region").and_then(Value::as_str) != Some("全省") {
                return Err("参数 region 必须是「全省」".into());
            }
            Ok(PiApiCall {
                name: "火灾统计".into(),
                total: 3,
                items: (1..=3)
                    .map(|index| PiApiItem {
                        title: format!("统计{index}"),
                        source: "省统计系统".into(),
                        text: format!("第{index}条：全省森林火灾 {index} 起"),
                    })
                    .collect(),
            })
        }
    }

    fn api_context() -> PiToolContext {
        context("x").with_apis(Arc::new(FakeApis))
    }

    #[test]
    fn api_search_returns_ids_and_parameter_signatures() {
        let ctx = api_context();
        let value = ctx
            .call("api_search", &json!({"query": "火灾", "limit": 3}))
            .unwrap();
        assert_eq!(value["results"][0]["id"], "stat.fire");
        assert_eq!(value["results"][0]["name"], "火灾统计");
        assert!(
            value["results"][0]["params"]
                .as_str()
                .unwrap()
                .contains("region"),
            "要给模型参数签名"
        );
        assert!(ctx.evidence().is_empty(), "搜接口不产生事实证据");
    }

    #[test]
    fn api_search_reports_zero_hits_without_guessing() {
        let ctx = api_context();
        let value = ctx
            .call("api_search", &json!({"query": "不存在的东西"}))
            .unwrap();
        assert_eq!(value["zero_hits"], true);
        assert_eq!(value["results"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn api_call_registers_evidence_and_bounds_returned_items() {
        let ctx = api_context();
        let value = ctx
            .call(
                "api_call",
                &json!({"api": "stat.fire", "args": {"region": "全省"}}),
            )
            .unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 3);
        assert_eq!(value["total"], 3);
        assert_eq!(value["truncated"], false);
        assert!(
            value["items"][0]["evidence"]
                .as_str()
                .unwrap()
                .starts_with('E')
        );
        let evidence = ctx.evidence();
        assert_eq!(evidence.len(), 3);
        assert!(
            evidence
                .iter()
                .all(|item| item.tool == "api_call" && item.fact)
        );
        assert!(
            ctx.fact_sources()
                .iter()
                .any(|text| text.contains("全省森林火灾 1 起")),
            "接口返回要能作为事实来源"
        );
    }

    #[test]
    fn api_call_rejects_bad_parameters_without_touching_evidence() {
        let ctx = api_context();
        let error = ctx
            .call(
                "api_call",
                &json!({"api": "stat.fire", "args": {"region": "全市"}}),
            )
            .unwrap_err();
        assert!(error.contains("region"), "{error}");
        assert!(ctx.evidence().is_empty());
    }

    #[test]
    fn api_call_marks_a_single_long_item_as_incomplete() {
        struct LongApi;
        impl PiApis for LongApi {
            fn search(&self, _: &str, _: usize) -> Result<Vec<PiApiHit>, String> {
                Ok(Vec::new())
            }
            fn call(&self, _: &str, _: &Map<String, Value>) -> Result<PiApiCall, String> {
                Ok(PiApiCall {
                    name: "长资料".into(),
                    total: 1,
                    items: vec![PiApiItem {
                        title: "统计说明".into(),
                        source: "统计系统".into(),
                        text: "中".repeat(MAX_API_ITEM_CHARS + 1),
                    }],
                })
            }
        }
        let ctx = context("x").with_apis(Arc::new(LongApi));
        let result = ctx.call("api_call", &json!({"api": "long"})).unwrap();
        assert_eq!(result["truncated"], true);
        assert_eq!(result["items"][0]["text_truncated"], true);
        assert!(result["note"].as_str().unwrap().contains("不完整"));
        assert_eq!(
            result["items"][0]["text"].as_str().unwrap().chars().count(),
            MAX_API_ITEM_CHARS
        );
        assert_eq!(ctx.evidence()[0].text.chars().count(), MAX_API_ITEM_CHARS);
    }

    #[test]
    fn api_search_only_lists_usable_endpoints() {
        let mut usable = ApiEndpoint {
            id: "stat.fire".into(),
            name: "火灾统计".into(),
            description: "森林火灾".into(),
            ..ApiEndpoint::default()
        };
        let mut closed = ApiEndpoint {
            id: "stat.write".into(),
            name: "火灾统计写入".into(),
            description: "森林火灾".into(),
            readonly: false,
            ..ApiEndpoint::default()
        };
        let mut hidden = ApiEndpoint {
            id: "stat.hidden".into(),
            name: "火灾统计内部".into(),
            description: "森林火灾".into(),
            ai: false,
            ..ApiEndpoint::default()
        };
        let mut unsupported = ApiEndpoint {
            id: "stat.upload".into(),
            name: "火灾上报".into(),
            description: "森林火灾".into(),
            unsupported: "multipart 暂不支持".into(),
            ..ApiEndpoint::default()
        };
        let store = ApiStore {
            endpoints: vec![
                std::mem::take(&mut usable),
                std::mem::take(&mut closed),
                std::mem::take(&mut hidden),
                std::mem::take(&mut unsupported),
            ],
            ..ApiStore::default()
        };
        let hits = super::search_apis(&store, "火灾", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "stat.fire");
    }

    /// 记数的假知识库：证明取消后不再发起检索 / 读全文。
    struct CountingKb {
        searches: Arc<AtomicUsize>,
        reads: Arc<AtomicUsize>,
        cancel_on_search: Option<Arc<AtomicBool>>,
    }

    impl PiKnowledge for CountingKb {
        fn enabled(&self) -> bool {
            true
        }

        fn search(&self, _query: &str, _top: usize) -> Result<PiSearchOutcome, String> {
            self.searches.fetch_add(1, Ordering::AcqRel);
            if let Some(flag) = &self.cancel_on_search {
                // 模拟「检索期间用户点了停止」：请求已经发出、无法中途 abort。
                flag.store(true, Ordering::Relaxed);
            }
            Ok(PiSearchOutcome {
                chunks: vec![PiChunk {
                    doc_id: 7,
                    title: "森林防火规定".into(),
                    section: "第三条".into(),
                    text: "各单位应于每年十月开展一次检查。".into(),
                }],
                warnings: Vec::new(),
            })
        }

        fn read(&self, doc_id: i64) -> Result<Option<(String, String)>, String> {
            self.reads.fetch_add(1, Ordering::AcqRel);
            Ok((doc_id == 7).then(|| ("森林防火规定".into(), "全文。".into())))
        }
    }

    /// 记数的假内网接口：证明取消后不再搜接口、不再发 HTTP。
    struct CountingApis {
        searches: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
        cancel_on_call: Option<Arc<AtomicBool>>,
    }

    impl PiApis for CountingApis {
        fn search(&self, _query: &str, _limit: usize) -> Result<Vec<PiApiHit>, String> {
            self.searches.fetch_add(1, Ordering::AcqRel);
            Ok(vec![PiApiHit {
                id: "stat.fire".into(),
                name: "火灾统计".into(),
                description: "按地区统计森林火灾".into(),
                signature: "region: 文字（必填）".into(),
            }])
        }

        fn call(&self, _api: &str, _args: &Map<String, Value>) -> Result<PiApiCall, String> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            if let Some(flag) = &self.cancel_on_call {
                flag.store(true, Ordering::Relaxed);
            }
            Ok(PiApiCall {
                name: "火灾统计".into(),
                total: 1,
                items: vec![PiApiItem {
                    title: "统计".into(),
                    source: "省统计系统".into(),
                    text: "第1条：全省森林火灾 1 起".into(),
                }],
            })
        }
    }

    fn counting_context(kb: Arc<dyn PiKnowledge>) -> PiToolContext {
        PiToolContext::with_all_sources(
            Arc::new(AppConfig::default()),
            DraftInput::default(),
            "# 通知\n\n原文。\n".into(),
            "改一下。".into(),
            Vec::new(),
            PiSources {
                knowledge: kb,
                manuscripts: Arc::new(FakeManuscripts),
                styles: Arc::new(FakeStyles),
            },
        )
    }

    /// 停止后工具入口直接拒绝：参数校验之前就返回，不再发起任何检索 / 网络调用。
    #[test]
    fn a_stopped_run_refuses_every_tool_before_touching_sources() {
        let searches = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let api_searches = Arc::new(AtomicUsize::new(0));
        let api_calls = Arc::new(AtomicUsize::new(0));
        let ctx = counting_context(Arc::new(CountingKb {
            searches: searches.clone(),
            reads: reads.clone(),
            cancel_on_search: None,
        }))
        .with_apis(Arc::new(CountingApis {
            searches: api_searches.clone(),
            calls: api_calls.clone(),
            cancel_on_call: None,
        }));
        ctx.attach_cancel(Arc::new(AtomicBool::new(true)));
        for (name, args) in [
            ("kb_search", json!({"query": "防火检查"})),
            ("kb_read", json!({"doc_id": 7})),
            ("ms_search", json!({"query": "防火通知"})),
            ("ms_read", json!({"id": 3})),
            ("api_search", json!({"query": "火灾"})),
            (
                "api_call",
                json!({"api": "stat.fire", "args": {"region": "全省"}}),
            ),
            ("vocab_search", json!({"query": "应急"})),
            ("current_date", json!({})),
            ("check_document", json!({"text": "# 通知\n\n原文。"})),
            ("doc_elements", json!({})),
            ("style_list", json!({})),
            ("style_read", json!({"id": "literal-style"})),
        ] {
            let error = ctx.call(name, &args).unwrap_err();
            assert!(error.contains("停止"), "{name} 应报告已停止：{error}");
        }
        assert_eq!(searches.load(Ordering::Acquire), 0, "停止后不得检索知识库");
        assert_eq!(reads.load(Ordering::Acquire), 0, "停止后不得读知识库全文");
        assert_eq!(api_searches.load(Ordering::Acquire), 0, "停止后不得搜接口");
        assert_eq!(api_calls.load(Ordering::Acquire), 0, "停止后不得调接口");
        assert!(ctx.evidence().is_empty(), "取消的工具调用不得写证据");
    }

    /// 取消发生在慢工具返回之前：请求已经发出（无法 abort），但结果不登记、不算成功。
    #[test]
    fn a_cancel_arriving_during_a_slow_tool_discards_its_result_and_evidence() {
        let cancel = Arc::new(AtomicBool::new(false));
        let searches = Arc::new(AtomicUsize::new(0));
        let ctx = counting_context(Arc::new(CountingKb {
            searches: searches.clone(),
            reads: Arc::new(AtomicUsize::new(0)),
            cancel_on_search: Some(cancel.clone()),
        }));
        ctx.attach_cancel(cancel);
        let error = ctx
            .call("kb_search", &json!({"query": "防火检查"}))
            .unwrap_err();
        assert!(error.contains("停止"), "{error}");
        assert_eq!(
            searches.load(Ordering::Acquire),
            1,
            "检索已经发出，停止信号只能让结果被丢弃，不能假装没调用"
        );
        assert!(ctx.evidence().is_empty(), "停止后返回的资料不得写证据");

        let cancel = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = context("x").with_apis(Arc::new(CountingApis {
            searches: Arc::new(AtomicUsize::new(0)),
            calls: calls.clone(),
            cancel_on_call: Some(cancel.clone()),
        }));
        ctx.attach_cancel(cancel);
        let error = ctx
            .call(
                "api_call",
                &json!({"api": "stat.fire", "args": {"region": "全省"}}),
            )
            .unwrap_err();
        assert!(error.contains("停止"), "{error}");
        assert_eq!(
            calls.load(Ordering::Acquire),
            1,
            "HTTP 请求已经发出，只能丢弃结果"
        );
        assert!(ctx.evidence().is_empty(), "停止后返回的接口资料不得写证据");
    }

    /// 挂了取消句柄但没置位：行为与以前一致，连续调用也不会泄漏在途槽位。
    #[test]
    fn an_attached_but_unset_cancel_flag_changes_nothing() {
        let ctx = api_context();
        let first = Arc::new(AtomicBool::new(false));
        ctx.attach_cancel(first.clone());
        // 只设置一次：第二个句柄被忽略，第一个仍是唯一信号。
        ctx.attach_cancel(Arc::new(AtomicBool::new(true)));
        for _ in 0..(MAX_IN_FLIGHT_TOOLS + 1) {
            // 多调一次：槽位若没在 Drop 里归还，这里会等到 10 秒后失败。
            let value = ctx
                .call("kb_search", &json!({"query": "防火检查"}))
                .unwrap();
            assert_eq!(value["zero_hits"], false);
        }
        assert_eq!(ctx.evidence().len(), MAX_IN_FLIGHT_TOOLS + 1);
        // 第一个句柄置位后才开始拒绝，证明没被第二个句柄覆盖。
        first.store(true, Ordering::Relaxed);
        let error = ctx
            .call("kb_search", &json!({"query": "防火检查"}))
            .unwrap_err();
        assert!(error.contains("停止"), "{error}");
    }

    /// 在途槽位：满了明确失败，Drop 归还；不依赖后台线程的时序。
    #[test]
    fn in_flight_slots_are_capped_and_returned_on_drop() {
        let ctx = context("x");
        let mut held = Vec::new();
        for _ in 0..MAX_IN_FLIGHT_TOOLS {
            held.push(ToolSlot::acquire(&ctx).expect("资源空闲时应能申请到在途槽位"));
        }
        assert!(in_flight_tools() >= MAX_IN_FLIGHT_TOOLS);
        let error = ToolSlot::acquire_within(&ctx, Duration::ZERO).unwrap_err();
        assert!(
            error.contains("上一个已停止的任务仍在收尾"),
            "满了要明确说明是谁占着：{error}"
        );
        // 释放一个后应能重新申请到；申请不到说明 Drop 没归还计数。
        drop(held.pop().expect("至少持有一个槽位"));
        let again = ToolSlot::acquire(&ctx).expect("释放后应能重新申请槽位");
        drop(again);
        assert!(in_flight_tools() <= MAX_IN_FLIGHT_TOOLS);
    }

    /// 长结果：25 条、首条超长；api_call 只给前 20 条并截断，api_read 能列全并读全文。
    struct ManyApi;

    impl PiApis for ManyApi {
        fn search(&self, _: &str, _: usize) -> Result<Vec<PiApiHit>, String> {
            Ok(Vec::new())
        }

        fn call(&self, api: &str, _: &Map<String, Value>) -> Result<PiApiCall, String> {
            assert_eq!(api, "many");
            Ok(PiApiCall {
                name: "长结果".into(),
                total: 25,
                items: (0..25)
                    .map(|index| PiApiItem {
                        title: format!("条目{index}"),
                        source: "统计系统".into(),
                        text: if index == 0 {
                            "长".repeat(MAX_API_ITEM_CHARS + 50)
                        } else {
                            format!("第{index}条正文")
                        },
                    })
                    .collect(),
            })
        }
    }

    fn many_context(dir: &std::path::Path) -> PiToolContext {
        context("x")
            .with_apis(Arc::new(ManyApi))
            .with_api_cache_dir(dir.to_path_buf())
    }

    #[test]
    fn api_call_caches_long_results_and_api_read_pages_the_full_item() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = many_context(dir.path());
        let value = ctx.call("api_call", &json!({"api": "many"})).unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 20);
        assert_eq!(value["truncated"], true);
        let result_id = value["result_id"].as_str().unwrap().to_string();
        assert!(result_id.starts_with("api-"), "{result_id}");

        let list = ctx
            .call("api_read", &json!({"result_id": result_id}))
            .unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 25);
        assert_eq!(list["items"][0]["chars"], MAX_API_ITEM_CHARS + 50);

        let full = ctx
            .call("api_read", &json!({"result_id": result_id, "item": 0}))
            .unwrap();
        assert_eq!(full["total_chars"], MAX_API_ITEM_CHARS + 50);
        assert_eq!(
            full["text"].as_str().unwrap().chars().count(),
            MAX_API_ITEM_CHARS + 50
        );
        assert!(full["evidence"].as_str().unwrap().starts_with('E'));

        let later = ctx
            .call(
                "api_read",
                &json!({"result_id": result_id, "item": 0, "offset": 2000, "limit_chars": 100}),
            )
            .unwrap();
        assert_eq!(later["offset"], 2000);
        assert_eq!(
            later["text"].as_str().unwrap().chars().count(),
            MAX_API_ITEM_CHARS + 50 - 2000
        );
    }

    #[test]
    fn api_read_recovers_a_cached_result_from_the_run_directory() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = many_context(dir.path());
        let value = ctx.call("api_call", &json!({"api": "many"})).unwrap();
        let result_id = value["result_id"].as_str().unwrap().to_string();
        let recovered = many_context(dir.path());
        let list = recovered
            .call("api_read", &json!({"result_id": result_id}))
            .unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 25);
        let item = recovered
            .call("api_read", &json!({"result_id": result_id, "item": 1}))
            .unwrap();
        assert_eq!(item["text"], "第1条正文");
    }

    #[test]
    fn api_read_rejects_unknown_or_invalid_result_ids() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = many_context(dir.path());
        assert!(
            ctx.call("api_read", &json!({"result_id": "api-zzzz"}))
                .is_err()
        );
        assert!(
            ctx.call("api_read", &json!({"result_id": "../run.json"}))
                .is_err(),
            "result_id 不能当路径用"
        );
    }
}
