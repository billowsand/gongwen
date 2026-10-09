//! 动笔之前：澄清、预研、检索。

use super::{
    Flow, assist, assist_json, check_cancel, fetch_into, has_sources, note, param, phase, prompt,
    tool_line,
};
use crate::agent::decision::ListConfirm;
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx};
use crate::agent::{clarify, elements};
use crate::element_fields;
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
    let field_max = if step.params.get("elements").and_then(Value::as_bool) == Some(false) {
        0
    } else {
        param(ctx, step, &["field_questions"], 4, 0..=6)
    };
    let checklist = elements::checklist(kind);
    let fields = element_fields::prompt_fields(&ctx.board.draft);
    if max == 0
        && (element_max == 0 || checklist.is_empty())
        && (field_max == 0 || fields.is_empty())
    {
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
    let mut asked_fields: Vec<clarify::Question> = Vec::new();
    if field_max > 0 {
        // 表单要素（`element_fields`）：先「要素抽取」——它的建议要拼进六要素检查的
        // 原文（「何人」才算已给），所以放在六要素检查前面。字段题单独占
        // `field_questions` 名额（默认 4，红头呈批件的必填要素正好 4 项；必填优先），不挤六要素的名额：点选题答起来快，
        // 挤掉「何时」「何法」代价更大。
        // 重新澄清（文种改过）时，以旧文种为前提的建议作废。
        ctx.board.field_suggestions.clear();
        let mut plan = element_fields::FieldPlan::default();
        if !fields.is_empty() {
            let source = extraction_source(ctx.board);
            // 所有字段都已填、原文里又一个名称都对不上词库：抽了也产不出建议，不调模型。
            let all_filled = fields
                .iter()
                .all(|field| !field.read(&ctx.board.draft).trim().is_empty());
            if !(all_filled && !any_vocabulary_name(&source, ctx.env.vocabulary)) {
                phase(ctx, "动笔前核对表单要素…");
                let name = step.param_str("fields_prompt").unwrap_or("要素抽取");
                let template = ctx
                    .env
                    .skill
                    .section(name)
                    .unwrap_or(element_fields::PROMPT);
                let locals = [
                    ("kind", kind.label().to_string()),
                    ("fields", element_fields::prompt_fields_text(&fields, kind)),
                    ("source", source.clone()),
                ];
                let text = ctx.board.render_with(template, &locals);
                let reply = assist(ctx, &text)?;
                plan = element_fields::plan(
                    &ctx.board.draft,
                    &reply,
                    &source,
                    ctx.env.vocabulary,
                    field_max,
                );
            }
        }
        // 不出题直接建议的：立刻进建议清单，不挡起草；同时记一句已确认信息交给起草，
        // 正文措辞跟着对上（冲突的「替换」建议不记，免得带偏起草）。
        for suggestion in &plan.suggestions {
            if !suggestion.conflict {
                ctx.board.notes.push(format!(
                    "{}：{}（{}；要素建议，待采纳进表单）",
                    suggestion.field.label(),
                    suggestion.value,
                    suggestion.source
                ));
            }
        }
        if !plan.suggestions.is_empty() || !plan.questions.is_empty() {
            tool_line(
                ctx,
                "check.fields",
                Permission::Check,
                format!(
                    "表单要素：建议 {} 条，待问 {} 题",
                    plan.suggestions.len(),
                    plan.questions.len()
                ),
            );
        }
        if !plan.suggestions.is_empty() {
            // 建议卡立刻出现在这一轮上，不挡起草（流程可能不挂起、一直跑到底，
            // 不能只靠答题后从黑板取）。
            (ctx.emit)(crate::agent::engine::Event::FieldSuggestions(
                plan.suggestions.clone(),
            ));
        }
        ctx.board
            .field_suggestions
            .extend(plan.suggestions.iter().cloned());
        asked_fields = plan.questions;
    }
    if element_max > 0 && !checklist.is_empty() {
        // 已填要素（表单已填 + 本批建议）拼进原文，模型才能把「何人」判为已给。
        let filled = element_fields::filled_summary(&ctx.board.draft, &ctx.board.field_suggestions);
        let filled_line = if filled.is_empty() {
            String::new()
        } else {
            format!("{filled}\n")
        };
        let request = format!(
            "{}
{filled_line}{}",
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
        // 字段题覆盖「何人」（主送、承办、联系人、呈报领导任一项出了题）时不再出「何人」题。
        // 电话通知例外：它的「何人」还包括联系人，而电话通知表单没有联系人字段。
        let covers_who = kind != crate::models::TemplateKind::PhoneNotice
            && asked_fields.iter().any(|q| {
                matches!(
                    q.target,
                    clarify::Target::Field(
                        element_fields::FieldId::Recipient
                            | element_fields::FieldId::ResponsibleUnit
                            | element_fields::FieldId::ContactPerson
                            | element_fields::FieldId::ReportingLeaders
                    )
                )
            });
        let asked = elements::questions(
            kind,
            &request,
            &reply,
            (1, element_max),
            ctx.env.vocabulary,
            covers_who,
        );
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
    let questions = clarify::merge_predraft(direction, [asked_fields, asked_elements].concat());
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

/// 要素抽取的原文里，`@` 引用文章（当证据并入的）每篇最多带多少字，防超长。
/// 按技能声明并进 `request` 的引用材料（references.rs）不截，与六要素检查一致。
const FIELD_REF_CAP: usize = 1500;

/// 「要素抽取」的原文：标题提示 + 要求与已确认信息 + `@` 引用文章的正文。
/// 摘录核对（`element_fields::plan`）用的必须正是给模型看的这份。
fn extraction_source(board: &crate::agent::board::Board) -> String {
    let mut source = format!(
        "{}\n{}",
        board.draft.title_hint.trim(),
        board.request_with_notes()
    );
    for id in &board.pinned {
        let Some(item) = board.evidence.get(*id) else {
            continue;
        };
        let text: String = item.text.chars().take(FIELD_REF_CAP).collect();
        source.push_str(&format!("\n【引用材料《{}》】\n{text}", item.doc_title));
    }
    source
}

/// 原文里出没出现过词库里的名称（规范名、对外名、简称、别名）：一个都没有时
/// 「要素抽取」产不出任何建议，可以不调模型。
fn any_vocabulary_name(text: &str, vocabulary: &[crate::models::VocabularyEntry]) -> bool {
    let hit = |name: &str| !name.trim().is_empty() && text.contains(name.trim());
    vocabulary.iter().any(|entry| {
        hit(&entry.canonical)
            || hit(&entry.external_name)
            || hit(&entry.abbr)
            || entry.aliases.iter().any(|alias| hit(alias))
    })
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
    let name = step.save_as.as_deref().unwrap_or("report_title");
    // 写了 `variants`：拟几个题名并排让人挑（方案比选，`docs/decision-modules.md` 9.6）。
    let variants = step.param_usize("variants").unwrap_or(1).clamp(1, 4);
    if variants >= 2 && is_generic_title(hint) {
        phase(ctx, format!("根据写作要求拟 {variants} 个报告题名…"));
        let locals = [("request", ctx.board.request_with_notes())];
        let mut text = prompt(ctx, step, "prompt", "报告题名", &locals)?;
        text.push_str(&format!(
            "\n\n请给出 {variants} 个不同角度的具体题名，每行一个，不加编号、引号和说明。"
        ));
        let reply = assist(ctx, &text)?;
        let titles = super::compare::split_variants(&reply, true);
        let titles = super::compare::gate(ctx, titles, variants, |title| !is_generic_title(title));
        if titles.is_empty() {
            anyhow::bail!("模型没有给出具体的报告题名，请填写报告名称后重试");
        }
        return super::compare::offer(ctx, titles, "报告题名", name);
    }
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
    ctx.board.vars.insert(name.into(), Value::String(title));
    Ok(Flow::Next)
}

/// 模型回复 → 一行一条，去掉编号与空行、「无」。
pub(super) fn lines_of(reply: &str, max: usize) -> Vec<String> {
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
    // 按原文拆的要点不让模型重列：重列就给了它改事实的机会。
    let name = step.save_as.as_deref().unwrap_or("items").to_string();
    Ok(super::confirm::list(
        ctx,
        step,
        items,
        ListConfirm::new("要点", name, false),
        true,
    ))
}

/// 大纲的修订提示词（技能里没有「优化大纲」这段时用）。
const OUTLINE_REVISE_TEMPLATE: &str = "根据原始写作要求和修改要求优化当前大纲。以当前大纲为基础，保留未要求调整的内容，\
     只输出大纲，每行一章，格式为“章标题：要点”，不加编号、说明或正文，最多 {max} 章。\n\n\
     【原始写作要求】\n{request}\n\n【当前大纲】\n{outline}\n\n【修改要求】\n{instruction}";

fn plan_list(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec, outline: bool) -> anyhow::Result<Flow> {
    let (default_max, default_var, label) = if outline {
        (8, "outline", "大纲")
    } else {
        (10, "items", "清单")
    };
    let max = param(ctx, step, &["max"], default_max, 1..=20);
    let name = step.save_as.as_deref().unwrap_or(default_var).to_string();
    let spec = if outline {
        ListConfirm::outline(name)
    } else {
        ListConfirm::new(label, name, true)
    };
    let template = if outline {
        OUTLINE_REVISE_TEMPLATE
    } else {
        super::confirm::REVISE_TEMPLATE
    };
    if let Some(flow) = super::confirm::take_revision(ctx, step, &spec, max, template)? {
        return Ok(flow);
    }
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
    Ok(super::confirm::list(ctx, step, items, spec, outline))
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
