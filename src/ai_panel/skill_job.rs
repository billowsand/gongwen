//! 技能任务：侧栏发起的每一轮都按一个技能跑流程引擎，挂在 `DraftPage` 上。
//!
//! - `start_skill`：选技能（显式 / 触发词 / 交模型判断）、备好黑板，后台线程跑
//!   `agent::engine`，事件转成 `DocJob` 回投；也用来接着跑挂起的那一轮；
//! - `answer_suspended`：挂起时的题答完——回答由引擎按确定性规则落到黑板，要切文种的由
//!   界面线程切（用户点选，不是模型改），然后从下一步接着跑；
//! - `apply_research_answers`：交付后附在提案上的题答完——要填、要删的交模型写进所在段落
//!   （`agent::gap_revise`，过闸门才用，不过就直接替换），重新定稿成提案。

use super::history::{self, HistoryPlan};
use super::{ReplyDraft, ResearchSnapshot, TurnRequest, TurnState, locate_selection};
use crate::agent::api::{ApiSecrets, ApiStore};
use crate::agent::backend::{LmBackend, ModelBackend};
use crate::agent::board::{Board, Finding};
use crate::agent::clarify::{self, Question, Reply, Target};
use crate::agent::engine::{self, Event, Outcome, SkillReport, Suspension};
use crate::agent::gaps::GapStatus;
use crate::agent::router::{self, Route, RouteContext};
use crate::agent::skill::{OutputKind, Skill, TextNeed};
use crate::agent::tools::{Env, Permission, RagSearch, SqliteManuscripts, ToolUse};
use crate::app::{DocJob, GongwenApp, WorkerResult};
use crate::draft_page::DraftPage;
use crate::models::GeneratedDraft;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// 挂起的一轮：技能、黑板与接着跑的位置。存在这一轮的卡片上，答完原样带回后台。
#[derive(Debug, Clone)]
pub(crate) struct SkillRun {
    pub(crate) skill: Skill,
    pub(crate) board: Board,
    pub(crate) suspension: Suspension,
    pub(crate) use_rag: bool,
}

/// 技能任务的结果，经 `DocJob::SkillDone` 回投。
pub(crate) enum SkillResult {
    /// 流程停下来问用户。
    Suspended(Box<SkillRun>),
    /// 定稿后的提案与过程。
    Proposal {
        skill: String,
        draft: GeneratedDraft,
        report: Box<SkillReport>,
    },
    /// 审核类技能的问题清单（不改稿）。
    Report {
        skill: String,
        /// 转修订建议时作检查器名，同一技能再跑一次只换掉自己上一轮的建议。
        skill_id: String,
        findings: Vec<Finding>,
        /// 风格学习学出的档案（待用户点「保存为风格」）。
        style: Option<Box<crate::agent::style::StyleProfile>>,
    },
}

/// 用哪个技能：选定了，或发送后交模型从候选里挑。
pub(super) enum Pick {
    Fixed(Box<Skill>),
    Ask(Vec<Skill>),
}

/// 交付后一批最多问几题（技能参数 `batch_questions` 的兜底值）。
const BATCH_QUESTIONS: usize = 4;

/// 流式增量攒批：50 ms 或 600 字节发一次，与第 ① 期的单次起草一致。
const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const FLUSH_BYTES: usize = 600;

/// 每道题的初始作答：程序有推荐项的先替用户选上，点「确认」就能走。
pub(crate) fn initial_replies(questions: &[Question]) -> Vec<ReplyDraft> {
    questions
        .iter()
        .map(|question| ReplyDraft {
            choice: question
                .choices
                .iter()
                .position(|choice| choice.recommended),
            custom: question.prefill.clone(),
            ..ReplyDraft::default()
        })
        .collect()
}

fn collect_replies(questions: &[Question], drafts: &[ReplyDraft]) -> Vec<(usize, Reply)> {
    questions
        .iter()
        .zip(drafts)
        .map(|(question, draft)| (question.id, draft.reply()))
        .collect()
}

/// 技能现在用不了的原因；能用返回 None。
pub(crate) fn unavailable(skill: &Skill, ctx: &RouteContext<'_>) -> Option<String> {
    if router::eligible(skill, ctx) {
        return None;
    }
    let why = if !skill.enabled {
        "已停用".to_string()
    } else if skill.when.text == TextNeed::Present && !ctx.has_text {
        "要在已有正文上用，正文还是空的：先起草或粘贴稿件".to_string()
    } else if skill.when.text == TextNeed::Empty && ctx.has_text {
        "只用于空稿".to_string()
    } else if !skill.applies_to.is_empty() && !skill.applies_to.contains(&ctx.kind) {
        format!("不适用于{}", ctx.kind.label())
    } else if ctx.has_selection {
        "不能在选区上用".to_string()
    } else {
        "要先在编辑器里选中一段".to_string()
    };
    Some(format!("「{}」{why}。", skill.name))
}

impl DraftPage<'_> {
    /// 发起一轮技能任务。`resume` 是挂起的那一轮：答完接着跑，不另开一轮。
    pub(crate) fn start_skill(
        &mut self,
        request: TurnRequest,
        resume: Option<(u64, Box<SkillRun>)>,
    ) -> Result<(), String> {
        if self.doc.read_only() {
            return Err("这篇稿件已发布或归档，只读。".into());
        }
        if self.doc.busy {
            return Err("这篇稿件还有任务在跑，稍等一下。".into());
        }
        let time = crate::prompt::TimeContext::now();
        let fresh = resume.is_none();
        let (pick, mut board, start, use_rag, title, plan) = match resume {
            Some((turn_id, run)) => {
                let SkillRun {
                    skill,
                    mut board,
                    suspension,
                    use_rag,
                } = *run;
                // 要素与正文以界面上现在的为准：用户可能刚切了文种。
                board.draft = self.doc.draft.clone();
                board.document = self.doc.generated_markdown.clone();
                let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) else {
                    return Err("这一轮已不在侧栏里。".into());
                };
                turn.questions.clear();
                turn.replies.clear();
                turn.resume();
                let title = turn.title.clone();
                (
                    Pick::Fixed(Box::new(skill)),
                    board,
                    suspension.resume_at,
                    use_rag,
                    title,
                    // 接着跑的沿用发起时的历史（已在黑板上）。
                    HistoryPlan::default(),
                )
            }
            None => self.prepare_skill(&request, &time)?,
        };
        board.system_prompt = crate::prompt::build_system_prompt(&time);
        // 接着跑的沿用发起时挑的风格（已在黑板上）。
        let style = if fresh {
            self.style_pick(&request, &board)
        } else {
            StylePick::None
        };
        self.doc.ai_panel.open = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.doc.ai_panel.cancel = Some(cancel.clone());
        self.doc.ai_prompt_last_label = title.clone();
        self.doc.ai_review_baseline = Some(self.doc.generated_markdown.clone());
        self.doc.ai_proposal = None;
        let (key, seq) = self.begin_job();
        let turn_id = self
            .doc
            .ai_panel
            .running_turn_mut()
            .map(|turn| {
                turn.usage_job_seq = Some(seq);
                turn.id
            })
            .expect("技能任务已有轮次");
        *self.status = format!("正在{title}…");

        let config = self.config.clone();
        // 接口定义与密钥在界面线程读：测试替换的配置目录只对当前线程有效。
        let (apis, secrets) = match (ApiStore::load(), ApiSecrets::load()) {
            (Ok(apis), Ok(secrets)) => (apis, secrets),
            (apis, secrets) => {
                for error in [apis.err(), secrets.err()].into_iter().flatten() {
                    self.doc
                        .ai_panel
                        .note(format!("数据接口配置读不出来：{error:#}"));
                }
                (ApiStore::default(), ApiSecrets::default())
            }
        };
        if let Pick::Fixed(skill) = &pick {
            let missing = crate::agent::skill::missing_apis(skill, &apis);
            if !missing.is_empty() {
                self.doc.ai_panel.note(format!(
                    "「{}」要用的数据接口还没配置或不能给 AI 用：{}，相关步骤会跳过。可在AI 管理页「数据接口」添加或调整。",
                    skill.name,
                    missing.join("、")
                ));
            }
        }
        // 不限文种，与知识库页的检索、问答一致。
        let kind_filter = None;
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let send = |job: DocJob| {
                let _ = tx.send(WorkerResult::Doc { key, seq, job });
            };
            let model = LmBackend::new(&config, cancel);
            if let Some(compaction) = &plan.compact {
                // 历史太长：先把较早的轮次压成会话摘要（16.15 A.5），摘要回投给界面存进会话。
                send(DocJob::ExportProgress("会话较长，先压缩较早的轮次…".into()));
                match compact_session(&model, compaction) {
                    Ok(summary) => {
                        board.history = history::render(&summary, &plan.recent);
                        send(DocJob::AiSessionSummary {
                            summary,
                            upto: compaction.upto,
                        });
                        send(DocJob::AiTool(
                            ToolUse::new(
                                "session.compact",
                                Permission::Read,
                                format!("会话压缩：前面 {} 轮并成摘要", compaction.lines.len()),
                            )
                            .line(),
                        ));
                    }
                    Err(error) => send(DocJob::AiNote(format!(
                        "会话压缩没做成（{error:#}），这次只带最近几轮。"
                    ))),
                }
            }
            board.system_prompt = with_history(&board.system_prompt, &board.history);
            let kb = RagSearch {
                enabled: use_rag,
                rag: config.resolved_rag(),
                chat: config.draft_chat().unwrap_or_default(),
                kind_filter,
            };
            let mut batch = Batch::default();
            let result = {
                let mut emit = |event: Event| match event {
                    Event::Content(text) => batch.push(&text, "", &send),
                    Event::Reasoning(text) => batch.push("", &text, &send),
                    other => {
                        batch.flush(&send);
                        send(match other {
                            Event::Tool(tool) => DocJob::AiTool(tool.line()),
                            Event::Phase(phase) => DocJob::ExportProgress(phase),
                            Event::Workspace(text) => DocJob::AiWorkspace(text),
                            Event::WriteBegin { prefix, suffix } => {
                                DocJob::AiWriteBegin { prefix, suffix }
                            }
                            Event::Note(note) => DocJob::AiNote(note),
                            Event::Content(_) | Event::Reasoning(_) => unreachable!("上面已分流"),
                        });
                    }
                };
                run_skill(
                    (pick, style),
                    &mut board,
                    start,
                    use_rag,
                    &config,
                    &model,
                    &kb,
                    (&apis, &secrets),
                    &mut emit,
                )
            };
            batch.flush(&send);
            let (result, used_style) = result;
            if let Some(id) = used_style {
                send(DocJob::StyleUsed(id));
            }
            send(DocJob::AiUsage {
                turn_id,
                usage: model.usage(),
            });
            for note in model.take_notices() {
                send(DocJob::AiNote(note));
            }
            send(DocJob::SkillDone(result.map(Box::new)));
        });
        Ok(())
    }

    /// 新开一轮：选技能、查条件、备黑板、在任务流里开卡片。
    pub(super) fn prepare_skill(
        &mut self,
        request: &TurnRequest,
        time: &crate::prompt::TimeContext,
    ) -> Result<(Pick, Board, usize, bool, String, HistoryPlan), String> {
        let load_notes = self.doc.ai_panel.reload_skills();
        let skills = self.doc.ai_panel.skills.clone();
        // 「改提案」：有待确认的提案、没锁选区时，修改类技能在提案上接着改（16.15 B.7）。
        // 新提案的改前稿仍是正文，接受时照常对照正文。
        let proposal_text = self
            .doc
            .ai_proposal
            .as_ref()
            .filter(|_| request.on_proposal && request.selection.is_none())
            .map(|proposal| proposal.result.markdown.clone());
        let markdown = proposal_text
            .as_ref()
            .unwrap_or(&self.doc.generated_markdown);
        let selected = match &request.selection {
            Some(selection) => Some(
                locate_selection(markdown, selection)
                    .ok_or("锁定的选区已被改动或删除，请重新选择。")?,
            ),
            None => None,
        };
        let (pinned, text) = match router::slash(&skills, &request.text) {
            Some((id, rest)) => (Some(id), rest.to_string()),
            None => (request.skill.clone(), request.text.clone()),
        };
        // 选技能时不看引用的书名（与输入框里的技能标签一致）；交给技能的原话照旧带着。
        let routing = super::mention::routing_text(&text, &request.refs);
        let ctx = RouteContext {
            text: &routing,
            has_text: !markdown.trim().is_empty(),
            has_selection: selected.is_some(),
            kind: self.doc.draft.kind,
        };
        let pick = match pinned {
            Some(id) => {
                let skill = skills
                    .iter()
                    .find(|skill| skill.id == id)
                    .ok_or_else(|| format!("没有 id 为「{id}」的技能。"))?;
                if let Some(why) = unavailable(skill, &ctx) {
                    return Err(why);
                }
                Pick::Fixed(Box::new(skill.clone()))
            }
            None => match router::route(&skills, &ctx) {
                Route::Picked(id) => Pick::Fixed(Box::new(
                    skills
                        .iter()
                        .find(|skill| skill.id == id)
                        .cloned()
                        .expect("路由只会选列表里的技能"),
                )),
                Route::Ambiguous(ids) => Pick::Ask(
                    skills
                        .iter()
                        .filter(|skill| ids.contains(&skill.id))
                        .cloned()
                        .collect(),
                ),
                Route::Nothing => return Err("当前情况下没有可用的技能。".into()),
            },
        };
        let candidates: Vec<&Skill> = match &pick {
            Pick::Fixed(skill) => vec![&**skill],
            Pick::Ask(list) => list.iter().collect(),
        };
        let preset = request
            .preset
            .filter(|_| candidates.iter().any(|skill| skill.uses_preset()))
            .and_then(|id| self.config.ai_prompt(id));
        if text.trim().is_empty() && preset.is_none() {
            return Err(if candidates.iter().any(|skill| skill.uses_preset()) {
                "写下要求，或选一个润色预设。".into()
            } else {
                "写下材料或要求。".into()
            });
        }
        let use_rag = request.use_rag
            && self.config.rag.enabled
            && candidates.iter().any(|skill| skill.uses_knowledge());
        let mut title = match &pick {
            Pick::Fixed(skill) => skill.name.clone(),
            Pick::Ask(_) => "自动选择技能".to_string(),
        };
        if let Some(prompt) = preset {
            title.push_str(" · ");
            title.push_str(&prompt.name);
        }
        let mut context = Vec::new();
        if proposal_text.is_some() {
            context.push("改提案".to_string());
        }
        if use_rag {
            context.push("知识库".to_string());
        }
        for reference in &request.refs {
            context.push(format!("《{}》", reference.title));
        }
        if let Some(text) = &selected {
            context.push(format!("选区 {} 字", text.chars().count()));
        } else if candidates.iter().all(|s| s.when.text == TextNeed::Present) {
            context.push("全文".to_string());
        } else if ctx.has_text {
            context.push("生成新稿提案".to_string());
        }
        // 本会话里答过的题并进「已确认」，下一轮动笔前澄清不再问（16.15 B.7）。
        let mut notes: Vec<String> = Vec::new();
        for note in self
            .doc
            .ai_panel
            .turns
            .iter()
            .filter_map(|turn| turn.request.as_ref())
            .flat_map(|earlier| earlier.notes.iter())
            .chain(request.notes.iter())
        {
            if !notes.contains(note) {
                notes.push(note.clone());
            }
        }
        let window =
            crate::lmstudio::context::peek_window(&self.config.draft_chat().unwrap_or_default())
                .tokens;
        let plan = history::plan(&self.doc.ai_panel, window, false);
        let board = Board {
            draft: self.doc.draft.clone(),
            request: text.trim().to_string(),
            notes,
            clarified: !request.notes.is_empty(),
            history: plan.text(),
            document: markdown.clone(),
            workspace: markdown.clone(),
            selection: selected,
            preset: preset.map(|p| p.instruction.clone()).unwrap_or_default(),
            refs: request.refs.clone(),
            time_sources: [
                &time.now,
                &time.yesterday,
                &time.today,
                &time.tomorrow,
                &time.day_after_tomorrow,
                &time.three_days_later,
                &time.this_week,
                &time.next_week,
            ]
            .map(String::as_str)
            .join("\n"),
            ..Board::default()
        };
        let prompt = if request.text.is_empty() {
            title.clone()
        } else {
            request.text.clone()
        };
        self.doc
            .ai_panel
            .push_turn(title.clone(), prompt, context, Some(request.clone()));
        for note in load_notes {
            self.doc.ai_panel.note(note);
        }
        Ok((pick, board, 0, use_rag, title, plan))
    }

    /// 这一轮用哪份写法风格（16.15 C.3）：用户指定的，或按文种与场合自动挑；分不出的交给后台
    /// 让辅助模型挑。
    fn style_pick(&self, request: &TurnRequest, board: &Board) -> StylePick {
        use crate::agent::style::{Ranked, StyleChoice, rank};
        let styles = &self.doc.ai_panel.styles;
        match &request.style {
            StyleChoice::Off => StylePick::None,
            StyleChoice::Fixed(id) => styles
                .iter()
                .find(|style| &style.id == id)
                .map_or(StylePick::None, |style| {
                    StylePick::Fixed(Box::new(style.clone()), "指定")
                }),
            StyleChoice::Auto => {
                let text = format!("{}\n{}", board.draft.title_hint, board.request);
                match rank(styles, board.draft.kind, &text) {
                    Ranked::Picked(style) => StylePick::Fixed(style, "自动选"),
                    Ranked::Tie(list) => StylePick::Ask(list),
                    Ranked::Nothing => StylePick::None,
                }
            }
        }
    }

    /// 手动压缩会话（`/compact`、「•••」→「压缩会话」）：最近 3 轮之前的都并进会话摘要。
    pub(crate) fn start_compact(&mut self) {
        if self.doc.busy {
            *self.status = "这篇稿件还有任务在跑，稍等一下。".into();
            return;
        }
        let window =
            crate::lmstudio::context::peek_window(&self.config.draft_chat().unwrap_or_default())
                .tokens;
        let Some(compaction) = history::plan(&self.doc.ai_panel, window, true).compact else {
            *self.status = format!(
                "会话还短（不超过 {} 轮新内容），不用压缩。",
                history::KEEP_RECENT
            );
            return;
        };
        let (key, seq) = self.begin_job();
        *self.status = "正在压缩会话…".into();
        let config = self.config.clone();
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let model = LmBackend::new(&config, Arc::new(AtomicBool::new(false)));
            let result = compact_session(&model, &compaction)
                .map(|summary| (summary, compaction.upto, compaction.lines.len()))
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::AiCompactDone(result),
            });
        });
    }

    /// 挂起时的题答完了：回答落到黑板（要切的文种由这里切），接着跑。
    pub(crate) fn answer_suspended(&mut self, turn_id: u64) {
        if self.doc.busy {
            *self.status = "这篇稿件还有任务在跑，稍等一下。".into();
            return;
        }
        let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) else {
            return;
        };
        let unanswered = turn
            .questions
            .iter()
            .zip(&turn.replies)
            .any(|(question, draft)| !question.skippable && draft.reply() == Reply::Skip);
        if unanswered {
            *self.status = "还有必答的题没选。".into();
            return;
        }
        let replies = collect_replies(&turn.questions, &turn.replies);
        let (Some(mut run), Some(mut request)) = (turn.run.take(), turn.request.clone()) else {
            return;
        };
        let kind = engine::apply_answers(&mut run.board, &run.suspension, &replies);
        // 「重新生成」沿用这次的回答，不再问一遍。
        request.notes.clone_from(&run.board.notes);
        turn.request = Some(request.clone());
        if let Some(kind) = kind {
            // 用户亲手点的「切换文种」：界面线程改要素，与在要素区下拉框里选是同一回事。
            self.doc.draft.kind = kind;
        }
        if let Err(error) = self.start_skill(request, Some((turn_id, run))) {
            *self.status = error;
        }
    }

    /// 交付后附在提案上的题答完了：按确定性规则落到工作稿，重新定稿成新的提案。
    pub(crate) fn apply_research_answers(&mut self, turn_id: u64) {
        let Some(turn) = self.doc.ai_panel.turn_mut(turn_id) else {
            return;
        };
        let Some(research) = turn.research.clone() else {
            return;
        };
        let replies = collect_replies(&turn.questions, &turn.replies);
        let questions = turn.questions.clone();
        let Some(before) = self.doc.ai_proposal.as_ref().map(|p| p.before.clone()) else {
            *self.status = "提案已不在了，没法按回答修订；可以重新起草。".into();
            return;
        };
        let summary_lines: Vec<String> = questions
            .iter()
            .zip(&replies)
            .map(|(question, (_, reply))| {
                let target = question.text.split('？').next().unwrap_or(&question.text);
                match reply {
                    Reply::Custom(text) => format!("{target}：{text}"),
                    Reply::Choice(index) => format!(
                        "{target}：{}",
                        question
                            .choices
                            .get(*index)
                            .map_or("", |c| c.label.as_str())
                    ),
                    Reply::Skip => format!("{target}：保留待核实"),
                }
            })
            .collect();
        let label = "按回答修订";
        let needs_model = questions
            .iter()
            .zip(&replies)
            .any(|(question, (_, reply))| {
                matches!(
                    clarify::gap_edit(question, reply),
                    clarify::GapEdit::Fill(_) | clarify::GapEdit::Drop
                )
            });
        if !needs_model {
            // 全是保留待核实、保留原文：确定性改完就定稿，不调模型。
            let mut ledger = research.ledger.clone();
            let raw = clarify::apply_gap_replies(&research.raw, &mut ledger, &questions, &replies);
            self.install_research_revision(
                label,
                summary_lines.join("\n"),
                before,
                ResearchSnapshot {
                    raw,
                    ledger,
                    sources: research.sources,
                },
            );
            *self.status = "已按你的回答修订，新的提案在侧栏里。".into();
            return;
        }
        if self.doc.busy {
            *self.status = "这篇稿件还有任务在跑，稍等一下。".into();
            return;
        }
        self.doc
            .ai_panel
            .push_turn(label.into(), summary_lines.join("\n"), Vec::new(), None);
        self.doc.ai_panel.set_phase("正在把你的回答写进所在段落…");
        let (key, seq) = self.begin_job();
        let cancel = Arc::new(AtomicBool::new(false));
        self.doc.ai_panel.cancel = Some(cancel.clone());
        let config = self.config.clone();
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let model = LmBackend::new(&config, cancel);
            let mut emit = |line: String| {
                let _ = tx.send(WorkerResult::Doc {
                    key,
                    seq,
                    job: DocJob::AiTool(line),
                });
            };
            let revised = crate::agent::gap_revise::revise(
                &model,
                &research.raw,
                research.ledger,
                &questions,
                &replies,
                &config.vocabulary,
                &mut emit,
            );
            // 还没定的几处接着出题，顺带要建议写法。
            use crate::agent::backend::ModelBackend as _;
            let mut remaining =
                clarify::gap_questions(&revised.ledger, BATCH_QUESTIONS, &config.vocabulary);
            let today = crate::prompt::TimeContext::now().today;
            if let Some(text) =
                clarify::suggestion_prompt(&remaining, &revised.ledger, &revised.raw, &today)
                && let Ok(reply) = model.complete(
                    crate::agent::backend::ModelRole::Assist,
                    crate::agent::tools::ASSIST_SYSTEM,
                    &text,
                    &mut |_| {},
                )
            {
                clarify::add_suggestions(&mut remaining, &reply.content);
            }
            let _ = tx.send(WorkerResult::Doc {
                key,
                seq,
                job: DocJob::GapRevised(Box::new(GapRevision {
                    before,
                    questions: remaining,
                    research: ResearchSnapshot {
                        raw: revised.raw,
                        ledger: revised.ledger,
                        sources: research.sources,
                    },
                })),
            });
        });
    }

    /// 撤回 AI 对一处缺口的概括：所在句换回原句（带「【待核实】」占位），缺口改为出题问用户，
    /// 重新定稿成提案。全是确定性替换，不调模型。
    pub(crate) fn revert_generalized(&mut self, turn_id: u64, gap_id: usize) {
        let Some(research) = self
            .doc
            .ai_panel
            .turn_mut(turn_id)
            .and_then(|turn| turn.research.clone())
        else {
            return;
        };
        let Some(before) = self.doc.ai_proposal.as_ref().map(|p| p.before.clone()) else {
            *self.status = "提案已不在了，没法撤回；可以重新起草。".into();
            return;
        };
        let mut ledger = research.ledger;
        let Some(gap) = ledger.get_mut(gap_id) else {
            return;
        };
        let GapStatus::Generalized(original) = gap.status.clone() else {
            return;
        };
        if !research.raw.contains(&gap.sentence) {
            *self.status = "找不到 AI 改写的那一句了，没法撤回。".into();
            return;
        }
        let raw = research.raw.replacen(&gap.sentence, &original, 1);
        let hint = gap.hint.clone();
        gap.sentence = original;
        gap.status = GapStatus::NoAnswer;
        self.install_research_revision(
            "撤回概括",
            format!("「{hint}」恢复成待核实，改为问你"),
            before,
            ResearchSnapshot {
                raw,
                ledger,
                sources: research.sources,
            },
        );
        *self.status = format!("已撤回「{hint}」的概括表述，题目在侧栏里。");
    }

    /// 研究式起草的工作稿改过之后：开新的一轮、重新定稿成提案，台账里还要问的出成题附上。
    fn install_research_revision(
        &mut self,
        label: &str,
        prompt: String,
        before: String,
        research: ResearchSnapshot,
    ) {
        self.doc
            .ai_panel
            .push_turn(label.to_string(), prompt, Vec::new(), None);
        finish_research_revision(self.doc, self.config, label, before, research, None);
    }
}

/// 按回答修订的后台结果：提案的对照基准、改好的工作稿与还要问的题（已附建议写法）。
pub(crate) struct GapRevision {
    pub(crate) before: String,
    pub(crate) questions: Vec<Question>,
    pub(crate) research: ResearchSnapshot,
}

/// 正在跑的那一轮收尾：改好的工作稿重新定稿成提案，台账里还要问的出成题附上。
pub(crate) fn finish_research_revision(
    doc: &mut crate::draft_page::DraftSession,
    config: &crate::models::AppConfig,
    label: &str,
    before: String,
    research: ResearchSnapshot,
    questions: Option<Vec<Question>>,
) {
    let draft = crate::draft_page::reviewed_draft(&doc.draft, config, &research.raw, false);
    let summary =
        GongwenApp::install_ai_proposal(doc, before, draft, label.to_string(), &config.vocabulary);
    let remaining = questions.unwrap_or_else(|| {
        clarify::gap_questions(&research.ledger, BATCH_QUESTIONS, &config.vocabulary)
    });
    let panel = &mut doc.ai_panel;
    let Some(id) = panel.running_turn_mut().map(|turn| turn.id) else {
        return;
    };
    panel.replace_content(research.raw.clone());
    panel.finish(TurnState::Proposed(summary));
    if let Some(turn) = panel.turn_mut(id) {
        turn.replies = initial_replies(&remaining);
        turn.questions = remaining;
        turn.research = Some(research);
    }
}

/// 让辅助模型把较早的轮次并进会话摘要。
fn compact_session(
    model: &dyn crate::agent::backend::ModelBackend,
    compaction: &history::Compaction,
) -> anyhow::Result<String> {
    let completion = model.complete(
        crate::agent::backend::ModelRole::Assist,
        crate::agent::tools::ASSIST_SYSTEM,
        &history::compact_prompt(compaction),
        &mut |_| {},
    )?;
    let summary = completion.content.trim().to_string();
    if summary.is_empty() {
        anyhow::bail!("模型没给出摘要");
    }
    Ok(summary)
}

/// 起草模型的系统提示带上会话历史：只用来理解指代，不当要求、不当出处。
fn with_history(system: &str, history: &str) -> String {
    if history.trim().is_empty() {
        return system.to_string();
    }
    format!(
        "{system}\n\n【本会话之前的往来】（只用来理解这次要求里的指代，如「再短一点」「第二条」指的是哪份稿子、\
         哪一条；不是新的要求，里面出现的事实也不能当作出处）\n{history}"
    )
}

/// 这一轮的写法风格：定了的，或要后台交模型挑的几份。
pub(crate) enum StylePick {
    None,
    /// 档案与来由（「指定」「自动选」）。
    Fixed(Box<crate::agent::style::StyleProfile>, &'static str),
    Ask(Vec<crate::agent::style::StyleProfile>),
}

/// 写稿的技能定下风格：排进黑板（`board.style`），返回用了哪份。审核类技能不用风格。
pub(super) fn apply_style(
    style: StylePick,
    skill: &Skill,
    board: &mut Board,
    model: &dyn crate::agent::backend::ModelBackend,
    emit: &mut dyn FnMut(Event),
) -> Option<String> {
    if skill.output == OutputKind::Report || !board.style.is_empty() {
        return None;
    }
    let (profile, reason) = match style {
        StylePick::None => return None,
        StylePick::Fixed(profile, reason) => (*profile, reason),
        StylePick::Ask(list) => {
            match crate::agent::style::choose_by_model(model, &list, &board.request) {
                Ok(Some(index)) => (list[index].clone(), "模型从几份里挑的"),
                Ok(None) => return None,
                Err(error) => {
                    emit(Event::Note(format!(
                        "挑风格时模型出错（{error:#}），这次不用风格。"
                    )));
                    return None;
                }
            }
        }
    };
    // 风格占起草模型上下文的十分之一以内，放不下先砍范例。
    let budget = model.window(crate::agent::backend::ModelRole::Draft).tokens / 10;
    board.style = crate::agent::style::render(&profile, budget);
    emit(Event::Tool(ToolUse::new(
        "style",
        Permission::Read,
        format!("风格：{}（{reason}）", profile.name),
    )));
    Some(profile.id)
}

/// 后台线程：定技能、定风格、跑引擎、定稿。返回结果与用了哪份风格。
#[allow(clippy::too_many_arguments)]
fn run_skill(
    (pick, style): (Pick, StylePick),
    board: &mut Board,
    start: usize,
    use_rag: bool,
    config: &crate::models::AppConfig,
    model: &LmBackend,
    kb: &RagSearch,
    (apis, secrets): (&ApiStore, &ApiSecrets),
    emit: &mut dyn FnMut(Event),
) -> (Result<SkillResult, String>, Option<String>) {
    let skill = match resolve_skill(pick, board, model, emit) {
        Ok(skill) => skill,
        Err(error) => return (Err(error), None),
    };
    let used = apply_style(style, &skill, board, model, emit);
    if !board.style.is_empty() {
        board.system_prompt = format!("{}\n\n{}", board.system_prompt, board.style);
    }
    (
        run_engine(
            skill,
            board,
            start,
            use_rag,
            config,
            model,
            kb,
            (apis, secrets),
            emit,
        ),
        used,
    )
}

/// 定技能：选定了的，或交模型从候选里挑。
fn resolve_skill(
    pick: Pick,
    board: &Board,
    model: &LmBackend,
    emit: &mut dyn FnMut(Event),
) -> Result<Skill, String> {
    Ok(match pick {
        Pick::Fixed(skill) => *skill,
        Pick::Ask(mut candidates) => {
            let refs: Vec<&Skill> = candidates.iter().collect();
            // 追问（「再短一点」）光看原话分不出该用哪个技能：带上最近一轮的往来。
            let routing = match board.history.rsplit("\n\n").next() {
                Some(last) if !last.trim().is_empty() => format!(
                    "{}\n\n（本会话上一轮：{}）",
                    board.request,
                    crate::agent::tools::short(last, 300)
                ),
                _ => board.request.clone(),
            };
            let index = router::choose_by_model(model, &refs, &routing)
                .map_err(|e| format!("选技能时模型出错：{e:#}"))?;
            let skill = candidates.swap_remove(index);
            emit(Event::Tool(ToolUse::new(
                "router",
                Permission::Read,
                format!("按「{}」处理（模型判断）", skill.name),
            )));
            skill
        }
    })
}

/// 跑引擎、定稿。
#[allow(clippy::too_many_arguments)]
fn run_engine(
    skill: Skill,
    board: &mut Board,
    start: usize,
    use_rag: bool,
    config: &crate::models::AppConfig,
    model: &LmBackend,
    kb: &RagSearch,
    (apis, secrets): (&ApiStore, &ApiSecrets),
    emit: &mut dyn FnMut(Event),
) -> Result<SkillResult, String> {
    let env = Env {
        config,
        vocabulary: &config.vocabulary,
        kb,
        manuscripts: &SqliteManuscripts,
        model,
        skill: &skill,
        apis,
        secrets,
    };
    match engine::run(board, &env, start, emit).map_err(|e| format!("{e:#}"))? {
        Outcome::Suspended(suspension) => Ok(SkillResult::Suspended(Box::new(SkillRun {
            board: board.clone(),
            skill,
            suspension,
            use_rag,
        }))),
        Outcome::Done if skill.output.is_report(board) => {
            let mut findings = board.findings.clone();
            // 自主步骤的答复（「文中共有 3 处日期」这类）作清单的第一条。
            if skill.output == OutputKind::Auto
                && !findings.iter().any(|f| f.group == "答复")
                && let Some(summary) = board
                    .var_text(crate::agent::ops::AGENT_SUMMARY)
                    .filter(|summary| !summary.trim().is_empty())
            {
                findings.insert(
                    0,
                    Finding {
                        group: "答复".into(),
                        text: summary,
                        excerpt: String::new(),
                        source: String::new(),
                        fix: None,
                    },
                );
            }
            let style = board
                .vars
                .get(crate::agent::ops::STYLE_PROFILE)
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .map(Box::new);
            Ok(SkillResult::Report {
                skill: skill.name,
                skill_id: skill.id,
                findings,
                style,
            })
        }
        Outcome::Done => {
            let report = SkillReport::from_board(board);
            if report.markdown.trim().is_empty() {
                return Err(format!("「{}」没有写出内容。", skill.name));
            }
            let draft = crate::draft_page::reviewed_draft(
                &board.draft,
                config,
                &report.markdown,
                report.truncated,
            );
            Ok(SkillResult::Proposal {
                skill: skill.name,
                draft,
                report: Box::new(report),
            })
        }
    }
}

/// 挂起时题目的用途，决定卡片上的说法。
pub(crate) fn asking_labels(questions: &[Question]) -> (&'static str, &'static str, &'static str) {
    if questions.iter().any(|q| q.target.is_predraft()) {
        (
            "动笔前先确认这几件事",
            "确认，开始起草",
            "跳过，按现有信息写",
        )
    } else if questions.iter().any(|q| q.target == Target::Pick) {
        ("先选一下", "确认，继续", "跳过")
    } else {
        ("这几处要你确认", "确认，继续", "保留待核实")
    }
}

/// 流式增量的攒批。
#[derive(Default)]
struct Batch {
    content: String,
    reasoning: String,
    last: Option<Instant>,
}

impl Batch {
    fn push(&mut self, content: &str, reasoning: &str, send: &dyn Fn(DocJob)) {
        self.content.push_str(content);
        self.reasoning.push_str(reasoning);
        let due = self
            .last
            .is_none_or(|last| last.elapsed() >= FLUSH_INTERVAL);
        if due || self.content.len() + self.reasoning.len() >= FLUSH_BYTES {
            self.flush(send);
        }
    }

    fn flush(&mut self, send: &dyn Fn(DocJob)) {
        if self.content.is_empty() && self.reasoning.is_empty() {
            return;
        }
        send(DocJob::AiStream {
            content: std::mem::take(&mut self.content),
            reasoning: std::mem::take(&mut self.reasoning),
            done: false,
        });
        self.last = Some(Instant::now());
    }
}
