//! 流程引擎：按技能的 `flow` 在黑板上逐步执行（`docs/ai-agent-workbench.md` 16.4、16.5）。
//!
//! - `step:` 执行一个算子（`ops`），`tool:` 直接调一个工具（`tools`）；
//! - `when` 不满足的步骤跳过；
//! - `for_each` 对列表变量逐项跑子流程；
//! - 算子或工具要问用户时，流程挂起：挂起就是一种检查点（`Reason::Ask`），黑板与下一步的
//!   位置都在里面，用户答完后由 [`apply_answers`] 把回答落到黑板，再从 `checkpoint.at` 接着跑。
//!
//! 引擎只产出工作稿与题目；定稿成提案、交用户接受由调用方负责（红线 1）。

use super::board::Board;
use super::checkpoint::{Checkpoint, Reason, StepPath};
use super::clarify::{self, Action, Question, Reply, Target};
use super::decision::Decision;
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

/// 流程停下来问用户。挂起就是一种检查点（`Reason::Ask`）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Suspension {
    /// 挂起时的现场；答完把回答落到 `checkpoint.board`，从 `checkpoint.at` 接着跑。
    pub(crate) checkpoint: Checkpoint,
    pub(crate) questions: Vec<Question>,
    /// 选择题选中的值存进哪个变量。
    pub(crate) save_as: Option<String>,
    /// 这一批题的形态：界面按它画卡片（`docs/decision-modules.md`）。旧会话没有，读回是一批
    /// 选择题，由 `decision::upgrade` 认出旧的大纲确认。
    #[serde(default)]
    pub(crate) decision: Decision,
}

#[derive(Debug, Clone)]
pub(crate) enum Outcome {
    Done,
    /// 挂起带着整块黑板的检查点，装箱别让 `Done` 也跟着占地方。
    Suspended(Box<Suspension>),
}

/// 一次技能运行交付的结果。
#[derive(Debug, Clone)]
pub(crate) struct SkillReport {
    pub(crate) cited_ids: Vec<usize>,
    pub(crate) generated_at: String,
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
        let workspace = evidence::normalize_citations(
            &board.workspace,
            &board.draft.research.bibliography_content,
        );
        Self {
            markdown: evidence::strip_citations(&workspace),
            cited_ids: evidence::citation_ids(&workspace),
            generated_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            ledger: board.ledger.clone(),
            evidence: board.evidence.clone(),
            questions: board.questions.clone(),
            rounds: board.rounds,
            truncated: board.truncated,
        }
    }
}

#[test]
fn report_collects_only_workspace_citations_before_stripping() {
    let mut board = Board {
        workspace: "一。[K1]三。[K3]".into(),
        ..Default::default()
    };
    let docs = (1..=5)
        .map(|i| evidence::EvidenceDoc {
            key: i.to_string(),
            title: "资料".into(),
            section: String::new(),
            kind_label: String::new(),
            text: "材料".into(),
        })
        .collect::<Vec<_>>();
    board.evidence.absorb_docs("材料", &docs);
    let report = SkillReport::from_board(&board);
    assert_eq!(report.cited_ids, [1, 3]);
    assert_eq!(report.markdown, "一。三。");
    assert_eq!(report.evidence.items().len(), 5);
}

#[test]
fn report_removes_miswritten_evidence_even_without_a_cite_step() {
    let mut board = Board {
        workspace: "依据[@K1]，另引[@K2; @unknown]，不存在的证据[@K99]。".into(),
        ..Default::default()
    };
    board.draft.research.bibliography_content = "@book{K2, title = {真实文献}}".into();
    let report = SkillReport::from_board(&board);
    assert_eq!(report.markdown, "依据，另引[@K2; @unknown]，不存在的证据。");
    assert_eq!(report.cited_ids, [1, 99]);
    assert!(board.workspace.contains("[@K1]"), "交付不改原始工作稿");
}

/// 从 `at` 接着执行技能流程；空路径表示全新开跑。
pub(crate) fn run(
    board: &mut Board,
    env: &Env<'_>,
    at: &[usize],
    emit: &mut dyn FnMut(Event),
) -> anyhow::Result<Outcome> {
    let mut ctx = ToolCtx { board, env, emit };
    // `@` 引用只在全新开跑（空路径）时落一次；从检查点接着跑时它们已经在黑板上了。
    if at.is_empty() && !ctx.board.refs.is_empty() {
        super::references::apply(&mut ctx);
    }
    let first = at.first().copied().unwrap_or(0);
    for (index, step) in env.skill.flow.iter().enumerate().skip(first) {
        // 只有续跑的第一步可能落在 `for_each` 中间，带上项起点；其余步从头跑。
        let item = if index == first {
            at.get(1).copied().unwrap_or(0)
        } else {
            0
        };
        if let Some(ask) = run_step(&mut ctx, step, index, item)? {
            // 要挂起：先存检查点（reason = Ask）再返回，挂起就是一种检查点。上游决策的题
            // 答完要回到这一步重做（下游的题以它为前提再出），续跑位置就落在本步。
            let at = if ask.again { index } else { index + 1 };
            let checkpoint = save(
                &mut ctx,
                vec![at],
                Reason::Ask,
                format!("等你回答：{}", step.label()),
            );
            return Ok(Outcome::Suspended(Box::new(Suspension {
                checkpoint,
                questions: ask.questions,
                save_as: ask.into.or_else(|| step.save_as.clone()),
                decision: ask.decision,
            })));
        }
        // 每步成功之后落一份检查点；不在步骤之前存（内容与上一份相同）。
        save(
            &mut ctx,
            vec![index + 1],
            Reason::Step,
            format!("已完成 {}", step.label()),
        );
    }
    Ok(Outcome::Done)
}

/// 一步成功之后落一份检查点，返回它（挂起时随 `Suspension` 交回调用方）。
/// 写盘失败不中断流程，只记一条说明——与工具出错同一个风格。
fn save(ctx: &mut ToolCtx<'_, '_>, at: StepPath, reason: Reason, label: String) -> Checkpoint {
    let checkpoint = Checkpoint {
        at,
        reason,
        label,
        board: ctx.board.clone(),
        partial: false,
    };
    if let Err(error) = ctx.env.ckpt.save(&checkpoint) {
        (ctx.emit)(Event::Note(format!("检查点没存上：{error}")));
    }
    checkpoint
}

/// 要问用户的题。
struct Ask {
    questions: Vec<Question>,
    /// 答案存进哪个变量（None 表示步骤的 `save_as`）。
    into: Option<String>,
    /// 答完回到这一步重做（[`Flow::SuspendAgain`]）。
    again: bool,
    decision: Decision,
}

impl Ask {
    fn new(questions: Vec<Question>, into: Option<String>) -> Self {
        Self {
            questions,
            into,
            again: false,
            decision: Decision::Choose,
        }
    }
}

/// 执行一步；要问用户时返回题目。
fn run_step(
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    index: usize,
    item_from: usize,
) -> anyhow::Result<Option<Ask>> {
    ops::check_cancel(ctx)?;
    if let Some(condition) = &step.when
        && !condition_holds(ctx.board, ctx.env, condition)
    {
        return Ok(None);
    }
    match (step.step.as_deref(), step.tool.as_deref()) {
        (Some("for_each"), None) => {
            for_each(ctx, step, index, item_from)?;
            Ok(None)
        }
        (Some(name), None) => {
            let operator =
                ops::find(name).ok_or_else(|| anyhow::anyhow!("不认识的算子「{name}」"))?;
            Ok(match operator(ctx, step)? {
                Flow::Next => None,
                Flow::Suspend(questions) => Some(Ask::new(questions, None)),
                Flow::Confirm(questions, list) => Some(Ask {
                    decision: Decision::ConfirmList(list.clone()),
                    ..Ask::new(questions, Some(list.var))
                }),
                Flow::SuspendAgain(questions) => Some(Ask {
                    again: true,
                    ..Ask::new(questions, None)
                }),
            })
        }
        (None, Some(tool)) => {
            Ok(run_tool(ctx, step, tool).map(|questions| Ask::new(questions, None)))
        }
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
fn for_each(
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    at: usize,
    from: usize,
) -> anyhow::Result<()> {
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
    for (index, item) in items.into_iter().take(limit).enumerate().skip(from) {
        ctx.board.vars.insert(name.clone(), item);
        ctx.board
            .vars
            .insert("index".into(), Value::from(index + 1));
        for sub in &step.body {
            if run_step(ctx, sub, 0, 0)?.is_some() {
                anyhow::bail!("for_each 的子流程里不能停下来问用户，请把提问挪到 for_each 之外");
            }
        }
        // 每做完一项落一份检查点：长循环中断后从下一项接着跑。
        save(
            ctx,
            vec![at, index + 1],
            Reason::Step,
            format!("已完成 {} 的第 {} 项", step.label(), index + 1),
        );
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
/// - 定文种：记下定下的文种（`premise`），不标记已澄清——流程回到澄清这一步按它出题；
/// - 动笔前澄清：回答记作已确认信息，标记已澄清；
/// - 缺口题：按确定性规则改工作稿与台账；
/// - 选择题：选中项的值存进 `save_as`。
/// - 自主步骤提的题：回答记作已确认信息（跳过的也记一句），流程回到这一步重做。
pub(crate) fn apply_answers(
    board: &mut Board,
    questions: &[Question],
    save_as: Option<&str>,
    replies: &[(usize, Reply)],
) -> Option<TemplateKind> {
    let mut kind = None;
    if questions.iter().any(|q| q.target == Target::Kind) {
        // 定文种是第一关：只定下前提，不算澄清过——流程回到澄清这一步，按定下的文种再出题。
        let (picked, _) = clarify::resolve_predraft(questions, replies, board.draft.kind);
        board.premise = Some(picked.unwrap_or(board.draft.kind));
        kind = picked;
    } else if questions.iter().any(|q| q.target.is_predraft()) {
        let (picked, notes) = clarify::resolve_predraft(questions, replies, board.draft.kind);
        board.notes.extend(notes);
        board.clarified = true;
        kind = picked;
    }
    if questions.iter().any(|q| q.target == Target::Agent) {
        board
            .notes
            .extend(clarify::resolve_agent(questions, replies));
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
        if let Some(name) = save_as
            && !value.is_null()
        {
            board.vars.insert(name.to_string(), value);
        }
    }
    kind
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
