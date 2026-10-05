//! AI 管理页「数据接口」分区（`docs/http-skill-design.md` 第一层）。
//!
//! - 首页是接口卡片列表：能查什么、要填什么、最近一次测试结果；
//! - 点开是详情：先是易读的名称、说明与查询条件，「鉴权」里直接粘贴 Key（没配鉴权的可以
//!   在这里加上），「试一下」按查询条件逐项填样例实测，地址、请求头、请求体、返回映射这些
//!   技术配置收在「高级配置」里；
//! - 密钥由程序统一管：卡片上标「缺密钥」「鉴权没过」，密钥表写明用在哪些接口；
//! - 「从文档添加」见 [`import`]：粘贴文档或选文件，程序解析 + 起草模型整理，自动试调一次。
//!
//! 实测在后台线程里跑，界面每帧取一次结果；测试结论记进 `api-tests.json`。

mod import;

use super::agent_settings::{code_block, message_ui, problems_ui};
use super::settings::setting_row;
use crate::agent::api::{
    self, ApiDestination, ApiEndpoint, ApiHeader, ApiInput, ApiMethod, ApiSecrets, ApiStore,
    ApiTestLog, InputKind, TestStatus, Trial,
};
use crate::agent::api_import::auth::{self, AuthPlace, AuthSpec};
use crate::agent::api_import::{infer, redact, rename_secret_refs};
use crate::app::GongwenApp;
use crate::theme;
use eframe::egui;
use import::ImportFlow;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

/// 原始返回最多显示这么多字，再长的界面上看不完，也拖慢界面。
const RAW_PREVIEW_CHARS: usize = 4000;
/// 测试结果里预览几条。
const PREVIEW_ITEMS: usize = 5;

/// 数据接口分区的状态。接口与密钥要点保存才落盘；测试记录随测随存。
#[derive(Default)]
pub(crate) struct ApisPage {
    loaded: bool,
    store: ApiStore,
    secrets: ApiSecrets,
    log: ApiTestLog,
    dirty: bool,
    view: View,
    message: Option<(bool, String)>,
    detail: Detail,
    import: Option<ImportFlow>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum View {
    #[default]
    List,
    Detail(usize),
    Import,
}

/// 详情页的临时状态，换接口时清空。
#[derive(Default)]
struct Detail {
    confirm_remove: bool,
    /// 「试一下」里填的值：参数名 → 值。
    args: BTreeMap<String, String>,
    test: Option<Receiver<Trial>>,
    result: Option<Trial>,
    /// 「鉴权」里改过 Key、还没重测。
    key_edited: bool,
    auth_form: AuthForm,
    /// 加鉴权时只加这一个接口（默认同一服务器上没带鉴权的一起加：一个服务通常共用一个 Key）。
    auth_this_only: bool,
}

/// 「添加鉴权」的小表单：密钥放请求头还是地址参数、叫什么、要不要 Bearer。
pub(super) struct AuthForm {
    place: AuthPlace,
    name: String,
    bearer: bool,
}

impl Default for AuthForm {
    fn default() -> Self {
        Self {
            place: AuthPlace::Header,
            name: "Authorization".into(),
            bearer: true,
        }
    }
}

/// 画「添加鉴权」表单；点了「加上」返回鉴权方式。
pub(super) fn auth_form_ui(ui: &mut egui::Ui, form: &mut AuthForm) -> Option<AuthSpec> {
    let mut added = None;
    ui.horizontal_wrapped(|ui| {
        ui.label("密钥放在");
        ui.selectable_value(&mut form.place, AuthPlace::Header, "请求头");
        ui.selectable_value(&mut form.place, AuthPlace::Query, "地址参数");
        ui.label("名字");
        ui.add(
            egui::TextEdit::singleline(&mut form.name)
                .hint_text("Authorization / X-API-Key / token")
                .desired_width(150.0),
        );
        if form.place == AuthPlace::Header {
            ui.checkbox(&mut form.bearer, "值前加 Bearer");
        }
        let spec = AuthSpec {
            place: form.place,
            name: form.name.trim().to_string(),
            bearer: form.bearer && form.place == AuthPlace::Header,
            basis: "手动添加".into(),
            value: String::new(),
        };
        if ui
            .add_enabled(spec.valid_name(), egui::Button::new("加上"))
            .on_disabled_hover_text(
                "名字用英文字母开头，只能有字母、数字、下划线（请求头还可以有短横线）",
            )
            .clicked()
        {
            added = Some(spec);
        }
    });
    added
}

/// 新加的鉴权用哪个密钥名：按字段名起，已有同名的沿用（同一个服务通常共用一个 Key）。
pub(super) fn secret_for(spec: &AuthSpec, secrets: &mut ApiSecrets) -> String {
    let name = redact::secret_name(&spec.name);
    secrets.secrets.entry(name.clone()).or_default();
    name
}

/// `http://10.0.0.9:8080/api/x` → `http://10.0.0.9:8080`。
pub(super) fn host_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split(['/', '?']).next().unwrap_or_default();
            format!("{scheme}://{host}").to_ascii_lowercase()
        }
        None => String::new(),
    }
}

/// 密钥输入框：打码，提示把 Key 粘进来。
pub(super) fn key_field(ui: &mut egui::Ui, value: &mut String) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .hint_text("把 Key 粘到这里")
            .desired_width(260.0),
    )
}

impl ApisPage {
    fn load(&mut self) {
        let mut errors = Vec::new();
        match ApiStore::load() {
            Ok(store) => self.store = store,
            Err(error) => errors.push(format!("{error:#}")),
        }
        match ApiSecrets::load() {
            Ok(secrets) => self.secrets = secrets,
            Err(error) => errors.push(format!("{error:#}")),
        }
        self.log = ApiTestLog::load().unwrap_or_default();
        if !errors.is_empty() {
            self.message = Some((false, errors.join("；")));
        }
        self.loaded = true;
    }

    fn save(&mut self) -> anyhow::Result<()> {
        self.store.save()?;
        self.secrets.save()?;
        self.dirty = false;
        Ok(())
    }

    fn open(&mut self, index: usize) {
        self.detail = Detail::default();
        if let Some(endpoint) = self.store.endpoints.get(index) {
            self.detail.args = endpoint
                .inputs
                .iter()
                .map(|input| (input.name.clone(), input.example.clone()))
                .collect();
        }
        self.view = View::Detail(index);
    }

    /// 记一次测试结论并落盘（测试记录不等「保存」）。
    fn record(&mut self, endpoint: &ApiEndpoint, trial: &Trial) {
        self.log.record(endpoint, trial);
        if let Err(error) = self.log.save() {
            self.message = Some((false, format!("测试记录保存失败：{error:#}")));
        }
    }
}

impl GongwenApp {
    /// 底部「保存」顺带保存数据接口页没保存的修改。
    pub(super) fn save_pending_apis(&mut self) {
        let page = &mut self.agent_settings.apis;
        if !page.dirty {
            return;
        }
        match page.save() {
            Ok(()) => page.message = Some((true, "已随设置一并保存。".into())),
            Err(error) => self.status = format!("数据接口保存失败：{error:#}"),
        }
    }

    pub(super) fn apis_section_ui(&mut self, ui: &mut egui::Ui) {
        let config = &self.config;
        let page = &mut self.agent_settings.apis;
        if !page.loaded {
            page.load();
        }
        match page.view {
            View::List => list_ui(ui, page),
            View::Detail(index) if index < page.store.endpoints.len() => detail_ui(ui, page, index),
            View::Detail(_) => page.view = View::List,
            View::Import => import::import_ui(ui, page, config),
        }
    }
}

// —— 列表 ——

fn list_ui(ui: &mut egui::Ui, page: &mut ApisPage) {
    ui.horizontal_wrapped(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::WandSparkles, "从文档添加").clicked() {
            page.import.get_or_insert_with(ImportFlow::default);
            page.view = View::Import;
            page.message = None;
        }
        if ui
            .add(theme::secondary_icon_button(theme::Icon::Plus, "手动添加"))
            .clicked()
        {
            let n = page.store.endpoints.len() + 1;
            let mut id = format!("api{n}");
            let mut k = n;
            while page.store.get(&id).is_some() {
                k += 1;
                id = format!("api{k}");
            }
            page.store.endpoints.push(ApiEndpoint {
                id,
                name: format!("新接口 {n}"),
                url: "http://".into(),
                ..ApiEndpoint::default()
            });
            page.dirty = true;
            page.open(page.store.endpoints.len() - 1);
        }
        if ui
            .add(theme::secondary_icon_button(
                theme::Icon::FileUp,
                "导入配置…",
            ))
            .on_hover_text("导入别处导出的接口配置文件（JSON）；同 id 的会覆盖")
            .clicked()
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
        if ui
            .add(theme::secondary_icon_button(
                theme::Icon::FileDown,
                "导出配置…",
            ))
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
        save_button(ui, page);
    });
    message_ui(ui, &page.message);
    ui.add_space(8.0);
    if page.store.endpoints.is_empty() {
        theme::notice(
            ui,
            theme::Icon::HelpCircle,
            theme::info(),
            theme::accent_soft(),
            "还没有接口。点「从文档添加」，粘贴接口文档或 cURL 命令，程序会自动填好配置并试调一次。",
        );
    }
    let mut open = None;
    for (index, endpoint) in page.store.endpoints.iter().enumerate() {
        let status = page.log.status(endpoint);
        let missing = page.secrets.missing(endpoint);
        let response = theme::clickable_card(ui, ("api_card", index), theme::card(), false, |ui| {
            ui.set_width(ui.available_width());
            endpoint_card(ui, endpoint, status, &missing);
        })
        .response;
        if response.clicked() {
            open = Some(index);
        }
        ui.add_space(6.0);
    }
    if let Some(index) = open {
        page.open(index);
    }
    ui.add_space(6.0);
    let unfilled = page
        .secrets
        .secrets
        .keys()
        .filter(|name| !page.secrets.filled(name))
        .count();
    let title = if unfilled > 0 {
        format!(
            "密钥（{} 个，{unfilled} 个没填）",
            page.secrets.secrets.len()
        )
    } else {
        format!("密钥（{} 个）", page.secrets.secrets.len())
    };
    egui::CollapsingHeader::new(title)
        .id_salt("api_secrets_section")
        .default_open(unfilled > 0)
        .show(ui, |ui| {
            page.dirty |= secrets_ui(ui, &mut page.secrets, &mut page.store, &page.log);
        });
}

fn save_button(ui: &mut egui::Ui, page: &mut ApisPage) {
    if !page.dirty {
        return;
    }
    if theme::primary_icon_button(ui, theme::Icon::Save, "保存").clicked() {
        page.message = Some(match page.save() {
            Ok(()) => (true, "已保存，下次发送时生效。".into()),
            Err(error) => (false, format!("保存失败：{error:#}")),
        });
    }
    ui.colored_label(theme::warn(), "有未保存的修改");
}

/// 一张接口卡片：名称与状态、能查什么、要填什么、地址。
fn endpoint_card(
    ui: &mut egui::Ui,
    endpoint: &ApiEndpoint,
    status: TestStatus<'_>,
    missing: &[String],
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&endpoint.name).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            status_chip(ui, endpoint, status, missing);
        });
    });
    if endpoint.description.trim().is_empty() {
        ui.colored_label(theme::warn(), "还没写说明：模型靠说明判断什么时候该用它");
    } else {
        ui.label(endpoint.description.trim());
    }
    if !endpoint.inputs.is_empty() {
        theme::caption(ui, &format!("查询条件：{}", inputs_summary(endpoint)));
    }
    ui.label(
        egui::RichText::new(format!("{} {}", endpoint.method.label(), endpoint.url))
            .size(theme::font_sizes::SMALL)
            .color(theme::text_muted()),
    );
}

/// 「地区（必填）、年份」。
fn inputs_summary(endpoint: &ApiEndpoint) -> String {
    endpoint
        .inputs
        .iter()
        .map(|input| {
            let label = if input.description.trim().is_empty() {
                input.name.clone()
            } else {
                input.description.trim().to_string()
            };
            if input.required {
                format!("{label}（必填）")
            } else {
                label
            }
        })
        .collect::<Vec<_>>()
        .join("、")
}

fn status_chip(
    ui: &mut egui::Ui,
    endpoint: &ApiEndpoint,
    status: TestStatus<'_>,
    missing: &[String],
) {
    let problems = endpoint.problems().len();
    if problems > 0 {
        theme::chip(
            ui,
            &format!("配置有 {problems} 个问题"),
            theme::danger(),
            theme::danger_soft(),
        );
        return;
    }
    if !missing.is_empty() {
        theme::chip(ui, "缺密钥", theme::warn(), theme::warn_soft()).on_hover_text(format!(
            "密钥「{}」还没填：点开接口，在「鉴权」里粘贴",
            missing.join("」「")
        ));
        return;
    }
    match status {
        TestStatus::Failed(record) if record.auth => {
            theme::chip(
                ui,
                &format!("鉴权没过 · {}", record.at),
                theme::danger(),
                theme::danger_soft(),
            )
            .on_hover_text(format!(
                "Key 可能不对或已过期，点开接口在「鉴权」里重新粘贴。{}",
                record.summary
            ));
        }
        TestStatus::Untested => {
            theme::chip(ui, "未测试", theme::text_muted(), theme::surface_sunk());
        }
        TestStatus::Passed(record) => {
            theme::chip(
                ui,
                &format!("测试通过 · {}", record.at),
                theme::success(),
                theme::success_soft(),
            )
            .on_hover_text(&record.summary);
        }
        TestStatus::Failed(record) => {
            theme::chip(
                ui,
                &format!("测试失败 · {}", record.at),
                theme::danger(),
                theme::danger_soft(),
            )
            .on_hover_text(&record.summary);
        }
        TestStatus::Stale(record) => {
            theme::chip(ui, "配置改过，需重测", theme::warn(), theme::warn_soft())
                .on_hover_text(format!("上次 {}：{}", record.at, record.summary));
        }
    }
}

// —— 详情 ——

fn detail_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    if let Some(rx) = &page.detail.test {
        match rx.try_recv() {
            Ok(trial) => {
                let endpoint = page.store.endpoints[index].clone();
                page.record(&endpoint, &trial);
                page.detail.result = Some(trial);
                page.detail.test = None;
            }
            Err(TryRecvError::Empty) => ui.ctx().request_repaint_after(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => page.detail.test = None,
        }
    }
    ui.horizontal(|ui| {
        if ui
            .add(theme::secondary_icon_button(theme::Icon::List, "全部接口"))
            .clicked()
        {
            page.view = View::List;
        }
        save_button(ui, page);
    });
    message_ui(ui, &page.message);
    ui.add_space(6.0);
    let status = page.log.status(&page.store.endpoints[index]);
    let missing = page.secrets.missing(&page.store.endpoints[index]);
    ui.horizontal(|ui| {
        ui.heading(&page.store.endpoints[index].name);
        status_chip(ui, &page.store.endpoints[index], status, &missing);
    });
    ui.add_space(6.0);

    page.dirty |= basic_form(ui, &mut page.store.endpoints[index], "detail");
    ui.add_space(10.0);
    auth_section_ui(ui, page, index);
    ui.add_space(10.0);
    let endpoint = &mut page.store.endpoints[index];
    section_title(ui, "试一下");
    try_ui(ui, &mut page.detail, endpoint, &page.secrets);
    if let Some(trial) = &page.detail.result
        && trial.raw.is_some()
    {
        // 拿到了真实返回、而返回映射还有空着的：给个一键补全。
        let mut preview = endpoint.clone();
        let inferred = trial
            .raw
            .as_ref()
            .and_then(|raw| serde_json::from_str::<Value>(&raw.body).ok())
            .map(|root| infer::infer(&root));
        if let Some(inferred) = inferred {
            let filled = infer::fill_mapping(&mut preview.mapping, &mut preview.success, &inferred);
            if !filled.is_empty()
                && ui
                    .add(theme::secondary_icon_button(
                        theme::Icon::WandSparkles,
                        &format!("按这次的返回补全：{}", filled.join("、")),
                    ))
                    .clicked()
            {
                *endpoint = preview;
                page.dirty = true;
                if let Some(trial) = &mut page.detail.result {
                    trial.remap(endpoint);
                }
            }
        }
    }
    ui.add_space(10.0);
    egui::CollapsingHeader::new("高级配置（地址、请求头、请求体、返回映射、成功判据）")
        .id_salt(("api_advanced", index))
        .show(ui, |ui| {
            page.dirty |= advanced_form(ui, endpoint, "detail");
        });
    problems_ui(ui, &endpoint.problems());
    ui.add_space(10.0);
    if page.detail.confirm_remove {
        ui.horizontal(|ui| {
            if theme::danger_icon_button(ui, theme::Icon::Trash, "确认删除这个接口").clicked()
            {
                page.store.endpoints.remove(index);
                page.dirty = true;
                page.view = View::List;
                page.detail = Detail::default();
            }
            ui.label("再点一次垃圾桶确认删除");
            if ui.button("取消").clicked() {
                page.detail.confirm_remove = false;
            }
        });
    } else if ui
        .add(theme::warning_icon_button(theme::Icon::Trash, "删除接口"))
        .clicked()
    {
        page.detail.confirm_remove = true;
    }
}

fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong());
    ui.add_space(2.0);
}

/// 详情里的「鉴权」：这个接口要的密钥直接在这里粘贴；没配鉴权的可以加上。
/// 改了 Key、离开输入框就自动重测一次。
fn auth_section_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    section_title(ui, "鉴权");
    let names = page.store.endpoints[index].secret_names();
    if names.is_empty() {
        let host = host_of(&page.store.endpoints[index].url);
        let others = page
            .store
            .endpoints
            .iter()
            .enumerate()
            .filter(|(i, e)| *i != index && host_of(&e.url) == host && !auth::has_auth(e))
            .count();
        theme::caption(ui, "这个接口没配密钥。接口要 Key 的话，在这里加上：");
        let added = auth_form_ui(ui, &mut page.detail.auth_form);
        if others > 0 {
            let mut all = !page.detail.auth_this_only;
            ui.checkbox(
                &mut all,
                format!("同一服务器上没配密钥的其他 {others} 个接口也加上"),
            );
            page.detail.auth_this_only = !all;
        }
        if let Some(spec) = added {
            let secret = secret_for(&spec, &mut page.secrets);
            let all = !page.detail.auth_this_only;
            for (i, endpoint) in page.store.endpoints.iter_mut().enumerate() {
                if i == index
                    || (all && host_of(&endpoint.url) == host && !auth::has_auth(endpoint))
                {
                    auth::apply(endpoint, &spec, &secret);
                }
            }
            page.dirty = true;
        }
        return;
    }
    let mut retest = false;
    let how = auth::from_endpoint(&page.store.endpoints[index]).map(|(spec, _)| spec.describe());
    for name in &names {
        let users: Vec<String> = page
            .store
            .endpoints
            .iter()
            .enumerate()
            .filter(|(i, e)| *i != index && e.secret_names().contains(name))
            .map(|(_, e)| e.name.clone())
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("密钥「{name}」"));
            let value = page.secrets.secrets.entry(name.clone()).or_default();
            let response = key_field(ui, value);
            let filled = !value.trim().is_empty();
            if response.changed() {
                page.dirty = true;
                page.detail.key_edited = true;
            }
            if response.lost_focus() && page.detail.key_edited && filled {
                page.detail.key_edited = false;
                retest = true;
            }
            if filled {
                theme::caption(ui, "已填");
            } else {
                ui.colored_label(theme::warn(), "未填");
            }
        });
        if !users.is_empty() {
            theme::caption(ui, &format!("也用在：{}（改了一起生效）", users.join("、")));
        }
    }
    if let Some(how) = how {
        theme::caption(
            ui,
            &format!("带法：{how}。要改带法在「高级配置」的请求头里改。"),
        );
    }
    theme::caption(ui, "Key 只存在本机，不导出、不发给模型；记得保存。");
    if retest && page.detail.test.is_none() {
        let endpoint = page.store.endpoints[index].clone();
        start_test(&mut page.detail, &endpoint, &page.secrets);
    }
}

/// 用「试一下」里填的值后台实测一次。
fn start_test(detail: &mut Detail, endpoint: &ApiEndpoint, secrets: &ApiSecrets) {
    let args: Map<String, Value> = endpoint
        .inputs
        .iter()
        .filter_map(|input| {
            let value = detail.args.get(&input.name)?.trim();
            (!value.is_empty()).then(|| (input.name.clone(), Value::String(value.into())))
        })
        .collect();
    detail.test = Some(spawn_trial(endpoint.clone(), args, secrets.clone()));
    detail.result = None;
}

/// 「试一下」：每个查询条件一个输入框，点测试走正式调用的同一条路。
fn try_ui(ui: &mut egui::Ui, detail: &mut Detail, endpoint: &ApiEndpoint, secrets: &ApiSecrets) {
    if endpoint.inputs.is_empty() {
        theme::caption(ui, "这个接口不需要查询条件。");
    }
    egui::Grid::new("api_try_args")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for input in &endpoint.inputs {
                let label = if input.description.trim().is_empty() {
                    input.name.clone()
                } else {
                    format!("{}（{}）", input.description.trim(), input.name)
                };
                ui.label(if input.required {
                    format!("{label} *")
                } else {
                    label
                });
                let value = detail.args.entry(input.name.clone()).or_default();
                ui.add(egui::TextEdit::singleline(value).desired_width(260.0));
                ui.end_row();
            }
        });
    let running = detail.test.is_some();
    ui.horizontal(|ui| {
        if theme::primary_icon_button_enabled(ui, !running, theme::Icon::PlugZap, "测试").clicked()
        {
            start_test(detail, endpoint, secrets);
        }
        if running {
            theme::spinner(ui, 14.0, theme::accent());
            ui.weak("请求中…");
        } else {
            ui.weak("用的是正在编辑的配置，不必先保存。");
        }
    });
    if let Some(trial) = &detail.result {
        ui.add_space(4.0);
        trial_ui(ui, trial, "detail");
        if trial.auth_rejected() {
            auth_hint(ui, auth::has_auth(endpoint));
        }
    }
}

/// 实测像是鉴权没过时的提示。
pub(super) fn auth_hint(ui: &mut egui::Ui, has_auth: bool) {
    theme::notice(
        ui,
        theme::Icon::Shield,
        theme::warn(),
        theme::warn_soft(),
        if has_auth {
            "看起来是鉴权没过：Key 不对、过期，或者接口要的带法和这里配的不一样。在「鉴权」里重新粘贴 Key，离开输入框会自动重测。"
        } else {
            "看起来接口要鉴权，但这里没配密钥：在「鉴权」里选好带法、加上，再粘贴 Key。"
        },
    );
}

/// 后台实测一次。
pub(super) fn spawn_trial(
    endpoint: ApiEndpoint,
    args: Map<String, Value>,
    secrets: ApiSecrets,
) -> Receiver<Trial> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(api::trial(&endpoint, &args, &secrets));
    });
    rx
}

/// 实测结果：结论、条目预览、请求与原始返回（折叠）。
pub(super) fn trial_ui(ui: &mut egui::Ui, trial: &Trial, salt: &str) {
    match &trial.error {
        None => {
            theme::notice(
                ui,
                theme::Icon::Check,
                theme::success(),
                theme::success_soft(),
                format!("调通了：{}", trial.summary()),
            );
        }
        Some(error) => {
            theme::notice(
                ui,
                theme::Icon::TriangleAlert,
                theme::danger(),
                theme::danger_soft(),
                error.clone(),
            );
        }
    }
    for item in trial.items.iter().take(PREVIEW_ITEMS) {
        ui.label(format!(
            "【{}】{}",
            item.title,
            crate::agent::tools::short(&item.text, 120)
        ));
        theme::caption(ui, &format!("出处：{} · 编号：{}", item.source, item.id));
    }
    if trial.items.len() > PREVIEW_ITEMS {
        theme::caption(ui, &format!("……共 {} 条", trial.items.len()));
    }
    if trial.request.is_some() || trial.raw.is_some() {
        egui::CollapsingHeader::new("请求与原始返回")
            .id_salt(("api_trial_raw", salt))
            .show(ui, |ui| {
                if let Some(request) = &trial.request {
                    ui.label("请求（密钥已打码）");
                    code_block(ui, &format!("api_request_{salt}"), request);
                }
                if let Some(raw) = &trial.raw {
                    ui.label(format!("原始返回（状态 {}）", raw.status));
                    let preview: String = raw.body.chars().take(RAW_PREVIEW_CHARS).collect();
                    code_block(ui, &format!("api_raw_{salt}"), &preview);
                }
            });
    }
}

// —— 表单 ——

/// 名称、说明与查询条件。返回是否改了东西。
pub(super) fn basic_form(ui: &mut egui::Ui, endpoint: &mut ApiEndpoint, salt: &str) -> bool {
    let before = endpoint.clone();
    setting_row(ui, "名称", None, |ui| {
        ui.add(egui::TextEdit::singleline(&mut endpoint.name).desired_width(260.0));
    });
    setting_row(
        ui,
        "说明",
        Some("这个接口能查什么、按什么查。AI 靠它判断什么时候该调用"),
        |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut endpoint.description)
                    .desired_rows(2)
                    .hint_text("例如：按地区、年份查森林火灾起数")
                    .desired_width(f32::INFINITY),
            );
        },
    );
    ui.add_space(4.0);
    ui.label("查询条件");
    let mut remove = None;
    egui::Grid::new(("api_inputs", salt))
        .num_columns(6)
        .striped(true)
        .show(ui, |ui| {
            ui.weak("参数名");
            ui.weak("说明");
            ui.weak("类型");
            ui.weak("必填");
            ui.weak("样例");
            ui.end_row();
            for (index, input) in endpoint.inputs.iter_mut().enumerate() {
                ui.add(egui::TextEdit::singleline(&mut input.name).desired_width(90.0));
                ui.add(
                    egui::TextEdit::singleline(&mut input.description)
                        .hint_text("如 地区名称")
                        .desired_width(150.0),
                );
                egui::ComboBox::from_id_salt(("api_input_kind", salt, index))
                    .selected_text(input.kind.label())
                    .width(60.0)
                    .show_ui(ui, |ui| {
                        for kind in InputKind::ALL {
                            ui.selectable_value(&mut input.kind, kind, kind.label());
                        }
                    });
                ui.checkbox(&mut input.required, "");
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
    if ui.small_button("添加查询条件").clicked() {
        endpoint.inputs.push(ApiInput::default());
    }
    *endpoint != before
}

/// 技术配置：id、方法、地址、请求头、请求体、返回映射、成功判据、去处、超时。
pub(super) fn advanced_form(ui: &mut egui::Ui, endpoint: &mut ApiEndpoint, salt: &str) -> bool {
    let before = endpoint.clone();
    setting_row(
        ui,
        "id",
        Some("技能里写 http.call:<id> 引用它；只能用英文字母、数字、下划线和短横线"),
        |ui| {
            ui.add(egui::TextEdit::singleline(&mut endpoint.id).desired_width(200.0));
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
    ui.label("请求头（密钥写成 {secret:名字}）");
    let mut remove = None;
    egui::Grid::new(("api_headers", salt))
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
    setting_row(
        ui,
        "成功判据",
        Some(
            "返回里表示查询成功的字段与取值，如 /code 等于 0；多个取值用 | 分开。不填则 HTTP 200 就算成功",
        ),
        |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut endpoint.success.pointer)
                    .hint_text("/code")
                    .desired_width(120.0),
            );
            ui.label("等于");
            ui.add(
                egui::TextEdit::singleline(&mut endpoint.success.equals)
                    .hint_text("0")
                    .desired_width(80.0),
            );
        },
    );
    setting_row(ui, "返回去处", None, |ui| {
        for destination in [ApiDestination::Evidence, ApiDestination::Variable] {
            ui.selectable_value(&mut endpoint.destination, destination, destination.label());
        }
    });
    *endpoint != before
}

/// 密钥表。值只在本机，输入框打码；每个密钥写明用在哪些接口、最近测试鉴权过没过。
/// 改名时接口里的引用跟着改。返回是否改了东西。
fn secrets_ui(
    ui: &mut egui::Ui,
    secrets: &mut ApiSecrets,
    store: &mut ApiStore,
    log: &ApiTestLog,
) -> bool {
    ui.weak("Key 单独存放在本机，导出接口、同步稿件时都不带，也不发给模型。");
    let mut changed = false;
    let mut remove = None;
    let mut renames = Vec::new();
    egui::Grid::new("api_secrets")
        .num_columns(4)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for (name, value) in secrets.secrets.iter_mut() {
                let mut new_name = name.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut new_name).desired_width(140.0))
                    .changed()
                {
                    renames.push((name.clone(), new_name));
                }
                changed |= key_field(ui, value).changed();
                let users: Vec<&ApiEndpoint> = store
                    .endpoints
                    .iter()
                    .filter(|e| e.secret_names().contains(name))
                    .collect();
                let rejected: Vec<&str> = users
                    .iter()
                    .filter(|e| matches!(log.status(e), TestStatus::Failed(r) if r.auth))
                    .map(|e| e.name.as_str())
                    .collect();
                ui.vertical(|ui| {
                    if value.trim().is_empty() {
                        ui.colored_label(theme::warn(), "没填");
                    } else if !rejected.is_empty() {
                        ui.colored_label(
                            theme::danger(),
                            format!("「{}」鉴权没过，Key 可能已过期", rejected.join("」「")),
                        );
                    }
                    if users.is_empty() {
                        theme::caption(ui, "没有接口在用");
                    } else {
                        let names: Vec<&str> = users.iter().map(|e| e.name.as_str()).collect();
                        theme::caption(ui, &format!("用在：{}", names.join("、")));
                    }
                });
                if ui.small_button("删除").clicked() {
                    remove = Some(name.clone());
                }
                ui.end_row();
            }
        });
    for (old, new) in renames {
        let valid = new.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && new.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid
            && !secrets.secrets.contains_key(&new)
            && let Some(value) = secrets.secrets.remove(&old)
        {
            secrets.secrets.insert(new.clone(), value);
            let renamed = [(old, new)];
            for endpoint in &mut store.endpoints {
                rename_secret_refs(endpoint, &renamed);
            }
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::agent::api::{MappedItem, RawResponse};

    /// 在一个 egui 帧里画一遍，返回画面上的文字。
    pub(in crate::app) fn render(mut add: impl FnMut(&mut egui::Ui)) -> Vec<String> {
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

    fn endpoint() -> ApiEndpoint {
        ApiEndpoint {
            id: "stat".into(),
            name: "火灾统计".into(),
            description: "按地区查森林火灾起数".into(),
            method: ApiMethod::Post,
            url: "http://10.0.0.8/stat".into(),
            body: "{\"q\": \"{region}\"}".into(),
            inputs: vec![ApiInput {
                name: "region".into(),
                description: "地区".into(),
                required: true,
                example: "全省".into(),
                ..ApiInput::default()
            }],
            ..ApiEndpoint::default()
        }
    }

    #[test]
    fn the_list_shows_what_each_endpoint_answers_and_its_test_status() {
        let mut page = ApisPage {
            loaded: true,
            store: ApiStore {
                endpoints: vec![endpoint()],
            },
            ..ApisPage::default()
        };
        let trial = Trial::default();
        page.log.record(&page.store.endpoints[0], &trial);
        let texts = render(|ui| list_ui(ui, &mut page));
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("火灾统计") && has("按地区查森林火灾起数"), "{texts:?}");
        assert!(has("查询条件：地区（必填）"), "{texts:?}");
        assert!(has("测试通过"), "{texts:?}");
        assert!(has("从文档添加") && has("密钥（0 个）"), "{texts:?}");
    }

    #[test]
    fn missing_keys_show_on_the_card_and_are_pasted_in_the_detail() {
        let mut with_key = endpoint();
        with_key.headers.push(ApiHeader {
            name: "Authorization".into(),
            value: "Bearer {secret:token}".into(),
        });
        let mut page = ApisPage {
            loaded: true,
            store: ApiStore {
                endpoints: vec![with_key],
            },
            ..ApisPage::default()
        };
        page.secrets.secrets.insert("token".into(), String::new());
        let texts = render(|ui| list_ui(ui, &mut page));
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("缺密钥"), "{texts:?}");
        assert!(has("1 个没填") && has("用在：火灾统计"), "{texts:?}");

        page.open(0);
        let texts = render(|ui| detail_ui(ui, &mut page, 0));
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("鉴权") && has("密钥「token」") && has("未填"),
            "{texts:?}"
        );
        assert!(has("请求头 Authorization: Bearer 密钥"), "{texts:?}");

        let mut bare = ApisPage {
            loaded: true,
            store: ApiStore {
                endpoints: vec![endpoint()],
            },
            ..ApisPage::default()
        };
        bare.open(0);
        let texts = render(|ui| detail_ui(ui, &mut bare, 0));
        assert!(
            texts.iter().any(|t| t.contains("这个接口没配密钥")),
            "{texts:?}"
        );
    }

    #[test]
    fn auth_failures_are_recognised() {
        let failed = |status: u16, body: &str| Trial {
            raw: Some(RawResponse {
                status,
                body: body.into(),
            }),
            error: Some("失败".into()),
            ..Trial::default()
        };
        assert!(failed(401, "").auth_rejected());
        assert!(failed(200, "<!DOCTYPE html><html>请登录</html>").auth_rejected());
        assert!(failed(200, "{\"code\": 401, \"msg\": \"x\"}").auth_rejected());
        assert!(failed(200, "{\"code\": 500, \"msg\": \"token 已过期\"}").auth_rejected());
        assert!(!failed(404, "not found").auth_rejected());
        assert!(!failed(500, "<html>服务器内部错误</html>").auth_rejected());
        assert!(!failed(200, "{\"code\": 500, \"msg\": \"查询超时\"}").auth_rejected());
        let mut record_log = ApiTestLog::default();
        record_log.record(&endpoint(), &failed(403, ""));
        assert!(record_log.records["stat"].auth);
    }

    #[test]
    fn the_detail_keeps_technical_fields_folded_and_shows_trial_results() {
        let mut page = ApisPage {
            loaded: true,
            store: ApiStore {
                endpoints: vec![endpoint()],
            },
            ..ApisPage::default()
        };
        page.open(0);
        page.detail.result = Some(Trial {
            request: Some("POST http://x".into()),
            raw: Some(RawResponse {
                status: 200,
                body: "{\"items\": [{\"title\": \"年度统计\", \"content\": \"共12起\"}]}".into(),
            }),
            items: vec![MappedItem {
                id: "1".into(),
                title: "年度统计".into(),
                text: "共12起".into(),
                source: "统计系统".into(),
            }],
            total: 1,
            error: None,
        });
        let texts = render(|ui| detail_ui(ui, &mut page, 0));
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(
            has("查询条件") && has("试一下") && has("高级配置"),
            "{texts:?}"
        );
        assert!(!has("请求体模板"), "技术配置默认折叠：{texts:?}");
        assert!(
            has("调通了：取到 1 条") && has("【年度统计】共12起"),
            "{texts:?}"
        );
        assert!(has("按这次的返回补全：列表位置"), "{texts:?}");

        let mut secrets = ApiSecrets::default();
        secrets.secrets.insert("token".into(), "s3cr3t".into());
        let mut store = page.store.clone();
        let texts = render(|ui| {
            secrets_ui(ui, &mut secrets, &mut store, &ApiTestLog::default());
        });
        assert!(
            !texts.iter().any(|t| t.contains("s3cr3t")),
            "密钥打码显示：{texts:?}"
        );
    }
}
