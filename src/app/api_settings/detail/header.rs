//! 接口详情顶部：名称与状态、用途、地址与密钥绑定条（没填或被拒时就地粘贴、自动重测）。

use super::*;

pub(super) fn header_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
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
pub(super) fn binding_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
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
            start_test(&mut page.detail, &endpoint, &page.secrets, false);
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
pub(super) fn no_auth_ui(ui: &mut egui::Ui, page: &mut ApisPage, index: usize) {
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
