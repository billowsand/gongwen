//! 「试一下」的第二种试法：说一句话让 AI 调（`docs/api-workbench.md` 6.1）。
//!
//! 模型拿到这个服务里给 AI 用的全部接口（工具说明与自主步骤里一样），挑一个、填参数，程序
//! 校验后实际调用一次。界面摆出模型选了哪个接口、填了什么、校验与调用结果；选对了点「存为
//! AI 用例」，人勾选要锁定的参数。AI 用例可以全部跑一遍，记下调对几条、用的哪个模型。

use super::try_tab::{code_view, pretty, status_line};
use super::*;
use crate::agent::apidef::tooling;
use crate::agent::board::value_to_text;

/// 起草模型的名字，记进测试记录（换模型可以对比）。
fn model_name(config: &AppConfig) -> String {
    config
        .draft_chat()
        .map(|chat| chat.model.trim().to_string())
        .unwrap_or_default()
}

/// 取回 AI 试调、全部跑一遍与出问法的结果。
pub(super) fn poll(ctx: &egui::Context, page: &mut ApisPage, index: usize) {
    let ai = &mut page.detail.ai;
    if let Some(rx) = &ai.attempting {
        match rx.try_recv() {
            Ok(Ok((question, attempt))) => {
                ai.attempting = None;
                ai.lock = attempt
                    .args
                    .keys()
                    .map(|name| (name.clone(), true))
                    .collect();
                ai.result = Some((question, attempt));
            }
            Ok(Err(error)) => {
                ai.attempting = None;
                ai.note = Some((false, error));
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
            Err(TryRecvError::Disconnected) => ai.attempting = None,
        }
    }
    let mut finished = false;
    if let Some(rx) = &ai.runs {
        loop {
            match rx.try_recv() {
                Ok((at, verdict)) => {
                    if ai.run_results.len() <= at {
                        ai.run_results.resize(at + 1, None);
                    }
                    ai.run_results[at] = Some(verdict);
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(150));
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
        ai.runs = None;
        let endpoint = page.store.endpoints[index].clone();
        let runs: Vec<(String, Result<(), String>)> = page
            .detail
            .ai
            .run_results
            .iter()
            .enumerate()
            .filter_map(|(at, verdict)| {
                Some((
                    endpoint.ai_cases.get(at)?.question.clone(),
                    verdict.clone()?,
                ))
            })
            .collect();
        let passed = runs.iter().filter(|(_, v)| v.is_ok()).count();
        let model = page
            .detail
            .ai
            .note
            .take()
            .map(|(_, m)| m)
            .unwrap_or_default();
        if !runs.is_empty() {
            page.record_ai_suite(&endpoint, &model, &runs);
        }
        let mut note = format!(
            "{} 条 AI 用例，调对 {passed} 条（模型 {model}）。",
            runs.len()
        );
        if let Some((question, Err(reason))) = runs.iter().find(|(_, v)| v.is_err()) {
            note.push_str(&format!("「{question}」：{reason}"));
        }
        page.detail.ai.note = Some((passed == runs.len(), note));
    }
    let ai = &mut page.detail.ai;
    if let Some(rx) = &ai.proposing {
        match rx.try_recv() {
            Ok(Ok((cases, notes))) => {
                ai.proposing = None;
                let mut text = format!("AI 出了 {} 个问法，逐条看过参数再加。", cases.len());
                if !notes.is_empty() {
                    text.push_str(&format!("（{}）", notes.join("；")));
                }
                ai.proposals = cases;
                ai.note = Some((true, text));
            }
            Ok(Err(error)) => {
                ai.proposing = None;
                ai.note = Some((false, error));
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
            Err(TryRecvError::Disconnected) => ai.proposing = None,
        }
    }
}

/// 后台跑一次 AI 试调。
fn start_attempt(page: &mut ApisPage, index: usize, config: &AppConfig, question: String) {
    let (tx, rx) = std::sync::mpsc::channel();
    let apis = page.store.clone();
    let secrets = page.secrets.clone();
    let scope = ai_cases::scope(&apis, &apis.endpoints[index]);
    let config = config.clone();
    std::thread::spawn(move || {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = LmBackend::new(&config, cancel);
        let result = ai_cases::attempt(&backend, &apis, &scope, &question, &secrets)
            .map(|attempt| (question, attempt))
            .map_err(|e| format!("调用模型失败：{e:#}"));
        let _ = tx.send(result);
    });
    page.detail.ai.attempting = Some(rx);
    page.detail.ai.result = None;
    page.detail.ai.note = None;
}

/// 后台把这个接口的 AI 用例逐条跑一遍。模型名先放在 `note` 里，跑完记录时取出。
fn start_runs(page: &mut ApisPage, index: usize, config: &AppConfig) {
    let (tx, rx) = std::sync::mpsc::channel();
    let apis = page.store.clone();
    let secrets = page.secrets.clone();
    let endpoint = apis.endpoints[index].clone();
    let scope = ai_cases::scope(&apis, &endpoint);
    let config = config.clone();
    page.detail.ai.run_results = vec![None; endpoint.ai_cases.len()];
    page.detail.ai.note = Some((true, model_name(&config)));
    std::thread::spawn(move || {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = LmBackend::new(&config, cancel);
        for (at, case) in endpoint.ai_cases.iter().enumerate() {
            let verdict = ai_cases::attempt(&backend, &apis, &scope, &case.question, &secrets)
                .map_err(|e| format!("调用模型失败：{e:#}"))
                .and_then(|attempt| ai_cases::judge(case, &endpoint.id, &attempt, &apis));
            if tx.send((at, verdict)).is_err() {
                return;
            }
        }
    });
    page.detail.ai.runs = Some(rx);
}

fn start_proposing(page: &mut ApisPage, index: usize, config: &AppConfig) {
    let (tx, rx) = std::sync::mpsc::channel();
    let endpoint = page.store.endpoints[index].clone();
    let config = config.clone();
    std::thread::spawn(move || {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let backend = LmBackend::new(&config, cancel);
        let _ = tx.send(ai_cases::propose(&backend, &endpoint));
    });
    page.detail.ai.proposing = Some(rx);
    page.detail.ai.note = None;
}

/// 「说一句话让 AI 调」整页：左边提问与 AI 用例，右边模型怎么调的。
pub(super) fn ai_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize, config: &AppConfig) {
    let width = ui.available_width();
    if width >= 720.0 {
        let gap = 14.0;
        let left = ((width - gap) * 0.42).floor();
        let right = width - gap - left - ui.spacing().item_spacing.x;
        ui.horizontal_top(|ui| {
            cell(ui, left, |ui| ask_box(ui, page, index, config));
            ui.add_space(gap - ui.spacing().item_spacing.x);
            cell(ui, right, |ui| answer_box(ui, page, index));
        });
    } else {
        ask_box(ui, page, index, config);
        ui.add_space(12.0);
        answer_box(ui, page, index);
    }
}

/// AI 用例一行：每条一个标签（跑过的标上调没调对）；选中的显示期望的参数与删除。
fn cases_bar(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) -> Option<usize> {
    let mut picked = None;
    let mut remove = None;
    let endpoint = &page.store.endpoints[index];
    if endpoint.ai_cases.is_empty() {
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        theme::caption(ui, "AI 用例");
        for (at, case) in endpoint.ai_cases.iter().enumerate() {
            let result = page.detail.ai.run_results.get(at).and_then(Option::as_ref);
            let mark = match result {
                Some(Ok(())) => "✓ ",
                Some(Err(_)) => "✗ ",
                None => "",
            };
            let label = crate::agent::tools::short(&case.question, 16);
            let mut response =
                ui.selectable_label(page.detail.ai.case == Some(at), format!("{mark}{label}"));
            let mut hover = case.question.clone();
            if let Some(Err(reason)) = result {
                hover.push('\n');
                hover.push_str(reason);
            }
            response = response.on_hover_text(hover);
            if response.clicked() {
                picked = Some(at);
            }
        }
    });
    if let Some(at) = page.detail.ai.case
        && let Some(case) = page.store.endpoints[index].ai_cases.get(at)
    {
        let expected: Vec<String> = case
            .expect_args
            .iter()
            .map(|(name, value)| format!("{name} = {}", value_to_text(value)))
            .collect();
        theme::caption(
            ui,
            &if expected.is_empty() {
                "期望：调这个接口（参数不限）".to_string()
            } else {
                format!("期望：调这个接口，{}", expected.join("，"))
            },
        );
        if ui.small_button("删除这条 AI 用例").clicked() {
            remove = Some(at);
        }
    }
    if let Some(at) = remove {
        page.store.endpoints[index].ai_cases.remove(at);
        if at < page.detail.ai.run_results.len() {
            page.detail.ai.run_results.remove(at);
        }
        page.detail.ai.case = None;
        page.dirty = true;
    }
    ui.add_space(6.0);
    picked
}

/// 左边：AI 用例、提问框、「让 AI 调」、全部跑一遍、AI 出问法与候选。
fn ask_box(ui: &mut egui::Ui, page: &mut ApisPage, index: usize, config: &AppConfig) {
    let endpoint = page.store.endpoints[index].clone();
    let scope = ai_cases::scope(&page.store, &endpoint);
    let has_model = config.draft_chat().is_ok();
    let mut ask = false;
    let mut run_all = false;
    let mut propose = false;
    let mut accept = None;
    let mut reject = None;
    let mut picked = None;
    panel(
        ui,
        theme::Icon::Sparkles,
        "提问",
        None,
        |_| {},
        |ui| {
            if !tooling::usable(&endpoint) {
                theme::notice(
                    ui,
                    theme::Icon::Shield,
                    theme::warn(),
                    theme::warn_soft(),
                    "这个接口不给 AI 用（会改数据、没开放给 AI，或者现在发不了），没法让 AI 试。",
                );
                return;
            }
            if !has_model {
                theme::notice(
                    ui,
                    theme::Icon::TriangleAlert,
                    theme::warn(),
                    theme::warn_soft(),
                    "还没有选起草模型：到「AI 管理 → 模型服务」选好再试。",
                );
                return;
            }
            picked = cases_bar(ui, page, index);
            ui.add(
                egui::TextEdit::multiline(&mut page.detail.ai.question)
                    .desired_rows(2)
                    .hint_text("像用户那样问一句，例如：2025 年全省森林火灾多少起？")
                    .desired_width(f32::INFINITY),
            );
            theme::caption(
                ui,
                &if scope.len() > tooling::TWO_LEVEL_AT {
                    format!(
                        "模型能挑这个服务里给 AI 用的 {} 个接口；接口多，模型要先搜再调。",
                        scope.len()
                    )
                } else {
                    format!(
                        "模型能挑这个服务里给 AI 用的 {} 个接口，工具说明和技能里看到的一样。",
                        scope.len()
                    )
                },
            );
            ui.add_space(6.0);
            let busy = page.detail.ai.attempting.is_some();
            ui.horizontal(|ui| {
                if theme::primary_icon_button_enabled(
                    ui,
                    !busy && !page.detail.ai.question.trim().is_empty(),
                    theme::Icon::Sparkles,
                    "让 AI 调",
                )
                .clicked()
                {
                    ask = true;
                }
                if busy {
                    theme::spinner(ui, 14.0, theme::accent());
                    ui.weak("模型在挑接口…");
                }
            });
            ui.add_space(4.0);
            let running = page.detail.ai.runs.is_some();
            let proposing = page.detail.ai.proposing.is_some();
            ui.horizontal_wrapped(|ui| {
                let count = endpoint.ai_cases.len();
                if count > 0
                    && ui
                        .add_enabled(
                            !running,
                            theme::secondary_icon_button(
                                theme::Icon::ListOrdered,
                                &format!("全部 {count} 条 AI 用例跑一遍"),
                            ),
                        )
                        .on_hover_text("逐条让模型调，判接口选对没有、写明的参数对不对、调用成没成，记下正确率")
                        .clicked()
                {
                    run_all = true;
                }
                if ui
                    .add_enabled(
                        !proposing,
                        theme::secondary_icon_button(theme::Icon::Sparkles, "AI 出几个问法"),
                    )
                    .on_hover_text("让模型按接口说明出几句不同问法与它认为的参数；你逐条确认后才加进 AI 用例")
                    .clicked()
                {
                    propose = true;
                }
                if running || proposing {
                    theme::spinner(ui, 14.0, theme::accent());
                    ui.weak(if running { "逐条让 AI 调…" } else { "AI 在出问法…" });
                }
            });
            if let Some((ok, text)) = &page.detail.ai.note
                && page.detail.ai.runs.is_none()
            {
                let color = if *ok {
                    theme::text_muted()
                } else {
                    theme::warn()
                };
                ui.colored_label(color, text);
            }
            for (at, case) in page.detail.ai.proposals.iter().enumerate() {
                ui.add_space(4.0);
                strip(ui, Tone::Ok, |ui| {
                    ui.label(&case.question);
                    let args: Vec<String> = case
                        .expect_args
                        .iter()
                        .map(|(name, value)| format!("{name} = {}", value_to_text(value)))
                        .collect();
                    theme::caption(ui, &format!("期望参数：{}", args.join("，")));
                    ui.horizontal(|ui| {
                        if ui.small_button("加进 AI 用例").clicked() {
                            accept = Some(at);
                        }
                        if ui.small_button("不要").clicked() {
                            reject = Some(at);
                        }
                    });
                });
            }
        },
    );
    if let Some(at) = picked {
        page.detail.ai.case = Some(at);
        page.detail.ai.question = page.store.endpoints[index].ai_cases[at].question.clone();
    }
    if ask {
        let question = page.detail.ai.question.trim().to_string();
        start_attempt(page, index, config, question);
    }
    if run_all {
        start_runs(page, index, config);
    }
    if propose {
        start_proposing(page, index, config);
    }
    if let Some(at) = accept {
        let case = page.detail.ai.proposals.remove(at);
        page.store.endpoints[index].ai_cases.push(case);
        page.dirty = true;
    }
    if let Some(at) = reject {
        page.detail.ai.proposals.remove(at);
    }
}

/// 右边：模型选了哪个接口、填了什么、校验与调用结果；选对了可以存成 AI 用例。
fn answer_box(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let endpoint = page.store.endpoints[index].clone();
    let mut save = false;
    panel(
        ui,
        theme::Icon::ArrowDown,
        "模型怎么调的",
        None,
        |_| {},
        |ui| {
            let Some((question, attempt)) = &page.detail.ai.result else {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    theme::caption(
                        ui,
                        "在左边问一句，点「让 AI 调」：这里显示模型选了哪个接口、填了什么参数、调用结果。",
                    );
                });
                ui.add_space(24.0);
                return;
            };
            theme::caption(ui, &format!("问：{question}"));
            ui.add_space(4.0);
            if !attempt.searches.is_empty() {
                theme::caption(ui, &format!("先搜了：{}", attempt.searches.join("、")));
            }
            match &attempt.api {
                None => {
                    theme::notice(
                        ui,
                        theme::Icon::TriangleAlert,
                        theme::danger(),
                        theme::danger_soft(),
                        if attempt.reply.is_empty() {
                            "模型没有调接口。".to_string()
                        } else {
                            format!("模型没有调接口，它说：{}", attempt.reply)
                        },
                    );
                    return;
                }
                Some(api) if *api == endpoint.id => {
                    ui.colored_label(
                        theme::success(),
                        format!("模型调了这个接口「{}」", endpoint.name),
                    );
                }
                Some(api) => {
                    let name = page
                        .store
                        .get(api)
                        .map_or(api.as_str(), |e| e.name.as_str());
                    theme::notice(
                        ui,
                        theme::Icon::TriangleAlert,
                        theme::warn(),
                        theme::warn_soft(),
                        format!(
                            "模型调的是另一个接口「{name}」。如果它选得对，到那个接口里去存 AI 用例；\
                             选得不对，多半是两个接口的用途写得不够分明。"
                        ),
                    );
                }
            }
            ui.add_space(4.0);
            if attempt.args.is_empty() {
                theme::caption(ui, "没填参数。");
            }
            for (name, value) in &attempt.args {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(name).monospace());
                    ui.label("=");
                    ui.label(value_to_text(value));
                });
            }
            if let Some(first) = &attempt.retried {
                theme::caption(
                    ui,
                    &format!("第一次参数不对，程序回给了模型、它改过一次：{first}"),
                );
            }
            if let Err(error) = &attempt.check
                && attempt.trial.is_none()
            {
                theme::notice(
                    ui,
                    theme::Icon::TriangleAlert,
                    theme::danger(),
                    theme::danger_soft(),
                    error.clone(),
                );
            }
            if let Some(trial) = &attempt.trial {
                ui.add_space(6.0);
                status_line(ui, trial, None, false);
                if let Some(error) = &trial.error {
                    theme::caption(ui, error);
                }
                if let Some(raw) = &trial.raw {
                    egui::CollapsingHeader::new("原始返回")
                        .id_salt(("ai_raw", index))
                        .show(ui, |ui| {
                            code_view(ui, ("ai_raw_view", index), &pretty(&raw.body))
                        });
                }
            }
            let picked_this = attempt.api.as_deref() == Some(endpoint.id.as_str());
            if picked_this && attempt.check.is_ok() {
                ui.add_space(8.0);
                theme::caption(
                    ui,
                    "选得对就存成 AI 用例。勾上要锁定的参数（不勾的模型可以自由发挥，比如页码）：",
                );
                ui.horizontal_wrapped(|ui| {
                    for (name, locked) in &mut page.detail.ai.lock {
                        ui.checkbox(locked, name.as_str());
                    }
                });
                if ui
                    .add(theme::secondary_icon_button(
                        theme::Icon::Save,
                        "存为 AI 用例",
                    ))
                    .clicked()
                {
                    save = true;
                }
            }
        },
    );
    if save && let Some((question, attempt)) = &page.detail.ai.result {
        let locked: Vec<&String> = page
            .detail
            .ai
            .lock
            .iter()
            .filter(|(_, locked)| *locked)
            .map(|(name, _)| name)
            .collect();
        let args: Map<String, Value> = attempt
            .args
            .iter()
            .filter(|(name, _)| locked.contains(name))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let case = ApiAiCase {
            question: question.clone(),
            expect_args: ai_cases::expect_from(&endpoint, &args)
                .into_iter()
                .collect(),
        };
        let cases = &mut page.store.endpoints[index].ai_cases;
        if cases.iter().any(|c| c.question == case.question) {
            page.detail.ai.note = Some((false, "已经有一样问法的 AI 用例了。".into()));
        } else {
            cases.push(case);
            page.detail.ai.case = Some(cases.len() - 1);
            page.dirty = true;
            page.detail.ai.note = Some((true, "已存为 AI 用例；记得保存。".into()));
        }
    }
}
