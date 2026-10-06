//! 流程引擎的测试。
//!
//! 前半部分是第二期研究式起草的流程测试，原样改为经引擎跑内置技能（第 ③ 期的验收底线：
//! 结果不变）；后半部分测引擎本身：工具步骤、条件、逐项、挂起与续跑、润色技能。

use super::*;
use crate::agent::backend::{Completion, ModelBackend, ModelRole};
use crate::agent::clarify::{Question, Target};
use crate::agent::gaps::GapStatus;
use crate::agent::skill::{Skill, builtin, builtin_research_draft, parse};
use crate::agent::tools::KnowledgeSearch;
use crate::agent::tools::testing::FakeManuscripts;
use crate::lmstudio::StreamDelta;
use crate::models::{AppConfig, DraftInput, TemplateKind, VocabularyEntry};
use crate::rag::RetrievedChunk;
use std::cell::{Cell, RefCell};

/// 第二期的流程入参，测试里照旧用它描述一次起草。
struct ResearchInput {
    draft: DraftInput,
    request: String,
    notes: Vec<String>,
    clarified: bool,
    system_prompt: String,
    time_sources: String,
}

/// 第二期的流程结果形状：动笔前挂起，或跑完交付。
enum ResearchOutcome {
    Clarify(Vec<Question>),
    Done(Box<SkillReport>),
}

type ResearchReport = SkillReport;

fn board_of(input: &ResearchInput) -> Board {
    Board {
        draft: input.draft.clone(),
        request: input.request.clone(),
        notes: input.notes.clone(),
        clarified: input.clarified,
        system_prompt: input.system_prompt.clone(),
        time_sources: input.time_sources.clone(),
        ..Board::default()
    }
}

/// 经引擎从头跑一遍技能流程。
fn run(
    input: &ResearchInput,
    skill: &Skill,
    vocabulary: &[VocabularyEntry],
    model: &dyn ModelBackend,
    kb: &dyn KnowledgeSearch,
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<ResearchOutcome> {
    let config = AppConfig::default();
    let manuscripts = FakeManuscripts { docs: Vec::new() };
    let env = Env {
        config: &config,
        vocabulary,
        kb,
        manuscripts: &manuscripts,
        model,
        skill,
        apis: &Default::default(),
        secrets: &Default::default(),
    };
    let mut board = board_of(input);
    Ok(match super::run(&mut board, &env, 0, emit)? {
        Outcome::Done => ResearchOutcome::Done(Box::new(SkillReport::from_board(&board))),
        Outcome::Suspended(suspension) => ResearchOutcome::Clarify(suspension.questions),
    })
}

/// 按角色与提示词决定回什么。
type Responder = Box<dyn Fn(ModelRole, &str) -> String>;

/// 按提示词内容决定怎么回的假模型，同时记下每次被问了什么。
struct FakeModel {
    respond: Responder,
    calls: RefCell<Vec<(ModelRole, String)>>,
    cancelled: Cell<bool>,
}

impl FakeModel {
    fn new(respond: impl Fn(ModelRole, &str) -> String + 'static) -> Self {
        Self {
            respond: Box::new(respond),
            calls: RefCell::new(Vec::new()),
            cancelled: Cell::new(false),
        }
    }

    fn asked(&self, needle: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|(_, prompt)| prompt.contains(needle))
            .count()
    }
}

impl ModelBackend for FakeModel {
    fn complete(
        &self,
        role: ModelRole,
        _system: &str,
        user: &str,
        on_delta: &mut dyn FnMut(StreamDelta<'_>),
    ) -> anyhow::Result<Completion> {
        self.calls.borrow_mut().push((role, user.to_string()));
        let reply = (self.respond)(role, user);
        on_delta(StreamDelta::Reasoning("想一想"));
        on_delta(StreamDelta::Content(&reply));
        Ok(Completion {
            content: reply,
            truncated: false,
        })
    }

    fn cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

/// 按检索词里的关键词返回片段的假知识库。
struct FakeKb {
    enabled: bool,
    docs: Vec<(&'static str, RetrievedChunk)>,
    queries: RefCell<Vec<String>>,
}

impl KnowledgeSearch for FakeKb {
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
}

fn chunk(id: i64, title: &str, text: &str) -> RetrievedChunk {
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

fn kb(enabled: bool) -> FakeKb {
    FakeKb {
        enabled,
        docs: vec![
            (
                "依据",
                chunk(
                    1,
                    "省森林防火条例",
                    "依据《省森林防火条例》第十条，各地应当建立防火责任制。",
                ),
            ),
            (
                "火灾情况",
                chunk(2, "年度报告", "2025年全省共发生森林火灾12次，均已扑灭。"),
            ),
        ],
        queries: RefCell::new(Vec::new()),
    }
}

fn input(request: &str, clarified: bool) -> ResearchInput {
    ResearchInput {
        draft: DraftInput {
            kind: TemplateKind::PlainDocument,
            title_hint: "冬季森林防火".into(),
            ..DraftInput::default()
        },
        request: request.into(),
        notes: Vec::new(),
        clarified,
        system_prompt: "SYSTEM".into(),
        time_sources: "2026年10月3日".into(),
    }
}

const DRAFT: &str = "# 关于做好冬季森林防火工作的通知\n\n\
一、去年全省共发生森林火灾12次[K2]。\n\n\
二、各地要按照【待核实：上级文件依据】建立责任制。\n\n\
三、请于【待核实：排查完成时限】前完成排查。\n\n\
四、共投入经费500万元。\n";

/// 正常的模型：预研列两个问题，起草给出 DRAFT，补全用证据里的条例名，核验都支持。
fn honest_model() -> FakeModel {
    FakeModel::new(|role, prompt| {
        if prompt.contains("会让整篇方向写错") {
            "无".into()
        } else if prompt.contains("到知识库里查清的问题") {
            "1. 近年火灾情况\n2. 上级防火文件依据".into()
        } else if prompt.contains("需要补全") {
            assert_eq!(role, ModelRole::Draft, "补全要用起草模型");
            "二、各地要按照《省森林防火条例》建立责任制[K1]。".into()
        } else if prompt.contains("具体事实（时间、数字") {
            "支持".into()
        } else if role == ModelRole::Draft {
            DRAFT.into()
        } else {
            "不支持".into()
        }
    })
}

fn run_with(
    input: &ResearchInput,
    skill: &Skill,
    model: &FakeModel,
    kb: &FakeKb,
) -> (anyhow::Result<ResearchOutcome>, Vec<Event>) {
    let mut events = Vec::new();
    let outcome = run(input, skill, &[], model, kb, &mut |event| {
        events.push(event)
    });
    (outcome, events)
}

fn done(outcome: anyhow::Result<ResearchOutcome>) -> ResearchReport {
    match outcome.expect("流程应当跑完") {
        ResearchOutcome::Done(report) => *report,
        ResearchOutcome::Clarify(questions) => panic!("不该停下来问：{questions:?}"),
    }
}

fn tool_lines(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Tool(tool) => Some(tool.summary.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_mismatched_kind_is_asked_before_writing() {
    let model = honest_model();
    let mut request = input("起草一份商洽函", false);
    request.draft.kind = TemplateKind::PlainDocument;
    let (outcome, events) = run_with(&request, &builtin_research_draft(), &model, &kb(true));
    let ResearchOutcome::Clarify(questions) = outcome.unwrap() else {
        panic!("文种对不上应当先问");
    };
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].target, Target::PreDraft);
    assert!(questions[0].text.contains("公函"), "{}", questions[0].text);
    assert_eq!(model.asked("会让整篇方向写错"), 1, "模型也判断过一次");
    assert_eq!(model.asked("到知识库里查清的问题"), 0, "问完才往下走");
    assert!(
        tool_lines(&events)
            .iter()
            .any(|line| line.contains("1 个问题"))
    );
}

#[test]
fn model_questions_are_asked_when_the_request_is_unclear() {
    let model = FakeModel::new(|_, prompt| {
        if prompt.contains("会让整篇方向写错") {
            "受文对象是谁｜各区县政府｜市直各部门".into()
        } else {
            DRAFT.into()
        }
    });
    let (outcome, _) = run_with(
        &input("写个防火的", false),
        &builtin_research_draft(),
        &model,
        &kb(true),
    );
    let ResearchOutcome::Clarify(questions) = outcome.unwrap() else {
        panic!("模型出了题就该先问");
    };
    assert_eq!(questions[0].text, "受文对象是谁？");
    assert_eq!(questions[0].choices.len(), 2);
}

#[test]
fn the_full_loop_fills_what_the_knowledge_base_can_answer_and_asks_the_rest() {
    let model = honest_model();
    let kb = kb(true);
    // 原话里带「依据」，第一次检索就拿到条例，编号 K1；火灾情况是 K2。
    let (outcome, events) = run_with(
        &input("起草冬季森林防火通知，依据上级要求", true),
        &builtin_research_draft(),
        &model,
        &kb,
    );
    let report = done(outcome);

    // 预研：原话 + 模型列的两个问题，都检索过。
    let queries = kb.queries.borrow().clone();
    assert!(
        queries
            .iter()
            .any(|q| q == "起草冬季森林防火通知，依据上级要求")
    );
    assert!(queries.iter().any(|q| q == "近年火灾情况"));
    assert!(queries.iter().any(|q| q == "上级防火文件依据"));
    assert_eq!(report.evidence.items().len(), 2);
    assert_eq!(report.evidence.get(1).unwrap().doc_title, "省森林防火条例");
    // 起草时证据带着编号和引用规则交给了模型。
    assert!(
        model
            .calls
            .borrow()
            .iter()
            .any(|(role, prompt)| *role == ModelRole::Draft
                && prompt.contains("【知识库证据与引用规则】")
                && prompt.contains("[K1]")
                && prompt.contains("[K2]"))
    );

    // 可检索的缺口用证据补上了，引用标记交付前剥掉。
    assert!(
        report
            .markdown
            .contains("按照《省森林防火条例》建立责任制。"),
        "{}",
        report.markdown
    );
    assert!(!report.markdown.contains("[K"), "{}", report.markdown);
    assert!(!report.markdown.contains("上级文件依据"));
    // 只有用户知道的时限不去检索。
    assert!(
        !queries.iter().any(|q| q.contains("排查完成时限")),
        "{queries:?}"
    );
    assert!(report.markdown.contains("【待核实：排查完成时限】"));

    let status = |hint: &str| {
        report
            .ledger
            .gaps
            .iter()
            .find(|gap| gap.hint == hint)
            .map(|gap| gap.status.clone())
            .unwrap_or_else(|| panic!("台账里没有「{hint}」"))
    };
    assert_eq!(status("上级文件依据"), GapStatus::Resolved(vec![1]));
    assert_eq!(status("排查完成时限"), GapStatus::Open);
    // 500万元：材料和知识库里都没有，检索两次后判无答案。
    assert_eq!(status("500万元"), GapStatus::NoAnswer);
    // 12次：证据里有，不算来源不明。
    assert!(
        report
            .ledger
            .gaps
            .iter()
            .all(|gap| !gap.hint.contains("12次"))
    );

    // 出题：时限（只有你知道）+ 经费（来源不明）。
    let texts: Vec<_> = report.questions.iter().map(|q| q.text.clone()).collect();
    assert_eq!(report.questions.len(), 2, "{texts:?}");
    assert!(texts[0].contains("排查完成时限"));
    assert!(texts[1].contains("500万元"));

    // 两句带引用的话都核验过。
    assert_eq!(model.asked("具体事实（时间、数字"), 2);
    let lines = tool_lines(&events);
    assert!(
        lines.iter().any(|l| l.starts_with("找到 3 处缺口")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("补全「上级文件依据」（来源 [K1]）")),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("还有 2 处需要你确认")),
        "{lines:?}"
    );
    assert!(report.rounds >= 1);
}

#[test]
fn a_revision_that_invents_facts_is_rejected() {
    let model = FakeModel::new(|role, prompt| {
        if prompt.contains("需要补全") {
            // 证据里没有 2024 年。
            "二、各地要按照2024年《省森林防火条例》建立责任制[K1]。".into()
        } else if prompt.contains("到知识库里查清的问题") {
            "无".into()
        } else if role == ModelRole::Draft {
            "# 通知\n\n二、各地要按照【待核实：上级文件依据】建立责任制。\n".into()
        } else {
            "支持".into()
        }
    });
    let (outcome, events) = run_with(
        &input("起草冬季森林防火通知", true),
        &builtin_research_draft(),
        &model,
        &kb(true),
    );
    let report = done(outcome);
    assert!(
        report.markdown.contains("【待核实：上级文件依据】"),
        "原句不动"
    );
    assert_eq!(report.ledger.gaps[0].status, GapStatus::NoAnswer);
    assert_eq!(report.questions.len(), 1);
    assert!(
        tool_lines(&events)
            .iter()
            .any(|l| l.contains("新写的「2024") && l.contains("证据里没有")),
        "{:?}",
        tool_lines(&events)
    );
}

#[test]
fn without_a_knowledge_base_every_gap_goes_to_the_user() {
    let model = honest_model();
    let kb = kb(false);
    let (outcome, events) = run_with(
        &input("起草冬季森林防火通知", true),
        &builtin_research_draft(),
        &model,
        &kb,
    );
    let report = done(outcome);
    assert!(kb.queries.borrow().is_empty());
    assert_eq!(model.asked("到知识库里查清的问题"), 0);
    assert_eq!(model.asked("需要补全"), 0);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Note(n) if n.contains("知识库未启用")))
    );
    // 依据、时限、经费、12次（没有证据可溯源了）全部交给用户，一批最多 4 题。
    assert_eq!(report.questions.len(), 4);
}

#[test]
fn an_unsupported_citation_is_stripped_and_only_untraceable_facts_are_asked() {
    // 第一句的事实在证据包里（K2），只是标成了 K1；第二句的 15 次哪里都没有。
    let model = FakeModel::new(|role, prompt| {
        if prompt.contains("具体事实（时间、数字") {
            "不支持".into()
        } else if prompt.contains("到知识库里查清的问题") {
            "近年火灾情况".into()
        } else if role == ModelRole::Draft {
            "# 通知

一、全省共发生森林火灾12次[K1]。

二、今年已发生15次[K1]。
"
            .into()
        } else {
            "无".into()
        }
    });
    let mut kb = kb(true);
    kb.docs = vec![
        ("火灾情况", chunk(9, "别的报告", "防火工作要常抓不懈。")),
        (
            "火灾情况",
            chunk(2, "年度报告", "2025年全省共发生森林火灾12次。"),
        ),
    ];
    let (outcome, events) = run_with(
        &input("起草冬季森林防火通知", true),
        &builtin_research_draft(),
        &model,
        &kb,
    );
    let report = done(outcome);
    assert!(
        report.markdown.contains("火灾12次。"),
        "引用去掉了，句子保留"
    );
    assert!(
        report.ledger.gaps.iter().all(|g| g.hint != "12次"),
        "12次在证据包里有，只是编号标错，不该出题"
    );
    let gap = report
        .ledger
        .gaps
        .iter()
        .find(|g| g.hint == "15次")
        .unwrap();
    assert_eq!(gap.status, GapStatus::NoAnswer);
    assert!(report.questions.iter().any(|q| q.text.contains("15次")));
    let lines = tool_lines(&events);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("核验不通过") && l.contains("已去掉引用")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("核验不通过") && l.contains("交你确认")),
        "{lines:?}"
    );
}

#[test]
fn stopping_ends_the_run_between_steps() {
    let model = honest_model();
    model.cancelled.set(true);
    let (outcome, _) = run_with(
        &input("起草冬季森林防火通知", true),
        &builtin_research_draft(),
        &model,
        &kb(true),
    );
    assert!(format!("{:#}", outcome.err().unwrap()).contains("已停止"));
    assert!(model.calls.borrow().is_empty());
}

#[test]
fn skill_parameters_bound_the_loop() {
    // 只给一轮、每个缺口只查一次：500万元一次没搜到就直接判无答案。
    let skill = parse(
        "test",
        "---\nmax_rounds: 1\nattempts_per_gap: 1\nmax_checks: 0\nresearch_questions: 0\n---\n",
        "测试",
    )
    .unwrap();
    let model = honest_model();
    let kb = kb(true);
    let (outcome, _) = run_with(
        &input("起草冬季森林防火通知", true),
        &skill_with_builtin_steps(skill),
        &model,
        &kb,
    );
    let report = done(outcome);
    assert_eq!(report.rounds, 1);
    assert_eq!(
        model.asked("到知识库里查清的问题"),
        0,
        "research_questions: 0 不预研"
    );
    assert_eq!(
        model.asked("具体事实（时间、数字"),
        0,
        "max_checks: 0 不核验"
    );
    let gap = report
        .ledger
        .gaps
        .iter()
        .find(|g| g.hint == "500万元")
        .unwrap();
    assert_eq!(gap.attempts, 1);
    assert_eq!(gap.status, GapStatus::NoAnswer);
}

/// 测试用：参数取自 `skill`，缺的提示词与流程用内置补齐（与用户覆盖文件的合并规则相同）。
fn skill_with_builtin_steps(skill: Skill) -> Skill {
    skill.merged_with(&builtin_research_draft())
}

/// 连真实模型与真实知识库跑一遍研究式起草。默认忽略；需要：
/// - `GONGWEN_LIVE_LLM_URL` / `GONGWEN_LIVE_LLM_MODEL` / `GONGWEN_LIVE_LLM_KEY`；
/// - `GONGWEN_LIVE_KB_DIR`：放着 `manuscripts.db` **副本**的目录；
/// - `GONGWEN_LIVE_EMBED_URL` / `GONGWEN_LIVE_EMBED_MODEL` / `GONGWEN_LIVE_EMBED_KEY`；
/// - 可选 `GONGWEN_LIVE_RERANK_MODEL`（同一接口的 /rerank）。
#[test]
#[ignore = "需要真实模型与知识库"]
fn live_research_draft() {
    let env = |key: &str| std::env::var(key).unwrap_or_default();
    if env("GONGWEN_LIVE_LLM_URL").is_empty() || env("GONGWEN_LIVE_KB_DIR").is_empty() {
        eprintln!("未设置联机测试的环境变量，跳过");
        return;
    }
    crate::storage::set_test_config_dir(Some(env("GONGWEN_LIVE_KB_DIR").into()));
    let mut config = crate::models::AppConfig::default();
    config.lm_studio.base_url = env("GONGWEN_LIVE_LLM_URL");
    config.lm_studio.model = env("GONGWEN_LIVE_LLM_MODEL");
    config.lm_studio.api_key = env("GONGWEN_LIVE_LLM_KEY");
    config.lm_studio.timeout_seconds = 300;
    let mut rag = crate::models::RagConfig {
        enabled: true,
        ..Default::default()
    };
    rag.embedding.base_url = env("GONGWEN_LIVE_EMBED_URL");
    rag.embedding.model = env("GONGWEN_LIVE_EMBED_MODEL");
    rag.embedding.api_key = env("GONGWEN_LIVE_EMBED_KEY");
    if env("GONGWEN_LIVE_RERANK_MODEL").is_empty() {
        rag.rerank.mode = crate::models::RerankMode::None;
    } else {
        rag.rerank.mode = crate::models::RerankMode::Api;
        rag.rerank.base_url = env("GONGWEN_LIVE_EMBED_URL");
        rag.rerank.api_key = env("GONGWEN_LIVE_EMBED_KEY");
        rag.rerank.model = env("GONGWEN_LIVE_RERANK_MODEL");
    }
    let kb = crate::agent::tools::RagSearch {
        enabled: true,
        rag,
        chat: config.lm_studio.clone(),
        kind_filter: None,
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = crate::agent::backend::LmBackend::new(&config, cancel);
    let time = crate::prompt::TimeContext::now();
    let mut research = ResearchInput {
        draft: DraftInput {
            kind: TemplateKind::PlainDocument,
            title_hint: "人工智能辅助决策应用情况调研".into(),
            ..DraftInput::default()
        },
        request: "起草一份通知，部署各单位开展人工智能辅助决策系统应用情况调研。\
                  通知里简要介绍美军梅文（Maven）项目的做法和教训作为参考，\
                  要求各单位按时报送调研报告。"
            .into(),
        notes: Vec::new(),
        clarified: false,
        system_prompt: crate::prompt::build_system_prompt(&time),
        time_sources: format!("{} {}", time.today, time.now),
    };
    let skill = builtin_research_draft();
    let started = std::time::Instant::now();
    let mut print = |event: Event| match event {
        Event::Tool(tool) => eprintln!(
            "[{:>5.1}s] {}",
            started.elapsed().as_secs_f32(),
            tool.line()
        ),
        Event::Note(note) => eprintln!("        · {note}"),
        _ => {}
    };
    let outcome = run(&research, &skill, &[], &model, &kb, &mut print).unwrap();
    let report = match outcome {
        ResearchOutcome::Clarify(questions) => {
            eprintln!("—— 动笔前的问题 ——");
            for question in &questions {
                let labels: Vec<_> = question.choices.iter().map(|c| c.label.as_str()).collect();
                eprintln!("  {} {:?}", question.text, labels);
            }
            // 每题选第一个选项，再跑一次。
            let replies: Vec<_> = questions
                .iter()
                .map(|q| (q.id, crate::agent::clarify::Reply::Choice(0)))
                .collect();
            let (_, notes) =
                crate::agent::clarify::resolve_predraft(&questions, &replies, research.draft.kind);
            research.notes = notes;
            research.clarified = true;
            match run(&research, &skill, &[], &model, &kb, &mut print).unwrap() {
                ResearchOutcome::Done(report) => *report,
                ResearchOutcome::Clarify(_) => panic!("问过一次不该再问"),
            }
        }
        ResearchOutcome::Done(report) => *report,
    };
    eprintln!(
        "—— 证据 {} 段，{} 轮，用时 {:?} ——",
        report.evidence.items().len(),
        report.rounds,
        started.elapsed()
    );
    for gap in &report.ledger.gaps {
        eprintln!(
            "  缺口 [{}] {} → {:?}（检索 {} 次）",
            gap.kind.label(),
            gap.hint,
            gap.status,
            gap.attempts
        );
    }
    for question in &report.questions {
        eprintln!("  题：{}", question.text);
    }
    eprintln!("—— 工作稿 ——\n{}", report.markdown);
    assert!(!report.markdown.trim().is_empty());
}

// —— 引擎本身 ——

/// 在给定黑板上从第 `start` 步跑技能，返回 (结果, 事件, 跑完的黑板)。
fn run_board(
    mut board: Board,
    skill: &Skill,
    model: &FakeModel,
    kb: &FakeKb,
    start: usize,
) -> (anyhow::Result<Outcome>, Vec<Event>, Board) {
    let config = AppConfig::default();
    let manuscripts = FakeManuscripts { docs: Vec::new() };
    let env = Env {
        config: &config,
        vocabulary: &[],
        kb,
        manuscripts: &manuscripts,
        model,
        skill,
        apis: &Default::default(),
        secrets: &Default::default(),
    };
    let mut events = Vec::new();
    let outcome = super::run(&mut board, &env, start, &mut |event| events.push(event));
    (outcome, events, board)
}

fn notes(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Note(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn silent_model() -> FakeModel {
    FakeModel::new(|_, _| "无".into())
}

#[test]
fn tool_steps_save_variables_absorb_evidence_and_respect_conditions() {
    let skill = parse(
        "t",
        "---\nname: 测试\ntools: [kb.search, doc.stats, note]\nflow:\n  - tool: kb.search\n    args: { query: \"{request}\" }\n    save_as: hits\n  - tool: doc.stats\n    when: has_text\n    save_as: stats\n  - tool: note\n    args: { text: \"查到：{hits}\" }\n    when: { var: hits }\n  - tool: note\n    args: { text: \"不该出现\" }\n    when: { not: has_sources }\n---\n",
        "测试",
    )
    .unwrap();
    let board = Board {
        request: "起草通知，依据上级要求".into(),
        ..Board::default()
    };
    let (outcome, events, board) = run_board(board, &skill, &silent_model(), &kb(true), 0);
    assert!(matches!(outcome.unwrap(), Outcome::Done));
    assert_eq!(board.evidence.items().len(), 1, "检索结果默认并入证据包");
    assert_eq!(board.evidence.items()[0].query, "起草通知，依据上级要求");
    assert_eq!(board.vars["hits"].as_array().unwrap().len(), 1);
    assert!(
        !board.vars.contains_key("stats"),
        "正文为空，has_text 不满足就跳过"
    );
    let notes = notes(&events);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].starts_with("查到：1. 省森林防火条例："),
        "{notes:?}"
    );
    assert!(
        tool_lines(&events)
            .iter()
            .any(|line| line.contains("检索知识库「")),
        "工具调用在任务流里留一行"
    );
}

#[test]
fn a_failing_tool_leaves_a_note_and_the_flow_goes_on() {
    let skill = parse(
        "t",
        "---\nname: 测试\ntools: [kb.search, note]\nflow:\n  - tool: kb.search\n    args: { query: 防火 }\n    save_as: hits\n  - tool: note\n    args: { text: 接着走 }\n---\n",
        "测试",
    )
    .unwrap();
    let (outcome, events, board) =
        run_board(Board::default(), &skill, &silent_model(), &kb(false), 0);
    assert!(matches!(outcome.unwrap(), Outcome::Done));
    let notes = notes(&events);
    assert!(
        notes[0].contains("工具 kb.search没有做成：知识库未启用"),
        "{notes:?}"
    );
    assert_eq!(notes[1], "接着走");
    assert!(!board.vars.contains_key("hits"));
}

#[test]
fn for_each_runs_the_body_per_item_and_refuses_to_ask_inside() {
    let skill = parse(
        "t",
        "---\nname: 测试\ntools: [note, ask.choice]\nflow:\n  - step: for_each\n    over: sections\n    as: section\n    do:\n      - tool: note\n        args: { text: \"第{index}项：{section}\" }\n---\n",
        "测试",
    )
    .unwrap();
    let mut board = Board::default();
    board.vars.insert(
        "sections".into(),
        serde_json::json!(["总体要求", "工作安排"]),
    );
    let (outcome, events, _) = run_board(board.clone(), &skill, &silent_model(), &kb(true), 0);
    assert!(matches!(outcome.unwrap(), Outcome::Done));
    assert_eq!(notes(&events), ["第1项：总体要求", "第2项：工作安排"]);

    let asking = parse(
        "t",
        "---\nname: 测试\ntools: [ask.choice]\nflow:\n  - step: for_each\n    over: sections\n    do:\n      - tool: ask.choice\n        args: { question: 要吗？, options: [要, 不要] }\n---\n",
        "测试",
    )
    .unwrap();
    let (outcome, _, _) = run_board(board, &asking, &silent_model(), &kb(true), 0);
    assert!(format!("{:#}", outcome.unwrap_err()).contains("不能停下来问用户"));
}

#[test]
fn a_choice_suspends_and_the_answer_resumes_from_the_next_step() {
    let skill = parse(
        "t",
        "---\nname: 测试\ntools: [ask.choice, note]\nflow:\n  - step: ask\n    choose_from: candidates\n    question: 照哪篇写？\n    save_as: baseline_id\n  - tool: note\n    args: { text: \"基准稿 {baseline_id}\" }\n---\n",
        "测试",
    )
    .unwrap();
    let mut board = Board::default();
    board.vars.insert(
        "candidates".into(),
        serde_json::json!([{"id": 7, "title": "去年的通知"}, {"id": 9, "title": "前年的通知"}]),
    );
    let (outcome, events, mut board) = run_board(board, &skill, &silent_model(), &kb(true), 0);
    let Outcome::Suspended(suspension) = outcome.unwrap() else {
        panic!("出了选择题就该挂起");
    };
    assert_eq!(suspension.resume_at, 1);
    assert_eq!(suspension.save_as.as_deref(), Some("baseline_id"));
    assert_eq!(suspension.questions[0].target, Target::Pick);
    assert_eq!(suspension.questions[0].text, "照哪篇写？");
    assert!(notes(&events).is_empty(), "挂起后不往下跑");

    let kind = apply_answers(
        &mut board,
        &suspension,
        &[(1, crate::agent::clarify::Reply::Choice(1))],
    );
    assert_eq!(kind, None);
    assert_eq!(board.vars["baseline_id"], 9);
    let (outcome, events, _) = run_board(
        board,
        &skill,
        &silent_model(),
        &kb(true),
        suspension.resume_at,
    );
    assert!(matches!(outcome.unwrap(), Outcome::Done));
    assert_eq!(notes(&events), ["基准稿 9"]);
}

#[test]
fn predraft_answers_become_notes_and_the_flow_continues_without_asking_again() {
    let model = honest_model();
    let skill = builtin_research_draft();
    let mut request = input("起草一份商洽函", false);
    request.draft.kind = TemplateKind::PlainDocument;
    let (outcome, _, mut board) = run_board(board_of(&request), &skill, &model, &kb(true), 0);
    let Outcome::Suspended(suspension) = outcome.unwrap() else {
        panic!("文种对不上应当先问");
    };
    assert_eq!(suspension.resume_at, 1, "答完从预研接着跑");
    let kind = apply_answers(
        &mut board,
        &suspension,
        &[(1, crate::agent::clarify::Reply::Choice(0))],
    );
    assert_eq!(
        kind,
        Some(TemplateKind::OfficialLetter),
        "切文种交回界面线程执行"
    );
    assert!(board.clarified);
    // 界面线程切完文种，带着新要素接着跑。
    board.draft.kind = TemplateKind::OfficialLetter;
    let (outcome, _, board) = run_board(board, &skill, &model, &kb(true), suspension.resume_at);
    assert!(matches!(outcome.unwrap(), Outcome::Done));
    assert_eq!(model.asked("会让整篇方向写错"), 1, "问过的不再问");
    assert!(model.asked("到知识库里查清的问题") >= 1);
    assert!(!board.workspace.is_empty());
}

#[test]
fn polish_runs_through_the_engine_with_the_selection_pinned() {
    let model = FakeModel::new(|_, _| "# 标题\n\n一、甲。\n\n二、乙改。\n".into());
    let skill = builtin(crate::agent::skill::POLISH).unwrap();
    let document = "# 标题\n\n一、甲。\n\n二、乙。\n".to_string();
    let board = Board {
        request: "压缩".into(),
        preset: "精简篇幅".into(),
        selection: Some("二、乙。".into()),
        workspace: document.clone(),
        document,
        system_prompt: "SYSTEM".into(),
        ..Board::default()
    };
    let (outcome, events, board) = run_board(board, &skill, &model, &kb(true), 0);
    assert!(matches!(outcome.unwrap(), Outcome::Done));
    let calls = model.calls.borrow();
    assert_eq!(calls.len(), 1);
    let (role, prompt) = &calls[0];
    assert_eq!(*role, ModelRole::Draft);
    assert!(prompt.contains("精简篇幅\n\n压缩"), "{prompt}");
    assert!(
        prompt.contains("【唯一允许修改的原文片段】\n二、乙。"),
        "{prompt}"
    );
    assert!(prompt.contains("一、甲。"), "改写要带上现有正文：{prompt}");
    assert_eq!(board.workspace, "# 标题\n\n一、甲。\n\n二、乙改。");
    let begin = events.iter().position(|e| matches!(e, Event::WriteBegin { prefix, suffix } if prefix.is_empty() && suffix.is_empty())).unwrap();
    let content = events
        .iter()
        .position(|e| matches!(e, Event::Content(_)))
        .unwrap();
    assert!(begin < content, "流式增量之前明确写稿目标");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Content(text) if text.contains("乙改"))),
        "改写结果流进界面"
    );
    let report = SkillReport::from_board(&board);
    assert!(report.questions.is_empty() && report.ledger.gaps.is_empty());
}

#[test]
fn rewriting_an_empty_document_is_refused() {
    let skill = builtin(crate::agent::skill::POLISH).unwrap();
    let board = Board {
        request: "压缩".into(),
        ..Board::default()
    };
    let (outcome, _, _) = run_board(board, &skill, &silent_model(), &kb(true), 0);
    assert!(format!("{:#}", outcome.unwrap_err()).contains("正文还是空的"));
}

#[test]
fn conditions_cover_kind_variables_and_negation() {
    use serde_json::json;
    let config = AppConfig::default();
    let manuscripts = FakeManuscripts { docs: Vec::new() };
    let skill = builtin_research_draft();
    let model = silent_model();
    let kb = kb(false);
    let env = Env {
        config: &config,
        vocabulary: &[],
        kb: &kb,
        manuscripts: &manuscripts,
        model: &model,
        skill: &skill,
        apis: &Default::default(),
        secrets: &Default::default(),
    };
    let mut board = Board::default();
    board.draft.kind = TemplateKind::ResearchReport;
    board.vars.insert("empty".into(), json!([]));
    board.vars.insert("hits".into(), json!([1]));
    let holds = |board: &Board, value: serde_json::Value| condition_holds(board, &env, &value);
    assert!(holds(&board, json!({"kind": ["研究报告", "公函"]})));
    assert!(holds(&board, json!({"kind": "ResearchReport"})));
    assert!(!holds(&board, json!({"kind": "公函"})));
    assert!(holds(&board, json!({"var": "hits"})));
    assert!(!holds(&board, json!({"var": "empty"})), "空列表算没有");
    assert!(!holds(&board, json!("has_sources")), "知识库没启用");
    assert!(holds(&board, json!({"not": "has_sources"})));
    assert!(
        !holds(&board, json!(["has_text", {"var": "hits"}])),
        "列表要全部满足"
    );
    assert!(!holds(&board, json!("乱写")), "认不出的条件当不满足");
}

// —— 第 ④ 期验收：数据接口经引擎走完「调用 → 映射 → 证据包 → 引用 → 核验」 ——

#[test]
fn an_api_feeds_evidence_that_is_cited_verified_and_gap_checked() {
    use crate::agent::api::test_server::TestServer;
    use crate::agent::api::{ApiEndpoint, ApiInput, ApiMapping, ApiStore, InputKind};
    let item =
        serde_json::json!({"items": [{"id": 7, "region": "全省", "year": 2025, "count": 12}]})
            .to_string();
    // 预检索一次，缺口「500万元」定向检索两次（第二次搜回的是旧资料，判无答案）。
    let server = TestServer::start(vec![(200, item.clone()), (200, item.clone()), (200, item)]);
    let apis = ApiStore {
        endpoints: vec![ApiEndpoint {
            id: "stat".into(),
            name: "火灾统计".into(),
            url: format!("{}/stat?region={{region}}", server.url),
            inputs: vec![ApiInput {
                name: "region".into(),
                kind: InputKind::Text,
                required: true,
                ..ApiInput::default()
            }],
            mapping: ApiMapping {
                list: "/items".into(),
                title: "{region}{year}年森林火灾统计".into(),
                text: "{region}{year}年共发生森林火灾{count}起".into(),
                source: "省应急厅统计系统".into(),
                ..ApiMapping::default()
            },
            ..ApiEndpoint::default()
        }],
        ..Default::default()
    };
    let skill = parse(
        "data",
        "---\nname: 数据分析段落\ntools: [http.call:stat]\nflow:\n  - step: retrieve\n    apis: [{ api: stat, args: { region: \"{query}\" } }]\n  - step: generate\n    prompt: 起草附加要求\n  - step: gap_loop\n    apis: [stat]\n  - step: verify\n  - step: ask\n---\n## 起草附加要求\n【证据】\n{evidence}\n## 缺口修订\n补「{hint}」：{sentence}\n{evidence}\n## 来源核对\n「{value}」：{sentence}\n{evidence}\n## 核验\n具体事实（时间、数字）：{sentence}\n{evidence}\n",
        "测试",
    )
    .unwrap();
    assert!(
        crate::agent::skill::validate(
            &skill,
            &crate::agent::ops::names(),
            &crate::agent::tools::ids()
        )
        .is_empty()
    );
    let model = FakeModel::new(|role, prompt| {
        if prompt.contains("具体事实（时间、数字）") {
            "支持".into()
        } else if prompt.starts_with("「") {
            "不支持".into()
        } else if role == ModelRole::Draft {
            "# 情况通报\n\n一、2025年全省共发生森林火灾12起[K1]。\n\n二、全年投入扑救经费500万元。\n".into()
        } else {
            "无".into()
        }
    });
    let config = AppConfig::default();
    let manuscripts = FakeManuscripts { docs: Vec::new() };
    let kb = kb(false);
    let env = Env {
        config: &config,
        vocabulary: &[],
        kb: &kb,
        manuscripts: &manuscripts,
        model: &model,
        skill: &skill,
        apis: &apis,
        secrets: &Default::default(),
    };
    let mut board = Board {
        request: "全省".into(),
        system_prompt: "SYSTEM".into(),
        ..Board::default()
    };
    let mut events = Vec::new();
    let outcome = super::run(&mut board, &env, 0, &mut |event| events.push(event)).unwrap();
    assert!(matches!(outcome, Outcome::Done));

    // 调用：检索词填进地址。
    assert!(
        server
            .request(0)
            .starts_with("GET /stat?region=%E5%85%A8%E7%9C%81 "),
        "{}",
        server.request(0)
    );
    // 映射 → 证据包：同一条资料只编一个号。
    let pack = &board.evidence;
    assert_eq!(pack.items().len(), 1);
    let evidence = &pack.items()[0];
    assert_eq!(evidence.key, "http:stat:7");
    assert_eq!(
        evidence.source_label(),
        "《全省2025年森林火灾统计》· 省应急厅统计系统"
    );
    // 引用：起草时证据带编号交给模型。
    assert!(
        model
            .calls
            .borrow()
            .iter()
            .any(|(role, prompt)| *role == ModelRole::Draft
                && prompt.contains("[K1]")
                && prompt.contains("全省2025年共发生森林火灾12起"))
    );
    // 核验：带引用的句子对照接口返回核过一次；12起有出处，不进台账。
    assert_eq!(model.asked("具体事实（时间、数字）"), 1);
    let report = SkillReport::from_board(&board);
    assert!(!report.markdown.contains("[K"), "{}", report.markdown);
    assert!(report.ledger.gaps.iter().all(|gap| gap.hint != "12起"));
    // 接口里查不到的经费：定向检索两次后交用户确认。
    let gap = report
        .ledger
        .gaps
        .iter()
        .find(|gap| gap.hint == "500万元")
        .expect("500万元应当入账");
    assert_eq!(gap.status, GapStatus::NoAnswer);
    assert_eq!(gap.attempts, 2);
    assert!(report.questions.iter().any(|q| q.text.contains("500万元")));
    assert!(
        tool_lines(&events)
            .iter()
            .filter(|line| line.contains("调用接口「火灾统计」→ 1 条"))
            .count()
            == 3,
        "{:?}",
        tool_lines(&events)
    );
}

#[test]
fn undeclared_apis_are_refused_even_inside_operators() {
    use crate::agent::api::{ApiEndpoint, ApiStore};
    let apis = ApiStore {
        endpoints: vec![ApiEndpoint {
            id: "stat".into(),
            name: "火灾统计".into(),
            url: "http://127.0.0.1:9/stat".into(),
            ..ApiEndpoint::default()
        }],
        ..Default::default()
    };
    let skill = parse(
        "t",
        "---\nname: 测试\ntools: [note]\nflow:\n  - step: retrieve\n    apis: [stat]\n---\n",
        "测试",
    )
    .unwrap();
    let config = AppConfig::default();
    let manuscripts = FakeManuscripts { docs: Vec::new() };
    let model = silent_model();
    let kb = kb(false);
    let env = Env {
        config: &config,
        vocabulary: &[],
        kb: &kb,
        manuscripts: &manuscripts,
        model: &model,
        skill: &skill,
        apis: &apis,
        secrets: &Default::default(),
    };
    let mut board = Board {
        request: "全省".into(),
        ..Board::default()
    };
    let mut events = Vec::new();
    super::run(&mut board, &env, 0, &mut |event| events.push(event)).unwrap();
    let notes = notes(&events);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("没有声明工具「http.call:stat」")),
        "{notes:?}"
    );
    assert!(board.evidence.is_empty());
}
