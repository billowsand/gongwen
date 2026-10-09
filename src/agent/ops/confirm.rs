//! 清单确认（`docs/decision-modules.md` 第七节、9.4、9.5）：任何清单变量都能停下来让用户确认、
//! 直接改，可修订的还能让 AI 按修改要求重列；勾选模式下逐条打勾取舍。
//!
//! - `confirm` 算子：确认流程里已有的清单变量（检索问题、要点、来函事项……）；
//! - `plan` 列出清单后也走这里（[`list`]），大纲确认就是其中一种；
//! - `pick_evidence` 算子：证据包里的资料逐份打勾，没勾的剔出去，之后的检索也不再找回来。
//!
//! 修订请求（`decision::REVISION`）由挂起的那一步自己处理（[`take_revision`]）：界面写入请求、
//! 把续跑位置拨回这一步，这一步见到请求就按要求重列、再停下来确认。

use super::prepare::lines_of;
use super::{Flow, assist, check_cancel, note, param, phase, tool_line};
use crate::agent::clarify::{Action, Choice, Question, Target};
use crate::agent::decision::{Decision, ListConfirm, REVISION};
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx, short};
use serde_json::Value;

/// 通用的修订提示词（技能里没有「优化{label}」这段时用）。
pub(super) const REVISE_TEMPLATE: &str = "根据原始写作要求和修改要求优化当前{label}。以当前{label}为基础，保留未要求\
     调整的内容，只输出{label}，每行一条，不加编号、说明，最多 {max} 条。\n\n\
     【原始写作要求】\n{request}\n\n【当前{label}】\n{current}\n\n【修改要求】\n{instruction}";

/// 证据取舍时勾中的资料键存在这个变量里，答完回到 `pick_evidence` 由它剔除没勾的。
const KEEP_VAR: &str = "_evidence_keep";

/// 清单里的一条：卡片上显示的文字、勾选模式下的说明、勾中后存进变量的原值。
struct Item {
    label: String,
    detail: String,
    value: Value,
}

impl Item {
    fn text(label: String) -> Self {
        Self {
            detail: String::new(),
            value: Value::String(label.clone()),
            label,
        }
    }
}

/// 步骤参数里的布尔开关。
fn flag(step: &StepSpec, key: &str) -> Option<bool> {
    step.params.get(key).and_then(Value::as_bool)
}

/// `confirm`：确认 `over` 指向的清单变量。参数 `label`（卡片上的叫法，默认「清单」）、
/// `revise`（能否让 AI 按要求重列，默认否）、`pick`（逐条打勾取舍，勾中的项按原值存回，
/// 不能改字也不能重列）、`max`（重列时最多几条，默认 20）、`revise_prompt`（重列用的提示词段，
/// 默认「优化{label}」）。变量不存在或为空时跳过。
pub(super) fn confirm(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let over = step.param_str("over").unwrap_or_default().to_string();
    let label = step.param_str("label").unwrap_or("清单");
    let spec = if flag(step, "pick") == Some(true) {
        ListConfirm::picking(label, over.clone())
    } else {
        ListConfirm::new(label, over.clone(), flag(step, "revise").unwrap_or(false))
    };
    let max = param(ctx, step, &["max"], 20, 1..=60);
    if spec.revisable
        && let Some(flow) = take_revision(ctx, step, &spec, max, REVISE_TEMPLATE)?
    {
        return Ok(flow);
    }
    let items: Vec<Item> = match ctx.board.vars.get(&over) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| Item {
                label: crate::agent::board::item_line(item).trim().to_string(),
                detail: String::new(),
                value: item.clone(),
            })
            .collect(),
        Some(Value::String(text)) => text
            .lines()
            .map(|line| Item::text(line.trim().to_string()))
            .collect(),
        _ => Vec::new(),
    };
    let items: Vec<Item> = items
        .into_iter()
        .filter(|item| !item.label.is_empty())
        .collect();
    if items.is_empty() {
        note(ctx, format!("没有可确认的「{over}」，跳过这一步"));
        return Ok(Flow::Next);
    }
    Ok(ask(ctx, items, spec, false))
}

/// 清单存进变量；`confirm`（缺省 `confirm_by_default`）为真时停下来让用户确认。步骤写了
/// `pick: true` 的改成逐条打勾（`spec` 换成勾选模式）。
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
    let spec = if flag(step, "pick") == Some(true) {
        ListConfirm::picking(&spec.label, spec.var)
    } else {
        spec
    };
    if flag(step, "confirm").unwrap_or(confirm_by_default) {
        ask(
            ctx,
            items.into_iter().map(Item::text).collect(),
            spec,
            false,
        )
    } else {
        Flow::Next
    }
}

/// `pick_evidence`：证据包里的文件不少于 `min` 份（默认 3）时，逐份打勾取舍（同一份文件的
/// 各段算一份；用户 `@` 引用的不列、一律保留）。答完回到这一步：没勾的文件剔出证据包并记下
/// 题名，之后的检索（逐章检索、缺口循环）不会再把它们找回来。
pub(super) fn pick_evidence(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    if let Some(keep) = ctx.board.vars.remove(KEEP_VAR) {
        let keep: Vec<String> = keep
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let pinned = ctx.board.pinned.clone();
        let dropped = ctx.board.evidence.exclude_except(&keep, &pinned);
        let line = if dropped == 0 {
            "资料都留着".to_string()
        } else {
            format!("去掉 {dropped} 份资料，之后的检索也不再用它们")
        };
        tool_line(ctx, "pick_evidence", Permission::Read, line);
        return Ok(Flow::Next);
    }
    let min = param(ctx, step, &["min"], 3, 1..=20);
    // 按文件归并：题名 → （类别, 段数, 第一段摘录）。
    let mut docs: Vec<(String, String, usize, String)> = Vec::new();
    for item in ctx.board.evidence.items() {
        if ctx.board.pinned.contains(&item.id) {
            continue;
        }
        match docs.iter_mut().find(|doc| doc.0 == item.doc_title) {
            Some(doc) => doc.2 += 1,
            None => docs.push((
                item.doc_title.clone(),
                item.kind_label.clone(),
                1,
                short(item.text.trim(), 80),
            )),
        }
    }
    if docs.len() < min {
        return Ok(Flow::Next);
    }
    let items: Vec<Item> = docs
        .into_iter()
        .map(|(title, kind, count, excerpt)| Item {
            label: if count > 1 {
                format!("{kind}《{title}》（{count} 段）")
            } else {
                format!("{kind}《{title}》")
            },
            detail: excerpt,
            value: Value::String(title),
        })
        .collect();
    let spec = ListConfirm {
        submit: "用勾选的资料继续".into(),
        hint: "勾掉过时、废止或不相干的资料".into(),
        ..ListConfirm::picking("资料", KEEP_VAR)
    };
    Ok(ask(ctx, items, spec, true))
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
    Ok(Some(ask(
        ctx,
        items.into_iter().map(Item::text).collect(),
        spec.clone(),
        false,
    )))
}

/// 停下来让用户确认清单（`Target::Pick`）。
///
/// - 普通模式：一个「就按这个写」选项加预填的编辑框；选它变量不变，改了存改后的文本；
/// - 勾选模式：每条一个选项、默认全勾（界面按 `ListConfirm.pick` 画勾选框），回答是勾中的
///   下标（`Reply::Many`），落地时存勾中各项的原值。
///
/// `again` 为真时答完回到这一步（由它处理答案，如证据取舍），否则往下走。
fn ask(ctx: &mut ToolCtx<'_, '_>, items: Vec<Item>, spec: ListConfirm, again: bool) -> Flow {
    let label = spec.label.as_str();
    tool_line(
        ctx,
        "ask.choice",
        Permission::AskUser,
        if spec.pick {
            format!("{} 份{label}请你取舍", items.len())
        } else {
            format!("{label}请你确认")
        },
    );
    let prefill = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let question = if spec.pick {
        Question {
            id: 1,
            text: format!("这些{label}都用吗？勾掉不要的"),
            choices: items
                .into_iter()
                .map(|item| Choice {
                    label: item.label,
                    detail: item.detail,
                    recommended: true,
                    action: Action::Pick(item.value),
                })
                .collect(),
            custom_hint: None,
            prefill,
            skippable: true,
            multi: false,
            target: Target::Pick,
        }
    } else {
        Question {
            id: 1,
            text: format!("按这个{label}写吗？可以直接在框里改，每行一条"),
            choices: vec![Choice {
                label: "就按这个写".into(),
                detail: String::new(),
                recommended: true,
                action: Action::Pick(Value::Null),
            }],
            custom_hint: Some(spec.hint.clone()),
            prefill,
            skippable: false,
            multi: false,
            target: Target::Pick,
        }
    };
    Flow::Decide {
        questions: vec![question],
        into: spec.var.clone(),
        decision: Decision::ConfirmList(spec),
        again,
    }
}
