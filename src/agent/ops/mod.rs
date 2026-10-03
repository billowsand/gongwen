//! 算子：内置的组合招式（`docs/ai-agent-workbench.md` 16.4），技能流程用 `step:` 调用。
//!
//! 由第二期的 `research.rs` 拆出，行为不变：
//! - `prepare`：`clarify`（动笔前澄清）、`plan`（预研列问题）、`retrieve`（多路检索）；
//! - `write`：`generate`（新稿 / 全文或选区改写）；
//! - `gap_loop`：缺口循环（识别 → 定向检索 → 局部补全 → 闸门）；
//! - `finish`：`verify`（核验引用）、`cite`（引用落到研究报告的文献与脚注）、`ask`（出题）；
//! - `review`：审核类技能的 `review`（全面审校）、`fact_check`（事实核查）、`report`（清单成问题）。
//!
//! `for_each` 由引擎直接处理。算子的参数先看步骤里写的，再看技能 `params` 里的同名项，
//! 都没有就用默认值，并夹在合理范围里。
//!
//! 检索类算子（`retrieve`、`gap_loop`）除了知识库，还可以查步骤里 `apis:` 列出的数据接口；
//! 接口调用照样经工具白名单，技能没声明 `http.call:<id>` 就调不到。

mod agent;
mod cite_check;
mod finish;
mod gap_loop;
mod prepare;
mod review;
mod write;

use super::backend::ModelRole;
use super::clarify::Question;
use super::engine::Event;
use super::skill::StepSpec;
use super::tools::{ASSIST_SYSTEM, Permission, ToolCtx, ToolUse, short};
use crate::lmstudio::StreamDelta;
use serde_json::Value;

/// 算子执行完之后怎么走。
pub(crate) enum Flow {
    Next,
    /// 停下来问用户；选择题的答案存进步骤的 `save_as`。
    Suspend(Vec<Question>),
    /// 停下来问用户，答案存进算子指定的变量（如待确认的大纲）。
    SuspendInto(Vec<Question>, String),
}

pub(crate) use agent::SUMMARY_VAR as AGENT_SUMMARY;

pub(crate) type Operator = fn(&mut ToolCtx<'_, '_>, &StepSpec) -> anyhow::Result<Flow>;

const OPERATORS: [(&str, Operator); 13] = [
    ("clarify", prepare::clarify),
    ("plan", prepare::plan),
    ("retrieve", prepare::retrieve),
    ("generate", write::generate),
    ("gap_loop", gap_loop::gap_loop),
    ("verify", finish::verify),
    ("cite", finish::cite),
    ("ask", finish::ask),
    ("review", review::review),
    ("fact_check", review::fact_check),
    ("report", review::report),
    ("agent", agent::agent),
    ("cite_check", cite_check::cite_check),
];

pub(crate) fn find(name: &str) -> Option<Operator> {
    OPERATORS
        .iter()
        .find(|(id, _)| *id == name)
        .map(|(_, operator)| *operator)
}

/// 引擎认识的全部算子名（含 `for_each`），给技能校验用。
pub(crate) fn names() -> Vec<&'static str> {
    OPERATORS
        .iter()
        .map(|(id, _)| *id)
        .chain(["for_each"])
        .collect()
}

pub(crate) fn check_cancel(ctx: &ToolCtx<'_, '_>) -> anyhow::Result<()> {
    if ctx.env.model.cancelled() {
        anyhow::bail!("已停止生成");
    }
    Ok(())
}

fn tool_line(
    ctx: &mut ToolCtx<'_, '_>,
    name: &'static str,
    permission: Permission,
    summary: String,
) {
    (ctx.emit)(Event::Tool(ToolUse::new(name, permission, summary)));
}

fn phase(ctx: &mut ToolCtx<'_, '_>, text: impl Into<String>) {
    (ctx.emit)(Event::Phase(text.into()));
}

fn note(ctx: &mut ToolCtx<'_, '_>, text: impl Into<String>) {
    (ctx.emit)(Event::Note(text.into()));
}

/// 整数参数：步骤里按 `keys` 依次找，再到技能 `params` 里找，都没有用 `default`。
fn param(
    ctx: &ToolCtx<'_, '_>,
    step: &StepSpec,
    keys: &[&str],
    default: usize,
    range: std::ops::RangeInclusive<usize>,
) -> usize {
    let skill = ctx.env.skill;
    keys.iter()
        .find_map(|key| step.param_usize(key))
        .or_else(|| {
            keys.iter()
                .find(|key| skill.params.contains_key(**key))
                .map(|key| skill.param_usize(key, default, range.clone()))
        })
        .unwrap_or(default)
        .clamp(*range.start(), *range.end())
}

/// 取步骤 `key` 指向的提示词（没写就用 `default`）并替换变量。技能里没有这段就报错，
/// 不拿空提示词去问模型。
fn prompt(
    ctx: &ToolCtx<'_, '_>,
    step: &StepSpec,
    key: &str,
    default: &str,
    locals: &[(&str, String)],
) -> anyhow::Result<String> {
    let name = step.param_str(key).unwrap_or(default);
    let template =
        ctx.env.skill.section(name).ok_or_else(|| {
            anyhow::anyhow!("技能「{}」里没有提示词「{name}」", ctx.env.skill.name)
        })?;
    Ok(ctx.board.render_with(template, locals))
}

/// 辅助步骤：思考照样流进界面，正文不流（它是给程序看的）。
fn assist(ctx: &mut ToolCtx<'_, '_>, prompt: &str) -> anyhow::Result<String> {
    check_cancel(ctx)?;
    let emit = &mut *ctx.emit;
    let completion =
        ctx.env
            .model
            .complete(ModelRole::Assist, ASSIST_SYSTEM, prompt, &mut |delta| {
                if let StreamDelta::Reasoning(text) = delta {
                    emit(Event::Reasoning(text.to_string()));
                }
            })?;
    Ok(completion.content.trim().to_string())
}

/// 检索知识库并把结果并入证据包，返回每段的 (证据键, 编号)。失败只留说明，不中断流程。
fn search_into(ctx: &mut ToolCtx<'_, '_>, query: &str) -> Vec<(String, usize)> {
    match ctx.env.kb.search(query) {
        Ok((chunks, warnings)) => {
            let pack = &mut ctx.board.evidence;
            let before = pack.items().len();
            let ids = pack.absorb(query, &chunks);
            let added = pack.items().len() - before;
            tool_line(
                ctx,
                "kb.search",
                Permission::Read,
                format!(
                    "检索「{}」→ {} 段（新增 {added}）",
                    short(query, 24),
                    chunks.len()
                ),
            );
            for warning in warnings {
                note(ctx, format!("知识库：{warning}"));
            }
            chunks
                .iter()
                .map(|chunk| format!("kb:{}", chunk.chunk_id))
                .zip(ids)
                .collect()
        }
        Err(error) => {
            note(ctx, format!("知识库检索失败：{error:#}"));
            Vec::new()
        }
    }
}

/// 步骤里写的数据接口：`apis: [stat, { api: policy, args: { keyword: "{query}" } }]`。
/// 只写 id 时，检索词填给接口的第一个输入变量。
fn api_sources(step: &StepSpec) -> Vec<(String, Option<Value>)> {
    let Some(Value::Array(items)) = step.params.get("apis") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::String(id) => Some((id.clone(), None)),
            Value::Object(map) => map
                .get("api")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), map.get("args").cloned())),
            _ => None,
        })
        .collect()
}

/// 这一步有没有可查的资料来源：知识库启用了，或写了数据接口。
fn has_sources(ctx: &ToolCtx<'_, '_>, step: &StepSpec) -> bool {
    ctx.env.kb.enabled() || !api_sources(step).is_empty()
}

/// 用一个检索词查这一步的全部来源（知识库 + 数据接口），结果并入证据包，
/// 返回每条资料的 (证据键, 编号)。
fn fetch_into(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec, query: &str) -> Vec<(String, usize)> {
    let mut found = if ctx.env.kb.enabled() {
        search_into(ctx, query)
    } else {
        Vec::new()
    };
    for (api, args) in api_sources(step) {
        found.extend(call_api_into(ctx, &api, args.as_ref(), query));
    }
    found
}

/// 调一个数据接口。经 `tools::call` 走白名单：技能没声明 `http.call:<id>` 就调不到。
fn call_api_into(
    ctx: &mut ToolCtx<'_, '_>,
    api: &str,
    args: Option<&Value>,
    query: &str,
) -> Vec<(String, usize)> {
    let args = match args {
        Some(template) => ctx
            .board
            .render_value_with(template, &[("query", query.to_string())]),
        None => {
            let first = ctx
                .env
                .apis
                .get(api)
                .and_then(|endpoint| endpoint.inputs.first())
                .map(|input| input.name.clone());
            match first {
                Some(name) => Value::Object(
                    [(name, Value::String(query.to_string()))]
                        .into_iter()
                        .collect(),
                ),
                None => Value::Null,
            }
        }
    };
    match super::tools::call(&format!("http.call:{api}"), ctx, &args) {
        Ok(output) => {
            tool_line(
                ctx,
                "http.call",
                Permission::External,
                output.summary.clone(),
            );
            if !output.evidence_by_default {
                return Vec::new();
            }
            let ids = ctx.board.evidence.absorb_docs(query, &output.evidence);
            output
                .evidence
                .into_iter()
                .map(|doc| doc.key)
                .zip(ids)
                .collect()
        }
        Err(error) => {
            note(ctx, format!("数据接口「{api}」：{error}"));
            Vec::new()
        }
    }
}

/// 去掉空白，比对字面用。
fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}
