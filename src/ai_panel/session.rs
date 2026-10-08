//! AI 侧栏会话的持久化（`docs/ai-agent-workbench.md` 16.15 B）。
//!
//! - 每篇稿件可以有多个会话，当前会话的各轮随状态变化写进稿件库（`manuscript::ai_sessions`），
//!   打开稿件时读回；还没入库的新稿只在内存里，第一次入库时整个会话一并落盘；
//! - 一轮存成一段 JSON：卡片上看得到的东西、挂起的流程（技能 id + 技能文本指纹 + 黑板 +
//!   续跑位置）、待确认的提案（改前正文 + 提案正文）；
//! - 读回时：跑到一半的标「已中断」；等回答的照常能答，技能改过的按新版接着跑，技能删了的不能续；
//!   待确认的提案正文没动过就重新装上，动过就标「已过期」。
//!
//! 何时落盘不靠在每处改状态的地方埋点：每帧比对各轮的指纹（只看状态、条数这类便宜的量），
//! 对不上的才序列化重写，最多一秒写一次；关标签、退出前强制补写。

use super::{AiPanel, AiTurn, ReplyDraft, ResearchSnapshot, SkillRun, TurnRequest, TurnState};
use crate::agent::board::Finding;
use crate::agent::checkpoint::Checkpoint;
use crate::agent::clarify::Question;
use crate::agent::engine::Suspension;
use crate::agent::skill::Skill;
use crate::manuscript::ManuscriptStore;
use crate::manuscript::ai_sessions::AiSessionRecord;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// 每篇稿件最多留几个会话。
pub(crate) const MAX_SESSIONS: usize = 20;
/// 两次落盘至少隔这么久（作答时每敲一个字都会改指纹）。
const SAVE_INTERVAL: Duration = Duration::from_secs(1);
/// 思考过程只留最后这么多字：它可能很长，又只是给人翻看的。
const REASONING_KEEP: usize = 20_000;

/// 存进库里的一轮。
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SavedTurn {
    #[serde(default)]
    pub(crate) provenance: String,
    #[serde(default)]
    pub(crate) usage: crate::agent::backend::UsageTotals,
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) context: Vec<String>,
    #[serde(default)]
    pub(crate) request: Option<TurnRequest>,
    pub(crate) state: TurnState,
    #[serde(default)]
    pub(crate) content: String,
    #[serde(default)]
    pub(crate) is_workspace: bool,
    /// 接受提案时排除的变更块数；旧会话没有这个字段，按 0（整篇接受）读回。
    #[serde(default)]
    pub(crate) excluded_hunks: usize,
    #[serde(default)]
    pub(crate) reasoning: String,
    #[serde(default)]
    pub(crate) notes: Vec<String>,
    #[serde(default)]
    pub(crate) steps: Vec<String>,
    #[serde(default)]
    pub(crate) questions: Vec<Question>,
    #[serde(default)]
    pub(crate) replies: Vec<ReplyDraft>,
    /// 这批题的形态；旧会话没有，读回按挂起的流程补（`decision::upgrade`）。
    #[serde(default)]
    pub(crate) decision: crate::agent::decision::Decision,
    #[serde(default)]
    pub(crate) research: Option<ResearchSnapshot>,
    /// 挂起的流程；读不出来时当没存过（这一轮读回为「已中断」），不拖垮整轮。
    #[serde(default, deserialize_with = "lenient_run")]
    pub(crate) run: Option<SavedRun>,
    #[serde(default)]
    pub(crate) findings: Vec<Finding>,
    #[serde(default)]
    pub(crate) elapsed_ms: u64,
    /// 这一轮交的、还没处理的提案。
    #[serde(default)]
    pub(crate) proposal: Option<SavedProposal>,
    /// 风格学习学出、还没保存的档案。
    #[serde(default)]
    pub(crate) style: Option<crate::agent::style::StyleProfile>,
}

/// 挂起的流程。技能本身不存（它在内置资源或配置目录里），只存 id 与文本指纹。
#[derive(Debug, Serialize)]
pub(crate) struct SavedRun {
    pub(crate) skill_id: String,
    pub(crate) skill_hash: String,
    /// 挂起的现场：黑板与接着跑的位置（挂起就是一种检查点，内核加固第 4 期）。
    pub(crate) checkpoint: Checkpoint,
    pub(crate) questions: Vec<Question>,
    pub(crate) save_as: Option<String>,
    pub(crate) decision: crate::agent::decision::Decision,
    pub(crate) use_rag: bool,
}

impl<'de> Deserialize<'de> for SavedRun {
    /// 宽容读回：旧格式（黑板与挂起分开存、`resume_at` 是单个数字）换算成检查点；
    /// 认不出来的交回 Err，外层让这一轮读回为「已中断」，不整轮丢。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        {
            let object = value
                .as_object_mut()
                .ok_or_else(|| serde::de::Error::custom("挂起的流程应当是一个对象"))?;
            if !object.contains_key("checkpoint") {
                let board = object
                    .remove("board")
                    .ok_or_else(|| serde::de::Error::custom("旧格式缺 board"))?;
                let suspension = object
                    .remove("suspension")
                    .ok_or_else(|| serde::de::Error::custom("旧格式缺 suspension"))?;
                let at = match suspension.get("resume_at") {
                    // 旧格式是单个数字；数组照收（中间格式）。
                    Some(serde_json::Value::Number(n)) => {
                        vec![n.as_u64().unwrap_or_default() as usize]
                    }
                    Some(serde_json::Value::Array(path)) => path
                        .iter()
                        .filter_map(serde_json::Value::as_u64)
                        .map(|index| index as usize)
                        .collect(),
                    _ => return Err(serde::de::Error::custom("resume_at 认不出")),
                };
                object.insert(
                    "checkpoint".into(),
                    serde_json::json!({
                        "at": at,
                        "reason": "Ask",
                        "label": "",
                        "board": board,
                        "partial": false,
                    }),
                );
                object.insert(
                    "questions".into(),
                    suspension
                        .get("questions")
                        .cloned()
                        .unwrap_or_else(|| serde_json::Value::Array(vec![])),
                );
                object.insert(
                    "save_as".into(),
                    suspension
                        .get("save_as")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                );
            }
        }
        // 委托给镜像结构体收尾，别再回到自己的 `deserialize`（会无限递归）。
        let data = SavedRunData::deserialize(value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            skill_id: data.skill_id,
            skill_hash: data.skill_hash,
            checkpoint: data
                .checkpoint
                .ok_or_else(|| serde::de::Error::custom("缺 checkpoint"))?,
            questions: data.questions,
            save_as: data.save_as,
            decision: data.decision,
            use_rag: data.use_rag,
        })
    }
}

/// `SavedRun` 反序列化的落点（不带自定义逻辑，见 `SavedRun::deserialize`）。
#[derive(Deserialize)]
struct SavedRunData {
    skill_id: String,
    skill_hash: String,
    checkpoint: Option<Checkpoint>,
    #[serde(default)]
    questions: Vec<Question>,
    #[serde(default)]
    save_as: Option<String>,
    #[serde(default)]
    decision: crate::agent::decision::Decision,
    #[serde(default)]
    use_rag: bool,
}

/// `run` 读不出来时不拖垮整轮：当没存过，读回逻辑会把状态改成「已中断」。
fn lenient_run<'de, D>(deserializer: D) -> Result<Option<SavedRun>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<serde_json::Value>::deserialize(deserializer)?
        .and_then(|value| serde_json::from_value(value).ok()))
}

/// 待确认的提案：改前正文、提案正文与标题。读回时重新过一遍定稿检查再装上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedProposal {
    pub(crate) before: String,
    pub(crate) markdown: String,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) truncated: bool,
}

impl SavedProposal {
    pub(crate) fn of(proposal: &crate::draft_page::AiProposal) -> Self {
        Self {
            before: proposal.before.clone(),
            markdown: proposal.result.markdown.clone(),
            label: proposal.label.clone(),
            truncated: proposal
                .result
                .warnings
                .iter()
                .any(|note| note.message.starts_with(super::TRUNCATED_NOTE)),
        }
    }
}

/// 技能文本的指纹：流程与提示词变了，挂起的流程续跑时要提示一句。
pub(crate) fn skill_hash(skill: &Skill) -> String {
    use sha2::{Digest, Sha256};
    let text = format!("{:?}\n{:?}\n{:?}", skill.flow, skill.sections, skill.params);
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

impl SavedTurn {
    fn of(turn: &AiTurn, proposal: Option<SavedProposal>) -> Self {
        let reasoning = {
            let total = turn.reasoning.chars().count();
            if total > REASONING_KEEP {
                let tail: String = turn
                    .reasoning
                    .chars()
                    .skip(total - REASONING_KEEP)
                    .collect();
                format!("……（前面 {} 字略）\n{tail}", total - REASONING_KEEP)
            } else {
                turn.reasoning.clone()
            }
        };
        Self {
            id: turn.id,
            usage: turn.usage.clone(),
            provenance: turn.provenance.clone(),
            title: turn.title.clone(),
            prompt: turn.prompt.clone(),
            context: turn.context.clone(),
            request: turn.request.clone(),
            state: turn.state.clone(),
            excluded_hunks: turn.excluded_hunks,
            content: turn.content.clone(),
            is_workspace: turn.is_workspace,
            reasoning,
            notes: turn.notes.clone(),
            steps: turn.steps.clone(),
            questions: turn.questions.clone(),
            replies: turn.replies.clone(),
            decision: turn.decision.clone(),
            research: turn.research.clone(),
            run: turn.run.as_ref().map(|run| SavedRun {
                skill_id: run.skill.id.clone(),
                skill_hash: skill_hash(&run.skill),
                checkpoint: run.suspension.checkpoint.clone(),
                questions: run.suspension.questions.clone(),
                save_as: run.suspension.save_as.clone(),
                decision: run.suspension.decision.clone(),
                use_rag: run.use_rag,
            }),
            findings: turn.findings.clone(),
            elapsed_ms: turn.elapsed().as_millis() as u64,
            proposal,
            style: turn.style.clone(),
        }
    }

    /// 读回成一轮。`document` 是现在的正文，`proposal_open` 为真表示这一轮的提案还能装回去
    /// （由调用方判断正文没动过、且它是最后一份提案）。返回这一轮与要重新装上的提案。
    fn restore(self, skills: &[Skill], proposal_live: bool) -> (AiTurn, Option<SavedProposal>) {
        let mut notes = self.notes;
        let mut questions = self.questions;
        let mut replies = self.replies;
        let mut run = None;
        let mut proposal = None;
        let state = match self.state {
            state if state.running() => TurnState::Interrupted,
            TurnState::Asking => match self.run {
                Some(saved) => match skills.iter().find(|skill| skill.id == saved.skill_id) {
                    Some(skill) => {
                        if skill_hash(skill) != saved.skill_hash {
                            notes.push(format!(
                                "技能「{}」的内容改过了，答完按新版接着跑。",
                                skill.name
                            ));
                        }
                        let SavedRun {
                            checkpoint,
                            questions: saved_questions,
                            save_as,
                            decision,
                            use_rag,
                            ..
                        } = saved;
                        let mut suspension = Suspension {
                            checkpoint,
                            questions: saved_questions,
                            save_as,
                            decision,
                        };
                        crate::agent::decision::upgrade(skill, &mut suspension);
                        run = Some(Box::new(SkillRun {
                            skill: skill.clone(),
                            suspension,
                            use_rag,
                        }));
                        TurnState::Asking
                    }
                    None => {
                        questions.clear();
                        replies.clear();
                        TurnState::Failed(format!(
                            "技能「{}」已经不在了，没法接着跑；可以重新生成。",
                            saved.skill_id
                        ))
                    }
                },
                None => TurnState::Interrupted,
            },
            TurnState::Proposed(summary) => {
                if proposal_live && self.proposal.is_some() {
                    proposal = self.proposal;
                    TurnState::Proposed(summary)
                } else {
                    questions.clear();
                    replies.clear();
                    TurnState::Expired
                }
            }
            other => other,
        };
        let is_workspace = self.is_workspace || matches!(state, TurnState::Proposed(_));
        let decision = run
            .as_ref()
            .map_or(self.decision, |run| run.suspension.decision.clone());
        let turn = AiTurn {
            usage_job_seq: None,
            provenance: self.provenance,
            usage: self.usage,
            id: self.id,
            title: self.title,
            prompt: self.prompt,
            context: self.context,
            request: self.request,
            state,
            excluded_hunks: self.excluded_hunks,
            content: self.content,
            is_workspace,
            stream_suffix: String::new(),
            reasoning: self.reasoning,
            phase: String::new(),
            notes,
            steps: self.steps,
            questions,
            replies,
            decision,
            research: self.research,
            run,
            // 检查点存在库里：读回后由 `refresh_resumable` 按库里最新一份填。
            resumable: None,
            findings: self.findings,
            style: self.style,
            started: Instant::now(),
            elapsed: Some(Duration::from_millis(self.elapsed_ms)),
        };
        (turn, proposal)
    }
}

/// 当前会话的本机状态。
#[derive(Debug, Default)]
pub(crate) struct Session {
    /// UUID；空表示还没开过（第一轮发出时补上）。
    pub(crate) id: String,
    pub(crate) title: String,
    /// 压缩出的会话摘要（16.15 A.5），只用于后续提示词。
    pub(crate) summary: String,
    /// 摘要覆盖到的最后一轮。
    pub(crate) compacted_upto: u64,
    pub(crate) created_at: String,
    /// 已经和哪篇稿件对齐过；None 表示还没入库，会话只在内存里。
    pub(crate) loaded_for: Option<i64>,
    /// 各轮上次落盘时的指纹。
    saved: BTreeMap<u64, u64>,
    saved_meta: Option<u64>,
    last_save: Option<Instant>,
    /// 历史会话列表的缓存，打开列表时重读。
    pub(crate) list: Vec<AiSessionRecord>,
}

impl Session {
    /// 下一次落盘时重写会话抬头（切过来的会话要标成当前）。
    pub(crate) fn force_meta_write(&mut self) {
        self.saved_meta = Some(0);
    }

    fn fresh() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: now(),
            ..Self::default()
        }
    }
}

fn now() -> String {
    chrono::Local::now().to_rfc3339()
}

/// 一轮的指纹：状态、各列表条数与作答。流式输出时不变（跑着的轮次只看「在跑」），
/// 免得每批增量都触发落盘。
fn fingerprint(turn: &AiTurn, has_proposal: bool) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if turn.state.running() {
        "running".hash(&mut hasher);
        // 优化清单时输入框仍可编辑：这些输入即使任务在跑也要触发保存。
        if turn
            .replies
            .first()
            .is_some_and(|reply| reply.revise_base.is_some())
        {
            format!("{:?}", turn.replies).hash(&mut hasher);
        }
    } else {
        format!("{:?}", turn.state).hash(&mut hasher);
        turn.excluded_hunks.hash(&mut hasher);
        turn.content.len().hash(&mut hasher);
        turn.notes.len().hash(&mut hasher);
        turn.steps.len().hash(&mut hasher);
        turn.questions.len().hash(&mut hasher);
        format!("{:?}", turn.replies).hash(&mut hasher);
        turn.findings.len().hash(&mut hasher);
        turn.research.is_some().hash(&mut hasher);
        turn.style.is_some().hash(&mut hasher);
        turn.run.is_some().hash(&mut hasher);
        turn.title.hash(&mut hasher);
    }
    has_proposal.hash(&mut hasher);
    turn.usage.calls.len().hash(&mut hasher);
    turn.provenance.hash(&mut hasher);
    hasher.finish()
}

impl AiPanel {
    /// 会话标题：用户改过的，或第一句原话。
    pub(crate) fn session_title(&self) -> String {
        if !self.session.title.trim().is_empty() {
            return self.session.title.clone();
        }
        self.turns
            .first()
            .map(|turn| crate::agent::tools::short(turn.prompt.trim(), 24))
            .unwrap_or_else(|| "新会话".into())
    }

    /// 最后一份提案在哪一轮（只有它的提案还挂在稿件上）。
    fn proposal_turn(&self) -> Option<u64> {
        self.turns
            .iter()
            .rev()
            .find(|turn| matches!(turn.state, TurnState::Proposed(_)))
            .map(|turn| turn.id)
    }

    fn meta_fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.session_title().hash(&mut hasher);
        self.session.summary.hash(&mut hasher);
        self.session.compacted_upto.hash(&mut hasher);
        self.open.hash(&mut hasher);
        self.turns
            .iter()
            .map(|turn| turn.id)
            .collect::<Vec<_>>()
            .hash(&mut hasher);
        hasher.finish()
    }

    /// 把改过的轮次写进库。`proposal` 是稿件上挂着的提案（跟着最后一份提案那一轮存）。
    /// 还没入库、会话还空着、没改动、离上次写不到一秒（`force` 除外）时什么都不做。
    /// 强制保存时补写运行中的轮次，保留普通指纹刻意跳过的流式输出与过程记录。
    pub(crate) fn save_session(
        &mut self,
        manuscript_id: Option<i64>,
        store: Option<&mut ManuscriptStore>,
        proposal: Option<&dyn Fn() -> SavedProposal>,
        force: bool,
    ) -> Option<String> {
        let id = manuscript_id?;
        let store = store?;
        if self.session.loaded_for != Some(id) {
            // 还没和库对齐（由 `DraftPage::sync_ai_session` 负责读回），先不写。
            return None;
        }
        if self.turns.is_empty() && self.session.saved_meta.is_none() {
            return None;
        }
        let proposal_turn = self.proposal_turn();
        let changed: Vec<&AiTurn> = self
            .turns
            .iter()
            .filter(|turn| {
                if force && turn.state.running() {
                    return true;
                }
                let print = fingerprint(turn, proposal_turn == Some(turn.id) && proposal.is_some());
                self.session.saved.get(&turn.id) != Some(&print)
            })
            .collect();
        let meta = self.meta_fingerprint();
        if changed.is_empty() && self.session.saved_meta == Some(meta) {
            return None;
        }
        if !force
            && self
                .session
                .last_save
                .is_some_and(|at| at.elapsed() < SAVE_INTERVAL)
        {
            return None;
        }
        if self.session.id.is_empty() {
            self.session = Session {
                loaded_for: Some(id),
                ..Session::fresh()
            };
        }
        let mut rows = Vec::new();
        let mut prints = Vec::new();
        for turn in changed {
            // 提案只在真要写那一轮时才拷出来：它是整篇正文，不必每帧都复制。
            let attached = proposal
                .filter(|_| proposal_turn == Some(turn.id))
                .map(|make| make());
            let print = fingerprint(turn, attached.is_some());
            match serde_json::to_string(&SavedTurn::of(turn, attached)) {
                Ok(json) => {
                    rows.push((turn.id as i64, json));
                    prints.push((turn.id, print));
                }
                Err(error) => return Some(format!("保存 AI 会话失败：{error}")),
            }
        }
        let record = AiSessionRecord {
            id: self.session.id.clone(),
            title: self.session_title(),
            summary: self.session.summary.clone(),
            compacted_upto: self.session.compacted_upto as i64,
            is_current: true,
            panel_open: self.open,
            created_at: if self.session.created_at.is_empty() {
                now()
            } else {
                self.session.created_at.clone()
            },
            updated_at: now(),
            turns: self.turns.len(),
        };
        let keep: Vec<i64> = self.turns.iter().map(|turn| turn.id as i64).collect();
        self.session.last_save = Some(Instant::now());
        let first = self.session.saved_meta.is_none();
        match store.save_ai_session(id, &record, &rows, &keep) {
            Ok(()) => {
                self.session
                    .saved
                    .retain(|turn, _| keep.contains(&(*turn as i64)));
                self.session.saved.extend(prints);
                self.session.saved_meta = Some(meta);
                if first {
                    // 新会话第一次落盘：顺手裁掉多出来的旧会话。
                    let _ = store.prune_ai_sessions(id, MAX_SESSIONS);
                }
                None
            }
            Err(error) => Some(format!("保存 AI 会话失败：{error:#}")),
        }
    }

    /// 换成一个空白的新会话。调用方先把当前会话写下去。
    pub(crate) fn start_new_session(&mut self) {
        let loaded_for = self.session.loaded_for;
        let list = std::mem::take(&mut self.session.list);
        self.turns.clear();
        self.workspace_view = None;
        self.workspace_compare = false;
        self.session = Session {
            loaded_for,
            list,
            ..Session::fresh()
        };
    }

    /// 把库里的一个会话装进侧栏，返回要重新装上的提案（正文没动过时）。
    pub(crate) fn load_session(
        &mut self,
        manuscript_id: i64,
        record: &AiSessionRecord,
        rows: Vec<(i64, String)>,
        document: &str,
    ) -> (Option<SavedProposal>, Vec<String>) {
        let skills = if self.skills.is_empty() {
            crate::agent::skill::load_all().0
        } else {
            self.skills.clone()
        };
        let mut problems = Vec::new();
        let mut saved: Vec<SavedTurn> = Vec::new();
        for (turn_id, json) in rows {
            match serde_json::from_str::<SavedTurn>(&json) {
                Ok(turn) => saved.push(turn),
                Err(error) => problems.push(format!("第 {turn_id} 轮读不出来：{error}")),
            }
        }
        let last_proposal = saved
            .iter()
            .rev()
            .find(|turn| matches!(turn.state, TurnState::Proposed(_)))
            .map(|turn| turn.id);
        let mut restored = None;
        self.turns = saved
            .into_iter()
            .map(|turn| {
                let live = Some(turn.id) == last_proposal
                    && turn.proposal.as_ref().is_some_and(|p| p.before == document);
                let (turn, proposal) = turn.restore(&skills, live);
                if proposal.is_some() {
                    restored = proposal;
                }
                turn
            })
            .collect();
        self.workspace_view = None;
        self.workspace_compare = false;
        self.next_id = self.turns.iter().map(|turn| turn.id).max().unwrap_or(0);
        let list = std::mem::take(&mut self.session.list);
        self.session = Session {
            id: record.id.clone(),
            title: record.title.clone(),
            summary: record.summary.clone(),
            compacted_upto: record.compacted_upto.max(0) as u64,
            created_at: record.created_at.clone(),
            loaded_for: Some(manuscript_id),
            list,
            ..Session::default()
        };
        // 读回的各轮按原样记作已存；状态被改写的（中断、过期）下次照常写回去。
        let proposal_turn = self.proposal_turn();
        for turn in &self.turns {
            if !matches!(turn.state, TurnState::Interrupted | TurnState::Expired)
                && !(proposal_turn == Some(turn.id) && restored.is_none())
            {
                let print = fingerprint(turn, proposal_turn == Some(turn.id));
                self.session.saved.insert(turn.id, print);
            }
        }
        self.session.saved_meta = Some(self.meta_fingerprint());
        (restored, problems)
    }

    /// 新稿第一次入库（或另存成新稿）：内存里的会话整个落盘。原来挂在别的稿件上的换个新 id，
    /// 不去动那篇的记录。
    pub(crate) fn adopt_session(&mut self, manuscript_id: i64) {
        let moved = self.session.loaded_for.is_some();
        self.session.loaded_for = Some(manuscript_id);
        self.session.saved.clear();
        self.session.saved_meta = None;
        if moved {
            self.session.id = uuid::Uuid::new_v4().to_string();
        }
        if self.session.id.is_empty() {
            let list = std::mem::take(&mut self.session.list);
            self.session = Session {
                loaded_for: Some(manuscript_id),
                list,
                ..Session::fresh()
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::board::Board;
    use crate::agent::clarify::{Action, Choice, Target};
    use crate::ai_panel::ProposalSummary;
    use crate::manuscript::NewManuscript;
    use crate::models::ManuscriptStatus;
    use std::path::Path;

    fn store() -> (ManuscriptStore, i64) {
        let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
        let id = store
            .create(
                &NewManuscript {
                    content_markdown: "正文。\n".into(),
                    status: ManuscriptStatus::Draft,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        (store, id)
    }

    fn question() -> Question {
        Question {
            id: 1,
            text: "会议时间？".into(),
            choices: vec![Choice {
                label: "下周一".into(),
                detail: String::new(),
                recommended: true,
                action: Action::Note("会议时间：下周一".into()),
            }],
            custom_hint: Some("自己填".into()),
            prefill: String::new(),
            skippable: true,
            target: Target::PreDraft,
        }
    }

    /// 存一个有三种状态的会话：问题清单、等回答、待确认的提案。
    fn saved_panel(store: &mut ManuscriptStore, id: i64) -> AiPanel {
        let mut panel = AiPanel {
            open: true,
            ..AiPanel::default()
        };
        panel.session.loaded_for = Some(id);
        panel.push_turn("问题清单".into(), "审一下".into(), vec![], None);
        panel.turns[0].findings = vec![Finding {
            group: "表述".into(),
            text: "病句".into(),
            excerpt: String::new(),
            source: String::new(),
            fix: None,
        }];
        panel.finish(TurnState::Reported { fixes: 0 });
        panel.push_turn("起草".into(), "写个通知".into(), vec![], None);
        let skill = crate::agent::skill::builtin(crate::agent::skill::RESEARCH_DRAFT).unwrap();
        panel.ask(Box::new(SkillRun {
            skill,
            suspension: Suspension {
                checkpoint: Checkpoint {
                    at: vec![2],
                    reason: crate::agent::checkpoint::Reason::Ask,
                    label: String::new(),
                    board: Board {
                        request: "写个通知".into(),
                        ..Board::default()
                    },
                    partial: false,
                },
                questions: vec![question()],
                save_as: None,
                decision: Default::default(),
            },
            use_rag: false,
        }));
        panel.push_turn("润色".into(), "压一压".into(), vec![], None);
        panel.finish(TurnState::Proposed(super::super::ProposalSummary::default()));
        let proposal = SavedProposal {
            before: "正文。\n".into(),
            markdown: "压过的正文。\n".into(),
            label: "润色".into(),
            truncated: false,
        };
        assert!(
            panel
                .save_session(Some(id), Some(store), Some(&|| proposal.clone()), true)
                .is_none()
        );
        panel
    }

    #[test]
    fn a_session_survives_a_restart() {
        let (mut store, id) = store();
        let before = saved_panel(&mut store, id);
        let records = store.list_ai_sessions(id).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].panel_open);
        assert_eq!(records[0].title, "审一下");
        let rows = store.load_ai_session_turns(&records[0].id).unwrap();
        assert_eq!(rows.len(), 3);

        let mut after = AiPanel::default();
        let (proposal, problems) = after.load_session(id, &records[0], rows, "正文。\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(after.session.id, before.session.id);
        let states: Vec<&TurnState> = after.turns.iter().map(|t| &t.state).collect();
        assert!(matches!(states[0], TurnState::Reported { fixes: 0 }));
        assert_eq!(after.turns[0].findings.len(), 1);
        assert_eq!(*states[1], TurnState::Asking, "等回答的照常能答");
        let run = after.turns[1].run.as_ref().expect("挂起的流程读回来了");
        assert_eq!(run.suspension.checkpoint.at, [2]);
        assert_eq!(run.suspension.checkpoint.board.request, "写个通知");
        assert_eq!(after.turns[1].questions.len(), 1);
        assert!(
            matches!(states[2], TurnState::Proposed(_)),
            "正文没动，提案还在"
        );
        assert_eq!(proposal.unwrap().markdown, "压过的正文。\n");
        // 再发一轮，编号接着往后排。
        let next = after.push_turn("x".into(), "y".into(), vec![], None);
        assert_eq!(next, 4);
    }

    #[test]
    fn editing_an_outline_during_optimization_triggers_saving_and_survives_restart() {
        let (mut store, id) = store();
        let mut panel = saved_panel(&mut store, id);
        let turn = &mut panel.turns[1];
        turn.resume();
        turn.replies[0].revise_base = Some("背景：原大纲".into());
        turn.replies[0].custom = "背景：原大纲".into();
        let before = fingerprint(turn, false);
        turn.replies[0].custom = "背景：等待期间的手工修改".into();
        turn.replies[0].revise_instruction = "加一章风险分析".into();
        turn.replies[0].revise_candidate = "风险：AI候选".into();
        assert_ne!(fingerprint(turn, false), before);
        let encoded = serde_json::to_string(&SavedTurn::of(turn, None)).unwrap();
        let saved: SavedTurn = serde_json::from_str(&encoded).unwrap();
        let (restored, _) = saved.restore(&crate::agent::skill::load_all().0, false);
        assert_eq!(restored.state, TurnState::Interrupted);
        assert_eq!(restored.replies, turn.replies);
    }

    #[test]
    fn provenance_and_usage_survive_acceptance_and_old_sessions_load() {
        let mut panel = AiPanel::default();
        panel.push_turn("起草".into(), "写稿".into(), vec![], None);
        panel.turns[0].provenance = super::super::proposal_source(
            "研究式起草",
            &["主模型".into()],
            &[1, 3],
            "2026-10-07 14:22",
        );
        panel.turns[0].usage.record(
            crate::agent::backend::ModelRole::Draft,
            "主模型",
            10,
            "稿",
            None,
            1,
        );
        panel.finish(TurnState::Proposed(Default::default()));
        panel.resolve_proposal(true, 0);
        let source = panel.turns[0].provenance.clone();
        let saved = SavedTurn::of(&panel.turns[0], None);
        let mut json = serde_json::to_value(saved).unwrap();
        let (restored, _) = serde_json::from_value::<SavedTurn>(json.clone())
            .unwrap()
            .restore(&[], false);
        assert_eq!(restored.state, TurnState::Accepted);
        assert_eq!(restored.provenance, source);
        assert!(source.contains("证据 K1、K3"));
        assert_eq!(restored.usage.calls.len(), 1);
        json.as_object_mut().unwrap().remove("usage");
        json.as_object_mut().unwrap().remove("provenance");
        let old: SavedTurn = serde_json::from_value(json).unwrap();
        assert!(old.usage.calls.is_empty());
        assert!(old.provenance.is_empty());
    }

    #[test]
    fn partial_acceptance_preserves_usage_and_provenance_together() {
        let mut panel = AiPanel::default();
        panel.push_turn("起草".into(), "写稿".into(), vec![], None);
        panel.turns[0].provenance = "研究式起草 · 备用模型 · 证据 K1、K3 · 2026-10-07 14:22".into();
        panel.turns[0].usage.record(
            crate::agent::backend::ModelRole::Draft,
            "备用模型",
            10,
            "稿",
            None,
            1,
        );
        panel.finish(TurnState::Proposed(Default::default()));
        panel.resolve_proposal(true, 2);
        let json = serde_json::to_string(&SavedTurn::of(&panel.turns[0], None)).unwrap();
        let (restored, _) = serde_json::from_str::<SavedTurn>(&json)
            .unwrap()
            .restore(&[], false);
        assert_eq!(restored.state, TurnState::Accepted);
        assert_eq!(restored.excluded_hunks, 2);
        assert_eq!(restored.provenance, panel.turns[0].provenance);
        assert_eq!(restored.usage, panel.turns[0].usage);
    }

    #[test]
    fn resuming_a_turn_keeps_elapsed_time_and_existing_usage() {
        let mut panel = AiPanel::default();
        panel.push_turn("起草".into(), "写稿".into(), vec![], None);
        let turn = &mut panel.turns[0];
        turn.elapsed = Some(Duration::from_secs(12));
        turn.usage.record(
            crate::agent::backend::ModelRole::Draft,
            "m",
            1,
            "稿",
            None,
            1,
        );
        turn.resume();
        assert!(turn.elapsed() >= Duration::from_secs(12));
        assert_eq!(turn.usage.calls.len(), 1);
    }

    #[test]
    fn stopped_workspace_survives_restart_and_session_switch_resets_the_view() {
        let mut panel = AiPanel::default();
        panel.push_turn("起草".into(), "写稿".into(), vec![], None);
        panel.begin_write("前文".into(), "后文".into());
        panel.append("补写", "", false);
        panel.finish(TurnState::Stopped);
        let json = serde_json::to_string(&SavedTurn::of(&panel.turns[0], None)).unwrap();
        let saved: SavedTurn = serde_json::from_str(&json).unwrap();
        let (turn, _) = saved.restore(&[], false);
        assert!(turn.is_workspace);
        assert_eq!(turn.content, "前文补写后文");
        assert!(turn.stream_suffix.is_empty());
        panel.start_new_session();
        assert_eq!(panel.workspace_view, None);
        assert!(panel.turns.is_empty());
    }

    #[test]
    fn a_running_turn_comes_back_interrupted() {
        let (mut store, id) = store();
        let mut panel = AiPanel::default();
        panel.session.loaded_for = Some(id);
        panel.push_turn("精简".into(), "再短一点".into(), vec![], None);
        panel.append("半截", "", false);
        panel.save_session(Some(id), Some(&mut store), None, true);
        let record = store.list_ai_sessions(id).unwrap().remove(0);
        let rows = store.load_ai_session_turns(&record.id).unwrap();
        let mut after = AiPanel::default();
        after.load_session(id, &record, rows, "");
        assert_eq!(after.turns[0].state, TurnState::Interrupted);
        assert!(!after.running());
    }

    #[test]
    fn closing_flushes_the_latest_running_output() {
        let (mut store, id) = store();
        let mut panel = AiPanel::default();
        panel.session.loaded_for = Some(id);
        panel.push_turn("起草".into(), "写个通知".into(), vec![], None);
        panel.append("开头", "先想一想", false);
        assert!(
            panel
                .save_session(Some(id), Some(&mut store), None, true)
                .is_none()
        );
        let session_id = panel.session.id.clone();
        let initial_rows = store.load_ai_session_turns(&session_id).unwrap();

        panel.append("和后续正文", "再想一想", false);
        panel.note("已读材料".into());
        panel.step("检查工作稿".into());
        // 普通逐帧保存仍跳过流式增量，不反复写库。
        assert!(
            panel
                .save_session(Some(id), Some(&mut store), None, false)
                .is_none()
        );
        assert_eq!(
            store.load_ai_session_turns(&session_id).unwrap(),
            initial_rows
        );

        // 关标签、退出前强制补写，即使状态指纹没变且尚未到一秒。
        assert!(
            panel
                .save_session(Some(id), Some(&mut store), None, true)
                .is_none()
        );
        let record = store.list_ai_sessions(id).unwrap().remove(0);
        let rows = store.load_ai_session_turns(&session_id).unwrap();
        let mut after = AiPanel::default();
        let (_, problems) = after.load_session(id, &record, rows, "");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(after.turns[0].state, TurnState::Interrupted);
        assert_eq!(after.turns[0].content, panel.turns[0].content);
        assert_eq!(after.turns[0].reasoning, panel.turns[0].reasoning);
        assert_eq!(after.turns[0].notes, panel.turns[0].notes);
        assert_eq!(after.turns[0].steps, panel.turns[0].steps);
    }

    #[test]
    fn a_proposal_expires_once_the_document_changed() {
        let (mut store, id) = store();
        saved_panel(&mut store, id);
        let record = store.list_ai_sessions(id).unwrap().remove(0);
        let rows = store.load_ai_session_turns(&record.id).unwrap();
        let mut after = AiPanel::default();
        let (proposal, _) = after.load_session(id, &record, rows, "别人改过的正文。\n");
        assert!(proposal.is_none());
        assert_eq!(after.turns[2].state, TurnState::Expired);
    }

    #[test]
    fn only_changed_turns_are_written_and_removed_ones_are_dropped() {
        let (mut store, id) = store();
        let mut panel = saved_panel(&mut store, id);
        let session = panel.session.id.clone();
        // 没改动：不写（也不受一秒间隔影响）。
        assert!(
            panel
                .save_session(Some(id), Some(&mut store), None, true)
                .is_none()
        );
        panel.clear_history();
        panel.save_session(Some(id), Some(&mut store), None, true);
        assert!(
            store.load_ai_session_turns(&session).unwrap().is_empty(),
            "清空记录连库里一起清"
        );
        panel.push_turn("精简".into(), "再短一点".into(), vec![], None);
        panel.finish(TurnState::Stopped);
        panel.save_session(Some(id), Some(&mut store), None, true);
        let rows = store.load_ai_session_turns(&session).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].1.contains("\"Stopped\""), "{}", rows[0].1);
    }

    #[test]
    fn unsaved_documents_keep_the_session_in_memory_until_adopted() {
        let (mut store, id) = store();
        let mut panel = AiPanel::default();
        panel.push_turn("起草".into(), "写个通知".into(), vec![], None);
        panel.finish(TurnState::Stopped);
        assert!(
            panel
                .save_session(None, Some(&mut store), None, true)
                .is_none()
        );
        assert!(store.list_ai_sessions(id).unwrap().is_empty());
        panel.adopt_session(id);
        panel.save_session(Some(id), Some(&mut store), None, true);
        let records = store.list_ai_sessions(id).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].turns, 1);
    }

    #[test]
    fn a_deleted_skill_cannot_be_resumed() {
        let saved = SavedTurn {
            provenance: String::new(),
            usage: Default::default(),
            id: 1,
            title: "x".into(),
            prompt: "y".into(),
            context: vec![],
            request: None,
            state: TurnState::Asking,
            content: String::new(),
            excluded_hunks: 0,
            is_workspace: false,
            reasoning: String::new(),
            notes: vec![],
            steps: vec![],
            questions: vec![question()],
            replies: vec![ReplyDraft::default()],
            decision: Default::default(),
            research: None,
            run: Some(SavedRun {
                skill_id: "gone".into(),
                skill_hash: String::new(),
                checkpoint: Checkpoint {
                    at: vec![1],
                    reason: crate::agent::checkpoint::Reason::Ask,
                    label: String::new(),
                    board: Board::default(),
                    partial: false,
                },
                questions: vec![],
                save_as: None,
                decision: Default::default(),
                use_rag: false,
            }),
            findings: vec![],
            elapsed_ms: 1200,
            proposal: None,
            style: None,
        };
        let (turn, _) = saved.restore(&[], false);
        assert!(matches!(&turn.state, TurnState::Failed(why) if why.contains("不在了")));
        assert!(turn.run.is_none());
        assert_eq!(turn.elapsed, Some(Duration::from_millis(1200)));
    }

    /// 第 2 期前的会话没有 excluded_hunks 字段，按 0（整篇接受）读回；带排除数的
    /// 新会话往返不丢（内核加固第 2 期）。
    #[test]
    fn excluded_hunks_survive_persistence_and_old_sessions_default_to_zero() {
        let old = r#"{"id":1,"title":"润色","prompt":"改","state":"Accepted"}"#;
        let saved: SavedTurn = serde_json::from_str(old).unwrap();
        assert_eq!(saved.excluded_hunks, 0);
        let (turn, _) = saved.restore(&[], false);
        assert_eq!(turn.state, TurnState::Accepted);
        assert_eq!(turn.excluded_hunks, 0);

        let mut panel = AiPanel::default();
        panel.push_turn("润色".into(), "改".into(), vec![], None);
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        panel.resolve_proposal(true, 2);
        let json = serde_json::to_string(&SavedTurn::of(&panel.turns[0], None)).unwrap();
        let saved: SavedTurn = serde_json::from_str(&json).unwrap();
        let (turn, _) = saved.restore(&[], false);
        assert_eq!(turn.excluded_hunks, 2);
    }

    /// 内核加固第 4 期前的会话：`resume_at` 是单个数字、黑板与挂起分开存，读回能续；
    /// 认不出来的挂起数据不拖垮整轮，读回为「已中断」。
    #[test]
    fn old_sessions_with_a_numeric_resume_at_still_resume() {
        let old = r#"{
            "id": 1, "title": "起草", "prompt": "写个通知", "state": "Asking",
            "questions": [],
            "run": {
                "skill_id": "research-draft", "skill_hash": "abc",
                "board": {"request": "写个通知"},
                "suspension": {"questions": [], "resume_at": 2, "save_as": null},
                "use_rag": false
            }
        }"#;
        let saved: SavedTurn = serde_json::from_str(old).unwrap();
        let run = saved.run.expect("旧格式的挂起读回来了");
        assert_eq!(run.checkpoint.at, [2], "数字 resume_at 换算成路径");
        assert_eq!(run.checkpoint.board.request, "写个通知");

        let bad = r#"{
            "id": 2, "title": "起草", "prompt": "写个通知", "state": "Asking",
            "run": {"skill_id": "x", "suspension": {"resume_at": "第二步"}}
        }"#;
        let saved: SavedTurn = serde_json::from_str(bad).unwrap();
        assert!(saved.run.is_none(), "认不出来的挂起数据当没存过");
        let (turn, _) = saved.restore(&[], false);
        assert_eq!(
            turn.state,
            TurnState::Interrupted,
            "整轮读回为「已中断」，不丢"
        );
    }
}
