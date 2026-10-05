//! 「服务与密钥」：一台服务器（`http://host:port`）上的接口通常共用一把 Key，在这里填一次、
//! 换一次，下面的接口都用它，换完可以全部重测。原来那张独立的密钥表收到这里——密钥挂在
//! 服务器下面，接口详情顶上的绑定条再写明各自用的是哪把。

use super::super::settings::setting_row;
use super::detail::{panel, short_host};
use super::{
    ApisPage, AuthForm, Health, View, auth_form_ui, host_of, key_chip, key_field, key_state,
    list_row, method_tag, path_of, secret_for,
};
use crate::agent::api::TestStatus;
use crate::agent::api_import::{auth, rename_secret_refs};
use crate::theme;
use eframe::egui;
use std::collections::BTreeMap;

/// 服务页的临时状态，换服务器时清空。
#[derive(Default)]
pub(super) struct ServiceState {
    /// 改过、还没触发重测的密钥。
    edited: Vec<String>,
    /// 密钥改名的输入框：原名 → 正在输入的新名（离开输入框才生效）。
    names: BTreeMap<String, String>,
    auth_form: AuthForm,
}

pub(super) fn service_ui(ui: &mut egui::Ui, page: &mut ApisPage, host: &str) {
    let members: Vec<usize> = page
        .store
        .endpoints
        .iter()
        .enumerate()
        .filter(|(_, e)| host_of(&e.url) == host)
        .map(|(i, _)| i)
        .collect();
    if members.is_empty() {
        page.view = View::None;
        return;
    }
    let state = key_state(page, &members);
    ui.horizontal(|ui| {
        ui.add(
            theme::Icon::Globe
                .image_sized(18.0)
                .tint(theme::text_muted()),
        );
        ui.label(
            egui::RichText::new(short_host(host))
                .monospace()
                .size(theme::font_sizes::HEADING)
                .strong(),
        );
        key_chip(ui, &state);
    });
    theme::caption(
        ui,
        &format!(
            "{host} 上有 {} 个接口。同一个服务通常共用一把 Key：在这里填一次，下面的接口都用它。",
            members.len()
        ),
    );
    ui.add_space(12.0);
    egui::ScrollArea::vertical()
        .id_salt(("api_service_scroll", host))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            keys_panel(ui, page, &members);
            ui.add_space(12.0);
            endpoints_panel(ui, page, &members);
            ui.add_space(16.0);
        });
}

/// 这台服务器用到的密钥：名字、Key、带法；都不带的可以一起加上鉴权。
fn keys_panel(ui: &mut egui::Ui, page: &mut ApisPage, members: &[usize]) {
    let mut names: Vec<String> = Vec::new();
    for &index in members {
        for name in page.store.endpoints[index].secret_names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    let mut retest = Vec::new();
    let mut rename = None;
    let mut added = None;
    panel(
        ui,
        theme::Icon::Shield,
        "密钥",
        None,
        |_| {},
        |ui| {
            if names.is_empty() {
                theme::caption(
                    ui,
                    "这台服务器上的接口都不带密钥。接口要 Key 的话，在这里加上鉴权，下面的接口一起加：",
                );
                added = auth_form_ui(ui, &mut page.service.auth_form);
                return;
            }
            for (n, name) in names.iter().enumerate() {
                if n > 0 {
                    ui.add_space(6.0);
                    theme::hairline(ui);
                    ui.add_space(6.0);
                }
                let users = page.users_of(name);
                let outside: Vec<String> = users
                    .iter()
                    .filter(|i| !members.contains(i))
                    .map(|&i| page.store.endpoints[i].name.clone())
                    .collect();
                let how = users.iter().find_map(|&i| {
                    auth::from_endpoint(&page.store.endpoints[i])
                        .filter(|(_, secret)| secret == name)
                        .map(|(spec, _)| spec.describe())
                });
                let rejected: Vec<String> = users
                    .iter()
                    .map(|&i| &page.store.endpoints[i])
                    .filter(|e| matches!(page.log.status(e), TestStatus::Failed(r) if r.auth))
                    .map(|e| e.name.clone())
                    .collect();
                setting_row(
                    ui,
                    "名字",
                    Some("请求头或地址里写成 {secret:名字} 引用它；改名时接口里的引用跟着改"),
                    |ui| {
                        let buffer = page
                            .service
                            .names
                            .entry(name.clone())
                            .or_insert_with(|| name.clone());
                        let response = ui.add(
                            egui::TextEdit::singleline(buffer)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(180.0),
                        );
                        if response.lost_focus() && buffer != name {
                            rename = Some((name.clone(), buffer.trim().to_string()));
                        }
                    },
                );
                setting_row(ui, "Key", None, |ui| {
                    let value = page.secrets.secrets.entry(name.clone()).or_default();
                    let response = key_field(ui, value);
                    let filled = !value.trim().is_empty();
                    if response.changed() {
                        page.dirty = true;
                        if !page.service.edited.contains(name) {
                            page.service.edited.push(name.clone());
                        }
                    }
                    if response.lost_focus() && filled && page.service.edited.contains(name) {
                        page.service.edited.retain(|n| n != name);
                        retest.extend(users.iter().copied());
                    }
                    if !filled {
                        ui.colored_label(theme::warn(), "还没填");
                    } else if !rejected.is_empty() {
                        ui.colored_label(
                            theme::danger(),
                            format!("「{}」鉴权没过，Key 可能已过期", rejected.join("」「")),
                        );
                    } else {
                        theme::caption(ui, "已填");
                    }
                });
                setting_row(ui, "带法", None, |ui| {
                    ui.label(how.as_deref().unwrap_or("—"));
                    theme::caption(ui, "要改带法，在接口的「技术配置 → 请求头」里改");
                });
                if !outside.is_empty() {
                    theme::caption(
                        ui,
                        &format!(
                            "别的服务器上的「{}」也用这把 Key，改了一起生效。",
                            outside.join("」「")
                        ),
                    );
                }
            }
            ui.add_space(6.0);
            theme::caption(
                ui,
                "粘贴后点别处，用到它的接口自动重测。Key 只存在本机，导出配置、同步稿件时都不带，也不发给模型；记得保存。",
            );
        },
    );
    if let Some(spec) = added {
        let secret = secret_for(&spec, &mut page.secrets);
        for &index in members {
            let endpoint = &mut page.store.endpoints[index];
            if !auth::has_auth(endpoint) {
                auth::apply(endpoint, &spec, &secret);
            }
        }
        page.dirty = true;
    }
    if let Some((old, new)) = rename {
        rename_secret(page, &old, &new);
    }
    if !retest.is_empty() {
        page.retest(&retest);
    }
}

/// 密钥改名：名字合格、没有重名才改，接口里的引用跟着改；不合格就退回原名。
fn rename_secret(page: &mut ApisPage, old: &str, new: &str) {
    let valid = new.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && new.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    page.service.names.remove(old);
    if !valid {
        page.message = Some((
            false,
            "密钥名要用英文字母或下划线开头，只能有字母、数字、下划线。".into(),
        ));
        return;
    }
    if page.secrets.secrets.contains_key(new) {
        page.message = Some((false, format!("已经有叫「{new}」的密钥了。")));
        return;
    }
    if let Some(value) = page.secrets.secrets.remove(old) {
        page.secrets.secrets.insert(new.to_string(), value);
    }
    let renamed = [(old.to_string(), new.to_string())];
    for endpoint in &mut page.store.endpoints {
        rename_secret_refs(endpoint, &renamed);
    }
    page.dirty = true;
}

/// 这台服务器上的接口：状态一目了然，点一行打开，可以全部重测。
fn endpoints_panel(ui: &mut egui::Ui, page: &mut ApisPage, members: &[usize]) {
    let busy = members
        .iter()
        .any(|&i| page.retesting(&page.store.endpoints[i]));
    let mut retest_all = false;
    let mut open = None;
    panel(
        ui,
        theme::Icon::List,
        "接口",
        None,
        |ui| {
            if busy {
                theme::spinner(ui, 14.0, theme::accent());
                theme::caption(ui, "重测中…");
            } else if ui
                .add(theme::secondary_icon_button(
                    theme::Icon::PlugZap,
                    "全部重测",
                ))
                .on_hover_text("每个接口用样例值各调一次")
                .clicked()
            {
                retest_all = true;
            }
        },
        |ui| {
            for &index in members {
                let endpoint = &page.store.endpoints[index];
                let health = Health::of(page, endpoint);
                let response = list_row(
                    ui,
                    ("api_service_row", index),
                    false,
                    egui::Margin::symmetric(8, 6),
                    |ui| {
                        ui.horizontal(|ui| {
                            theme::dot(ui, health.dot());
                            ui.label(&endpoint.name);
                            method_tag(ui, endpoint.method);
                            theme::caption(ui, path_of(&endpoint.url));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    health.chip(ui);
                                },
                            );
                        });
                    },
                );
                if response.clicked() {
                    open = Some(index);
                }
            }
            ui.add_space(4.0);
            theme::caption(ui, "换了 Key 以后点「全部重测」，看哪些接口调得通。");
        },
    );
    if retest_all {
        page.retest(members);
    }
    if let Some(index) = open {
        page.open(index);
    }
}
