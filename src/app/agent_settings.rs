//! AI 管理页「智能体」分组：技能、数据接口、工具调试台（`docs/ai-agent-workbench.md` 16.9、16.13）。
//!
//! - 技能：列出内置与用户技能，启用 / 停用，查看与编辑 SKILL.md（保存时校验），复制内置、
//!   新建、删除、导入导出；
//! - 数据接口：增删改内网查询接口，填样例输入实测「请求 → 原始返回 → 映射结果」，密钥单独存放；
//! - 工具调试台：选一个工具、填参数、看输出。用的是当前稿件的副本，写工作稿的工具只改副本。
//!
//! 接口实测与工具调用可能要等网络，都放在后台线程里跑，界面每帧取一次结果。

use super::settings::setting_row;
use crate::agent::api::{
    self, ApiDestination, ApiEndpoint, ApiHeader, ApiInput, ApiMethod, ApiSecrets, ApiStore,
    InputKind, MappedItem, RawResponse,
};
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

/// 原始返回最多显示这么多字，再长的在文件里看不完，也拖慢界面。
const RAW_PREVIEW_CHARS: usize = 4000;

/// 三个分区的状态。纯当次会话，不进配置。
#[derive(Default)]
pub(crate) struct AgentSettings {
    skills: SkillsPage,
    apis: ApisPage,
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
struct ApisPage {
    loaded: bool,
    store: ApiStore,
    secrets: ApiSecrets,
    dirty: bool,
    selected: Option<usize>,
    confirm_remove: Option<usize>,
    test_args: String,
    test: Option<Receiver<ApiTestResult>>,
    result: Option<ApiTestResult>,
    message: Option<(bool, String)>,
}

type ApiTestResult = Result<(String, RawResponse, Vec<MappedItem>), String>;

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

fn message_ui(ui: &mut egui::Ui, message: &Option<(bool, String)>) {
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

fn problems_ui(ui: &mut egui::Ui, problems: &[String]) {
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

    // —— 数据接口 ——

    /// 底部「保存设置」顺带保存数据接口页没保存的修改。
    pub(super) fn save_pending_apis(&mut self) {
        let page = &mut self.agent_settings.apis;
        if !page.dirty {
            return;
        }
        match page.store.save().and_then(|_| page.secrets.save()) {
            Ok(()) => {
                page.dirty = false;
                page.message = Some((true, "已随设置一并保存。".into()));
            }
            Err(error) => {
                self.status = format!("数据接口保存失败：{error:#}");
            }
        }
    }

    pub(super) fn apis_section_ui(&mut self, ui: &mut egui::Ui) {
        let page = &mut self.agent_settings.apis;
        if !page.loaded {
            match (ApiStore::load(), ApiSecrets::load()) {
                (Ok(store), Ok(secrets)) => {
                    page.store = store;
                    page.secrets = secrets;
                }
                (store, secrets) => {
                    let errors: Vec<String> = [store.err(), secrets.err()]
                        .into_iter()
                        .flatten()
                        .map(|e| format!("{e:#}"))
                        .collect();
                    page.message = Some((false, errors.join("；")));
                }
            }
            page.loaded = true;
        }
        if let Some(rx) = &page.test {
            match rx.try_recv() {
                Ok(result) => {
                    page.result = Some(result);
                    page.test = None;
                }
                Err(TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(Duration::from_millis(100))
                }
                Err(TryRecvError::Disconnected) => page.test = None,
            }
        }
        ui.horizontal_wrapped(|ui| {
            if theme::icon_button(ui, theme::Icon::Plus, "添加接口").clicked() {
                let n = page.store.endpoints.len() + 1;
                page.store.endpoints.push(ApiEndpoint {
                    id: format!("api{n}"),
                    name: format!("新接口 {n}"),
                    url: "http://".into(),
                    ..ApiEndpoint::default()
                });
                page.selected = Some(page.store.endpoints.len() - 1);
                page.dirty = true;
                page.result = None;
            }
            if theme::primary_icon_button_enabled(
                ui,
                page.dirty,
                theme::Icon::Save,
                "保存接口与密钥",
            )
            .clicked()
            {
                page.message = Some(match page.store.save().and_then(|_| page.secrets.save()) {
                    Ok(()) => {
                        page.dirty = false;
                        (true, "已保存，下次发送时生效。".into())
                    }
                    Err(error) => (false, format!("保存失败：{error:#}")),
                });
            }
            if theme::icon_button(ui, theme::Icon::FileUp, "导入…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("接口配置", &["json"])
                    .pick_file()
            {
                page.message = Some(
                    match std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|text| {
                            serde_json::from_str::<ApiStore>(&text).map_err(|e| e.to_string())
                        }) {
                        Ok(incoming) => {
                            let (added, replaced) = page.store.merge(incoming);
                            page.dirty = true;
                            (
                                true,
                                format!("导入 {added} 个、覆盖 {replaced} 个，记得保存。"),
                            )
                        }
                        Err(error) => (false, format!("导入失败：{error}")),
                    },
                );
            }
            if theme::icon_button(ui, theme::Icon::FileDown, "导出…")
                .on_hover_text("只导出接口定义，不带密钥")
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name("数据接口.json")
                    .add_filter("接口配置", &["json"])
                    .save_file()
            {
                page.message = Some(
                    match serde_json::to_string_pretty(&page.store)
                        .map_err(|e| e.to_string())
                        .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()))
                    {
                        Ok(()) => (true, format!("已导出到 {}（不含密钥）", path.display())),
                        Err(error) => (false, format!("导出失败：{error}")),
                    },
                );
            }
            if page.dirty {
                ui.colored_label(theme::warn(), "有未保存的修改");
            }
        });
        message_ui(ui, &page.message);
        ui.add_space(6.0);
        if page.store.endpoints.is_empty() {
            ui.weak("还没有接口。点「添加接口」，填地址、输入变量和返回映射，再用下面的「测试」试一下。");
        }
        let mut select = None;
        for (index, endpoint) in page.store.endpoints.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        page.selected == Some(index),
                        format!("{}（{}）", endpoint.name, endpoint.id),
                    )
                    .clicked()
                {
                    select = Some(index);
                }
                ui.weak(format!("{} {}", endpoint.method.label(), endpoint.url));
                let problems = endpoint.problems().len();
                if problems > 0 {
                    theme::chip(
                        ui,
                        &format!("{problems} 个问题"),
                        theme::warn(),
                        theme::warn_soft(),
                    );
                }
            });
        }
        if let Some(index) = select {
            page.selected = Some(index);
            page.result = None;
            page.confirm_remove = None;
            page.test_args = example_args_text(&page.store.endpoints[index]);
        }
        if let Some(index) = page.selected.filter(|i| *i < page.store.endpoints.len()) {
            ui.add_space(8.0);
            theme::hairline(ui);
            ui.add_space(8.0);
            let changed = endpoint_form(ui, &mut page.store.endpoints[index]);
            page.dirty |= changed;
            ui.add_space(4.0);
            if page.confirm_remove == Some(index) {
                ui.horizontal(|ui| {
                    if theme::danger_icon_button(ui, theme::Icon::Trash, "确认删除这个接口")
                        .clicked()
                    {
                        page.store.endpoints.remove(index);
                        page.selected = None;
                        page.confirm_remove = None;
                        page.dirty = true;
                    }
                    if ui.button("取消").clicked() {
                        page.confirm_remove = None;
                    }
                });
            } else if theme::icon_button(ui, theme::Icon::Trash, "删除接口").clicked() {
                page.confirm_remove = Some(index);
            }
            if let Some(endpoint) = page.store.endpoints.get(index).cloned() {
                ui.add_space(8.0);
                api_test_ui(ui, page, &endpoint);
            }
        }
        ui.add_space(10.0);
        theme::hairline(ui);
        ui.add_space(8.0);
        page.dirty |= secrets_ui(ui, &mut page.secrets);
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

/// 接口编辑表单。返回是否改了东西。
fn endpoint_form(ui: &mut egui::Ui, endpoint: &mut ApiEndpoint) -> bool {
    let before = endpoint.clone();
    setting_row(
        ui,
        "id",
        Some("技能里写 http.call:<id> 引用它；只能用英文字母、数字、下划线和短横线"),
        |ui| {
            ui.add(egui::TextEdit::singleline(&mut endpoint.id).desired_width(200.0));
        },
    );
    setting_row(ui, "名称", None, |ui| {
        ui.add(egui::TextEdit::singleline(&mut endpoint.name).desired_width(260.0));
    });
    setting_row(
        ui,
        "说明",
        Some("写清这个接口能回答什么，给技能作者与模型看"),
        |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut endpoint.description)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            );
        },
    );
    setting_row(
        ui,
        "方法",
        Some("只做查询：POST 也只用于带请求体的查询"),
        |ui| {
            for method in [ApiMethod::Get, ApiMethod::Post] {
                ui.selectable_value(&mut endpoint.method, method, method.label());
            }
        },
    );
    setting_row(
        ui,
        "地址",
        Some("可以带变量：http://10.0.0.8/api/stat?region={region}"),
        |ui| {
            ui.add(egui::TextEdit::singleline(&mut endpoint.url).desired_width(f32::INFINITY));
        },
    );
    setting_row(ui, "超时（秒）", None, |ui| {
        ui.add(egui::DragValue::new(&mut endpoint.timeout_seconds).range(1..=300));
    });
    ui.add_space(4.0);
    ui.label("输入变量");
    let mut remove = None;
    egui::Grid::new("api_inputs")
        .num_columns(6)
        .striped(true)
        .show(ui, |ui| {
            ui.weak("名字");
            ui.weak("类型");
            ui.weak("必填");
            ui.weak("说明");
            ui.weak("样例");
            ui.end_row();
            for (index, input) in endpoint.inputs.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(&mut input.name).desired_width(90.0));
                egui::ComboBox::from_id_salt(("api_input_kind", index))
                    .selected_text(input.kind.label())
                    .width(60.0)
                    .show_ui(ui, |ui| {
                        for kind in InputKind::ALL {
                            ui.selectable_value(&mut input.kind, kind, kind.label());
                        }
                    });
                ui.checkbox(&mut input.required, "");
                ui.add(egui::TextEdit::singleline(&mut input.description).desired_width(140.0));
                ui.add(egui::TextEdit::singleline(&mut input.example).desired_width(90.0));
                if ui.small_button("删除").clicked() {
                    remove = Some(index);
                }
                ui.end_row();
            }
        });
    if let Some(index) = remove {
        endpoint.inputs.remove(index);
    }
    if ui.small_button("添加输入变量").clicked() {
        endpoint.inputs.push(ApiInput::default());
    }
    ui.add_space(4.0);
    ui.label("请求头（密钥写成 {secret:名字}）");
    let mut remove = None;
    egui::Grid::new("api_headers")
        .num_columns(3)
        .show(ui, |ui| {
            for (index, header) in endpoint.headers.iter_mut().enumerate() {
                ui.add(
                    egui::TextEdit::singleline(&mut header.name)
                        .hint_text("Authorization")
                        .desired_width(140.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut header.value)
                        .hint_text("Bearer {secret:token}")
                        .desired_width(260.0),
                );
                if ui.small_button("删除").clicked() {
                    remove = Some(index);
                }
                ui.end_row();
            }
        });
    if let Some(index) = remove {
        endpoint.headers.remove(index);
    }
    if ui.small_button("添加请求头").clicked() {
        endpoint.headers.push(ApiHeader::default());
    }
    if endpoint.method == ApiMethod::Post {
        ui.add_space(4.0);
        ui.label("请求体模板（JSON；单独一个 \"{变量}\" 会按类型换成数字或是否）");
        ui.add(
            egui::TextEdit::multiline(&mut endpoint.body)
                .code_editor()
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
    }
    ui.add_space(4.0);
    ui.label("返回映射（写英文字段名、/JSON 指针、{模板} 或固定文字；留空按常见字段名猜）");
    let mapping = &mut endpoint.mapping;
    setting_row(
        ui,
        "列表位置",
        Some("JSON 指针，如 /data/items；留空表示整个返回"),
        |ui| {
            ui.add(egui::TextEdit::singleline(&mut mapping.list).desired_width(260.0));
        },
    );
    setting_row(ui, "标题", None, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut mapping.title)
                .hint_text("title")
                .desired_width(260.0),
        );
    });
    setting_row(
        ui,
        "正文",
        Some("如 {region}{year}年共发生森林火灾{count}起"),
        |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut mapping.text)
                    .hint_text("text")
                    .desired_width(f32::INFINITY),
            );
        },
    );
    setting_row(ui, "出处", None, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut mapping.source)
                .hint_text("省统计系统")
                .desired_width(260.0),
        );
    });
    setting_row(
        ui,
        "编号",
        Some("同一条资料只进一次证据包，靠它认"),
        |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut mapping.id)
                    .hint_text("id")
                    .desired_width(160.0),
            );
        },
    );
    setting_row(ui, "返回去处", None, |ui| {
        for destination in [ApiDestination::Evidence, ApiDestination::Variable] {
            ui.selectable_value(&mut endpoint.destination, destination, destination.label());
        }
    });
    problems_ui(ui, &endpoint.problems());
    *endpoint != before
}

fn example_args_text(endpoint: &ApiEndpoint) -> String {
    serde_json::to_string_pretty(&Value::Object(endpoint.example_args())).unwrap_or_default()
}

fn api_test_ui(ui: &mut egui::Ui, page: &mut ApisPage, endpoint: &ApiEndpoint) {
    ui.label(egui::RichText::new("测试").strong());
    ui.weak("填一组样例输入，看实际发出的请求、原始返回与映射结果。用的是上面正在编辑的配置（不必先保存）。");
    if page.test_args.trim().is_empty() {
        page.test_args = example_args_text(endpoint);
    }
    ui.add(
        egui::TextEdit::multiline(&mut page.test_args)
            .code_editor()
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        let running = page.test.is_some();
        if theme::primary_icon_button_enabled(ui, !running, theme::Icon::PlugZap, "测试").clicked()
        {
            match serde_json::from_str::<Value>(if page.test_args.trim().is_empty() {
                "{}"
            } else {
                &page.test_args
            }) {
                Ok(Value::Object(args)) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let endpoint = endpoint.clone();
                    let secrets = page.secrets.clone();
                    std::thread::spawn(move || {
                        let result = api::call(&endpoint, &args, &secrets)
                            .map(|(prepared, raw, items)| (prepared.describe(), raw, items));
                        let _ = tx.send(result);
                    });
                    page.test = Some(rx);
                    page.result = None;
                }
                Ok(_) => {
                    page.result = Some(Err(
                        "样例输入要写成 JSON 对象，如 {\"region\": \"全省\"}".into()
                    ))
                }
                Err(error) => page.result = Some(Err(format!("样例输入不是合法的 JSON：{error}"))),
            }
        }
        if running {
            theme::spinner(ui, 14.0, theme::accent());
            ui.weak("请求中…");
        }
    });
    match &page.result {
        Some(Ok((request, raw, items))) => {
            ui.colored_label(
                theme::success(),
                format!("返回 {}，映射出 {} 条", raw.status, items.len()),
            );
            ui.label("请求");
            code_block(ui, "api_test_request", request);
            ui.label("原始返回");
            let preview: String = raw.body.chars().take(RAW_PREVIEW_CHARS).collect();
            code_block(ui, "api_test_raw", &preview);
            ui.label("映射结果");
            for item in items {
                ui.label(format!("【{}】{}", item.title, item.text));
                ui.weak(format!("出处：{} · 编号：{}", item.source, item.id));
            }
        }
        Some(Err(error)) => {
            ui.colored_label(theme::danger(), error);
        }
        None => {}
    }
}

fn code_block(ui: &mut egui::Ui, id: &str, text: &str) {
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

/// 密钥表。值只在本机，输入框打码。返回是否改了东西。
fn secrets_ui(ui: &mut egui::Ui, secrets: &mut ApiSecrets) -> bool {
    ui.label(egui::RichText::new("密钥").strong());
    ui.weak("请求头或地址里写 {secret:名字} 引用。密钥单独存放在本机，导出接口时不带。");
    let mut changed = false;
    let mut remove = None;
    let mut renames = Vec::new();
    egui::Grid::new("api_secrets")
        .num_columns(3)
        .show(ui, |ui| {
            for (name, value) in secrets.secrets.iter_mut() {
                let mut new_name = name.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut new_name).desired_width(140.0))
                    .changed()
                {
                    renames.push((name.clone(), new_name));
                }
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(value)
                            .password(true)
                            .desired_width(260.0),
                    )
                    .changed();
                if ui.small_button("删除").clicked() {
                    remove = Some(name.clone());
                }
                ui.end_row();
            }
        });
    for (old, new) in renames {
        if !new.is_empty()
            && !secrets.secrets.contains_key(&new)
            && let Some(value) = secrets.secrets.remove(&old)
        {
            secrets.secrets.insert(new, value);
            changed = true;
        }
    }
    if let Some(name) = remove {
        secrets.secrets.remove(&name);
        changed = true;
    }
    if ui.small_button("添加密钥").clicked() {
        let mut n = secrets.secrets.len() + 1;
        while secrets.secrets.contains_key(&format!("key{n}")) {
            n += 1;
        }
        secrets.secrets.insert(format!("key{n}"), String::new());
        changed = true;
    }
    changed
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

    /// 在一个 egui 帧里画一遍，返回画面上的文字。
    fn render(mut add: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 3000.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(raw(), |ui| add(ui));
        let output = ctx.run_ui(raw(), |ui| add(ui));
        fn collect(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
            match shape {
                egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }

    #[test]
    fn the_api_form_test_panel_and_secrets_render() {
        let mut endpoint = ApiEndpoint {
            id: "stat".into(),
            name: "火灾统计".into(),
            method: ApiMethod::Post,
            url: "http://10.0.0.8/stat".into(),
            body: "{\"q\": \"{region}\"}".into(),
            inputs: vec![ApiInput {
                name: "region".into(),
                example: "全省".into(),
                ..ApiInput::default()
            }],
            ..ApiEndpoint::default()
        };
        let texts = render(|ui| {
            endpoint_form(ui, &mut endpoint);
        });
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("请求体模板") && has("返回映射") && has("输入变量"),
            "{texts:?}"
        );
        assert_eq!(
            example_args_text(&endpoint),
            "{
  \"region\": \"全省\"
}"
        );

        let mut page = ApisPage {
            result: Some(Ok((
                "GET http://x".into(),
                RawResponse {
                    status: 200,
                    body: "{}".into(),
                },
                vec![MappedItem {
                    id: "1".into(),
                    title: "年度统计".into(),
                    text: "共12起".into(),
                    source: "统计系统".into(),
                }],
            ))),
            ..ApisPage::default()
        };
        let texts = render(|ui| api_test_ui(ui, &mut page, &endpoint));
        assert!(
            texts.iter().any(|t| t.contains("返回 200，映射出 1 条")),
            "{texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("【年度统计】共12起")),
            "{texts:?}"
        );

        let mut secrets = ApiSecrets::default();
        secrets.secrets.insert("token".into(), "s3cr3t".into());
        let texts = render(|ui| {
            secrets_ui(ui, &mut secrets);
        });
        assert!(
            !texts.iter().any(|t| t.contains("s3cr3t")),
            "密钥打码显示：{texts:?}"
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
