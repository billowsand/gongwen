//! 技能夹具测试的共用件：按提示词回话的假模型、按关键词返回片段的假知识库，以及「在黑板上
//! 跑技能、挂起时代用户回答、接着跑」的驱动。

use super::api::ApiStore;
use super::backend::{Completion, ModelBackend, ModelRole};
use super::board::Board;
use super::clarify::Reply;
use super::engine::{self, Event, Outcome, Suspension};
use super::skill::Skill;
use super::tools::testing::FakeManuscripts;
use super::tools::{Env, KnowledgeSearch};
use crate::lmstudio::StreamDelta;
use crate::models::{AppConfig, TemplateKind};
use crate::rag::RetrievedChunk;
use std::cell::RefCell;

type Responder = Box<dyn Fn(ModelRole, &str) -> String>;

#[test]
fn skill_usage_accumulates_before_and_after_answering() {
    let skill = super::skill::parse("usage", "---\nname: 用量\ntools: [llm.generate, ask.choice]\nflow:\n  - step: generate\n    prompt: 正文\n  - tool: ask.choice\n    args: { question: 继续吗？, options: [继续, 停止] }\n  - step: generate\n    prompt: 正文\n---\n## 正文\n请写工作稿。\n", "测试").unwrap();
    for reported in [false, true] {
        let mut model = ScriptedModel::new(|_, _| "工作稿。".into());
        if reported {
            model = model.with_usage(crate::lmstudio::Usage {
                prompt_tokens: 10,
                completion_tokens: 3,
                total_tokens: 13,
            });
        }
        let kb = KeywordKb::disabled();
        let mut driver = Driver::new(&skill, &model, &kb, Board::default());
        let suspended = driver.run().expect("中间提问挂起");
        assert_eq!(model.usage().calls.len(), 1);
        driver.answer(&suspended, &[(suspended.questions[0].id, Reply::Choice(0))]);
        assert!(driver.run().is_none());
        let totals = model.usage();
        assert_eq!(totals.calls.len(), 2);
        assert_eq!(totals.calls.iter().any(|c| c.estimated), !reported);
        if reported {
            assert_eq!(
                totals
                    .calls
                    .iter()
                    .map(|c| c.tokens.total_tokens)
                    .sum::<u64>(),
                26
            );
        }
    }
}

/// 按提示词内容决定怎么回的假模型，记下每次被问了什么。
pub(crate) struct ScriptedModel {
    usage: RefCell<super::backend::UsageTotals>,
    reported_usage: Option<crate::lmstudio::Usage>,
    respond: Responder,
    pub(crate) calls: RefCell<Vec<(ModelRole, String)>>,
    /// 假装的上下文窗口（token）。
    window: usize,
}

impl ScriptedModel {
    pub(crate) fn new(respond: impl Fn(ModelRole, &str) -> String + 'static) -> Self {
        Self {
            usage: RefCell::new(Default::default()),
            reported_usage: None,
            respond: Box::new(respond),
            calls: RefCell::new(Vec::new()),
            window: crate::lmstudio::context::DEFAULT_WINDOW,
        }
    }

    /// 换一个上下文窗口，测装箱用。
    pub(crate) fn with_window(mut self, tokens: usize) -> Self {
        self.window = tokens;
        self
    }

    pub(crate) fn with_usage(mut self, usage: crate::lmstudio::Usage) -> Self {
        self.reported_usage = Some(usage);
        self
    }

    /// 提示词里含 `needle` 的调用有几次。
    pub(crate) fn asked(&self, needle: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|(_, prompt)| prompt.contains(needle))
            .count()
    }

    /// 含 `needle` 的那几次调用的提示词。
    pub(crate) fn prompts(&self, needle: &str) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|(_, prompt)| prompt.contains(needle))
            .map(|(_, prompt)| prompt.clone())
            .collect()
    }
}

impl ModelBackend for ScriptedModel {
    fn usage(&self) -> super::backend::UsageTotals {
        self.usage.borrow().clone()
    }
    fn complete(
        &self,
        role: ModelRole,
        system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        self.calls.borrow_mut().push((role, user.to_string()));
        let reply = (self.respond)(role, user);
        self.usage.borrow_mut().record(
            role,
            "脚本模型",
            crate::lmstudio::context::estimate_tokens(system)
                + crate::lmstudio::context::estimate_tokens(user),
            &reply,
            self.reported_usage,
            0,
        );
        on_delta(StreamDelta::Content(&reply));
        Ok(Completion {
            content: reply,
            truncated: false,
        })
    }

    fn cancelled(&self) -> bool {
        false
    }

    fn window(&self, _role: ModelRole) -> crate::lmstudio::context::Window {
        crate::lmstudio::context::Window {
            tokens: self.window,
            source: crate::lmstudio::context::WindowSource::Manual,
        }
    }
}

/// 检索词里含关键词就返回对应片段的假知识库。
pub(crate) struct KeywordKb {
    pub(crate) enabled: bool,
    pub(crate) docs: Vec<(&'static str, RetrievedChunk)>,
    pub(crate) queries: RefCell<Vec<String>>,
}

impl KeywordKb {
    pub(crate) fn new(docs: Vec<(&'static str, RetrievedChunk)>) -> Self {
        Self {
            enabled: true,
            docs,
            queries: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::new(Vec::new())
        }
    }
}

impl KnowledgeSearch for KeywordKb {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn search(&self, query: &str) -> anyhow::Result<(Vec<RetrievedChunk>, Vec<String>)> {
        self.queries.borrow_mut().push(query.to_string());
        Ok((
            self.docs
                .iter()
                .filter(|(word, _)| query.contains(word))
                .map(|(_, chunk)| chunk.clone())
                .collect(),
            Vec::new(),
        ))
    }

    /// 文档列表：每个片段算一篇（同一 `doc_id` 只列一次）。
    fn list(
        &self,
        _kind: Option<TemplateKind>,
    ) -> anyhow::Result<Vec<(i64, String, TemplateKind)>> {
        let mut out: Vec<(i64, String, TemplateKind)> = Vec::new();
        for (_, chunk) in &self.docs {
            if !out.iter().any(|(id, ..)| *id == chunk.doc_id) {
                out.push((chunk.doc_id, chunk.doc_title.clone(), chunk.kind));
            }
        }
        Ok(out)
    }

    /// 按文档 id 读全文：片段的 `doc_id` 对上就返回那一段。
    fn read(&self, doc_id: i64) -> anyhow::Result<Option<(String, String)>> {
        Ok(self
            .docs
            .iter()
            .find(|(_, chunk)| chunk.doc_id == doc_id)
            .map(|(_, chunk)| (chunk.doc_title.clone(), chunk.text.clone())))
    }
}

pub(crate) fn chunk(id: i64, title: &str, text: &str) -> RetrievedChunk {
    RetrievedChunk {
        chunk_id: id,
        doc_id: id,
        doc_title: title.into(),
        kind: TemplateKind::PlainDocument,
        section: String::new(),
        text: text.into(),
        vector_score: 0.0,
        bm25_score: 0.0,
        fused_score: 0.0,
        rerank_score: None,
    }
}

/// 跑技能的驱动：持有环境，记下全部事件与每次挂起。
pub(crate) struct Driver<'a> {
    pub(crate) skill: &'a Skill,
    pub(crate) model: &'a ScriptedModel,
    pub(crate) kb: &'a KeywordKb,
    pub(crate) manuscripts: FakeManuscripts,
    pub(crate) apis: ApiStore,
    pub(crate) vocabulary: Vec<crate::models::VocabularyEntry>,
    pub(crate) board: Board,
    pub(crate) events: Vec<Event>,
    pub(crate) next: usize,
}

impl<'a> Driver<'a> {
    pub(crate) fn new(
        skill: &'a Skill,
        model: &'a ScriptedModel,
        kb: &'a KeywordKb,
        board: Board,
    ) -> Self {
        Self {
            skill,
            model,
            kb,
            manuscripts: FakeManuscripts { docs: Vec::new() },
            apis: ApiStore::default(),
            vocabulary: Vec::new(),
            board,
            events: Vec::new(),
            next: 0,
        }
    }

    /// 从上次停下的地方跑到挂起或结束。挂起时返回题目。
    pub(crate) fn run(&mut self) -> Option<Suspension> {
        self.try_run().expect("技能应当跑通")
    }

    /// 同 [`Driver::run`]，出错时把错误交回来。
    pub(crate) fn try_run(&mut self) -> anyhow::Result<Option<Suspension>> {
        let config = AppConfig::default();
        let env = Env {
            config: &config,
            vocabulary: &self.vocabulary,
            kb: self.kb,
            manuscripts: &self.manuscripts,
            model: self.model,
            skill: self.skill,
            apis: &self.apis,
            secrets: &Default::default(),
        };
        let events = &mut self.events;
        Ok(
            match engine::run(&mut self.board, &env, self.next, &mut |event| {
                events.push(event)
            })? {
                Outcome::Done => None,
                Outcome::Suspended(suspension) => {
                    self.next = suspension.resume_at;
                    Some(suspension)
                }
            },
        )
    }

    /// 像用户点「确认」一样回答挂起的题，返回用户选中的文种（若有）。
    pub(crate) fn answer(
        &mut self,
        suspension: &Suspension,
        replies: &[(usize, Reply)],
    ) -> Option<TemplateKind> {
        engine::apply_answers(&mut self.board, suspension, replies)
    }

    /// 任务流里的工具调用行。
    pub(crate) fn tool_lines(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Tool(tool) => Some(tool.summary.clone()),
                _ => None,
            })
            .collect()
    }
}
