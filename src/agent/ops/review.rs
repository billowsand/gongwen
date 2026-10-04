//! 审核类算子：只出问题清单，不改稿（决定 F7）。有明确改法的问题带上一处替换，交付后转成
//! 审校抽屉里的修订建议，由用户逐条采纳。
//!
//! - `review`：全面审校。`checks` 选查哪几样：`elements`（要素与格式校验，确定性）、`model`
//!   （模型诊断表述与结构，提示词默认「诊断」）；
//! - `fact_check`：事实核查。抽出正文里的时间、数字、文件，逐条检索知识库与 `apis:` 里的数据接口，
//!   判有出处 / 无出处 / 与资料矛盾（提示词默认「事实核对」）；
//! - `report`：把一个清单变量（如要点）原样列成问题清单。

use super::{Flow, assist, check_cancel, fetch_into, param, phase, prompt, squash, tool_line};
use crate::agent::board::{Finding, Fix};
use crate::agent::gaps::sentence_at;
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx, short};
use crate::ai_guard::FactKind;
use serde_json::Value;
use std::collections::BTreeMap;

/// 发起时的正文（审的就是它）；空稿不审。
fn document(ctx: &ToolCtx<'_, '_>) -> anyhow::Result<String> {
    let text = ctx.board.document.clone();
    if text.trim().is_empty() {
        anyhow::bail!("正文还是空的，没有可审的内容");
    }
    Ok(text)
}

fn checks(step: &StepSpec) -> Vec<String> {
    match step.params.get("checks") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(one)) => vec![one.clone()],
        _ => vec!["elements".into(), "model".into()],
    }
}

/// `review`：全面审校。
pub(super) fn review(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let text = document(ctx)?;
    let checks = checks(step);
    let mut findings = Vec::new();
    if checks.iter().any(|c| c == "elements") {
        for note in crate::draft_page::review_notes(&ctx.board.draft, ctx.env.config, &text) {
            let excerpt = note
                .span
                .as_ref()
                .and_then(|span| text.get(span.clone()))
                .map(|s| short(s.trim(), 40))
                .unwrap_or_default();
            findings.push(Finding {
                group: "要素与格式".into(),
                text: note.message,
                excerpt,
                source: "规则校验".into(),
                fix: None,
            });
        }
        tool_line(
            ctx,
            "check.elements",
            Permission::Check,
            format!("要素与格式校验：{} 个问题", findings.len()),
        );
    }
    if checks.iter().any(|c| c == "model") {
        check_cancel(ctx)?;
        phase(ctx, "通读诊断…");
        let max = param(ctx, step, &["max"], 12, 1..=30);
        // 长稿放不进上下文时按章节分几段诊断（16.15 A.4），改法仍按全文定位。
        let fixed = prompt(
            ctx,
            step,
            "prompt",
            "诊断",
            &[("max", max.to_string()), ("document", String::new())],
        )?;
        let window = ctx
            .env
            .model
            .window(crate::agent::backend::ModelRole::Assist);
        let room = crate::agent::budget::room_chars(
            window.tokens,
            &format!("{}{fixed}", crate::agent::tools::ASSIST_SYSTEM),
        );
        let parts = crate::agent::budget::split_by_budget(&text, room);
        if parts.len() > 1 {
            super::note(
                ctx,
                format!(
                    "正文较长，模型上下文 {} 放不下全文，分 {} 段诊断",
                    window.label(),
                    parts.len()
                ),
            );
        }
        let mut diagnosed = Vec::new();
        for part in &parts {
            check_cancel(ctx)?;
            let locals = [("max", max.to_string()), ("document", part.clone())];
            let reply = assist(ctx, &prompt(ctx, step, "prompt", "诊断", &locals)?)?;
            diagnosed.extend(parse_diagnosis(&reply, &text, ctx.env.vocabulary, max));
        }
        let fixes = diagnosed.iter().filter(|f| f.fix.is_some()).count();
        tool_line(
            ctx,
            "review",
            Permission::Check,
            format!("通读诊断：{} 个问题，{fixes} 处给出改法", diagnosed.len()),
        );
        findings.extend(diagnosed);
    }
    ctx.board.findings.extend(findings);
    Ok(Flow::Next)
}

/// 模型诊断的每行「分组｜问题｜原文片段｜改法」→ 问题。改法只在原文片段在正文里恰好出现一次、
/// 且改后没有动关键事实时才收下，否则只作提示——模型的改法进抽屉之前，闸门得先过一遍。
fn parse_diagnosis(
    reply: &str,
    text: &str,
    vocabulary: &[crate::models::VocabularyEntry],
    max: usize,
) -> Vec<Finding> {
    reply
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "无")
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(['｜', '|']).map(str::trim).collect();
            let (group, problem, excerpt, after) = match parts.as_slice() {
                [group, problem, excerpt, after, ..] => (*group, *problem, *excerpt, *after),
                [group, problem, excerpt] => (*group, *problem, *excerpt, ""),
                _ => return None,
            };
            let clean = |s: &str| {
                s.trim()
                    .trim_matches(['「', '」', '“', '”', '"'])
                    .trim()
                    .to_string()
            };
            let excerpt = clean(excerpt);
            let after = clean(after);
            if problem.is_empty() {
                return None;
            }
            let fix =
                (!excerpt.is_empty() && !after.is_empty() && after != "无" && after != excerpt)
                    .then(|| text.match_indices(&excerpt).collect::<Vec<_>>())
                    .filter(|hits| hits.len() == 1)
                    .map(|hits| hits[0].0)
                    .filter(|_| fix_passes_gate(&excerpt, &after, vocabulary))
                    .map(|at| Fix {
                        span: at..at + excerpt.len(),
                        before: excerpt.clone(),
                        after: after.clone(),
                    });
            Some(Finding {
                group: if group.is_empty() {
                    "表述".into()
                } else {
                    group.to_string()
                },
                text: problem.to_string(),
                excerpt: short(&excerpt, 40),
                source: "模型诊断，需人工判断".into(),
                fix,
            })
        })
        .take(max)
        .collect()
}

/// 模型给的改法：不能多行，不能大改，不能动时间、数字、单位、文件这些关键事实。
fn fix_passes_gate(
    before: &str,
    after: &str,
    vocabulary: &[crate::models::VocabularyEntry],
) -> bool {
    if after.contains('\n') {
        return false;
    }
    let (b, a) = (before.chars().count(), after.chars().count());
    if a > b * 2 + 20 || a * 3 < b {
        return false;
    }
    let facts = |text: &str| {
        let mut values: Vec<String> = crate::ai_guard::extract_key_facts(text, vocabulary)
            .into_iter()
            .map(|fact| fact.value)
            .collect();
        values.sort();
        values.dedup();
        values
    };
    facts(before) == facts(after)
}

/// `fact_check`：事实核查。
pub(super) fn fact_check(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let text = document(ctx)?;
    let max = param(ctx, step, &["max"], 15, 1..=40);
    let plain = crate::agent::evidence::strip_citations(&text);
    let facts: Vec<crate::ai_guard::FactToken> =
        crate::ai_guard::extract_key_facts(&plain, ctx.env.vocabulary)
            .into_iter()
            .filter(|fact| {
                matches!(
                    fact.kind,
                    FactKind::DateTime | FactKind::Number | FactKind::Document
                )
            })
            .collect();
    // 只核最长的那个：「2026年10月23日」不再单独核「23日」。
    let mut values: Vec<String> = Vec::new();
    for fact in &facts {
        let contained = facts
            .iter()
            .any(|other| other.value.len() > fact.value.len() && other.value.contains(&fact.value));
        if !contained && !values.contains(&fact.value) {
            values.push(fact.value.clone());
        }
    }
    values.truncate(max);
    let material = squash(&format!(
        "{}\n{}",
        ctx.board.request,
        ctx.board.notes.join("\n")
    ));
    let mut findings = Vec::new();
    // 同一句里有几条事实时只检索一次。
    let mut searched: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for value in values {
        check_cancel(ctx)?;
        let Some(pos) = plain.find(&value) else {
            continue;
        };
        let sentence = plain[sentence_at(&plain, pos)].trim().to_string();
        phase(ctx, format!("核查「{value}」…"));
        let (group, source) = if !material.is_empty() && material.contains(&squash(&value)) {
            ("有出处", "你提供的材料".to_string())
        } else {
            judge(ctx, step, &value, &sentence, &mut searched)?
        };
        findings.push(Finding {
            group: group.into(),
            text: value.clone(),
            excerpt: short(&sentence, 60),
            source,
            fix: None,
        });
    }
    // 矛盾的排最前，其次无出处。
    findings.sort_by_key(|f| match f.group.as_str() {
        "与资料矛盾" => 0,
        "无出处" => 1,
        _ => 2,
    });
    let count = |group: &str| findings.iter().filter(|f| f.group == group).count();
    let summary = format!(
        "核查 {} 条事实：有出处 {}，无出处 {}，与资料矛盾 {}",
        findings.len(),
        count("有出处"),
        count("无出处"),
        count("与资料矛盾")
    );
    tool_line(ctx, "check.facts", Permission::Check, summary);
    ctx.board.findings.extend(findings);
    Ok(Flow::Next)
}

/// 一条事实：检索 → 字面撞上就认 → 否则交模型判支持 / 矛盾 / 无关。返回 (分组, 出处说明)。
fn judge(
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    value: &str,
    sentence: &str,
    searched: &mut BTreeMap<String, Vec<usize>>,
) -> anyhow::Result<(&'static str, String)> {
    let ids = match searched.get(sentence) {
        Some(ids) => ids.clone(),
        None => {
            let ids: Vec<usize> = fetch_into(ctx, step, sentence)
                .into_iter()
                .map(|(_, id)| id)
                .collect();
            searched.insert(sentence.to_string(), ids.clone());
            ids
        }
    };
    if ids.is_empty() {
        return Ok(("无出处", "知识库与数据接口里都没有查到".into()));
    }
    let label = |ctx: &ToolCtx<'_, '_>, id: usize| {
        ctx.board
            .evidence
            .get(id)
            .map(|e| format!("[K{id}]{}", e.source_label()))
            .unwrap_or_default()
    };
    let wanted = squash(value);
    if wanted.chars().count() >= 5
        && let Some(id) = ids.iter().find(|id| {
            ctx.board
                .evidence
                .get(**id)
                .is_some_and(|e| squash(&e.text).contains(&wanted))
        })
    {
        return Ok(("有出处", label(ctx, *id)));
    }
    let locals = [
        ("value", value.to_string()),
        ("sentence", sentence.to_string()),
        ("evidence", ctx.board.evidence.format(&ids, 4000)),
    ];
    let reply = assist(ctx, &prompt(ctx, step, "prompt", "事实核对", &locals)?)?;
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
    Ok(if reply.contains("矛盾") {
        let detail = reply
            .split_once(['：', ':'])
            .map(|(_, rest)| rest.trim().to_string())
            .unwrap_or_default();
        let source = named.map(|id| label(ctx, id)).unwrap_or_default();
        (
            "与资料矛盾",
            if detail.is_empty() {
                source
            } else {
                format!("{source} 资料里是：{detail}")
            },
        )
    } else if reply.contains("支持") && !reply.contains("不支持") {
        let id = named.or_else(|| (ids.len() == 1).then(|| ids[0]));
        ("有出处", id.map(|id| label(ctx, id)).unwrap_or_default())
    } else {
        ("无出处", "查到的资料不能证实".into())
    })
}

/// `report`：把清单变量（`from`，默认 `items`）列成问题清单，分组写在 `group`（默认「要点」）。
pub(super) fn report(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let from = step.param_str("from").unwrap_or("items");
    let group = step.param_str("group").unwrap_or("要点").to_string();
    let items: Vec<String> = match ctx.board.vars.get(from) {
        Some(Value::Array(items)) => items
            .iter()
            .map(crate::agent::board::value_to_text)
            .collect(),
        Some(Value::String(text)) => text.lines().map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let findings: Vec<Finding> = items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .map(|item| Finding {
            group: group.clone(),
            text: item,
            excerpt: String::new(),
            source: String::new(),
            fix: None,
        })
        .collect();
    tool_line(
        ctx,
        "report",
        Permission::Read,
        format!("整理出{group} {} 条", findings.len()),
    );
    ctx.board.findings.extend(findings);
    Ok(Flow::Next)
}
