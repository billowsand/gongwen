//! 方案比选（`docs/decision-modules.md` 9.6）：让模型出几个不同的版本（标题、开头段、结构……），
//! 过闸门后整段并排给用户挑，挑中的存进变量；也可以自己写。
//!
//! - `compare` 算子：按提示词出 `count` 个（默认 3，2–4）版本，存进 `save_as`；
//! - `plan mode: title` 写了 `variants` 时也走这里（[`offer`]）：出几个报告题名让人挑。
//!
//! 闸门（确定性，过不了的版本丢掉）：去空、去重、去「方案一：」这类前缀，每个不超过 600 字；
//! 带了出处里没有的日期、数字、文件名的丢掉（`gaps::untraced_facts`，对照材料、已确认信息、
//! 证据与正文）。剩一个就直接用，一个不剩就报错。不设推荐项：模型没给理由，程序不替人挑。

use super::{Flow, assist, note, param, phase, prompt, tool_line};
use crate::agent::clarify::{Action, Choice, Question, Target};
use crate::agent::decision::Decision;
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx};
use serde_json::Value;

/// 一个版本最多多少字。
const MAX_CHARS: usize = 600;
/// 多行版本之间的分隔行。
const SEPARATOR: &str = "===";

/// 追加在提示词末尾的格式要求。
fn format_hint(count: usize, single_line: bool) -> String {
    if single_line {
        format!("\n\n请给出 {count} 个不同的写法，每行一个，不加编号、引号和说明。")
    } else {
        format!(
            "\n\n请给出 {count} 个不同的写法，每个之间用单独一行「{SEPARATOR}」隔开，不加编号和说明。"
        )
    }
}

/// 模型回复 → 各个版本：单行的按行拆；多行的按分隔行拆，没写分隔行就按空行拆。
/// 去掉「方案一：」「1.」这类前缀、Markdown 标题井号与包裹的引号。
pub(super) fn split_variants(reply: &str, single_line: bool) -> Vec<String> {
    let reply = crate::prompt::sanitize_model_markdown(reply);
    let blocks: Vec<String> = if single_line {
        reply.lines().map(str::to_string).collect()
    } else if reply.lines().any(|line| line.trim() == SEPARATOR) {
        reply
            .split('\n')
            .collect::<Vec<_>>()
            .split(|line| line.trim() == SEPARATOR)
            .map(|lines| lines.join("\n"))
            .collect()
    } else {
        reply.split("\n\n").map(str::to_string).collect()
    };
    blocks
        .into_iter()
        .map(|block| {
            let block = block.trim().trim_start_matches('#').trim();
            let block = strip_label(block);
            let block = crate::agent::clarify::strip_numbering(block).trim();
            block
                .trim_matches(['「', '」', '“', '”', '"', '《', '》'])
                .trim()
                .to_string()
        })
        .filter(|block| !block.is_empty() && block != "无")
        .collect()
}

/// 「方案一：」「版本2：」「写法三:」这类前缀。
fn strip_label(text: &str) -> &str {
    for head in ["方案", "版本", "写法", "备选"] {
        if let Some(rest) = text.strip_prefix(head)
            && let Some((number, body)) = rest.split_once(['：', ':'])
            && number.chars().count() <= 3
            && number
                .chars()
                .all(|ch| ch.is_ascii_digit() || "一二三四五六七八九十".contains(ch))
        {
            return body.trim();
        }
    }
    text
}

/// 过闸门：去重、限长、丢掉带出处里没有的事实的；`keep` 再按调用方的规矩筛（如题名不能是文种名）。
/// 返回留下的版本，丢掉的在过程里说一句。
pub(super) fn gate(
    ctx: &mut ToolCtx<'_, '_>,
    variants: Vec<String>,
    count: usize,
    keep: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut sources = ctx.board.sources_text();
    sources.push('\n');
    sources.push_str(&ctx.board.document);
    let mut out: Vec<String> = Vec::new();
    let mut dropped = Vec::new();
    for variant in variants {
        if out.contains(&variant) || !keep(&variant) {
            continue;
        }
        if variant.chars().count() > MAX_CHARS {
            dropped.push("太长".to_string());
            continue;
        }
        let untraced = crate::agent::gaps::untraced_facts(&variant, &sources, ctx.env.vocabulary);
        if let Some(fact) = untraced.first() {
            dropped.push(format!("写了出处里没有的「{}」", fact.value));
            continue;
        }
        out.push(variant);
        if out.len() == count {
            break;
        }
    }
    if !dropped.is_empty() {
        note(
            ctx,
            format!(
                "有 {} 个版本没过闸门：{}",
                dropped.len(),
                dropped.join("；")
            ),
        );
    }
    out
}

/// 几个版本交给用户挑（`Decision::Compare`）；只剩一个就直接用，不问。
pub(super) fn offer(
    ctx: &mut ToolCtx<'_, '_>,
    variants: Vec<String>,
    label: &str,
    var: &str,
) -> anyhow::Result<Flow> {
    match variants.len() {
        0 => anyhow::bail!("模型给出的{label}都没过闸门，请把要求写具体些后重试"),
        1 => {
            note(ctx, format!("只有一个{label}过了闸门，直接用它"));
            let only = variants.into_iter().next().unwrap_or_default();
            ctx.board.vars.insert(var.into(), Value::String(only));
            Ok(Flow::Next)
        }
        count => {
            tool_line(
                ctx,
                "ask.choice",
                Permission::AskUser,
                format!("{count} 个{label}请你挑"),
            );
            let numbers = ["一", "二", "三", "四"];
            let question = Question {
                id: 1,
                text: format!("用哪个{label}？"),
                choices: variants
                    .into_iter()
                    .enumerate()
                    .map(|(index, text)| Choice {
                        label: format!("方案{}", numbers.get(index).copied().unwrap_or("")),
                        detail: text.clone(),
                        recommended: false,
                        action: Action::Pick(Value::String(text)),
                    })
                    .collect(),
                custom_hint: Some("都不合适，自己写".into()),
                prefill: String::new(),
                skippable: false,
                multi: false,
                target: Target::Pick,
            };
            Ok(Flow::Decide {
                questions: vec![question],
                decision: Decision::Compare(crate::agent::decision::CompareSpec {
                    label: label.to_string(),
                    var: var.to_string(),
                }),
                into: var.to_string(),
                again: false,
            })
        }
    }
}

/// `compare`：参数 `prompt`（提示词段，默认「方案」）、`count`（几个版本，默认 3，2–4）、
/// `save_as`（挑中的存哪，必填）、`label`（卡片上的叫法，默认「方案」）、`line: true`（每个版本
/// 只有一行，如标题）。提示词里可用 `{request}`、`{count}`。
pub(super) fn compare(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let count = param(ctx, step, &["count"], 3, 2..=4);
    let label = step.param_str("label").unwrap_or("方案").to_string();
    let single_line = step
        .params
        .get("line")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(var) = step.save_as.clone() else {
        anyhow::bail!("compare 要写 save_as：挑中的版本存进哪个变量");
    };
    phase(ctx, format!("拟 {count} 个{label}…"));
    let locals = [
        ("request", ctx.board.request_with_notes()),
        ("count", count.to_string()),
    ];
    let mut text = prompt(ctx, step, "prompt", "方案", &locals)?;
    text.push_str(&format_hint(count, single_line));
    let reply = assist(ctx, &text)?;
    let variants = split_variants(&reply, single_line);
    let variants = gate(ctx, variants, count, |_| true);
    offer(ctx, variants, &label, &var)
}
