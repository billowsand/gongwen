//! `generate`：调起草模型写工作稿。
//!
//! - `mode: new`（默认）：按材料与证据写新稿，提示词那一段（默认「起草附加要求」）带着
//!   `{evidence}` 拼在参考资料位置；
//! - `mode: rewrite`：在正文上改写（润色），提示词那一段是修改要求；有选区时加上「只许改这
//!   一段」的硬约束，并附事实锁定清单。
//!
//! 结果都只写进工作稿；定稿成提案由调用方负责。

use super::{Flow, check_cancel, param, phase};
use crate::agent::backend::ModelRole;
use crate::agent::board::Board;
use crate::agent::engine::Event;
use crate::agent::skill::StepSpec;
use crate::agent::tools::ToolCtx;
use crate::lmstudio::StreamDelta;

pub(super) fn generate(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    check_cancel(ctx)?;
    let (user, label) = match step.param_str("mode").unwrap_or("new") {
        "new" => (new_draft_prompt(ctx, step)?, "起草中…"),
        "rewrite" => (rewrite_prompt(ctx, step)?, "改写中…"),
        other => anyhow::bail!("generate 不认识 mode「{other}」（可用 new / rewrite）"),
    };
    phase(ctx, label);
    let system = ctx.board.system_prompt.clone();
    let emit = &mut *ctx.emit;
    let completion = ctx
        .env
        .model
        .complete(ModelRole::Draft, &system, &user, &mut |delta| match delta {
            StreamDelta::Content(text) => emit(Event::Content(text.to_string())),
            StreamDelta::Reasoning(text) => emit(Event::Reasoning(text.to_string())),
        })?;
    ctx.board.truncated |= completion.truncated;
    ctx.board.workspace = crate::prompt::sanitize_model_markdown(&completion.content);
    (ctx.emit)(Event::Workspace(ctx.board.workspace.clone()));
    Ok(Flow::Next)
}

fn new_draft_prompt(ctx: &ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<String> {
    let board = &ctx.board;
    let reference = if board.evidence.is_empty() {
        String::new()
    } else {
        let chars = param(ctx, step, &["evidence_chars"], 8000, 1000..=40000);
        let evidence = board.evidence.format(&board.evidence.all_ids(), chars);
        let name = step.param_str("prompt").unwrap_or("起草附加要求");
        let text = match ctx.env.skill.section(name) {
            Some(template) => board.render_with(template, &[("evidence", evidence)]),
            None => evidence,
        };
        format!("\n\n{text}")
    };
    Ok(crate::prompt::build_draft_prompt(
        &board.draft,
        ctx.env.vocabulary,
        &material_block(board),
        &reference,
    ))
}

fn rewrite_prompt(ctx: &ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<String> {
    let board = &ctx.board;
    let current = board.document.trim();
    if current.is_empty() {
        anyhow::bail!("正文还是空的，没有可改写的内容");
    }
    let name = step.param_str("prompt").unwrap_or("润色");
    let request = match ctx.env.skill.section(name) {
        Some(template) => board.render(template),
        None => board.request.clone(),
    };
    let instruction = crate::ai_panel::polish_instruction("", &request, board.selection.as_deref());
    let protected = crate::ai_guard::protected_facts_prompt(current, ctx.env.vocabulary);
    Ok(crate::prompt::build_optimize_prompt(
        &board.draft,
        current,
        &format!("{}{protected}", instruction.trim()),
    ))
}

fn material_block(board: &Board) -> String {
    let mut text = format!("【原始材料与写作要求】\n{}", board.request.trim());
    if !board.notes.is_empty() {
        text.push_str("\n\n【动笔前已确认——优先于原始材料】");
        for note in &board.notes {
            text.push_str("\n- ");
            text.push_str(note);
        }
    }
    text
}
