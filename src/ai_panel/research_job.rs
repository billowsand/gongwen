//! 研究式起草的后台任务与回答处理，挂在 `DraftPage` 上。
//!
//! - `start_research`：开一轮（或接着动笔前问过的那一轮），后台线程跑 `agent::research`，
//!   事件转成 `DocJob` 回投；
//! - `answer_predraft`：动笔前的题答完——要切文种的由界面线程切（用户点选，不是模型改），
//!   其余作为已确认信息，接着跑；
//! - `apply_research_answers`：第一稿之后的题答完——按确定性规则落到工作稿，重新定稿成
//!   提案，不再调模型。

use super::{ReplyDraft, ResearchSnapshot, TurnRequest, TurnState};
use crate::agent::backend::LmBackend;
use crate::agent::clarify::{self, Question, Reply};
use crate::agent::research::{self, ResearchEvent, ResearchInput, ResearchOutcome, ResearchReport};
use crate::agent::skill;
use crate::agent::tools::RagSearch;
use crate::app::{DocJob, GongwenApp, WorkerResult};
use crate::draft_page::DraftPage;
use crate::models::GeneratedDraft;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// 研究式起草的结果，经 `DocJob::ResearchDone` 回投。
pub(crate) enum ResearchResult {
    /// 动笔前有题要问。
    Clarify(Vec<Question>),
    /// 定稿后的提案与研究过程。
    Proposal {
        draft: GeneratedDraft,
        report: Box<ResearchReport>,
    },
}

/// 第一稿之后一批最多问几题（技能参数 `batch_questions` 的兜底值）。
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

impl DraftPage<'_> {
    /// 发起研究式起草。`reuse` 是动笔前问过题的那一轮：答完接着跑，不另开一轮。
    pub(crate) fn start_research(
        &mut self,
        request: TurnRequest,
        clarified: bool,
        reuse: Option<u64>,
    ) -> Result<(), String> {
        if self.doc.read_only() {
            return Err("这篇稿件已发布或归档，只读。".into());
        }
        if self.doc.busy {
            return Err("这篇稿件还有任务在跑，稍等一下。".into());
        }
        if request.text.trim().is_empty() {
            return Err("写下材料或起草要求。".into());
        }
        let use_rag = request.use_rag && self.config.rag.enabled;
        let has_text = !self.doc.generated_markdown.trim().is_empty();
        let title = if use_rag {
            "起草 · 知识库"
        } else {
            "起草"
        }
        .to_string();
        match reuse.and_then(|id| self.doc.ai_panel.turn_mut(id)) {
            Some(turn) => {
                turn.request = Some(request.clone());
                turn.questions.clear();
                turn.replies.clear();
                turn.state = TurnState::Waiting;
                turn.started = Instant::now();
                turn.elapsed = None;
            }
            None => {
                let mut context = Vec::new();
                if use_rag {
                    context.push("知识库".to_string());
                }
                if has_text {
                    context.push("生成新稿提案".to_string());
                }
                let prompt = request.text.clone();
                self.doc
                    .ai_panel
                    .push_turn(title.clone(), prompt, context, Some(request.clone()));
            }
        }
        self.doc.ai_panel.open = true;
        let (skill, skill_note) = skill::load_research_draft();
        if let Some(note) = skill_note {
            self.doc.ai_panel.note(note);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.doc.ai_panel.cancel = Some(cancel.clone());
        self.doc.ai_prompt_last_label = title;
        self.doc.ai_review_baseline = Some(self.doc.generated_markdown.clone());
        self.doc.ai_proposal = None;
        let (key, seq) = self.begin_job();
        *self.status = "正在研究式起草…".into();

        let time = crate::prompt::TimeContext::now();
        let input = ResearchInput {
            draft: self.doc.draft.clone(),
            request: request.text.clone(),
            notes: request.notes.clone(),
            clarified,
            system_prompt: crate::prompt::build_system_prompt(&time),
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
        };
        let config = self.config.clone();
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
            let outcome = {
                let mut emit = |event: ResearchEvent| match event {
                    ResearchEvent::Content(text) => batch.push(&text, "", &send),
                    ResearchEvent::Reasoning(text) => batch.push("", &text, &send),
                    other => {
                        batch.flush(&send);
                        send(match other {
                            ResearchEvent::Tool(tool) => DocJob::AiTool(tool.line()),
                            ResearchEvent::Phase(phase) => DocJob::ExportProgress(phase),
                            ResearchEvent::Workspace(text) => DocJob::AiWorkspace(text),
                            ResearchEvent::Note(note) => DocJob::AiNote(note),
                            ResearchEvent::Content(_) | ResearchEvent::Reasoning(_) => {
                                unreachable!("上面已分流")
                            }
                        });
                    }
                };
                research::run(&input, &skill, &config.vocabulary, &model, &kb, &mut emit)
            };
            batch.flush(&send);
            let result = match outcome {
                Ok(ResearchOutcome::Clarify(questions)) => Ok(ResearchResult::Clarify(questions)),
                Ok(ResearchOutcome::Done(report)) => {
                    let draft = crate::draft_page::reviewed_draft(
                        &input.draft,
                        &config,
                        &report.markdown,
                        report.truncated,
                    );
                    Ok(ResearchResult::Proposal { draft, report })
                }
                Err(error) => Err(format!("{error:#}")),
            };
            send(DocJob::ResearchDone(result.map(Box::new)));
        });
        Ok(())
    }

    /// 动笔前的题答完了：切文种（如果选了）、记下回答，接着起草。
    pub(crate) fn answer_predraft(&mut self, turn_id: u64) {
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
        let (kind, notes) = clarify::resolve_predraft(&turn.questions, &replies);
        let Some(mut request) = turn.request.clone() else {
            return;
        };
        request.notes.extend(notes);
        if let Some(kind) = kind {
            // 用户亲手点的「切换文种」：界面线程改要素，与在要素区下拉框里选是同一回事。
            self.doc.draft.kind = kind;
        }
        if let Err(error) = self.start_research(request, true, Some(turn_id)) {
            *self.status = error;
        }
    }

    /// 第一稿之后的题答完了：按确定性规则落到工作稿，重新定稿成新的提案。
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
