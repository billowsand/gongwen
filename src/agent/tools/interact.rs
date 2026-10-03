//! H 组：模型、选择题、留言。

use super::{
    Input, Permission, Tool, ToolCtx, ToolOutput, arg_bool, arg_str, arg_strings, optional,
    required,
};
use crate::agent::backend::ModelRole;
use crate::agent::clarify::{Action, Choice, Question, Target};
use crate::agent::engine::Event;
use crate::lmstudio::StreamDelta;
use serde_json::{Map, Value, json};

pub(super) const TOOLS: [&dyn Tool; 3] = [&Generate, &AskChoice, &Note];

/// 辅助步骤（列问题、核对、出题）的系统提示。
pub(crate) const ASSIST_SYSTEM: &str =
    "你是公文写作的辅助判断程序。严格按要求的格式输出，不解释、不寒暄、不加 Markdown。";

struct Generate;

impl Tool for Generate {
    fn id(&self) -> &'static str {
        "llm.generate"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "用技能里的某段提示词调模型，返回文字；target 为 workspace 时直接写入工作稿"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("prompt", "技能正文里的提示词标题"),
            optional("input", "填进提示词 {input} 的内容"),
            optional(
                "role",
                "draft（起草模型，默认）/ assist（辅助模型，温度 0）",
            ),
            optional("target", "workspace 表示把结果写入工作稿；不给就只返回文字"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let section = arg_str(args, "prompt").unwrap_or_default();
        let template = ctx
            .env
            .skill
            .section(&section)
            .ok_or_else(|| format!("技能里没有提示词「{section}」"))?
            .replace("{input}", &arg_str(args, "input").unwrap_or_default());
        let prompt = ctx.board.render(&template);
        let role = match arg_str(args, "role").as_deref() {
            Some("assist") => ModelRole::Assist,
            _ => ModelRole::Draft,
        };
        let to_workspace = arg_str(args, "target").as_deref() == Some("workspace");
        let system = match role {
            ModelRole::Draft => ctx.board.system_prompt.clone(),
            ModelRole::Assist => ASSIST_SYSTEM.to_string(),
        };
        let emit = &mut *ctx.emit;
        let completion = ctx
            .env
            .model
            .complete(role, &system, &prompt, &mut |delta| match delta {
                StreamDelta::Reasoning(text) => emit(Event::Reasoning(text.to_string())),
                StreamDelta::Content(text) if to_workspace => {
                    emit(Event::Content(text.to_string()));
                }
                StreamDelta::Content(_) => {}
            })
            .map_err(|e| format!("{e:#}"))?;
        ctx.board.truncated |= completion.truncated;
        let text = if to_workspace {
            crate::prompt::sanitize_model_markdown(&completion.content)
        } else {
            completion.content.trim().to_string()
        };
        let chars = text.chars().count();
        if to_workspace {
            ctx.board.workspace = text.clone();
            (ctx.emit)(Event::Workspace(text.clone()));
        }
        Ok(ToolOutput::new(
            Value::String(text),
            format!(
                "调用{}模型（提示词「{section}」）→ {chars} 字{}",
                if role == ModelRole::Draft {
                    "起草"
                } else {
                    "辅助"
                },
                if to_workspace {
                    "，写入工作稿"
                } else {
                    ""
                }
            ),
        ))
    }
}

struct AskChoice;

impl Tool for AskChoice {
    fn id(&self) -> &'static str {
        "ask.choice"
    }
    fn permission(&self) -> Permission {
        Permission::AskUser
    }
    fn description(&self) -> &'static str {
        "向用户出一道选择题，流程停下等回答；选中项的值存进步骤的 save_as"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[
            required("question", "问题"),
            optional("options", "选项文字列表"),
            optional(
                "from",
                "从某个变量的列表里出选项（如 ms.search 的结果），选中后存那一项的 id",
            ),
            optional("custom", "是否允许自己填写，默认是"),
            optional("skip", "是否允许跳过，默认否"),
        ];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let question = arg_str(args, "question").unwrap_or_default();
        let mut choices: Vec<Choice> = arg_strings(args, "options")
            .into_iter()
            .map(|label| Choice {
                action: Action::Pick(Value::String(label.clone())),
                label,
                detail: String::new(),
                recommended: false,
            })
            .collect();
        if let Some(var) = arg_str(args, "from") {
            let list = ctx
                .board
                .vars
                .get(&var)
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| format!("变量「{var}」不是列表，没法从里面出选项"))?;
            for item in list.iter().take(8) {
                let label = ["title", "name", "canonical"]
                    .iter()
                    .find_map(|key| item.get(*key).and_then(Value::as_str))
                    .map_or_else(|| super::super::board::value_to_text(item), str::to_string);
                let detail = ["kind", "status", "doc_date"]
                    .iter()
                    .filter_map(|key| item.get(*key).and_then(Value::as_str))
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                choices.push(Choice {
                    label,
                    detail,
                    recommended: false,
                    action: Action::Pick(item.get("id").cloned().unwrap_or_else(|| item.clone())),
                });
            }
        }
        if choices.is_empty() && !arg_bool(args, "custom").unwrap_or(true) {
            return Err("选择题既没有选项也不让自己填，没法问".into());
        }
        if let Some(first) = choices.first_mut() {
            first.recommended = true;
        }
        let count = choices.len();
        let mut output =
            ToolOutput::new(Value::Null, format!("问你：{question}（{count} 个选项）"));
        output.suspend = Some(vec![Question {
            id: 1,
            text: question,
            choices,
            custom_hint: arg_bool(args, "custom")
                .unwrap_or(true)
                .then(|| "自己填写".to_string()),
            skippable: arg_bool(args, "skip").unwrap_or(false),
            target: Target::Pick,
        }]);
        Ok(output)
    }
}

struct Note;

impl Tool for Note {
    fn id(&self) -> &'static str {
        "note"
    }
    fn permission(&self) -> Permission {
        Permission::Read
    }
    fn description(&self) -> &'static str {
        "在任务流里留一条说明"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required("text", "说明文字")];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let text = arg_str(args, "text").unwrap_or_default();
        (ctx.emit)(Event::Note(text.clone()));
        Ok(ToolOutput::new(json!({"text": text}), "留下一条说明"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use super::*;

    #[test]
    fn generate_renders_the_prompt_and_can_write_the_workspace() {
        let mut fixture = Fixture::new("原文");
        fixture.board.request = "冬季防火".into();
        fixture
            .skill
            .sections
            .insert("提炼".into(), "围绕{request}提炼：{input}".into());
        let out = fixture
            .call(
                "llm.generate",
                json!({"prompt": "提炼", "input": "材料甲", "role": "assist"}),
            )
            .unwrap();
        assert_eq!(out.value, "模型的回答");
        let prompts = fixture.model.prompts.borrow();
        assert_eq!(prompts[0].0, ModelRole::Assist);
        assert_eq!(prompts[0].1, "围绕冬季防火提炼：材料甲");
        drop(prompts);
        assert_eq!(fixture.board.workspace, "原文", "不给 target 不写工作稿");
        fixture
            .call(
                "llm.generate",
                json!({"prompt": "提炼", "target": "workspace"}),
            )
            .unwrap();
        assert_eq!(fixture.board.workspace, "模型的回答");
        assert!(
            fixture
                .call("llm.generate", json!({"prompt": "没有这段"}))
                .unwrap_err()
                .contains("没有提示词")
        );
    }

    #[test]
    fn ask_choice_suspends_with_options_from_a_list_variable() {
        let mut fixture = Fixture::new("");
        fixture.board.vars.insert(
            "candidates".into(),
            json!([{"id": 7, "title": "去年的通知", "kind": "普通公文", "status": "已发布"}, {"id": 9, "title": "前年的通知"}]),
        );
        let out = fixture
            .call(
                "ask.choice",
                json!({"question": "照哪篇写？", "from": "candidates", "custom": false}),
            )
            .unwrap();
        let questions = out.suspend.expect("应当挂起");
        let question = &questions[0];
        assert_eq!(question.target, Target::Pick);
        assert_eq!(question.choices[0].label, "去年的通知");
        assert_eq!(question.choices[0].detail, "普通公文 · 已发布");
        assert!(question.choices[0].recommended);
        assert_eq!(question.choices[1].action, Action::Pick(json!(9)));
        assert!(question.custom_hint.is_none());
        assert!(
            fixture
                .call("ask.choice", json!({"question": "？", "custom": false}))
                .unwrap_err()
                .contains("没法问")
        );
    }

    #[test]
    fn notes_reach_the_task_flow() {
        let mut fixture = Fixture::new("");
        fixture
            .call("note", json!({"text": "知识库里没有同类稿件"}))
            .unwrap();
        assert!(matches!(&fixture.events[0], Event::Note(text) if text == "知识库里没有同类稿件"));
    }
}
