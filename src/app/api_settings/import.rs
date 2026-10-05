//! 「从文档添加」：粘贴接口文档或选文件 → 识别 → 逐个核对候选接口 → 加入。
//!
//! 识别（程序解析 + 起草模型整理，见 `agent::api_import`）在后台跑；识别出的只查询接口
//! 配置齐了就自动用样例值试调一次，调通后按真实返回补返回映射。改数据的接口不能加入，
//! 判断不了的要人确认只查询。密钥先进一份工作副本，点「加入」才写进本机密钥表。

use super::{ApisPage, View, advanced_form, basic_form, code_block, spawn_trial, trial_ui};
use crate::agent::api::{ApiSecrets, Trial};
use crate::agent::api_import::{
    self, Access, Analysis, Draft, Material, merge_secrets, refine_with_reply, rename_secret_refs,
    secret_refs,
};
use crate::agent::backend::{LmBackend, ModelBackend};
use crate::models::AppConfig;
use crate::theme;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

/// 直接按文本读的格式；其余走文档转换（Word、PDF……）。
const PLAIN: &[&str] = &[
    "txt", "md", "markdown", "json", "yaml", "yml", "http", "sh", "curl",
];

/// 一次「从文档添加」的状态。
#[derive(Default)]
pub(in crate::app) struct ImportFlow {
    text: String,
    /// 读进来的文件名。
    file: Option<String>,
    reading: Option<Receiver<Result<(String, String), String>>>,
    running: Option<Running>,
    outcome: Option<Outcome>,
    error: Option<String>,
    /// 密钥工作副本：本机已有的 + 识别出来的。
    secrets: ApiSecrets,
}

struct Running {
    rx: Receiver<Result<(Analysis, Material), String>>,
    cancel: Arc<AtomicBool>,
}

struct Outcome {
    notes: Vec<String>,
    model_used: bool,
    /// 发给模型的文字（已脱敏）与遮了几处。
    redacted: String,
    masked: usize,
    candidates: Vec<Candidate>,
}

struct Candidate {
    draft: Draft,
    chosen: bool,
    /// 判断不了性质时，人勾选「只查询」。
    confirmed: bool,
    /// 资料只给了路径时，人补的服务器地址。
    origin: String,
    trial: Option<Trial>,
    testing: Option<Receiver<Trial>>,
    /// 自动试调过一次就不再自动调。
    auto_tried: bool,
    /// 按实测返回补了哪些项。
    refined: Vec<&'static str>,
}

impl Candidate {
    fn new(draft: Draft) -> Self {
        Self {
            chosen: draft.access != Access::Write,
            draft,
            confirmed: false,
            origin: String::new(),
            trial: None,
            testing: None,
            auto_tried: false,
            refined: Vec::new(),
        }
    }

    /// 当作只查询处理：判定只查询，或人确认了。
    fn query(&self) -> bool {
        self.draft.access == Access::Query
            || (self.draft.access == Access::Unknown && self.confirmed)
    }

    /// 挡住测试与加入的问题（说明没写不挡）。
    fn blockers(&self, secrets: &ApiSecrets) -> Vec<String> {
        let mut blockers: Vec<String> = self
            .draft
            .missing(secrets)
            .into_iter()
            .filter(|m| !m.starts_with("说明") && !(self.confirmed && m.starts_with("确认")))
            .collect();
        blockers.extend(self.draft.endpoint.problems().into_iter().filter(|p| {
            // 地址问题已经在缺项里说过了。
            !p.contains("http:// 或 https://")
        }));
        blockers
    }

    fn can_test(&self, secrets: &ApiSecrets) -> bool {
        self.query() && self.testing.is_none() && self.blockers(secrets).is_empty()
    }

    fn start_test(&mut self, secrets: &ApiSecrets) {
        self.testing = Some(spawn_trial(
            self.draft.endpoint.clone(),
            self.draft.endpoint.example_args(),
            secrets.clone(),
        ));
        self.trial = None;
        self.auto_tried = true;
    }

    /// 取回实测结果；拿到真实返回就补返回映射，再用同一份返回重新映射一遍（不再发请求）。
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.testing else {
            return;
        };
        match rx.try_recv() {
            Ok(mut trial) => {
                self.testing = None;
                if let Some(raw) = &trial.raw
                    && (200..300).contains(&raw.status)
                {
                    let body = raw.body.clone();
                    self.refined = refine_with_reply(&mut self.draft, &body);
                    if !self.refined.is_empty() {
                        trial.remap(&self.draft.endpoint);
                    }
                }
                self.trial = Some(trial);
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => self.testing = None,
        }
    }
}

/// 起草模型配了没有。
fn model_name(config: &AppConfig) -> Option<String> {
    let model = config.lm_studio.model.trim();
    (!model.is_empty()).then(|| model.to_string())
}

pub(super) fn import_ui(ui: &mut egui::Ui, page: &mut ApisPage, config: &AppConfig) {
    let flow = page.import.get_or_insert_with(ImportFlow::default);
    poll(ui.ctx(), flow, &page.secrets);
    if ui
        .add(theme::secondary_icon_button(theme::Icon::List, "全部接口"))
        .clicked()
    {
        page.view = View::List;
        return;
    }
    ui.add_space(6.0);
    input_ui(ui, flow, config, &page.store);
    let Some(outcome) = &mut flow.outcome else {
        return;
    };
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    for note in &outcome.notes {
        theme::notice(
            ui,
            theme::Icon::HelpCircle,
            theme::info(),
            theme::accent_soft(),
            note.clone(),
        );
        ui.add_space(4.0);
    }
    if outcome.model_used {
        egui::CollapsingHeader::new(format!(
            "发给模型的内容（遮掉了 {} 处密钥）",
            outcome.masked
        ))
        .id_salt("api_import_redacted")
        .show(ui, |ui| {
            code_block(ui, "api_import_redacted_text", &outcome.redacted)
        });
    }
    if outcome.candidates.is_empty() {
        return;
    }
    ui.label(egui::RichText::new(format!("识别出 {} 个接口", outcome.candidates.len())).strong());
    theme::caption(
        ui,
        "核对每个接口：勾选要加入的，补齐标黄的缺项。只查询且配置齐全的会自动用样例值试调一次。",
    );
    ui.add_space(6.0);
    for (index, candidate) in outcome.candidates.iter_mut().enumerate() {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            candidate_ui(ui, index, candidate, &mut flow.secrets);
        });
        ui.add_space(6.0);
    }
    // 自动试调：只查询、配置齐了、还没试过的。
    for candidate in &mut outcome.candidates {
        if !candidate.auto_tried && candidate.can_test(&flow.secrets) {
            candidate.start_test(&flow.secrets);
        }
    }
    add_bar_ui(ui, page);
}

/// 资料输入区：粘贴框、选文件、识别。
fn input_ui(
    ui: &mut egui::Ui,
    flow: &mut ImportFlow,
    config: &AppConfig,
    store: &crate::agent::api::ApiStore,
) {
    match model_name(config) {
        Some(model) => theme::caption(
            ui,
            &format!(
                "识别时程序先解析 cURL、请求行和 JSON 示例，再把资料（密钥遮掉后）交给起草模型「{model}」整理名称、说明和参数含义。"
            ),
        ),
        None => theme::notice(
            ui,
            theme::Icon::TriangleAlert,
            theme::warn(),
            theme::warn_soft(),
            "没有配置起草模型：只能识别 cURL、请求行等结构，名称和说明要自己填。可在「模型服务」里配置。",
        ),
    };
    ui.add_space(4.0);
    let collapsed = flow.outcome.is_some();
    ui.add(
        egui::TextEdit::multiline(&mut flow.text)
            .hint_text(
                "粘贴接口文档、cURL 命令、请求与返回示例……\n\n例如：\ncurl -X POST 'http://10.0.0.9/api/policy/search' -H 'Authorization: Bearer xxx' -d '{\"keyword\": \"中小企业\"}'",
            )
            .desired_rows(if collapsed { 4 } else { 12 })
            .desired_width(f32::INFINITY),
    );
    let busy = flow.reading.is_some() || flow.running.is_some();
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(
                !busy,
                theme::secondary_icon_button(theme::Icon::Open, "选文件…"),
            )
            .on_hover_text("文本、Markdown、JSON、YAML、Word、PDF 等")
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter(
                    "接口文档",
                    &[
                        "txt", "md", "json", "yaml", "yml", "docx", "doc", "pdf", "xlsx", "rtf",
                    ],
                )
                .pick_file()
        {
            flow.reading = Some(read_file(path));
            flow.error = None;
        }
        if let Some(file) = &flow.file {
            theme::caption(ui, &format!("已读入 {file}"));
        }
        let ready = !flow.text.trim().is_empty() && !busy;
        if theme::primary_icon_button_enabled(ui, ready, theme::Icon::WandSparkles, "识别")
            .clicked()
        {
            start(flow, config, store);
        }
        if let Some(running) = &flow.running {
            theme::spinner(ui, 14.0, theme::accent());
            ui.weak(if model_name(config).is_some() {
                "识别中（模型整理可能要一会儿）…"
            } else {
                "识别中…"
            });
            if ui.button("停止").clicked() {
                running.cancel.store(true, Ordering::Relaxed);
            }
        }
        if flow.reading.is_some() {
            theme::spinner(ui, 14.0, theme::accent());
            ui.weak("读取文件…");
        }
    });
    if let Some(error) = &flow.error {
        ui.colored_label(theme::danger(), error);
    }
}

fn read_file(path: PathBuf) -> Receiver<Result<(String, String), String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let name = crate::doc_import::file_label(&path);
        let text = if PLAIN.contains(&extension.as_str()) {
            crate::text_file::read_to_string(&path).map_err(|e| format!("读取 {name} 失败：{e}"))
        } else {
            crate::doc_import::to_markdown(&path).map_err(|e| format!("{e:#}"))
        };
        let _ = tx.send(text.map(|text| (name, text)));
    });
    rx
}

fn start(flow: &mut ImportFlow, config: &AppConfig, store: &crate::agent::api::ApiStore) {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let text = flow.text.clone();
    let taken: Vec<String> = store.endpoints.iter().map(|e| e.id.clone()).collect();
    let use_model = model_name(config).is_some();
    let config = config.clone();
    let stop = cancel.clone();
    std::thread::spawn(move || {
        let material = Material::new(&text);
        let backend = use_model.then(|| LmBackend::new(&config, stop.clone()));
        let analysis = api_import::analyze(
            &material,
            backend.as_ref().map(|b| b as &dyn ModelBackend),
            &taken,
        );
        let result = if stop.load(Ordering::Relaxed) {
            Err("已停止识别。".to_string())
        } else {
            Ok((analysis, material))
        };
        let _ = tx.send(result);
    });
    flow.running = Some(Running { rx, cancel });
    flow.error = None;
}

/// 取回读文件与识别的结果。
fn poll(ctx: &egui::Context, flow: &mut ImportFlow, saved: &ApiSecrets) {
    if let Some(rx) = &flow.reading {
        match rx.try_recv() {
            Ok(Ok((name, text))) => {
                flow.text = text;
                flow.file = Some(name);
                flow.reading = None;
            }
            Ok(Err(error)) => {
                flow.error = Some(error);
                flow.reading = None;
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => flow.reading = None,
        }
    }
    if let Some(running) = &flow.running {
        match running.rx.try_recv() {
            Ok(Ok((analysis, material))) => {
                flow.running = None;
                flow.secrets = saved.clone();
                let renamed = merge_secrets(&mut flow.secrets, &analysis.secrets);
                let candidates = analysis
                    .drafts
                    .into_iter()
                    .map(|mut draft| {
                        rename_secret_refs(&mut draft.endpoint, &renamed);
                        Candidate::new(draft)
                    })
                    .collect();
                flow.outcome = Some(Outcome {
                    notes: analysis.notes,
                    model_used: analysis.model_used,
                    masked: material.masked(),
                    redacted: material.redacted,
                    candidates,
                });
            }
            Ok(Err(error)) => {
                flow.running = None;
                flow.error = Some(error);
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => {
                flow.running = None;
                flow.error = Some("识别中断了。".into());
            }
        }
    }
    if let Some(outcome) = &mut flow.outcome {
        for candidate in &mut outcome.candidates {
            candidate.poll(ctx);
        }
    }
}

/// 一个候选接口的卡片。
fn candidate_ui(
    ui: &mut egui::Ui,
    index: usize,
    candidate: &mut Candidate,
    secrets: &mut ApiSecrets,
) {
    let writes = candidate.draft.access == Access::Write;
    ui.horizontal(|ui| {
        ui.add_enabled(!writes, egui::Checkbox::without_text(&mut candidate.chosen))
            .on_hover_text("加入数据接口");
        ui.add(
            egui::TextEdit::singleline(&mut candidate.draft.endpoint.name)
                .font(egui::TextStyle::Body)
                .desired_width(220.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            match candidate.draft.access {
                Access::Query => theme::chip(ui, "只查询", theme::success(), theme::success_soft()),
                Access::Write => theme::chip(ui, "会改数据", theme::danger(), theme::danger_soft()),
                Access::Unknown => theme::chip(ui, "性质待确认", theme::warn(), theme::warn_soft()),
            };
        });
    });
    if writes {
        candidate.chosen = false;
        theme::notice(
            ui,
            theme::Icon::Shield,
            theme::danger(),
            theme::danger_soft(),
            format!(
                "这个接口会改外部系统的数据，数据接口只收查询类接口，不能加入。{}",
                basis(&candidate.draft.access_basis)
            ),
        );
        return;
    }
    ui.add(
        egui::TextEdit::multiline(&mut candidate.draft.endpoint.description)
            .hint_text("这个接口能查什么、按什么查（AI 靠它判断什么时候调用）")
            .desired_rows(1)
            .desired_width(f32::INFINITY),
    );
    let endpoint = &candidate.draft.endpoint;
    ui.label(
        egui::RichText::new(format!("{} {}", endpoint.method.label(), endpoint.url))
            .size(theme::font_sizes::SMALL)
            .color(theme::text_muted()),
    );
    if !endpoint.inputs.is_empty() {
        theme::caption(ui, &format!("查询条件：{}", inputs_line(&candidate.draft)));
    }
    if !candidate.draft.origins.is_empty() {
        let line = candidate
            .draft
            .origins
            .iter()
            .map(|(field, origin)| format!("{field}·{}", origin.label()))
            .collect::<Vec<_>>()
            .join("　");
        theme::caption(ui, &format!("来源：{line}"));
    }

    // —— 人要补的东西 ——
    if candidate.draft.access == Access::Unknown {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut candidate.confirmed, "我确认这个接口只查询、不会改数据");
            if !candidate.draft.access_basis.is_empty() {
                theme::caption(ui, &candidate.draft.access_basis);
            }
        });
    }
    let url = candidate.draft.endpoint.url.trim().to_string();
    if url.starts_with('/') {
        ui.horizontal(|ui| {
            ui.label("服务器地址");
            ui.add(
                egui::TextEdit::singleline(&mut candidate.origin)
                    .hint_text("http://10.0.0.9:8080")
                    .desired_width(220.0),
            );
            let origin = candidate.origin.trim().trim_end_matches('/');
            let valid = origin.starts_with("http://") || origin.starts_with("https://");
            if ui.add_enabled(valid, egui::Button::new("补上")).clicked() {
                candidate.draft.endpoint.url = format!("{origin}{url}");
            }
        });
    }
    for name in secret_refs(&candidate.draft.endpoint) {
        let empty = secrets
            .secrets
            .get(&name)
            .is_none_or(|v| v.trim().is_empty());
        if empty {
            ui.horizontal(|ui| {
                ui.label(format!("密钥「{name}」"));
                let value = secrets.secrets.entry(name.clone()).or_default();
                ui.add(
                    egui::TextEdit::singleline(value)
                        .password(true)
                        .hint_text("资料里是占位写法，填真实的值")
                        .desired_width(260.0),
                );
            });
        }
    }
    for input in &mut candidate.draft.endpoint.inputs {
        if input.required && input.example.trim().is_empty() {
            ui.horizontal(|ui| {
                ui.label(format!("「{}」的样例值", label_of(input)));
                ui.add(
                    egui::TextEdit::singleline(&mut input.example)
                        .hint_text("测试用")
                        .desired_width(160.0),
                );
            });
        }
    }
    let blockers = candidate.blockers(secrets);
    for blocker in &blockers {
        ui.colored_label(theme::warn(), format!("· 还缺：{blocker}"));
    }
    for note in &candidate.draft.notes {
        theme::caption(ui, &format!("· {note}"));
    }

    // —— 试调 ——
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can_test = candidate.can_test(secrets);
        let label = if candidate.trial.is_some() {
            "重新测试"
        } else {
            "测试"
        };
        if ui
            .add_enabled(
                can_test,
                theme::secondary_icon_button(theme::Icon::PlugZap, label),
            )
            .on_disabled_hover_text("先补齐上面的缺项，并确认只查询")
            .clicked()
        {
            candidate.start_test(secrets);
        }
        if candidate.testing.is_some() {
            theme::spinner(ui, 14.0, theme::accent());
            ui.weak("正在用样例值试调…");
        }
    });
    if let Some(trial) = &candidate.trial {
        trial_ui(ui, trial, &format!("import_{index}"));
        if !candidate.refined.is_empty() {
            theme::caption(
                ui,
                &format!("按实测返回补了：{}", candidate.refined.join("、")),
            );
        }
    }
    egui::CollapsingHeader::new("修改配置")
        .id_salt(("api_import_edit", index))
        .show(ui, |ui| {
            basic_form(
                ui,
                &mut candidate.draft.endpoint,
                &format!("import_{index}"),
            );
            ui.add_space(6.0);
            advanced_form(
                ui,
                &mut candidate.draft.endpoint,
                &format!("import_{index}"),
            );
        });
}

fn basis(text: &str) -> String {
    if text.trim().is_empty() {
        String::new()
    } else {
        format!("依据：{}", text.trim())
    }
}

fn label_of(input: &crate::agent::api::ApiInput) -> String {
    if input.description.trim().is_empty() {
        input.name.clone()
    } else {
        format!("{}（{}）", input.description.trim(), input.name)
    }
}

/// 「keyword 政策主题（必填，例：中小企业）；size 返回条数」。
fn inputs_line(draft: &Draft) -> String {
    draft
        .endpoint
        .inputs
        .iter()
        .map(|input| {
            let mut extra = vec![input.kind.label().to_string()];
            if input.required {
                extra.push("必填".into());
            }
            if !input.example.trim().is_empty() {
                extra.push(format!("例：{}", input.example.trim()));
            }
            let description = if input.description.trim().is_empty() {
                String::new()
            } else {
                format!(" {}", input.description.trim())
            };
            format!("{}{description}（{}）", input.name, extra.join("，"))
        })
        .collect::<Vec<_>>()
        .join("；")
}

/// 底部：加入选中的接口 / 放弃。
fn add_bar_ui(ui: &mut egui::Ui, page: &mut ApisPage) {
    let Some(flow) = &page.import else {
        return;
    };
    let Some(outcome) = &flow.outcome else {
        return;
    };
    let chosen: Vec<&Candidate> = outcome.candidates.iter().filter(|c| c.chosen).collect();
    let unready: Vec<String> = chosen
        .iter()
        .filter(|c| !c.query() || !c.draft.endpoint.problems().is_empty())
        .map(|c| c.draft.endpoint.name.clone())
        .collect();
    let testing = chosen.iter().any(|c| c.testing.is_some());
    ui.add_space(4.0);
    let mut add = false;
    let mut discard = false;
    ui.horizontal_wrapped(|ui| {
        let enabled = !chosen.is_empty() && unready.is_empty() && !testing;
        if theme::primary_icon_button_enabled(
            ui,
            enabled,
            theme::Icon::Save,
            &format!("加入选中的 {} 个接口", chosen.len()),
        )
        .clicked()
        {
            add = true;
        }
        if ui.button("放弃").clicked() {
            discard = true;
        }
        if !unready.is_empty() {
            ui.colored_label(
                theme::warn(),
                format!("「{}」还没确认只查询或配置有问题", unready.join("」「")),
            );
        } else if testing {
            ui.weak("等试调结束…");
        } else if chosen
            .iter()
            .any(|c| c.trial.as_ref().is_none_or(|t| !t.ok()))
        {
            ui.weak("有接口还没调通，也可以先加入，之后在详情里再测。");
        }
    });
    if discard {
        page.import = None;
        page.view = View::List;
    } else if add {
        add_chosen(page);
    }
}

fn add_chosen(page: &mut ApisPage) {
    let Some(flow) = page.import.take() else {
        return;
    };
    let Some(outcome) = flow.outcome else {
        return;
    };
    let mut added = Vec::new();
    for candidate in outcome.candidates.into_iter().filter(|c| c.chosen) {
        let mut endpoint = candidate.draft.endpoint;
        // 识别之后别处可能新加了同 id 的接口。
        let base = endpoint.id.clone();
        let mut n = 2;
        while page.store.get(&endpoint.id).is_some() {
            endpoint.id = format!("{base}_{n}");
            n += 1;
        }
        if let Some(trial) = &candidate.trial {
            page.log.record(&endpoint, trial);
        }
        added.push(endpoint.name.clone());
        page.store.endpoints.push(endpoint);
    }
    page.secrets = flow.secrets;
    let saved = page.save().and_then(|_| page.log.save());
    page.message = Some(match saved {
        Ok(()) => (true, format!("已加入并保存：{}。", added.join("、"))),
        Err(error) => {
            page.dirty = true;
            (false, format!("已加入，但保存失败：{error:#}"))
        }
    });
    page.view = View::List;
}

#[cfg(test)]
mod tests {
    use super::super::tests::render;
    use super::*;

    const DOC: &str = "curl -X POST 'http://127.0.0.1:9/api/policy/search' -H 'Authorization: Bearer <你的令牌>' -d '{\"keyword\": \"中小企业\"}'";

    fn outcome_page() -> ApisPage {
        let material = Material::new(DOC);
        let analysis = api_import::analyze(&material, None, &[]);
        let mut flow = ImportFlow {
            text: DOC.into(),
            ..ImportFlow::default()
        };
        let mut secrets = ApiSecrets::default();
        merge_secrets(&mut secrets, &analysis.secrets);
        flow.secrets = secrets;
        flow.outcome = Some(Outcome {
            notes: analysis.notes,
            model_used: false,
            masked: material.masked(),
            redacted: material.redacted,
            candidates: analysis.drafts.into_iter().map(Candidate::new).collect(),
        });
        ApisPage {
            loaded: true,
            view: View::Import,
            import: Some(flow),
            ..ApisPage::default()
        }
    }

    #[test]
    fn candidates_show_what_is_missing_and_wait_for_confirmation() {
        let mut page = outcome_page();
        let config = AppConfig::default();
        let texts = render(|ui| import_ui(ui, &mut page, &config));
        let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
        assert!(has("识别出 1 个接口"), "{texts:?}");
        assert!(has("没有配置起草模型"), "{texts:?}");
        assert!(
            has("性质待确认") && has("我确认这个接口只查询"),
            "{texts:?}"
        );
        assert!(has("密钥「token」"), "占位令牌要人填：{texts:?}");
        assert!(has("还没确认只查询"), "{texts:?}");
        let flow = page.import.as_ref().unwrap();
        let candidate = &flow.outcome.as_ref().unwrap().candidates[0];
        assert!(candidate.testing.is_none(), "没确认只查询不自动试调");
    }

    #[test]
    fn confirmed_and_complete_candidates_are_tried_then_added() {
        let server = crate::agent::api::test_server::TestServer::start(vec![(
            200,
            serde_json::json!({"code": 0, "data": {"list": [{"id": 7, "title": "意见", "content": "正文"}]}})
                .to_string(),
        )]);
        let mut page = outcome_page();
        {
            let flow = page.import.as_mut().unwrap();
            flow.secrets.secrets.insert("token".into(), "real".into());
            let candidate = &mut flow.outcome.as_mut().unwrap().candidates[0];
            candidate.confirmed = true;
            candidate.draft.endpoint.url = format!("{}/api/policy/search", server.url);
        }
        let config = AppConfig::default();
        let _ = render(|ui| import_ui(ui, &mut page, &config));
        // 等后台试调回来。
        for _ in 0..100 {
            let flow = page.import.as_mut().unwrap();
            let ctx = egui::Context::default();
            poll(&ctx, flow, &ApiSecrets::default());
            if flow.outcome.as_ref().unwrap().candidates[0].trial.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let flow = page.import.as_ref().unwrap();
        let candidate = &flow.outcome.as_ref().unwrap().candidates[0];
        let trial = candidate.trial.as_ref().expect("自动试调过");
        assert!(trial.ok(), "{:?}", trial.error);
        assert_eq!(candidate.draft.endpoint.mapping.list, "/data/list");
        assert!(candidate.refined.contains(&"成功判据"));
        assert!(server.request(0).contains("Bearer real"));

        let dir = std::env::temp_dir().join(format!("gongwen-api-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::storage::set_test_config_dir(Some(dir.clone()));
        add_chosen(&mut page);
        crate::storage::set_test_config_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(page.view, View::List);
        assert_eq!(page.store.endpoints.len(), 1);
        assert_eq!(page.secrets.secrets["token"], "real");
        assert!(matches!(
            page.log.status(&page.store.endpoints[0]),
            crate::agent::api::TestStatus::Passed(_)
        ));
        assert!(
            page.message.as_ref().unwrap().1.contains("已加入并保存"),
            "{:?}",
            page.message
        );
    }
}
