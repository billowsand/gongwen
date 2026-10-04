//! `@` 引用的文章：引擎在第一步之前按技能的 `references` 声明处理（16.13、决定 F14）。
//!
//! - `evidence`（默认）：全文并入证据包，可引用、要核验；编号记进 `pinned`，逐节检索时也一直带着；
//! - `baseline`：第一篇当基准稿，写进 `baseline` / `baseline_id` 两个变量（与 `ms.read` 的结果同形），
//!   不进证据包——仿写因此跳过「照哪篇写」的候选选择；其余几篇照样当证据；
//! - `material` / `letter`：全文并进材料，放在用户原话前面（复函当来函）。
//!
//! 正文在后台线程按 id 现读，界面线程只传标题与 id。读不出来的说一声跳过，不中断流程。

use super::board::{RefSource, Reference};
use super::engine::Event;
use super::evidence::EvidenceDoc;
use super::skill::RefUse;
use super::tools::{Permission, ToolCtx, ToolUse};
use serde_json::{Value, json};

/// 读出来的一篇引用。
struct Loaded {
    title: String,
    text: String,
    /// 稿件库的才知道文种。
    kind: Option<crate::models::TemplateKind>,
    /// 证据包去重键，与 `ms.read` / `kb.read` 一致：同一篇再被工具读到不重复编号。
    key: String,
    /// 写进 `baseline` 变量的值。
    value: Value,
}

fn load(ctx: &ToolCtx<'_, '_>, reference: &Reference) -> Result<Loaded, String> {
    match reference.source {
        RefSource::Manuscript => {
            let doc = ctx
                .env
                .manuscripts
                .read(reference.id, None)
                .map_err(|e| format!("{e:#}"))?
                .ok_or("稿件库里已经没有这篇")?;
            Ok(Loaded {
                key: format!("ms:{}:latest", doc.id),
                value: json!({
                    "id": doc.id,
                    "title": doc.title,
                    "kind": doc.kind.label(),
                    "status": doc.status.label(),
                    "doc_number": doc.doc_number,
                    "doc_date": doc.doc_date,
                    "version": Value::Null,
                    "text": doc.markdown,
                    "elements": serde_json::to_value(&doc.draft).unwrap_or(Value::Null),
                }),
                kind: Some(doc.kind),
                title: doc.title,
                text: doc.markdown,
            })
        }
        RefSource::Knowledge => {
            let (title, text) = ctx
                .env
                .kb
                .read(reference.id)
                .map_err(|e| format!("{e:#}"))?
                .ok_or("知识库里已经没有这篇")?;
            Ok(Loaded {
                key: format!("kbdoc:{}", reference.id),
                value: json!({ "id": reference.id, "title": title, "text": text }),
                kind: None,
                title,
                text,
            })
        }
    }
}

/// 读一篇引用当样稿（风格学习）。
pub(crate) fn load_sample(
    ctx: &ToolCtx<'_, '_>,
    reference: &Reference,
) -> Result<crate::agent::style::Sample, String> {
    let loaded = load(ctx, reference)?;
    Ok(crate::agent::style::Sample {
        source: reference.source,
        id: reference.id,
        title: loaded.title,
        kind: loaded.kind,
        text: loaded.text,
    })
}

/// 按技能的声明把引用落到黑板上。
pub(crate) fn apply(ctx: &mut ToolCtx<'_, '_>) {
    let refs = ctx.board.refs.clone();
    let usage = ctx.env.skill.references;
    if usage == RefUse::Sample {
        // 样稿由算子自己读（风格学习），不进证据也不进材料。
        return;
    }
    let mut material = Vec::new();
    let mut baseline_taken = false;
    for reference in &refs {
        let loaded = match load(ctx, reference) {
            Ok(loaded) if !loaded.text.trim().is_empty() => loaded,
            Ok(_) => {
                (ctx.emit)(Event::Note(format!(
                    "引用的《{}》没有正文，跳过。",
                    reference.title
                )));
                continue;
            }
            Err(error) => {
                (ctx.emit)(Event::Note(format!(
                    "引用的《{}》读不出来（{error}），跳过。",
                    reference.title
                )));
                continue;
            }
        };
        let chars = loaded.text.chars().filter(|c| !c.is_whitespace()).count();
        let how = match usage {
            RefUse::Baseline if !baseline_taken => {
                baseline_taken = true;
                ctx.board
                    .vars
                    .insert("baseline_id".into(), Value::from(reference.id));
                ctx.board.vars.insert("baseline".into(), loaded.value);
                "当基准稿"
            }
            RefUse::Material | RefUse::Letter => {
                let label = if usage == RefUse::Letter {
                    "来函"
                } else {
                    "引用材料"
                };
                material.push(format!(
                    "【{label}《{}》】\n{}",
                    loaded.title,
                    loaded.text.trim()
                ));
                if usage == RefUse::Letter {
                    "当来函"
                } else {
                    "并进材料"
                }
            }
            _ => {
                let doc = EvidenceDoc {
                    key: loaded.key,
                    title: loaded.title.clone(),
                    section: String::new(),
                    kind_label: reference.source.label().into(),
                    text: loaded.text,
                };
                let ids = ctx.board.evidence.absorb_docs("@引用", &[doc]);
                for id in ids {
                    if !ctx.board.pinned.contains(&id) {
                        ctx.board.pinned.push(id);
                    }
                }
                "并入证据"
            }
        };
        (ctx.emit)(Event::Tool(ToolUse::new(
            "ref",
            Permission::Read,
            format!(
                "引用{}《{}》（{chars} 字），{how}",
                reference.source.label(),
                loaded.title
            ),
        )));
    }
    if !material.is_empty() {
        let request = ctx.board.request.trim();
        ctx.board.request = if request.is_empty() {
            material.join("\n\n")
        } else {
            let label = if usage == RefUse::Letter {
                "【答复要求】"
            } else {
                "【要求】"
            };
            format!("{}\n\n{label}\n{request}", material.join("\n\n"))
        };
    }
}
