//! 「模型服务商管理」分区与按功能选模型的可搜索选择器。
//!
//! 提供商卡片管连接身份（名称、地址、密钥、模型清单缓存）；模型选择器按提供商
//! 分组列出各家的缓存清单，交互与字体选择一致：弹层顶部筛选框自动聚焦、输入即筛
//! （同时匹配模型名与提供商名）、选中即关。一家都没刷新出模型时退化为「提供商
//! 下拉 + 手填模型名」，与改造前的兜底一致。

use super::settings::setting_field;
use crate::app::{GongwenApp, warn};
use crate::lmstudio::context::{WindowSource, peek_window, tokens_label};
use crate::models::{ModelKind, ModelRef, PROVIDER_PRESETS, ProviderConfig, ProviderPreset};
use crate::theme;
use eframe::egui;

/// 一家提供商的探测状态（本进程内，不落盘；模型清单缓存在配置里）。
#[derive(Default, Clone)]
pub(crate) struct ProviderProbe {
    pub busy: bool,
    pub connected: bool,
    pub note: String,
}

/// 连接状态与模型数量分开显示；未探测和探测失败不能混为一谈。
fn probe_chip(
    status: Option<&ProviderProbe>,
    provider: &ProviderConfig,
) -> (String, egui::Color32) {
    if !provider.enabled {
        return ("已停用".into(), theme::text_muted());
    }
    match status {
        Some(s) if s.busy => ("正在连接…".into(), theme::text_muted()),
        Some(s) if s.connected => ("已连接".into(), theme::success()),
        Some(s) if !s.note.is_empty() => ("连接失败".into(), warn()),
        _ if !provider.models.is_empty() => ("待测试 · 有缓存".into(), theme::text_muted()),
        _ => ("待测试".into(), theme::text_muted()),
    }
}

#[derive(Clone, Copy)]
enum ProviderAction {
    Probe,
    Edit,
    Toggle,
    Delete,
}

/// 操作区固定在右侧；窄窗口移到独立一行，避免与身份信息相互挤压。
fn provider_actions_ui(
    ui: &mut egui::Ui,
    provider: &ProviderConfig,
    busy: bool,
    editing: bool,
    action: &mut Option<ProviderAction>,
) {
    // 用水平行约束操作区高度；直接在纵向容器里右对齐会拿整页剩余高度居中。
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("更多", |ui| {
                if ui
                    .add(theme::menu_item(
                        if provider.enabled {
                            theme::Icon::Square
                        } else {
                            theme::Icon::SquareCheck
                        },
                        if provider.enabled {
                            "停用服务商"
                        } else {
                            "启用服务商"
                        },
                    ))
                    .clicked()
                {
                    *action = Some(ProviderAction::Toggle);
                    ui.close();
                }
                ui.separator();
                if ui
                    .add(theme::menu_item(theme::Icon::Trash, "删除服务商"))
                    .clicked()
                {
                    *action = Some(ProviderAction::Delete);
                    ui.close();
                }
            });
            if ui
                .add(theme::icon_text_button(
                    theme::Icon::Edit,
                    if editing { "收起" } else { "编辑" },
                ))
                .clicked()
            {
                *action = Some(ProviderAction::Edit);
            }
            if ui
                .add_enabled(
                    !busy,
                    theme::icon_text_button(theme::Icon::Refresh, "刷新模型"),
                )
                .on_hover_text("测试连接并更新模型清单")
                .clicked()
            {
                *action = Some(ProviderAction::Probe);
            }
        })
    });
}

fn provider_identity_ui(
    ui: &mut egui::Ui,
    provider: &ProviderConfig,
    status: Option<&ProviderProbe>,
) {
    let (label, color) = probe_chip(status, provider);
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(&provider.name)
                    .size(18.0)
                    .strong()
                    .color(theme::text()),
            )
            .truncate(),
        )
        .on_hover_text(&provider.name);
        theme::chip(ui, &label, color, theme::surface_sunk());
    });
}

/// 等宽卡片：身份与操作、接口地址、按需展开的模型清单和编辑表单。
fn provider_card_ui(
    ui: &mut egui::Ui,
    provider: &mut ProviderConfig,
    status: Option<&ProviderProbe>,
    editing: bool,
) -> egui::InnerResponse<Option<ProviderAction>> {
    theme::card()
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut action = None;
            let busy = status.is_some_and(|s| s.busy);
            let width = ui.available_width();
            if width >= 560.0 {
                ui.horizontal_top(|ui| {
                    let identity_width = width - 260.0 - ui.spacing().item_spacing.x;
                    ui.allocate_ui_with_layout(
                        egui::vec2(identity_width, 30.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(identity_width);
                            provider_identity_ui(ui, provider, status);
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(260.0, 30.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(260.0);
                            provider_actions_ui(ui, provider, busy, editing, &mut action);
                        },
                    );
                });
            } else {
                provider_identity_ui(ui, provider, status);
                ui.add_space(4.0);
                provider_actions_ui(ui, provider, busy, editing, &mut action);
            }
            ui.add_space(6.0);
            let address = if provider.base_url.is_empty() {
                "尚未填写接口地址"
            } else {
                &provider.base_url
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(address)
                        .family(egui::FontFamily::Monospace)
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_muted()),
                )
                .truncate(),
            )
            .on_hover_text(address);
            // 成功状态已在标题显示；只将失败原因留在正文，避免重复报数。
            if let Some(s) = status
                && !s.busy
                && !s.connected
                && !s.note.is_empty()
            {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&s.note)
                        .size(theme::font_sizes::SMALL)
                        .color(warn()),
                );
            }
            ui.add_space(8.0);
            ui.separator();
            if provider.models.is_empty() {
                ui.label(
                    egui::RichText::new("暂无模型清单，点击「刷新模型」获取。")
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_muted()),
                );
            } else {
                egui::CollapsingHeader::new(format!("模型清单 · {} 个", provider.models.len()))
                    .id_salt("models")
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("model_list")
                            .max_height(180.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    for model in &provider.models {
                                        theme::chip(
                                            ui,
                                            model,
                                            theme::text_soft(),
                                            theme::surface_sunk(),
                                        );
                                    }
                                });
                            });
                    });
            }
            if editing {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                setting_field(ui, "名称", &mut provider.name, "服务商名称");
                setting_field(ui, "接口地址", &mut provider.base_url, "包含 /v1");
                // 密钥用密码框显示，免于编辑时明文暴露在界面上。
                super::settings::setting_row(ui, "API Key", None, |ui| {
                    crate::ime::exempt(
                        ui.add(
                            theme::field(
                                &mut provider.api_key,
                                "本地服务通常可留空；只保存在本机",
                                f32::INFINITY,
                            )
                            .password(true),
                        ),
                    );
                });
            }
            action
        })
}

impl GongwenApp {
    /// 「模型服务商管理」分区：分组添加入口与等宽服务商卡片。
    pub(crate) fn providers_section_ui(&mut self, ui: &mut egui::Ui) {
        let mut preset_to_add = None;
        theme::card()
            .fill(theme::surface_sunk())
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new("添加服务商").strong().size(16.0));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, local) in [("本地 / 内网服务", true), ("在线服务", false)]
                    {
                        ui.menu_button((theme::Icon::Plus.image(), label), |ui| {
                            for preset in PROVIDER_PRESETS
                                .iter()
                                .filter(|p| matches!(p.id, "lmstudio" | "ollama" | "vllm") == local)
                            {
                                if ui
                                    .add(theme::menu_text_item(preset.name))
                                    .on_hover_text(preset.note)
                                    .clicked()
                                {
                                    preset_to_add = Some(preset);
                                    ui.close();
                                }
                            }
                        });
                    }
                    if ui
                        .add(theme::icon_text_button(theme::Icon::Plus, "自定义"))
                        .on_hover_text("任意 OpenAI 兼容接口")
                        .clicked()
                    {
                        preset_to_add = Some(&CUSTOM_PRESET);
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("选择预设后填写连接信息；地址与密钥仅保存在本机。")
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_muted()),
                );
            });
        if let Some(preset) = preset_to_add {
            self.add_provider_from_preset(preset);
        }
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("已添加的服务商").strong().size(16.0));
            ui.weak(format!("{} 家", self.config.providers.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !self.config.providers.is_empty(),
                        theme::icon_text_button(theme::Icon::Refresh, "全部刷新"),
                    )
                    .on_hover_text("测试所有已启用的服务商，并更新模型清单")
                    .clicked()
                {
                    self.start_all_provider_probes();
                }
            });
        });
        ui.add_space(8.0);
        if self.config.providers.is_empty() {
            ui.label(
                egui::RichText::new("尚未添加服务商，请从上方选择本地或在线服务。")
                    .color(theme::text_muted()),
            );
        }
        // 每张卡片使用独立 ID；动作在绘制结束后执行，编辑字段仍直接写入配置。
        let mut pending = None;
        for provider in &mut self.config.providers {
            let editing = self.provider_edit.contains(&provider.id);
            let status = self.provider_status.get(&provider.id);
            let provider_id = provider.id.clone();
            let action = ui
                .push_id(&provider_id, |ui| {
                    provider_card_ui(ui, provider, status, editing).inner
                })
                .inner;
            if let Some(action) = action {
                pending = Some((provider.id.clone(), action));
            }
            ui.add_space(10.0);
        }
        if let Some((id, action)) = pending {
            match action {
                ProviderAction::Probe => self.start_provider_probe(&id),
                ProviderAction::Edit => {
                    if !self.provider_edit.remove(&id) {
                        self.provider_edit.insert(id);
                    }
                }
                ProviderAction::Toggle => {
                    if let Some(provider) = self.config.provider_mut(&id) {
                        provider.enabled = !provider.enabled;
                    }
                }
                ProviderAction::Delete => self.delete_provider(&id),
            }
        }
    }
    fn add_provider_from_preset(&mut self, preset: &ProviderPreset) {
        let id = self.config.next_provider_id();
        self.config.providers.push(ProviderConfig {
            id: id.clone(),
            name: preset.name.to_string(),
            base_url: preset.base_url.to_string(),
            tags: preset.tags.iter().map(|t| (*t).to_string()).collect(),
            ..ProviderConfig::default()
        });
        // 地址空（vLLM 内网、OpenCode-Go、自定义）直接展开编辑让人填。
        if preset.base_url.is_empty() || preset.needs_key {
            self.provider_edit.insert(id.clone());
        }
        if !preset.base_url.is_empty() && !preset.needs_key {
            self.start_provider_probe(&id);
        }
        self.status = format!("已添加「{}」。", preset.name);
    }

    /// 删除提供商：引用它的功能保留选择（界面上出警示芯片），这里提示一声。
    fn delete_provider(&mut self, id: &str) {
        let used: Vec<&str> = [
            (&self.config.draft_model, "文字起草"),
            (&self.config.revise_model.model_ref, "文字复核"),
            (&self.config.rag.embedding.model_ref, "知识库 embedding"),
            (&self.config.rag.rerank.model_ref, "知识库 rerank"),
        ]
        .into_iter()
        .filter(|(mref, _)| mref.provider_id == id)
        .map(|(_, func)| func)
        .collect();
        let name = self
            .config
            .provider(id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.config.providers.retain(|p| p.id != id);
        self.provider_edit.remove(id);
        self.provider_status.remove(id);
        self.status = if used.is_empty() {
            format!("已删除「{name}」。")
        } else {
            format!(
                "已删除「{name}」；{}还在引用它，请到「模型服务」重新选择模型。",
                used.join("、")
            )
        };
    }
}

/// 「自定义」预设：不落进 [`PROVIDER_PRESETS`] 的匹配逻辑，只供添加按钮用。
const CUSTOM_PRESET: ProviderPreset = ProviderPreset {
    id: "custom",
    name: "自定义提供商",
    base_url: "",
    needs_key: true,
    note: "任意 OpenAI 兼容接口",
    tags: &[],
};

/// 功能块标题旁的提供商状态芯片：名字 + 模型数；引用失效时换成警示。
pub(crate) fn provider_ref_chip(
    ui: &mut egui::Ui,
    config: &crate::models::AppConfig,
    mref: &ModelRef,
) {
    if mref.is_empty() {
        return;
    }
    match config.provider(&mref.provider_id) {
        Some(p) => {
            let text = if p.models.is_empty() {
                format!("{} · 未刷新", p.name)
            } else {
                format!("{} · {} 个模型", p.name, p.models.len())
            };
            let color = if p.enabled { theme::accent() } else { warn() };
            theme::chip(ui, &text, color, theme::surface());
        }
        None => {
            theme::chip(ui, "原提供商已删除", warn(), theme::surface());
        }
    }
}

/// 用途过滤规则：选对话模型时只藏认得出是向量 / 重排的（认不出的按对话照列，
/// 误藏比误列糟）；选向量 / 重排时严格只列同类，其余进「其他类型」展开行。
fn kind_matches_want(kind: ModelKind, want: ModelKind) -> bool {
    match want {
        ModelKind::Chat => !matches!(kind, ModelKind::Embedding | ModelKind::Rerank),
        _ => kind == want,
    }
}

/// 按功能选模型：弹层按提供商分组列出缓存的模型清单，可筛选、按用途过滤。
///
/// `allow_empty` 提供「留空」项（复核 = 沿用起草，rerank = 跳过重排）；
/// `prefer_tags` 让带对应用途标签的提供商排在前面；`want` 是要选的用途——
/// 选对话模型时藏起向量 / 重排模型，选向量 / 重排时严格只列同类，
/// 被过滤的收进弹层底部「其他类型」展开行（`show_all`）防止误藏。
/// `filter` / `show_all` 的存放处由调用方按 salt 存在应用状态里。
#[allow(clippy::too_many_arguments)]
pub(crate) fn model_picker(
    ui: &mut egui::Ui,
    salt: &str,
    providers: &[ProviderConfig],
    selection: &mut ModelRef,
    allow_empty: Option<&str>,
    prefer_tags: &[&str],
    want: ModelKind,
    filter: &mut String,
    show_all: &mut bool,
) {
    let available: Vec<&ProviderConfig> = providers
        .iter()
        .filter(|p| p.enabled && !p.models.is_empty())
        .collect();

    // 兜底：一家都没刷新出模型清单时，退回「提供商下拉 + 手填模型名」。
    if available.is_empty() {
        let mut picked = selection.provider_id.clone();
        egui::ComboBox::from_id_salt(format!("{salt}_provider"))
            .selected_text(
                providers
                    .iter()
                    .find(|p| p.id == picked)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "选择提供商".to_string()),
            )
            .width(160.0)
            .show_ui(ui, |ui| {
                for p in providers.iter().filter(|p| p.enabled) {
                    ui.selectable_value(&mut picked, p.id.clone(), &p.name);
                }
            });
        if picked != selection.provider_id {
            selection.provider_id = picked;
        }
        crate::ime::exempt(ui.text_edit_singleline(&mut selection.model)).on_hover_text(
            "还没有任何提供商刷新出模型清单；可手填模型名，或先到「模型服务商管理」刷新",
        );
        return;
    }

    let selected_label = if selection.is_empty() {
        allow_empty.unwrap_or("请选择模型").to_string()
    } else {
        match providers.iter().find(|p| p.id == selection.provider_id) {
            Some(p) => format!("{}（{}）", selection.model, p.name),
            None => format!("{}（提供商已删除）", selection.model),
        }
    };
    let button = ui
        .push_id(format!("model_picker_{salt}"), |ui| {
            ui.add(theme::dropdown_button(selected_label, 340.0))
        })
        .inner;
    egui::Popup::menu(&button)
        .width(400.0)
        // 与字体选择同样的理由：弹层里有筛选框，只能点外面才关，选中后主动关。
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(ui.available_width());
            let search = ui.add(theme::field(filter, "输入模型或提供商名筛选", 360.0));
            if !search.has_focus() && ui.memory(|memory| memory.focused().is_none()) {
                search.request_focus();
            }
            ui.separator();
            if let Some(empty_label) = allow_empty
                && ui
                    .selectable_label(selection.is_empty(), format!("（{empty_label}）"))
                    .clicked()
            {
                selection.model.clear();
                ui.close();
            }
            let mut list = theme::popup_scroll(280.0);
            if search.changed() {
                list = list.vertical_scroll_offset(0.0);
            }
            list.show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let needle = filter.trim().to_lowercase();
                // 用途标签匹配的提供商排前面；其余保持添加顺序。
                let mut groups: Vec<&&ProviderConfig> = available.iter().collect();
                groups.sort_by_key(|p| {
                    if p.tags.iter().any(|t| prefer_tags.contains(&t.as_str())) {
                        0
                    } else {
                        1
                    }
                });
                let mut shown = 0usize;
                let mut others = 0usize;
                for provider in groups {
                    let kind_of = |model: &String| {
                        provider
                            .kinds
                            .get(model)
                            .copied()
                            .unwrap_or(ModelKind::Unknown)
                    };
                    let items: Vec<&String> = provider
                        .models
                        .iter()
                        .filter(|model| {
                            needle.is_empty()
                                || model.to_lowercase().contains(&needle)
                                || provider.name.to_lowercase().contains(&needle)
                        })
                        .collect();
                    // 用途过滤：认得出用途的按 want 筛选，认不出的（Unknown）在对话
                    // 选择器里照列、在向量 / 重排选择器里收进展开行——误藏比误列糟。
                    let visible: Vec<&String> = items
                        .iter()
                        .copied()
                        .filter(|model| *show_all || kind_matches_want(kind_of(model), want))
                        .collect();
                    others += items.len() - visible.len();
                    if visible.is_empty() {
                        continue;
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {} 个模型",
                            provider.name,
                            provider.models.len()
                        ))
                        .size(theme::font_sizes::SMALL)
                        .color(theme::text_muted()),
                    );
                    for model in visible {
                        shown += 1;
                        let picked =
                            selection.provider_id == provider.id && selection.model == *model;
                        // 服务自报过上下文窗口的行尾标出来，帮着挑模型。
                        let window_label = {
                            let probe = crate::models::LmStudioConfig {
                                base_url: provider.base_url.clone(),
                                model: model.clone(),
                                context_window: 0,
                                ..Default::default()
                            };
                            let window = peek_window(&probe);
                            (window.source == WindowSource::Service)
                                .then(|| tokens_label(window.tokens))
                        };
                        let mut row = match window_label {
                            Some(label) => format!("{model}　{label}"),
                            None => model.clone(),
                        };
                        // 「显示全部」时类型不符的标出来历，免得误选。
                        let kind = kind_of(model);
                        if *show_all && kind != ModelKind::Unknown && kind != want {
                            row = format!("{row}　（{}）", kind.label());
                        }
                        if ui.selectable_label(picked, row).clicked() {
                            selection.provider_id = provider.id.clone();
                            selection.model = model.clone();
                            ui.close();
                        }
                    }
                }
                if shown == 0 && others == 0 {
                    ui.weak("没有匹配的模型；去「模型服务商管理」刷新模型清单。");
                }
                if others > 0 {
                    ui.separator();
                    let label = if *show_all {
                        format!("收起 {others} 个其他类型的模型")
                    } else {
                        format!("还有 {others} 个其他类型的模型，点击显示")
                    };
                    if ui
                        .selectable_label(
                            false,
                            egui::RichText::new(label)
                                .size(theme::font_sizes::SMALL)
                                .color(theme::text_muted()),
                        )
                        .clicked()
                    {
                        *show_all = !*show_all;
                    }
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provider(name: &str, count: usize) -> ProviderConfig {
        ProviderConfig {
            id: name.into(),
            name: name.into(),
            base_url: format!("https://example.com/{}/v1", "long-path/".repeat(20)),
            models: (0..count).map(|n| format!("model-{n}")).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn cached_models_do_not_imply_a_failed_connection() {
        let provider = sample_provider("DMXAPI", 523);
        assert_eq!(probe_chip(None, &provider).0, "待测试 · 有缓存");
        let failed = ProviderProbe {
            note: "连接失败：超时".into(),
            ..Default::default()
        };
        assert_eq!(probe_chip(Some(&failed), &provider).0, "连接失败");
        let connected = ProviderProbe {
            connected: true,
            ..Default::default()
        };
        assert_eq!(probe_chip(Some(&connected), &provider).0, "已连接");
    }

    /// 真正运行 egui 布局，覆盖窄窗口、长地址和不同模型数量，防止卡片再次按内容收缩。
    #[test]
    fn cards_keep_equal_width_and_collapse_large_model_lists() {
        let ctx = egui::Context::default();
        theme::configure_icons(&ctx);
        for width in [360.0, 760.0, 1040.0] {
            let mut providers = [
                sample_provider("DMXAPI", 523),
                sample_provider("硅基流动", 97),
            ];
            let mut rects = Vec::new();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width + 40.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(width);
                    rects.clear();
                    for provider in &mut providers {
                        let id = provider.id.clone();
                        let rect = ui
                            .push_id(id, |ui| {
                                provider_card_ui(ui, provider, None, false).response.rect
                            })
                            .inner;
                        rects.push(rect);
                    }
                },
            );
            assert!((rects[0].width() - width).abs() < 2.0, "{width}: {rects:?}");
            assert!(
                (rects[0].width() - rects[1].width()).abs() < 1.0,
                "{rects:?}"
            );
            assert!(
                (rects[0].height() - rects[1].height()).abs() < 1.0,
                "{rects:?}"
            );
            assert!(rects.iter().all(|rect| rect.height() < 180.0), "{rects:?}");
            let mut more_positions = Vec::new();
            fn collect_more(shape: &egui::Shape, positions: &mut Vec<egui::Pos2>) {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == "更多" => {
                        positions.push(text.pos)
                    }
                    egui::Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect_more(shape, positions);
                        }
                    }
                    _ => {}
                }
            }
            for shape in &output.shapes {
                collect_more(&shape.shape, &mut more_positions);
            }
            assert_eq!(more_positions.len(), 2);
            for (position, card) in more_positions.iter().zip(&rects) {
                assert!(
                    position.x > card.right() - 80.0,
                    "操作区没有靠右：{position:?}, {card:?}"
                );
            }
        }
    }
}
