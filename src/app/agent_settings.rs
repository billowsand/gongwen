//! AI 管理页「智能体」分组：技能、工具调试台（`docs/ai-agent-workbench.md` 16.9、16.13）。
//! 数据接口分区在 `api_settings.rs`。
//!
//! - 技能管理独立在 `skill_manager/`，此处只保留状态入口；
//! - 工具调试台：选一个工具、填参数、看输出。用的是当前稿件的副本，写工作稿的工具只改副本。
//!
//! 工具调用可能要等网络，放在后台线程里跑，界面每帧取一次结果。

use super::api_settings::ApisPage;
use super::settings::setting_row;
use super::skill_manager::SkillsPage;
use crate::agent::api::{ApiSecrets, ApiStore};
use crate::agent::board::Board;
use crate::agent::skill::{self, Skill};
use crate::agent::tools::{self, Permission};
use crate::app::GongwenApp;
use crate::theme;
use eframe::egui;
use serde_json::Value;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

/// 三个分区的状态。纯当次会话，不进配置。
#[derive(Default)]
pub(crate) struct AgentSettings {
    pub(super) skills: SkillsPage,
    pub(super) apis: ApisPage,
    console: ConsolePage,
}

#[derive(Default)]
struct ConsolePage {
    tool: String,
    args: String,
    running: Option<Receiver<ConsoleResult>>,
    result: Option<ConsoleResult>,
}

struct ConsoleResult {
    outcome: Result<ConsoleOutput, String>,
    notes: Vec<String>,
}

struct ConsoleOutput {
    summary: String,
    value: String,
    evidence: Vec<String>,
    /// 工作稿被改了才有。
    workspace: Option<String>,
    questions: Vec<String>,
}

impl GongwenApp {
    // —— 工具调试台 ——

    pub(super) fn tool_console_section_ui(&mut self, ui: &mut egui::Ui) {
        let page = &mut self.agent_settings.console;
        if let Some(rx) = &page.running {
            match rx.try_recv() {
                Ok(result) => {
                    page.result = Some(result);
                    page.running = None;
                }
                Err(TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(Duration::from_millis(100))
                }
                Err(TryRecvError::Disconnected) => page.running = None,
            }
        }
        let available = console_tools();
        if page.tool.is_empty() {
            page.tool = "doc.outline".into();
            page.args = args_template(&page.tool);
        }
        setting_row(ui, "工具", None, |ui| {
            let current = tools::find(&page.tool)
                .map(|tool| format!("{} {}", tool.permission().icon(), tool.id()))
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("console_tool")
                .selected_text(current)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for tool in &available {
                        let label = format!("{} {}", tool.permission().icon(), tool.id());
                        if ui
                            .selectable_label(page.tool == tool.id(), label)
                            .on_hover_text(tool.description())
                            .clicked()
                        {
                            page.tool = tool.id().to_string();
                            page.args = args_template(&page.tool);
                            page.result = None;
                        }
                    }
                });
        });
        if let Some(tool) = tools::find(&page.tool) {
            ui.weak(format!(
                "{}（{}）",
                tool.description(),
                tool.permission().label()
            ));
            for input in tool.inputs() {
                ui.weak(format!(
                    "· {}{}：{}",
                    input.name,
                    if input.required { "（必填）" } else { "" },
                    input.doc
                ));
            }
            if tool.id() == "http.call" {
                let ids: Vec<String> = ApiStore::load()
                    .unwrap_or_default()
                    .endpoints
                    .into_iter()
                    .map(|e| e.id)
                    .collect();
                ui.weak(if ids.is_empty() {
                    "还没有配置数据接口。".to_string()
                } else {
                    format!("已配置的接口：{}", ids.join("、"))
                });
            }
        }
        ui.add_space(4.0);
        ui.label("参数（JSON）");
        crate::app::widgets::bounded_text_edit(
            ui,
            "tool_console_args",
            8,
            egui::TextEdit::multiline(&mut page.args)
                .code_editor()
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        let doc = self.active_doc_ref();
        let context = match doc {
            Some(doc) => format!(
                "对象：当前稿件《{}》的副本（{} 字）；写工作稿的工具只改副本。",
                if doc.draft.title_hint.trim().is_empty() {
                    "未命名"
                } else {
                    doc.draft.title_hint.trim()
                },
                doc.generated_markdown.chars().count()
            ),
            None => "对象：空白稿（没有打开稿件）。".to_string(),
        };
        ui.weak(context);
        let board = Board {
            draft: doc.map(|d| d.draft.clone()).unwrap_or_default(),
            document: doc
                .map(|d| d.generated_markdown.clone())
                .unwrap_or_default(),
            workspace: doc
                .map(|d| d.generated_markdown.clone())
                .unwrap_or_default(),
            ..Board::default()
        };
        let config = self.config.clone();
        let page = &mut self.agent_settings.console;
        ui.horizontal(|ui| {
            let running = page.running.is_some();
            if theme::primary_icon_button_enabled(ui, !running, theme::Icon::Sparkles, "运行")
                .clicked()
            {
                match serde_json::from_str::<Value>(if page.args.trim().is_empty() {
                    "{}"
                } else {
                    &page.args
                }) {
                    Ok(args) => {
                        page.result = None;
                        page.running = Some(run_tool_in_background(
                            page.tool.clone(),
                            args,
                            board,
                            config,
                        ));
                    }
                    Err(error) => {
                        page.result = Some(ConsoleResult {
                            outcome: Err(format!("参数不是合法的 JSON：{error}")),
                            notes: Vec::new(),
                        })
                    }
                }
            }
            if running {
                theme::spinner(ui, 14.0, theme::accent());
                ui.weak("运行中…");
            }
        });
        if let Some(result) = &page.result {
            ui.add_space(6.0);
            console_result_ui(ui, result);
        }
    }
}

pub(super) fn code_block(ui: &mut egui::Ui, id: &str, text: &str) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(200.0)
        .show(ui, |ui| {
            let mut text = text.to_string();
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .interactive(false)
                    .desired_width(f32::INFINITY),
            );
        });
}

/// 调试台能选的工具：读、检查、写（只改副本）与外部调用；问用户与调模型在技能里试。
fn console_tools() -> Vec<&'static dyn tools::Tool> {
    tools::all()
        .into_iter()
        .filter(|tool| tool.permission() != Permission::AskUser && tool.id() != "llm.generate")
        .collect()
}

/// 按工具的输入列出一份参数模板，必填的在前。
fn args_template(id: &str) -> String {
    let Some(tool) = tools::find(id) else {
        return "{}".into();
    };
    let mut map = serde_json::Map::new();
    for input in tool.inputs().iter().filter(|input| input.required) {
        map.insert(input.name.into(), Value::String(String::new()));
    }
    serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

/// 调试台的技能：放行全部工具（电话号码除外——那仍要技能显式声明）。
fn console_skill() -> Skill {
    let mut skill =
        skill::parse("console", "---\nname: 工具调试台\n---\n", "调试台").expect("调试台技能");
    skill.tools = tools::ids().into_iter().map(str::to_string).collect();
    skill
}

fn run_tool_in_background(
    tool: String,
    args: Value,
    mut board: Board,
    config: crate::models::AppConfig,
) -> Receiver<ConsoleResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    // 接口定义与密钥在界面线程读（测试替换的配置目录只对当前线程有效）。
    let apis = ApiStore::load().unwrap_or_default();
    let secrets = ApiSecrets::load().unwrap_or_default();
    std::thread::spawn(move || {
        let skill = console_skill();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let model = crate::agent::backend::LmBackend::new(&config, cancel);
        let kb = crate::agent::tools::RagSearch {
            enabled: config.rag.enabled,
            rag: config.resolved_rag(),
            chat: config.draft_chat().unwrap_or_default(),
            kind_filter: None,
        };
        let env = tools::Env {
            config: &config,
            vocabulary: &config.vocabulary,
            kb: &kb,
            manuscripts: &tools::SqliteManuscripts,
            model: &model,
            skill: &skill,
            apis: &apis,
            secrets: &secrets,
        };
        let before = board.workspace.clone();
        let mut notes = Vec::new();
        let outcome = {
            let mut emit = |event: crate::agent::engine::Event| {
                if let crate::agent::engine::Event::Note(text) = event {
                    notes.push(text);
                }
            };
            let mut ctx = tools::ToolCtx {
                board: &mut board,
                env: &env,
                emit: &mut emit,
            };
            tools::call(&tool, &mut ctx, &args)
        };
        let outcome = outcome.map(|output| ConsoleOutput {
            summary: output.summary,
            value: serde_json::to_string_pretty(&output.value).unwrap_or_default(),
            evidence: output
                .evidence
                .iter()
                .map(|doc| {
                    format!(
                        "《{}》{} —— {}",
                        doc.title,
                        doc.section,
                        crate::agent::tools::short(&doc.text, 60)
                    )
                })
                .collect(),
            workspace: (board.workspace != before).then(|| board.workspace.clone()),
            questions: output
                .suspend
                .unwrap_or_default()
                .iter()
                .map(|q| q.text.clone())
                .collect(),
        });
        let _ = tx.send(ConsoleResult { outcome, notes });
    });
    rx
}

fn console_result_ui(ui: &mut egui::Ui, result: &ConsoleResult) {
    match &result.outcome {
        Ok(output) => {
            ui.colored_label(theme::success(), &output.summary);
            ui.label("输出");
            code_block(ui, "console_value", &output.value);
            if !output.evidence.is_empty() {
                ui.label(format!(
                    "会并入证据包的资料（{} 段）",
                    output.evidence.len()
                ));
                for line in &output.evidence {
                    ui.weak(line);
                }
            }
            if let Some(workspace) = &output.workspace {
                ui.label("改动后的工作稿（副本）");
                code_block(ui, "console_workspace", workspace);
            }
            for question in &output.questions {
                ui.weak(format!("会问用户：{question}"));
            }
        }
        Err(error) => {
            ui.colored_label(theme::danger(), error);
        }
    }
    for note in &result.notes {
        ui.weak(format!("· {note}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_offers_no_asking_or_model_tools_and_templates_list_required_inputs() {
        let ids: Vec<&str> = console_tools().iter().map(|tool| tool.id()).collect();
        assert!(
            ids.contains(&"kb.search") && ids.contains(&"http.call") && ids.contains(&"ws.replace")
        );
        assert!(!ids.contains(&"ask.choice") && !ids.contains(&"llm.generate"));
        assert_eq!(
            serde_json::from_str::<Value>(&args_template("ws.replace")).unwrap(),
            serde_json::json!({"pattern": ""})
        );
        let skill = console_skill();
        assert!(skill.allows_tool("http.call:anything"));
        assert!(
            !skill.allows_tool("vocab.persons.phone"),
            "电话号码仍要技能显式声明"
        );
    }
}
