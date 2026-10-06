//! 接口详情（右栏）。
//!
//! 顶上固定：名称与状态、一句话用途、完整地址、密钥绑定条——这个接口用哪把 Key、怎么带、
//! 和哪些接口共用、最近验证过没有；没填或被拒时就地粘贴，离开输入框自动重测，共用这把 Key
//! 的接口一起重测。下面分三页：
//! - 「接口说明」读起来像一份接口文档：用途、查询条件、返回什么、技能里怎么引用；点
//!   「编辑说明」就地改，不另开一套表单；
//! - 「试一下」左边填请求、右边看响应：状态、耗时、整理后的条目、原始返回、实际请求。
//!   一个接口有几种用法时存成几组样例（识别时文档的请求示例、模型编的典型例子），点一下填入，
//!   也可以「全部试一遍」；没有样例的可以让 AI 按接口说明编几组；
//! - 「技术配置」按「请求 → 返回」分组，删除放在最下面。

use super::forms::{self, cell, flex_width, table_head, table_row};
use super::{
    ApisPage, AuthForm, Health, RAW_PREVIEW_CHARS, View, auth_form_ui, host_of, key_field,
    method_tag, secret_for, spawn_trial,
};
use crate::agent::api::{
    self, ApiEndpoint, ApiExample, ApiSecrets, InputKind, MappedItem, TestStatus, Trial,
};
use crate::agent::api_import::{auth, examples, infer};
use crate::agent::backend::LmBackend;
use crate::models::AppConfig;
use crate::theme;
use eframe::egui;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// 「试一下」的响应区最多列几条。
const MAX_SHOWN_ITEMS: usize = 20;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Doc,
    Try,
    Config,
}

/// 响应区看哪一页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    Items,
    Raw,
    Request,
}

/// 详情页的临时状态，换接口时清空。
#[derive(Default)]
pub(super) struct Detail {
    pub(super) tab: Tab,
    /// 正在编辑说明：开始编辑时的原样，「放弃修改」用。
    pub(super) editing: Option<ApiEndpoint>,
    confirm_remove: bool,
    /// 「试一下」里填的值：参数名 → 值。
    pub(super) args: BTreeMap<String, String>,
    pub(super) test: Option<Receiver<Trial>>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
    pub(super) result: Option<Trial>,
    /// 响应区看哪一页；空着按结果选（调通看条目，失败看原始返回）。
    output: Option<Output>,
    /// 密钥条展开了输入框（「更换 Key」）。
    key_open: bool,
    /// 改过 Key、还没重测。
    key_edited: bool,
    /// 展开了「添加鉴权」。
    auth_open: bool,
    auth_form: AuthForm,
    /// 加鉴权时只加这一个接口（默认同一服务器上没带鉴权的一起加：一个服务通常共用一个 Key）。
    auth_this_only: bool,
    /// 「试一下」里选中的样例；None 表示手填。
    pub(super) example: Option<usize>,
    /// 「全部样例试一遍」：后台逐个发，(样例序号, 结果)。
    runs: Option<Receiver<(usize, Trial)>>,
    /// 每组样例最近一次的结果（按样例序号）。
    run_results: Vec<Option<Trial>>,
    /// AI 正在编样例。
    generating: Option<Receiver<Generated>>,
    /// 样例相关的提示：(是否顺利, 文字)。
    example_note: Option<(bool, String)>,
}

/// 打开接口时「试一下」先填好：有样例的填第一组，没有的填参数样例。
pub(super) fn preselect(detail: &mut Detail, endpoint: &ApiEndpoint) {
    match endpoint.examples.first() {
        Some(example) => {
            detail.example = Some(0);
            detail.args = form_args(endpoint, example);
        }
        None => {
            detail.args = endpoint
                .inputs
                .iter()
                .map(|input| (input.name.clone(), pretty_json(endpoint, &input.example)))
                .collect();
        }
    }
}

/// AI 编样例的结果：收下的样例与没收的说明，或出错原因。
type Generated = Result<(Vec<ApiExample>, Vec<String>), String>;

/// 样例的输入转成「试一下」表单里的文字。
fn form_args(endpoint: &ApiEndpoint, example: &ApiExample) -> BTreeMap<String, String> {
    endpoint
        .args_of(example)
        .into_iter()
        .map(|(name, value)| {
            let text = match value {
                Value::String(text) => text,
                other => other.to_string(),
            };
            (name, pretty_json(endpoint, &text))
        })
        .collect()
}

/// JSON 参数排版成多行，方便看和改。
fn pretty_json(endpoint: &ApiEndpoint, text: &str) -> String {
    text.trim_start()
        .starts_with(['{', '['])
        .then(|| serde_json::from_str::<Value>(text).ok())
        .flatten()
        .filter(|_| endpoint.inputs.iter().any(|i| i.kind == InputKind::Json))
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| text.to_string())
}

impl Detail {
    pub(super) fn start_editing(&mut self, endpoint: &ApiEndpoint) {
        self.editing = Some(endpoint.clone());
        self.tab = Tab::Doc;
    }
}

/// 放弃编辑说明：名称、用途、查询条件回到开始编辑时的样子。
pub(super) fn discard_edit(page: &mut ApisPage, index: usize) {
    if let Some(original) = page.detail.editing.take() {
        let endpoint = &mut page.store.endpoints[index];
        endpoint.name = original.name;
        endpoint.description = original.description;
        endpoint.inputs = original.inputs;
    }
}

pub(super) fn endpoint_ui(
    ui: &mut egui::Ui,
    page: &mut ApisPage,
    index: usize,
    config: &AppConfig,
) {
    poll_test(ui.ctx(), page, index);
    poll_examples(ui.ctx(), page, index);
    header_ui(ui, page, index);
    if page.view != View::Endpoint(index) {
        // 刚删掉了。
        return;
    }
    ui.add_space(10.0);
    binding_ui(ui, page, index);
    ui.add_space(10.0);
    let problems = page.store.endpoints[index].problems().len();
    tab_bar(ui, &mut page.detail.tab, problems);
    ui.add_space(12.0);
    let tab = page.detail.tab;
    egui::ScrollArea::vertical()
        .id_salt(("api_detail_scroll", index, tab as u8))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            match tab {
                Tab::Doc if page.detail.editing.is_some() => edit_ui(ui, page, index),
                Tab::Doc => doc_ui(ui, page, index),
                Tab::Try => try_ui(ui, page, index, config),
                Tab::Config => config_ui(ui, page, index),
            }
            ui.add_space(16.0);
        });
}

fn poll_test(ctx: &egui::Context, page: &mut ApisPage, index: usize) {
    let Some(rx) = &page.detail.test else {
        return;
    };
    match rx.try_recv() {
        Ok(trial) => {
            let endpoint = page.store.endpoints[index].clone();
            page.record(&endpoint, &trial);
            page.detail.elapsed = page.detail.started.map(|start| start.elapsed());
            page.detail.result = Some(trial);
            page.detail.output = None;
            page.detail.test = None;
        }
        Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
        Err(TryRecvError::Disconnected) => page.detail.test = None,
    }
}

/// 取回「全部样例试一遍」与「AI 生成样例」的结果。
fn poll_examples(ctx: &egui::Context, page: &mut ApisPage, index: usize) {
    let mut finished = false;
    if let Some(rx) = &page.detail.runs {
        loop {
            match rx.try_recv() {
                Ok((at, trial)) => {
                    let results = &mut page.detail.run_results;
                    if results.len() <= at {
                        results.resize(at + 1, None);
                    }
                    if page.detail.example == Some(at) {
                        page.detail.result = Some(trial.clone());
                        page.detail.output = None;
                    }
                    results[at] = Some(trial);
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
    }
    if finished {
        page.detail.runs = None;
        let results: Vec<Trial> = page.detail.run_results.iter().flatten().cloned().collect();
        let passed = results.iter().filter(|t| t.ok()).count();
        // 测试记录：有没通的记没通的那一次，全通记最后一次。
        if let Some(trial) = results.iter().find(|t| !t.ok()).or(results.last()).cloned() {
            let endpoint = page.store.endpoints[index].clone();
            page.record(&endpoint, &trial);
        }
        page.detail.example_note = Some((
            passed == results.len(),
            format!("{} 组样例，调通 {passed} 组。", results.len()),
        ));
    }
    if let Some(rx) = &page.detail.generating {
        match rx.try_recv() {
            Ok(Ok((made, notes))) => {
                page.detail.generating = None;
                let endpoint = &mut page.store.endpoints[index];
                let before = endpoint.examples.clone();
                endpoint.examples = examples::combine(made, &before);
                let added = endpoint
                    .examples
                    .iter()
                    .filter(|e| !before.iter().any(|b| b.args == e.args))
                    .count();
                let mut text = if added > 0 {
                    page.dirty = true;
                    page.detail.run_results.clear();
                    page.detail.example = None;
                    format!("AI 编了 {added} 组样例，排在最前面；记得保存。")
                } else {
                    "AI 没编出新的样例（和已有的重复或不合格式）。".to_string()
                };
                if !notes.is_empty() {
                    text.push_str(&format!("（{}）", notes.join("；")));
                }
                page.detail.example_note = Some((true, text));
            }
            Ok(Err(error)) => {
                page.detail.generating = None;
                page.detail.example_note = Some((false, error));
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
            Err(TryRecvError::Disconnected) => page.detail.generating = None,
        }
    }
}

/// 后台把每组样例试一遍。
fn start_runs(detail: &mut Detail, endpoint: &ApiEndpoint, secrets: &ApiSecrets) {
    let (tx, rx) = std::sync::mpsc::channel();
    let endpoint = endpoint.clone();
    let secrets = secrets.clone();
    detail.run_results = vec![None; endpoint.examples.len()];
    detail.example_note = None;
    std::thread::spawn(move || {
        for (at, example) in endpoint.examples.iter().enumerate() {
            let trial = api::trial(&endpoint, &endpoint.args_of(example), &secrets);
            if tx.send((at, trial)).is_err() {
                return;
            }
        }
    });
    detail.runs = Some(rx);
}

#[cfg(test)]
pub(super) fn start_runs_for_test(page: &mut ApisPage, endpoint: &ApiEndpoint) {
    start_runs(&mut page.detail, endpoint, &page.secrets);
}

#[cfg(test)]
impl ApisPage {
    pub(super) fn detail_runs_done(&self) -> bool {
        self.detail.runs.is_none() && self.detail.example_note.is_some()
    }
}

/// 后台让起草模型按接口说明编样例。
fn start_generating(detail: &mut Detail, endpoint: &ApiEndpoint, config: &AppConfig) {
    let (tx, rx) = std::sync::mpsc::channel();
    let endpoint = endpoint.clone();
    let config = config.clone();
    detail.example_note = None;
    std::thread::spawn(move || {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = LmBackend::new(&config, cancel);
        let _ = tx.send(examples::generate(&backend, &endpoint));
    });
    detail.generating = Some(rx);
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
    detail.output = None;
    detail.started = Some(Instant::now());
    detail.elapsed = None;
}

// —— 顶部 ——

fn header_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let mut go_try = false;
    let mut remove = false;
    let mut copy = None;
    {
        let endpoint = &page.store.endpoints[index];
        let health = Health::of(page, endpoint);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&endpoint.name)
                    .size(theme::font_sizes::HEADING)
                    .strong(),
            );
            health.chip(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let more = ui
                    .add(
                        egui::Button::image(theme::Icon::Menu.image_sized(16.0))
                            .image_tint_follows_text_color(true)
                            .frame_when_inactive(false),
                    )
                    .on_hover_text("更多");
                egui::Popup::menu(&more).show(|ui| {
                    if ui
                        .add(theme::menu_item(theme::Icon::Copy, "复制技能引用"))
                        .clicked()
                    {
                        copy = Some(format!("http.call:{}", endpoint.id));
                    }
                    if ui
                        .add(theme::menu_item(theme::Icon::Copy, "复制地址"))
                        .clicked()
                    {
                        copy = Some(endpoint.url.clone());
                    }
                    if ui
                        .add(theme::menu_item(theme::Icon::Trash, "删除接口…"))
                        .clicked()
                    {
                        remove = true;
                    }
                });
                if ui
                    .add(theme::secondary_icon_button(theme::Icon::PlugZap, "试一下"))
                    .clicked()
                {
                    go_try = true;
                }
            });
        });
        let description = endpoint.description.trim();
        if description.is_empty() {
            ui.colored_label(
                theme::warn(),
                "还没写用途：AI 靠它判断什么时候调用这个接口。",
            );
        } else {
            ui.add(
                egui::Label::new(egui::RichText::new(description).color(theme::text_soft()))
                    .truncate(),
            );
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(theme::canvas())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(7))
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        method_tag(ui, endpoint.method);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&endpoint.url)
                                    .monospace()
                                    .size(13.0)
                                    .color(theme::text_soft()),
                            )
                            .truncate(),
                        );
                        if theme::icon_button(ui, theme::Icon::Copy, "复制地址").clicked() {
                            copy = Some(endpoint.url.clone());
                        }
                    });
                });
        });
    }
    if let Some(text) = copy {
        ui.ctx().copy_text(text);
    }
    if go_try {
        page.detail.tab = Tab::Try;
    }
    if remove {
        page.detail.confirm_remove = true;
    }
    if page.detail.confirm_remove {
        ui.add_space(8.0);
        let mut confirmed = false;
        strip(ui, Tone::Danger, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "删除「{}」？引用它的技能运行时会报「找不到接口」。",
                    page.store.endpoints[index].name
                ));
                if ui
                    .add(theme::warning_icon_button(theme::Icon::Trash, "确认删除"))
                    .clicked()
                {
                    confirmed = true;
                }
                if ui.button("取消").clicked() {
                    page.detail.confirm_remove = false;
                }
            });
        });
        if confirmed {
            page.store.endpoints.remove(index);
            page.dirty = true;
            match page.store.endpoints.len() {
                0 => {
                    page.detail = Detail::default();
                    page.view = View::None;
                }
                len => page.open(index.min(len - 1)),
            }
        }
    }
}

/// 绑定条的色调。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Ok,
    Warn,
    Danger,
    Muted,
}

/// 整行的提示条：淡底、细边、圆角。
pub(super) fn strip<R>(ui: &mut egui::Ui, tone: Tone, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (fill, stroke) = match tone {
        Tone::Ok => (theme::canvas(), theme::border()),
        Tone::Warn => (theme::warn_soft(), theme::warn().gamma_multiply(0.35)),
        Tone::Danger => (theme::danger_soft(), theme::danger().gamma_multiply(0.35)),
        Tone::Muted => (theme::surface(), theme::border()),
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// 绑定条左边的钥匙徽记。
pub(super) fn key_badge(ui: &mut egui::Ui, tone: Tone) {
    let (fg, bg) = match tone {
        Tone::Ok => (theme::success(), theme::success_soft()),
        Tone::Warn => (theme::warn(), theme::surface()),
        Tone::Danger => (theme::danger(), theme::surface()),
        Tone::Muted => (theme::text_muted(), theme::surface_sunk()),
    };
    egui::Frame::new()
        .fill(bg)
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(5))
        .show(ui, |ui| {
            ui.add(theme::Icon::Shield.image_sized(15.0).tint(fg));
        });
}

/// 密钥绑定条：这个接口用的每把 Key 一条。
fn binding_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let names = page.store.endpoints[index].secret_names();
    if names.is_empty() {
        no_auth_ui(ui, page, index);
        return;
    }
    let endpoint = &page.store.endpoints[index];
    let how = auth::from_endpoint(endpoint).map(|(spec, _)| spec.describe());
    let status = page.log.status(endpoint);
    let rejected = matches!(status, TestStatus::Failed(record) if record.auth)
        || page
            .detail
            .result
            .as_ref()
            .is_some_and(Trial::auth_rejected);
    let passed_at = match status {
        TestStatus::Passed(record) => Some(record.at.clone()),
        _ => None,
    };
    let host = short_host(&host_of(&endpoint.url));
    let mut retest = None;
    for name in &names {
        let users = page.users_of(name);
        let filled = page.secrets.filled(name);
        let tone = if !filled {
            Tone::Warn
        } else if rejected {
            Tone::Danger
        } else {
            Tone::Ok
        };
        let input_open = tone != Tone::Ok || page.detail.key_open;
        strip(ui, tone, |ui| {
            ui.horizontal_top(|ui| {
                key_badge(ui, tone);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(format!("密钥 {name}")).strong());
                        match tone {
                            Tone::Warn => {
                                ui.colored_label(theme::warn(), "还没填");
                            }
                            Tone::Danger => {
                                ui.colored_label(
                                    theme::danger(),
                                    "被接口拒绝了：Key 可能不对或已过期",
                                );
                            }
                            _ => {
                                theme::caption(
                                    ui,
                                    &format!("· 带在{}", how.as_deref().unwrap_or("请求里")),
                                );
                                if !page.detail.key_open && ui.small_button("更换 Key").clicked()
                                {
                                    page.detail.key_open = true;
                                }
                            }
                        }
                    });
                    let mut line = if users.len() > 1 {
                        format!(
                            "{host} 上的 {} 个接口共用这把 Key，换了一起生效",
                            users.len()
                        )
                    } else {
                        "只有这个接口在用这把 Key".to_string()
                    };
                    if tone == Tone::Ok {
                        if let Some(at) = &passed_at {
                            line.push_str(&format!(" · 上次验证通过 {at}"));
                        }
                    } else if let Some(how) = &how {
                        line = format!("带法：{how} · {line}");
                    }
                    theme::caption(ui, &line);
                    if input_open {
                        ui.horizontal(|ui| {
                            let value = page.secrets.secrets.entry(name.clone()).or_default();
                            let response = key_field(ui, value);
                            let filled_now = !value.trim().is_empty();
                            if response.changed() {
                                page.dirty = true;
                                page.detail.key_edited = true;
                            }
                            if response.lost_focus() && page.detail.key_edited && filled_now {
                                page.detail.key_edited = false;
                                retest = Some(name.clone());
                            }
                            theme::caption(ui, "粘贴后点别处，自动重测");
                            if page.detail.key_open && ui.small_button("收起").clicked() {
                                page.detail.key_open = false;
                            }
                        });
                        theme::caption(ui, "Key 只存在本机，不导出、不发给模型；记得保存。");
                    }
                });
            });
        });
        ui.add_space(4.0);
    }
    if let Some(name) = retest {
        page.detail.key_open = false;
        if page.detail.test.is_none() {
            let endpoint = page.store.endpoints[index].clone();
            start_test(&mut page.detail, &endpoint, &page.secrets);
        }
        let others: Vec<usize> = page
            .users_of(&name)
            .into_iter()
            .filter(|&i| i != index)
            .collect();
        page.retest(&others);
    }
}

/// 不带密钥的接口：一条灰条，要的话展开「添加鉴权」。
fn no_auth_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let host = host_of(&page.store.endpoints[index].url);
    let others = page
        .store
        .endpoints
        .iter()
        .enumerate()
        .filter(|(i, e)| *i != index && host_of(&e.url) == host && !auth::has_auth(e))
        .count();
    let mut added = None;
    strip(ui, Tone::Muted, |ui| {
        ui.horizontal(|ui| {
            key_badge(ui, Tone::Muted);
            ui.label("这个接口不带密钥");
            theme::caption(ui, "接口要 Key 的话，加上鉴权");
            if !page.detail.auth_open && ui.small_button("添加鉴权").clicked() {
                page.detail.auth_open = true;
            }
        });
        if page.detail.auth_open {
            ui.add_space(4.0);
            added = auth_form_ui(ui, &mut page.detail.auth_form);
            if others > 0 {
                let mut all = !page.detail.auth_this_only;
                ui.checkbox(
                    &mut all,
                    format!("同一服务器上不带密钥的其他 {others} 个接口也加上"),
                );
                page.detail.auth_this_only = !all;
            }
        }
    });
    if let Some(spec) = added {
        let secret = secret_for(&spec, &mut page.secrets);
        let all = !page.detail.auth_this_only;
        for (i, endpoint) in page.store.endpoints.iter_mut().enumerate() {
            if i == index || (all && host_of(&endpoint.url) == host && !auth::has_auth(endpoint)) {
                auth::apply(endpoint, &spec, &secret);
            }
        }
        page.dirty = true;
        page.detail.auth_open = false;
        page.detail.key_open = true;
    }
}

/// `http://10.0.0.8:8080` → `10.0.0.8:8080`。
pub(super) fn short_host(host: &str) -> String {
    host.split_once("://")
        .map_or(host, |(_, rest)| rest)
        .to_string()
}

/// 三页的页签：选中的一页文字着强调色、下面一道粗线。
fn tab_bar(ui: &mut egui::Ui, tab: &mut Tab, problems: usize) {
    let mut selected_rect = None;
    let row = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (value, icon, label) in [
                (Tab::Doc, theme::Icon::Book, "接口说明"),
                (Tab::Try, theme::Icon::PlugZap, "试一下"),
                (Tab::Config, theme::Icon::Settings, "技术配置"),
            ] {
                let selected = *tab == value;
                let color = if selected {
                    theme::accent()
                } else {
                    theme::text_muted()
                };
                let mut text = egui::RichText::new(label).color(color);
                if selected {
                    text = text.strong();
                }
                let response = ui.add(
                    egui::Button::image_and_text(icon.image_sized(15.0).tint(color), text)
                        .frame_when_inactive(false)
                        .min_size(egui::vec2(0.0, 32.0)),
                );
                if value == Tab::Config && problems > 0 {
                    super::small_chip(
                        ui,
                        None,
                        &problems.to_string(),
                        theme::danger(),
                        Some(theme::danger_soft()),
                    )
                    .on_hover_text(format!("配置有 {problems} 个问题"));
                }
                if selected {
                    selected_rect = Some(response.rect);
                }
                if response.clicked() {
                    *tab = value;
                }
                ui.add_space(10.0);
            }
        })
        .response
        .rect;
    let y = row.bottom() + 2.0;
    let painter = ui.painter();
    painter.hline(
        ui.max_rect().x_range(),
        y,
        egui::Stroke::new(1.0, theme::border()),
    );
    if let Some(rect) = selected_rect {
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() + 4.0, y - 1.5),
                egui::pos2(rect.right() - 4.0, y + 1.0),
            ),
            1.0,
            theme::accent(),
        );
    }
    ui.add_space(4.0);
}

// —— 接口说明 ——

/// 文档里的一节标题，右边可以挂按钮。
fn doc_heading(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(15.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(4.0);
}

fn doc_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let mut edit = false;
    let mut go = None;
    {
        let endpoint = &page.store.endpoints[index];
        doc_heading(ui, "用途", |ui| {
            if ui
                .add(theme::secondary_icon_button(theme::Icon::Edit, "编辑说明"))
                .clicked()
            {
                edit = true;
            }
        });
        let description = endpoint.description.trim();
        if description.is_empty() {
            theme::notice(
                ui,
                theme::Icon::TriangleAlert,
                theme::warn(),
                theme::warn_soft(),
                "还没写用途。AI 靠这段文字判断什么时候调用这个接口：写清能查什么、按什么查。点「编辑说明」补上。",
            );
        } else {
            ui.add(
                egui::Label::new(egui::RichText::new(description).color(theme::text_soft())).wrap(),
            );
            theme::caption(ui, "AI 靠这段文字判断什么时候调用这个接口。");
        }

        ui.add_space(18.0);
        doc_heading(ui, "查询条件", |_| {});
        params_table(ui, endpoint);

        if !endpoint.examples.is_empty() {
            ui.add_space(18.0);
            doc_heading(ui, "用法示例", |_| {});
            for (at, example) in endpoint.examples.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(&example.name).strong());
                    if !example.note.is_empty() {
                        theme::caption(ui, &example.note);
                    }
                    if ui.small_button("去试一下").clicked() {
                        go = Some(at);
                    }
                });
            }
            theme::caption(
                ui,
                "每组是一种典型用法；「试一下」里点名字填入，也可以全部试一遍。",
            );
        }

        ui.add_space(18.0);
        doc_heading(ui, "返回什么", |_| {});
        returns_ui(ui, page, endpoint);

        ui.add_space(18.0);
        doc_heading(ui, "在技能里引用", |_| {});
        let reference = format!("http.call:{}", endpoint.id);
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(theme::canvas())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&reference).monospace());
                });
            if theme::icon_button(ui, theme::Icon::Copy, "复制").clicked() {
                ui.ctx().copy_text(reference.clone());
            }
        });
        theme::caption(ui, "写进技能 SKILL.md 的 tools 里，技能就能调用这个接口。");
    }
    if edit {
        let snapshot = page.store.endpoints[index].clone();
        page.detail.start_editing(&snapshot);
    }
    if let Some(at) = go {
        let endpoint = page.store.endpoints[index].clone();
        page.detail.tab = Tab::Try;
        page.detail.example = Some(at);
        page.detail.args = form_args(&endpoint, &endpoint.examples[at]);
        page.detail.result = page.detail.run_results.get(at).cloned().flatten();
        page.detail.output = None;
    }
}

/// 查询条件的只读表：参数、类型、必填、说明、样例。说明列换行显示全文。
fn params_table(ui: &mut egui::Ui, endpoint: &ApiEndpoint) {
    if endpoint.inputs.is_empty() {
        theme::caption(ui, "不需要查询条件。");
        return;
    }
    const NAME: f32 = 130.0;
    const KIND: f32 = 48.0;
    const REQUIRED: f32 = 44.0;
    const EXAMPLE: f32 = 150.0;
    let describe = flex_width(ui, &[NAME, KIND, REQUIRED, EXAMPLE]);
    table_head(
        ui,
        &[
            ("参数", NAME),
            ("类型", KIND),
            ("必填", REQUIRED),
            ("说明", describe),
            ("样例", EXAMPLE),
        ],
    );
    for input in &endpoint.inputs {
        table_row(ui, |ui| {
            cell(ui, NAME, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&input.name).monospace()).wrap());
            });
            cell(ui, KIND, |ui| {
                ui.label(input.kind.label());
            });
            cell(ui, REQUIRED, |ui| {
                if input.required {
                    ui.colored_label(theme::danger(), "必填");
                } else {
                    theme::caption(ui, "可选");
                }
            });
            cell(ui, describe, |ui| {
                let text = input.description.trim();
                if text.is_empty() {
                    theme::caption(ui, "（没写说明）");
                } else {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).color(theme::text_soft()))
                            .wrap(),
                    );
                }
            });
            cell(ui, EXAMPLE, |ui| {
                let example = input.example.trim();
                if !example.is_empty() {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(example)
                                .monospace()
                                .size(theme::font_sizes::SMALL)
                                .color(theme::text_soft()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(example);
                }
            });
        });
    }
}

/// 「返回什么」：每条结果的各项从哪取，加上最近一次测试拿到的样子。
fn returns_ui(ui: &mut egui::Ui, page: &ApisPage, endpoint: &ApiEndpoint) {
    ui.label(format!(
        "每次取回一组条目，放进{}；每一条这样整理：",
        endpoint.destination.label()
    ));
    ui.add_space(4.0);
    let mapping = &endpoint.mapping;
    let field = |value: &str| -> egui::RichText {
        if value.trim().is_empty() {
            egui::RichText::new("没指定，按常见字段名猜").color(theme::text_muted())
        } else {
            egui::RichText::new(value.trim()).monospace()
        }
    };
    let success = if endpoint.success.is_set() {
        egui::RichText::new(format!(
            "{} 等于 {}",
            endpoint.success.pointer.trim(),
            endpoint.success.equals.trim()
        ))
        .monospace()
    } else {
        egui::RichText::new("HTTP 200 就算成功")
    };
    let list = if mapping.list.trim().is_empty() {
        egui::RichText::new("整个返回")
    } else {
        egui::RichText::new(mapping.list.trim()).monospace()
    };
    let rows = [
        ("条目位置", list),
        ("标题", field(&mapping.title)),
        ("正文", field(&mapping.text)),
        ("出处", field(&mapping.source)),
        ("编号", field(&mapping.id)),
        ("成功判据", success),
    ];
    egui::Frame::new()
        .fill(theme::canvas())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (label, value) in rows {
                ui.horizontal_top(|ui| {
                    cell(ui, 80.0, |ui| {
                        theme::caption(ui, label);
                    });
                    ui.add(egui::Label::new(value).wrap());
                });
            }
        });
    ui.add_space(6.0);
    match &page.detail.result {
        Some(trial) if trial.ok() && !trial.items.is_empty() => {
            theme::caption(ui, "这次试调取到的第一条：");
            item_card(ui, 1, &trial.items[0]);
        }
        _ => match page.log.records.get(&endpoint.id) {
            Some(record) => {
                theme::caption(ui, &format!("上次测试 {}：{}", record.at, record.summary));
            }
            None => {
                theme::caption(ui, "还没测过：到「试一下」里发一次请求看看。");
            }
        },
    }
}

fn item_card(ui: &mut egui::Ui, number: usize, item: &MappedItem) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                theme::caption(ui, &format!("#{number}"));
                ui.label(egui::RichText::new(&item.title).strong());
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::agent::tools::short(&item.text, 400))
                        .color(theme::text_soft()),
                )
                .wrap(),
            );
            theme::caption(ui, &format!("出处：{} · 编号：{}", item.source, item.id));
        });
    ui.add_space(6.0);
}

/// 就地编辑名称、用途与查询条件。
fn edit_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    theme::notice(
        ui,
        theme::Icon::Edit,
        theme::info(),
        theme::accent_soft(),
        "正在编辑说明。改完点「完成」回到阅读视图；改动跟其他设置一起保存。",
    );
    ui.add_space(10.0);
    page.dirty |= forms::info_form(ui, &mut page.store.endpoints[index], "detail");
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        if theme::primary_icon_button(ui, theme::Icon::Check, "完成").clicked() {
            page.detail.editing = None;
        }
        if ui
            .add(theme::secondary_icon_button(theme::Icon::Undo, "放弃修改"))
            .clicked()
        {
            discard_edit(page, index);
        }
    });
}

// —— 试一下 ——

/// 带标题栏的框：「请求」「响应」与技术配置的各组都用它。
pub(super) fn panel<R>(
    ui: &mut egui::Ui,
    icon: theme::Icon,
    title: &str,
    hint: Option<&str>,
    right: impl FnOnce(&mut egui::Ui),
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(egui::CornerRadius::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(icon.image_sized(15.0).tint(theme::text_muted()));
                        ui.label(egui::RichText::new(title).strong());
                        if let Some(hint) = hint {
                            theme::caption(ui, hint);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
                    });
                });
            theme::hairline(ui);
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui)
                })
                .inner
        })
        .inner
}

fn try_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize, config: &AppConfig) {
    let width = ui.available_width();
    if width >= 720.0 {
        let gap = 14.0;
        let left = ((width - gap) * 0.42).floor();
        let right = width - gap - left - ui.spacing().item_spacing.x;
        ui.horizontal_top(|ui| {
            cell(ui, left, |ui| request_box(ui, page, index, config));
            ui.add_space(gap - ui.spacing().item_spacing.x);
            cell(ui, right, |ui| response_box(ui, page, index));
        });
    } else {
        request_box(ui, page, index, config);
        ui.add_space(12.0);
        response_box(ui, page, index);
    }
}

/// 样例一行：每组一个可点的标签（试过的标上通没通），选中的显示说明与改名、删除。
/// 返回点了哪一组。
fn examples_bar(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) -> Option<usize> {
    let mut picked = None;
    let mut remove = None;
    let endpoint = &page.store.endpoints[index];
    if endpoint.examples.is_empty() {
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        theme::caption(ui, "样例");
        for (at, example) in endpoint.examples.iter().enumerate() {
            let mark = match page.detail.run_results.get(at).and_then(Option::as_ref) {
                Some(trial) if trial.ok() => "✓ ",
                Some(_) => "✗ ",
                None => "",
            };
            let selected = page.detail.example == Some(at);
            let mut response = ui.selectable_label(selected, format!("{mark}{}", example.name));
            if !example.note.is_empty() {
                response = response.on_hover_text(&example.note);
            }
            if response.clicked() {
                picked = Some(at);
            }
        }
    });
    if let Some(at) = page.detail.example
        && at < page.store.endpoints[index].examples.len()
    {
        let example = &mut page.store.endpoints[index].examples[at];
        ui.horizontal_wrapped(|ui| {
            ui.label("名称");
            page.dirty |= ui
                .add(egui::TextEdit::singleline(&mut example.name).desired_width(160.0))
                .changed();
            if ui.small_button("删除这组").clicked() {
                remove = Some(at);
            }
        });
        if !example.note.is_empty() {
            theme::caption(ui, &example.note);
        }
    }
    if let Some(at) = remove {
        page.store.endpoints[index].examples.remove(at);
        if at < page.detail.run_results.len() {
            page.detail.run_results.remove(at);
        }
        page.detail.example = None;
        page.dirty = true;
    }
    ui.add_space(6.0);
    picked
}

/// 左边：按查询条件逐项填写，点「发送请求」。有样例的先选一组。
fn request_box(ui: &mut egui::Ui, page: &mut ApisPage, index: usize, config: &AppConfig) {
    let mut fill = false;
    let mut send = false;
    let mut run_all = false;
    let mut save = false;
    let mut generate = false;
    let mut picked = None;
    let running = page.detail.test.is_some();
    let has_model = config.draft_chat().is_ok();
    let generating = page.detail.generating.is_some();
    let running_all = page.detail.runs.is_some();
    let examples_count = page.store.endpoints[index].examples.len();
    let has_inputs = !page.store.endpoints[index].inputs.is_empty();
    panel(
        ui,
        theme::Icon::ArrowUp,
        "请求",
        None,
        |ui| {
            if has_inputs
                && ui
                    .small_button("填入参数样例")
                    .on_hover_text("每个条件填上它自己的样例值")
                    .clicked()
            {
                fill = true;
            }
        },
        |ui| {
            picked = examples_bar(ui, page, index);
            let endpoint = page.store.endpoints[index].clone();
            if endpoint.inputs.is_empty() {
                theme::caption(ui, "这个接口不需要查询条件。");
            }
            for input in &endpoint.inputs {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let title = input.description.trim();
                    ui.label(if title.is_empty() { &input.name } else { title });
                    if !title.is_empty() {
                        ui.label(
                            egui::RichText::new(&input.name)
                                .monospace()
                                .size(theme::font_sizes::SMALL)
                                .color(theme::text_muted()),
                        );
                    }
                    if input.required {
                        ui.colored_label(theme::danger(), "*");
                    }
                });
                let value = page.detail.args.entry(input.name.clone()).or_default();
                if input.kind == InputKind::Json {
                    ui.add(
                        egui::TextEdit::multiline(value)
                            .code_editor()
                            .desired_rows(5)
                            .hint_text(input.example.as_str())
                            .desired_width(f32::INFINITY),
                    );
                    if !value.trim().is_empty()
                        && serde_json::from_str::<Value>(value.trim()).is_err()
                    {
                        ui.colored_label(theme::warn(), "不是合法的 JSON");
                    }
                } else {
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .hint_text(input.example.as_str())
                            .desired_width(f32::INFINITY),
                    );
                }
                ui.add_space(6.0);
            }
            let names = endpoint.secret_names();
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    theme::Icon::Shield
                        .image_sized(13.0)
                        .tint(theme::text_muted()),
                );
                if names.is_empty() {
                    theme::caption(ui, "不带密钥");
                } else {
                    let missing = page.secrets.missing(&endpoint);
                    if missing.is_empty() {
                        theme::caption(ui, &format!("自动带上密钥 {}（已填）", names.join("、")));
                    } else {
                        ui.colored_label(
                            theme::warn(),
                            format!("密钥 {} 还没填，请求会被拒绝", missing.join("、")),
                        );
                    }
                }
            });
            let problems = endpoint.problems().len();
            if problems > 0 {
                ui.colored_label(
                    theme::warn(),
                    format!("配置有 {problems} 个问题，见「技术配置」"),
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::primary_icon_button_enabled(
                    ui,
                    !running,
                    theme::Icon::PlugZap,
                    "发送请求",
                )
                .clicked()
                {
                    send = true;
                }
                if running {
                    theme::spinner(ui, 14.0, theme::accent());
                    ui.weak("请求中…");
                } else {
                    theme::caption(ui, "用正在编辑的配置，不必先保存");
                }
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if examples_count > 1
                    && ui
                        .add_enabled(
                            !running_all,
                            theme::secondary_icon_button(
                                theme::Icon::ListOrdered,
                                &format!("全部 {examples_count} 组试一遍"),
                            ),
                        )
                        .clicked()
                {
                    run_all = true;
                }
                if ui
                    .add(theme::secondary_icon_button(theme::Icon::Save, "存为样例"))
                    .on_hover_text("把现在填的值存成一组样例")
                    .clicked()
                {
                    save = true;
                }
                if has_model
                    && ui
                        .add_enabled(
                            !generating,
                            theme::secondary_icon_button(theme::Icon::Sparkles, "AI 编几组样例"),
                        )
                        .on_hover_text("按接口说明编几组典型用法，每种分支至少一组，内容取材于公文")
                        .clicked()
                {
                    generate = true;
                }
                if generating || running_all {
                    theme::spinner(ui, 14.0, theme::accent());
                    ui.weak(if generating {
                        "AI 在编样例…"
                    } else {
                        "逐组试调中…"
                    });
                }
            });
            if let Some((ok, text)) = &page.detail.example_note {
                let color = if *ok {
                    theme::text_muted()
                } else {
                    theme::warn()
                };
                ui.colored_label(color, text);
            }
        },
    );
    let endpoint = page.store.endpoints[index].clone();
    if let Some(at) = picked {
        page.detail.example = Some(at);
        page.detail.args = form_args(&endpoint, &endpoint.examples[at]);
        page.detail.result = page.detail.run_results.get(at).cloned().flatten();
        page.detail.output = None;
    }
    if run_all {
        start_runs(&mut page.detail, &endpoint, &page.secrets);
    }
    if save {
        let args: BTreeMap<String, String> = page
            .detail
            .args
            .iter()
            .filter(|(name, value)| {
                !value.trim().is_empty() && endpoint.inputs.iter().any(|i| i.name == **name)
            })
            .map(|(name, value)| {
                let compact = serde_json::from_str::<Value>(value.trim())
                    .ok()
                    .filter(|v| v.is_object() || v.is_array())
                    .map_or(value.trim().to_string(), |v| v.to_string());
                (name.clone(), compact)
            })
            .collect();
        let examples = &mut page.store.endpoints[index].examples;
        if examples.iter().any(|e| e.args == args) {
            page.detail.example_note = Some((false, "已经有一样的样例了。".into()));
        } else {
            examples.push(ApiExample {
                name: format!("样例 {}", examples.len() + 1),
                note: "手动保存".into(),
                args,
            });
            page.detail.example = Some(examples.len() - 1);
            page.dirty = true;
            page.detail.example_note = Some((true, "已存为样例，可以改个名字；记得保存。".into()));
        }
    }
    if generate {
        start_generating(&mut page.detail, &endpoint, config);
    }
    if fill {
        page.detail.example = None;
        page.detail.args = endpoint
            .inputs
            .iter()
            .map(|input| (input.name.clone(), pretty_json(&endpoint, &input.example)))
            .collect();
    }
    if send {
        start_test(&mut page.detail, &endpoint, &page.secrets);
    }
}

/// 右边：结论一行、出错原因、整理后的条目 / 原始返回 / 实际请求。
fn response_box(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let running = page.detail.test.is_some() || page.retesting(&page.store.endpoints[index]);
    let mut output = page.detail.output;
    let mut apply = None;
    panel(
        ui,
        theme::Icon::ArrowDown,
        "响应",
        None,
        |_| {},
        |ui| {
            let Some(trial) = &page.detail.result else {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    if running {
                        theme::spinner(ui, 20.0, theme::accent());
                        ui.weak("请求中…");
                    } else {
                        ui.add(
                            theme::Icon::PlugZap
                                .image_sized(30.0)
                                .tint(theme::border_strong()),
                        );
                        theme::caption(ui, "填好左边的条件，点「发送请求」，结果显示在这里。");
                    }
                });
                ui.add_space(24.0);
                return;
            };
            status_line(ui, trial, page.detail.elapsed, running);
            if let Some(error) = &trial.error {
                ui.add_space(4.0);
                theme::notice(
                    ui,
                    theme::Icon::TriangleAlert,
                    theme::danger(),
                    theme::danger_soft(),
                    error.clone(),
                );
                if trial.auth_rejected() {
                    ui.add_space(4.0);
                    theme::notice(
                        ui,
                        theme::Icon::Shield,
                        theme::warn(),
                        theme::warn_soft(),
                        if auth::has_auth(&page.store.endpoints[index]) {
                            "看起来是鉴权没过：Key 不对、过期，或者接口要的带法和这里配的不一样。在上方密钥条里粘贴新的 Key，离开输入框会自动重测。"
                        } else {
                            "看起来接口要鉴权，但这里没配密钥：在上方点「添加鉴权」选好带法，再粘贴 Key。"
                        },
                    );
                }
            }
            ui.add_space(8.0);
            let current = output.unwrap_or(if trial.ok() {
                Output::Items
            } else {
                Output::Raw
            });
            let mut chosen = current;
            theme::segmented(ui, |ui| {
                ui.selectable_value(
                    &mut chosen,
                    Output::Items,
                    format!("整理后的条目 {}", trial.items.len()),
                );
                ui.selectable_value(&mut chosen, Output::Raw, "原始返回");
                ui.selectable_value(&mut chosen, Output::Request, "实际请求");
            });
            if chosen != current {
                output = Some(chosen);
            }
            ui.add_space(8.0);
            match chosen {
                Output::Items => {
                    if trial.items.is_empty() {
                        theme::caption(
                            ui,
                            if trial.ok() {
                                "返回里没有条目。"
                            } else {
                                "没整理出条目，看看原始返回。"
                            },
                        );
                    }
                    for (number, item) in trial.items.iter().take(MAX_SHOWN_ITEMS).enumerate() {
                        item_card(ui, number + 1, item);
                    }
                    if trial.items.len() > MAX_SHOWN_ITEMS {
                        theme::caption(
                            ui,
                            &format!(
                                "……只显示前 {MAX_SHOWN_ITEMS} 条，共 {} 条",
                                trial.items.len()
                            ),
                        );
                    }
                }
                Output::Raw => match &trial.raw {
                    Some(raw) => code_view(ui, ("api_raw", index), &pretty(&raw.body)),
                    None => {
                        theme::caption(ui, "请求没发出去，没有返回。");
                    }
                },
                Output::Request => match &trial.request {
                    Some(request) => {
                        theme::caption(ui, "密钥已打码");
                        code_view(ui, ("api_request", index), request);
                    }
                    None => {
                        theme::caption(ui, "请求没组出来，看上面的出错原因。");
                    }
                },
            }
            apply = mapping_suggestion(ui, trial, &page.store.endpoints[index]);
        },
    );
    page.detail.output = output;
    if let Some(preview) = apply {
        page.store.endpoints[index] = preview;
        page.dirty = true;
        if let Some(trial) = &mut page.detail.result {
            trial.remap(&page.store.endpoints[index]);
        }
    }
}

/// 结论一行：HTTP 状态、耗时、取到几条。
fn status_line(ui: &mut egui::Ui, trial: &Trial, elapsed: Option<Duration>, running: bool) {
    let color = if trial.ok() {
        theme::success()
    } else {
        theme::danger()
    };
    ui.horizontal_wrapped(|ui| {
        let icon = if trial.ok() {
            theme::Icon::Check
        } else {
            theme::Icon::TriangleAlert
        };
        ui.add(icon.image_sized(16.0).tint(color));
        match &trial.raw {
            Some(raw) => {
                ui.label(
                    egui::RichText::new(format!("HTTP {}", raw.status))
                        .monospace()
                        .strong()
                        .color(color),
                );
            }
            None => {
                ui.colored_label(theme::danger(), "没发出去");
            }
        }
        if let Some(elapsed) = elapsed {
            theme::caption(ui, "·");
            theme::caption(ui, &format!("{} ms", elapsed.as_millis()));
        }
        if trial.ok() {
            theme::caption(ui, "·");
            ui.label(trial.summary());
        }
        if running {
            theme::spinner(ui, 14.0, theme::accent());
            theme::caption(ui, "重测中…");
        }
    });
}

/// 拿到了真实返回、而返回映射还有空着的：给个一键补全。返回补全后的接口。
fn mapping_suggestion(
    ui: &mut egui::Ui,
    trial: &Trial,
    endpoint: &ApiEndpoint,
) -> Option<ApiEndpoint> {
    let raw = trial.raw.as_ref()?;
    let root = serde_json::from_str::<Value>(&raw.body).ok()?;
    let inferred = infer::infer(&root);
    let mut preview = endpoint.clone();
    let filled = infer::fill_mapping(&mut preview.mapping, &mut preview.success, &inferred);
    if filled.is_empty() {
        return None;
    }
    let mut clicked = false;
    ui.add_space(4.0);
    strip(ui, Tone::Ok, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                theme::Icon::WandSparkles
                    .image_sized(15.0)
                    .tint(theme::accent()),
            );
            ui.label(format!("返回映射里「{}」还空着", filled.join("、")));
            if ui
                .add(theme::secondary_icon_button(
                    theme::Icon::WandSparkles,
                    "按这次的返回补全",
                ))
                .clicked()
            {
                clicked = true;
            }
        });
    });
    clicked.then_some(preview)
}

/// JSON 排好缩进，别的原样。
fn pretty(body: &str) -> String {
    let text = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| body.to_string());
    let total = text.chars().count();
    if total > RAW_PREVIEW_CHARS {
        let head: String = text.chars().take(RAW_PREVIEW_CHARS).collect();
        format!("{head}\n……（共 {total} 字，只显示前 {RAW_PREVIEW_CHARS} 字）")
    } else {
        text
    }
}

/// 只读的等宽文字框：能选中复制，右上角一个复制按钮。
fn code_view(ui: &mut egui::Ui, id: impl egui::AsIdSalt, text: &str) {
    egui::Frame::new()
        .fill(theme::canvas())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if theme::icon_button(ui, theme::Icon::Copy, "复制").clicked() {
                        ui.ctx().copy_text(text.to_string());
                    }
                });
            });
            ui.push_id(id, |ui| {
                let mut shown = text;
                ui.add(
                    egui::TextEdit::multiline(&mut shown)
                        .code_editor()
                        .frame(egui::Frame::new())
                        .desired_width(f32::INFINITY),
                );
            });
        });
}

// —— 技术配置 ——

fn config_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let problems = page.store.endpoints[index].problems();
    if problems.is_empty() {
        theme::caption(
            ui,
            "从文档识别时程序已经填好，一般不用动。改了影响请求或返回的项，要重新测一次。",
        );
    } else {
        let mut text = format!("配置有 {} 个问题：", problems.len());
        for problem in &problems {
            text.push_str(&format!("\n· {problem}"));
        }
        theme::notice(
            ui,
            theme::Icon::TriangleAlert,
            theme::warn(),
            theme::warn_soft(),
            text,
        );
    }
    ui.add_space(10.0);
    panel(
        ui,
        theme::Icon::ArrowUp,
        "请求",
        None,
        |_| {},
        |ui| {
            page.dirty |= forms::request_form(ui, &mut page.store.endpoints[index], "detail");
        },
    );
    ui.add_space(12.0);
    panel(
        ui,
        theme::Icon::ArrowDown,
        "返回",
        Some("写英文字段名、/JSON 指针、{模板} 或固定文字；留空按常见字段名猜"),
        |_| {},
        |ui| {
            page.dirty |= forms::response_form(ui, &mut page.store.endpoints[index]);
        },
    );
    ui.add_space(12.0);
    let shared: Vec<String> = page.store.endpoints[index]
        .secret_names()
        .into_iter()
        .filter(|name| page.users_of(name).len() > 1)
        .collect();
    panel(
        ui,
        theme::Icon::Trash,
        "删除接口",
        None,
        |_| {},
        |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut text = "删除后，引用它的技能运行时会报「找不到接口」。".to_string();
                if !shared.is_empty() {
                    text.push_str(&format!(
                        "密钥 {} 还有其他接口在用，不会删。",
                        shared.join("、")
                    ));
                }
                theme::caption(ui, &text);
                if ui
                    .add(theme::warning_icon_button(
                        theme::Icon::Trash,
                        "删除这个接口",
                    ))
                    .clicked()
                {
                    page.detail.confirm_remove = true;
                }
            });
        },
    );
}
