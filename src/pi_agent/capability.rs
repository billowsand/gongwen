//! Pi 合成校准档案：只存非敏感身份、合成证据摘要与保守策略结论。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(crate) const EVIDENCE_VERSION: u32 = 3;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 32;
const CALIBRATION_REQUEST: &str = "仅用本轮材料起草一份完整通知。先用 kb_search 查询“综合窗口”，再用 kb_read 阅读命中的知识库文章，从中提取综合窗口办理时间延长至哪一天；用 ms_search 查询“综合窗口”，再用 ms_read 阅读命中的旧稿，从中提取办理地点（楼、层、窗口号）。日期和地点必须以实际读取的资料为准，不猜测。正文写明办理日期与办理地点；不要写测试说明，也不要写周末安排。交完整 Markdown 正文。";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CapabilityWindowSource {
    NativeLoaded,
    Service,
    Observed,
    Manual,
    ModelTable,
    Default,
}

impl CapabilityWindowSource {
    pub(crate) fn from_window(source: crate::lmstudio::context::WindowSource) -> Self {
        use crate::lmstudio::context::WindowSource;
        match source {
            WindowSource::NativeLoaded => Self::NativeLoaded,
            WindowSource::Service => Self::Service,
            WindowSource::Observed => Self::Observed,
            WindowSource::Manual => Self::Manual,
            WindowSource::ModelTable => Self::ModelTable,
            WindowSource::Default => Self::Default,
        }
    }

    pub(crate) fn is_actual_loaded(self) -> bool {
        self == Self::NativeLoaded
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::NativeLoaded => "LM Studio 已加载窗口",
            Self::Service => "服务自报窗口（非已加载实测）",
            Self::Observed => "服务端超长反馈窗口（非已加载实测）",
            Self::Manual => "手动窗口上限（非已加载实测）",
            Self::ModelTable => "按模型名估算窗口（非已加载实测）",
            Self::Default => "默认估算窗口（非已加载实测）",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityIdentity {
    /// 规范化后的服务地址，已移除 userinfo、query 与 fragment。
    pub(crate) endpoint: String,
    pub(crate) model_id: String,
    pub(crate) context_window: u32,
    /// 区分已加载实测窗口与服务报告 / 手动值 / 估值，防止同数字跨来源复用。
    pub(crate) window_source: CapabilityWindowSource,
    pub(crate) thinking: String,
    pub(crate) evidence_version: u32,
    pub(crate) host_version: String,
    pub(crate) skill_digest: String,
}

impl CapabilityIdentity {
    pub(crate) fn new(
        endpoint: &str,
        model_id: &str,
        context_window: u32,
        window_source: CapabilityWindowSource,
        thinking: &str,
        skill_digest: &str,
    ) -> Result<Self, String> {
        let endpoint = safe_endpoint(endpoint)?;
        let model_id = model_id.trim().to_string();
        if model_id.is_empty() || model_id.chars().count() > 256 {
            return Err("Pi 校准身份里的模型编号为空或过长。".into());
        }
        Ok(Self {
            endpoint,
            model_id,
            context_window,
            window_source,
            thinking: thinking.chars().take(32).collect(),
            evidence_version: EVIDENCE_VERSION,
            host_version: super::protocol::HOST_VERSION.into(),
            skill_digest: skill_digest.chars().take(64).collect(),
        })
    }

    pub(crate) fn key(&self) -> String {
        let encoded = serde_json::to_vec(self).unwrap_or_default();
        Sha256::digest(encoded)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

fn safe_endpoint(value: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(value.trim()).map_err(|_| "Pi 模型地址不是有效 URL。".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Pi 模型地址必须是 http 或 https。".into());
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityConclusion {
    /// 只有专门的长输入与远距事实检查通过时才允许设置。
    pub(crate) verified_work_budget: Option<u32>,
    /// None 表示证据不足；不能把网络失败解释成短步骤偏好。
    pub(crate) needs_short_steps: Option<bool>,
    pub(crate) recommendation: String,
    pub(crate) tool_test_passed: bool,
    pub(crate) regular_success: u16,
    pub(crate) phased_success: u16,
    pub(crate) paired_cases: u16,
    #[serde(default)]
    pub(crate) strategy_modes: Vec<String>,
    #[serde(default)]
    pub(crate) elapsed_ms: u64,
    #[serde(default)]
    pub(crate) technical_failures: u16,
    pub(crate) long_context_verified: bool,
    pub(crate) completed_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityProfile {
    pub(crate) identity: Option<CapabilityIdentity>,
    pub(crate) conclusion: CapabilityConclusion,
    #[serde(default)]
    pub(crate) evidence_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalibrationEvidence {
    pub(crate) version: u32,
    pub(crate) task: String,
    pub(crate) materials: Vec<String>,
    pub(crate) knowledge_fact: String,
    pub(crate) manuscript: String,
    pub(crate) runs: Vec<CalibrationRunEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalibrationRunEvidence {
    pub(crate) strategy: String,
    pub(crate) actual_mode: Option<String>,
    pub(crate) duration_ms: u64,
    pub(crate) technical_success: bool,
    pub(crate) tool_coverage: bool,
    pub(crate) machine_judgment: bool,
    pub(crate) delivery: Option<String>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

pub(crate) fn save_evidence(
    directory: &Path,
    evidence: &CalibrationEvidence,
) -> Result<String, String> {
    const MAX_EVIDENCE_BYTES: usize = 256 * 1024;
    const MAX_EVIDENCE_CHARS: usize = 16 * 1024;
    if evidence.runs.len() > 4
        || evidence.materials.len() > 4
        || evidence.runs.iter().any(|run| {
            run.delivery
                .as_ref()
                .is_some_and(|text| text.len() > MAX_EVIDENCE_CHARS)
        })
    {
        return Err("校准证据超过运行数或单篇成品上限。".into());
    }
    let bytes =
        serde_json::to_vec_pretty(evidence).map_err(|e| format!("组装校准证据失败：{e}"))?;
    if bytes.len() > MAX_EVIDENCE_BYTES {
        return Err("校准证据超过 256 KiB 上限。".into());
    }
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    std::fs::create_dir_all(directory).map_err(|e| format!("创建校准证据目录失败：{e}"))?;
    let path = directory.join(format!("{digest}.json"));
    crate::vault::write_path(&path, "pi", &bytes)
        .map_err(|e| format!("保存校准原始证据失败：{e:#}"))?;
    Ok(digest)
}

pub(crate) fn verify_evidence(directory: &Path, digest: &str) -> bool {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    let path = directory.join(format!("{digest}.json"));
    let Ok(metadata) = std::fs::metadata(&path) else {
        return false;
    };
    if metadata.len() > MAX_FILE_BYTES + crate::vault::SEALED_FILE_OVERHEAD as u64 {
        return false;
    }
    let Ok(bytes) = crate::vault::read_path(&path, "pi", MAX_FILE_BYTES) else {
        return false;
    };
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        == digest
}

/// 一轮合成校准的机器判定。
///
/// `timed_out` 把「跑完但没做对」与「超时没跑完」分开：超时既不算能力差（不能据此
/// 认定模型需要分阶段），也绝不算成功——它只是没有形成可用观察。
#[derive(Debug, Clone, Default)]
struct RunMetric {
    /// host 报的实际执行模式（regular / phased）。
    mode: Option<String>,
    /// 程序判定这轮成品是否包含合成事实、且没有编造周末安排。
    success: bool,
    /// 四个检索 / 读取工具是否都用过。
    tools_ok: bool,
    elapsed_ms: u64,
    /// 技术执行是否跑完（超时、网络失败、取消都算否）。
    technical_success: bool,
    /// 是否因单轮执行超时结束。
    timed_out: bool,
}

/// 单轮执行超时。分阶段模式两轮跑同一篇，原来的 120 秒会把「慢」误判成
/// 「能力不足」，所以放宽到 300 秒。
const ROUND_TIMEOUT: Duration = Duration::from_secs(300);
/// 一次校准的总时限（含四轮）。
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

fn timed_out_count(metrics: &[RunMetric]) -> u16 {
    metrics.iter().filter(|run| run.timed_out).count() as u16
}

fn completed_count(metrics: &[RunMetric]) -> u16 {
    metrics.iter().filter(|run| !run.timed_out).count() as u16
}

fn has_reliable_paired_coverage(metrics: &[RunMetric]) -> bool {
    metrics.len() == 4
        && metrics
            .iter()
            .all(|run| run.mode.is_some() && run.tools_ok && run.technical_success)
        && metrics[0].mode.as_deref() == Some("regular")
        && metrics[1].mode.as_deref() == Some("phased")
        && metrics[2].mode.as_deref() == Some("regular")
        && metrics[3].mode.as_deref() == Some("phased")
}

/// 是否两组都显示「分阶段更好」。
///
/// 额外要求没有任何一轮超时：超时说明这轮没跑完，不能当作「常规模式能力不够」的
/// 证据；它只是没有形成策略观察。
fn phased_was_better(paired: bool, metrics: &[RunMetric]) -> bool {
    paired
        && metrics.iter().all(|run| !run.timed_out)
        && !metrics[0].success
        && metrics[1].success
        && !metrics[2].success
        && metrics[3].success
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityFile {
    version: u32,
    entries: Vec<CapabilityProfile>,
}

impl CapabilityProfile {
    pub(crate) fn matches(&self, identity: &CapabilityIdentity) -> bool {
        self.identity.as_ref() == Some(identity)
    }

    pub(crate) fn init_values(&self) -> (Option<u32>, bool) {
        (
            self.identity
                .as_ref()
                .filter(|identity| identity.window_source.is_actual_loaded())
                .and_then(|identity| {
                    self.conclusion
                        .long_context_verified
                        .then_some(self.conclusion.verified_work_budget)
                        .flatten()
                        .map(|budget| budget.min(identity.context_window))
                        .filter(|budget| *budget > 0)
                }),
            self.conclusion.paired_cases >= 2
                && self.conclusion.tool_test_passed
                && self.conclusion.technical_failures == 0
                && self.conclusion.needs_short_steps == Some(true),
        )
    }
}

pub(crate) fn load(path: &Path) -> Result<Vec<CapabilityProfile>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = std::fs::metadata(path).map_err(|e| format!("读取 Pi 能力档案状态失败：{e}"))?;
    if metadata.len() > MAX_FILE_BYTES + crate::vault::SEALED_FILE_OVERHEAD as u64 {
        return Err("Pi 能力档案超过 256 KiB 上限，已保守忽略。".into());
    }
    let bytes = crate::vault::read_path(path, "pi", MAX_FILE_BYTES)
        .map_err(|e| format!("读取 Pi 能力档案失败：{e:#}"))?;
    let file: CapabilityFile =
        serde_json::from_slice(&bytes).map_err(|e| format!("Pi 能力档案损坏，已保守忽略：{e}"))?;
    if file.version != EVIDENCE_VERSION || file.entries.len() > MAX_ENTRIES {
        return Err("Pi 能力档案版本或条目数不受支持，已保守忽略。".into());
    }
    Ok(file.entries)
}

pub(crate) fn save(path: &Path, entries: &[CapabilityProfile]) -> Result<(), String> {
    if entries.len() > MAX_ENTRIES {
        return Err("Pi 能力档案条目超过上限。".into());
    }
    let file = CapabilityFile {
        version: EVIDENCE_VERSION,
        entries: entries.to_vec(),
    };
    let bytes =
        serde_json::to_vec_pretty(&file).map_err(|e| format!("组装 Pi 能力档案失败：{e}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Pi 能力档案超过 256 KiB 上限。".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 Pi 能力目录失败：{e}"))?;
    }
    crate::vault::write_path(path, "pi", &bytes).map_err(|e| format!("保存 Pi 能力档案失败：{e:#}"))
}

/// 生产能力测试：所有写入都在系统临时目录，资料源是程序自造的 SQLite/FTS。
/// 一次同题 auto/phased 对照，只能产生策略观察，绝不产生 verifiedWorkBudget。
pub(crate) fn run_paired_calibration(
    model: super::PiModel,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(String),
) -> Result<(CapabilityProfile, CalibrationEvidence), String> {
    use super::{
        MaterialRole, PiBridge, PiJob, PiMaterialInput, PiOutcome, PiRunLog, PiRunRecord,
        PiRuntime, PiTask, PiToolContext, PiWorkspace, run,
    };
    use crate::models::{AppConfig, DraftInput, RagConfig, RerankMode, TemplateKind};
    use crate::pi_agent::tools::PiSources;

    // 校准也留在当前槽位的 Pi 管理目录里：加密模式下 host 的 GWA2 逻辑路径
    // 必须以配置目录为根，不能因为工作区位于系统临时目录就退化成明文。
    let root = crate::storage::slot_subdir("pi")
        .map_err(|error| format!("创建校准目录失败：{error:#}"))?
        .join("calibration")
        .join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&root).map_err(|error| format!("创建临时校准目录失败：{error}"))?;
    struct CalibrationDir(std::path::PathBuf);
    impl Drop for CalibrationDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = CalibrationDir(root.clone());
    let kb_path = root.join("knowledge.sqlite");
    let ms_path = root.join("manuscripts.sqlite");
    let knowledge_doc = {
        let mut store = crate::knowledge::KnowledgeStore::open(&kb_path)
            .map_err(|error| format!("创建临时知识库失败：{error:#}"))?;
        let id = store.import_document(&crate::knowledge::KnowledgeImportItem {
            source: crate::knowledge::KnowledgeSource::Markdown,
            source_manuscript_id: None,
            source_path: "synthetic/extension.md".into(),
            kind: TemplateKind::OfficialLetter,
            title: "古杉服务窗口延长安排（合成材料）".into(),
            content_markdown: "# 古杉服务窗口延长安排\n\n## 时间\n\n古杉市政务服务中心综合窗口办理时间延长至2031年3月16日，周末安排不在材料范围内。\n".into(),
        }).map_err(|error| format!("写入临时知识库失败：{error:#}"))?;
        let text =
            "古杉市政务服务中心综合窗口办理时间延长至2031年3月16日，周末安排不在材料范围内。";
        store
            .replace_chunks(
                id,
                &[crate::knowledge::NewKnowledgeChunk {
                    ord: 0,
                    section: "时间".into(),
                    text: text.into(),
                    tokens: crate::rag::tokenize(text),
                    embedding: None,
                    dims: 0,
                }],
                "synthetic-fts-only",
            )
            .map_err(|error| format!("建立临时知识库 FTS 索引失败：{error:#}"))?;
        id
    };
    let manuscript_id = {
        let mut store = crate::manuscript::ManuscriptStore::open(&ms_path)
            .map_err(|error| format!("创建临时稿件库失败：{error:#}"))?;
        let draft = DraftInput {
            kind: TemplateKind::PlainDocument,
            title_hint: "古杉市综合窗口服务调整通知（合成旧稿）".into(),
            ..Default::default()
        };
        store.create(&crate::manuscript::NewManuscript {
            snapshot: draft,
            content_markdown: "# 古杉市综合窗口服务调整通知\n\n办理地点为北楼3层第6号窗口。该地点仅用于合成测试。\n".into(),
            status: crate::models::ManuscriptStatus::Draft,
            ..Default::default()
        }, None).map_err(|error| format!("写入临时稿件库失败：{error:#}"))?
    };
    let rag = RagConfig {
        enabled: true,
        embedding: crate::models::EmbeddingConfig {
            model: "synthetic-unavailable-embedding".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            timeout_seconds: 1,
            ..Default::default()
        },
        rerank: crate::models::RerankConfig {
            mode: RerankMode::None,
            ..Default::default()
        },
        ..Default::default()
    };
    let chat = crate::models::LmStudioConfig::default();

    let runtime = PiRuntime::locate()?;
    let material = PiMaterialInput {
        title: "本轮自造事项要求".into(),
        source: "程序合成校准材料".into(),
        role: MaterialRole::Fact,
        text: "事项为优化古杉市综合窗口服务。请起草一份简短通知；未给出的周末安排不得推断。".into(),
    };
    let request = CALIBRATION_REQUEST.to_string();
    let skills = super::pi_skills::SkillSnapshot::build_builtin(Some("gongwen-draft"))?;
    // 先证明临时资料可检索/读取，准备错误不能算到模型能力上。
    let probe = PiToolContext::with_all_sources(
        Arc::new(AppConfig::default()),
        DraftInput::default(),
        String::new(),
        String::new(),
        Vec::new(),
        PiSources {
            knowledge: Arc::new(CalibrationKnowledge {
                path: kb_path.clone(),
                rag: rag.clone(),
                chat: chat.clone(),
            }),
            manuscripts: Arc::new(CalibrationManuscripts {
                path: ms_path.clone(),
            }),
            styles: Arc::new(CalibrationStyles),
        },
    );
    let kb_hits = probe.call("kb_search", &serde_json::json!({"query":"综合窗口"}))?;
    let ms_hits = probe.call("ms_search", &serde_json::json!({"query":"综合窗口"}))?;
    if !kb_hits["results"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["doc_id"] == knowledge_doc))
        || !ms_hits["results"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == manuscript_id))
    {
        return Err(format!(
            "临时资料检索预检失败（知识库{}条、稿件库{}条），尚未测试模型能力。",
            kb_hits["results"].as_array().map_or(0, Vec::len),
            ms_hits["results"].as_array().map_or(0, Vec::len)
        ));
    }
    let current = "";
    let mut run_metrics: Vec<RunMetric> = Vec::new();
    let mut run_evidence = Vec::new();
    let started_all = std::time::Instant::now();
    let mut skill_digest = String::new();
    let preferences = [
        super::strategy::PiStrategyPreference::Auto,
        super::strategy::PiStrategyPreference::Phased,
        super::strategy::PiStrategyPreference::Auto,
        super::strategy::PiStrategyPreference::Phased,
    ];
    for (index, preference) in preferences.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Pi 能力测试已停止。".into());
        }
        if started_all.elapsed() >= TOTAL_TIMEOUT {
            return Err(format!(
                "Pi 能力测试超过 {} 分钟总时限，未形成完整对照。",
                TOTAL_TIMEOUT.as_secs() / 60
            ));
        }
        progress(
            if preference == super::strategy::PiStrategyPreference::Auto {
                "正在跑合成同题对照（自动策略）…"
            } else {
                "正在跑合成同题对照（倾向分阶段）…"
            }
            .into(),
        );
        let docs_root = root.join("workspaces");
        let material_list = vec![
            material.clone(),
            PiMaterialInput {
                title: "第二份独立事实核对材料".into(),
                source: "程序合成材料".into(),
                role: MaterialRole::Fact,
                text: "通知对象为古杉市各窗口工作人员，按现行窗口流程办理。".into(),
            },
        ];
        let (workspace, metas) = PiWorkspace::prepare_with_snapshot(
            &docs_root,
            index as u64 + 1,
            current,
            &request,
            &material_list,
            &skills,
        )?;
        skill_digest = super::pi_skills::content_digest(&workspace.root)?;
        let input = workspace.read_request()?;
        let run_id = uuid::Uuid::new_v4().to_string();
        let request_id = format!("capability-{index}-{}", uuid::Uuid::new_v4());
        let record = PiRunRecord::start(
            run_id.clone(),
            request_id.clone(),
            &workspace,
            current,
            current,
            current,
            &input,
            None,
            &request,
            "通知",
            "2030-01-01",
            Some("gongwen-draft"),
            None,
            metas.clone(),
            "能力测试",
            &model.model_id,
        )
        .with_strategy(preference);
        let record = Arc::new(PiRunLog::open(&workspace, record)?);
        let tools = Arc::new(PiToolContext::with_all_sources(
            Arc::new(AppConfig::default()),
            DraftInput {
                kind: TemplateKind::PlainDocument,
                ..Default::default()
            },
            current.into(),
            request.clone(),
            material_list.iter().map(|item| item.text.clone()).collect(),
            PiSources {
                knowledge: Arc::new(CalibrationKnowledge {
                    path: kb_path.clone(),
                    rag: rag.clone(),
                    chat: chat.clone(),
                }),
                manuscripts: Arc::new(CalibrationManuscripts {
                    path: ms_path.clone(),
                }),
                styles: Arc::new(CalibrationStyles),
            },
        ));
        let strategy = Arc::new(std::sync::Mutex::new(None));
        let strategy_seen = strategy.clone();
        let started = std::time::Instant::now();
        let outcome = run(
            PiJob {
                runtime: runtime.clone(),
                workspace,
                model: model.clone(),
                task: PiTask {
                    request: request.clone(),
                    kind: "通知".into(),
                    system_date: "2030-01-01".into(),
                    skill: Some("gongwen-draft".into()),
                    strategy: preference,
                    capability: super::strategy::PiCapabilitySnapshot::default(),
                    selection: None,
                    materials: metas,
                },
                before: current.into(),
                base: current.into(),
                tools: tools.clone(),
                bridge: Arc::new(PiBridge::default()),
                record,
                resume: false,
                request_id,
                run_id,
                timeout: ROUND_TIMEOUT.min(TOTAL_TIMEOUT.saturating_sub(started_all.elapsed())),
                // 校准只连本地临时资料与模型服务，不经过用户代理。
                proxy_env: Vec::new(),
            },
            cancel.clone(),
            |event| {
                if let super::PiEvent::Strategy(info) = event {
                    *strategy_seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(info.mode);
                }
            },
        );
        let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let mode = strategy.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let technical_success = matches!(outcome, Ok(PiOutcome::Document(_)));
        let content = match &outcome {
            Ok(PiOutcome::Document(delivery)) => Some(delivery.content.clone()),
            Ok(PiOutcome::Report(_) | PiOutcome::Style(_)) | Err(_) => None,
        };
        let tools_ok = tools.evidence().iter().any(|item| item.tool == "kb_search")
            && tools.evidence().iter().any(|item| item.tool == "kb_read")
            && tools.evidence().iter().any(|item| item.tool == "ms_search")
            && tools.evidence().iter().any(|item| item.tool == "ms_read");
        let success = content.as_deref().is_some_and(calibration_facts_present) && tools_ok;
        // 超时不是「能力不足」：单独记下来，既不据此偏好分阶段，也绝不算成功。
        let timed_out = outcome
            .as_ref()
            .err()
            .is_some_and(|error| error.contains("Pi 智能写作超时"));
        run_evidence.push(CalibrationRunEvidence {
            strategy: if preference == super::strategy::PiStrategyPreference::Auto {
                "auto"
            } else {
                "phased"
            }
            .into(),
            actual_mode: mode.clone(),
            duration_ms: elapsed_ms,
            technical_success,
            tool_coverage: tools_ok,
            machine_judgment: success,
            // 原始成品不截断；保存证据时按体积明确拒绝，不能用半稿冒充完整证据。
            delivery: content,
            error: outcome.as_ref().err().map(|error| {
                let mut error = error.replace(&model.base_url, "[模型服务]");
                if !model.api_key.is_empty() {
                    error = error.replace(&model.api_key, "[已隐藏]");
                }
                error.chars().take(512).collect()
            }),
        });
        run_metrics.push(RunMetric {
            mode,
            success,
            tools_ok,
            elapsed_ms,
            technical_success,
            timed_out,
        });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Pi 能力测试已停止，未形成或保存结论。".into());
    }
    let context_window = model.context_window;
    let key_identity = CapabilityIdentity::new(
        &model.base_url,
        &model.model_id,
        context_window,
        model.window_source,
        if model.reasoning {
            model.thinking_level.as_deref().unwrap_or("enabled")
        } else {
            "default"
        },
        &skill_digest,
    )?;
    let paired = has_reliable_paired_coverage(&run_metrics);
    let phased_better = phased_was_better(paired, &run_metrics);
    let timeouts = timed_out_count(&run_metrics);
    let completed = completed_count(&run_metrics);
    let profile = CapabilityProfile {
        identity: Some(key_identity),
        // 校准只做一轮短任务观察，不产生已验证工作预算（也不验证长上下文）：
        // verified_work_budget 保持 None、long_context_verified 保持 false。
        conclusion: CapabilityConclusion {
            verified_work_budget: None,
            needs_short_steps: phased_better.then_some(true),
            recommendation: if paired {
                format!(
                    "两组合成同题对照：自动 {} / 2，分阶段 {} / 2；超时 {timeouts} 轮 / 完成 {completed} 轮；{}。",
                    run_metrics
                        .iter()
                        .step_by(2)
                        .filter(|run| run.success)
                        .count(),
                    run_metrics
                        .iter()
                        .skip(1)
                        .step_by(2)
                        .filter(|run| run.success)
                        .count(),
                    if phased_better {
                        "两组均显示分阶段更适合短任务"
                    } else {
                        "本次未形成策略偏好"
                    }
                )
            } else {
                format!(
                    "网络/host失败或未取得有效常规/分阶段成对结果（超时 {timeouts} 轮 / 完成 {completed} 轮）；不形成策略结论。"
                )
            },
            tool_test_passed: run_metrics.iter().all(|run| run.tools_ok),
            regular_success: run_metrics
                .iter()
                .step_by(2)
                .filter(|run| run.success)
                .count() as u16,
            phased_success: run_metrics
                .iter()
                .skip(1)
                .step_by(2)
                .filter(|run| run.success)
                .count() as u16,
            paired_cases: if paired { 2 } else { 0 },
            strategy_modes: run_metrics
                .iter()
                .filter_map(|run| run.mode.clone())
                .collect(),
            elapsed_ms: run_metrics.iter().map(|run| run.elapsed_ms).sum(),
            technical_failures: run_metrics
                .iter()
                .filter(|run| !run.technical_success)
                .count() as u16,
            long_context_verified: false,
            completed_at: chrono::Local::now().to_rfc3339(),
        },
        evidence_digest: String::new(),
    };
    let evidence = CalibrationEvidence {
        version: EVIDENCE_VERSION,
        task: request,
        materials: vec![
            material.text,
            "通知对象为古杉市各窗口工作人员，按现行窗口流程办理。".into(),
        ],
        knowledge_fact:
            "古杉市政务服务中心综合窗口办理时间延长至2031年3月16日，周末安排不在材料范围内。".into(),
        manuscript: "办理地点为北楼3层第6号窗口。该地点仅用于合成测试。".into(),
        runs: run_evidence,
    };
    Ok((profile, evidence))
}

fn calibration_facts_present(text: &str) -> bool {
    let comparable: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    comparable.contains("2031年3月16日")
        && comparable.contains("北楼3层第6号窗口")
        && !comparable.contains("周末")
        && !comparable.contains("合成测试")
}

struct CalibrationKnowledge {
    path: std::path::PathBuf,
    rag: crate::models::RagConfig,
    chat: crate::models::LmStudioConfig,
}
impl super::tools::PiKnowledge for CalibrationKnowledge {
    fn enabled(&self) -> bool {
        true
    }
    fn search(&self, query: &str, top: usize) -> Result<super::tools::PiSearchOutcome, String> {
        let outcome = crate::rag::retrieve(&self.rag, &self.chat, &self.path, query, None)
            .map_err(|error| format!("临时知识库检索失败：{error:#}"))?;
        Ok(super::tools::PiSearchOutcome {
            chunks: outcome
                .chunks
                .into_iter()
                .take(top)
                .map(|chunk| super::tools::PiChunk {
                    doc_id: chunk.doc_id,
                    title: chunk.doc_title,
                    section: chunk.section,
                    text: chunk.text,
                })
                .collect(),
            warnings: outcome.warnings,
        })
    }
    fn read(&self, doc_id: i64) -> Result<Option<(String, String)>, String> {
        crate::knowledge::KnowledgeStore::open(&self.path)
            .and_then(|mut store| store.get_doc_content(doc_id))
            .map_err(|error| format!("临时知识库读取失败：{error:#}"))
    }
}

struct CalibrationManuscripts {
    path: std::path::PathBuf,
}
impl super::tools::PiManuscripts for CalibrationManuscripts {
    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<super::tools::PiManuscriptRow>, String> {
        let mut store = crate::manuscript::ManuscriptStore::open(&self.path)
            .map_err(|error| format!("打开临时稿件库失败：{error:#}"))?;
        let filter = crate::manuscript::ManuscriptFilter {
            keyword: query.into(),
            ..Default::default()
        };
        store
            .list(&filter)
            .map(|rows| {
                rows.into_iter()
                    .take(limit)
                    .map(|row| super::tools::PiManuscriptRow {
                        id: row.id,
                        title: row.title,
                        kind: row.kind,
                        status: format!("{:?}", row.status),
                        doc_number: row.doc_number,
                        doc_date: row.doc_date,
                        version: None,
                        markdown: String::new(),
                    })
                    .collect()
            })
            .map_err(|error| format!("临时稿件库检索失败：{error:#}"))
    }
    fn read(
        &self,
        id: i64,
        _version: Option<i64>,
    ) -> Result<Option<super::tools::PiManuscriptRow>, String> {
        let mut store = crate::manuscript::ManuscriptStore::open(&self.path)
            .map_err(|error| format!("打开临时稿件库失败：{error:#}"))?;
        store
            .get(id)
            .map(|row| {
                row.map(|row| super::tools::PiManuscriptRow {
                    id: row.id,
                    title: row.title,
                    kind: row.kind,
                    status: format!("{:?}", row.status),
                    doc_number: row.doc_number,
                    doc_date: row.doc_date,
                    version: None,
                    markdown: row.content_markdown,
                })
            })
            .map_err(|error| format!("临时稿件库读取失败：{error:#}"))
    }
}

struct CalibrationStyles;
impl super::tools::PiStyles for CalibrationStyles {
    fn list(
        &self,
        _kind: crate::models::TemplateKind,
    ) -> Result<super::tools::PiStyleList, String> {
        Ok(super::tools::PiStyleList {
            rows: Vec::new(),
            truncated: false,
        })
    }
    fn read(
        &self,
        _id: &str,
        _kind: crate::models::TemplateKind,
    ) -> Result<super::tools::PiStyleLookup, String> {
        Ok(super::tools::PiStyleLookup::Unavailable {
            reason: "unknown",
            message: "校准隔离区没有风格档案。".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(window: u32) -> CapabilityIdentity {
        CapabilityIdentity::new(
            "https://user:secret@example.test/v1/?token=private",
            "model",
            window,
            CapabilityWindowSource::NativeLoaded,
            "high",
            "skill-v1",
        )
        .unwrap()
    }

    #[test]
    fn identities_are_safe_and_context_window_specific() {
        let old = identity(16_128);
        let new = identity(262_144);
        assert!(!old.endpoint.contains("secret"));
        assert!(!old.endpoint.contains("token"));
        assert_ne!(old.key(), new.key());
        assert_ne!(old, identity(16_384));
        let same_estimate = CapabilityIdentity::new(
            "https://user:secret@example.test/v1/?token=private",
            "model",
            16_128,
            CapabilityWindowSource::ModelTable,
            "high",
            "skill-v1",
        )
        .unwrap();
        assert_ne!(old, same_estimate, "相同数值但来源不同不能串用");
    }

    #[test]
    fn short_evidence_never_becomes_a_verified_budget() {
        let profile = CapabilityProfile {
            identity: Some(identity(262_144)),
            conclusion: CapabilityConclusion {
                verified_work_budget: Some(96_000),
                needs_short_steps: Some(true),
                recommendation: "短测试覆盖不足".into(),
                regular_success: 1,
                phased_success: 1,
                paired_cases: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(profile.init_values(), (None, false));
    }

    #[test]
    fn paired_short_step_preference_is_independent_of_long_context_capacity() {
        let mut profile = CapabilityProfile {
            identity: Some(identity(262_144)),
            conclusion: CapabilityConclusion {
                paired_cases: 2,
                tool_test_passed: true,
                phased_success: 2,
                needs_short_steps: Some(true),
                verified_work_budget: Some(96_000),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(profile.init_values(), (None, true));
        profile.conclusion.technical_failures = 1;
        assert_eq!(profile.init_values(), (None, false));
    }

    #[test]
    fn estimated_windows_never_enable_a_verified_work_budget() {
        let profile = CapabilityProfile {
            identity: Some(
                CapabilityIdentity::new(
                    "https://example.test/v1",
                    "model",
                    128_000,
                    CapabilityWindowSource::Service,
                    "high",
                    "skill-v1",
                )
                .unwrap(),
            ),
            conclusion: CapabilityConclusion {
                verified_work_budget: Some(96_000),
                long_context_verified: true,
                paired_cases: 2,
                tool_test_passed: true,
                technical_failures: 0,
                needs_short_steps: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(profile.init_values(), (None, true));
    }

    #[test]
    fn retrieved_facts_allow_typographic_spaces_but_not_wrong_values_or_unasked_arrangements() {
        assert!(!CALIBRATION_REQUEST.contains("2031年3月16日"));
        assert!(!CALIBRATION_REQUEST.contains("北楼3层第6号窗口"));
        assert!(calibration_facts_present(
            "办理至2031 年 3 月 16 日，办理地点北楼3层第6号窗口。"
        ));
        assert!(!calibration_facts_present(
            "办理至2031年3月17日，办理地点北楼3层第6号窗口。"
        ));
        assert!(!calibration_facts_present(
            "办理至2031年3月16日，办理地点北楼3层第6号窗口，周末照常办理。"
        ));
        assert!(!calibration_facts_present("未找到资料。"));
    }

    #[test]
    fn profile_io_is_bounded_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pi/capability.json");
        let profile = CapabilityProfile {
            identity: Some(identity(16_128)),
            ..Default::default()
        };
        save(&path, std::slice::from_ref(&profile)).unwrap();
        assert_eq!(load(&path).unwrap(), vec![profile]);
        std::fs::write(&path, b"{").unwrap();
        assert!(load(&path).unwrap_err().contains("损坏"));
        std::fs::write(
            &path,
            vec![b' '; MAX_FILE_BYTES as usize + crate::vault::SEALED_FILE_OVERHEAD + 1],
        )
        .unwrap();
        assert!(load(&path).unwrap_err().contains("256 KiB"));
    }

    #[test]
    fn evidence_is_separate_bounded_and_hash_verified() {
        let dir = tempfile::tempdir().unwrap();
        let evidence = CalibrationEvidence {
            version: EVIDENCE_VERSION,
            task: "程序自造任务".into(),
            materials: vec!["自造材料".into()],
            knowledge_fact: "只有临时库里有的事实".into(),
            manuscript: "自造旧稿".into(),
            runs: vec![CalibrationRunEvidence {
                strategy: "auto".into(),
                actual_mode: Some("regular".into()),
                duration_ms: 10,
                technical_success: true,
                tool_coverage: true,
                machine_judgment: true,
                delivery: Some("完整合成正文".into()),
                error: None,
            }],
        };
        let digest = save_evidence(dir.path(), &evidence).unwrap();
        assert!(verify_evidence(dir.path(), &digest));
        std::fs::write(dir.path().join(format!("{digest}.json")), b"tampered").unwrap();
        assert!(!verify_evidence(dir.path(), &digest));
        assert!(!verify_evidence(dir.path(), "../../anything"));
    }

    fn metric(
        mode: Option<&str>,
        success: bool,
        tools_ok: bool,
        technical_success: bool,
    ) -> RunMetric {
        RunMetric {
            mode: mode.map(str::to_string),
            success,
            tools_ok,
            elapsed_ms: 10,
            technical_success,
            timed_out: false,
        }
    }

    #[test]
    fn transport_or_tool_failures_cannot_become_a_strategy_conclusion() {
        let failed = vec![metric(Some("regular"), false, false, false); 4];
        assert!(!has_reliable_paired_coverage(&failed));
        let missing_strategy = vec![metric(None, false, false, false); 4];
        assert!(!has_reliable_paired_coverage(&missing_strategy));
    }

    /// 超时不等于「能力不足」：常规两轮超时、分阶段两轮做对，也不能据此说模型
    /// 需要分阶段；同时超时轮次绝不算成功。
    #[test]
    fn a_timeout_is_neither_capability_evidence_nor_success() {
        let metrics = vec![
            RunMetric {
                timed_out: true,
                technical_success: false,
                ..metric(Some("regular"), false, true, false)
            },
            metric(Some("phased"), true, true, true),
            RunMetric {
                timed_out: true,
                technical_success: false,
                ..metric(Some("regular"), false, true, false)
            },
            metric(Some("phased"), true, true, true),
        ];
        assert_eq!(timed_out_count(&metrics), 2);
        assert_eq!(completed_count(&metrics), 2);
        assert!(
            !has_reliable_paired_coverage(&metrics),
            "超时的轮次不是有效对照"
        );
        assert!(
            !phased_was_better(false, &metrics),
            "超时不能算作常规模式能力差"
        );
        // 防御：即使某轮技术上「跑完」但被标记超时，也不参与策略偏好。
        let paired_but_timed_out = vec![
            RunMetric {
                timed_out: true,
                ..metric(Some("regular"), false, true, true)
            },
            metric(Some("phased"), true, true, true),
            metric(Some("regular"), false, true, true),
            metric(Some("phased"), true, true, true),
        ];
        assert!(has_reliable_paired_coverage(&paired_but_timed_out));
        assert!(!phased_was_better(true, &paired_but_timed_out));
    }

    /// 本地模型验收入口。明确 opt-in；运行前需由验收者设置模型地址/ID及 Pi runtime 环境。
    #[test]
    #[ignore = "需显式授权本机LM Studio并设置GONGWEN_PI_LIVE_CALIBRATION=1"]
    fn live_qwen_synthetic_sqlite_paired_calibration() {
        use crate::models::LmStudioConfig;
        if std::env::var("GONGWEN_PI_LIVE_CALIBRATION").as_deref() != Ok("1") {
            panic!("set GONGWEN_PI_LIVE_CALIBRATION=1 to opt in");
        }
        let base_url =
            std::env::var("GONGWEN_PI_CALIBRATION_URL").expect("GONGWEN_PI_CALIBRATION_URL");
        let model_id =
            std::env::var("GONGWEN_PI_CALIBRATION_MODEL").expect("GONGWEN_PI_CALIBRATION_MODEL");
        let api_key = std::env::var("GONGWEN_PI_CALIBRATION_API_KEY").unwrap_or_default();
        let mut chat = LmStudioConfig {
            base_url: base_url.clone(),
            model: model_id.clone(),
            api_key,
            timeout_seconds: 10,
            ..Default::default()
        };
        let actual = crate::lmstudio::context::window(&chat);
        assert!(
            actual.tokens > 0
                && actual.source == crate::lmstudio::context::WindowSource::NativeLoaded,
            "LM Studio did not report a loaded context window: {actual:?}"
        );
        let context_window = actual.tokens.min(u32::MAX as usize).max(1) as u32;
        let thinking =
            std::env::var("GONGWEN_PI_CALIBRATION_THINKING").unwrap_or_else(|_| "high".into());
        chat.reasoning_effort = match thinking.as_str() {
            "off" => crate::models::ReasoningEffort::Default,
            "low" => crate::models::ReasoningEffort::Low,
            "medium" => crate::models::ReasoningEffort::Medium,
            "high" => crate::models::ReasoningEffort::High,
            _ => crate::models::ReasoningEffort::High,
        };
        let model = super::super::PiModel {
            base_url,
            api_key: chat.api_key.clone(),
            model_id,
            context_window,
            window_source: CapabilityWindowSource::from_window(actual.source),
            max_tokens: 16_384.min(context_window.saturating_sub(1_024).max(1)),
            reasoning: thinking != "off",
            thinking_level: Some(thinking),
        };
        let (mut profile, evidence) =
            run_paired_calibration(model, Arc::new(AtomicBool::new(false)), |message| {
                eprintln!("{message}")
            })
            .expect("synthetic SQLite paired calibration");
        let temp_root = std::path::PathBuf::from(
            std::env::var("TEMP").unwrap_or_else(|_| "D:/gongwen/tmp".into()),
        )
        .join("pi-implementation/phase-6ab-live-evidence");
        profile.evidence_digest = save_evidence(&temp_root.join("evidence"), &evidence).unwrap();
        save(&temp_root.join("profile.json"), &[profile.clone()]).unwrap();
        eprintln!(
            "profile={}; evidence={}",
            temp_root.join("profile.json").display(),
            temp_root
                .join("evidence")
                .join(format!("{}.json", profile.evidence_digest))
                .display()
        );
        assert_eq!(evidence.runs.len(), 4);
        assert!(
            evidence.runs.iter().all(|run| run.tool_coverage),
            "all modes must retrieve isolated SQLite facts"
        );
        assert!(
            evidence
                .runs
                .iter()
                .all(|run| run.technical_success && run.machine_judgment),
            "all modes must deliver the retrieved facts in a valid document; inspect saved evidence for failures"
        );
    }
}
