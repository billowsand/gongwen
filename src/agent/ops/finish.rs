//! 收尾：核验引用、引用落到研究报告的文献体系、出题。

use super::{Flow, assist, check_cancel, param, phase, prompt, squash, tool_line};
use crate::agent::clarify;
use crate::agent::engine::Event;
use crate::agent::evidence;
use crate::agent::skill::StepSpec;
use crate::agent::tools::{self, Permission, ToolCtx, short};
use crate::ai_guard::FactKind;
use crate::export::bibliography;
use crate::models::TemplateKind;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

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
    let board = &mut *ctx.board;
    board.ledger.mark_declined(&board.notes);
    // 来源不明的事实若在材料里同一处写着别的值，作为选项给出（事实冲突裁决）。
    let sources = board.candidate_sources();
    board.ledger.attach_candidates(&sources, ctx.env.vocabulary);
    let mut questions = clarify::gap_questions(&ctx.board.ledger, max, ctx.env.vocabulary);
    if questions.is_empty() {
        return Ok(Flow::Next);
    }
    // 不让人对着空框发愣：能出建议写法的，交辅助模型一次出齐。出错就不给建议，题照问。
    if let Some(text) = clarify::suggestion_prompt(
        &questions,
        &ctx.board.ledger,
        &ctx.board.request,
        &crate::prompt::TimeContext::now().today,
    ) {
        check_cancel(ctx)?;
        phase(ctx, "给待确认的几处想几个建议写法…");
        if let Ok(reply) = assist(ctx, &text) {
            clarify::add_suggestions(&mut questions, &reply);
        }
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

/// `cite`：研究报告里，把 [K#] 引用落到报告的文献体系。证据的题名与文献库（文档要素里的
/// BibTeX）某条题名对得上，就换成 `[@key]`；对不上的换成行内脚注 `[^k3]:(来源：《题名》· 出处)`，
/// 同一份资料在一章里只挂第一处。
///
/// 文献库只读：用不上的证据不会被加进文献库（要素由用户维护，红线 2）。非研究报告不做，
/// 交付时引用标记照常剥掉。`style: footnote` 时一律用脚注。放在 `verify` 之后——核验要靠
/// [K#] 找回证据。
pub(super) fn cite(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    if ctx.board.draft.kind != TemplateKind::ResearchReport {
        return Ok(Flow::Next);
    }
    let library = if step.param_str("style") == Some("footnote") {
        bibliography::Library::default()
    } else {
        bibliography::parse(&ctx.board.draft.research.bibliography_content)
    };
    let pack = &ctx.board.evidence;
    let mut uses: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut to_bib, mut to_note) = (0usize, 0usize);
    let workspace = ctx.board.workspace.clone();
    // 每章开头的位置。同一份资料在一章里只挂一次脚注：一本书引了几十处，满篇都是同一条
    // 脚注没法读（实测踩过）；逐句的出处仍在核实清单里。
    let chapters: Vec<usize> = workspace
        .match_indices("\n## ")
        .map(|(pos, _)| pos + 1)
        .collect();
    let mut noted: BTreeSet<(usize, String)> = BTreeSet::new();
    let cited = evidence::CITATION.replace_all(&workspace, |caps: &regex::Captures<'_>| {
        let at = caps.get(0).map_or(0, |m| m.start());
        let chapter = chapters.partition_point(|start| *start <= at);
        let mut keys: Vec<String> = Vec::new();
        let mut notes = String::new();
        for id in evidence::citation_ids(&caps[0]) {
            let Some(item) = pack.get(id) else {
                continue;
            };
            match bib_key(&library, &item.doc_title) {
                Some(key) => {
                    if !keys.contains(&key) {
                        keys.push(key);
                        to_bib += 1;
                    }
                }
                None => {
                    if !noted.insert((chapter, item.doc_title.clone())) {
                        continue;
                    }
                    let count = uses.entry(id).or_default();
                    *count += 1;
                    let suffix = if *count > 1 {
                        format!("-{count}")
                    } else {
                        String::new()
                    };
                    let source = footnote_text(&item.source_label());
                    notes.push_str(&format!("[^k{id}{suffix}]:(来源：{source})"));
                    to_note += 1;
                }
            }
        }
        let mut out = String::new();
        if !keys.is_empty() {
            out.push_str(&format!("[@{}]", keys.join("; @")));
        }
        out.push_str(&notes);
        out
    });
    if to_bib + to_note == 0 {
        return Ok(Flow::Next);
    }
    ctx.board.workspace = cited.into_owned();
    tool_line(
        ctx,
        "cite",
        Permission::WriteWorkspace,
        format!("引用落到报告：文献库 {to_bib} 处，脚注 {to_note} 处"),
    );
    (ctx.emit)(Event::Workspace(ctx.board.workspace.clone()));
    Ok(Flow::Next)
}

/// 证据题名对上文献库里的哪一条：去掉书名号与空白后相同，或一方包含另一方（至少 6 个字）。
fn bib_key(library: &bibliography::Library, title: &str) -> Option<String> {
    let normalize = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '《' | '》' | '“' | '”' | '"'))
            .collect::<String>()
            .to_lowercase()
    };
    let wanted = normalize(title);
    if wanted.chars().count() < 2 {
        return None;
    }
    library
        .entries
        .iter()
        .find(|entry| {
            let have = normalize(&entry.title);
            !have.is_empty()
                && (have == wanted
                    || (wanted.chars().count().min(have.chars().count()) >= 6
                        && (have.contains(&wanted) || wanted.contains(&have))))
        })
        .map(|entry| entry.key.clone())
}

/// 脚注内容里不能出现半角右括号（脚注写法不支持嵌套），换成全角。
fn footnote_text(text: &str) -> String {
    text.replace('(', "（").replace(')', "）")
}
