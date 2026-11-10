//! 运行记录、恢复校验与交付回执。
//!
//! 每次 Pi 运行在运行目录下留下宿主记录（都在模型可写区域 `workspace/`
//! 之外）：
//!
//! ```text
//! run.json          Rust 写的运行记录：身份、基线、要求、材料、模型引用、状态
//! receipt.json      host 写的交付回执：成品快照路径、哈希、字节数、摘要
//! final.md          host 写的成品不可变快照（回执指向它）
//! sources/index-<哈希>.json 事实来源索引（单项在 sources/items/<哈希>.md）
//! pi/session/       Durable JSONL 会话目录（旧 session.sqlite 只作密封归档）
//! ```
//!
//! 记录只放元数据，不放密钥、模型地址与正文：正文快照在工作区 `input/` 里，
//! 事实来源单独保存，都有哈希。读坏记录返回中文问题，不崩窗口。

use super::delivery::sha256_hex;
use super::materials::PiMaterialMeta;
use super::protocol::{HOST_VERSION, PROTOCOL_VERSION};
use super::strategy::PiCapabilitySnapshot;
use super::strategy::PiStrategyPreference;
use super::workspace::PiWorkspace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 运行记录格式版本；旧版本记录按「不能恢复」处理并说明原因。
pub(crate) const RUN_VERSION: u32 = 1;
/// 运行记录文件的读取上限。
pub(crate) const MAX_RECORD_BYTES: u64 = 1024 * 1024;
/// 回执文件的读取上限。host 写回执前按 1 MiB 硬卡（清单最多 100 条），这里留一倍余量：
/// 不能出现「写得进、重启读不回」。
pub(crate) const MAX_RECEIPT_BYTES: u64 = 2 * 1024 * 1024;
/// 成品快照的读取上限（与交付上限一致）。
pub(crate) const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;
/// 恢复时读回的事实来源总量上限（所有单项相加）。
pub(crate) const MAX_SOURCES_BYTES: u64 = 8 * 1024 * 1024;
/// Durable JSONL 会话的主文件（相对 `pi/session/`）。旧版只有 `session.sqlite`，
/// 已经不能当 JSONL 会话续跑。
const DURABLE_SESSION_FILE: &str = "main.jsonl";
/// 旧版 Durable 会话库；只作密封归档，不参与恢复。
const LEGACY_SESSION_FILE: &str = "session.sqlite";
/// 一次状态纠正扫描最多看多少个运行目录；异常大的目录不能把界面卡住。
const MAX_RECONCILE_RUNS: usize = 4096;
/// 单条事实来源（材料全文 / 一次工具返回全文）的上限。材料单份 ≤512 KiB，
/// 整篇知识库读取 ≤160K 字符（中文约 480 KiB），2 MiB 足够覆盖且能挡住异常大项。
const MAX_SOURCE_ITEM_BYTES: u64 = 2 * 1024 * 1024;
/// 来源索引（一项一条哈希）的上限。
const MAX_SOURCES_INDEX_BYTES: u64 = 512 * 1024;
/// 每个运行只保留最近几个来源索引版本；更旧的索引删掉，避免目录无限增长。
const MAX_SOURCE_INDEX_VERSIONS: usize = 2;
/// 没被任何保留索引引用的项文件，超过这么久才删除（给并发恢复留余量）。
const SOURCE_ITEM_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// 已确认回答最多保留多少条。
pub(crate) const MAX_CONFIRMED: usize = 100;
/// 记录里最多留多少行证据摘要。
pub(crate) const MAX_EVIDENCE_LINES: usize = 200;
/// 一行证据摘要的显示上限。
const MAX_EVIDENCE_CHARS: usize = 500;

/// 一轮运行的状态。`Running` 是开跑时写的，进程死掉后读回来仍是它，界面据此
/// 判断「可以接着写」；`Delivered` 表示成品已交出，`Accepted` 表示成品已写进正文
/// （旧提案被接受与 Pi 直接写入都用它，语义都是「这一轮的成品已经进正文了」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PiRunStatus {
    #[default]
    Running,
    Interrupted,
    Failed,
    Parked,
    Delivered,
    Accepted,
}

impl PiRunStatus {
    /// 还能不能从这份运行记录恢复。交付过的还可以接着改（下一轮），已写进正文的不再恢复。
    pub(crate) fn recoverable(self) -> bool {
        !matches!(self, Self::Accepted)
    }
}

/// 恢复时用的选区：偏移量与原文哈希都校验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PiRunSelection {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) sha256: String,
}

/// 一份运行记录。字段都可有界序列化，读取时整份卡上限。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiRunRecord {
    #[serde(default = "run_version")]
    pub(crate) version: u32,
    pub(crate) run_id: String,
    pub(crate) request_id: String,
    pub(crate) protocol: u32,
    pub(crate) host_version: String,
    /// 工作区相对位置：`pi/workspaces/doc-1/run-…`，相对配置目录。
    pub(crate) workspace: String,
    /// 开跑时的正式正文哈希。
    pub(crate) baseline: String,
    /// 写作基准（未采纳候选稿）的哈希；没有候选稿时等于 `baseline`。
    pub(crate) base: String,
    /// `input/current.md` 的哈希，用来查快照完整性。
    pub(crate) input: String,
    /// `input/request.md` 的哈希（模型实际读到的要求文件，含技能与材料索引）。
    #[serde(default)]
    pub(crate) input_request: String,
    /// `input/candidate.md` 的哈希；None 表示本轮没有候选稿。
    #[serde(default)]
    pub(crate) candidate: Option<String>,
    pub(crate) request: String,
    /// 用户原始要求，与模型侧追加的技能、选区说明分开；复制成新任务时使用。
    #[serde(default)]
    pub(crate) user_request: Option<String>,
    pub(crate) request_sha256: String,
    pub(crate) kind: String,
    pub(crate) system_date: String,
    #[serde(default)]
    pub(crate) skill: Option<String>,
    /// 统一写作入口的策略倾向（自动 / 倾向分阶段）。恢复时**按记录值**走，不看输入框
    /// 当前选的是什么；旧记录没有这一项，按自动（与旧行为一致）。
    #[serde(default)]
    pub(crate) strategy: PiStrategyPreference,
    /// 启动时选中的能力档案快照；恢复严格沿用当次快照。
    #[serde(default)]
    pub(crate) capability: PiCapabilitySnapshot,
    #[serde(default)]
    pub(crate) selection: Option<PiRunSelection>,
    #[serde(default)]
    pub(crate) materials: Vec<PiMaterialMeta>,
    /// 只记模型引用的两个名字，不存地址与密钥。
    pub(crate) model_provider: String,
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) confirmed: Vec<String>,
    /// 用户回答的**纯答案值**（不是「问题：回答」的显示字符串，也不是模型题面里的
    /// 假设）。恢复时只把这些当作已确认事实。
    #[serde(default)]
    pub(crate) answers: Vec<String>,
    #[serde(default)]
    pub(crate) evidence: Vec<String>,
    #[serde(default)]
    pub(crate) status: PiRunStatus,
    /// 失败 / 停下的原因（给人看，已截断）。
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) receipt: Option<PiReceiptSummary>,
    /// 成品被交付检查拦下（程序侧拒绝）。「接着写」重放只会拿回同一份被拒稿，
    /// 恢复时这一项为真就不允许接着写，只能从头开一轮；成品本身留在轮次卡片里，
    /// 用户可显式取回或丢弃。旧记录没有此项，按否。
    #[serde(default)]
    pub(crate) delivery_rejected: bool,
    /// 本轮事实来源索引的哈希（`sources/index-<hash>.json`），用于会话重载后
    /// 继续改未采纳提案；旧运行仍可能是单一 `sources/<hash>.md`。
    #[serde(default)]
    pub(crate) sources: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

fn run_version() -> u32 {
    RUN_VERSION
}

/// 随会话存进稿件库的薄引用：完整记录留在工作区的 `pi/run.json`，会话里只存
/// 定位与校验所需的东西，避免把大段正文要求与证据搬进会话 JSON。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiRunRef {
    #[serde(default)]
    pub(crate) run_id: String,
    #[serde(default)]
    pub(crate) request_id: String,
    #[serde(default)]
    pub(crate) workspace: String,
    #[serde(default)]
    pub(crate) baseline: String,
    #[serde(default)]
    pub(crate) status: PiRunStatus,
    #[serde(default)]
    pub(crate) model_provider: String,
    #[serde(default)]
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) sources: Option<String>,
    #[serde(default)]
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) updated_at: String,
}

impl PiRunRef {
    pub(crate) fn from_record(record: &PiRunRecord) -> Self {
        Self {
            run_id: record.run_id.clone(),
            request_id: record.request_id.clone(),
            workspace: record.workspace.clone(),
            baseline: record.baseline.clone(),
            status: record.status,
            model_provider: record.model_provider.clone(),
            model: record.model.clone(),
            sources: record.sources.clone(),
            created_at: record.created_at.clone(),
            updated_at: record.updated_at.clone(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.run_id.is_empty() || self.workspace.is_empty()
    }

    pub(crate) fn recoverable(&self) -> bool {
        !self.is_empty() && self.status.recoverable()
    }
}

/// 回执摘要（也存进运行记录，避免重复读 `receipt.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiReceiptSummary {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) bytes: usize,
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) call_id: String,
}

/// host 落盘的交付回执。`document` 指向 `pi/final.md`，`report` 的条目在
/// `items` 里。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiReceipt {
    #[serde(default = "run_version")]
    pub(crate) version: u32,
    pub(crate) run_id: String,
    pub(crate) request_id: String,
    /// `document` 或 `report`。
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) sha256: String,
    #[serde(default)]
    pub(crate) bytes: usize,
    #[serde(default)]
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) call_id: String,
    #[serde(default)]
    pub(crate) items: Vec<super::protocol::PiReportItem>,
    #[serde(default)]
    pub(crate) at: String,
}

impl PiReceipt {
    pub(crate) fn summary(&self) -> PiReceiptSummary {
        PiReceiptSummary {
            kind: self.kind.clone(),
            path: self.path.clone(),
            sha256: self.sha256.clone(),
            bytes: self.bytes,
            summary: self.summary.clone(),
            call_id: self.call_id.clone(),
        }
    }
}

impl PiRunRecord {
    /// 开跑前写一份。`record.write` 必须在启动 host 之前完成。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        run_id: String,
        request_id: String,
        workspace: &PiWorkspace,
        formal: &str,
        base: &str,
        input: &str,
        input_request: &str,
        candidate: Option<&str>,
        request: &str,
        kind: &str,
        system_date: &str,
        skill: Option<&str>,
        selection: Option<PiRunSelection>,
        materials: Vec<PiMaterialMeta>,
        model_provider: &str,
        model: &str,
    ) -> Self {
        let now = chrono::Local::now().to_rfc3339();
        Self {
            version: RUN_VERSION,
            run_id,
            request_id,
            protocol: PROTOCOL_VERSION,
            host_version: HOST_VERSION.to_string(),
            workspace: workspace.label.clone(),
            baseline: sha256_hex(formal),
            base: sha256_hex(base),
            input: sha256_hex(input),
            input_request: sha256_hex(input_request),
            candidate: candidate.map(sha256_hex),
            request: request.to_string(),
            user_request: None,
            request_sha256: sha256_hex(request),
            kind: kind.to_string(),
            system_date: system_date.to_string(),
            skill: skill.map(str::to_string),
            strategy: PiStrategyPreference::Auto,
            capability: PiCapabilitySnapshot::default(),
            selection,
            materials,
            model_provider: model_provider.to_string(),
            model: model.to_string(),
            confirmed: Vec::new(),
            answers: Vec::new(),
            evidence: Vec::new(),
            status: PiRunStatus::Running,
            message: String::new(),
            receipt: None,
            delivery_rejected: false,
            sources: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// 原子写。记录里有正文要求与材料标题，写入前确认没超过整份上限。
    pub(crate) fn write(&self, workspace: &PiWorkspace) -> Result<(), String> {
        self.write_to_dir(&workspace.run)
    }

    /// 记下这一轮用户选的策略倾向。开跑前写记录时调它，恢复时照着记录走，
    /// 不会因为输入框后来换了选择而在中途把常规任务变成分阶段。
    pub(crate) fn with_strategy(mut self, strategy: PiStrategyPreference) -> Self {
        self.strategy = strategy;
        self
    }

    pub(crate) fn with_capability(mut self, capability: PiCapabilitySnapshot) -> Self {
        self.capability = capability;
        self
    }

    /// 按运行目录写（接受提案时只拿得到目录）。
    pub(crate) fn write_to_dir(&self, run_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("组装 Pi 运行记录失败：{error}"))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err("Pi 运行记录超过大小上限，已停止记录。".into());
        }
        crate::vault::write_path(&run_dir.join("run.json"), "pi", &bytes)
            .map_err(|error| format!("写不出 Pi 运行记录：{error:#}"))
    }

    /// 从运行目录读一份记录；坏记录返回可读原因，不 panic。
    pub(crate) fn read_from(run_dir: &Path) -> Result<Self, String> {
        let path = run_dir.join("run.json");
        let bytes = read_bounded(&path, MAX_RECORD_BYTES)?;
        let record: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Pi 运行记录读不出来（{}）：{error}", path.display()))?;
        if record.version != RUN_VERSION {
            return Err(format!(
                "Pi 运行记录版本是 {}，当前程序只认 {RUN_VERSION}；不能接着写。",
                record.version
            ));
        }
        Ok(record)
    }

    /// 记录与这次实际状态是否还对得上。返回 Err 表示不能恢复。
    pub(crate) fn check_resume(
        &self,
        label: &str,
        run_id: &str,
        formal: &str,
    ) -> Result<(), String> {
        self.check_request()?;
        if self.version != RUN_VERSION {
            return Err(format!(
                "Pi 运行记录版本是 {}，当前程序只认 {RUN_VERSION}。",
                self.version
            ));
        }
        if self.run_id != run_id {
            return Err("这条运行记录的 runId 与轮次对不上，不能接着写。".into());
        }
        if self.workspace != label {
            return Err("这条运行记录指向的工作区与轮次对不上，不能接着写。".into());
        }
        // 协议升级只切断旧 Durable 状态的续跑；已经落盘的交付回执与不可变成品
        // 仍可经新 host 重放。旧 session.sqlite 不当成 JSONL 打开。
        let receipt_only = self.receipt.is_some()
            && (self.status == PiRunStatus::Delivered || self.delivery_rejected);
        if self.protocol != PROTOCOL_VERSION && !receipt_only {
            return Err(format!(
                "这一轮是用协议 {} 跑的，当前 host 协议是 {PROTOCOL_VERSION}；不能在原工作区接着写，请「从头写」。",
                self.protocol
            ));
        }
        if self.host_version != HOST_VERSION && !receipt_only {
            return Err(format!(
                "这一轮用的是 Pi host {}，当前是 {HOST_VERSION}；不能在原工作区接着写，请「从头写」。",
                self.host_version
            ));
        }
        if self.baseline != sha256_hex(formal) {
            return Err(
                "正式正文在这一轮开始后改过，不能再接着写原来的工作区；请「从头写」或重新发送要求。"
                    .into(),
            );
        }
        if !self.status.recoverable() {
            return Err("这一轮的成品已经被接受，不需要再恢复。".into());
        }
        Ok(())
    }

    /// 记录里的要求原文哈希必须自洽。
    pub(crate) fn check_request(&self) -> Result<(), String> {
        if self.request_sha256 != sha256_hex(&self.request) {
            return Err("Pi 运行记录里的要求原文被改过，不能接着写。".into());
        }
        Ok(())
    }

    /// 选区的原文哈希必须对得上，否则模型改的已经不是同一段。
    pub(crate) fn check_selection(&self, formal: &str) -> Result<(), String> {
        let Some(selection) = &self.selection else {
            return Ok(());
        };
        let Some(text) = formal.get(selection.start..selection.end) else {
            return Err("锁定选区已经不在当前正文里，不能再接着写。".into());
        };
        if sha256_hex(text) != selection.sha256 {
            return Err("锁定选区的内容在这一轮开始后改过，不能再接着写。".into());
        }
        Ok(())
    }

    /// 材料文件必须与记录里的哈希、字节数一致。
    pub(crate) fn check_materials(&self, workspace: &PiWorkspace) -> Result<(), String> {
        let indexed = workspace.read_materials()?;
        if serde_json::to_value(indexed).map_err(|error| error.to_string())?
            != serde_json::to_value(&self.materials).map_err(|error| error.to_string())?
        {
            return Err("材料索引的用途或来源与本轮快照不一致，不能恢复。".into());
        }
        for material in &self.materials {
            if !safe_material_path(&material.file) {
                return Err(format!("运行记录里的材料路径不合法：{}", material.file));
            }
            let path = workspace.root.join(&material.file);
            let bytes = read_bounded(&path, super::materials::MAX_MATERIAL_BYTES as u64)?;
            if bytes.len() != material.bytes {
                return Err(format!(
                    "材料《{}》的体积变了，不能再接着写。",
                    material.title
                ));
            }
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("材料《{}》不是合法 UTF-8。", material.title))?;
            if sha256_hex(&text) != material.sha256 {
                return Err(format!(
                    "材料《{}》的内容变了，不能再接着写。",
                    material.title
                ));
            }
        }
        Ok(())
    }
}

/// 工作区快照完整性：正式快照、候选稿、技能与 Durable 会话。
pub(crate) fn check_workspace_files(
    workspace: &PiWorkspace,
    record: &PiRunRecord,
) -> Result<(), String> {
    if !workspace.root.is_dir() {
        return Err(format!(
            "Pi 工作区已经不在了（{}），不能接着写。",
            workspace.label
        ));
    }
    let current = read_bounded(&workspace.root.join("input/current.md"), MAX_SNAPSHOT_BYTES)?;
    let current =
        String::from_utf8(current).map_err(|_| "工作区里的正文快照不是 UTF-8".to_string())?;
    if sha256_hex(&current) != record.input {
        return Err("工作区里的正文快照与运行记录对不上，不能接着写。".into());
    }
    let request_file = read_bounded(&workspace.root.join("input/request.md"), MAX_SNAPSHOT_BYTES)?;
    let request_file =
        String::from_utf8(request_file).map_err(|_| "工作区里的要求文件不是 UTF-8".to_string())?;
    if record.input_request.is_empty() || sha256_hex(&request_file) != record.input_request {
        return Err("工作区里的要求文件与运行记录对不上，不能接着写。".into());
    }
    let candidate_path = workspace.root.join("input/candidate.md");
    match &record.candidate {
        Some(expected) => {
            let text = String::from_utf8(read_bounded(&candidate_path, MAX_SNAPSHOT_BYTES)?)
                .map_err(|_| "工作区里的候选稿不是 UTF-8".to_string())?;
            if sha256_hex(&text) != *expected {
                return Err("工作区里的候选稿与运行记录对不上，不能接着写。".into());
            }
        }
        None => {
            if candidate_path.exists() {
                return Err("工作区里多出了一份本轮没有的候选稿，不能接着写。".into());
            }
        }
    }
    // 先纠正「记录还停在 Running、本机其实已经没有活动宿主」的旧状态，再核对会话：
    // 这一条路径是「接着写」的恢复入口，状态纠正失败要如实返回，不能带着一条
    // 无法解释的 Running 记录继续。
    reconcile_run_record(&workspace.run)?;
    check_durable_session(workspace, record)?;
    for relative in [
        "input/request.md",
        "skills/pi-skills",
        "skills/gongwen-markdown",
    ] {
        if !workspace.root.join(relative).exists() {
            return Err(format!(
                "Pi 工作区缺了 {relative}，不能接着写；可以点「从头写」重新开始。"
            ));
        }
    }
    // 手选技能必须仍在原快照里；不因为全局目录后来改了名 / 停用了就换别的技能。
    if let Some(skill) = &record.skill {
        if !crate::pi_agent::pi_skills::valid_name(skill) {
            return Err("运行记录里的技能名不合法，不能接着写。".into());
        }
        if !workspace
            .root
            .join("skills/pi-skills")
            .join(skill)
            .join("SKILL.md")
            .is_file()
        {
            return Err(format!(
                "这一轮用的技能「{skill}」已经不在工作区快照里，不能接着写；可以点「从头写」重新开始。"
            ));
        }
    }
    verify_skill_snapshot(workspace)?;
    Ok(())
}

/// Durable 会话完整性。
///
/// 运行记录里已经有证据 / 来源 / 交付回执 / 已确认回答时，说明这一轮**确实跑过 host**，
/// 那么 `pi/session/main.jsonl` 必须还在且不是空文件。会话丢了却当成「没有会话、
/// 从空会话接着跑」，会把已登记的阶段成果与工具调用历史一起丢掉；宁可拒绝恢复、
/// 让用户显式「从头写」。旧版 `session.sqlite` 只作密封归档，不能当 JSONL 续跑。
fn check_durable_session(workspace: &PiWorkspace, record: &PiRunRecord) -> Result<(), String> {
    let main = workspace.state.join(DURABLE_SESSION_FILE);
    if session_main_present(&main)? {
        return Ok(());
    }
    // 旧协议 / 旧 host 的交付回执重放（docs/encrypted-resource-lifecycle.md 第六轮
    // 保留的兼容路径）：host 只读回执与成品快照就交回同一份结果，不打开 Durable
    // 会话，因此会话缺失不拦。除此之外一律要求 JSONL 会话在。
    if is_receipt_only_replay(record) {
        return Ok(());
    }
    let legacy = workspace.run.join("pi").join(LEGACY_SESSION_FILE);
    if legacy.is_file() {
        return Err(
            "这一轮只有旧版 SQLite 会话（pi/session.sqlite），当前版本改用 JSONL 会话，\
             不能接着写；可以点「从头写」重新开始。"
                .into(),
        );
    }
    if record_has_session_evidence(record) {
        return Err(
            "这一轮的 Durable 会话文件（pi/session/main.jsonl）缺失或为空，不能接着写；\
             可以点「从头写」重新开始。"
                .into(),
        );
    }
    // 记录里没有任何会话痕迹（例如开跑前就取消了）：host 还来不及写会话，
    // 接着写会开一份新的 JSONL 会话，语义上不丢东西。
    Ok(())
}

/// 旧协议 / 旧 host 的「现成交付回执重放」：只交回已落盘的成品，不开 Durable 会话。
fn is_receipt_only_replay(record: &PiRunRecord) -> bool {
    record.protocol != PROTOCOL_VERSION
        && record.receipt.is_some()
        && (record.status == PiRunStatus::Delivered || record.delivery_rejected)
}

/// `pi/session/main.jsonl` 是否存在且有内容。
///
/// 加密工作区里的会话文件是 GWA1/GWA2 封套，物理字节永远大于明文：只看「长度 > 0」
/// 会把一封空明文也当成有内容，所以按封套头把固定开销算进去。文件存在但读不出
/// 文件头（权限、I/O 错误）一样按不可用处理，并把原因写进错误串。
fn session_main_present(path: &Path) -> Result<bool, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "读不到这一轮的 Durable 会话文件（{}）：{error}",
                path.display()
            ));
        }
    };
    if !metadata.is_file() {
        return Ok(false);
    }
    let mut head = [0u8; 4];
    let read = {
        use std::io::Read as _;
        std::fs::File::open(path)
            .and_then(|mut file| file.read(&mut head))
            .map_err(|error| {
                format!(
                    "读不到这一轮的 Durable 会话文件（{}）：{error}",
                    path.display()
                )
            })?
    };
    // 空文件、比封套头还短的文件都算「没有会话」。
    if read < head.len() {
        return Ok(false);
    }
    let minimum = if crate::vault::is_sealed_file(&head) {
        crate::vault::SEALED_FILE_OVERHEAD as u64
    } else {
        0
    };
    Ok(metadata.len() > minimum)
}

/// 运行记录里是否留下了「这一轮确实跑过 host」的痕迹。状态码本身不算：
/// 开跑前取消也会写 Interrupted，那时会话还没建。
fn record_has_session_evidence(record: &PiRunRecord) -> bool {
    !record.evidence.is_empty()
        || record.sources.is_some()
        || record.receipt.is_some()
        || !record.confirmed.is_empty()
        || !record.answers.is_empty()
        || record.delivery_rejected
        || matches!(
            record.status,
            PiRunStatus::Delivered | PiRunStatus::Parked | PiRunStatus::Failed
        )
}

/// 恢复前按 `input/skills.json` 回验技能快照内容。文件名存在不等于内容还是原来那一份：
/// 备份工具、同步盘或手工改动都可能换掉文件。旧运行没有索引时不拦（保持兼容）。
fn verify_skill_snapshot(workspace: &PiWorkspace) -> Result<(), String> {
    let path = workspace.root.join("input/skills.json");
    if !path.exists() {
        return Ok(());
    }
    let bytes = read_bounded(&path, MAX_SOURCES_INDEX_BYTES)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Pi 技能索引读不出来，不能接着写：{error}"))?;
    let Some(skills) = value.get("skills").and_then(serde_json::Value::as_array) else {
        return Err("Pi 技能索引格式不正确，不能接着写。".into());
    };
    let mut total = 0u64;
    for skill in skills.iter().take(64) {
        let Some(dir) = skill.get("path").and_then(serde_json::Value::as_str) else {
            return Err("Pi 技能索引缺少目录，不能接着写。".into());
        };
        if !dir.starts_with("skills/pi-skills/")
            || dir.contains('\\')
            || dir
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err("Pi 技能索引里的目录不合法，不能接着写。".into());
        }
        let Some(files) = skill.get("files").and_then(serde_json::Value::as_array) else {
            return Err("Pi 技能索引缺少文件清单，不能接着写。".into());
        };
        for file in files.iter().take(256) {
            let Some(relative) = file.get("path").and_then(serde_json::Value::as_str) else {
                return Err("Pi 技能索引里的文件路径不合法，不能接着写。".into());
            };
            let Some(expected) = file.get("sha256").and_then(serde_json::Value::as_str) else {
                return Err("Pi 技能索引里的文件哈希缺失，不能接着写。".into());
            };
            if relative.is_empty()
                || relative.starts_with('/')
                || relative.contains('\\')
                || relative
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || expected.len() != 64
                || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("Pi 技能索引里的文件记录不合法，不能接着写。".into());
            }
            let file_path = workspace.root.join(dir).join(relative);
            let content = read_bounded(&file_path, MAX_SNAPSHOT_BYTES)?;
            total = total.saturating_add(content.len() as u64);
            if total > 128 * 1024 * 1024 {
                return Err(
                    "Pi 技能快照超过校验上限，不能接着写；可以点「从头写」重新开始。".into(),
                );
            }
            if sha256_hex_bytes(&content) != expected.to_ascii_lowercase() {
                return Err(format!(
                    "技能快照文件内容变了（{dir}/{relative}），不能接着写；可以点「从头写」重新开始。"
                ));
            }
        }
    }
    Ok(())
}

/// 交付前再确认「修改前基线」确实在盘上。
///
/// 直接写入正文时，撤销记录靠这份快照：读不到或与 `before` 不一致，就不能保证
/// 「先保留基线、再替换正文」，宁可拒绝交付也不形成无撤销记录的替换。
/// 比的是完整文本，不是哈希指针——指针相同不代表文件还在。
pub(crate) fn check_baseline_snapshot(workspace: &PiWorkspace, before: &str) -> Result<(), String> {
    let path = workspace.root.join("input/current.md");
    let bytes = read_bounded(&path, MAX_SNAPSHOT_BYTES)
        .map_err(|error| format!("本轮修改前基线快照不在了，不能直接写入正文：{error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "工作区里的正文快照不是 UTF-8，不能直接写入正文".to_string())?;
    if text != before {
        return Err("工作区里的修改前基线快照与本轮正文不一致，不能直接写入正文".into());
    }
    Ok(())
}

/// 成品已经写进正文：把运行记录标成 `Accepted`，这一轮不再出现在「接着写」里，
/// 重启后重放回执也不会再自动覆盖一次。旧记录里的 `Accepted` 就是这个含义，直接沿用。
pub(crate) fn consume_run(run: &PiRunRef) -> Result<(), String> {
    if run.is_empty() {
        return Err("这一轮没有可标记的 Pi 运行记录".into());
    }
    let root = resolve_workspace(&run.workspace)?;
    let Some(dir) = root.parent() else {
        return Err("Pi 运行目录定位失败".into());
    };
    let mut record = PiRunRecord::read_from(dir)?;
    if record.run_id != run.run_id {
        return Err("运行记录与这一轮的运行标识对不上，未标记".into());
    }
    record.status = PiRunStatus::Accepted;
    record.updated_at = chrono::Local::now().to_rfc3339();
    record.write_to_dir(dir)
}

/// 成品被程序拒收：标记运行记录。「接着写」重放只会拿回同一份被拒稿，
/// 标记后恢复入口只允许从头写；成品留在轮次卡片里，可显式取回或丢弃。
pub(crate) fn reject_run(run: &PiRunRef) -> Result<(), String> {
    if run.is_empty() {
        return Err("这一轮没有可标记的 Pi 运行记录".into());
    }
    let root = resolve_workspace(&run.workspace)?;
    let Some(dir) = root.parent() else {
        return Err("Pi 运行目录定位失败".into());
    };
    let mut record = PiRunRecord::read_from(dir)?;
    if record.run_id != run.run_id {
        return Err("运行记录与这一轮的运行标识对不上，未标记".into());
    }
    record.delivery_rejected = true;
    record.updated_at = chrono::Local::now().to_rfc3339();
    record.write_to_dir(dir)
}

/// 这一轮上次的成品是不是被交付检查拦下。读不到记录时按否处理，
/// 由恢复路径上的其它校验（基线、快照、版本）兜底。
pub(crate) fn run_delivery_rejected(run: &PiRunRef) -> bool {
    if run.is_empty() {
        return false;
    }
    let Ok(root) = resolve_workspace(&run.workspace) else {
        return false;
    };
    let Some(dir) = root.parent() else {
        return false;
    };
    PiRunRecord::read_from(dir)
        .map(|record| record.run_id == run.run_id && record.delivery_rejected)
        .unwrap_or(false)
}

/// 把工作区相对引用解析成真实目录，并确认它仍在配置目录的 Pi 工作区里。
pub(crate) fn resolve_workspace(label: &str) -> Result<PathBuf, String> {
    if label.len() > 512 || label.contains('\\') || label.contains('\0') || label.contains(':') {
        return Err("Pi 工作区引用写法不合法。".into());
    }
    let parts: Vec<&str> = label.split('/').collect();
    if parts.len() != 4
        || parts[0] != "pi"
        || parts[1] != "workspaces"
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err("Pi 工作区引用不在配置目录的 pi/workspaces 下。".into());
    }
    let base = super::workspace::base_dir()?;
    // label 标识运行目录；调用者需要的是模型工作区根，parent 才是宿主运行目录。
    let dir = base.join(parts[2]).join(parts[3]).join("workspace");
    checked_inside(&base, &dir)
}

fn checked_inside(base: &Path, dir: &Path) -> Result<PathBuf, String> {
    let base =
        std::fs::canonicalize(base).map_err(|error| format!("读不到 Pi 工作区根目录：{error}"))?;
    let real = std::fs::canonicalize(dir).map_err(|error| {
        format!(
            "Pi 工作区已经不在了（{}）：{error}",
            dir.file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        )
    })?;
    if !real.starts_with(&base) {
        return Err("Pi 工作区指向配置目录之外，已拒绝。".into());
    }
    Ok(real)
}

/// `input/materials/01.md` 这类程序生成的文件名。
fn safe_material_path(file: &str) -> bool {
    let Some(name) = file.strip_prefix("input/materials/") else {
        return false;
    };
    name.len() == 5
        && name.as_bytes()[0].is_ascii_digit()
        && name.as_bytes()[1].is_ascii_digit()
        && name.ends_with(".md")
}

/// 从运行目录读回执。没有回执返回 `Ok(None)`；坏回执返回 Err（宁可停下也不重复生成）。
pub(crate) fn read_receipt(run_dir: &Path, run_id: &str) -> Result<Option<PiReceipt>, String> {
    let path = run_dir.join("receipt.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_bounded(&path, MAX_RECEIPT_BYTES)?;
    let receipt: PiReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Pi 交付回执读不出来（{}）：{error}", path.display()))?;
    if receipt.version != RUN_VERSION {
        return Err(format!(
            "Pi 交付回执版本是 {}，当前程序只认 {RUN_VERSION}，已拒绝。",
            receipt.version
        ));
    }
    if receipt.run_id != run_id {
        return Err("Pi 交付回执属于另一轮运行，已拒绝。".into());
    }
    if receipt.request_id.trim().is_empty() {
        return Err("Pi 交付回执缺少本轮的输入标识，已拒绝。".into());
    }
    match receipt.kind.as_str() {
        // 三种文件型交付：正文 Markdown、风格 JSON、任务助手 JSON。
        "document" | "style" | "task" => {
            if !super::delivery::safe_work_path(&receipt.path) {
                return Err("Pi 交付回执里的成品路径不合法，已拒绝。".into());
            }
            if receipt.summary.chars().count() > 2_000 {
                return Err("Pi 交付回执的摘要太长，已拒绝。".into());
            }
            let content = read_snapshot(run_dir, &receipt)?;
            if content.len() != receipt.bytes {
                return Err("Pi 成品快照的字节数与回执不一致，已拒绝。".into());
            }
            if sha256_hex(&content) != receipt.sha256.to_ascii_lowercase() {
                return Err("Pi 成品快照的哈希与回执不一致，已拒绝。".into());
            }
        }
        "report" => {
            if receipt.items.len() > 100 {
                return Err("Pi 清单条目超过上限，已拒绝。".into());
            }
        }
        _ => return Err("Pi 交付回执的交付类型不认识，已拒绝。".into()),
    }
    Ok(Some(receipt))
}

/// 回执指向的成品快照。只读 `pi/final.md`，不读模型可写的 `work/`。
pub(crate) fn read_snapshot(run_dir: &Path, receipt: &PiReceipt) -> Result<String, String> {
    if !matches!(receipt.kind.as_str(), "document" | "style" | "task") {
        return Err("这份回执不是文件交付。".into());
    }
    let file = if receipt.kind == "document" {
        "final.md"
    } else {
        "final.json"
    };
    let bytes = read_bounded(&run_dir.join(file), MAX_SNAPSHOT_BYTES)?;
    let text =
        String::from_utf8(bytes).map_err(|_| "Pi 成品快照不是合法 UTF-8，已拒绝。".to_string())?;
    if text.trim().is_empty() || text.contains('\0') {
        return Err("Pi 成品快照是空的或含 NUL 字节，已拒绝。".into());
    }
    Ok(text)
}

/// 字节内容的 SHA-256（索引与来源项都按字节哈希）。
fn sha256_hex_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn modified_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// 来源按「单项文件 + 索引」保存：每次工具返回只新写变化的单项，索引极小且幂等。
/// 旧版单一聚合文件的路径仍在 [`read_sources`] 里兼容读取。
///
/// 返回索引内容的哈希（写入 `run.json` 的 `sources` 指针）。
pub(crate) fn write_sources(run_dir: &Path, sources: &[String]) -> Result<String, String> {
    let dir = run_dir.join("sources");
    let items = dir.join("items");
    std::fs::create_dir_all(&items).map_err(|error| format!("建立来源目录失败：{error}"))?;
    let mut hashes: Vec<String> = Vec::with_capacity(sources.len());
    for source in sources {
        let bytes = source.as_bytes();
        if bytes.len() as u64 > MAX_SOURCE_ITEM_BYTES {
            return Err(format!(
                "单条事实来源超过上限（{} / {} 字节），未保存。",
                bytes.len(),
                MAX_SOURCE_ITEM_BYTES
            ));
        }
        let hash = sha256_hex_bytes(bytes);
        let path = items.join(format!("{hash}.md"));
        if !path.exists() {
            crate::vault::write_path(&path, "pi", bytes)
                .map_err(|error| format!("写不出 Pi 事实来源：{error:#}"))?;
        }
        hashes.push(hash);
    }
    let index = serde_json::json!({ "version": 1, "items": hashes });
    let bytes =
        serde_json::to_vec(&index).map_err(|error| format!("组装事实来源索引失败：{error}"))?;
    if bytes.len() as u64 > MAX_SOURCES_INDEX_BYTES {
        return Err("事实来源条目过多，索引超过上限，未保存。".into());
    }
    let hash = sha256_hex_bytes(&bytes);
    let path = dir.join(format!("index-{hash}.json"));
    if !path.exists() {
        crate::vault::write_path(&path, "pi", &bytes)
            .map_err(|error| format!("写不出 Pi 事实来源索引：{error:#}"))?;
    }
    // 只清理没有索引引用的项文件；索引本身等运行记录指针切换成功后再清
    // （见 [`prune_sources`]），否则“写新索引成功、改指针失败”会删掉仍被
    // 旧指针指向的那一份，恢复时读不到来源。
    prune_source_items(&dir, &items);
    Ok(hash)
}

/// 运行记录指针已经切到 `current` 之后，保留最新几个索引版本并清理项文件。
/// 无论 mtime 排序如何，`current` 必须保留。清理失败一律忽略：它是节省磁盘，
/// 不能反过来影响本轮。
fn prune_sources(run_dir: &Path, current: &str) {
    let dir = run_dir.join("sources");
    let items = dir.join("items");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let current_path = dir.join(format!("index-{}.json", current.to_ascii_lowercase()));
    let mut indexes: Vec<(u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("index-") && name.ends_with(".json"))
                .then_some((modified_secs(&path), path))
        })
        .collect();
    indexes.sort_by_key(|right| std::cmp::Reverse(right.0));
    let mut kept: Vec<PathBuf> = indexes
        .iter()
        .take(MAX_SOURCE_INDEX_VERSIONS)
        .map(|(_, path)| path.clone())
        .collect();
    if current_path.exists() && !kept.contains(&current_path) {
        kept.push(current_path);
    }
    for (_, path) in &indexes {
        if !kept.iter().any(|kept| kept == path) {
            let _ = std::fs::remove_file(path);
        }
    }
    prune_source_items(&dir, &items);
}

/// 删掉没被任何现存索引引用的项文件（超过 TTL 才删）。
fn prune_source_items(dir: &Path, items: &Path) {
    let mut referenced = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !(name.starts_with("index-") && name.ends_with(".json")) {
            continue;
        }
        let Ok(bytes) = read_bounded(&path, MAX_SOURCES_INDEX_BYTES) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(hashes) = value.get("items").and_then(serde_json::Value::as_array) {
            for hash in hashes {
                if let Some(hash) = hash.as_str() {
                    referenced.insert(hash.to_ascii_lowercase());
                }
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(items) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        if referenced.contains(&stem.to_ascii_lowercase()) {
            continue;
        }
        let old_enough = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| now.duration_since(time).ok())
            .is_some_and(|age| age > SOURCE_ITEM_TTL);
        if old_enough {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 读回事实来源并校验哈希。优先读新的「索引 + 单项」格式，兼容旧的单一聚合文件。
/// 哈希对不上就当没有，不把不可信内容当依据。
pub(crate) fn read_sources(run_dir: &Path, expected: &str) -> Result<Option<String>, String> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("事实来源哈希写法不合法。".into());
    }
    let expected = expected.to_ascii_lowercase();
    let dir = run_dir.join("sources");
    let index = dir.join(format!("index-{expected}.json"));
    if index.exists() {
        let bytes = read_bounded(&index, MAX_SOURCES_INDEX_BYTES)?;
        if sha256_hex_bytes(&bytes) != expected {
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("事实来源索引读不出来：{error}"))?;
        let items = value
            .get("items")
            .and_then(serde_json::Value::as_array)
            .ok_or("事实来源索引格式不正确")?;
        let mut parts = Vec::with_capacity(items.len());
        let mut total = 0u64;
        for item in items {
            let Some(hash) = item.as_str() else {
                return Err("事实来源索引项不是字符串。".into());
            };
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("事实来源索引项哈希写法不合法。".into());
            }
            let path = dir
                .join("items")
                .join(format!("{}.md", hash.to_ascii_lowercase()));
            let bytes = read_bounded(&path, MAX_SOURCE_ITEM_BYTES)?;
            if sha256_hex_bytes(&bytes) != hash.to_ascii_lowercase() {
                return Ok(None);
            }
            total = total.saturating_add(bytes.len() as u64);
            if total > MAX_SOURCES_BYTES {
                return Err(
                    "本轮事实来源超过恢复上限（8 MiB），不能接着写；请「从头写」重新开始。".into(),
                );
            }
            parts.push(
                String::from_utf8(bytes)
                    .map_err(|_| "Pi 事实来源不是合法 UTF-8，未采用。".to_string())?,
            );
        }
        return Ok(Some(parts.join("\n\n---\n\n")));
    }
    // 兼容本阶段早期开发记录（单一聚合文件）：只在哈希核对通过时使用。
    let versioned = dir.join(format!("{expected}.md"));
    let path = if versioned.exists() {
        versioned
    } else {
        run_dir.join("sources.md")
    };
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_bounded(&path, MAX_SOURCES_BYTES)?;
    let text =
        String::from_utf8(bytes).map_err(|_| "Pi 事实来源不是合法 UTF-8，未采用。".to_string())?;
    if sha256_hex(&text) != expected {
        return Ok(None);
    }
    Ok(Some(text))
}

/// 读有上限的文件；加密资源按明文长度限界，旧明文与 GWA1 仅作兼容读取。
pub(crate) fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    crate::vault::read_path(path, "pi", max)
        .map_err(|error| format!("读不到 {}：{error:#}", path.display()))
}

/// 一轮运行的后台记录。只在 Rust 侧写；host 只写 `receipt.json` 与 `final.md`。
#[derive(Debug)]
pub(crate) struct PiRunLog {
    workspace: PiWorkspace,
    record: Mutex<PiRunRecord>,
}

impl PiRunLog {
    /// 已有一份校验过的记录（恢复路径），不再重复写一次。
    pub(crate) fn from_record(workspace: PiWorkspace, record: PiRunRecord) -> Result<Self, String> {
        Ok(Self {
            workspace,
            record: Mutex::new(record),
        })
    }

    /// 开跑前写一份，并把记录交给后台线程。
    pub(crate) fn open(workspace: &PiWorkspace, record: PiRunRecord) -> Result<Self, String> {
        record.write(workspace)?;
        Self::from_record(workspace.clone(), record)
    }

    pub(crate) fn snapshot(&self) -> PiRunRecord {
        self.record
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    fn update(&self, edit: impl FnOnce(&mut PiRunRecord)) {
        let _ = self.try_update(edit);
    }

    fn try_update(&self, edit: impl FnOnce(&mut PiRunRecord)) -> Result<(), String> {
        let mut record = self
            .record
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut next = record.clone();
        edit(&mut next);
        next.updated_at = chrono::Local::now().to_rfc3339();
        next.write(&self.workspace)?;
        *record = next;
        Ok(())
    }

    pub(crate) fn ready(&self, host_version: &str) -> Result<(), String> {
        let host_version = host_version.to_string();
        self.try_update(|record| {
            record.host_version = host_version;
            record.status = PiRunStatus::Running;
            record.message.clear();
        })
    }

    /// 记录用户确认的回答：`display` 是「问题：回答」的显示行，`values` 是纯答案值。
    /// 两者分开存：模型题面可能含未确认的假设，不能把整段显示文字当事实。
    #[cfg(test)]
    pub(crate) fn confirmed(&self, display: &[String], values: &[String]) -> Result<(), String> {
        let display: Vec<String> = display
            .iter()
            .take(MAX_CONFIRMED)
            .map(|value| value.chars().take(2_000).collect())
            .collect();
        let values: Vec<String> = values
            .iter()
            .take(MAX_CONFIRMED)
            .map(|value| value.chars().take(2_000).collect())
            .collect();
        self.try_update(|record| {
            record.confirmed = display;
            record.answers = values;
        })
    }

    /// 把当前真实事实来源立刻落盘（工具返回 / 用户回答后、交给 host 之前）。
    /// 崩溃后恢复的闸门只认这份文件，不从候选稿或题面重建。
    pub(crate) fn persist_sources(&self, sources: &[String]) -> Result<Option<String>, String> {
        let hash = write_sources(&self.workspace.run, sources)?;
        self.try_update(|record| record.sources = Some(hash.clone()))?;
        prune_sources(&self.workspace.run, &hash);
        Ok(Some(hash))
    }

    /// 回答值与其来源指针一次提交；写失败时不能留下半份“已确认”记录。
    pub(crate) fn confirmed_sources(
        &self,
        display: &[String],
        values: &[String],
        sources: &[String],
    ) -> Result<(), String> {
        let hash = write_sources(&self.workspace.run, sources)?;
        self.try_update(|record| {
            record.confirmed.extend(
                display
                    .iter()
                    .take(MAX_CONFIRMED)
                    .map(|text| text.chars().take(2_000).collect::<String>()),
            );
            record.answers.extend(
                values
                    .iter()
                    .take(MAX_CONFIRMED)
                    .map(|text| text.chars().take(2_000).collect::<String>()),
            );
            record.confirmed.truncate(MAX_CONFIRMED);
            record.answers.truncate(MAX_CONFIRMED);
            record.sources = Some(hash.clone());
        })?;
        prune_sources(&self.workspace.run, &hash);
        Ok(())
    }

    pub(crate) fn evidence(&self, line: &str) {
        let line: String = line.chars().take(MAX_EVIDENCE_CHARS).collect();
        self.update(|record| {
            if record.evidence.len() < MAX_EVIDENCE_LINES {
                record.evidence.push(line);
            }
        });
    }

    /// 写终态。写盘失败向调用方返回原因：失败现场要能在错误串里说出来，
    /// 不能悄悄停在 Running。
    ///
    /// 内存里的记录**先**更新成终态再写盘：即使锁屏/退出时磁盘写入失败，
    /// 本进程后续回写与界面提示拿到的也仍是终态；盘上残留的 Running 由
    /// [`reconcile_interrupted_runs`] 在下次解锁/列历史轮次时纠正。
    pub(crate) fn finished(&self, status: PiRunStatus, message: &str) -> Result<(), String> {
        let message: String = message.chars().take(2_000).collect();
        let next = {
            let mut record = self
                .record
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let mut next = record.clone();
            next.status = status;
            next.message = message;
            next.updated_at = chrono::Local::now().to_rfc3339();
            *record = next.clone();
            next
        };
        next.write(&self.workspace)
    }

    pub(crate) fn delivered(
        &self,
        receipt: &PiReceipt,
        sources: Option<&[String]>,
    ) -> Result<Option<String>, String> {
        // 来源只是恢复用的辅助材料：写不出来不能反过来否掉已经落盘的成品，
        // 否则回执重放会在同一个错误上无限循环。失败时保留旧指针并在证据里留痕。
        let sources = match sources {
            Some(sources) => match write_sources(&self.workspace.run, sources) {
                Ok(hash) => Some(hash),
                Err(error) => {
                    self.evidence(&format!("本轮事实来源未落盘：{error}"));
                    self.record
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .sources
                        .clone()
                }
            },
            None => self
                .record
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .sources
                .clone(),
        };
        let summary = receipt.summary();
        let hash = sources.clone();
        self.try_update(|record| {
            record.status = PiRunStatus::Delivered;
            record.message.clear();
            record.receipt = Some(summary);
            record.sources = sources;
        })?;
        if let Some(hash) = &hash {
            prune_sources(&self.workspace.run, hash);
        }
        Ok(hash)
    }
}

/// 从轮次里的薄引用读回盘上的运行状态；读不到返回 None（界面保持原值，不猜）。
pub(crate) fn run_status(run: &PiRunRef) -> Option<PiRunStatus> {
    if run.is_empty() {
        return None;
    }
    let root = resolve_workspace(&run.workspace).ok()?;
    let dir = root.parent()?;
    let record = PiRunRecord::read_from(dir).ok()?;
    (record.run_id == run.run_id).then_some(record.status)
}

/// 把「记录仍是 Running、本机却已经没有活动宿主」的轮次改写成 Interrupted。
///
/// 终态写盘失败（例如锁屏/退出时磁盘错误）会让记录永远停在 Running；
/// 解锁后列出历史轮次、清理工作区或恢复某一轮时先调这里，把状态纠正回来，
/// 用户看到的是「上次运行被停止」而不是「上次运行没有收尾」。返回改写的轮次数。
///
/// 只在持有密钥句柄、能写配置目录时调用；任何一条纠正写盘失败都返回 Err，
/// 不把失败吞成「已经修好了」。
pub(crate) fn reconcile_interrupted_runs() -> Result<usize, String> {
    // 本进程正在准备或运行 Pi 时不做批量纠正：记录刚写成 Running、Rust 还没拿到
    // 运行所有权锁的短窗口里把它改成中断，只会让界面闪一下错误状态。另一个进程里
    // 正在跑的轮次由 rust-owner.lock 拦住（见 [`reconcile_run_record`]）。
    if super::process::writing_active() {
        return Ok(0);
    }
    let base = super::workspace::base_dir()?;
    let mut scanned = 0usize;
    let mut fixed = 0u32;
    let mut first_error: Option<String> = None;
    let docs = std::fs::read_dir(&base)
        .map_err(|error| format!("读不了 Pi 工作区目录（{}）：{error}", base.display()))?;
    for doc in docs.flatten() {
        if scanned >= MAX_RECONCILE_RUNS {
            break;
        }
        let Ok(file_type) = doc.file_type() else {
            continue;
        };
        if !file_type.is_dir() || !doc.file_name().to_string_lossy().starts_with("doc-") {
            continue;
        }
        let Ok(runs) = std::fs::read_dir(doc.path()) else {
            continue;
        };
        for run in runs.flatten() {
            if scanned >= MAX_RECONCILE_RUNS {
                break;
            }
            let name = run.file_name().to_string_lossy().into_owned();
            if !name.starts_with("run-") {
                continue;
            }
            let Ok(file_type) = run.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            scanned += 1;
            match reconcile_run_record(&run.path()) {
                Ok(true) => fixed = fixed.saturating_add(1),
                Ok(false) => {}
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
    }
    if let Some(error) = first_error {
        return Err(format!(
            "有轮次的中断状态纠正失败（已纠正 {fixed} 轮）：{error}"
        ));
    }
    Ok(fixed as usize)
}

/// 纠正一个运行目录：记录是 Running 且没有活动宿主时改写为 Interrupted。
///
/// `Ok(true)` 表示改写了；`Ok(false)` 表示不需要改、正在跑或拿不准（锁/记录读不出来）。
/// 改写失败返回 Err；判断是否需要改写时读不出的目录一律跳过，避免把损坏记录猜成中断。
pub(crate) fn reconcile_run_record(run_dir: &Path) -> Result<bool, String> {
    let record = match PiRunRecord::read_from(run_dir) {
        Ok(record) => record,
        Err(_) => return Ok(false),
    };
    if record.status != PiRunStatus::Running {
        return Ok(false);
    }
    // 先拿 Rust 侧运行所有权：拿不到说明本机（或另一个窗口）正在跑这一轮。
    // 拿不准（锁文件打不开）时也不改，避免把正在运行的记录写成中断。
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(run_dir.join("rust-owner.lock"))
    else {
        return Ok(false);
    };
    if lock.try_lock().is_err() {
        return Ok(false);
    }
    if host_owner_is_live(run_dir) {
        return Ok(false);
    }
    let mut updated = record;
    updated.status = PiRunStatus::Interrupted;
    updated.message =
        "上次运行在本机进程收尾前就结束了，已标记为中断；可以「接着写」或「从头写」。".into();
    updated.updated_at = chrono::Local::now().to_rfc3339();
    // 失败不能吞：调用方（列历史轮次 / 清理 / 恢复入口）要如实提示。
    updated.write_to_dir(run_dir)?;
    drop(lock);
    Ok(true)
}

/// Node host 是否仍持有 `host-owner.lock` 的 SQLite 排他锁。
///
/// host 在整轮运行期间用 `BEGIN EXCLUSIVE` 占着这个库；Rust 崩溃而 host 还活着的
/// 极端情况下，只看 `rust-owner.lock` 会误判。打不开或读不准（不是 SQLite、权限
/// 错误等）时保守当成「仍有 host」，宁可不纠正，也不把正在跑的轮次写成中断。
fn host_owner_is_live(run_dir: &Path) -> bool {
    let path = run_dir.join("host-owner.lock");
    if !path.is_file() {
        return false;
    }
    let Ok(connection) =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
    else {
        return true;
    };
    // 不等待：锁被占时立刻返回 BUSY，界面线程不能卡住。
    let _ = connection.busy_timeout(std::time::Duration::ZERO);
    match connection.execute_batch("BEGIN EXCLUSIVE") {
        Ok(()) => {
            let _ = connection.execute_batch("ROLLBACK");
            false
        }
        Err(error) => match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => true,
            // 不是数据库：host 的锁文件被密封迁移或其他工具碰过，那个 host 已经不可能
            // 还持有有效锁，按「没有活动 host」处理。
            Some(rusqlite::ErrorCode::NotADatabase) => false,
            _ => true,
        },
    }
}

#[cfg(test)]
/// 测试夹具：按工作区现有快照写一份最小运行记录。
pub(crate) fn test_log(
    workspace: &PiWorkspace,
    request_id: &str,
    run_id: &str,
) -> std::sync::Arc<PiRunLog> {
    let current = std::fs::read_to_string(workspace.root.join("input/current.md"))
        .unwrap_or_else(|_| "# 当前\n".into());
    let request = std::fs::read_to_string(workspace.root.join("input/request.md"))
        .unwrap_or_else(|_| "要求".into());
    let record = PiRunRecord::start(
        run_id.into(),
        request_id.into(),
        workspace,
        &current,
        &current,
        &current,
        &request,
        None,
        &request,
        "通知",
        "2026-11-01",
        None,
        None,
        Vec::new(),
        "fixture",
        "fixture",
    );
    std::sync::Arc::new(PiRunLog::open(workspace, record).expect("写得出测试运行记录"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_sample_cannot_be_promoted_by_changing_the_material_index() {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        let dir = tempfile::tempdir().unwrap();
        let (workspace, metas) = PiWorkspace::prepare(
            dir.path(),
            1,
            "正文",
            "要求",
            &[super::super::materials::PiMaterialInput {
                title: "样稿".into(),
                source: "用户".into(),
                role: super::super::materials::MaterialRole::StyleSample,
                text: "2026年已有的样稿事实".into(),
            }],
            &crate::pi_agent::pi_skills::load_catalog(),
            None,
        )
        .unwrap();
        let mut saved = record(&workspace);
        saved.materials = metas;
        saved.check_materials(&workspace).unwrap();
        let index = workspace.root.join("input/materials/index.json");
        let mut forged: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&index).unwrap()).unwrap();
        forged[0]["role"] = "fact".into();
        std::fs::write(index, forged.to_string()).unwrap();
        assert!(
            saved
                .check_materials(&workspace)
                .unwrap_err()
                .contains("用途")
        );
    }

    #[test]
    fn source_snapshots_remain_readable_when_pointer_update_is_interrupted() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let log = PiRunLog::open(&workspace, record(&workspace)).unwrap();
        let first = log.persist_sources(&["旧依据".into()]).unwrap().unwrap();
        let newer = write_sources(&workspace.run, &["新依据".into()]).unwrap();
        assert_ne!(first, newer);
        assert_eq!(
            read_sources(&workspace.run, &first).unwrap().as_deref(),
            Some("旧依据")
        );
        assert_eq!(
            PiRunRecord::read_from(&workspace.run)
                .unwrap()
                .sources
                .as_deref(),
            Some(first.as_str())
        );
        // 模拟运行记录无法替换：不改变内存指针，且向调用方报告失败。
        std::fs::remove_file(workspace.run.join("run.json")).unwrap();
        std::fs::create_dir(workspace.run.join("run.json")).unwrap();
        assert!(log.persist_sources(&["第三份依据".into()]).is_err());
        assert_eq!(log.snapshot().sources.as_deref(), Some(first.as_str()));
        assert_eq!(
            read_sources(&workspace.run, &first).unwrap().as_deref(),
            Some("旧依据")
        );
    }

    /// 指针更新成功后才会清理旧索引：当前指针指向的那一份必须一直可读，
    /// 索引版本数有上限，不能随工具调用次数无限增长。
    #[test]
    fn source_indexes_keep_the_current_pointer_and_bound_the_versions() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let log = PiRunLog::open(&workspace, record(&workspace)).unwrap();
        let mut hashes = Vec::new();
        for index in 0..5 {
            let source = format!("依据{index}");
            hashes.push(
                log.persist_sources(&[source])
                    .unwrap()
                    .expect("每次落盘都有索引"),
            );
        }
        let current = hashes.last().unwrap();
        assert_eq!(
            read_sources(&workspace.run, current).unwrap().as_deref(),
            Some("依据4")
        );
        let indexes = std::fs::read_dir(workspace.run.join("sources"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("index-"))
            .count();
        assert!(
            indexes <= MAX_SOURCE_INDEX_VERSIONS + 1,
            "旧索引没有清理：{indexes} 个"
        );
    }

    fn workspace(dir: &Path) -> PiWorkspace {
        let run = dir.join("run-1");
        let root = run.join("workspace");
        std::fs::create_dir_all(root.join("input/materials")).unwrap();
        std::fs::create_dir_all(root.join("skills/pi-skills")).unwrap();
        std::fs::create_dir_all(root.join("skills/gongwen-markdown")).unwrap();
        std::fs::create_dir_all(run.join("pi/session")).unwrap();
        std::fs::write(root.join("input/current.md"), "正式正文").unwrap();
        std::fs::write(root.join("input/request.md"), "要求：改短").unwrap();
        std::fs::write(root.join("input/materials/index.json"), "[]").unwrap();
        PiWorkspace {
            root,
            state: run.join("pi/session"),
            run,
            label: "pi/workspaces/doc-1/run-1".into(),
        }
    }

    fn record(workspace: &PiWorkspace) -> PiRunRecord {
        PiRunRecord::start(
            "run-1".into(),
            "req-1".into(),
            workspace,
            "正式正文",
            "正式正文",
            "正式正文",
            "要求：改短",
            None,
            "要求：改短",
            "通知",
            "2026-11-01",
            None,
            None,
            Vec::new(),
            "p1",
            "pi-model",
        )
    }

    #[test]
    fn a_record_round_trips_and_checks_out() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let record = record(&workspace);
        record.write(&workspace).unwrap();
        let read = PiRunRecord::read_from(&workspace.run).unwrap();
        assert_eq!(read.run_id, "run-1");
        read.check_resume(&workspace.label, "run-1", "正式正文")
            .unwrap();
        check_workspace_files(&workspace, &read).unwrap();
    }

    #[test]
    fn the_strategy_preference_survives_serialization_and_restart() {
        use crate::pi_agent::PiStrategyPreference;
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let record = record(&workspace).with_strategy(PiStrategyPreference::Phased);
        record.write(&workspace).unwrap();
        // 相当于重启后重读运行记录：用户后来把输入框改回「自动」也不影响这一轮。
        let read = PiRunRecord::read_from(&workspace.run).unwrap();
        assert_eq!(read.strategy, PiStrategyPreference::Phased);
        assert_eq!(
            serde_json::to_value(&read).unwrap()["strategy"],
            serde_json::json!("phased"),
            "存的是 snake_case 字符串，不是调试输出"
        );
    }

    #[test]
    fn old_records_without_a_strategy_are_read_as_auto() {
        use crate::pi_agent::PiStrategyPreference;
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let record = record(&workspace);
        let mut value = serde_json::to_value(&record).unwrap();
        value.as_object_mut().unwrap().remove("strategy");
        std::fs::write(
            workspace.run.join("run.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let read = PiRunRecord::read_from(&workspace.run).unwrap();
        assert_eq!(read.strategy, PiStrategyPreference::Auto);
        assert_eq!(
            record.strategy,
            PiStrategyPreference::Auto,
            "新记录默认自动"
        );
    }

    #[test]
    fn legacy_protocol_can_replay_only_an_existing_delivery_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let mut record = record(&workspace);
        record.protocol = PROTOCOL_VERSION - 1;
        record.host_version = "0.3.6".into();
        record.status = PiRunStatus::Running;
        assert!(
            record
                .check_resume(&workspace.label, "run-1", "正式正文")
                .is_err()
        );

        record.status = PiRunStatus::Delivered;
        record.receipt = Some(PiReceiptSummary {
            kind: "document".into(),
            path: "pi/final.md".into(),
            sha256: sha256_hex("成品"),
            bytes: 6,
            summary: "旧成品".into(),
            call_id: "call-1".into(),
        });
        record
            .check_resume(&workspace.label, "run-1", "正式正文")
            .unwrap();
    }

    #[test]
    fn an_accepted_run_cannot_be_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let mut record = record(&workspace);
        record.status = PiRunStatus::Accepted;
        assert!(!PiRunStatus::Accepted.recoverable());
        let error = record
            .check_resume(&workspace.label, "run-1", "正式正文")
            .unwrap_err();
        assert!(error.contains("接受"), "{error}");
        assert!(!PiRunRef::from_record(&record).recoverable());
        record.status = PiRunStatus::Delivered;
        record
            .check_resume(&workspace.label, "run-1", "正式正文")
            .unwrap();
        assert!(PiRunRef::from_record(&record).recoverable());
    }

    /// 被拒标记随记录落盘，旧记录（没有这一项）按否读回。
    #[test]
    fn delivery_rejected_survives_serialization_and_defaults_to_false() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let mut record = record(&workspace);
        record.delivery_rejected = true;
        record.write(&workspace).unwrap();
        let read = PiRunRecord::read_from(&workspace.run).unwrap();
        assert!(read.delivery_rejected, "被拒标记要随记录落盘");
        // 旧记录 JSON 没有这一项：按否读回，不影响其它字段。
        let mut value = serde_json::to_value(&read).unwrap();
        value.as_object_mut().unwrap().remove("deliveryRejected");
        let old: PiRunRecord = serde_json::from_value(value).unwrap();
        assert!(!old.delivery_rejected, "旧记录没有该项，按否处理");
        assert_eq!(old.run_id, "run-1");
    }

    #[test]
    fn a_changed_baseline_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let record = record(&workspace);
        let error = record
            .check_resume(&workspace.label, "run-1", "正文被改过")
            .unwrap_err();
        assert!(error.contains("改过"), "{error}");
        let error = record
            .check_resume("pi/workspaces/doc-1/run-2", "run-1", "正式正文")
            .unwrap_err();
        assert!(error.contains("对不上"), "{error}");
        let error = record
            .check_resume(&workspace.label, "run-2", "正式正文")
            .unwrap_err();
        assert!(error.contains("runId"), "{error}");
    }

    #[test]
    fn a_tampered_snapshot_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let record = record(&workspace);
        std::fs::write(workspace.root.join("input/current.md"), "偷换的正文").unwrap();
        let error = check_workspace_files(&workspace, &record).unwrap_err();
        assert!(error.contains("快照"), "{error}");
        std::fs::write(workspace.root.join("input/current.md"), "正式正文").unwrap();
        check_workspace_files(&workspace, &record).unwrap();
        // 没有会话痕迹（开跑前就取消之类）时，会话目录缺失不拦恢复。
        std::fs::remove_dir_all(&workspace.state).unwrap();
        check_workspace_files(&workspace, &record).unwrap();
        // 旧版只有 session.sqlite：不能当 JSONL 会话续跑。
        std::fs::write(workspace.run.join("pi/session.sqlite"), b"legacy sqlite").unwrap();
        let error = check_workspace_files(&workspace, &record).unwrap_err();
        assert!(error.contains("SQLite"), "{error}");
        assert!(error.contains("从头写"), "{error}");
        // 有真正的 JSONL 会话时，旧 sqlite 只作归档，不影响恢复。
        std::fs::create_dir_all(&workspace.state).unwrap();
        std::fs::write(workspace.state.join("main.jsonl"), b"{\"seq\":1}\n").unwrap();
        check_workspace_files(&workspace, &record).unwrap();
    }

    /// 记录里已经有证据 / 来源 / 回执，说明这一轮确实跑过 host：会话主文件
    /// 缺失或为 0 字节时必须拒绝恢复，不能静默当成空会话接着跑。
    #[test]
    fn a_session_with_evidence_requires_a_non_empty_jsonl_file() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let mut record = record(&workspace);
        record.evidence.push("工具返回了检索结果".into());
        record.write(&workspace).unwrap();
        let error = check_workspace_files(&workspace, &record).unwrap_err();
        assert!(error.contains("main.jsonl"), "{error}");
        assert!(error.contains("从头写"), "{error}");

        // 有文件但被清零，同样不能续跑。
        std::fs::write(workspace.state.join("main.jsonl"), b"").unwrap();
        let error = check_workspace_files(&workspace, &record).unwrap_err();
        assert!(error.contains("缺失或为空"), "{error}");

        // 交付回执同样算「已有会话」的证据。
        std::fs::remove_file(workspace.state.join("main.jsonl")).unwrap();
        record.receipt = Some(PiReceiptSummary {
            kind: "document".into(),
            path: "work/final.md".into(),
            sha256: sha256_hex("成品"),
            bytes: 6,
            summary: "旧成品".into(),
            call_id: "call-1".into(),
        });
        record.write(&workspace).unwrap();
        let error = check_workspace_files(&workspace, &record).unwrap_err();
        assert!(error.contains("main.jsonl"), "{error}");

        // 会话文件有内容就能通过。
        std::fs::write(workspace.state.join("main.jsonl"), b"{\"seq\":1}\n").unwrap();
        check_workspace_files(&workspace, &record).unwrap();
    }

    /// 旧协议只允许「启动前已有同类交付回执」的现成回放：回放不开 Durable 会话，
    /// 会话缺失不拦；没有回执的旧协议轮次仍然不能恢复。
    #[test]
    fn a_legacy_receipt_replay_does_not_need_a_jsonl_session() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let mut record = record(&workspace);
        record.protocol = PROTOCOL_VERSION - 1;
        record.host_version = "0.3.6".into();
        record.status = PiRunStatus::Delivered;
        record.receipt = Some(PiReceiptSummary {
            kind: "document".into(),
            path: "work/final.md".into(),
            sha256: sha256_hex("成品"),
            bytes: 6,
            summary: "旧成品".into(),
            call_id: "call-1".into(),
        });
        record.write(&workspace).unwrap();
        std::fs::remove_dir_all(&workspace.state).unwrap();
        record
            .check_resume(&workspace.label, "run-1", "正式正文")
            .unwrap();
        check_workspace_files(&workspace, &record).unwrap();

        // 回执没了：旧协议轮次不能续跑，会话缺失也要报可读错误。
        record.receipt = None;
        record.write(&workspace).unwrap();
        let error = record
            .check_resume(&workspace.label, "run-1", "正式正文")
            .unwrap_err();
        assert!(error.contains("协议"), "{error}");
        let error = check_workspace_files(&workspace, &record).unwrap_err();
        assert!(error.contains("main.jsonl"), "{error}");
    }

    /// 没有会话痕迹（例如开跑前就取消）不把「空会话」当错误；host 还来不及写。
    #[test]
    fn a_run_without_session_evidence_may_start_a_fresh_jsonl_session() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let mut record = record(&workspace);
        record.status = PiRunStatus::Interrupted;
        record.write(&workspace).unwrap();
        std::fs::remove_dir_all(&workspace.state).unwrap();
        check_workspace_files(&workspace, &record).unwrap();
    }

    /// 锁屏/退出时终态写盘失败：内存里仍要保留终态，调用方拿到可读错误。
    #[test]
    fn a_failed_terminal_write_keeps_the_in_memory_status() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let log = PiRunLog::open(&workspace, record(&workspace)).unwrap();
        // 把 run.json 换成目录，让原子写失败（模拟磁盘/权限异常）。
        std::fs::remove_file(workspace.run.join("run.json")).unwrap();
        std::fs::create_dir(workspace.run.join("run.json")).unwrap();
        let error = log
            .finished(PiRunStatus::Interrupted, "用户停止")
            .unwrap_err();
        assert!(!error.is_empty(), "写盘失败必须向调用方报错");
        assert_eq!(
            log.snapshot().status,
            PiRunStatus::Interrupted,
            "内存里的终态不能被写盘失败撤回"
        );
    }

    #[test]
    fn a_corrupt_record_is_a_readable_problem_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        std::fs::write(workspace.run.join("run.json"), b"{ not json").unwrap();
        let error = PiRunRecord::read_from(&workspace.run).unwrap_err();
        assert!(error.contains("读不出来"), "{error}");
        std::fs::write(workspace.run.join("run.json"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let error = PiRunRecord::read_from(&workspace.run).unwrap_err();
        assert!(error.contains("上限"), "{error}");
    }

    #[test]
    fn receipts_are_bound_to_the_run_and_snapshot_hash() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let content = "# 成品\n";
        std::fs::write(workspace.run.join("final.md"), content).unwrap();
        let receipt = PiReceipt {
            version: RUN_VERSION,
            run_id: "run-1".into(),
            request_id: "req-1".into(),
            kind: "document".into(),
            path: "work/final.md".into(),
            sha256: sha256_hex(content),
            bytes: content.len(),
            summary: "改好了".into(),
            call_id: "c1".into(),
            items: Vec::new(),
            at: "2026-11-01T00:00:00+08:00".into(),
        };
        std::fs::write(
            workspace.run.join("receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let read = read_receipt(&workspace.run, "run-1").unwrap().unwrap();
        assert_eq!(read.sha256, sha256_hex(content));
        assert!(read_receipt(&workspace.run, "run-2").is_err());
        std::fs::write(workspace.run.join("final.md"), "偷换").unwrap();
        assert!(read_receipt(&workspace.run, "run-1").is_err());
    }

    #[test]
    fn answers_are_stored_apart_from_the_question_display_text() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let log = PiRunLog::open(&workspace, record(&workspace)).unwrap();
        log.confirmed(&["想强调哪方面：责任".to_string()], &["责任".to_string()])
            .unwrap();
        let read = PiRunRecord::read_from(&workspace.run).unwrap();
        assert_eq!(read.answers, vec!["责任"]);
        assert_eq!(read.confirmed, vec!["想强调哪方面：责任"]);
        // 只把纯答案当事实；题面里的「哪方面」不能变成来源。
        let hash = log.persist_sources(&read.answers.clone()).unwrap().unwrap();
        let sources = read_sources(&workspace.run, &hash).unwrap().unwrap();
        assert_eq!(sources, "责任");
        assert!(!sources.contains("想强调"));
        assert_eq!(
            PiRunRecord::read_from(&workspace.run).unwrap().sources,
            Some(hash)
        );
    }

    #[test]
    fn sources_are_reloaded_only_when_the_hash_matches() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = workspace(dir.path());
        let hash = write_sources(&workspace.run, &["材料一".into(), "材料二".into()]).unwrap();
        assert_eq!(
            read_sources(&workspace.run, &hash).unwrap().as_deref(),
            Some("材料一\n\n---\n\n材料二")
        );
        assert!(
            read_sources(&workspace.run, &"0".repeat(64))
                .unwrap()
                .is_none()
        );
    }

    /// 状态纠正测试用的临时配置目录；测试结束恢复，避免影响并行测试。
    struct ReconcileConfigReset;
    impl Drop for ReconcileConfigReset {
        fn drop(&mut self) {
            crate::storage::set_test_config_dir(None);
        }
    }

    fn reconcile_config_dir() -> (tempfile::TempDir, ReconcileConfigReset) {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        (config, ReconcileConfigReset)
    }

    /// 在配置目录的 Pi 工作区里造一个可被纠正扫描发现的运行目录。
    fn run_under(doc_dir: &Path, name: &str, status: PiRunStatus) -> PathBuf {
        std::fs::create_dir_all(doc_dir).unwrap();
        let run = doc_dir.join(name);
        let root = run.join("workspace");
        std::fs::create_dir_all(root.join("input/materials")).unwrap();
        std::fs::create_dir_all(root.join("skills/pi-skills")).unwrap();
        std::fs::create_dir_all(root.join("skills/gongwen-markdown")).unwrap();
        std::fs::create_dir_all(run.join("pi/session")).unwrap();
        std::fs::write(root.join("input/current.md"), "正式正文").unwrap();
        std::fs::write(root.join("input/request.md"), "要求：改短").unwrap();
        std::fs::write(root.join("input/materials/index.json"), "[]").unwrap();
        std::fs::write(run.join("pi/session/main.jsonl"), b"{\"seq\":1}\n").unwrap();
        let workspace = PiWorkspace {
            root,
            state: run.join("pi/session"),
            run: run.clone(),
            label: format!(
                "pi/workspaces/{}/{}",
                doc_dir.file_name().unwrap_or_default().to_string_lossy(),
                name
            ),
        };
        let mut record = record(&workspace);
        record.status = status;
        record.write(&workspace).unwrap();
        run
    }

    /// Running 但没有活动宿主：列历史轮次/清理时纠正为 Interrupted，消息可读。
    #[test]
    fn stale_running_records_are_marked_interrupted_when_no_host_is_alive() {
        let (_config, _reset) = reconcile_config_dir();
        let base = crate::pi_agent::base_dir().unwrap();
        let run = run_under(&base.join("doc-1"), "run-1", PiRunStatus::Running);
        let delivered = run_under(&base.join("doc-2"), "run-2", PiRunStatus::Delivered);

        assert_eq!(reconcile_interrupted_runs().unwrap(), 1);
        let record = PiRunRecord::read_from(&run).unwrap();
        assert_eq!(record.status, PiRunStatus::Interrupted);
        assert!(
            record.message.contains("中断") && record.message.contains("接着写"),
            "纠正后的说明要能指导用户：{}",
            record.message
        );
        // 已经收尾的轮次不动。
        assert_eq!(
            PiRunRecord::read_from(&delivered).unwrap().status,
            PiRunStatus::Delivered
        );
        assert_eq!(reconcile_interrupted_runs().unwrap(), 0, "幂等");
    }

    /// 加密工作区里运行记录是 GWA2：纠正要按当前槽位密钥重新密封，能读回。
    #[test]
    fn reconcile_rewrites_gwa2_records_while_unlocked() {
        let (_config, _reset) = reconcile_config_dir();
        crate::vault::set_key_handle([27; 32], true);
        struct KeyReset;
        impl Drop for KeyReset {
            fn drop(&mut self) {
                crate::vault::clear_key_handle();
            }
        }
        let _key = KeyReset;
        let base = crate::pi_agent::base_dir().unwrap();
        let run = run_under(&base.join("doc-1"), "run-1", PiRunStatus::Running);
        assert!(
            std::fs::read(run.join("run.json"))
                .unwrap()
                .starts_with(b"GWA2"),
            "测试前提：记录应为密文"
        );
        assert_eq!(reconcile_interrupted_runs().unwrap(), 1);
        let record = PiRunRecord::read_from(&run).unwrap();
        assert_eq!(record.status, PiRunStatus::Interrupted);
        assert!(
            std::fs::read(run.join("run.json"))
                .unwrap()
                .starts_with(b"GWA2"),
            "纠正后的记录仍须是密文"
        );
    }

    /// Rust 侧还持着运行所有权锁（另一窗口/本进程正在跑）时不改状态。
    #[test]
    fn a_run_owned_by_a_live_rust_worker_is_not_marked_interrupted() {
        let (_config, _reset) = reconcile_config_dir();
        let base = crate::pi_agent::base_dir().unwrap();
        let run = run_under(&base.join("doc-1"), "run-1", PiRunStatus::Running);
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(run.join("rust-owner.lock"))
            .unwrap();
        lock.try_lock().expect("测试进程先拿到运行所有权锁");
        assert_eq!(reconcile_interrupted_runs().unwrap(), 0);
        assert_eq!(
            PiRunRecord::read_from(&run).unwrap().status,
            PiRunStatus::Running
        );
        drop(lock);
        assert_eq!(reconcile_interrupted_runs().unwrap(), 1);
        assert_eq!(
            PiRunRecord::read_from(&run).unwrap().status,
            PiRunStatus::Interrupted
        );
    }

    /// host 仍持有 SQLite 排他锁（Rust 崩溃但 host 还活着）时不改状态。
    #[test]
    fn a_run_whose_host_still_holds_the_lock_is_left_running() {
        let (_config, _reset) = reconcile_config_dir();
        let base = crate::pi_agent::base_dir().unwrap();
        let run = run_under(&base.join("doc-1"), "run-1", PiRunStatus::Running);
        let held = rusqlite::Connection::open(run.join("host-owner.lock")).unwrap();
        held.execute_batch("BEGIN EXCLUSIVE").unwrap();
        assert_eq!(reconcile_interrupted_runs().unwrap(), 0);
        assert_eq!(
            PiRunRecord::read_from(&run).unwrap().status,
            PiRunStatus::Running
        );
        held.execute_batch("ROLLBACK").unwrap();
        drop(held);
        assert_eq!(reconcile_interrupted_runs().unwrap(), 1);
        assert_eq!(
            PiRunRecord::read_from(&run).unwrap().status,
            PiRunStatus::Interrupted
        );
    }
}
