//! 技能任务：侧栏发起的每一轮都按一个技能跑流程引擎，挂在 `DraftPage` 上。
//!
//! - `start_skill`：选技能（显式 / 触发词 / 交模型判断）、备好黑板，后台线程跑
//!   `agent::engine`，事件转成 `DocJob` 回投；也用来接着跑挂起的那一轮；
//! - `answer_suspended`：挂起时的题答完——回答由引擎按确定性规则落到黑板，要切文种的由
//!   界面线程切（用户点选，不是模型改），然后从下一步接着跑；
//! - `apply_research_answers`：交付后附在提案上的题答完——按确定性规则落到工作稿，重新
//!   定稿成提案，不再调模型。

use super::{ReplyDraft, ResearchSnapshot, TurnRequest, TurnState, locate_selection};
use crate::agent::api::{ApiSecrets, ApiStore};
use crate::agent::backend::LmBackend;
use crate::agent::board::Board;
use crate::agent::clarify::{self, Question, Reply, Target};
use crate::agent::engine::{self, Event, Outcome, SkillReport, Suspension};
use crate::agent::router::{self, Route, RouteContext};
use crate::agent::skill::{Skill, TextNeed};
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
}

/// 用哪个技能：选定了，或发送后交模型从候选里挑。
enum Pick {
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
        let (pick, mut board, start, use_rag, title) = match resume {
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
                turn.state = TurnState::Waiting;
                turn.started = Instant::now();
                turn.elapsed = None;
                let title = turn.title.clone();
                (
                    Pick::Fixed(Box::new(skill)),
                    board,
                    suspension.resume_at,
                    use_rag,
                    title,
                )
            }
            None => self.prepare_skill(&request, &time)?,
        };
        board.system_prompt = crate::prompt::build_system_prompt(&time);
        self.doc.ai_panel.open = true;
        let cancel = Arc::new(AtomicBool::new(false));
        self.doc.ai_panel.cancel = Some(cancel.clone());
        self.doc.ai_prompt_last_label = title.clone();
        self.doc.ai_review_baseline = Some(self.doc.generated_markdown.clone());
        self.doc.ai_proposal = None;
        let (key, seq) = self.begin_job();
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
                    "「{}」要用的数据接口还没配置：{}，相关步骤会跳过。可在设置页「数据接口」添加。",
                    skill.name,
                    missing.join("、")
                ));
            }
        }
        let kind_filter = self.doc.rag_kind_filter.resolve(self.doc.draft.kind);
        let tx = self.sender.clone();
        std::thread::spawn(move || {
            let send = |job: DocJob| {
                let _ = tx.send(WorkerResult::Doc { key, seq, job });
            };
            let model = LmBackend::new(&config, cancel);
            let kb = RagSearch {
                enabled: use_rag,
                rag: config.rag.clone(),
                chat: config.lm_studio.clone(),
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
                            Event::Note(note) => DocJob::AiNote(note),
                            Event::Content(_) | Event::Reasoning(_) => unreachable!("上面已分流"),
                        });
                    }
                };
                run_skill(
                    pick,
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
            send(DocJob::SkillDone(result.map(Box::new)));
        });
        Ok(())
    }

    /// 新开一轮：选技能、查条件、备黑板、在任务流里开卡片。
    fn prepare_skill(
        &mut self,
        request: &TurnRequest,
        time: &crate::prompt::TimeContext,
    ) -> Result<(Pick, Board, usize, bool, String), String> {
        let load_notes = self.doc.ai_panel.reload_skills();
        let skills = self.doc.ai_panel.skills.clone();
        let markdown = &self.doc.generated_markdown;
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
        let ctx = RouteContext {
            text: &text,
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
        if use_rag {
            context.push("知识库".to_string());
        }
        if let Some(text) = &selected {
            context.push(format!("选区 {} 字", text.chars().count()));
        } else if candidates.iter().all(|s| s.when.text == TextNeed::Present) {
            context.push("全文".to_string());
        } else if ctx.has_text {
            context.push("生成新稿提案".to_string());
        }
        let board = Board {
            draft: self.doc.draft.clone(),
            request: text.trim().to_string(),
            notes: request.notes.clone(),
            clarified: !request.notes.is_empty(),
            document: markdown.clone(),
            workspace: markdown.clone(),
            selection: selected,
            preset: preset.map(|p| p.instruction.clone()).unwrap_or_default(),
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
        Ok((pick, board, 0, use_rag, title))
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
        let mut ledger = research.ledger.clone();
        let raw = clarify::apply_gap_replies(&research.raw, &mut ledger, &questions, &replies);
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
        let draft = crate::draft_page::reviewed_draft(&self.doc.draft, self.config, &raw, false);
        let label = "按回答修订".to_string();
        self.doc
            .ai_panel
            .push_turn(label.clone(), summary_lines.join("\n"), Vec::new(), None);
        let summary = GongwenApp::install_ai_proposal(
            self.doc,
            before,
            draft,
            label,
            &self.config.vocabulary,
        );
        let remaining = clarify::gap_questions(&ledger, BATCH_QUESTIONS);
        let panel = &mut self.doc.ai_panel;
        let new_id = panel.turns.last().map(|turn| turn.id);
        panel.replace_content(raw.clone());
        panel.finish(TurnState::Proposed(summary));
        if let Some(turn) = new_id.and_then(|id| panel.turn_mut(id)) {
            turn.replies = initial_replies(&remaining);
            turn.questions = remaining;
            turn.research = Some(ResearchSnapshot {
                raw,
                ledger,
                sources: research.sources,
            });
        }
        *self.status = "已按你的回答修订，新的提案在侧栏里。".into();
    }
}

/// 后台线程：定技能、跑引擎、定稿。
#[allow(clippy::too_many_arguments)]
fn run_skill(
    pick: Pick,
    board: &mut Board,
    start: usize,
    use_rag: bool,
    config: &crate::models::AppConfig,
    model: &LmBackend,
    kb: &RagSearch,
    (apis, secrets): (&ApiStore, &ApiSecrets),
    emit: &mut dyn FnMut(Event),
) -> Result<SkillResult, String> {
    let skill = match pick {
        Pick::Fixed(skill) => *skill,
        Pick::Ask(mut candidates) => {
            let refs: Vec<&Skill> = candidates.iter().collect();
            let index = router::choose_by_model(model, &refs, &board.request)
                .map_err(|e| format!("选技能时模型出错：{e:#}"))?;
            let skill = candidates.swap_remove(index);
            emit(Event::Tool(ToolUse::new(
                "router",
                Permission::Read,
                format!("按「{}」处理（模型判断）", skill.name),
            )));
            skill
        }
    };
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
    if questions.iter().any(|q| q.target == Target::PreDraft) {
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
