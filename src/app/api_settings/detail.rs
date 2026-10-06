//! 接口详情（右栏）。
//!
//! 顶上固定：名称与状态、一句话用途、完整地址、密钥绑定条——这个接口用哪把 Key、怎么带、
//! 和哪些接口共用、最近验证过没有；没填或被拒时就地粘贴，离开输入框自动重测，共用这把 Key
//! 的接口一起重测。下面分三页：
//! - 「接口说明」读起来像一份接口文档：用途、查询条件、返回什么、技能里怎么引用；点
//!   「编辑说明」就地改，不另开一套表单；
//! - 「试一下」左边填请求、右边看响应：状态、耗时、整理后的条目、原始返回、实际请求。
//!   一个接口的几种用法存成几条用例（识别时文档的请求示例、模型编的典型例子、手动存的），
//!   每条可以写期望（`apidef::cases`），点一下填入；「全部跑一遍」逐条判通过与否并留记录，
//!   这是接口可用的证明。会改数据的接口「发送请求」要先确认；
//! - 「技术配置」按「请求 → 返回」分组，删除放在最下面。
//!
//! 本文件放状态、后台任务与分页调度；各部分的画法在子模块：[`header`]、[`doc`]、[`try_tab`]、
//! [`config`]。

mod config;
mod doc;
mod header;
mod try_tab;
use config::config_ui;
use doc::{doc_ui, edit_ui, item_card};
use header::{Tone, binding_ui, header_ui, strip};
use try_tab::try_ui;

use super::forms::{self, cell, flex_width, table_head, table_row};
use super::{
    ApisPage, AuthForm, Health, RAW_PREVIEW_CHARS, View, auth_form_ui, host_of, key_field,
    method_tag, secret_for, spawn_trial,
};
use crate::agent::api::{
    self, ApiEndpoint, ApiExample, ApiSecrets, InputKind, MappedItem, TestStatus, Trial,
};
use crate::agent::api_import::{auth, examples, infer};
use crate::agent::apidef::cases::{self, ExpectKind};
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

/// 一条用例最近一次跑的结果：实测与判定（判定先看调通，再看期望）。
pub(super) type CaseRun = (Trial, Result<(), String>);

/// 「加期望」的小表单。
pub(super) struct ExpectForm {
    pub(super) kind: ExpectKind,
    pub(super) pointer: String,
    pub(super) value: String,
    pub(super) error: Option<String>,
}

impl Default for ExpectForm {
    fn default() -> Self {
        Self {
            kind: ExpectKind::NotEmpty,
            pointer: String::new(),
            value: String::new(),
            error: None,
        }
    }
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
    /// 「试一下」里选中的用例；None 表示手填。
    pub(super) example: Option<usize>,
    /// 「全部跑一遍」：后台逐条跑，(用例序号, 结果)。
    runs: Option<Receiver<(usize, CaseRun)>>,
    /// 每条用例最近一次的结果（按用例序号）。
    run_results: Vec<Option<CaseRun>>,
    /// 这一次「发送请求」按选中用例的期望判的结果；没选用例时为空。
    verdict: Option<Result<(), String>>,
    /// 「加期望」的小表单。
    expect_form: ExpectForm,
    /// 会改数据的接口点了「发送请求」：等人确认。内容是要发出的请求（密钥打码）或组不出请求的原因。
    confirm_send: Option<Result<String, String>>,
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

/// 选中一条用例：填入它的参数，显示它最近一次的结果。
pub(super) fn pick_example(page: &mut ApisPage, index: usize, at: usize) {
    let endpoint = &page.store.endpoints[index];
    let Some(example) = endpoint.examples.get(at) else {
        return;
    };
    page.detail.args = form_args(endpoint, example);
    page.detail.example = Some(at);
    let run = page.detail.run_results.get(at).cloned().flatten();
    page.detail.result = run.as_ref().map(|(trial, _)| trial.clone());
    page.detail.verdict = run.map(|(_, verdict)| verdict);
    page.detail.output = None;
    page.detail.expect_form = ExpectForm::default();
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
            // 选着用例发的：按它的期望判，顺带更新它的 ✓ / ✗。
            page.detail.verdict = None;
            if let Some(at) = page.detail.example
                && let Some(example) = endpoint.examples.get(at)
            {
                let verdict = cases::judge(&trial, &example.expect);
                let results = &mut page.detail.run_results;
                if results.len() <= at {
                    results.resize(at + 1, None);
                }
                results[at] = Some((trial.clone(), verdict.clone()));
                page.detail.verdict = Some(verdict);
            }
            page.detail.elapsed = page.detail.started.map(|start| start.elapsed());
            page.detail.result = Some(trial);
            page.detail.output = None;
            page.detail.test = None;
        }
        Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
        Err(TryRecvError::Disconnected) => page.detail.test = None,
    }
}

/// 取回「全部跑一遍」与「AI 编用例」的结果。
fn poll_examples(ctx: &egui::Context, page: &mut ApisPage, index: usize) {
    let mut finished = false;
    if let Some(rx) = &page.detail.runs {
        loop {
            match rx.try_recv() {
                Ok((at, run)) => {
                    let results = &mut page.detail.run_results;
                    if results.len() <= at {
                        results.resize(at + 1, None);
                    }
                    if page.detail.example == Some(at) {
                        page.detail.result = Some(run.0.clone());
                        page.detail.verdict = Some(run.1.clone());
                        page.detail.output = None;
                    }
                    results[at] = Some(run);
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
        let endpoint = page.store.endpoints[index].clone();
        let runs: Vec<(String, Trial, Result<(), String>)> = page
            .detail
            .run_results
            .iter()
            .enumerate()
            .filter_map(|(at, run)| {
                let (trial, verdict) = run.clone()?;
                let name = endpoint.examples.get(at)?.name.clone();
                Some((name, trial, verdict))
            })
            .collect();
        let passed = runs.iter().filter(|(_, _, v)| v.is_ok()).count();
        if !runs.is_empty() {
            page.record_suite(&endpoint, &runs);
        }
        let mut note = format!("{} 条用例，通过 {passed} 条。", runs.len());
        if let Some((name, _, Err(reason))) = runs.iter().find(|(_, _, v)| v.is_err()) {
            note.push_str(&format!("「{name}」：{reason}"));
        }
        page.detail.example_note = Some((passed == runs.len(), note));
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
                    format!(
                        "AI 编了 {added} 条用例，排在最前面；先跑一遍，跑通的再按真实返回加期望。记得保存。"
                    )
                } else {
                    "AI 没编出新的用例（和已有的重复或不合格式）。".to_string()
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

/// 后台把每条用例跑一遍：实测，再判期望。
fn start_runs(detail: &mut Detail, endpoint: &ApiEndpoint, secrets: &ApiSecrets) {
    let (tx, rx) = std::sync::mpsc::channel();
    let endpoint = endpoint.clone();
    let secrets = secrets.clone();
    detail.run_results = vec![None; endpoint.examples.len()];
    detail.example_note = None;
    std::thread::spawn(move || {
        for (at, example) in endpoint.examples.iter().enumerate() {
            let run = cases::run(&endpoint, example, &secrets);
            if tx.send((at, run)).is_err() {
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

/// 「试一下」里填的值 → 调用参数（空着的不给）。
fn form_values(detail: &Detail, endpoint: &ApiEndpoint) -> Map<String, Value> {
    endpoint
        .inputs
        .iter()
        .filter_map(|input| {
            let value = detail.args.get(&input.name)?.trim();
            (!value.is_empty()).then(|| (input.name.clone(), Value::String(value.into())))
        })
        .collect()
}

/// 用「试一下」里填的值后台实测一次。`by_hand` 是人确认过的发送：会改数据的接口也发。
fn start_test(detail: &mut Detail, endpoint: &ApiEndpoint, secrets: &ApiSecrets, by_hand: bool) {
    let args = form_values(detail, endpoint);
    detail.test = Some(if by_hand {
        let (tx, rx) = std::sync::mpsc::channel();
        let (endpoint, secrets) = (endpoint.clone(), secrets.clone());
        std::thread::spawn(move || {
            let _ = tx.send(api::trial_by_hand(&endpoint, &args, &secrets));
        });
        rx
    } else {
        spawn_trial(endpoint.clone(), args, secrets.clone())
    });
    detail.verdict = None;
    detail.result = None;
    detail.output = None;
    detail.started = Some(Instant::now());
    detail.elapsed = None;
}

// —— 顶部 ——

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
