//! 收尾：核验引用、出题。

use super::{Flow, assist, check_cancel, param, phase, prompt, squash, tool_line};
use crate::agent::clarify;
use crate::agent::engine::Event;
use crate::agent::evidence;
use crate::agent::skill::StepSpec;
use crate::agent::tools::{self, Permission, ToolCtx, short};
use crate::ai_guard::FactKind;
use serde_json::{Map, Value};

/// `verify`：带引用的句子逐句对照引到的片段，不支持的去掉引用，其中证据包里都找不到的
/// 事实交用户确认。参数 `max`（技能参数 `max_checks`），提示词默认「核验」。证据包为空时跳过。
pub(super) fn verify(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let max = param(ctx, step, &["max", "max_checks"], 8, 0..=30);
    if max == 0 || ctx.board.evidence.is_empty() {
        return Ok(Flow::Next);
    }
    let mut cited = evidence::cited_sentences(&ctx.board.workspace);
    // 有具体事实的句子先核：引错了事实比引错了说法要紧。
    cited.sort_by_key(|(sentence, _)| {
        crate::ai_guard::extract_key_facts(&evidence::strip_citations(sentence), &[]).is_empty()
    });
    let mut checked = 0;
    for (sentence, ids) in cited.into_iter().take(max) {
        check_cancel(ctx)?;
        let ids: Vec<usize> = ids
            .into_iter()
            .filter(|id| ctx.board.evidence.get(*id).is_some())
            .collect();
        if ids.is_empty() {
            continue;
        }
        phase(ctx, "核验引用…");
        let plain = evidence::strip_citations(&sentence);
        let locals = [
            ("sentence", plain.clone()),
            ("evidence", ctx.board.evidence.format(&ids, 4000)),
        ];
        let text = prompt(ctx, step, "prompt", "核验", &locals)?;
        let reply = assist(ctx, &text)?;
        checked += 1;
        if !reply.contains("不支持") {
            continue;
        }
        let workspace = &mut ctx.board.workspace;
        if let Some(pos) = workspace.find(&sentence) {
            workspace.replace_range(pos..pos + sentence.len(), &plain);
        }
        // 只有整个证据包里都找不到的事实才交用户：实测里模型常把编号标错（事实出自 K7
        // 却标了 K3），这时事实本身有出处，只是引用对不上，不该拿来打扰用户。
        let pack = &ctx.board.evidence;
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
            ctx.board.ledger.add_unsupported(&fact.value, &plain);
        }
        tool_line(
            ctx,
            "check.references",
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
        );
    }
    if checked > 0 {
        (ctx.emit)(Event::Workspace(ctx.board.workspace.clone()));
        tool_line(
            ctx,
            "check.references",
            Permission::Check,
            format!("核验了 {checked} 句带引用的话"),
        );
    }
    Ok(Flow::Next)
}

/// `ask`：出选择题。
///
/// - 默认：把台账里要用户确认的缺口出成题，附在提案上交付（不挂起，答完按确定性规则改稿）；
///   参数 `max`（技能参数 `batch_questions`）；
/// - `mode: wait`：同样的题，但流程停下等回答，答完接着跑后面的步骤；
/// - `choose_from: 变量`：从列表变量里出一道选择题（问法写在 `question`），挂起等回答，
///   选中项的 id 存进 `save_as`。
pub(super) fn ask(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    if let Some(from) = step.param_str("choose_from") {
        let mut args = Map::new();
        let question = step.param_str("question").unwrap_or("选一个");
        args.insert("question".into(), Value::String(ctx.board.render(question)));
        args.insert("from".into(), Value::String(from.to_string()));
        for key in ["custom", "skip"] {
            if let Some(value) = step.params.get(key) {
                args.insert(key.into(), value.clone());
            }
        }
        let tool = tools::find("ask.choice").expect("ask.choice 是内置工具");
        let output = tool.run(ctx, &args).map_err(anyhow::Error::msg)?;
        tool_line(ctx, "ask.choice", Permission::AskUser, output.summary);
        return Ok(output.suspend.map_or(Flow::Next, Flow::Suspend));
    }
    let max = param(ctx, step, &["max", "batch_questions"], 4, 1..=10);
    let questions = clarify::gap_questions(&ctx.board.ledger, max);
    if questions.is_empty() {
        return Ok(Flow::Next);
    }
    tool_line(
        ctx,
        "ask.choice",
        Permission::AskUser,
        format!("还有 {} 处需要你确认", questions.len()),
    );
    if step.param_str("mode") == Some("wait") {
        return Ok(Flow::Suspend(questions));
    }
    ctx.board.questions = questions;
    Ok(Flow::Next)
}
