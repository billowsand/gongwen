//! 流程引擎：按技能的 `flow` 在黑板上逐步执行（`docs/ai-agent-workbench.md` 16.4、16.5）。
//!
//! - `step:` 执行一个算子（`ops`），`tool:` 直接调一个工具（`tools`）；
//! - `when` 不满足的步骤跳过；
//! - `for_each` 对列表变量逐项跑子流程；
//! - 算子或工具要问用户时，流程挂起：黑板与下一步的位置交回调用方保存，用户答完后由
//!   [`apply_answers`] 把回答落到黑板，再从 [`Suspension::resume_at`] 接着跑。
//!
//! 引擎只产出工作稿与题目；定稿成提案、交用户接受由调用方负责（红线 1）。

use super::board::Board;
use super::clarify::{self, Action, Question, Reply, Target};
use super::evidence::{self, EvidencePack};
use super::gaps::Ledger;
use super::ops::{self, Flow};
use super::skill::StepSpec;
use super::tools::{self, Env, ToolCtx, ToolUse};
use crate::models::TemplateKind;
use serde_json::Value;

/// `for_each` 默认最多处理几项，防止列表意外很长时跑个没完。
const FOR_EACH_LIMIT: usize = 20;

/// 流程向界面报告的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    Tool(ToolUse),
    /// 阶段提示，会被下一阶段冲掉。
    Phase(String),
    /// 起草正文的增量。
    Content(String),
    /// 开始一次写稿：增量插入前缀与后缀之间，区分重写、追加与局部补写。
    WriteBegin {
        prefix: String,
        suffix: String,
    },
    Reasoning(String),
    /// 工作稿整体换新（补全之后）。
    Workspace(String),
    Note(String),
}

/// 流程停下来问用户。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Suspension {
    pub(crate) questions: Vec<Question>,
    /// 答完从第几步接着跑。
    pub(crate) resume_at: usize,
    /// 选择题选中的值存进哪个变量。
    pub(crate) save_as: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Outcome {
    Done,
    Suspended(Suspension),
}

/// 一次技能运行交付的结果。
#[derive(Debug, Clone)]
pub(crate) struct SkillReport {
    /// 剥掉引用标记的工作稿，交给定稿。
    pub(crate) markdown: String,
    pub(crate) ledger: Ledger,
    pub(crate) evidence: EvidencePack,
    /// 交付后要问用户的题（附在提案上，不挂起）。
    pub(crate) questions: Vec<Question>,
    pub(crate) rounds: usize,
    pub(crate) truncated: bool,
}

impl SkillReport {
    pub(crate) fn from_board(board: &Board) -> Self {
        Self {
            markdown: evidence::strip_citations(&board.workspace),
            ledger: board.ledger.clone(),
            evidence: board.evidence.clone(),
            questions: board.questions.clone(),
            rounds: board.rounds,
            truncated: board.truncated,
        }
    }
}

/// 从第 `start` 步开始执行技能流程。
pub(crate) fn run(
    board: &mut Board,
    env: &Env<'_>,
    start: usize,
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<Outcome> {
    let mut ctx = ToolCtx { board, env, emit };
    // `@` 引用只在第一步之前落一次；挂起后接着跑时已经在黑板上了。
    if start == 0 && !ctx.board.refs.is_empty() {
        super::references::apply(&mut ctx);
    }
    for (index, step) in env.skill.flow.iter().enumerate().skip(start) {
        if let Some((questions, into)) = run_step(&mut ctx, step)? {
            return Ok(Outcome::Suspended(Suspension {
                questions,
                resume_at: index + 1,
                save_as: into.or_else(|| step.save_as.clone()),
            }));
        }
    }
    Ok(Outcome::Done)
}

/// 要问用户的题，以及答案存进哪个变量（None 表示步骤的 `save_as`）。
type Ask = (Vec<Question>, Option<String>);

/// 执行一步；要问用户时返回题目。
fn run_step(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Option<Ask>> {
    ops::check_cancel(ctx)?;
    if let Some(condition) = &step.when
        && !condition_holds(ctx.board, ctx.env, condition)
    {
        return Ok(None);
    }
    match (step.step.as_deref(), step.tool.as_deref()) {
        (Some("for_each"), None) => {
            for_each(ctx, step)?;
            Ok(None)
        }
        (Some(name), None) => {
            let operator =
                ops::find(name).ok_or_else(|| anyhow::anyhow!("不认识的算子「{name}」"))?;
            Ok(match operator(ctx, step)? {
                Flow::Next => None,
                Flow::Suspend(questions) => Some((questions, None)),
                Flow::SuspendInto(questions, var) => Some((questions, Some(var))),
            })
        }
        (None, Some(tool)) => Ok(run_tool(ctx, step, tool).map(|questions| (questions, None))),
        _ => anyhow::bail!("{}的写法不对：step 与 tool 必须二选一", step.label()),
    }
}

/// `tool:` 步骤：替换参数里的变量、调工具、记一行、存变量、并入证据包。工具出错只留
/// 一条说明，流程接着走——后面的步骤可以用 `when: { var: … }` 判断结果有没有拿到。
fn run_tool(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec, id: &str) -> Option<Vec<Question>> {
    let args = step
        .args
        .as_ref()
        .map_or(Value::Null, |args| ctx.board.render_value(args));
    match tools::call(id, ctx, &args) {
        Ok(output) => {
            if let Some(tool) = tools::find(id) {
                (ctx.emit)(Event::Tool(ToolUse::new(
                    tool.id(),
                    tool.permission(),
                    output.summary.clone(),
                )));
            }
            if step.evidence.unwrap_or(output.evidence_by_default) && !output.evidence.is_empty() {
                let label = args
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string();
                ctx.board.evidence.absorb_docs(&label, &output.evidence);
            }
            if let Some(name) = &step.save_as {
                ctx.board.vars.insert(name.clone(), output.value);
            }
            output.suspend
        }
        Err(error) => {
            (ctx.emit)(Event::Note(format!("{}没有做成：{error}", step.label())));
            None
        }
    }
}

/// `for_each`：对列表变量逐项执行子流程，当前项存在 `as` 指定的变量（默认 `item`），
/// 序号存在 `index`。子流程里不能停下来问用户。
fn for_each(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<()> {
    let over = step.param_str("over").unwrap_or_default();
    let items: Vec<Value> = match ctx.board.vars.get(over) {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| Value::String(line.to_string()))
            .collect(),
        _ => {
            (ctx.emit)(Event::Note(format!(
                "没有可逐项处理的「{over}」，跳过这一步"
            )));
            return Ok(());
        }
    };
    let name = step.param_str("as").unwrap_or("item").to_string();
    let limit = step.param_usize("max").unwrap_or(FOR_EACH_LIMIT);
    for (index, item) in items.into_iter().take(limit).enumerate() {
        ctx.board.vars.insert(name.clone(), item);
        ctx.board
            .vars
            .insert("index".into(), Value::from(index + 1));
        for sub in &step.body {
            if run_step(ctx, sub)?.is_some() {
                anyhow::bail!("for_each 的子流程里不能停下来问用户，请把提问挪到 for_each 之外");
            }
        }
    }
    Ok(())
}

/// `when` 条件：`has_sources` 等名字、`{ kind: [文种…] }`、`{ var: 名字 }`、`{ not: 条件 }`，
/// 列表表示全部满足。写法由 `skill::validate` 把关，这里认不出的一律当不满足。
pub(crate) fn condition_holds(board: &Board, env: &Env<'_>, condition: &Value) -> bool {
    match condition {
        Value::String(name) => match name.as_str() {
            // 知识库启用了，或技能声明了已配置的数据接口。
            "has_sources" => {
                env.kb.enabled()
                    || env.apis.endpoints.iter().any(|endpoint| {
                        env.skill.allows_tool(&format!("http.call:{}", endpoint.id))
                    })
            }
            "has_text" => !board.document.trim().is_empty(),
            "has_selection" => board
                .selection
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            "has_evidence" => !board.evidence.is_empty(),
            _ => false,
        },
        Value::Array(all) => all.iter().all(|c| condition_holds(board, env, c)),
        Value::Object(map) => map.iter().all(|(key, value)| match key.as_str() {
            "kind" => kind_matches(board.draft.kind, value),
            "var" => value.as_str().is_some_and(|name| var_present(board, name)),
            "not" => !condition_holds(board, env, value),
            _ => false,
        }),
        _ => false,
    }
}

fn kind_matches(kind: TemplateKind, value: &Value) -> bool {
    let matches = |name: &str| {
        let name = name.trim();
        kind.label() == name || crate::manuscript::kind_to_str(kind) == name
    };
    match value {
        Value::String(name) => matches(name),
        Value::Array(names) => names.iter().filter_map(Value::as_str).any(matches),
        _ => false,
    }
}

fn var_present(board: &Board, name: &str) -> bool {
    match board.vars.get(name) {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

/// 把用户对挂起题目的回答落到黑板上，返回用户选中的文种（切文种由界面线程执行，
/// 与在要素区下拉框里选是同一回事；引擎不碰要素）。
///
/// - 动笔前澄清：回答记作已确认信息，标记已澄清；
/// - 缺口题：按确定性规则改工作稿与台账；
/// - 选择题：选中项的值存进 `save_as`。
pub(crate) fn apply_answers(
    board: &mut Board,
    suspension: &Suspension,
    replies: &[(usize, Reply)],
) -> Option<TemplateKind> {
    let questions = &suspension.questions;
    let mut kind = None;
    if questions.iter().any(|q| q.target.is_predraft()) {
        let (picked, notes) = clarify::resolve_predraft(questions, replies, board.draft.kind);
        board.notes.extend(notes);
        board.clarified = true;
        kind = picked;
    }
    if questions.iter().any(|q| matches!(q.target, Target::Gap(_))) {
        let workspace = std::mem::take(&mut board.workspace);
        board.workspace =
            clarify::apply_gap_replies(&workspace, &mut board.ledger, questions, replies);
    }
    for question in questions.iter().filter(|q| q.target == Target::Pick) {
        let Some((_, reply)) = replies.iter().find(|(id, _)| *id == question.id) else {
            continue;
        };
        // 选了值为空的选项（「就按这个」）或跳过：变量保持现值。
        let value = match reply {
            Reply::Choice(index) => match question.choices.get(*index).map(|c| &c.action) {
                Some(Action::Pick(value)) => value.clone(),
                _ => Value::Null,
            },
            Reply::Custom(text) => Value::String(text.clone()),
            Reply::Skip => Value::Null,
        };
        if let Some(name) = &suspension.save_as
            && !value.is_null()
        {
            board.vars.insert(name.clone(), value);
        }
    }
    kind
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
