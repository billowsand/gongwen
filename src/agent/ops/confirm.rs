//! 清单确认（`docs/decision-modules.md` 第七节）：任何清单变量都能停下来让用户确认、直接改，
//! 可修订的还能让 AI 按修改要求重列。
//!
//! - `confirm` 算子：确认流程里已有的清单变量（检索问题、要点、来函事项……）；
//! - `plan` 列出清单后也走这里（[`list`]），大纲确认就是其中一种。
//!
//! 修订请求（`decision::REVISION`）由挂起的那一步自己处理（[`take_revision`]）：界面写入请求、
//! 把续跑位置拨回这一步，这一步见到请求就按要求重列、再停下来确认。

use super::prepare::lines_of;
use super::{Flow, assist, check_cancel, note, param, phase, tool_line};
use crate::agent::clarify::{Action, Choice, Question, Target};
use crate::agent::decision::{ListConfirm, REVISION};
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx};
use serde_json::Value;

/// 通用的修订提示词（技能里没有「优化{label}」这段时用）。
pub(super) const REVISE_TEMPLATE: &str = "根据原始写作要求和修改要求优化当前{label}。以当前{label}为基础，保留未要求\
     调整的内容，只输出{label}，每行一条，不加编号、说明，最多 {max} 条。\n\n\
     【原始写作要求】\n{request}\n\n【当前{label}】\n{current}\n\n【修改要求】\n{instruction}";

/// `confirm`：确认 `over` 指向的清单变量。参数 `label`（卡片上的叫法，默认「清单」）、
/// `revise`（能否让 AI 按要求重列，默认否）、`max`（重列时最多几条，默认 20）、
/// `revise_prompt`（重列用的提示词段，默认「优化{label}」）。变量不存在或为空时跳过。
pub(super) fn confirm(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let over = step.param_str("over").unwrap_or_default().to_string();
    let label = step.param_str("label").unwrap_or("清单");
    let revisable = step
        .params
        .get("revise")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let spec = ListConfirm::new(label, over.clone(), revisable);
    let max = param(ctx, step, &["max"], 20, 1..=60);
    if let Some(flow) = take_revision(ctx, step, &spec, max, REVISE_TEMPLATE)? {
        return Ok(flow);
    }
    let items = match ctx.board.vars.get(&over) {
        Some(Value::Array(items)) => items
            .iter()
            .map(crate::agent::board::item_line)
            .collect::<Vec<_>>(),
        Some(Value::String(text)) => text.lines().map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let items: Vec<String> = items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if items.is_empty() {
        note(ctx, format!("没有可确认的「{over}」，跳过这一步"));
        return Ok(Flow::Next);
    }
    Ok(ask(ctx, &items, spec))
}

/// 清单存进变量；`confirm`（缺省 `confirm_by_default`）为真时停下来让用户确认。
pub(super) fn list(
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    items: Vec<String>,
    spec: ListConfirm,
    confirm_by_default: bool,
) -> Flow {
    ctx.board.vars.insert(
        spec.var.clone(),
        Value::Array(items.iter().cloned().map(Value::String).collect()),
    );
    let confirm = step
        .params
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(confirm_by_default);
    if confirm {
        ask(ctx, &items, spec)
    } else {
        Flow::Next
    }
}

/// 挂起的那一步开头调用：有修订请求就按要求重列，重列后**一律再确认**（不能因为技能关了
/// 初次确认就直接往下写）。模型没给出清单时保留当前文本、记一条说明，仍回到确认。
/// 没有修订请求返回 None，调用方照常产出清单。
pub(super) fn take_revision(
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    spec: &ListConfirm,
    max: usize,
    default_template: &str,
) -> anyhow::Result<Option<Flow>> {
    let Some(revision) = ctx.board.vars.get(REVISION).cloned() else {
        return Ok(None);
    };
    let label = spec.label.as_str();
    let current = revision["current"].as_str().unwrap_or_default();
    let instruction = revision["instruction"].as_str().unwrap_or_default();
    phase(ctx, format!("按修改要求优化{label}…"));
    let locals = [
        ("max", max.to_string()),
        ("label", label.to_string()),
        ("request", ctx.board.request_with_notes()),
        ("current", current.to_string()),
        // 旧技能的「优化大纲」段里写的是 {outline}。
        ("outline", current.to_string()),
        ("instruction", instruction.to_string()),
    ];
    let section = step
        .param_str("revise_prompt")
        .map_or_else(|| format!("优化{label}"), str::to_string);
    let template = ctx.env.skill.section(&section).unwrap_or(default_template);
    let text = ctx.board.render_with(template, &locals);
    let result = assist(ctx, &text).map(|reply| crate::prompt::sanitize_model_markdown(&reply));
    check_cancel(ctx)?;
    let items = match result {
        Ok(reply) if !lines_of(&reply, max).is_empty() => lines_of(&reply, max),
        result => {
            let reason = result.err().map_or(format!("模型未返回有效{label}"), |e| {
                format!("{e:#}")
            });
            note(
                ctx,
                format!("{label}优化未完成（{reason}），已保留当前{label}，可继续修改或重试。"),
            );
            current.lines().map(str::to_string).collect()
        }
    };
    ctx.board.vars.remove(REVISION);
    ctx.board.vars.insert(
        spec.var.clone(),
        Value::Array(items.iter().cloned().map(Value::String).collect()),
    );
    Ok(Some(ask(ctx, &items, spec.clone())))
}

/// 停下来让用户在框里改；选「就按这个写」变量不变，改了存改后的文本（`Target::Pick`）。
fn ask(ctx: &mut ToolCtx<'_, '_>, items: &[String], spec: ListConfirm) -> Flow {
    let label = spec.label.as_str();
    tool_line(
        ctx,
        "ask.choice",
        Permission::AskUser,
        format!("{label}请你确认"),
    );
    let question = Question {
        id: 1,
        text: format!("按这个{label}写吗？可以直接在框里改，每行一条"),
        choices: vec![Choice {
            label: "就按这个写".into(),
            detail: String::new(),
            recommended: true,
            action: Action::Pick(Value::Null),
        }],
        custom_hint: Some(spec.hint.clone()),
        prefill: items.join("\n"),
        skippable: false,
        target: Target::Pick,
    };
    Flow::Confirm(vec![question], spec)
}
