//! AI 管理页「智能体」分组：技能、工具调试台（`docs/ai-agent-workbench.md` 16.9、16.13）。
//! 数据接口分区在 `api_settings.rs`。
//!
//! - 技能：列出内置与用户技能，启用 / 停用，查看与编辑 SKILL.md（保存时校验），复制内置、
//!   新建、删除、导入导出；
//! - 工具调试台：选一个工具、填参数、看输出。用的是当前稿件的副本，写工作稿的工具只改副本。
//!
//! 工具调用可能要等网络，放在后台线程里跑，界面每帧取一次结果。

use super::api_settings::ApisPage;
use super::settings::setting_row;
use crate::agent::api::{ApiSecrets, ApiStore};
use crate::agent::board::Board;
use crate::agent::skill::{self, Skill};
use crate::agent::skill_files;
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
    skills: SkillsPage,
    pub(super) apis: ApisPage,
    console: ConsolePage,
}

#[derive(Default)]
struct SkillsPage {
    loaded: bool,
    skills: Vec<Skill>,
    /// 加载时的说明（文件写坏了之类）。
    notes: Vec<String>,
    selected: Option<String>,
    editor: Option<SkillEditor>,
    new_id: String,
    /// 等待二次确认删除的技能 id。
    confirm_remove: Option<String>,
    message: Option<(bool, String)>,
}

struct SkillEditor {
    id: String,
    text: String,
    saved: String,
    problems: Vec<String>,
    error: Option<String>,
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

pub(super) fn message_ui(ui: &mut egui::Ui, message: &Option<(bool, String)>) {
    if let Some((ok, text)) = message {
        ui.colored_label(
            if *ok {
                theme::success()
            } else {
                theme::danger()
            },
            text,
        );
    }
}

pub(super) fn problems_ui(ui: &mut egui::Ui, problems: &[String]) {
    for problem in problems {
        ui.colored_label(theme::warn(), format!("· {problem}"));
    }
}

fn origin_label(skill: &Skill) -> &'static str {
    match (
        skill.origin == "内置",
        skill::builtin_text(&skill.id).is_some(),
    ) {
        (true, _) => "内置",
        (false, true) => "已改写内置",
        (false, false) => "我的",
    }
}

impl GongwenApp {
    // —— 技能 ——

    fn reload_skills_page(&mut self) {
        let page = &mut self.agent_settings.skills;
        let (skills, notes) = skill::load_all();
        page.skills = skills;
        page.notes = notes;
        page.loaded = true;
    }

    pub(super) fn skills_section_ui(&mut self, ui: &mut egui::Ui) {
        if !self.agent_settings.skills.loaded {
            self.reload_skills_page();
        }
        let apis = ApiStore::load().unwrap_or_default();
        let mut reload = false;
        {
            let page = &mut self.agent_settings.skills;
            ui.horizontal_wrapped(|ui| {
                if theme::icon_button(ui, theme::Icon::Refresh, "重新读取").clicked() {
                    reload = true;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut page.new_id)
                        .hint_text("新技能 id，如 data-brief")
                        .desired_width(150.0),
                );
                if theme::icon_button(ui, theme::Icon::FilePlus, "新建技能").clicked() {
                    let id = page.new_id.trim().to_string();
                    match skill_files::create(&id) {
                        Ok(text) => {
                            page.editor = Some(SkillEditor {
                                id: id.clone(),
                                saved: text.clone(),
                                text,
                                problems: Vec::new(),
                                error: None,
                            });
                            page.selected = Some(id);
                            page.new_id.clear();
                            page.message = Some((true, "已按模板新建，改好后保存。".into()));
                            reload = true;
                        }
                        Err(error) => page.message = Some((false, format!("{error:#}"))),
                    }
                }
                if theme::icon_button(ui, theme::Icon::FileUp, "导入技能包…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("技能包", &["skill", "zip"])
                        .pick_file()
                {
                    page.message = Some(import_message(skill_files::import(&path)));
                    reload = true;
                }
                if theme::icon_button(ui, theme::Icon::Folder, "导入文件夹…").clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_folder()
                {
                    page.message = Some(import_message(skill_files::import(&path)));
                    reload = true;
                }
            });
            message_ui(ui, &page.message);
            for note in &page.notes {
                ui.colored_label(theme::warn(), note);
            }
            ui.add_space(6.0);
            let mut toggles = Vec::new();
            for skill in &page.skills {
                let problems = skill_files::problems_of(skill);
                let missing = skill::missing_apis(skill, &apis);
                ui.horizontal(|ui| {
                    let mut enabled = skill.enabled;
                    if ui
                        .checkbox(&mut enabled, "")
                        .on_hover_text("停用后不出现在 / 列表与技能标签里，也不会被自动选到")
                        .changed()
                    {
                        toggles.push((skill.id.clone(), enabled));
                    }
                    let selected = page.selected.as_deref() == Some(skill.id.as_str());
                    if ui
                        .selectable_label(selected, format!("{}（{}）", skill.name, skill.id))
                        .clicked()
                    {
                        page.selected = Some(skill.id.clone());
                        page.confirm_remove = None;
                    }
                    theme::chip(
                        ui,
                        origin_label(skill),
                        theme::text_soft(),
                        theme::surface_sunk(),
                    );
                    if !problems.is_empty() {
                        theme::chip(
                            ui,
                            &format!("{} 个问题", problems.len()),
                            theme::warn(),
                            theme::warn_soft(),
                        );
                    }
                    if !missing.is_empty() {
                        theme::chip(ui, "接口未配置", theme::warn(), theme::warn_soft());
                    }
                });
            }
            for (id, enabled) in toggles {
                if let Err(error) = skill_files::set_enabled(&id, enabled) {
                    page.message = Some((false, format!("{error:#}")));
                }
                reload = true;
            }
        }
        if reload {
            self.reload_skills_page();
        }
        ui.add_space(8.0);
        theme::hairline(ui);
        ui.add_space(8.0);
        self.skill_detail_ui(ui, &apis);
    }

    fn skill_detail_ui(&mut self, ui: &mut egui::Ui, apis: &ApiStore) {
        let page = &mut self.agent_settings.skills;
        let Some(skill) = page
            .selected
            .as_ref()
            .and_then(|id| page.skills.iter().find(|s| &s.id == id))
            .cloned()
        else {
            ui.weak("点上面的技能查看详情。");
            return;
        };
        let mut reload = false;
        ui.label(egui::RichText::new(&skill.name).strong());
        ui.label(&skill.description);
        if !skill.triggers.is_empty() {
            ui.weak(format!("触发词：{}", skill.triggers.join("、")));
        }
        ui.weak(format!("来源：{}", skill.origin));
        ui.add_space(4.0);
        ui.label("能用的工具：");
        for id in &skill.tools {
            ui.weak(tools::describe(id).unwrap_or_else(|| format!("? {id}（不认识）")));
        }
        let missing = skill::missing_apis(&skill, apis);
        if !missing.is_empty() {
            ui.colored_label(
                theme::warn(),
                format!(
                    "还没配置的数据接口：{}，在「数据接口」页添加后才能调用。",
                    missing.join("、")
                ),
            );
        }
        problems_ui(ui, &skill_files::problems_of(&skill));
        ui.add_space(6.0);
        let has_file = skill_files::has_user_file(&skill.id);
        let builtin = skill::builtin_text(&skill.id).is_some();
        ui.horizontal_wrapped(|ui| {
            let editing = page.editor.as_ref().is_some_and(|e| e.id == skill.id);
            if has_file && !editing && theme::icon_button(ui, theme::Icon::Edit, "编辑").clicked()
            {
                match skill_files::source_text(&skill.id) {
                    Ok(text) => {
                        page.editor = Some(SkillEditor {
                            id: skill.id.clone(),
                            saved: text.clone(),
                            text,
                            problems: Vec::new(),
                            error: None,
                        })
                    }
                    Err(error) => page.message = Some((false, format!("{error:#}"))),
                }
            }
            if builtin
                && !has_file
                && theme::icon_button(ui, theme::Icon::Copy, "复制为我的技能")
                    .on_hover_text("在配置目录写一份可改的副本，同 id 覆盖内置；只改想改的部分也行")
                    .clicked()
            {
                let text = skill::builtin_text(&skill.id)
                    .unwrap_or_default()
                    .to_string();
                match skill_files::save(&skill.id, &text) {
                    Ok(_) => {
                        page.editor = Some(SkillEditor {
                            id: skill.id.clone(),
                            saved: text.clone(),
                            text,
                            problems: Vec::new(),
                            error: None,
                        });
                        page.message = Some((true, "已复制，改好后保存即生效。".into()));
                        reload = true;
                    }
                    Err(error) => page.message = Some((false, error)),
                }
            }
            if theme::icon_button(ui, theme::Icon::FileDown, "导出…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("{}.skill", skill.id))
                    .add_filter("技能包", &["skill"])
                    .save_file()
            {
                page.message = Some(match skill_files::export(&skill.id, &path) {
                    Ok(path) => (true, format!("已导出到 {}", path.display())),
                    Err(error) => (false, format!("{error:#}")),
                });
            }
            if has_file {
                let label = if builtin {
                    "恢复内置"
                } else {
                    "删除我的技能"
                };
                if page.confirm_remove.as_deref() == Some(skill.id.as_str()) {
                    if theme::danger_icon_button(ui, theme::Icon::Trash, &format!("确认{label}"))
                        .clicked()
                    {
                        page.message = Some(match skill_files::remove(&skill.id) {
                            Ok(()) => (true, format!("已{label}。")),
                            Err(error) => (false, format!("{error:#}")),
                        });
                        page.confirm_remove = None;
                        page.editor = None;
                        if !builtin {
                            page.selected = None;
                        }
                        reload = true;
                    }
                    if ui.button("取消").clicked() {
                        page.confirm_remove = None;
                    }
                } else if theme::icon_button(ui, theme::Icon::Trash, label).clicked() {
                    page.confirm_remove = Some(skill.id.clone());
                }
            }
        });
        if let Some(editor) = page.editor.as_mut().filter(|e| e.id == skill.id) {
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("skill_editor")
                .max_height(420.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut editor.text)
                            .code_editor()
                            .desired_rows(22)
                            .desired_width(f32::INFINITY),
                    );
                });
            let mut close = false;
            ui.horizontal(|ui| {
                if theme::icon_button(ui, theme::Icon::SquareCheck, "校验").clicked() {
                    match skill_files::check_text(&editor.id, &editor.text) {
                        Ok(problems) => {
                            editor.error = None;
                            editor.problems = problems;
                            if editor.problems.is_empty() {
                                page.message = Some((true, "校验通过。".into()));
                            }
                        }
                        Err(error) => editor.error = Some(error),
                    }
                }
                let dirty = editor.text != editor.saved;
                if theme::primary_icon_button_enabled(ui, dirty, theme::Icon::Save, "保存")
                    .clicked()
                {
                    match skill_files::save(&editor.id, &editor.text) {
                        Ok(problems) => {
                            editor.error = None;
                            editor.saved = editor.text.clone();
                            page.message = Some(if problems.is_empty() {
                                (true, "已保存，下次发送时生效。".into())
                            } else {
                                (
                                    false,
                                    "已保存，但有问题：加载时会退回内置或停用，见下面的清单。"
                                        .into(),
                                )
                            });
                            editor.problems = problems;
                            reload = true;
                        }
                        Err(error) => editor.error = Some(format!("没有保存：{error}")),
                    }
                }
                if ui.button("关闭").clicked() {
                    close = true;
                }
                if dirty {
                    ui.weak("有未保存的修改");
                }
            });
            if let Some(error) = &editor.error {
                ui.colored_label(theme::danger(), error);
            }
            problems_ui(ui, &editor.problems);
            if close {
                page.editor = None;
            }
        }
        if reload {
            self.reload_skills_page();
        }
    }

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
        ui.add(
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

fn import_message(result: anyhow::Result<(Vec<String>, Vec<String>)>) -> (bool, String) {
    match result {
        Ok((ids, notes)) => {
            let mut text = if ids.is_empty() {
                "没有导入任何技能。".to_string()
            } else {
                format!("已导入：{}。", ids.join("、"))
            };
            if !notes.is_empty() {
                text.push_str(&notes.join("；"));
            }
            (!ids.is_empty() && notes.is_empty(), text)
        }
        Err(error) => (false, format!("导入失败：{error:#}")),
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
            rag: config.rag.clone(),
            chat: config.lm_studio.clone(),
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

    #[test]
    fn importing_reports_what_happened() {
        assert_eq!(
            import_message(Ok((vec!["a".into()], vec![]))),
            (true, "已导入：a。".into())
        );
        let (ok, text) = import_message(Ok((vec!["a".into()], vec!["「b」解析失败".into()])));
        assert!(!ok && text.contains("「b」解析失败"));
    }
}
