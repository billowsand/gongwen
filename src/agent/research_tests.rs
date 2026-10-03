//! 研究式起草流程的测试：按脚本回复的假模型 + 假知识库，跑通每条路径。

use super::*;
use crate::agent::backend::Completion;
use crate::agent::clarify::Target;
use crate::agent::skill::{builtin_research_draft, parse};
use crate::models::TemplateKind;
use crate::rag::RetrievedChunk;
use std::cell::{Cell, RefCell};

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
) -> (anyhow::Result<ResearchOutcome>, Vec<ResearchEvent>) {
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

fn tool_lines(events: &[ResearchEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ResearchEvent::Tool(tool) => Some(tool.summary.clone()),
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
            .any(|e| matches!(e, ResearchEvent::Note(n) if n.contains("知识库未启用")))
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

/// 测试用：参数取自 `skill`，缺的步骤用内置补齐。
fn skill_with_builtin_steps(mut skill: Skill) -> Skill {
    let builtin = builtin_research_draft();
    for (key, value) in builtin.sections {
        skill.sections.entry(key).or_insert(value);
    }
    skill
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
    let mut print = |event: ResearchEvent| match event {
        ResearchEvent::Tool(tool) => eprintln!(
            "[{:>5.1}s] {}",
            started.elapsed().as_secs_f32(),
            tool.line()
        ),
        ResearchEvent::Note(note) => eprintln!("        · {note}"),
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
            let (_, notes) = crate::agent::clarify::resolve_predraft(&questions, &replies);
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
