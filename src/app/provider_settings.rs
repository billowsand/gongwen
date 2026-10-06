//! 「模型服务商管理」分区与按功能选模型的可搜索选择器。
//!
//! 提供商卡片管连接身份（名称、地址、密钥、模型清单缓存）；模型选择器按提供商
//! 分组列出各家的缓存清单，交互与字体选择一致：弹层顶部筛选框自动聚焦、输入即筛
//! （同时匹配模型名与提供商名）、选中即关。一家都没刷新出模型时退化为「提供商
//! 下拉 + 手填模型名」，与改造前的兜底一致。

use super::settings::{setting_continuation, setting_field, sub_heading};
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

/// 状态芯片文案：已连接 · N 个模型 / 未连接 / 已停用。
fn probe_chip(
    status: Option<&ProviderProbe>,
    provider: &ProviderConfig,
) -> (String, egui::Color32) {
    if !provider.enabled {
        return ("已停用".into(), theme::text_muted());
    }
    match status {
        Some(s) if s.busy => ("正在连接…".into(), theme::text_muted()),
        Some(s) if s.connected => (
            format!("已连接 · {} 个模型", provider.models.len()),
            theme::accent(),
        ),
        _ if !provider.models.is_empty() => ("未连接 · 用缓存清单".into(), warn()),
        _ => ("未连接".into(), warn()),
    }
}

impl GongwenApp {
    /// 「模型服务商管理」分区：预设添加 + 提供商卡片列表。
    pub(crate) fn providers_section_ui(&mut self, ui: &mut egui::Ui) {
        sub_heading(ui, "添加提供商", None);
        ui.horizontal_wrapped(|ui| {
            for preset in PROVIDER_PRESETS {
                if ui
                    .add(theme::icon_text_button(theme::Icon::Plus, preset.name))
                    .on_hover_text(preset.note)
                    .clicked()
                {
                    self.add_provider_from_preset(preset);
                }
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::Plus, "自定义"))
                .on_hover_text("任意 OpenAI 兼容接口，全部自己填")
                .clicked()
            {
                self.add_provider_from_preset(&CUSTOM_PRESET);
            }
        });
        setting_continuation(ui, |ui| {
            ui.weak("点预设即建好一条，地址自动填好，只补密钥；本地服务（LM Studio / Ollama）连密钥都不用。");
        });

        sub_heading(
            ui,
            "已添加的提供商",
            Some(
                "地址与密钥只保存在本机；「测试连接 / 刷新模型」读到的模型清单缓存进配置，服务没开时也能选模型。",
            ),
        );
        if self.config.providers.is_empty() {
            ui.weak("还没有提供商：从上面的预设添加，或点「自定义」。");
        }
        // 逐张卡片画；动作先记下来，循环结束后统一执行，避免借用打架。
        let mut probe = None;
        let mut toggle_edit = None;
        let mut toggle_enabled = None;
        let mut delete = None;
        for index in 0..self.config.providers.len() {
            let provider = self.config.providers[index].clone();
            let editing = self.provider_edit.contains(&provider.id);
            let status = self.provider_status.get(&provider.id);
            let (chip, chip_color) = probe_chip(status, &provider);
            theme::card()
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&provider.name);
                        theme::chip(ui, &chip, chip_color, theme::surface());
                        ui.label(
                            egui::RichText::new(if provider.base_url.is_empty() {
                                "（未填地址）".to_string()
                            } else {
                                provider.base_url.clone()
                            })
                            .size(theme::font_sizes::SMALL)
                            .color(theme::text_muted())
                            .family(egui::FontFamily::Monospace),
                        );
                        if ui
                            .add_enabled(
                                !status.is_some_and(|s| s.busy),
                                theme::icon_text_button(
                                    theme::Icon::PlugZap,
                                    "测试连接 / 刷新模型",
                                ),
                            )
                            .clicked()
                        {
                            probe = Some(provider.id.clone());
                        }
                        if ui
                            .add(theme::icon_text_button(
                                theme::Icon::Edit,
                                if editing { "收起" } else { "编辑" },
                            ))
                            .clicked()
                        {
                            toggle_edit = Some(provider.id.clone());
                        }
                        if ui
                            .add(theme::icon_text_button(
                                if provider.enabled {
                                    theme::Icon::Square
                                } else {
                                    theme::Icon::SquareCheck
                                },
                                if provider.enabled { "停用" } else { "启用" },
                            ))
                            .clicked()
                        {
                            toggle_enabled = Some(provider.id.clone());
                        }
                        if ui
                            .add(theme::icon_text_button(theme::Icon::Trash, "删除"))
                            .clicked()
                        {
                            delete = Some(provider.id.clone());
                        }
                    });
                    if let Some(s) = status
                        && !s.note.is_empty()
                        && !s.busy
                    {
                        ui.label(
                            egui::RichText::new(&s.note)
                                .size(theme::font_sizes::SMALL)
                                .color(if s.connected {
                                    theme::text_muted()
                                } else {
                                    warn()
                                }),
                        );
                    }
                    if !provider.models.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for model in provider.models.iter().take(8) {
                                theme::chip(ui, model, theme::text_soft(), theme::surface_sunk());
                            }
                            if provider.models.len() > 8 {
                                ui.weak(format!("… 共 {} 个", provider.models.len()));
                            }
                        });
                    }
                    if editing {
                        ui.add_space(6.0);
                        ui.separator();
                        setting_field(ui, "名称", &mut self.config.providers[index].name, "");
                        setting_field(
                            ui,
                            "接口地址",
                            &mut self.config.providers[index].base_url,
                            "包含 /v1",
                        );
                        setting_field(
                            ui,
                            "API Key",
                            &mut self.config.providers[index].api_key,
                            "本地服务通常可留空；只保存在本机",
                        );
                    }
                });
            ui.add_space(4.0);
        }
        if let Some(id) = probe {
            self.start_provider_probe(&id);
        }
        if let Some(id) = toggle_edit
            && !self.provider_edit.remove(&id)
        {
            self.provider_edit.insert(id);
        }
        if let Some(id) = toggle_enabled
            && let Some(provider) = self.config.provider_mut(&id)
        {
            provider.enabled = !provider.enabled;
        }
        if let Some(id) = delete {
            self.delete_provider(&id);
        }
        setting_continuation(ui, |ui| {
            if ui
                .add(theme::icon_text_button(
                    theme::Icon::Refresh,
                    "全部刷新模型",
                ))
                .clicked()
            {
                self.start_all_provider_probes();
            }
        });
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
