//! 「试一下」页：左边填请求、右边看响应。用例一键填入、写期望、全部跑一遍留记录，AI 编用例；
//! 会改数据的接口发送前要人确认。

use super::*;
use crate::agent::api::ApiInput;
use crate::agent::board::value_to_text;
use crate::modal::{self, Dismiss};

pub(super) fn try_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize, config: &AppConfig) {
    theme::segmented(ui, |ui| {
        ui.selectable_value(&mut page.detail.mode, TryMode::Manual, "手填参数")
            .on_hover_text("直接调接口；用例证明接口可用");
        ui.selectable_value(&mut page.detail.mode, TryMode::Ai, "说一句话让 AI 调")
            .on_hover_text("看模型挑哪个接口、填什么参数；选对了存成 AI 用例");
    });
    ui.add_space(8.0);
    if page.detail.mode == TryMode::Ai {
        ai_try::ai_ui(ui, page, index, config);
        return;
    }
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
    confirm_send_ui(ui, page, index);
}

/// 用例一行：每条一个可点的标签（跑过的标上过没过），选中的显示说明、改名、删除与期望。
/// 返回点了哪一条。
pub(super) fn examples_bar(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) -> Option<usize> {
    let mut picked = None;
    let mut remove = None;
    let endpoint = &page.store.endpoints[index];
    if endpoint.examples.is_empty() {
        return None;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        theme::caption(ui, "用例");
        for (at, example) in endpoint.examples.iter().enumerate() {
            let mark = match page.detail.run_results.get(at).and_then(Option::as_ref) {
                Some((_, Ok(()))) => "✓ ",
                Some(_) => "✗ ",
                None => "",
            };
            let selected = page.detail.example == Some(at);
            let mut response = ui.selectable_label(selected, format!("{mark}{}", example.name));
            let mut hover = example.note.clone();
            if let Some((_, Err(reason))) = page.detail.run_results.get(at).and_then(Option::as_ref)
            {
                if !hover.is_empty() {
                    hover.push('\n');
                }
                hover.push_str(reason);
            }
            if !hover.is_empty() {
                response = response.on_hover_text(hover);
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
            if ui.small_button("删除这条").clicked() {
                remove = Some(at);
            }
        });
        if !example.note.is_empty() {
            theme::caption(ui, &example.note);
        }
        expect_editor(ui, page, index, at);
    }
    if let Some(at) = remove {
        page.store.endpoints[index].examples.remove(at);
        if at < page.detail.run_results.len() {
            page.detail.run_results.remove(at);
        }
        page.detail.example = None;
        page.detail.verdict = None;
        page.dirty = true;
    }
    ui.add_space(6.0);
    picked
}

/// 选中用例的期望：列出来可删；按种类、位置、值加一条；调通过的按这次返回给几条建议，点一下加上。
fn expect_editor(ui: &mut egui::Ui, page: &mut ApisPage, index: usize, at: usize) {
    ui.add_space(4.0);
    theme::caption(ui, "期望（先要调通且业务成功，再逐条检查）");
    let mut remove = None;
    for (i, expect) in page.store.endpoints[index].examples[at]
        .expect
        .iter()
        .enumerate()
    {
        ui.horizontal(|ui| {
            ui.label(format!("· {}", expect.label()));
            if theme::icon_button(ui, theme::Icon::Trash, "删掉这条期望").clicked() {
                remove = Some(i);
            }
        });
    }
    let mut added: Option<cases::Expect> = None;
    let form = &mut page.detail.expect_form;
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt(("api_expect_kind", index))
            .selected_text(form.kind.label())
            .width(84.0)
            .show_ui(ui, |ui| {
                for kind in ExpectKind::ALL {
                    ui.selectable_value(&mut form.kind, kind, kind.label());
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut form.pointer)
                .font(egui::TextStyle::Monospace)
                .hint_text("/data/items（空 = 整个返回）")
                .desired_width(170.0),
        );
        if form.kind.needs_value() {
            let hint = match form.kind {
                ExpectKind::MinItems => "条数",
                ExpectKind::Equals => "值",
                _ => "文字",
            };
            ui.add(
                egui::TextEdit::singleline(&mut form.value)
                    .hint_text(hint)
                    .desired_width(90.0),
            );
        }
        if ui.small_button("加上").clicked() {
            match form.kind.build(&form.pointer, &form.value) {
                Ok(expect) => added = Some(expect),
                Err(error) => form.error = Some(error),
            }
        }
    });
    if let Some(error) = &page.detail.expect_form.error {
        ui.colored_label(theme::warn(), error);
    }
    // 建议：只按调通的真实返回给，人点了才加。
    let endpoint = &page.store.endpoints[index];
    let suggestions: Vec<cases::Expect> = match &page.detail.result {
        Some(trial) if trial.ok() => trial
            .raw
            .as_ref()
            .map(|raw| cases::suggest(endpoint, &form_values(&page.detail, endpoint), &raw.body))
            .unwrap_or_default()
            .into_iter()
            .filter(|expect| !endpoint.examples[at].expect.contains(expect))
            .collect(),
        _ => Vec::new(),
    };
    if !suggestions.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            theme::caption(ui, "按这次的返回建议：");
            for expect in suggestions {
                if ui
                    .small_button(format!("＋ {}", expect.label()))
                    .on_hover_text("点一下加进这条用例的期望")
                    .clicked()
                {
                    added = Some(expect);
                }
            }
        });
    }
    let example = &mut page.store.endpoints[index].examples[at];
    if let Some(i) = remove {
        example.expect.remove(i);
        page.dirty = true;
    }
    if let Some(expect) = added {
        if !example.expect.contains(&expect) {
            example.expect.push(expect);
            page.dirty = true;
        }
        page.detail.expect_form = ExpectForm::default();
    }
}

/// 一个查询条件的输入控件：有枚举的、是否类的出下拉框，JSON 类出多行框，其余单行框。
fn input_widget(ui: &mut egui::Ui, index: usize, input: &ApiInput, value: &mut String) {
    let mut options: Vec<String> = input
        .schema
        .as_ref()
        .and_then(|schema| schema.get("enum"))
        .and_then(Value::as_array)
        .map(|values| values.iter().map(value_to_text).collect())
        .unwrap_or_default();
    if options.is_empty() && input.kind == InputKind::Bool {
        options = vec!["true".into(), "false".into()];
    }
    let shown = |text: &str| match (input.kind, text) {
        (_, "") => "（不填）".to_string(),
        (InputKind::Bool, "true") => "是".to_string(),
        (InputKind::Bool, "false") => "否".to_string(),
        _ => text.to_string(),
    };
    if !options.is_empty() {
        egui::ComboBox::from_id_salt(("api_input", index, &input.name))
            .selected_text(shown(value.trim()))
            .width(ui.available_width().min(260.0))
            .show_ui(ui, |ui| {
                if !input.required {
                    ui.selectable_value(value, String::new(), shown(""));
                }
                for option in &options {
                    ui.selectable_value(value, option.clone(), shown(option));
                }
            });
        return;
    }
    if input.kind == InputKind::Json {
        ui.add(
            egui::TextEdit::multiline(value)
                .code_editor()
                .desired_rows(5)
                .hint_text(input.example.as_str())
                .desired_width(f32::INFINITY),
        );
        if !value.trim().is_empty() && serde_json::from_str::<Value>(value.trim()).is_err() {
            ui.colored_label(theme::warn(), "不是合法的 JSON");
        }
    } else {
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(input.example.as_str())
                .desired_width(f32::INFINITY),
        );
    }
}

/// 左边：按查询条件逐项填写，点「发送请求」。有用例的先选一条。
pub(super) fn request_box(
    ui: &mut egui::Ui,
    page: &mut ApisPage,
    index: usize,
    config: &AppConfig,
) {
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
    let readonly = page.store.endpoints[index].readonly;
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
                input_widget(ui, index, input, value);
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
            if !readonly {
                ui.colored_label(
                    theme::warn(),
                    "这个接口会改数据：发送前要确认，不参加「全部跑一遍」，也不给 AI 调。",
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
                if examples_count > 0
                    && readonly
                    && ui
                        .add_enabled(
                            !running_all,
                            theme::secondary_icon_button(
                                theme::Icon::ListOrdered,
                                &format!("全部 {examples_count} 条用例跑一遍"),
                            ),
                        )
                        .on_hover_text("逐条实测并检查期望，结果记下来，作为这个接口可用的证明")
                        .clicked()
                {
                    run_all = true;
                }
                if ui
                    .add(theme::secondary_icon_button(theme::Icon::Save, "存为用例"))
                    .on_hover_text("把现在填的值存成一条用例；调通过的会按返回建议期望")
                    .clicked()
                {
                    save = true;
                }
                if has_model
                    && ui
                        .add_enabled(
                            !generating,
                            theme::secondary_icon_button(theme::Icon::Sparkles, "AI 编几条用例"),
                        )
                        .on_hover_text("按接口说明编几条典型用法，每种分支至少一条，内容取材于公文；期望要跑通后再加")
                        .clicked()
                {
                    generate = true;
                }
                if generating || running_all {
                    theme::spinner(ui, 14.0, theme::accent());
                    ui.weak(if generating {
                        "AI 在编用例…"
                    } else {
                        "逐条跑用例中…"
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
        pick_example(page, index, at);
    }
    if run_all {
        start_runs(&mut page.detail, &endpoint, &page.secrets);
    }
    if save {
        save_case(page, index);
    }
    if generate {
        start_generating(&mut page.detail, &endpoint, config);
    }
    if fill {
        page.detail.example = None;
        page.detail.verdict = None;
        page.detail.args = endpoint
            .inputs
            .iter()
            .map(|input| (input.name.clone(), pretty_json(&endpoint, &input.example)))
            .collect();
    }
    if send {
        if endpoint.readonly {
            start_test(&mut page.detail, &endpoint, &page.secrets, false);
        } else {
            // 会改数据：先把要发的请求摆给人看，确认了才发。
            let args = form_values(&page.detail, &endpoint);
            page.detail.confirm_send =
                Some(api::prepare(&endpoint, &args, &page.secrets).map(|p| p.describe()));
        }
    }
}

/// 「存为用例」：现在填的值存成一条用例并选中它。这次调通过的，结果直接算进它的 ✓，
/// 下面接着显示按返回建议的期望。
fn save_case(page: &mut ApisPage, index: usize) {
    let endpoint = &page.store.endpoints[index];
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
        page.detail.example_note = Some((false, "已经有一样的用例了。".into()));
        return;
    }
    examples.push(ApiExample {
        name: format!("用例 {}", examples.len() + 1),
        note: "手动保存".into(),
        args,
        ..Default::default()
    });
    let at = examples.len() - 1;
    page.detail.example = Some(at);
    page.detail.expect_form = ExpectForm::default();
    page.dirty = true;
    let passed = page
        .detail
        .result
        .as_ref()
        .filter(|trial| trial.ok())
        .cloned();
    let results = &mut page.detail.run_results;
    if results.len() <= at {
        results.resize(at + 1, None);
    }
    results[at] = passed.clone().map(|trial| (trial, Ok(())));
    page.detail.verdict = passed.as_ref().map(|_| Ok(()));
    page.detail.example_note = Some((
        true,
        if passed.is_some() {
            "已存为用例：可以改个名字，下面「按这次的返回建议」点一下就能加期望；记得保存。".into()
        } else {
            "已存为用例：可以改个名字；调通以后会按返回建议期望。记得保存。".into()
        },
    ));
}

/// 会改数据的接口：发送前的确认框，列出要发出的请求（密钥打码）。
fn confirm_send_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
    let Some(request) = page.detail.confirm_send.clone() else {
        return;
    };
    let mut confirmed = false;
    let mut cancelled = false;
    let response = modal::dialog(
        ui.ctx(),
        egui::Id::new(("api_confirm_send", index)),
        "发送会改数据的请求？",
        480.0,
        Dismiss::EscOrBackdrop,
        |ui| {
            theme::notice(
                ui,
                theme::Icon::TriangleAlert,
                theme::warn(),
                theme::warn_soft(),
                "这个接口没有标成「只查询」，发出去可能改变对方系统里的数据。请核对下面的请求再发。",
            );
            ui.add_space(8.0);
            match &request {
                Ok(text) => code_view(ui, ("api_confirm_request", index), text),
                Err(error) => {
                    ui.colored_label(theme::danger(), format!("请求组不出来：{error}"));
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if request.is_ok()
                    && theme::danger_icon_button(ui, theme::Icon::PlugZap, "确认发送").clicked()
                {
                    confirmed = true;
                }
                if ui.button("取消").clicked() {
                    cancelled = true;
                }
            });
        },
    );
    if confirmed {
        page.detail.confirm_send = None;
        let endpoint = page.store.endpoints[index].clone();
        start_test(&mut page.detail, &endpoint, &page.secrets, true);
    } else if cancelled || response.dismissed {
        page.detail.confirm_send = None;
    }
}

/// 右边：结论一行、出错原因、整理后的条目 / 原始返回 / 实际请求。
pub(super) fn response_box(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
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
            verdict_line(ui, page, index, trial);
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
pub(super) fn status_line(
    ui: &mut egui::Ui,
    trial: &Trial,
    elapsed: Option<Duration>,
    running: bool,
) {
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
pub(super) fn mapping_suggestion(
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
pub(super) fn pretty(body: &str) -> String {
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
pub(super) fn code_view(ui: &mut egui::Ui, id: impl egui::AsIdSalt, text: &str) {
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

/// 选着用例发的、调通了：说一句满没满足它的期望。
fn verdict_line(ui: &mut egui::Ui, page: &ApisPage, index: usize, trial: &Trial) {
    let (Some(verdict), Some(at)) = (&page.detail.verdict, page.detail.example) else {
        return;
    };
    let Some(example) = page.store.endpoints[index].examples.get(at) else {
        return;
    };
    if !trial.ok() {
        return;
    }
    ui.add_space(4.0);
    match verdict {
        Ok(()) if example.expect.is_empty() => {
            ui.colored_label(
                theme::success(),
                format!("用例「{}」通过（只要求调通且业务成功）", example.name),
            );
        }
        Ok(()) => {
            ui.colored_label(
                theme::success(),
                format!(
                    "用例「{}」通过：满足全部 {} 条期望",
                    example.name,
                    example.expect.len()
                ),
            );
        }
        Err(reason) => {
            theme::notice(
                ui,
                theme::Icon::TriangleAlert,
                theme::danger(),
                theme::danger_soft(),
                format!("用例「{}」没通过：{reason}", example.name),
            );
        }
    }
}
