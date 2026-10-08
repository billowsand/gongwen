//! 动笔之前：澄清、预研、检索。

use super::{
    Flow, assist, assist_json, check_cancel, fetch_into, has_sources, note, param, phase, prompt,
    tool_line,
};
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx};
use crate::agent::{clarify, elements};
use serde_json::Value;

/// 预研列出的检索词存在这个变量里，`retrieve` 默认从这里取。
const QUERIES: &str = "queries";
/// 最近一次 `retrieve` 查到的证据编号。
pub(super) const FOUND: &str = "found";

/// `clarify`：程序规则 + 模型判断，只问会让整篇写偏的事；有题就挂起。已经问过（黑板标了
/// 已澄清）就跳过。参数 `max`（技能参数 `pre_questions`），提示词默认「动笔前澄清」。
///
/// 再按文种的六要素清单查一遍（`elements.rs`）：时限、对象、联系人这些只有起草人知道的事，
/// 动笔前就问，答案写进第一稿。参数 `elements`（为否时不查）、`element_questions`
/// （最多几道），提示词 `elements_prompt`（默认「要素检查」，技能里没写就用内置的一份）。
///
/// 分两关（`clarify.rs` 开头的分层）：先定文种——要求点名的文种与当前对不上，或模型问的是
/// 文种，就**只问这一题**，答完回到本步重做（`Flow::SuspendAgain`）；文种定了才按它出方向题
/// 与六要素题。要素区里改过文种（当前文种 ≠ 定下的文种）时，以旧文种为前提的澄清作废重问。
pub(super) fn clarify(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let kind = ctx.board.draft.kind;
    if ctx.board.premise.is_some_and(|premise| premise != kind) {
        ctx.board.premise = None;
        ctx.board.clarified = false;
    }
    if ctx.board.clarified {
        return Ok(Flow::Next);
    }
    let max = param(ctx, step, &["max", "pre_questions"], 3, 0..=5);
    let element_max = if step.params.get("elements").and_then(Value::as_bool) == Some(false) {
        0
    } else {
        param(ctx, step, &["element_questions"], 4, 0..=6)
    };
    let checklist = elements::checklist(kind);
    if max == 0 && (element_max == 0 || checklist.is_empty()) {
        return Ok(Flow::Next);
    }
    // 第一关：定文种（程序判断，不调模型）。
    if ctx.board.premise.is_none()
        && let Some(question) = clarify::kind_question(&ctx.board.request, kind)
    {
        return Ok(ask_kind(ctx, question));
    }
    phase(ctx, "动笔前检查要求是否明确…");
    let mut direction = Vec::new();
    if max > 0 {
        let locals = [
            ("max", max.to_string()),
            ("title", ctx.board.draft.title_hint.trim().to_string()),
            ("request", ctx.board.request_with_notes()),
        ];
        let text = prompt(ctx, step, "prompt", "动笔前澄清", &locals)?;
        // 要 JSON（端点支持就由服务端保证格式）；不支持时模型照提示词写「问题｜选项」，两种都认。
        let text = format!("{text}\n\n{}", clarify::JSON_HINT);
        let reply = assist_json(ctx, &text, clarify::questions_schema())?;
        let parsed = clarify::parse_model_questions(&reply, max);
        // 模型问的是文种：同样是第一关，单独先问；同批其余的题以旧文种为前提，不要。
        if ctx.board.premise.is_none()
            && let Some(question) = clarify::model_kind_question(&parsed, kind)
        {
            return Ok(ask_kind(ctx, question));
        }
        direction = clarify::predraft_questions(parsed, max);
    }
    // 第一关过了：后面的题都以这个文种为前提。
    ctx.board.premise = Some(kind);
    let mut asked_elements = Vec::new();
    if element_max > 0 && !checklist.is_empty() {
        let request = format!(
            "{}
{}",
            ctx.board.draft.title_hint.trim(),
            ctx.board.request_with_notes()
        );
        let name = step.param_str("elements_prompt").unwrap_or("要素检查");
        let template = ctx.env.skill.section(name).unwrap_or(elements::PROMPT);
        let locals = [
            ("kind", ctx.board.draft.kind.label().to_string()),
            ("checklist", elements::checklist_text(&checklist)),
            ("today", crate::prompt::TimeContext::now().today),
            ("request", request.clone()),
        ];
        let text = ctx.board.render_with(template, &locals);
        let reply = assist(ctx, &text)?;
        let asked =
            elements::questions(kind, &request, &reply, (1, element_max), ctx.env.vocabulary);
        if !asked.is_empty() {
            tool_line(
                ctx,
                "check.elements",
                Permission::Check,
                format!(
                    "六要素：{} 没讲清楚",
                    asked
                        .iter()
                        .filter_map(|q| match q.target {
                            clarify::Target::Element(element) => Some(element.label()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("、")
                ),
            );
        }
        asked_elements = asked;
    }
    let questions = clarify::merge_predraft(direction, asked_elements);
    if questions.is_empty() {
        return Ok(Flow::Next);
    }
    tool_line(
        ctx,
        "ask.choice",
        Permission::AskUser,
        format!(
            "按{}动笔前有 {} 个问题要你确认",
            kind.label(),
            questions.len()
        ),
    );
    Ok(Flow::Suspend(questions))
}

/// 只问定文种这一题，答完回到 `clarify` 重做：其余的题按定下的文种再出。
fn ask_kind(ctx: &mut ToolCtx<'_, '_>, question: clarify::Question) -> Flow {
    tool_line(
        ctx,
        "ask.choice",
        Permission::AskUser,
        "先定文种：其余问题按定下的文种再出".to_string(),
    );
    Flow::SuspendAgain(vec![question])
}

/// `plan`：让模型列一份清单存进变量。
///
/// - `mode: queries`（默认）：要到知识库里查清的问题，连同用户原话存进 `save_as`（默认
///   `queries`）；参数 `max`（技能参数 `research_questions`），为 0 时只存原话；提示词默认「预研」；
/// - `mode: list`：任意清单（来函事项、材料要点……），存进 `save_as`（默认 `items`）；
/// - `mode: outline`：大纲，每行一节，存进 `save_as`（默认 `outline`）；`confirm`（默认是）时
///   停下来让用户在框里改，答案存回同一个变量；
/// - `mode: split`：**不调模型**，把用户原话按行（只有一段时按句）拆成要点，存进 `save_as`
///   （默认 `items`）——材料里的事实原样保留，模型没机会在这一步改错；`confirm` 默认是。
/// - `mode: title`：沿用具体标题提示，否则按要求拟题，存进 `save_as`（默认 `report_title`）；
///   只供工作稿使用，不回写文档要素。
pub(super) fn plan(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    match step.param_str("mode").unwrap_or("queries") {
        "queries" => plan_queries(ctx, step),
        "list" => plan_list(ctx, step, false),
        "outline" => plan_list(ctx, step, true),
        "split" => plan_split(ctx, step),
        "title" => plan_title(ctx, step),
        other => {
            anyhow::bail!(
                "plan 不认识 mode「{other}」（可用 queries / list / outline / split / title）"
            )
        }
    }
}

/// 文种名与格式占位文字不能作为报告题名。
fn is_generic_title(title: &str) -> bool {
    matches!(
        title.trim(),
        "" | "研究报告"
            | "调研报告"
            | "政策研究报告"
            | "咨询报告"
            | "决策参考"
            | "报告题名"
            | "报告名称"
            | "本报告"
    )
}

fn plan_title(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let hint = ctx.board.draft.title_hint.trim();
    let title = if is_generic_title(hint) {
        phase(ctx, "根据写作要求拟定报告题名…");
        let locals = [("request", ctx.board.request_with_notes())];
        let text = prompt(ctx, step, "prompt", "报告题名", &locals)?;
        let reply = assist(ctx, &text)?;
        let title = crate::prompt::sanitize_model_markdown(&reply);
        let title = title.trim().trim_start_matches('#').trim();
        if title.lines().count() != 1 || is_generic_title(title) {
            anyhow::bail!("模型没有给出具体的报告题名，请填写报告名称后重试");
        }
        title.to_string()
    } else {
        hint.to_string()
    };
    let name = step.save_as.as_deref().unwrap_or("report_title");
    ctx.board.vars.insert(name.into(), Value::String(title));
    Ok(Flow::Next)
}

/// 模型回复 → 一行一条，去掉编号与空行、「无」。
fn lines_of(reply: &str, max: usize) -> Vec<String> {
    reply
        .lines()
        .map(|line| {
            let line = line.trim().trim_start_matches(['-', '*', '#', ' ']).trim();
            let line = clarify::strip_numbering(line).trim();
            // 模型照抄格式说明里的「事项：」「章标题：」前缀时去掉。
            ["事项：", "事项:", "章标题：", "要点："]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .unwrap_or(line)
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty() && line != "无")
        .take(max)
        .collect()
}

/// 以冒号收尾、说的是「整理成 / 写成 / 起草」的一行，是写作要求而不是材料。
fn is_instruction(line: &str) -> bool {
    line.ends_with(['：', ':'])
        && ["整理", "写成", "写一", "起草", "形成", "拟", "改成"]
            .iter()
            .any(|verb| line.contains(verb))
}

fn plan_split(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let max = param(ctx, step, &["max"], 30, 1..=60);
    let request = ctx.board.request.clone();
    // `@` 引用的材料并进来时带「【引用材料《…》】」抬头，后面跟「【要求】」——
    // 抬头不是要点，要求那一段也不是材料。
    let material = request
        .split_once("\n【要求】")
        .map_or(request.as_str(), |(material, _)| material);
    let mut items: Vec<String> = material
        .lines()
        // 引用的稿件开头是「# 标题」，那是文件名，不是要点。
        .filter(|line| !line.trim_start().starts_with("# "))
        .map(|line| line.trim().trim_start_matches(['-', '*', '•', '·']).trim())
        .filter(|line| !line.is_empty())
        .filter(|line| !(line.starts_with('【') && line.ends_with('】')))
        .map(str::to_string)
        .collect();
    // 开头那句「根据以下纪要整理成一份通知：」是要求，不是材料。
    if items.len() > 1 && is_instruction(&items[0]) {
        items.remove(0);
    }
    if items.len() < 2 {
        items = crate::agent::gaps::sentence_spans(&request)
            .into_iter()
            .map(|span| request[span].trim().to_string())
            .filter(|sentence| !sentence.is_empty())
            .collect();
    }
    items.truncate(max);
    if items.is_empty() {
        anyhow::bail!("材料是空的，没有可拆的要点");
    }
    tool_line(
        ctx,
        "plan",
        Permission::Read,
        format!("按原文拆出要点 {} 条（不经模型）", items.len()),
    );
    let name = step.save_as.as_deref().unwrap_or("items").to_string();
    finish_list(ctx, step, items, name, "要点", true)
}

fn plan_list(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec, outline: bool) -> anyhow::Result<Flow> {
    let (default_max, default_var, label) = if outline {
        (8, "outline", "大纲")
    } else {
        (10, "items", "清单")
    };
    let max = param(ctx, step, &["max"], default_max, 1..=20);
    phase(ctx, format!("列{label}…"));
    let locals = [
        ("max", max.to_string()),
        ("request", ctx.board.request_with_notes()),
    ];
    let text = prompt(
        ctx,
        step,
        "prompt",
        if outline { "大纲" } else { "清单" },
        &locals,
    )?;
    let reply = assist(ctx, &text)?;
    let items = lines_of(&reply, max);
    if items.is_empty() {
        anyhow::bail!("模型没有列出{label}");
    }
    tool_line(
        ctx,
        "plan",
        Permission::Read,
        format!("列出{label} {} 条", items.len()),
    );
    let name = step.save_as.as_deref().unwrap_or(default_var).to_string();
    finish_list(ctx, step, items, name, label, outline)
}

/// 清单存进变量；要确认时停下来让用户在框里改。
fn finish_list(
    ctx: &mut ToolCtx<'_, '_>,
    step: &StepSpec,
    items: Vec<String>,
    name: String,
    label: &str,
    confirm_by_default: bool,
) -> anyhow::Result<Flow> {
    ctx.board.vars.insert(
        name.clone(),
        Value::Array(items.iter().cloned().map(Value::String).collect()),
    );
    let confirm = step
        .params
        .get("confirm")
        .and_then(Value::as_bool)
        .unwrap_or(confirm_by_default);
    if !confirm {
        return Ok(Flow::Next);
    }
    tool_line(
        ctx,
        "ask.choice",
        Permission::AskUser,
        format!("{label}请你确认"),
    );
    let question = clarify::Question {
        id: 1,
        text: format!("按这个{label}写吗？可以直接在框里改，每行一条"),
        choices: vec![clarify::Choice {
            label: "就按这个写".into(),
            detail: String::new(),
            recommended: true,
            action: clarify::Action::Pick(Value::Null),
        }],
        custom_hint: Some("每行一条".into()),
        prefill: items.join("\n"),
        skippable: false,
        target: clarify::Target::Pick,
    };
    Ok(Flow::SuspendInto(vec![question], name))
}

fn plan_queries(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
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
        let planned = lines_of(&reply, max);
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

/// `retrieve`：逐个检索词查知识库与 `apis:` 列出的数据接口，结果并入证据包。检索词取
/// `from` 指定的变量（默认 `queries`），没有就用用户原话。一个来源都没有时只留一条说明。
/// 这次查到的证据编号存进 `found`，逐节生成时只用它们。
pub(super) fn retrieve(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    // 用户 `@` 引用的文章每一节都带着，检索结果排在后面。
    let pinned: Vec<Value> = ctx.board.pinned.iter().map(|id| Value::from(*id)).collect();
    ctx.board.vars.insert(FOUND.into(), Value::Array(pinned));
    if !has_sources(ctx, step) {
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
    let mut found: Vec<usize> = ctx.board.pinned.clone();
    for query in queries.iter().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        check_cancel(ctx)?;
        for (_, id) in fetch_into(ctx, step, query) {
            if !found.contains(&id) {
                found.push(id);
            }
        }
    }
    ctx.board.vars.insert(
        FOUND.into(),
        Value::Array(found.into_iter().map(Value::from).collect()),
    );
    Ok(Flow::Next)
}
