//! 研究式起草：先查后写，围绕缺口反复检索、局部补全，最后核验引用、出题问用户。
//!
//! 流程（`docs/ai-agent-workbench.md` 14.3）：
//!
//! ```text
//! ⓪ 动笔前澄清 → ① 预研列问题 → ② 多路检索成证据包 → ③ 带证据起草
//! → ④–⑧ 缺口循环（识别 → 定向检索 → 局部补全，没进展或满轮数就停）
//! → ⑨ 核验引用 → ⑩ 出题
//! ```
//!
//! 模型与知识库都经接口注入，测试用按脚本回复的假模型跑通全部路径。流程只产出工作稿，
//! 定稿成提案、交用户接受由调用方负责。

use super::backend::{ModelBackend, ModelRole};
use super::clarify::{self, Question};
use super::evidence::{self, EvidencePack};
use super::gaps::{Gap, GapKind, GapStatus, Ledger, find_placeholders};
use super::skill::Skill;
use super::tools::{KnowledgeSearch, Permission, ToolUse};
use crate::ai_guard::FactKind;
use crate::lmstudio::StreamDelta;
use crate::models::{DraftInput, VocabularyEntry};

/// 来源不明的事实，字面在证据里出现就认出处的最短长度；更短的交模型按语境核对。
const LITERAL_MATCH_MIN_CHARS: usize = 5;

/// 辅助步骤（列问题、核对、出题）的系统提示。
const ASSIST_SYSTEM: &str =
    "你是公文写作的辅助判断程序。严格按要求的格式输出，不解释、不寒暄、不加 Markdown。";

/// 一次研究式起草的输入。
pub(crate) struct ResearchInput {
    pub(crate) draft: DraftInput,
    /// 用户原话：材料与写作要求。
    pub(crate) request: String,
    /// 动笔前澄清的回答，作为已确认信息交给起草。
    pub(crate) notes: Vec<String>,
    /// 已经问过动笔前澄清（第二次进来时跳过）。
    pub(crate) clarified: bool,
    /// 起草的系统提示（含日期规则）。
    pub(crate) system_prompt: String,
    /// 今天、明天这些日期，算作合法出处，不然年份会被当成来源不明。
    pub(crate) time_sources: String,
}

/// 流程向界面报告的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResearchEvent {
    Tool(ToolUse),
    /// 阶段提示，会被下一阶段冲掉。
    Phase(String),
    /// 起草正文的增量。
    Content(String),
    Reasoning(String),
    /// 工作稿整体换新（补全之后）。
    Workspace(String),
    Note(String),
}

/// 研究式起草的结果。
#[derive(Debug, Clone)]
pub(crate) struct ResearchReport {
    /// 剥掉引用标记的工作稿，交给定稿。
    pub(crate) markdown: String,
    pub(crate) ledger: Ledger,
    pub(crate) evidence: EvidencePack,
    /// 第一稿之后要问用户的题。
    pub(crate) questions: Vec<Question>,
    pub(crate) rounds: usize,
    pub(crate) truncated: bool,
}

pub(crate) enum ResearchOutcome {
    /// 动笔前有题要问，流程到此为止，等回答后带着 `clarified` 重来。
    Clarify(Vec<Question>),
    Done(Box<ResearchReport>),
}

/// 从技能文件读出的流程参数。
struct Params {
    max_rounds: usize,
    research_questions: usize,
    pre_questions: usize,
    batch_questions: usize,
    attempts_per_gap: u8,
    evidence_chars: usize,
    max_checks: usize,
}

impl Params {
    fn from(skill: &Skill) -> Self {
        Self {
            max_rounds: skill.param_usize("max_rounds", 3, 1..=6),
            research_questions: skill.param_usize("research_questions", 6, 0..=12),
            pre_questions: skill.param_usize("pre_questions", 3, 0..=5),
            batch_questions: skill.param_usize("batch_questions", 4, 1..=10),
            attempts_per_gap: skill.param_usize("attempts_per_gap", 2, 1..=4) as u8,
            evidence_chars: skill.param_usize("evidence_chars", 8000, 1000..=40000),
            max_checks: skill.param_usize("max_checks", 8, 0..=30),
        }
    }
}

pub(crate) fn run(
    input: &ResearchInput,
    skill: &Skill,
    vocabulary: &[VocabularyEntry],
    model: &dyn ModelBackend,
    kb: &dyn KnowledgeSearch,
    emit: &mut dyn FnMut(ResearchEvent),
) -> anyhow::Result<ResearchOutcome> {
    let params = Params::from(skill);
    let kind_label = input.draft.kind.label();
    let request = request_with_notes(input);

    // ⓪ 动笔前澄清：程序规则 + 模型判断，只问会让整篇写偏的事。
    if !input.clarified && params.pre_questions > 0 {
        emit(ResearchEvent::Phase("动笔前检查要求是否明确…".into()));
        let prompt = skill
            .render(
                "动笔前澄清",
                &[
                    ("max", &params.pre_questions.to_string()),
                    ("kind", kind_label),
                    ("title", input.draft.title_hint.trim()),
                    ("request", &request),
                ],
            )
            .unwrap_or_default();
        let reply = assist(model, &prompt, emit)?;
        let parsed = clarify::parse_model_questions(&reply, params.pre_questions);
        let questions = clarify::predraft_questions(
            &input.request,
            input.draft.kind,
            parsed,
            params.pre_questions,
        );
        if !questions.is_empty() {
            emit(tool(
                "ask_user",
                Permission::AskUser,
                format!("动笔前有 {} 个问题要你确认", questions.len()),
            ));
            return Ok(ResearchOutcome::Clarify(questions));
        }
    }

    // ①② 预研与多路检索。
    let mut pack = EvidencePack::default();
    if kb.enabled() {
        let mut queries = vec![input.request.trim().to_string()];
        if params.research_questions > 0 {
            emit(ResearchEvent::Phase("预研：列出要查的问题…".into()));
            let prompt = skill
                .render(
                    "预研",
                    &[
                        ("max", &params.research_questions.to_string()),
                        ("kind", kind_label),
                        ("request", &request),
                    ],
                )
                .unwrap_or_default();
            let reply = assist(model, &prompt, emit)?;
            let planned: Vec<String> = reply
                .lines()
                .map(|line| clarify::strip_numbering(line.trim()).trim().to_string())
                .filter(|line| !line.is_empty() && line != "无")
                .take(params.research_questions)
                .collect();
            emit(tool(
                "plan",
                Permission::Read,
                format!("预研列出 {} 个要查的问题", planned.len()),
            ));
            for question in planned {
                if !queries.contains(&question) {
                    queries.push(question);
                }
            }
        }
        for query in queries.iter().filter(|q| !q.is_empty()) {
            check_cancel(model)?;
            search_into(kb, &mut pack, query, emit);
        }
    } else {
        emit(ResearchEvent::Note(
            "知识库未启用：只按材料起草，缺口全部交给你确认。".into(),
        ));
    }

    // ③ 带证据起草。
    check_cancel(model)?;
    emit(ResearchEvent::Phase("起草中…".into()));
    let material = material_block(input);
    let reference = if pack.is_empty() {
        String::new()
    } else {
        let evidence = pack.format(&pack.all_ids(), params.evidence_chars);
        format!(
            "\n\n{}",
            skill
                .render("起草附加要求", &[("evidence", &evidence)])
                .unwrap_or(evidence)
        )
    };
    let user = crate::prompt::build_draft_prompt(&input.draft, vocabulary, &material, &reference);
    let completion = model.complete(
        ModelRole::Draft,
        &input.system_prompt,
        &user,
        &mut |delta| match delta {
            StreamDelta::Content(text) => emit(ResearchEvent::Content(text.to_string())),
            StreamDelta::Reasoning(text) => emit(ResearchEvent::Reasoning(text.to_string())),
        },
    )?;
    let truncated = completion.truncated;
    let mut workspace = crate::prompt::sanitize_model_markdown(&completion.content);
    emit(ResearchEvent::Workspace(workspace.clone()));

    // ④–⑧ 缺口循环。
    let mut ledger = Ledger::default();
    let mut rounds = 0;
    for round in 1..=params.max_rounds {
        let sources = sources_text(input, &pack);
        ledger.sync(&workspace, &sources, vocabulary);
        if round == 1 {
            emit(tool("check_gaps", Permission::Check, gap_summary(&ledger)));
        }
        let targets = ledger.retrieval_targets(params.attempts_per_gap);
        if targets.is_empty() {
            break;
        }
        rounds = round;
        if !kb.enabled() {
            for id in targets {
                if let Some(gap) = ledger.get_mut(id) {
                    gap.status = GapStatus::NoAnswer;
                }
            }
            break;
        }
        emit(ResearchEvent::Phase(format!(
            "第 {round} 轮：围绕 {} 处缺口检索…",
            targets.len()
        )));
        let mut resolved = 0;
        for id in targets {
            check_cancel(model)?;
            if fill_gap(
                id,
                round,
                &params,
                input,
                skill,
                vocabulary,
                model,
                kb,
                &mut pack,
                &mut ledger,
                &mut workspace,
                emit,
            )? {
                resolved += 1;
            }
        }
        emit(tool(
            "round",
            Permission::Check,
            format!("第 {round} 轮补全 {resolved} 处"),
        ));
    }
    ledger.sync(&workspace, &sources_text(input, &pack), vocabulary);

    // ⑨ 核验引用。
    if params.max_checks > 0 && !pack.is_empty() {
        verify_citations(
            &params,
            skill,
            model,
            &pack,
            &mut ledger,
            &mut workspace,
            emit,
        )?;
    }

    // ⑩ 出题。
    let questions = clarify::gap_questions(&ledger, params.batch_questions);
    if !questions.is_empty() {
        emit(tool(
            "ask_user",
            Permission::AskUser,
            format!("还有 {} 处需要你确认", questions.len()),
        ));
    }
    Ok(ResearchOutcome::Done(Box::new(ResearchReport {
        markdown: evidence::strip_citations(&workspace),
        ledger,
        evidence: pack,
        questions,
        rounds,
        truncated,
    })))
}

fn tool(name: &'static str, permission: Permission, summary: String) -> ResearchEvent {
    ResearchEvent::Tool(ToolUse::new(name, permission, summary))
}

fn check_cancel(model: &dyn ModelBackend) -> anyhow::Result<()> {
    if model.cancelled() {
        anyhow::bail!("已停止生成");
    }
    Ok(())
}

/// 辅助步骤：思考照样流进界面，正文不流（它是给程序看的）。
fn assist(
    model: &dyn ModelBackend,
    prompt: &str,
    emit: &mut dyn FnMut(ResearchEvent),
) -> anyhow::Result<String> {
    check_cancel(model)?;
    let completion = model.complete(ModelRole::Assist, ASSIST_SYSTEM, prompt, &mut |delta| {
        if let StreamDelta::Reasoning(text) = delta {
            emit(ResearchEvent::Reasoning(text.to_string()));
        }
    })?;
    Ok(completion.content.trim().to_string())
}

fn request_with_notes(input: &ResearchInput) -> String {
    let mut text = input.request.trim().to_string();
    if !input.notes.is_empty() {
        text.push_str("\n\n已确认：");
        for note in &input.notes {
            text.push_str("\n- ");
            text.push_str(note);
        }
    }
    text
}

fn material_block(input: &ResearchInput) -> String {
    let mut text = format!("【原始材料与写作要求】\n{}", input.request.trim());
    if !input.notes.is_empty() {
        text.push_str("\n\n【动笔前已确认——优先于原始材料】");
        for note in &input.notes {
            text.push_str("\n- ");
            text.push_str(note);
        }
    }
    text
}

/// 算作出处的全部文字：材料、已确认的回答、日期、要素、证据包。
fn sources_text(input: &ResearchInput, pack: &EvidencePack) -> String {
    let mut text = String::new();
    text.push_str(&input.request);
    text.push('\n');
    text.push_str(&input.notes.join("\n"));
    text.push('\n');
    text.push_str(&input.time_sources);
    text.push('\n');
    text.push_str(&serde_json::to_string(&input.draft).unwrap_or_default());
    text.push('\n');
    text.push_str(&pack.text_of(&pack.all_ids()));
    text
}

fn search_into(
    kb: &dyn KnowledgeSearch,
    pack: &mut EvidencePack,
    query: &str,
    emit: &mut dyn FnMut(ResearchEvent),
) -> Vec<(i64, usize)> {
    match kb.search(query) {
        Ok((chunks, warnings)) => {
            let before = pack.items().len();
            let ids = pack.absorb(query, &chunks);
            let added = pack.items().len() - before;
            emit(tool(
                "search_knowledge",
                Permission::Read,
                format!(
                    "检索「{}」→ {} 段（新增 {added}）",
                    short(query, 24),
                    chunks.len()
                ),
            ));
            for warning in warnings {
                emit(ResearchEvent::Note(format!("知识库：{warning}")));
            }
            chunks.iter().map(|c| c.chunk_id).zip(ids).collect()
        }
        Err(error) => {
            emit(ResearchEvent::Note(format!("知识库检索失败：{error:#}")));
            Vec::new()
        }
    }
}

/// 缺口的检索词：第一次用所在句（去掉占位与引用）加提示；第二次换成提示加标题，
/// 换个说法再搜一次（CRAG 式改写，用规则，不调模型）。
fn gap_query(gap: &Gap, attempt: u8, title: &str) -> String {
    let sentence = evidence::strip_citations(&gap.sentence);
    let sentence = find_placeholders(&sentence)
        .iter()
        .fold(sentence.clone(), |text, p| text.replace(&p.literal, ""));
    let query = if attempt <= 1 {
        if gap.kind == GapKind::Untraced {
            sentence
        } else {
            format!("{} {}", sentence.trim(), gap.hint)
        }
    } else {
        format!("{} {}", gap.hint, title.trim())
    };
    query.trim().to_string()
}

/// 处理一个缺口：检索、补全或核对出处。补全成功返回 true。
#[allow(clippy::too_many_arguments)]
fn fill_gap(
    id: usize,
    round: usize,
    params: &Params,
    input: &ResearchInput,
    skill: &Skill,
    vocabulary: &[VocabularyEntry],
    model: &dyn ModelBackend,
    kb: &dyn KnowledgeSearch,
    pack: &mut EvidencePack,
    ledger: &mut Ledger,
    workspace: &mut String,
    emit: &mut dyn FnMut(ResearchEvent),
) -> anyhow::Result<bool> {
    let Some(gap) = ledger.get(id).cloned() else {
        return Ok(false);
    };
    let attempt = gap.attempts + 1;
    let query = gap_query(&gap, attempt, &input.draft.title_hint);
    emit(ResearchEvent::Phase(format!(
        "第 {round} 轮 · 查「{}」…",
        gap.hint
    )));
    let found = search_into(kb, pack, &query, emit);
    let fresh = found
        .iter()
        .any(|(chunk, _)| !gap.seen_chunks.contains(chunk));
    let ids: Vec<usize> = found.iter().map(|(_, id)| *id).collect();
    let last_try = attempt >= params.attempts_per_gap;
    {
        let entry = ledger.get_mut(id).expect("刚取过");
        entry.attempts = attempt;
        entry.queries.push(query);
        entry
            .seen_chunks
            .extend(found.iter().map(|(chunk, _)| *chunk));
    }
    let give_up = |ledger: &mut Ledger, emit: &mut dyn FnMut(ResearchEvent), why: &str| {
        if last_try {
            if let Some(entry) = ledger.get_mut(id) {
                entry.status = GapStatus::NoAnswer;
            }
            emit(tool(
                "check_gaps",
                Permission::Check,
                format!("「{}」{why}，留给你确认", gap.hint),
            ));
        }
    };
    // 什么都没搜到，或搜回来的全是看过的旧片段：这个检索词到头了。
    if ids.is_empty() || (!fresh && attempt > 1) {
        give_up(ledger, emit, "知识库里没有答案");
        return Ok(false);
    }

    if gap.kind == GapKind::Untraced {
        return match confirm_source(skill, model, pack, &gap, &ids, emit)? {
            Some(source) => {
                if let Some(entry) = ledger.get_mut(id) {
                    entry.status = GapStatus::Resolved(vec![source]);
                }
                emit(tool(
                    "check_source",
                    Permission::Check,
                    format!("「{}」出处找到了：[K{source}]", gap.hint),
                ));
                Ok(true)
            }
            None => {
                give_up(ledger, emit, "找不到出处");
                Ok(false)
            }
        };
    }

    let evidence_text = pack.format(&ids, params.evidence_chars / 2);
    let prompt = skill
        .render(
            "缺口修订",
            &[
                ("hint", &gap.hint),
                ("sentence", &gap.sentence),
                ("evidence", &evidence_text),
            ],
        )
        .unwrap_or_default();
    let reply = model
        .complete(ModelRole::Draft, ASSIST_SYSTEM, &prompt, &mut |delta| {
            if let StreamDelta::Reasoning(text) = delta {
                emit(ResearchEvent::Reasoning(text.to_string()));
            }
        })?
        .content;
    match gate_revision(&gap, &reply, &pack.text_of(&ids), vocabulary) {
        Ok(revised) => {
            let Some(pos) = workspace.find(&gap.sentence) else {
                return Ok(false);
            };
            workspace.replace_range(pos..pos + gap.sentence.len(), &revised);
            let cited: Vec<usize> = evidence::citation_ids(&revised)
                .into_iter()
                .filter(|cited| ids.contains(cited))
                .collect();
            let sources = if cited.is_empty() { ids.clone() } else { cited };
            let label = sources
                .iter()
                .map(|id| format!("[K{id}]"))
                .collect::<String>();
            if let Some(entry) = ledger.get_mut(id) {
                entry.status = GapStatus::Resolved(sources);
                entry.sentence = revised.clone();
            }
            emit(tool(
                "workspace_edit",
                Permission::WriteWorkspace,
                format!("补全「{}」（来源 {label}）", gap.hint),
            ));
            emit(ResearchEvent::Workspace(workspace.clone()));
            Ok(true)
        }
        Err(why) => {
            emit(tool(
                "workspace_edit",
                Permission::WriteWorkspace,
                format!("「{}」没补上：{why}", gap.hint),
            ));
            give_up(ledger, emit, "知识库里没有可用的答案");
            Ok(false)
        }
    }
}

/// 补全结果的闸门。模型改的只是一句话，过不了就丢，原句不动。
fn gate_revision(
    gap: &Gap,
    reply: &str,
    evidence_text: &str,
    vocabulary: &[VocabularyEntry],
) -> Result<String, String> {
    let revised = reply
        .trim()
        .trim_matches(['「', '」', '“', '”', '"'])
        .trim()
        .to_string();
    if revised.is_empty() || revised.contains("无法补全") {
        return Err("证据回答不了".into());
    }
    if revised.lines().count() > 1 {
        return Err("回了多行，像在解释而不是改写".into());
    }
    if revised.contains(&gap.literal) {
        return Err("占位还在".into());
    }
    let before = evidence::strip_citations(&gap.sentence);
    let after = evidence::strip_citations(&revised);
    // 原样还回来就是没补上，与「无法补全」同义。
    if before.trim() == after.trim() {
        return Err("证据回答不了".into());
    }
    let (before_len, after_len) = (before.chars().count(), after.chars().count());
    if after_len * 2 < before_len || after_len > before_len * 3 + 60 {
        return Err("改动幅度过大".into());
    }
    if markup_counts(&before) != markup_counts(&after) {
        return Err("动了 Markdown 标记".into());
    }
    // 新增的事实必须能在证据里找到——补全只许用证据，不许编。
    let known = crate::ai_guard::extract_key_facts(&before, vocabulary);
    let squashed: String = evidence_text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    for fact in crate::ai_guard::extract_key_facts(&after, vocabulary) {
        if known.contains(&fact)
            || !matches!(
                fact.kind,
                FactKind::DateTime | FactKind::Number | FactKind::Document
            )
        {
            continue;
        }
        let value: String = fact.value.chars().filter(|c| !c.is_whitespace()).collect();
        if !squashed.contains(&value) {
            return Err(format!("新写的「{}」证据里没有", fact.value));
        }
    }
    Ok(revised)
}

fn markup_counts(text: &str) -> Vec<(char, usize)> {
    ['*', '_', '`', '|', '#', '(', ')']
        .iter()
        .map(|ch| (*ch, text.matches(*ch).count()))
        .collect()
}

/// 来源不明的事实：证据原文里有就直接认（程序判断）；说法不同时再问模型。
fn confirm_source(
    skill: &Skill,
    model: &dyn ModelBackend,
    pack: &EvidencePack,
    gap: &Gap,
    ids: &[usize],
    emit: &mut dyn FnMut(ResearchEvent),
) -> anyhow::Result<Option<usize>> {
    let squash = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let value = squash(&gap.literal);
    // 字面撞上只对够长的值算数：「23日」「2个」这种随便哪段都可能有，得让模型看语境。
    if value.chars().count() >= LITERAL_MATCH_MIN_CHARS
        && let Some(id) = ids.iter().find(|id| {
            pack.get(**id)
                .is_some_and(|e| squash(&e.text).contains(&value))
        })
    {
        return Ok(Some(*id));
    }
    let prompt = skill
        .render(
            "来源核对",
            &[
                ("value", &gap.literal),
                ("sentence", &evidence::strip_citations(&gap.sentence)),
                ("evidence", &pack.format(ids, 4000)),
            ],
        )
        .unwrap_or_default();
    let reply = assist(model, &prompt, emit)?;
    if reply.contains("不支持") || !reply.contains("支持") {
        return Ok(None);
    }
    // 「支持 K3」：认模型点名的那段；只给了一段证据时不点名也算它。
    let named = reply
        .split('K')
        .skip(1)
        .filter_map(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<usize>()
                .ok()
        })
        .find(|id| ids.contains(id));
    Ok(named.or_else(|| (ids.len() == 1).then(|| ids[0])))
}

/// 核验：带引用的句子逐句对照引到的片段，不支持的去掉引用，其中的事实交用户确认。
fn verify_citations(
    params: &Params,
    skill: &Skill,
    model: &dyn ModelBackend,
    pack: &EvidencePack,
    ledger: &mut Ledger,
    workspace: &mut String,
    emit: &mut dyn FnMut(ResearchEvent),
) -> anyhow::Result<()> {
    let mut cited = evidence::cited_sentences(workspace);
    // 有具体事实的句子先核：引错了事实比引错了说法要紧。
    cited.sort_by_key(|(sentence, _)| {
        crate::ai_guard::extract_key_facts(&evidence::strip_citations(sentence), &[]).is_empty()
    });
    let mut checked = 0;
    for (sentence, ids) in cited.into_iter().take(params.max_checks) {
        check_cancel(model)?;
        let ids: Vec<usize> = ids
            .into_iter()
            .filter(|id| pack.get(*id).is_some())
            .collect();
        if ids.is_empty() {
            continue;
        }
        emit(ResearchEvent::Phase("核验引用…".into()));
        let plain = evidence::strip_citations(&sentence);
        let prompt = skill
            .render(
                "核验",
                &[("sentence", &plain), ("evidence", &pack.format(&ids, 4000))],
            )
            .unwrap_or_default();
        let reply = assist(model, &prompt, emit)?;
        checked += 1;
        if !reply.contains("不支持") {
            continue;
        }
        if let Some(pos) = workspace.find(&sentence) {
            workspace.replace_range(pos..pos + sentence.len(), &plain);
        }
        // 只有整个证据包里都找不到的事实才交用户：实测里模型常把编号标错（事实出自 K7
        // 却标了 K3），这时事实本身有出处，只是引用对不上，不该拿来打扰用户。
        let squash = |text: &str| {
            text.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };
        let everything = squash(&pack.text_of(&pack.all_ids()));
        let facts: Vec<_> = crate::ai_guard::extract_key_facts(&plain, &[])
            .into_iter()
            .filter(|fact| {
                matches!(
                    fact.kind,
                    FactKind::DateTime | FactKind::Number | FactKind::Document
                ) && !everything.contains(&squash(&fact.value))
            })
            .collect();
        for fact in &facts {
            ledger.add_unsupported(&fact.value, &plain);
        }
        emit(tool(
            "verify_citation",
            Permission::Check,
            format!(
                "核验不通过：「{}」与引用片段不符{}",
                short(&plain, 20),
                if facts.is_empty() {
                    "，已去掉引用"
                } else {
                    "，交你确认"
                }
            ),
        ));
    }
    if checked > 0 {
        emit(ResearchEvent::Workspace(workspace.clone()));
        emit(tool(
            "verify_citation",
            Permission::Check,
            format!("核验了 {checked} 句带引用的话"),
        ));
    }
    Ok(())
}

fn gap_summary(ledger: &Ledger) -> String {
    let count = |kind: GapKind| ledger.gaps.iter().filter(|g| g.kind == kind).count();
    if ledger.gaps.is_empty() {
        return "第一稿没有待核实的缺口".into();
    }
    format!(
        "找到 {} 处缺口：可检索 {}，需你提供 {}，来源不明 {}",
        ledger.gaps.len(),
        count(GapKind::Retrievable),
        count(GapKind::NeedsUser),
        count(GapKind::Untraced)
    )
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
#[path = "research_tests.rs"]
mod tests;
