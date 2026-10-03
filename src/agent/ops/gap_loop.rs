//! `gap_loop`：缺口循环（`docs/ai-agent-workbench.md` 14.3 ④–⑧）。
//!
//! 每轮先对照工作稿刷新台账，再对可检索的缺口逐个定向检索：占位交模型用证据补全（过闸门
//! 才写回），来源不明的事实找出处。没有进展、检索次数用完或满轮数就停。
//!
//! 参数：`rounds`（技能参数 `max_rounds`）、`attempts`（`attempts_per_gap`）、
//! `evidence_chars`、`apis`（除知识库外还查哪些数据接口）；提示词 `fill_prompt`（默认「缺口修订」）、
//! `source_prompt`（默认「来源核对」）。

use super::{
    Flow, assist, check_cancel, fetch_into, has_sources, param, phase, prompt, squash, tool_line,
};
use crate::agent::backend::ModelRole;
use crate::agent::engine::Event;
use crate::agent::evidence;
use crate::agent::gaps::{Gap, GapKind, GapStatus, Ledger, find_placeholders};
use crate::agent::skill::StepSpec;
use crate::agent::tools::{ASSIST_SYSTEM, Permission, ToolCtx};
use crate::ai_guard::FactKind;
use crate::lmstudio::StreamDelta;
use crate::models::VocabularyEntry;

/// 来源不明的事实，字面在证据里出现就认出处的最短长度；更短的交模型按语境核对。
const LITERAL_MATCH_MIN_CHARS: usize = 5;

struct Limits {
    rounds: usize,
    attempts: u8,
    evidence_chars: usize,
}

pub(super) fn gap_loop(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let limits = Limits {
        rounds: param(ctx, step, &["rounds", "max_rounds"], 3, 1..=6),
        attempts: param(ctx, step, &["attempts", "attempts_per_gap"], 2, 1..=4) as u8,
        evidence_chars: param(ctx, step, &["evidence_chars"], 8000, 1000..=40000),
    };
    let vocabulary = ctx.env.vocabulary;
    for round in 1..=limits.rounds {
        let sources = ctx.board.sources_text();
        let board = &mut *ctx.board;
        board.ledger.sync(&board.workspace, &sources, vocabulary);
        if round == 1 {
            let summary = gap_summary(&ctx.board.ledger);
            tool_line(ctx, "check.placeholders", Permission::Check, summary);
        }
        let targets = ctx.board.ledger.retrieval_targets(limits.attempts);
        if targets.is_empty() {
            break;
        }
        ctx.board.rounds = round;
        if !has_sources(ctx, step) {
            for id in targets {
                if let Some(gap) = ctx.board.ledger.get_mut(id) {
                    gap.status = GapStatus::NoAnswer;
                }
            }
            break;
        }
        phase(
            ctx,
            format!("第 {round} 轮：围绕 {} 处缺口检索…", targets.len()),
        );
        let mut resolved = 0;
        for id in targets {
            check_cancel(ctx)?;
            if fill_gap(ctx, step, id, round, &limits)? {
                resolved += 1;
            }
        }
        tool_line(
            ctx,
            "gap_loop",
            Permission::Check,
            format!("第 {round} 轮补全 {resolved} 处"),
        );
    }
    let sources = ctx.board.sources_text();
    let board = &mut *ctx.board;
    board.ledger.sync(&board.workspace, &sources, vocabulary);
    Ok(Flow::Next)
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
fn fill_gap(
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    id: usize,
    round: usize,
    limits: &Limits,
) -> anyhow::Result<bool> {
    let Some(gap) = ctx.board.ledger.get(id).cloned() else {
        return Ok(false);
    };
    let attempt = gap.attempts + 1;
    let query = gap_query(&gap, attempt, &ctx.board.draft.title_hint);
    phase(ctx, format!("第 {round} 轮 · 查「{}」…", gap.hint));
    let found = fetch_into(ctx, step, &query);
    let fresh = found.iter().any(|(key, _)| !gap.seen_chunks.contains(key));
    let ids: Vec<usize> = found.iter().map(|(_, id)| *id).collect();
    let last_try = attempt >= limits.attempts;
    {
        let entry = ctx.board.ledger.get_mut(id).expect("刚取过");
        entry.attempts = attempt;
        entry.queries.push(query);
        entry
            .seen_chunks
            .extend(found.into_iter().map(|(key, _)| key));
    }
    let give_up = |ctx: &mut ToolCtx<'_, '_>, why: &str| {
        if last_try {
            if let Some(entry) = ctx.board.ledger.get_mut(id) {
                entry.status = GapStatus::NoAnswer;
            }
            tool_line(
                ctx,
                "check.placeholders",
                Permission::Check,
                format!("「{}」{why}，留给你确认", gap.hint),
            );
        }
    };
    // 什么都没搜到，或搜回来的全是看过的旧片段：这个检索词到头了。
    if ids.is_empty() || (!fresh && attempt > 1) {
        give_up(ctx, "知识库里没有答案");
        return Ok(false);
    }

    if gap.kind == GapKind::Untraced {
        return match confirm_source(ctx, step, &gap, &ids)? {
            Some(source) => {
                if let Some(entry) = ctx.board.ledger.get_mut(id) {
                    entry.status = GapStatus::Resolved(vec![source]);
                }
                tool_line(
                    ctx,
                    "check.facts",
                    Permission::Check,
                    format!("「{}」出处找到了：[K{source}]", gap.hint),
                );
                Ok(true)
            }
            None => {
                give_up(ctx, "找不到出处");
                Ok(false)
            }
        };
    }

    let evidence_text = ctx.board.evidence.format(&ids, limits.evidence_chars / 2);
    let locals = [
        ("hint", gap.hint.clone()),
        ("sentence", gap.sentence.clone()),
        ("evidence", evidence_text),
    ];
    let text = prompt(ctx, step, "fill_prompt", "缺口修订", &locals)?;
    let emit = &mut *ctx.emit;
    let reply = ctx
        .env
        .model
        .complete(ModelRole::Draft, ASSIST_SYSTEM, &text, &mut |delta| {
            if let StreamDelta::Reasoning(text) = delta {
                emit(Event::Reasoning(text.to_string()));
            }
        })?
        .content;
    let support = ctx.board.evidence.text_of(&ids);
    match gate_revision(&gap, &reply, &support, ctx.env.vocabulary) {
        Ok(revised) => {
            let workspace = &mut ctx.board.workspace;
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
            if let Some(entry) = ctx.board.ledger.get_mut(id) {
                entry.status = GapStatus::Resolved(sources);
                entry.sentence = revised;
            }
            tool_line(
                ctx,
                "ws.replace",
                Permission::WriteWorkspace,
                format!("补全「{}」（来源 {label}）", gap.hint),
            );
            (ctx.emit)(Event::Workspace(ctx.board.workspace.clone()));
            Ok(true)
        }
        Err(why) => {
            tool_line(
                ctx,
                "ws.replace",
                Permission::WriteWorkspace,
                format!("「{}」没补上：{why}", gap.hint),
            );
            give_up(ctx, "知识库里没有可用的答案");
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
    let squashed = squash(evidence_text);
    for fact in crate::ai_guard::extract_key_facts(&after, vocabulary) {
        if known.contains(&fact)
            || !matches!(
                fact.kind,
                FactKind::DateTime | FactKind::Number | FactKind::Document
            )
        {
            continue;
        }
        if !squashed.contains(&squash(&fact.value)) {
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
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    gap: &Gap,
    ids: &[usize],
) -> anyhow::Result<Option<usize>> {
    let value = squash(&gap.literal);
    // 字面撞上只对够长的值算数：「23日」「2个」这种随便哪段都可能有，得让模型看语境。
    if value.chars().count() >= LITERAL_MATCH_MIN_CHARS
        && let Some(id) = ids.iter().find(|id| {
            ctx.board
                .evidence
                .get(**id)
                .is_some_and(|e| squash(&e.text).contains(&value))
        })
    {
        return Ok(Some(*id));
    }
    let locals = [
        ("value", gap.literal.clone()),
        ("sentence", evidence::strip_citations(&gap.sentence)),
        ("evidence", ctx.board.evidence.format(ids, 4000)),
    ];
    let text = prompt(ctx, step, "source_prompt", "来源核对", &locals)?;
    let reply = assist(ctx, &text)?;
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
