//! 算子：内置的组合招式（`docs/ai-agent-workbench.md` 16.4），技能流程用 `step:` 调用。
//!
//! 由第二期的 `research.rs` 拆出，行为不变：
//! - `prepare`：`clarify`（动笔前澄清）、`plan`（预研列问题）、`retrieve`（多路检索）；
//! - `write`：`generate`（新稿 / 全文或选区改写）；
//! - `gap_loop`：缺口循环（识别 → 定向检索 → 局部补全 → 闸门）；
//! - `finish`：`verify`（核验引用）、`ask`（出题）。
//!
//! `for_each` 由引擎直接处理。算子的参数先看步骤里写的，再看技能 `params` 里的同名项，
//! 都没有就用默认值，并夹在合理范围里。

mod finish;
mod gap_loop;
mod prepare;
mod write;

use super::backend::ModelRole;
use super::clarify::Question;
use super::engine::Event;
use super::skill::StepSpec;
use super::tools::{ASSIST_SYSTEM, Permission, ToolCtx, ToolUse, short};
use crate::lmstudio::StreamDelta;

/// 算子执行完之后怎么走。
pub(crate) enum Flow {
    Next,
    /// 停下来问用户。
    Suspend(Vec<Question>),
}

pub(crate) type Operator = fn(&mut ToolCtx<'_, '_>, &StepSpec) -> anyhow::Result<Flow>;

const OPERATORS: [(&str, Operator); 7] = [
    ("clarify", prepare::clarify),
    ("plan", prepare::plan),
    ("retrieve", prepare::retrieve),
    ("generate", write::generate),
    ("gap_loop", gap_loop::gap_loop),
    ("verify", finish::verify),
    ("ask", finish::ask),
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

/// 去掉空白，比对字面用。
fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}
