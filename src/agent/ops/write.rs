//! `generate`：调起草模型写工作稿。
//!
//! - `mode: new`（默认）：按材料写新稿。`prompt` 指向的一段是附加要求（仿写的基准稿、复函的
//!   来函事项……），总是带上；`evidence_prompt`（默认「起草附加要求」）是证据与引用规则，只在
//!   证据包不空时带上，`{evidence}` 换成带编号的证据；
//! - `mode: rewrite`：在正文上改写（润色），提示词那一段是修改要求；有选区时加上「只许改这
//!   一段」的硬约束，并附事实锁定清单。`source: workspace` 改工作稿而不是正文；`fit_length: true`
//!   时按原话里要的字数检查，差得多就再改一次；
//! - `mode: section`：写一节，接在工作稿末尾（政策研究报告逐节写）。`{evidence}` 只放最近一次
//!   `retrieve` 查到的证据，`{workspace}` 是已经写好的部分；
//! - `mode: fill`：生成一段替换工作稿里的占位记号 `marker`（如全文写完后补摘要）。
//!
//! 结果都只写进工作稿；定稿成提案由调用方负责。

use super::prepare::FOUND;
use super::{Flow, check_cancel, note, param, phase, prompt, tool_line};
use crate::agent::backend::ModelRole;
use crate::agent::board::Board;
use crate::agent::engine::Event;
use crate::agent::skill::StepSpec;
use crate::agent::tools::Permission;
use crate::agent::tools::ToolCtx;
use crate::lmstudio::StreamDelta;
use serde_json::Value;

/// 生成结果放到哪里。
enum Placement {
    /// 整篇换掉。
    Replace,
    /// 接在末尾。
    Append,
    /// 换掉工作稿里的这个记号。
    Marker(String),
}

pub(super) fn generate(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    check_cancel(ctx)?;
    let (user, label, placement) = match step.param_str("mode").unwrap_or("new") {
        "new" => (
            new_draft_prompt(ctx, step)?,
            "起草中…".to_string(),
            Placement::Replace,
        ),
        "rewrite" => (
            rewrite_prompt(ctx, step)?,
            "改写中…".to_string(),
            Placement::Replace,
        ),
        "section" => {
            let label = format!("写「{}」…", section_label(ctx));
            (section_prompt(ctx, step)?, label, Placement::Append)
        }
        "fill" => {
            let marker = step
                .param_str("marker")
                .ok_or_else(|| {
                    anyhow::anyhow!("generate 的 fill 模式要写 marker（工作稿里要替换的记号）")
                })?
                .to_string();
            if !ctx.board.workspace.contains(&marker) {
                note(ctx, format!("工作稿里没有「{marker}」，跳过这一步"));
                return Ok(Flow::Next);
            }
            let text = section_prompt(ctx, step)?;
            (text, "补写中…".to_string(), Placement::Marker(marker))
        }
        other => {
            anyhow::bail!("generate 不认识 mode「{other}」（可用 new / rewrite / section / fill）")
        }
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
    let text = crate::prompt::sanitize_model_markdown(&completion.content);
    let workspace = &mut ctx.board.workspace;
    match placement {
        Placement::Replace => *workspace = text,
        Placement::Append => {
            let trimmed = workspace.trim_end().len();
            workspace.truncate(trimmed);
            if !workspace.is_empty() {
                workspace.push_str("\n\n");
            }
            workspace.push_str(text.trim());
            workspace.push('\n');
        }
        Placement::Marker(marker) => {
            *workspace = workspace.replacen(&marker, text.trim(), 1);
        }
    }
    (ctx.emit)(Event::Workspace(ctx.board.workspace.clone()));
    if step.param_str("mode") == Some("rewrite")
        && step.params.get("fit_length").and_then(Value::as_bool) == Some(true)
    {
        fit_length(ctx, step)?;
    }
    Ok(Flow::Next)
}

fn new_draft_prompt(ctx: &ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<String> {
    let board = &ctx.board;
    let chars = param(ctx, step, &["evidence_chars"], 8000, 1000..=40000);
    let evidence = if board.evidence.is_empty() {
        String::new()
    } else {
        board.evidence.format(&board.evidence.all_ids(), chars)
    };
    let mut extra = Vec::new();
    if let Some(name) = step.param_str("prompt") {
        let template = ctx.env.skill.section(name).ok_or_else(|| {
            anyhow::anyhow!("技能「{}」里没有提示词「{name}」", ctx.env.skill.name)
        })?;
        // 第 ③ 期的写法把证据规则写在 prompt 里：没有证据时这段整个不带。
        if !(evidence.is_empty() && template.contains("{evidence}")) {
            extra.push(board.render_with(template, &[("evidence", evidence.clone())]));
        }
    }
    if !evidence.is_empty() {
        let named = step.param_str("evidence_prompt");
        if named.is_some() || step.param_str("prompt").is_none() {
            let name = named.unwrap_or("起草附加要求");
            extra.push(match ctx.env.skill.section(name) {
                Some(template) => board.render_with(template, &[("evidence", evidence.clone())]),
                None => evidence,
            });
        }
    }
    let reference = extra
        .iter()
        .filter(|text| !text.trim().is_empty())
        .map(|text| format!("\n\n{}", text.trim()))
        .collect::<String>();
    Ok(crate::prompt::build_draft_prompt(
        &board.draft,
        ctx.env.vocabulary,
        &material_block(board),
        &reference,
    ))
}

/// 改写的对象：默认是发起时的正文；`source: workspace` 时改工作稿（前面的步骤已经动过它，
/// 如规范化先换了单位名称）。
fn rewrite_source<'b>(board: &'b Board, step: &StepSpec) -> &'b str {
    if step.param_str("source") == Some("workspace") {
        board.workspace.trim()
    } else {
        board.document.trim()
    }
}

fn rewrite_instruction(ctx: &ToolCtx<'_, '_>, step: &StepSpec) -> String {
    let board = &ctx.board;
    let name = step.param_str("prompt").unwrap_or("润色");
    let request = match ctx.env.skill.section(name) {
        Some(template) => board.render(template),
        None => board.request.clone(),
    };
    crate::ai_panel::polish_instruction("", &request, board.selection.as_deref())
}

fn rewrite_prompt(ctx: &ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<String> {
    let board = &ctx.board;
    let current = rewrite_source(board, step);
    if current.is_empty() {
        anyhow::bail!("正文还是空的，没有可改写的内容");
    }
    let instruction = rewrite_instruction(ctx, step);
    let protected = crate::ai_guard::protected_facts_prompt(current, ctx.env.vocabulary);
    Ok(crate::prompt::build_optimize_prompt(
        &board.draft,
        current,
        &format!("{}{protected}", instruction.trim()),
    ))
}

/// 用户原话里要的字数：「压缩到800字」「扩写到1500字左右」「控制在2000字以内」。
fn target_chars(request: &str) -> Option<usize> {
    static TARGET: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(\d{2,5})\s*字").expect("字数正则"));
    TARGET
        .captures(request)
        .and_then(|caps| caps[1].parse().ok())
}

/// 正文字数：不算空白与 Markdown 标记。
fn body_chars(text: &str) -> usize {
    text.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '#' | '*' | '|' | '-' | '>'))
        .count()
}

/// `fit_length`：改写后字数离要求超过 15% 就带着差距再改一次（只再改一次）。
fn fit_length(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<()> {
    let Some(target) = target_chars(&ctx.board.request) else {
        return Ok(());
    };
    let actual = body_chars(&ctx.board.workspace);
    let off = actual.abs_diff(target) * 100 / target.max(1);
    if off <= 15 {
        tool_line(
            ctx,
            "doc.stats",
            Permission::Check,
            format!("字数 {actual}（要求约 {target}）"),
        );
        return Ok(());
    }
    tool_line(
        ctx,
        "doc.stats",
        Permission::Check,
        format!("字数 {actual}，离要求的约 {target} 字差得多，再改一次"),
    );
    check_cancel(ctx)?;
    let direction = if actual > target { "压缩" } else { "扩充" };
    let current = ctx.board.workspace.trim().to_string();
    let protected = crate::ai_guard::protected_facts_prompt(&current, ctx.env.vocabulary);
    let instruction = format!(
        "{}\n\n【字数】现在约 {actual} 字，要求约 {target} 字。请在不改变任何事实的前提下继续{direction}到约 {target} 字。{protected}",
        rewrite_instruction(ctx, step).trim()
    );
    let user = crate::prompt::build_optimize_prompt(&ctx.board.draft, &current, &instruction);
    phase(ctx, format!("{direction}到约 {target} 字…"));
    let system = ctx.board.system_prompt.clone();
    let emit = &mut *ctx.emit;
    let completion = ctx
        .env
        .model
        .complete(ModelRole::Draft, &system, &user, &mut |delta| {
            if let StreamDelta::Reasoning(text) = delta {
                emit(Event::Reasoning(text.to_string()));
            }
        })?;
    ctx.board.truncated |= completion.truncated;
    ctx.board.workspace = crate::prompt::sanitize_model_markdown(&completion.content);
    let after = body_chars(&ctx.board.workspace);
    tool_line(
        ctx,
        "doc.stats",
        Permission::Check,
        format!("再改后字数 {after}（要求约 {target}）"),
    );
    (ctx.emit)(Event::Workspace(ctx.board.workspace.clone()));
    Ok(())
}

/// 逐节生成的提示：文种写法规则 + 技能里那一段（`{evidence}` 只放这一节查到的证据）。
fn section_prompt(ctx: &ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<String> {
    let board = &ctx.board;
    let chars = param(ctx, step, &["evidence_chars"], 6000, 1000..=40000);
    let ids: Vec<usize> = match board.vars.get(FOUND) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_u64)
            .map(|id| id as usize)
            .collect(),
        _ => board.evidence.all_ids(),
    };
    let evidence = if ids.is_empty() {
        "（没有查到可用的证据：只按材料写，具体事实写「【待核实：缺什么】」）".to_string()
    } else {
        board.evidence.format(&ids, chars)
    };
    let locals = [
        ("evidence", evidence),
        ("request", board.request_with_notes()),
    ];
    let body = prompt(ctx, step, "prompt", "写一节", &locals)?;
    Ok(format!(
        "{}\n\n{}",
        crate::prompt::kind_rules(board.draft.kind),
        body
    ))
}

/// 逐节生成时这一节叫什么（`for_each` 的当前项），阶段提示用。
fn section_label(ctx: &ToolCtx<'_, '_>) -> String {
    let item = ["section", "item"]
        .iter()
        .find_map(|name| ctx.board.vars.get(*name))
        .map(crate::agent::board::value_to_text)
        .unwrap_or_default();
    let title = item
        .split(['：', ':', '｜', '|'])
        .next()
        .unwrap_or_default();
    crate::agent::tools::short(title.trim(), 16)
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
