//! 上下文预算的夹具测试（`docs/ai-agent-workbench.md` 16.15 A）：证据按预算缩、改写放不下就停、
//! 长稿分段诊断、自主步骤压缩较早的工具结果。

use super::board::Board;
use super::engine::Event;
use super::skill::{self, Skill};
use super::testkit::{Driver, KeywordKb, ScriptedModel, chunk};
use crate::lmstudio::context::estimate_tokens;
use crate::models::{DraftInput, TemplateKind};

fn board(document: &str, request: &str) -> Board {
    Board {
        draft: DraftInput {
            kind: TemplateKind::PlainDocument,
            ..DraftInput::default()
        },
        request: request.into(),
        document: document.into(),
        system_prompt: "SYSTEM".into(),
        ..Board::default()
    }
}

fn trial_skill(front: &str, body: &str) -> Skill {
    let text = format!("---\nname: 试验\ndescription: 测试\n{front}\n---\n\n{body}\n");
    let skill = skill::parse("trial", &text, "测试").expect("技能能解析");
    let problems = skill::validate(&skill, &super::ops::names(), &super::tools::ids());
    assert!(problems.is_empty(), "{problems:?}");
    skill
}

fn notes(driver: &Driver<'_>) -> Vec<String> {
    driver
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Note(note) => Some(note.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn evidence_shrinks_to_fit_the_window() {
    let skill = trial_skill("tools: [llm.generate]\nflow:\n  - step: generate", "");
    let model = ScriptedModel::new(|_, _| "通知正文".into()).with_window(8192);
    let kb = KeywordKb::disabled();
    let mut start = board("", "写一个防火通知");
    start
        .evidence
        .absorb("防火", &[chunk(1, "条例", &"证".repeat(30_000))]);
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.run();
    let prompt = model.calls.borrow()[0].1.clone();
    let budget = super::budget::input_budget(8192);
    assert!(
        estimate_tokens("SYSTEM") + estimate_tokens(&prompt) <= budget,
        "装箱后不超输入预算"
    );
    assert!(prompt.contains("[K1]"), "证据还在，只是短了");
    let notes = notes(&driver);
    assert!(
        notes.iter().any(|n| n.contains("证据只放了约")),
        "{notes:?}"
    );

    // 窗口够大时照设置值放，不留说明。
    let model = ScriptedModel::new(|_, _| "通知正文".into()).with_window(131_072);
    let mut start = board("", "写一个防火通知");
    start
        .evidence
        .absorb("防火", &[chunk(1, "条例", &"证".repeat(30_000))]);
    let mut driver = Driver::new(&skill, &model, &kb, start);
    driver.run();
    assert!(model.calls.borrow()[0].1.contains(&"证".repeat(8000)));
    assert!(notes_of(&driver).is_empty());
}

fn notes_of(driver: &Driver<'_>) -> Vec<String> {
    notes(driver)
        .into_iter()
        .filter(|n| n.contains("证据只放了"))
        .collect()
}

#[test]
fn a_rewrite_that_cannot_fit_stops_with_a_clear_message() {
    let skill = trial_skill(
        "tools: [llm.generate]\nflow:\n  - step: generate\n    mode: rewrite",
        "## 润色\n\n{request}",
    );
    let model = ScriptedModel::new(|_, _| "改后".into()).with_window(8192);
    let kb = KeywordKb::disabled();
    let document = "这是一段很长的正文。".repeat(600);
    let mut driver = Driver::new(&skill, &model, &kb, board(&document, "润色一下"));
    let error = driver.try_run().unwrap_err().to_string();
    assert!(error.contains("请选中一部分再做"), "{error}");
    assert!(error.contains("8k"), "{error}");
    assert!(model.calls.borrow().is_empty(), "放不下就不发请求");
}

#[test]
fn a_long_document_is_diagnosed_in_parts() {
    let skill = trial_skill(
        "output: report\ntools: [check.elements, llm.generate]\nflow:\n  - step: review\n    checks: [model]",
        "## 诊断\n\n找问题，最多 {max} 条。\n\n【正文】\n{document}",
    );
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("# 二、") {
            "表述｜第二章有问题｜第二章内容｜无".into()
        } else {
            "表述｜第一章有问题｜第一章内容｜无".into()
        }
    })
    .with_window(8192);
    let kb = KeywordKb::disabled();
    let para = format!("{}\n", "第一章内容。".repeat(500));
    let second = format!("{}\n", "第二章内容。".repeat(500));
    let document = format!("# 一、总体要求\n{para}{para}# 二、重点任务\n{second}{second}");
    let mut driver = Driver::new(&skill, &model, &kb, board(&document, "审一下"));
    driver.run();
    assert!(model.calls.borrow().len() >= 2, "分段诊断");
    let texts: Vec<&str> = driver
        .board
        .findings
        .iter()
        .map(|f| f.text.as_str())
        .collect();
    assert!(texts.contains(&"第一章有问题"), "{texts:?}");
    assert!(texts.contains(&"第二章有问题"), "{texts:?}");
    assert!(
        notes(&driver)
            .iter()
            .any(|n| n.contains("分") && n.contains("段诊断")),
        "{:?}",
        notes(&driver)
    );
}

fn call(name: &str, arguments: &str) -> String {
    format!("<tool_call>{{\"name\": \"{name}\", \"arguments\": {arguments}}}</tool_call>")
}

#[test]
fn the_agent_compacts_older_tool_results() {
    let skill = trial_skill(
        "output: auto\ntools: [kb.search]\nflow:\n  - step: agent",
        "## 任务\n\n{request}",
    );
    let model = ScriptedModel::new(|_, transcript| {
        for word in ["甲", "乙", "丙"] {
            if !transcript.contains(&format!("检索知识库「{word}」")) {
                return call("kb_search", &format!(r#"{{"query": "{word}"}}"#));
            }
        }
        call("finish", r#"{"summary": "查完了"}"#)
    })
    .with_window(12_000);
    let kb = KeywordKb::new(vec![
        ("甲", chunk(1, "甲文", &"甲".repeat(2000))),
        ("乙", chunk(2, "乙文", &"乙".repeat(2000))),
        ("丙", chunk(3, "丙文", &"丙".repeat(2000))),
    ]);
    let mut driver = Driver::new(&skill, &model, &kb, board("", "查甲乙丙"));
    driver.run();
    assert_eq!(driver.board.vars["agent_summary"], "查完了");
    let notes = notes(&driver);
    assert!(notes.iter().any(|n| n.contains("只留了摘要")), "{notes:?}");
    let last = model.calls.borrow().last().unwrap().1.clone();
    assert!(last.contains("（原文已省略，需要可再调）"), "{last}");
    assert!(!last.contains(&"甲".repeat(2000)), "最早的结果已经压掉");
    assert!(last.contains(&"丙".repeat(2000)), "最近的结果保留原文");
}
