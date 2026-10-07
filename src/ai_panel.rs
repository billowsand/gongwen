//! 起草页右侧的 AI 侧栏。
//!
//! 设计见 `docs/ai-agent-workbench.md` 第十六节。每一轮都按一个技能（SKILL.md）跑流程引擎
//! （`crate::agent::engine`）：技能默认自动选（上下文过滤 + 触发词，分不出高下时交模型判断），
//! 也可以在输入框里敲 `/` 从弹出列表里指定；敲 `@` 引用稿件库或知识库里的文章（16.13）。
//!
//! 模型输出逐段流进任务流；流程要问用户时挂起，答完接着跑。结果一律是提案，由用户点
//! 「采用 / 接受」才落入正文（红线 1）；审核类技能交问题清单。
//!
//! 本文件只放状态与纯逻辑；界面在 `ai_panel/ui.rs`（任务流）与 `ai_panel/composer_ui.rs`
//! （输入框），`/`、`@` 的识别与过滤在 `ai_panel/mention.rs`。

use crate::agent::board::Reference;
use crate::agent::clarify::{Question, Reply};
use crate::agent::gaps::Ledger;
use crate::agent::skill::Skill;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

mod composer_ui;
mod history;
mod mention;
mod provenance;
pub(crate) use provenance::source_line as proposal_source;
pub(crate) mod session;
mod session_ui;
mod skill_job;
mod ui;
mod workspace_ui;

pub(crate) use skill_job::{
    GapRevision, SkillResult, SkillRun, finish_research_revision, initial_replies,
};

/// 自动选技能、输入框还空着时的提示。
const AUTO_HINT: &str = "说说要做什么：起草、润色……输入 / 选技能，@ 引用文章";

/// 输入区的状态。随稿件保存，切换标签页互不影响。
#[derive(Debug, Default)]
pub(crate) struct Composer {
    /// 用户用 `/` 或技能标签指定的技能 id；None 表示自动选。
    pub(crate) skill: Option<String>,
    pub(crate) text: String,
    /// 打开侧栏时锁定的选区（字节区间）与当时的原文。正文被改动后按原文重新定位。
    pub(crate) selection: Option<(Range<usize>, String)>,
    /// 润色预设（`AppConfig::ai_prompts` 的 id）。None 表示不用预设。
    pub(crate) preset: Option<u32>,
    /// 技能用到知识库时检索。
    pub(crate) use_rag: bool,
    pub(crate) error: Option<String>,
    /// 首次打开时按设置给过默认值没有。之后以用户的勾选为准，不再覆盖。
    pub(crate) primed: bool,
    /// `@` 引用的文章；输入框里对应留着 `@《标题》` 记号，记号删了引用也就没了。
    pub(crate) refs: Vec<Reference>,
    /// `/`、`@` 弹出层的状态。
    pub(crate) popup: mention::PopupState,
    /// 有待确认的提案时，修改类技能改提案而不是改正文（16.15 B.7）。默认是；底栏的「改提案」
    /// 标签点掉就回到改正文。
    pub(crate) skip_proposal: bool,
    /// 写法风格：自动挑 / 指定一份 / 不用（16.15 C.3）。
    pub(crate) style: crate::agent::style::StyleChoice,
}

/// 一轮请求的原始参数，「重新生成」照它再发一次。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TurnRequest {
    /// 指定的技能 id；None 表示按上下文与触发词自动选。
    pub(crate) skill: Option<String>,
    pub(crate) text: String,
    pub(crate) selection: Option<(Range<usize>, String)>,
    pub(crate) preset: Option<u32>,
    pub(crate) use_rag: bool,
    /// `@` 引用的文章。
    pub(crate) refs: Vec<Reference>,
    /// 动笔前澄清的回答（重跑时沿用，不再问一遍）。
    pub(crate) notes: Vec<String>,
    /// 这一轮改的是待确认的提案（「改提案」）。
    #[serde(default)]
    pub(crate) on_proposal: bool,
    /// 写法风格的选择。
    #[serde(default)]
    pub(crate) style: crate::agent::style::StyleChoice,
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
    /// 提案已就绪，等用户采用或放弃。
    Proposed(ProposalSummary),
    /// 审核类技能交了问题清单；`fixes` 条有改法的已放进审校抽屉。
    Reported {
        fixes: usize,
    },
    Accepted,
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
    pub(crate) usage: crate::agent::backend::UsageTotals,
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
    /// 研究式起草的结果：台账与证据，按回答修订时要用。
    pub(crate) research: Option<ResearchSnapshot>,
    /// 挂起的流程：答完题从这里接着跑。
    pub(crate) run: Option<Box<SkillRun>>,
    /// 审核类技能的问题清单。
    pub(crate) findings: Vec<crate::agent::board::Finding>,
    /// 风格学习学出、还没保存的档案。
    pub(crate) style: Option<crate::agent::style::StyleProfile>,
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
    pub(crate) styles: Vec<crate::agent::style::StyleProfile>,
    next_id: u64,
    /// 当前会话：随稿件存进稿件库（16.15 B）。
    pub(crate) session: session::Session,
    /// 当前在左侧查看的工作稿轮次；None 表示查看正式正文。
    pub(crate) workspace_view: Option<u64>,
    pub(crate) workspace_compare: bool,
}

impl AiPanel {
    /// 用量是只读记录，允许停止后的原任务补回；任务产物仍走原有的过期检查。
    pub(crate) fn record_usage(
        &mut self,
        id: u64,
        seq: u64,
        usage: crate::agent::backend::UsageTotals,
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
            research: None,
            run: None,
            findings: Vec::new(),
            style: None,
            started: Instant::now(),
            elapsed: None,
        });
        if self.turns.len() > MAX_TURNS {
            let excess = self.turns.len() - MAX_TURNS;
            self.turns.drain(..excess);
        }
        self.next_id
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
            turn.replies = initial_replies(&run.suspension.questions);
            turn.questions = run.suspension.questions.clone();
            turn.run = Some(run);
            turn.settle(TurnState::Asking);
        }
    }

    /// 重读技能列表（内置 + 配置目录），返回加载时的说明。
    pub(crate) fn reload_skills(&mut self) -> Vec<String> {
        let (skills, mut notes) = crate::agent::skill::load_all();
        self.skills = skills;
        match crate::agent::style::StyleBook::load() {
            Ok(book) => self.styles = book.styles,
            Err(error) => notes.push(format!("风格档案读不出来：{error:#}")),
        }
        notes
    }

    pub(crate) fn turn_mut(&mut self, id: u64) -> Option<&mut AiTurn> {
        self.turns.iter_mut().find(|turn| turn.id == id)
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
        }
    }

    /// 清空任务流。跑着的那一轮保留，否则回来的增量没处落。
    pub(crate) fn clear_history(&mut self) {
        self.turns.retain(|turn| turn.state.running());
    }
}

/// 一道选择题的作答状态。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReplyDraft {
    pub(crate) choice: Option<usize>,
    pub(crate) custom: String,
    pub(crate) skip: bool,
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

/// 拼润色指令：预设 + 用户的话 + 选区硬约束。
pub(crate) fn polish_instruction(preset: &str, custom: &str, selected: Option<&str>) -> String {
    let mut instruction = [preset.trim(), custom.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
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

    fn panel_with_turn() -> AiPanel {
        let mut panel = AiPanel::default();
        panel.push_turn("润色".into(), "压一压".into(), vec![], None);
        panel
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
            board: crate::agent::board::Board::default(),
            suspension: crate::agent::engine::Suspension {
                questions: Vec::new(),
                resume_at: 1,
                save_as: None,
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
        let mut usage = crate::agent::backend::UsageTotals::default();
        usage.record(
            crate::agent::backend::ModelRole::Draft,
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
    fn polish_instruction_joins_parts_and_pins_the_selection() {
        assert_eq!(
            polish_instruction("", "  ", None),
            "只做公文格式规整，不改写事实和措辞。"
        );
        assert_eq!(
            polish_instruction("精简", "不改时限", None),
            "精简\n\n不改时限"
        );
        let pinned = polish_instruction("", "压缩", Some("二、乙"));
        assert!(pinned.starts_with("压缩"));
        assert!(pinned.contains("【唯一允许修改的原文片段】\n二、乙"));
    }
}
