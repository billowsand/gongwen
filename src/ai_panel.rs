//! 起草页右侧的 AI 侧栏。
//!
//! 新请求使用 Pi 文件工作区与标准 SKILL.md；`/` 选择 Pi 技能，`@` 引用文章。
//! 旧流程仅保留既有会话的展示、恢复及共享服务。结果经确定性闸门和用户接受才进入正文。
//!
//! 本文件只放状态与纯逻辑；界面在 `ai_panel/ui.rs`（任务流）与 `ai_panel/composer_ui.rs`
//! （输入框），`/`、`@` 的识别与过滤在 `ai_panel/mention.rs`。

use crate::agent::clarify::{Question, Reply};
use crate::agent::gaps::Ledger;
use crate::agent::skill::Skill;
use crate::ai_services::references::Reference;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

mod appearance;
mod checkpoint;
mod commit_intent;
mod composer_ui;
pub(crate) mod field_card;
mod history;
mod mention;
mod paste;
pub(crate) mod pi_ask;
pub(crate) mod pi_job;
pub(crate) mod pi_prepare;
pub(crate) mod pi_write;
mod provenance;
pub(crate) use provenance::source_line as proposal_source;
pub(crate) mod session;
mod session_ui;
pub(crate) mod skill_job;
mod ui;
mod workspace_ui;

pub(crate) use skill_job::{
    GapRevision, SkillResult, SkillRun, finish_research_revision, initial_replies,
};

/// 输入区的状态。随稿件保存，切换标签页互不影响。
#[derive(Debug, Default)]
pub(crate) struct Composer {
    pub(crate) text: String,
    /// 打开侧栏时锁定的选区（字节区间）与当时的原文。正文被改动后按原文重新定位。
    pub(crate) selection: Option<(Range<usize>, String)>,
    pub(crate) error: Option<String>,
    /// `@` 引用的文章；输入框里对应留着 `@《标题》` 记号，记号删了引用也就没了。
    pub(crate) refs: Vec<Reference>,
    /// 大段粘贴生成的临时材料，标签可单独移除。
    pub(crate) materials: Vec<paste::PastedFile>,
    /// `/`、`@` 弹出层的状态。
    pub(crate) popup: mention::PopupState,
    /// 有待确认的提案时，修改类技能改提案而不是改正文（16.15 B.7）。默认是；底栏的「改提案」
    /// 标签点掉就回到改正文。
    pub(crate) skip_proposal: bool,
    /// 用户说了「把工作稿提交到正文」（`commit_intent`）：输入框上方出「写入正文？」确认卡。
    pub(crate) commit_prompt: bool,
    /// Pi 入口用哪份标准技能（`assets/pi-skills/`）；None 表示自动。
    pub(crate) pi_skill: Option<String>,
    /// 统一写作入口的策略倾向：自动 / 倾向分阶段。
    pub(crate) pi_strategy: crate::pi_agent::PiStrategyPreference,
    /// 新粘贴材料的默认用途（旧字段保留，作为没单独设置过的材料的回退值）。
    pub(crate) pi_material_role: crate::pi_agent::MaterialRole,
    /// 逐篇粘贴材料的用途，与 `materials` 同下标；旧会话没有这项就用默认值。
    pub(crate) pi_material_roles: Vec<crate::pi_agent::MaterialRole>,
    /// `@` 引用的默认用途。默认事实材料，可改成样稿让模型只学写法。
    pub(crate) pi_ref_role: crate::pi_agent::MaterialRole,
    /// 逐篇 `@` 引用的用途，按「来源 + 编号」存，避免同名不同来源串用。
    pub(crate) pi_ref_roles: Vec<((String, i64), crate::pi_agent::MaterialRole)>,
    /// Pi 追问时保留的已确认回答（只来自用户点选 / 填写，不是模型总结）。
    pub(crate) pi_notes: Vec<String>,
}

impl Composer {
    /// 第 `index` 份粘贴材料的用途；旧会话没有逐篇设置时回退到默认值。
    pub(crate) fn material_role(&self, index: usize) -> crate::pi_agent::MaterialRole {
        self.pi_material_roles
            .get(index)
            .copied()
            .unwrap_or(self.pi_material_role)
    }

    /// 某篇 `@` 引用的用途。键是来源 + 编号，不复用标题：同名不同库的两篇各算各的。
    pub(crate) fn ref_role(
        &self,
        source: crate::ai_services::references::RefSource,
        id: i64,
    ) -> crate::pi_agent::MaterialRole {
        let key = (source.label().to_string(), id);
        self.pi_ref_roles
            .iter()
            .find(|(saved, _)| *saved == key)
            .map(|(_, role)| *role)
            .unwrap_or(self.pi_ref_role)
    }
}
/// 一轮请求的原始参数，「重新生成」照它再发一次。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TurnRequest {
    /// 指定的技能 id；None 表示按上下文与触发词自动选。
    pub(crate) skill: Option<String>,
    pub(crate) text: String,
    pub(crate) selection: Option<(Range<usize>, String)>,
    pub(crate) use_rag: bool,
    /// `@` 引用的文章。
    pub(crate) refs: Vec<Reference>,
    /// 粘贴材料的全文快照，随会话保存，旧会话默认没有材料。
    #[serde(default)]
    pub(crate) materials: Vec<paste::Material>,
    /// 动笔前澄清的回答（重跑时沿用，不再问一遍）。
    pub(crate) notes: Vec<String>,
    /// 定文种题定下的文种（重跑时沿用，不再问；当前文种与它不符时作废重问）。
    #[serde(default)]
    pub(crate) premise: Option<crate::models::TemplateKind>,
    /// 这一轮改的是待确认的提案（「改提案」）。
    #[serde(default)]
    pub(crate) on_proposal: bool,
    /// 写法风格的选择。
    #[serde(default)]
    pub(crate) style: crate::ai_services::style::StyleChoice,
}

/// 结果卡上的摘要，在提案到达时算一次，不必每帧重算。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProposalSummary {
    /// 提案正文字数。
    pub(crate) chars: usize,
    /// 提案前正文是否为空：空稿显示「采用为正文」，有稿显示「接受」。
    pub(crate) was_empty: bool,
    pub(crate) fact_changes: usize,
    pub(crate) warnings: usize,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum TurnState {
    /// 已发出，还没收到第一个字。
    Waiting,
    /// 正在接收模型输出。
    Streaming,
    /// 模型说完了，程序在跑清洗、规整与闸门。
    Checking,
    /// 流程停下来问用户（动笔前澄清、选基准稿……），答完接着跑。
    Asking,
    /// 待决事项：正文里还没定的「【待核实】」列成题，什么时候答都行；答完按回答修订出提案
    /// （`docs/decision-modules.md` 第八节）。不挂着流程，题目对着列出时的正文。
    Pending,
    /// 提案已就绪，等用户采用或放弃。
    Proposed(ProposalSummary),
    /// 审核类技能交了问题清单；`fixes` 条有改法的已放进审校抽屉。
    Reported {
        fixes: usize,
    },
    /// 成品已经写进正文（旧提案接受与 Pi 直接写入都用它）；`pi_write` 有值时
    /// 卡片上还能整次撤销。
    Accepted,
    /// Pi 交了成品，但运行期间正文被改过 / 稿件已只读 / 落盘失败：
    /// 正文不动，成品留在这一轮里等用户显式取回或按当前正文重做。
    Conflict,
    Discarded,
    /// 被同一篇稿件的新任务取代。
    Superseded,
    Stopped,
    Failed(String),
    /// 跑到一半程序关了（读回会话时）。
    Interrupted,
    /// 提案交出之后正文改过了（读回会话时），不能再接受。
    Expired,
}

impl TurnState {
    pub(crate) fn running(&self) -> bool {
        matches!(self, Self::Waiting | Self::Streaming | Self::Checking)
    }
}

/// 任务流里的一轮。
#[derive(Debug)]
pub(crate) struct AiTurn {
    /// 用量回投所属的后台任务；停止后仍可记录，不能串到新任务或另一会话。
    pub(crate) usage_job_seq: Option<u64>,
    pub(crate) provenance: String,
    pub(crate) usage: crate::ai_services::backend::UsageTotals,
    pub(crate) id: u64,
    /// 卡片抬头：「润色 · 精简篇幅」。
    pub(crate) title: String,
    /// 用户原话；旧工作台发起的任务没有原话，显示任务名。
    pub(crate) prompt: String,
    /// 上下文 chip：「选区 128 字」「知识库」。
    pub(crate) context: Vec<String>,
    /// 侧栏发起的才有，旧入口发起的不能「重新生成」。
    pub(crate) request: Option<TurnRequest>,
    pub(crate) state: TurnState,
    /// 模型正文（流式累加，未经清洗）。
    pub(crate) content: String,
    /// 接受提案时用户排除了几处变更块；0 表示整篇接受（或提案尚未被接受）。
    /// 随会话存进稿件库，追问时模型知道哪些改动没有落进正文。
    pub(crate) excluded_hunks: usize,
    /// 明确产出稿件后，全文只在左侧 AI 工作稿展示。
    pub(crate) is_workspace: bool,
    /// 流式补写时留在插入位置之后的原文，不进入模型增量。
    pub(crate) stream_suffix: String,
    /// 思考过程。
    pub(crate) reasoning: String,
    /// 阶段提示：「正在检索知识库…」「正在校验…」。会被下一阶段冲掉。
    pub(crate) phase: String,
    /// 要一直留在卡片上的说明：知识库命中了哪几篇、为什么没命中。
    pub(crate) notes: Vec<String>,
    /// 研究式起草的过程：每次工具调用一行。
    pub(crate) steps: Vec<String>,
    /// 要用户回答的选择题（动笔前，或第一稿之后）。
    pub(crate) questions: Vec<Question>,
    /// 每道题当前的作答状态，与 `questions` 一一对应。
    pub(crate) replies: Vec<ReplyDraft>,
    /// 这批题的形态（与 `questions` 一起设置）：清单确认在 AI 重列期间 `run` 交给了后台，
    /// 卡片仍要按它画。
    pub(crate) decision: crate::agent::decision::Decision,
    /// 研究式起草的结果：台账与证据，按回答修订时要用。
    pub(crate) research: Option<ResearchSnapshot>,
    /// 挂起的流程：答完题从这里接着跑。
    pub(crate) run: Option<Box<SkillRun>>,
    /// 这一轮能不能「接着跑」（内核加固第 4 期）：`Some(停在哪一步 · 什么时候存的)`。
    /// 读回会话与每次落盘后由 `session::refresh_resumable` 按库里最新检查点填；
    /// `None` 表示没有检查点，卡片保持现在的样子，不假装能续。
    pub(crate) resumable: Option<String>,
    /// 审核类技能的问题清单。
    pub(crate) findings: Vec<crate::agent::board::Finding>,
    /// 要素建议卡（`docs/element-fill-design.md` 3.3）：动笔前要素抽取的建议，
    /// 用户点采纳才写表单（红线 2）。随会话存盘。
    pub(crate) field_cards: Vec<field_card::FieldCardRow>,
    /// 风格学习学出、还没保存的档案。
    pub(crate) style: Option<crate::ai_services::style::StyleProfile>,
    /// Pi 这一轮的运行引用：中断 / 停止 / 失败后据此「接着写」或「从头写」。
    /// 已接受或旧流程轮次为 None；完整记录留在工作区的 `pi/run.json`。
    pub(crate) pi_run: Option<crate::pi_agent::PiRunRef>,
    /// Pi 成品直接写入正文的撤销记录（前后文本、运行标识、提示）。
    pub(crate) pi_write: Option<pi_write::PiWrite>,
    /// 交付检查通过、但没能写进正文的成品（冲突 / 保存失败 / 只读）。
    pub(crate) pi_pending: Option<pi_write::PiPendingDelivery>,
    /// Pi 写入前后的可选差异视图状态（不随会话存盘，读回重新开）。
    pub(crate) pi_diff: pi_write::PiDiffView,
    pub(crate) started: Instant,
    /// 结束时定格的耗时；运行中为 None，按 `started` 现算。
    pub(crate) elapsed: Option<Duration>,
}

impl AiTurn {
    pub(crate) fn resume(&mut self) {
        self.started = Instant::now()
            .checked_sub(self.elapsed())
            .unwrap_or_else(Instant::now);
        self.elapsed = None;
        self.state = TurnState::Waiting;
    }
    pub(crate) fn elapsed(&self) -> Duration {
        self.elapsed.unwrap_or_else(|| self.started.elapsed())
    }

    fn settle(&mut self, state: TurnState) {
        if self.elapsed.is_none() {
            self.elapsed = Some(self.started.elapsed());
        }
        self.phase.clear();
        self.state = state;
    }
}

/// 模型输出撞上长度上限时附在审校提示里的文字；结果卡据此亮「输出被截断」。
pub(crate) const TRUNCATED_NOTE: &str = "模型输出达到长度上限被截断，正文可能不完整。";

/// 一个会话最多保留的轮数，多了从最早的删起（库里一并删）。
const MAX_TURNS: usize = 100;

/// 一篇稿件的 AI 侧栏。
#[derive(Debug, Default)]
pub(crate) struct AiPanel {
    pub(crate) open: bool,
    pub(crate) composer: Composer,
    pub(crate) turns: Vec<AiTurn>,
    /// 正在跑的那一轮的停止开关。
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    /// 技能列表的缓存，技能标签与 `/` 弹出列表每帧要用；打开侧栏和每次发送时重读。
    pub(crate) skills: Vec<Skill>,
    /// 风格档案的缓存（底栏「风格」标签用），与技能列表一起重读。
    pub(crate) styles: Vec<crate::ai_services::style::StyleProfile>,
    next_id: u64,
    /// 当前会话：随稿件存进稿件库（16.15 B）。
    pub(crate) session: session::Session,
    /// Pi 正在等用户回答的题；只活在本轮，不写会话 JSON、不跨重启恢复。
    pub(crate) pi_ask: Option<pi_ask::PiAsk>,
    /// Pi 审核 / 找资料交回的侧栏清单。
    pub(crate) pi_report: Option<pi_ask::PiReportView>,
    /// 未采纳 Pi 提案的原始依据；与正式基线和候选稿绑定，不以候选稿自身作证。
    pub(crate) pi_sources: Option<pi_job::PiProposalSources>,
    /// 提问卡上点了「停止本轮」：交给应用层回收任务。
    pub(crate) pi_cancel: bool,
    /// 本轮 Pi 的回答通道；题卡靠它把答案送回后台进程。
    pub(crate) pi_bridge: Option<Arc<crate::pi_agent::PiBridge>>,
    /// Pi 标准技能目录的缓存。每帧只查一次 revision，不重复读盘；管理页保存 /
    /// 启停 / 导入后 revision 变化，下一次画侧栏或发送时重读。
    pub(crate) pi_skills: Option<crate::pi_agent::pi_skills::Catalog>,
    pub(crate) pi_skills_revision: Option<u64>,
    /// 当前在左侧查看的工作稿轮次；None 表示查看正式正文。
    pub(crate) workspace_view: Option<u64>,
    pub(crate) workspace_compare: bool,
}

impl AiPanel {
    /// 当前要显示的 AI 工作稿轮次：优先用户选中的那一轮，否则最近一轮工作稿。
    /// 只是「有没有工作稿」的判断，不改视图状态；切换视图仍由界面点击决定。
    pub(crate) fn workspace_turn_id(&self) -> Option<u64> {
        self.workspace_view
            .filter(|id| {
                self.turns
                    .iter()
                    .any(|turn| turn.id == *id && turn.is_workspace)
            })
            .or_else(|| {
                self.turns
                    .iter()
                    .rev()
                    .find(|turn| turn.is_workspace)
                    .map(|turn| turn.id)
            })
    }

    /// 按需刷新 Pi 技能目录。只比较一个原子计数，不每帧遍历技能包。
    pub(crate) fn ensure_pi_skills(&mut self) -> &crate::pi_agent::pi_skills::Catalog {
        let revision = crate::pi_agent::pi_skills::revision();
        if self.pi_skills.is_none() || self.pi_skills_revision != Some(revision) {
            self.pi_skills = Some(crate::pi_agent::pi_skills::load_catalog());
            self.pi_skills_revision = Some(revision);
        }
        self.pi_skills.as_ref().expect("刚放进目录，必然存在")
    }

    /// 用量是只读记录，允许停止后的原任务补回；任务产物仍走原有的过期检查。
    pub(crate) fn record_usage(
        &mut self,
        id: u64,
        seq: u64,
        usage: crate::ai_services::backend::UsageTotals,
    ) -> bool {
        let Some(turn) = self
            .turn_mut(id)
            .filter(|turn| turn.usage_job_seq == Some(seq))
        else {
            return false;
        };
        turn.usage.append(usage);
        turn.usage_job_seq = None;
        true
    }

    /// 新开一轮。之前还挂着的提案随之作废——新任务会把 `ai_proposal` 清掉。
    pub(crate) fn push_turn(
        &mut self,
        title: String,
        prompt: String,
        context: Vec<String>,
        request: Option<TurnRequest>,
    ) -> u64 {
        for turn in &mut self.turns {
            if matches!(turn.state, TurnState::Proposed(_)) {
                turn.state = TurnState::Superseded;
            }
        }
        self.next_id += 1;
        self.turns.push(AiTurn {
            usage_job_seq: None,
            provenance: String::new(),
            usage: Default::default(),
            id: self.next_id,
            title,
            prompt,
            context,
            request,
            state: TurnState::Waiting,
            content: String::new(),
            is_workspace: false,
            stream_suffix: String::new(),
            excluded_hunks: 0,
            reasoning: String::new(),
            phase: String::new(),
            notes: Vec::new(),
            steps: Vec::new(),
            questions: Vec::new(),
            replies: Vec::new(),
            decision: Default::default(),
            research: None,
            run: None,
            resumable: None,
            findings: Vec::new(),
            field_cards: Vec::new(),
            style: None,
            pi_run: None,
            pi_write: None,
            pi_pending: None,
            pi_diff: Default::default(),
            started: Instant::now(),
            elapsed: None,
        });
        if self.turns.len() > MAX_TURNS {
            let excess = self.turns.len() - MAX_TURNS;
            self.turns.drain(..excess);
        }
        self.next_id
    }

    /// 同一张卡片按新前提重来：定文种题切了文种，技能、要素清单、其余的题都要按新文种重新
    /// 选、重新出。卡片上已有的过程行与说明留着（看得出为什么重来），题目、挂起的流程与
    /// 产物清掉，计时接着走。找不到这一轮返回 false。
    pub(crate) fn restart_turn(
        &mut self,
        id: u64,
        title: String,
        context: Vec<String>,
        request: TurnRequest,
    ) -> bool {
        let Some(turn) = self.turn_mut(id) else {
            return false;
        };
        turn.title = title;
        turn.context = context;
        turn.request = Some(request);
        turn.questions.clear();
        turn.replies.clear();
        turn.run = None;
        turn.research = None;
        turn.resumable = None;
        turn.findings.clear();
        turn.field_cards.clear();
        turn.style = None;
        turn.content.clear();
        turn.stream_suffix.clear();
        turn.is_workspace = false;
        turn.resume();
        true
    }

    /// 正在跑的那一轮（最多一轮：同一篇稿件同时只有一个后台任务）。
    pub(crate) fn running_turn_mut(&mut self) -> Option<&mut AiTurn> {
        self.turns
            .iter_mut()
            .rev()
            .find(|turn| turn.state.running())
    }

    pub(crate) fn running(&self) -> bool {
        self.turns.iter().any(|turn| turn.state.running())
    }

    /// 现在不宜列待决事项：有任务在跑，或已经有题挂着等回答（挂起的流程、开着的待决清单）。
    /// 有没处理的提案由调用方另查（提案不在侧栏里）。
    pub(crate) fn pending_blocked(&self) -> bool {
        self.turns.iter().any(|turn| {
            turn.state.running() || matches!(turn.state, TurnState::Asking | TurnState::Pending)
        })
    }

    /// 把正文里还没定的「【待核实】」列成一张待决事项卡片，一批最多问
    /// [`skill_job::BATCH_QUESTIONS`] 处，其余的答完这批再列。正文里没有占位返回 None。
    pub(crate) fn open_pending(
        &mut self,
        document: &str,
        vocabulary: &[crate::models::VocabularyEntry],
    ) -> Option<u64> {
        let ledger = Ledger::from_placeholders(document);
        let questions =
            crate::agent::clarify::gap_questions(&ledger, skill_job::BATCH_QUESTIONS, vocabulary);
        if questions.is_empty() {
            return None;
        }
        let total = ledger.needs_user().len();
        let prompt = if total > questions.len() {
            format!("正文里还有 {total} 处待核实，先填这 {} 处", questions.len())
        } else {
            format!("正文里还有 {total} 处待核实")
        };
        let id = self.push_turn("待决事项".into(), prompt, Vec::new(), None);
        let turn = self.turn_mut(id)?;
        turn.replies = initial_replies(&questions);
        turn.questions = questions;
        turn.research = Some(ResearchSnapshot {
            raw: document.to_string(),
            ledger,
            sources: Vec::new(),
        });
        turn.settle(TurnState::Pending);
        Some(id)
    }

    /// 收到一批增量。`done` 表示模型已经说完，接下来是程序校验。
    pub(crate) fn append(&mut self, content: &str, reasoning: &str, done: bool) {
        let Some(turn) = self.running_turn_mut() else {
            return;
        };
        let insert_at = turn.content.len() - turn.stream_suffix.len();
        turn.content.insert_str(insert_at, content);
        turn.reasoning.push_str(reasoning);
        if done {
            turn.state = TurnState::Checking;
            turn.phase = "正在校验…".into();
        } else if !content.is_empty() || !reasoning.is_empty() {
            turn.state = TurnState::Streaming;
            // 只有正文到了才冲掉阶段提示：研究式起草在预研、检索时也有思考增量，
            // 那时「预研：列出要查的问题…」这类提示还得留着。
            if !content.is_empty() {
                turn.phase.clear();
            }
        }
    }

    pub(crate) fn set_phase(&mut self, phase: &str) {
        if let Some(turn) = self.running_turn_mut() {
            turn.phase = phase.to_string();
        }
    }

    /// Pi 主机报来进度：这一轮进入「写作中」，阶段提示显示在卡片上。
    pub(crate) fn pi_progress(&mut self, phase: &str) {
        if let Some(turn) = self.running_turn_mut() {
            turn.state = TurnState::Streaming;
            turn.phase = phase.to_string();
        }
    }

    pub(crate) fn step(&mut self, line: String) {
        if let Some(turn) = self.running_turn_mut() {
            turn.steps.push(line);
        }
    }

    /// 工作稿整体换新（补全之后），卡片上显示最新的一版。
    pub(crate) fn replace_content(&mut self, content: String) {
        let mut open = None;
        if let Some(turn) = self.running_turn_mut() {
            if !turn.is_workspace {
                open = Some(turn.id);
            }
            turn.is_workspace = true;
            turn.stream_suffix.clear();
            turn.content = content;
        }
        if let Some(id) = open {
            self.workspace_view = Some(id);
            self.workspace_compare = false;
        }
    }

    /// 每次写稿先重置流式插入位置；同一轮只自动打开一次，尊重用户切回正文。
    pub(crate) fn begin_write(&mut self, prefix: String, suffix: String) {
        let mut open = None;
        if let Some(turn) = self.running_turn_mut() {
            if !turn.is_workspace {
                open = Some(turn.id);
            }
            turn.is_workspace = true;
            turn.content = prefix + &suffix;
            turn.stream_suffix = suffix;
        }
        if let Some(id) = open {
            self.workspace_view = Some(id);
            self.workspace_compare = false;
        }
    }

    /// 流程挂起要问用户：这一轮停下来等回答，挂起的流程存在卡片上。
    pub(crate) fn ask(&mut self, run: Box<SkillRun>) {
        self.cancel = None;
        if let Some(turn) = self.running_turn_mut() {
            let previous = turn.replies.first().cloned();
            turn.replies = initial_replies(&run.suspension.questions);
            if let Some(list) = run.suspension.decision.list().filter(|list| list.revisable)
                && let Some(previous) = previous
                && let Some(reply) = turn.replies.first_mut()
            {
                if previous
                    .revise_base
                    .as_ref()
                    .is_some_and(|base| base != &previous.custom)
                {
                    reply.revise_candidate = reply.custom.clone();
                    reply.custom = previous.custom;
                    turn.notes.push(format!(
                        "优化期间你又修改了{}，已保留手工内容；AI 优化结果可单独采用。",
                        list.label
                    ));
                }
                reply.revise_instruction = previous.revise_instruction;
            }
            // 勾选模式默认全勾（程序把每条都标成推荐）。
            if run.suspension.decision.list().is_some_and(|list| list.pick)
                && let (Some(reply), Some(question)) =
                    (turn.replies.first_mut(), run.suspension.questions.first())
            {
                reply.picked = (0..question.choices.len())
                    .filter(|index| question.choices[*index].recommended)
                    .collect();
            }
            turn.questions = run.suspension.questions.clone();
            turn.decision = run.suspension.decision.clone();
            turn.run = Some(run);
            turn.settle(TurnState::Asking);
        }
    }

    /// 重读技能列表（内置 + 配置目录），返回加载时的说明。
    pub(crate) fn reload_skills(&mut self) -> Vec<String> {
        let (skills, mut notes) = crate::agent::skill::load_all();
        self.skills = skills;
        match crate::ai_services::style::StyleBook::load() {
            Ok(book) => self.styles = book.styles,
            Err(error) => notes.push(format!("风格档案读不出来：{error:#}")),
        }
        notes
    }

    pub(crate) fn turn_mut(&mut self, id: u64) -> Option<&mut AiTurn> {
        self.turns.iter_mut().find(|turn| turn.id == id)
    }

    /// 动笔前「要素抽取」的建议到了（事件通路）：合进正在跑的那一轮的建议卡，
    /// 卡片立刻出现，起草照常往下跑。
    pub(crate) fn offer_field_suggestions(
        &mut self,
        list: Vec<crate::element_fields::FieldSuggestion>,
    ) {
        if let Some(turn) = self.running_turn_mut() {
            field_card::merge(&mut turn.field_cards, list);
        }
    }

    pub(crate) fn note(&mut self, note: String) {
        if let Some(turn) = self.running_turn_mut()
            && !turn.notes.contains(&note)
        {
            // 研究式起草每检索一次都可能报同一句「重复片段已合并」，留一条就够。
            turn.notes.push(note);
        }
    }

    pub(crate) fn finish(&mut self, state: TurnState) {
        self.cancel = None;
        if let Some(turn) = self.running_turn_mut() {
            turn.settle(state);
        }
    }

    /// 提案被接受或放弃（无论从结果卡还是旧审阅窗点的）。`excluded_hunks` 是
    /// 接受时用户排除的变更块数，留在卡片与会话里给追问的模型看。
    pub(crate) fn resolve_proposal(&mut self, accepted: bool, excluded_hunks: usize) {
        self.workspace_view = None;
        if let Some(turn) = self
            .turns
            .iter_mut()
            .rev()
            .find(|turn| matches!(turn.state, TurnState::Proposed(_)))
        {
            turn.excluded_hunks = if accepted { excluded_hunks } else { 0 };
            turn.state = if accepted {
                TurnState::Accepted
            } else {
                TurnState::Discarded
            };
            // 接受过的 Pi 轮次不再提供「接着写」：正文已经采纳，重复恢复只会把同一份
            // 成品当新提案重装。运行记录本身留在工作区，不再参与恢复。
            if accepted && let Some(run) = turn.pi_run.as_mut() {
                run.status = crate::pi_agent::PiRunStatus::Accepted;
                run.updated_at = chrono::Local::now().to_rfc3339();
                // 运行记录也标成已接受：会话没来得及落盘就重启时，服务器端校验
                // 也不会再把这一轮当可恢复。
                if let Ok(root) = crate::pi_agent::resolve_workspace(&run.workspace)
                    && let Some(dir) = root.parent()
                    && let Ok(mut record) = crate::pi_agent::PiRunRecord::read_from(dir)
                {
                    record.status = crate::pi_agent::PiRunStatus::Accepted;
                    let _ = record.write_to_dir(dir);
                }
            }
        }
    }

    /// 用户丢弃保留的成品：卡片转「已放弃」，正文不变。
    pub(crate) fn discard_pi_pending(&mut self, turn_id: u64) -> bool {
        let Some(turn) = self.turn_mut(turn_id) else {
            return false;
        };
        turn.pi_pending = None;
        turn.notes.push("已丢弃保留的 AI 成品，正文未改变。".into());
        turn.settle(TurnState::Discarded);
        true
    }

    /// 清空任务流。跑着的那一轮保留，否则回来的增量没处落。
    pub(crate) fn clear_history(&mut self) {
        self.turns.retain(|turn| turn.state.running());
    }

    /// 这一轮 Pi 的成品是不是已经写过。同一份交付回执（恢复重放、重启后重试）
    /// 只能自动应用一次；撤销过的也不能被盖回来。
    pub(crate) fn pi_run_already_written(&self, run_id: &str) -> bool {
        self.turns.iter().any(|turn| {
            turn.pi_write
                .as_ref()
                .is_some_and(|w| w.matches_run(run_id))
        })
    }

    /// 成品已写进正文：卡片转「已写入」，并记住精确的前后文本供撤销。
    pub(crate) fn mark_pi_written(&mut self, turn_id: u64, write: pi_write::PiWrite) -> bool {
        let Some(turn) = self.turn_mut(turn_id) else {
            return false;
        };
        for advisory in &write.advisories {
            let line = format!("提示：{advisory}");
            if !turn.notes.contains(&line) {
                turn.notes.push(line);
            }
        }
        turn.content = write.after.clone();
        turn.is_workspace = true;
        turn.stream_suffix.clear();
        turn.pi_pending = None;
        turn.pi_write = Some(write);
        if let Some(run) = turn.pi_run.as_mut() {
            run.status = crate::pi_agent::PiRunStatus::Accepted;
        }
        turn.settle(TurnState::Accepted);
        true
    }

    /// 交付检查过了但没写进正文：正文不动，成品留在这一轮里等用户显式取回。
    pub(crate) fn park_pi_delivery(
        &mut self,
        turn_id: u64,
        pending: pi_write::PiPendingDelivery,
        conflict: bool,
        reason: String,
    ) -> bool {
        let Some(turn) = self.turn_mut(turn_id) else {
            return false;
        };
        turn.content = pending.markdown.clone();
        turn.is_workspace = true;
        turn.stream_suffix.clear();
        turn.pi_pending = Some(pending);
        let notes = &mut turn.notes;
        for advisory in &turn.pi_pending.as_ref().expect("刚放进去").advisories {
            let line = format!("提示：{advisory}");
            if !notes.contains(&line) {
                notes.push(line);
            }
        }
        if conflict {
            turn.settle(TurnState::Conflict);
        } else {
            turn.notes.push(reason.clone());
            turn.settle(TurnState::Failed(reason));
        }
        true
    }
}

/// 一道选择题的作答状态。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReplyDraft {
    pub(crate) choice: Option<usize>,
    pub(crate) custom: String,
    pub(crate) skip: bool,
    /// 清单确认期间的修改要求；与手工编辑的清单一起保存。旧会话里叫 `outline_*`（只有大纲能改）。
    #[serde(default, alias = "outline_instruction")]
    pub(crate) revise_instruction: String,
    /// 发起优化时的文本快照，用于保护等待期间新增的手工修改。
    #[serde(default, alias = "outline_base")]
    pub(crate) revise_base: Option<String>,
    /// 等待期间发生手工修改时，AI 返回的清单先存成候选，不覆盖当前文本。
    #[serde(default, alias = "outline_candidate")]
    pub(crate) revise_candidate: String,
    /// 勾选模式的清单确认：勾中了哪几条（选项下标）。
    #[serde(default)]
    pub(crate) picked: Vec<usize>,
}

impl ReplyDraft {
    /// 写了自己的答案就以它为准；否则看选了哪个选项；都没有就是跳过。
    pub(crate) fn reply(&self) -> Reply {
        if self.skip {
            Reply::Skip
        } else if !self.custom.trim().is_empty() {
            Reply::Custom(self.custom.trim().to_string())
        } else if let Some(index) = self.choice {
            Reply::Choice(index)
        } else {
            Reply::Skip
        }
    }
}

/// 研究式起草留下的东西，按回答修订时要用。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ResearchSnapshot {
    /// 定稿前的工作稿（引用标记已剥）。按回答修订在它上面改，再走一遍定稿。
    pub(crate) raw: String,
    pub(crate) ledger: Ledger,
    /// 证据：(编号, 出处)。核实清单里显示来源用。
    pub(crate) sources: Vec<(usize, String)>,
}

/// 在当前正文里找回锁定的选区。
///
/// 原位还是那段文字就用原位；否则全篇找，只有一处才认，多处或找不到都放弃——
/// 猜错的代价是把别处改坏（同 `revision::Anchor` 的取舍）。
pub(crate) fn locate_selection(
    markdown: &str,
    selection: &(Range<usize>, String),
) -> Option<String> {
    let (range, text) = selection;
    if text.trim().is_empty() {
        return None;
    }
    if markdown.get(range.clone()) == Some(text.as_str()) {
        return Some(text.clone());
    }
    let mut hits = markdown.match_indices(text.as_str());
    let first = hits.next()?;
    hits.next().is_none().then(|| first.1.to_string())
}

/// 拼润色指令：用户的话 + 选区硬约束。
pub(crate) fn polish_instruction(custom: &str, selected: Option<&str>) -> String {
    let mut instruction = custom.trim().to_string();
    if instruction.is_empty() {
        instruction = "只做公文格式规整，不改写事实和措辞。".into();
    }
    if let Some(selected) = selected {
        instruction.push_str(&format!(
            "\n\n【唯一允许修改的原文片段】\n{selected}\n【范围硬约束】只修改上述片段，片段之外的文字和 Markdown 标记必须逐字保持不变；仍须返回完整 Markdown。"
        ));
    }
    instruction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_roles_are_per_item_and_old_sessions_fall_back_to_the_default() {
        use crate::ai_services::references::RefSource;
        use crate::pi_agent::MaterialRole;
        let mut composer = Composer::default();
        assert_eq!(
            composer.material_role(0),
            MaterialRole::Fact,
            "旧会话默认事实材料"
        );
        composer.pi_material_roles = vec![MaterialRole::Fact, MaterialRole::StyleSample];
        assert_eq!(composer.material_role(0), MaterialRole::Fact);
        assert_eq!(composer.material_role(1), MaterialRole::StyleSample);
        assert_eq!(
            composer.material_role(9),
            composer.pi_material_role,
            "没单独设过的回退默认值"
        );

        // 两篇标题一样、来源不同：按「来源 + 编号」记，不能串用。
        composer.pi_ref_roles = vec![
            (("稿件库".into(), 7), MaterialRole::StyleSample),
            (("知识库".into(), 7), MaterialRole::Fact),
        ];
        assert_eq!(
            composer.ref_role(RefSource::Manuscript, 7),
            MaterialRole::StyleSample
        );
        assert_eq!(
            composer.ref_role(RefSource::Knowledge, 7),
            MaterialRole::Fact
        );
        assert_eq!(
            composer.ref_role(RefSource::Knowledge, 8),
            composer.pi_ref_role
        );
    }

    #[test]
    fn strategy_defaults_to_auto_and_keeps_the_old_default_role() {
        let composer = Composer::default();
        assert_eq!(
            composer.pi_strategy,
            crate::pi_agent::PiStrategyPreference::Auto
        );
        assert_eq!(
            composer.pi_material_role,
            crate::pi_agent::MaterialRole::Fact
        );
        assert_eq!(composer.pi_ref_role, crate::pi_agent::MaterialRole::Fact);
    }

    fn panel_with_turn() -> AiPanel {
        let mut panel = AiPanel::default();
        panel.push_turn("润色".into(), "压一压".into(), vec![], None);
        panel
    }

    #[test]
    fn workspace_turn_falls_back_to_the_latest_workspace_copy() {
        let mut panel = panel_with_turn();
        assert_eq!(
            panel.workspace_turn_id(),
            None,
            "普通轮次不算工作稿，正文视图不该出现纸签"
        );
        panel.push_turn("起草".into(), "写一版".into(), vec![], None);
        panel.turns[1].is_workspace = true;
        assert_eq!(
            panel.workspace_turn_id(),
            Some(2),
            "没选中时跟最近一轮工作稿"
        );
        panel.workspace_view = Some(1);
        assert_eq!(
            panel.workspace_turn_id(),
            Some(2),
            "选中的轮次已经不作工作稿时回落到最近一轮"
        );
        panel.workspace_view = Some(2);
        assert_eq!(panel.workspace_turn_id(), Some(2));
        panel.workspace_view = Some(99);
        panel.turns[1].is_workspace = false;
        assert_eq!(
            panel.workspace_turn_id(),
            None,
            "没有工作稿时清掉失效的选中项"
        );
    }

    #[test]
    fn workspace_streams_replace_append_and_fill_without_stealing_the_view() {
        let mut panel = panel_with_turn();
        panel.append("", "先检索", false);
        assert_eq!(panel.workspace_view, None, "思考不打开工作稿");
        panel.begin_write(String::new(), String::new());
        assert_eq!(panel.workspace_view, Some(1));
        panel.append("# 初稿", "", false);
        panel.replace_content("# 初稿\n".into());
        panel.workspace_view = None;
        panel.begin_write("# 初稿\n\n".into(), String::new());
        panel.append("第二节", "", false);
        assert_eq!(panel.turns[0].content, "# 初稿\n\n第二节");
        assert_eq!(panel.workspace_view, None, "用户切回正文后不抢视图");
        panel.begin_write("摘要：".into(), "\n\n# 初稿\n\n第二节".into());
        panel.append("综述", "", false);
        panel.append("。", "", false);
        assert_eq!(panel.turns[0].content, "摘要：综述。\n\n# 初稿\n\n第二节");
        panel.replace_content("清洗后的稿件".into());
        assert!(panel.turns[0].stream_suffix.is_empty());
        panel.begin_write(String::new(), String::new());
        panel.append("改写稿", "", false);
        panel.finish(TurnState::Stopped);
        assert_eq!(panel.turns[0].content, "改写稿", "停止保留已生成内容");
        panel.append("迟到内容", "", false);
        assert_eq!(panel.turns[0].content, "改写稿");
    }

    #[test]
    fn reply_drafts_prefer_custom_text_then_choice_then_skip() {
        let mut draft = ReplyDraft::default();
        assert_eq!(draft.reply(), Reply::Skip);
        draft.choice = Some(1);
        assert_eq!(draft.reply(), Reply::Choice(1));
        draft.custom = " 12月1日 ".into();
        assert_eq!(draft.reply(), Reply::Custom("12月1日".into()));
        draft.skip = true;
        assert_eq!(draft.reply(), Reply::Skip);
    }

    #[test]
    fn asking_parks_the_turn_until_answered() {
        let mut panel = panel_with_turn();
        panel.cancel = Some(Arc::new(AtomicBool::new(false)));
        panel.step("⌕ 检索".into());
        panel.ask(Box::new(SkillRun {
            skill: crate::agent::skill::builtin(crate::agent::skill::POLISH).unwrap(),
            suspension: crate::agent::engine::Suspension {
                checkpoint: crate::agent::checkpoint::Checkpoint {
                    at: vec![1],
                    reason: crate::agent::checkpoint::Reason::Ask,
                    label: String::new(),
                    board: crate::agent::board::Board::default(),
                    partial: false,
                },
                questions: Vec::new(),
                save_as: None,
                decision: Default::default(),
            },
            use_rag: false,
        }));
        assert_eq!(panel.turns[0].state, TurnState::Asking);
        assert!(panel.turns[0].run.is_some(), "挂起的流程存在卡片上");
        assert!(!panel.running(), "问题等着回答时不算在跑");
        assert!(panel.cancel.is_none());
        assert_eq!(panel.turns[0].steps, ["⌕ 检索"]);
    }

    #[test]
    fn deltas_flow_into_the_running_turn() {
        let mut panel = panel_with_turn();
        panel.append("", "想", false);
        assert_eq!(panel.turns[0].state, TurnState::Streaming);
        panel.append("一、", "", false);
        panel.append("总体要求", "", true);
        let turn = &panel.turns[0];
        assert_eq!(turn.content, "一、总体要求");
        assert_eq!(turn.reasoning, "想");
        assert_eq!(turn.state, TurnState::Checking);
        assert!(panel.running());
    }

    #[test]
    fn notes_stay_after_the_phase_moves_on() {
        let mut panel = panel_with_turn();
        panel.set_phase("正在检索知识库…");
        panel.note("知识库：已注入 3 段知识库参考：《甲》".into());
        panel.append("正文", "", false);
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        let turn = &panel.turns[0];
        assert!(turn.phase.is_empty());
        assert_eq!(turn.notes, ["知识库：已注入 3 段知识库参考：《甲》"]);
        // 同一句不重复留。
        panel.note("知识库：已注入 3 段知识库参考：《甲》".into());
        assert_eq!(panel.turns[0].notes.len(), 1);
        // 结束之后的说明没有归属，丢掉。
        panel.note("迟到".into());
        assert_eq!(panel.turns[0].notes.len(), 1);
    }

    #[test]
    fn empty_batches_do_not_flip_waiting_to_streaming() {
        let mut panel = panel_with_turn();
        panel.append("", "", false);
        assert_eq!(panel.turns[0].state, TurnState::Waiting);
    }

    #[test]
    fn finishing_settles_elapsed_and_clears_phase() {
        let mut panel = panel_with_turn();
        panel.set_phase("正在检索知识库…");
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        let turn = &panel.turns[0];
        assert!(turn.elapsed.is_some());
        assert!(turn.phase.is_empty());
        assert!(!panel.running());
        // 没有在跑的轮次时，迟到的增量直接丢掉。
        panel.append("迟到", "", false);
        assert!(panel.turns[0].content.is_empty());
    }

    #[test]
    fn stopped_usage_returns_to_its_original_turn_without_touching_new_tasks() {
        let mut panel = AiPanel::default();
        let first = panel.push_turn("起草".into(), "写稿".into(), vec![], None);
        panel.turns[0].usage_job_seq = Some(4);
        panel.finish(TurnState::Stopped);
        panel.push_turn("起草".into(), "另写".into(), vec![], None);
        panel.turns[1].usage_job_seq = Some(5);
        let mut usage = crate::ai_services::backend::UsageTotals::default();
        usage.record(
            crate::ai_services::backend::ModelRole::Draft,
            "m",
            10,
            "半稿",
            None,
            1,
        );
        assert!(panel.record_usage(first, 4, usage.clone()));
        assert_eq!(panel.turns[0].usage.calls.len(), 1);
        assert_eq!(panel.turns[0].state, TurnState::Stopped);
        assert!(panel.turns[1].usage.calls.is_empty());
        assert!(
            !panel.record_usage(first, 4, usage.clone()),
            "重复回投不重复计数"
        );
        panel.turns[1].id = first;
        panel.turns.remove(0);
        assert!(
            !panel.record_usage(first, 4, usage),
            "编号相同的另一会话不能收旧任务记录"
        );
    }

    #[test]
    fn a_new_turn_supersedes_a_pending_proposal() {
        let mut panel = panel_with_turn();
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        panel.push_turn("起草".into(), "写个通知".into(), vec![], None);
        assert_eq!(panel.turns[0].state, TurnState::Superseded);
        // 已作废的提案不会被后来的接受误标。
        panel.finish(TurnState::Proposed(ProposalSummary::default()));
        panel.resolve_proposal(true, 0);
        assert_eq!(panel.turns[0].state, TurnState::Superseded);
        assert_eq!(panel.turns[1].state, TurnState::Accepted);
    }

    #[test]
    fn history_is_capped_and_clearing_keeps_the_running_turn() {
        let mut panel = AiPanel::default();
        for index in 0..(MAX_TURNS + 5) {
            panel.push_turn(format!("第{index}轮"), String::new(), vec![], None);
            if index + 1 < MAX_TURNS + 5 {
                panel.finish(TurnState::Stopped);
            }
        }
        assert_eq!(panel.turns.len(), MAX_TURNS);
        assert_eq!(panel.turns[0].title, "第5轮");
        panel.clear_history();
        assert_eq!(panel.turns.len(), 1);
        assert!(panel.turns[0].state.running());
    }

    #[test]
    fn selection_is_found_in_place_or_by_unique_text() {
        let text = "一、甲\n二、乙\n三、丙";
        let start = text.find("二、乙").unwrap();
        let selection = (start..start + "二、乙".len(), "二、乙".to_string());
        assert_eq!(
            locate_selection(text, &selection).as_deref(),
            Some("二、乙")
        );
        // 前面插了字，原位对不上，但全篇只有一处。
        let edited = format!("前言\n{text}");
        assert_eq!(
            locate_selection(&edited, &selection).as_deref(),
            Some("二、乙")
        );
        // 出现两处：无法判定，放弃。
        let twice = format!("{text}\n二、乙");
        assert_eq!(locate_selection(&twice, &(0..3, "二、乙".into())), None);
        // 已被删掉。
        assert_eq!(locate_selection("一、甲", &selection), None);
    }

    #[test]
    fn polish_instruction_falls_back_and_pins_the_selection() {
        assert_eq!(
            polish_instruction("  ", None),
            "只做公文格式规整，不改写事实和措辞。"
        );
        let pinned = polish_instruction("压缩", Some("二、乙"));
        assert!(pinned.starts_with("压缩"));
        assert!(pinned.contains("【唯一允许修改的原文片段】\n二、乙"));
    }
}
