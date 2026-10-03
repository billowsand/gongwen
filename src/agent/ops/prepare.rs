//! 动笔之前：澄清、预研、检索。

use super::{Flow, assist, check_cancel, note, param, phase, prompt, search_into, tool_line};
use crate::agent::clarify;
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx};
use serde_json::Value;

/// 预研列出的检索词存在这个变量里，`retrieve` 默认从这里取。
const QUERIES: &str = "queries";

/// `clarify`：程序规则 + 模型判断，只问会让整篇写偏的事；有题就挂起。已经问过（黑板标了
/// 已澄清）就跳过。参数 `max`（技能参数 `pre_questions`），提示词默认「动笔前澄清」。
pub(super) fn clarify(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let max = param(ctx, step, &["max", "pre_questions"], 3, 0..=5);
    if ctx.board.clarified || max == 0 {
        return Ok(Flow::Next);
    }
    phase(ctx, "动笔前检查要求是否明确…");
    let locals = [
        ("max", max.to_string()),
        ("title", ctx.board.draft.title_hint.trim().to_string()),
        ("request", ctx.board.request_with_notes()),
    ];
    let text = prompt(ctx, step, "prompt", "动笔前澄清", &locals)?;
    let reply = assist(ctx, &text)?;
    let parsed = clarify::parse_model_questions(&reply, max);
    let questions =
        clarify::predraft_questions(&ctx.board.request, ctx.board.draft.kind, parsed, max);
    if questions.is_empty() {
        return Ok(Flow::Next);
    }
    tool_line(
        ctx,
        "ask.choice",
        Permission::AskUser,
        format!("动笔前有 {} 个问题要你确认", questions.len()),
    );
    Ok(Flow::Suspend(questions))
}

/// `plan`：让模型列出要到知识库里查清的问题，连同用户原话存进 `save_as`（默认 `queries`）。
/// 参数 `max`（技能参数 `research_questions`），为 0 时只存原话；提示词默认「预研」。
pub(super) fn plan(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let max = param(ctx, step, &["max", "research_questions"], 6, 0..=12);
    let mut queries = vec![ctx.board.request.trim().to_string()];
    if max > 0 {
        phase(ctx, "预研：列出要查的问题…");
        let locals = [
            ("max", max.to_string()),
            ("request", ctx.board.request_with_notes()),
        ];
        let text = prompt(ctx, step, "prompt", "预研", &locals)?;
        let reply = assist(ctx, &text)?;
        let planned: Vec<String> = reply
            .lines()
            .map(|line| clarify::strip_numbering(line.trim()).trim().to_string())
            .filter(|line| !line.is_empty() && line != "无")
            .take(max)
            .collect();
        tool_line(
            ctx,
            "plan",
            Permission::Read,
            format!("预研列出 {} 个要查的问题", planned.len()),
        );
        for question in planned {
            if !queries.contains(&question) {
                queries.push(question);
            }
        }
    }
    let name = step.save_as.as_deref().unwrap_or(QUERIES).to_string();
    ctx.board.vars.insert(
        name,
        Value::Array(queries.into_iter().map(Value::String).collect()),
    );
    Ok(Flow::Next)
}

/// `retrieve`：逐个检索词查知识库，结果并入证据包。检索词取 `from` 指定的变量（默认
/// `queries`），没有就用用户原话。知识库没启用时只留一条说明。
pub(super) fn retrieve(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    if !ctx.env.kb.enabled() {
        note(ctx, "知识库未启用：只按材料起草，缺口全部交给你确认。");
        return Ok(Flow::Next);
    }
    let from = step.param_str("from").unwrap_or(QUERIES);
    let queries: Vec<String> = match ctx.board.vars.get(from) {
        Some(Value::Array(items)) => items
            .iter()
            .map(crate::agent::board::value_to_text)
            .collect(),
        Some(Value::String(text)) => text.lines().map(str::to_string).collect(),
        _ => vec![ctx.board.request.trim().to_string()],
    };
    for query in queries.iter().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        check_cancel(ctx)?;
        search_into(ctx, query);
    }
    Ok(Flow::Next)
}
