//! AI 管理页：AI 相关的设置都收在这里——模型服务商管理、模型服务（按功能选模型）、
//! 技能、数据接口与风格。
//!
//! 布局与设置页相同：左侧分区菜单、右侧当前分区、底部常驻保存条。表单行、
//! 小标题、菜单项这些画法直接复用 `app::settings` 的，两页看起来是一套东西。
//! 网络代理是全局的（写作、复核、知识库都走它），留在设置页。
//!
//! 模型分两层管：「模型服务商管理」只管连接身份（地址、密钥、模型清单缓存，
//! 见 `app::provider_settings`）；「模型服务」按功能（智能写作（Pi）/ 文字复核 /
//! 知识库检索）各自挑一个提供商的模型，不再碰地址密钥。

use super::provider_settings::{model_picker, provider_ref_chip};
use super::settings::{
    DETAIL_MAX_WIDTH, FOOTER_HEIGHT, MENU_COLUMN_WIDTH, MENU_PANEL_MARGIN, MENU_WIDTH,
    setting_continuation, setting_row, settings_menu_item, sub_heading,
};
use crate::app::{GongwenApp, warn};
use crate::models::{ModelKind, RerankMode};
use crate::theme;
use eframe::egui;

/// 服务商卡片需要同时容纳名称、状态和操作区，比普通设置表单稍宽。
const PROVIDERS_MAX_WIDTH: f32 = 1040.0;

/// AI 管理页的分区。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AiSection {
    /// 服务提供商：接口地址、密钥与模型清单缓存。
    #[default]
    Providers,
    /// 按功能配模型：智能写作（Pi）、文字复核、知识库检索。
    ModelService,
    /// 技能：SKILL.md 的管理。
    Skills,
    /// 数据接口：内网查询接口。
    DataApis,
    /// 写法风格档案（16.15 C）。
    Styles,
}

/// 左侧菜单的分组。
const MENU_GROUPS: [(&str, &[AiSection]); 3] = [
    ("模型", &[AiSection::Providers, AiSection::ModelService]),
    ("智能体", &[AiSection::Skills, AiSection::DataApis]),
    ("写法", &[AiSection::Styles]),
];

impl AiSection {
    #[cfg(test)]
    const ALL: [AiSection; 5] = [
        AiSection::Providers,
        AiSection::ModelService,
        AiSection::Skills,
        AiSection::DataApis,
        AiSection::Styles,
    ];

    /// 菜单里显示的中文名，同时是右栏的标题。
    pub(crate) fn label(self) -> &'static str {
        match self {
            AiSection::Providers => "模型服务商管理",
            AiSection::ModelService => "模型服务",
            AiSection::Skills => "技能",
            AiSection::DataApis => "数据接口",
            AiSection::Styles => "风格",
        }
    }

    fn icon(self) -> theme::Icon {
        match self {
            AiSection::Providers => theme::Icon::Globe,
            AiSection::ModelService => theme::Icon::PlugZap,
            AiSection::Skills => theme::Icon::WandSparkles,
            AiSection::DataApis => theme::Icon::Braces,
            AiSection::Styles => theme::Icon::Quote,
        }
    }

    fn description(self) -> &'static str {
        match self {
            AiSection::Providers => {
                "管理连接地址与密钥。「模型服务」里的每个功能都从这里挑一个提供商的模型。"
            }
            AiSection::ModelService => {
                "「智能写作（Pi）」是唯一的写作模型，也服务知识库问答、检索重排、数据接口 AI 调用与\
                 任务助手识别；「文字复核」可另配一个小模型。地址与密钥到「模型服务商管理」里统一维护。"
            }
            AiSection::Skills => {
                "Pi 标准技能：标准 SKILL.md（YAML frontmatter + 自然语言步骤）与随包辅助文件。\
                 可以编辑、新建、复制内置、启停、恢复内置、导入导出；旧流程技能不再供 Pi 执行。"
            }
            AiSection::DataApis => {
                "配置内网查询接口（统计、政策库、业务系统、算法服务），供写作时取用数据。只做查询，\
                 不发任何会改变对方状态的请求；接口地址与密钥只存在本机。Pi 写作时先用 api_search\
                 搜只读接口、再用 api_call 调用，返回条目并入证据。"
            }
            AiSection::Styles => {
                "由 Pi 的风格学习技能从库内或上传样稿生成风格文件，查看、编辑并保存后，Pi 起草、改写时\
                 按文种与场合用 style_list / style_read 只读取用。只学写法不学事实；档案只存本机，可导出给同事。"
            }
        }
    }
}

/// 分区标题：图标 + 名称，下面一行小字说明，再一道分隔线。
fn section_header_ui(ui: &mut egui::Ui, section: AiSection) {
    if section == AiSection::DataApis {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("数据接口").strong());
            ui.weak("接入与测试").on_hover_text(section.description());
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        return;
    }
    ui.horizontal(|ui| {
        ui.add(
            section
                .icon()
                .image()
                .tint(theme::accent())
                .fit_to_exact_size(egui::vec2(20.0, 20.0)),
        );
        ui.label(
            egui::RichText::new(section.label())
                .size(if section == AiSection::Providers {
                    24.0
                } else {
                    theme::font_sizes::HEADING
                })
                .strong()
                .color(theme::text()),
        );
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(section.description())
            .size(theme::font_sizes::SMALL)
            .color(theme::text_muted()),
    );
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(8.0);
}

impl GongwenApp {
    /// AI 管理页：左侧分区菜单 + 右侧当前分区 + 底部保存条，与设置页同一套布局。
    pub(crate) fn ai_manage_ui(&mut self, ui: &mut egui::Ui) {
        let body_height =
            (ui.available_height() - FOOTER_HEIGHT - ui.spacing().item_spacing.y).max(0.0);
        let body_width = ui.available_width();
        ui.allocate_ui(egui::vec2(body_width, body_height), |ui| {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(MENU_COLUMN_WIDTH, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.ai_menu_ui(ui),
                );
                ui.add_space(10.0);
                let detail_width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(detail_width, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.ai_detail_ui(ui),
                );
            });
        });
        self.ai_footer_ui(ui);
    }

    fn ai_menu_ui(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::surface_sunk())
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(MENU_PANEL_MARGIN, 8))
            .show(ui, |ui| {
                ui.set_width(MENU_WIDTH);
                egui::ScrollArea::vertical()
                    .id_salt("ai_manage_menu_scroll")
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for (index, (group, sections)) in MENU_GROUPS.iter().enumerate() {
                            if index > 0 {
                                ui.add_space(10.0);
                            }
                            ui.label(
                                egui::RichText::new(*group)
                                    .size(theme::font_sizes::SMALL)
                                    .color(theme::text_muted()),
                            );
                            ui.add_space(2.0);
                            for section in *sections {
                                let selected = self.ai_section == *section;
                                if settings_menu_item(ui, selected, section.icon(), section.label())
                                    .clicked()
                                {
                                    self.ai_section = *section;
                                }
                            }
                        }
                    });
            });
    }

    fn ai_detail_ui(&mut self, ui: &mut egui::Ui) {
        let section = self.ai_section;
        let max_width = match section {
            AiSection::Providers => PROVIDERS_MAX_WIDTH,
            _ => DETAIL_MAX_WIDTH,
        };
        theme::card()
            .inner_margin(egui::Margin::symmetric(18, 14))
            .corner_radius(egui::CornerRadius::same(10))
            .show(ui, |ui| {
                if section == AiSection::Skills {
                    ui.set_min_size(ui.available_size());
                    self.skills_section_ui(ui);
                    return;
                }
                // 数据接口是左列表、右详情，两栏各自滚动，占满整个分区，不套外层滚动区。
                if section == AiSection::DataApis {
                    ui.set_min_size(ui.available_size());
                    section_header_ui(ui, section);
                    self.apis_section_ui(ui);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt(format!("ai_manage_scroll_{section:?}"))
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        if section == AiSection::Providers {
                            let width = max_width.min(ui.available_width());
                            let inset = (ui.available_width() - width) * 0.5;
                            ui.horizontal_top(|ui| {
                                ui.add_space(inset);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(width, 0.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(width);
                                        section_header_ui(ui, section);
                                        self.providers_section_ui(ui);
                                        ui.add_space(8.0);
                                    },
                                );
                            });
                            return;
                        }
                        ui.set_max_width(max_width.min(ui.available_width()));
                        section_header_ui(ui, section);
                        match section {
                            AiSection::Providers => self.providers_section_ui(ui),
                            AiSection::ModelService => self.model_service_section_ui(ui),
                            AiSection::Skills => self.skills_section_ui(ui),
                            AiSection::DataApis => self.apis_section_ui(ui),
                            AiSection::Styles => self.styles_section_ui(ui),
                        }
                        ui.add_space(8.0);
                    });
            });
    }

    /// 底部常驻保存条：数据接口没保存的修改一并落盘。
    fn ai_footer_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        if self.ai_section == AiSection::Skills {
            ui.weak("Pi 技能文件在「技能」分区单独校验与保存；切换文件会保留未保存的修改。");
            return;
        }
        ui.horizontal(|ui| {
            if theme::primary_icon_button(ui, theme::Icon::Save, "保存").clicked() {
                self.save_ai_manage();
            }
            ui.add_space(8.0);
            ui.weak("各分区的修改点保存后写入本机配置；技能文件在技能分区里单独保存。");
        });
    }

    /// 保存 AI 管理页：写配置与数据接口。
    pub(crate) fn save_ai_manage(&mut self) {
        self.persist();
        self.save_pending_apis();
    }
}

impl GongwenApp {
    /// 检查器开关。
    ///
    /// 逐项可关，是因为**一个爱误报的检查器会把整个功能连坐关掉**：用户不会
    /// 去分辨是哪一项在捣乱，只会不再打开这个面板。给了开关，坏的那项可以
    /// 单独摘掉，好的留下。
    fn revise_tasks_ui(&mut self, ui: &mut egui::Ui) {
        setting_row(ui, "启用的检查器", None, |ui| {
            ui.weak("每多开一项，一轮复核的调用次数就多一倍");
        });
        for task in &crate::revise_model::TASKS {
            let mut on = self.config.revise_model.task_enabled(task.id);
            setting_continuation(ui, |ui| {
                if ui.checkbox(&mut on, task.label).changed() {
                    self.config.revise_model.set_task_enabled(task.id, on);
                }
                if crate::revise_model::DEFAULT_DISABLED_TASKS.contains(&task.id) {
                    theme::chip(ui, "默认关闭", warn(), theme::surface());
                }
            });
        }
    }
}

impl GongwenApp {
    /// 取模型选择器的弹层状态（筛选词 + 是否显示其他类型），供 `model_picker` 借用。
    fn picker_state(&self, salt: &str) -> (String, bool) {
        (
            self.model_filter.get(salt).cloned().unwrap_or_default(),
            self.model_picker_show_all.contains(salt),
        )
    }

    /// 把模型选择器的弹层状态存回去。
    fn store_picker_state(&mut self, salt: &str, filter: String, show_all: bool) {
        self.model_filter.insert(salt.into(), filter);
        if show_all {
            self.model_picker_show_all.insert(salt.into());
        } else {
            self.model_picker_show_all.remove(salt);
        }
    }

    /// 「模型服务」分区：按功能配模型——智能写作（Pi）、文字复核、知识库检索。
    /// 地址与密钥不在这里填；模型从「模型服务商管理」缓存的清单里挑，
    /// 选择器按提供商分组、可筛选（与字体选择同一套交互）。
    fn model_service_section_ui(&mut self, ui: &mut egui::Ui) {
        let providers = self.config.providers.clone();

        // ── 智能写作（Pi） ──────────────────────────────────────────
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            sub_heading(ui, "智能写作（Pi）", None);
            let mref = self.config.pi_model.clone();
            provider_ref_chip(ui, &self.config, &mref);
        });
        ui.label(
            egui::RichText::new(
                "唯一的写作模型：侧栏写作/修改/提炼/风格学习，以及知识库问答、检索重排、\
                 数据接口 AI 调用、任务助手识别都改用它。不选就不启动 Pi，也不回落到旧起草模型；\
                 地址与密钥仍在「模型服务商管理」里维护；上下文与输出预算自动管理。\
                 每轮执行上限 30 分钟，等用户回答时暂停计时。",
            )
            .size(theme::font_sizes::SMALL)
            .color(theme::text_muted()),
        );
        setting_row(
            ui,
            "模型",
            Some("不配置时 Pi 入口给出明确的配置提示"),
            |ui| {
                let (mut filter, mut show_all) = self.picker_state("pi");
                model_picker(
                    ui,
                    "pi",
                    &providers,
                    &mut self.config.pi_model,
                    Some("未选择"),
                    &["chat"],
                    ModelKind::Chat,
                    &mut filter,
                    &mut show_all,
                );
                self.store_picker_state("pi", filter, show_all);
            },
        );
        setting_row(
            ui,
            "思考深度",
            Some("只对支持深度参数的模型生效；默认「沿用服务端」，即不发送深度参数。"),
            |ui| {
                let options = self
                    .config
                    .pi_options
                    .get_or_insert_with(crate::models::PiOptions::default);
                egui::ComboBox::from_id_salt("pi_reasoning_effort")
                    .selected_text(options.reasoning_effort.label())
                    .show_ui(ui, |ui| {
                        for effort in crate::models::ReasoningEffort::ALL {
                            ui.selectable_value(
                                &mut options.reasoning_effort,
                                effort,
                                effort.label(),
                            );
                        }
                    });
            },
        );
        self.pi_capability_ui(ui);

        ui.add_space(10.0);

        // ── 文字复核 ────────────────────────────────────────────────
        ui.horizontal(|ui| {
            sub_heading(ui, "文字复核", None);
            let mref = self.config.revise_model.model_ref.clone();
            provider_ref_chip(ui, &self.config, &mref);
        });
        ui.label(
            egui::RichText::new(
                "逐句检查语病与错别字，结果进「修订建议」，逐条确认后才改正文。\
                 独立的小模型查错，Pi 写作和风格学习不使用它；不配模型就沿用写作模型。",
            )
            .size(theme::font_sizes::SMALL)
            .color(theme::text_muted()),
        );
        ui.checkbox(
            &mut self.config.revise_model.enabled,
            "启用文字复核（起草页「审校」分区出现入口）",
        );
        ui.add_enabled_ui(self.config.revise_model.enabled, |ui| {
            setting_row(ui, "模型", Some("留空 = 沿用写作模型"), |ui| {
                let (mut filter, mut show_all) = self.picker_state("revise");
                model_picker(
                    ui,
                    "revise",
                    &providers,
                    &mut self.config.revise_model.model_ref,
                    Some("沿用写作模型"),
                    &["chat"],
                    ModelKind::Chat,
                    &mut filter,
                    &mut show_all,
                );
                self.store_picker_state("revise", filter, show_all);
            });

            sub_heading(ui, "检查器", None);
            self.revise_tasks_ui(ui);
        });

        // ── 知识库检索 ──────────────────────────────────────────────
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            sub_heading(ui, "知识库检索", None);
            let mref = self.config.rag.embedding.model_ref.clone();
            provider_ref_chip(ui, &self.config, &mref);
        });
        ui.label(
            egui::RichText::new(
                "用 embedding 与 rerank 模型检索知识库，Pi 写作时据此取证据。\
                 两个模型与写作模型相互独立。",
            )
            .size(theme::font_sizes::SMALL)
            .color(theme::text_muted()),
        );
        ui.checkbox(&mut self.config.rag.enabled, "启用知识库检索增强")
            .on_hover_text("关闭后，AI 侧栏输入框底栏的“知识库”开关不生效");
        ui.add_enabled_ui(self.config.rag.enabled, |ui| {
            setting_row(ui, "Embedding 模型", None, |ui| {
                let (mut filter, mut show_all) = self.picker_state("embedding");
                model_picker(
                    ui,
                    "embedding",
                    &providers,
                    &mut self.config.rag.embedding.model_ref,
                    None,
                    &["embedding"],
                    ModelKind::Embedding,
                    &mut filter,
                    &mut show_all,
                );
                self.store_picker_state("embedding", filter, show_all);
            });

            let rerank_hint = match self.config.rag.rerank.mode {
                RerankMode::None => {
                    "直接按混合召回的融合分取前 N 条。够用，只是排序不如重排精准。"
                }
                RerankMode::Api => {
                    "需要能提供 rerank 接口的服务（Jina / Cohere / TEI / Infinity 等）。注意：LM Studio 与 Ollama 目前均不提供该专用接口。"
                }
                RerankMode::Llm => {
                    "复用上面的对话模型给候选片段打分，不必另起服务。代价是每次检索多一次模型调用（低温短输出，通常几秒）。"
                }
            };
            setting_row(ui, "重排方式", Some(rerank_hint), |ui| {
                egui::ComboBox::from_id_salt("rerank_mode_selector")
                    .selected_text(self.config.rag.rerank.mode.label())
                    .width(300.0)
                    .show_ui(ui, |ui| {
                        for mode in RerankMode::ALL {
                            ui.selectable_value(
                                &mut self.config.rag.rerank.mode,
                                mode,
                                mode.label(),
                            );
                        }
                    });
            });
            if self.config.rag.rerank.mode == RerankMode::Api {
                setting_row(ui, "Rerank 模型", Some("留空则跳过重排"), |ui| {
                    let (mut filter, mut show_all) = self.picker_state("rerank");
                    model_picker(
                        ui,
                        "rerank",
                        &providers,
                        &mut self.config.rag.rerank.model_ref,
                        Some("不使用"),
                        &["rerank"],
                        ModelKind::Rerank,
                        &mut filter,
                        &mut show_all,
                    );
                    self.store_picker_state("rerank", filter, show_all);
                });
                setting_continuation(ui, |ui| {
                    ui.weak(
                        "rerank 的端点路径与响应字段等进阶项可在 config.json 的 rag.rerank 节调整，适配不同服务。",
                    );
                });
            }
            if self.config.rag.rerank.mode != RerankMode::None {
                setting_continuation(ui, |ui| {
                    if ui
                        .add_enabled(
                            !self.rerank_verify_busy,
                            theme::icon_text_button(theme::Icon::PlugZap, "验证重排是否真的生效"),
                        )
                        .on_hover_text(
                            "真跑一次重排。只测“连接”是不够的：服务遇到不认识的端点路径\n\
                                     可能照样返回 200，看着像连上了，实际每次重排都在静默失败。",
                        )
                        .clicked()
                    {
                        self.start_rerank_verify();
                    }
                });
                if let Some((ok, message)) = self.rerank_verify_result.clone() {
                    setting_continuation(ui, |ui| {
                        ui.colored_label(if ok { theme::accent() } else { warn() }, message);
                    });
                }
            }
        });
    }
}

impl GongwenApp {
    fn pi_capability_ui(&mut self, ui: &mut egui::Ui) {
        let current_key = self
            .config
            .pi_chat()
            .ok()
            .and_then(|chat| crate::ai_panel::pi_job::pi_model(&chat).ok())
            .and_then(|model| {
                crate::pi_agent::capability::CapabilityIdentity::new(
                    &model.base_url,
                    &model.model_id,
                    model.context_window,
                    model.window_source,
                    if model.reasoning {
                        model.thinking_level.as_deref().unwrap_or("enabled")
                    } else {
                        "default"
                    },
                    "",
                )
                .ok()
            })
            .map(|identity| identity.key())
            .unwrap_or_default();
        if current_key != self.pi_calibration_view_key {
            self.pi_calibration_view_key = current_key;
            self.pi_calibration_note = self.latest_pi_calibration_summary();
        }
        self.poll_pi_calibration();
        // 进入这一页顺手统计一次工作区占用；失败留标记，不每帧重试。
        self.refresh_pi_workspace_usage();
        if self.pi_calibration_rx.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        if self.pi_calibration_note.is_empty() {
            self.pi_calibration_note = self.latest_pi_calibration_summary();
        }
        setting_row(
            ui,
            "能力校准",
            Some(
                "只发送程序生成的通知材料；在隔离临时目录运行，不读取当前稿件与真实知识库。测试最多运行四个同题写作轮次，可随时停止。",
            ),
            |ui| {
                ui.label(egui::RichText::new(&self.pi_calibration_note).color(theme::text_muted()));
                if self.pi_calibration_rx.is_some() {
                    if ui.button("停止测试").clicked() {
                        if let Some(cancel) = &self.pi_calibration_cancel {
                            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        self.pi_calibration_note = "正在停止合成能力测试…".into();
                    }
                } else if ui.button("测试写作能力").clicked() {
                    self.start_pi_calibration();
                }
            },
        );
        ui.weak("短合成测试只给工具与策略建议；不验证长上下文工作预算。");
        // 工作区只增不减会越占越多：给一个看得见的占用数字和一个保守的清理入口。
        let usage = match (self.pi_workspace_usage, self.pi_workspace_error.as_deref()) {
            (Some((bytes, runs)), _) => {
                format!("Pi 工作区占用 {} MB（{runs} 轮）", bytes / (1024 * 1024))
            }
            (None, Some(error)) => format!("Pi 工作区占用读取失败：{error}"),
            (None, None) => "正在统计 Pi 工作区占用…".to_string(),
        };
        let busy = self.pi_workspace_busy;
        let mut prune = false;
        setting_row(
            ui,
            "工作区",
            Some(
                "清理会保留每篇稿件、任务整理和任务填写各自最近三轮。超过七天的已写入正文轮次、已结束的任务助手作业和孤儿目录可清理。",
            ),
            |ui| {
                ui.label(egui::RichText::new(&usage).color(theme::text_muted()));
                if ui
                    .add_enabled(!busy, egui::Button::new("清理旧工作区"))
                    .on_disabled_hover_text("正在统计或清理，请稍等")
                    .clicked()
                {
                    prune = true;
                }
            },
        );
        if prune {
            self.start_pi_workspace_prune();
        }
    }

    fn latest_pi_calibration_summary(&self) -> String {
        // 槽位感知：诱饵系统读自己的 `alt-pi`，不会露出真实系统的校准档案。
        let Ok(path) = crate::storage::slot_subdir("pi").map(|dir| dir.join("capability.json"))
        else {
            return "尚未校准。普通 Pi 写作按实际窗口预算运行。".into();
        };
        match crate::pi_agent::capability::load(&path) {
            Ok(entries) => entries
                .iter()
                .rev()
                .find(|entry| self.pi_calibration_profile_matches_current(entry))
                .map(|entry| {
                    format!(
                        "上次：{} · 当时窗口 {} token（{}）· {}；每轮写作仍核对实际部署与技能内容。",
                        entry.conclusion.completed_at,
                        entry
                            .identity
                            .as_ref()
                            .map(|id| id.context_window)
                            .unwrap_or_default(),
                        entry
                            .identity
                            .as_ref()
                            .map(|id| id.window_source.description())
                            .unwrap_or("窗口来源未知"),
                        entry.conclusion.recommendation
                    )
                })
                .unwrap_or_else(|| "尚未校准。普通 Pi 写作按实际窗口预算运行。".into()),
            Err(error) => format!("校准档案不可用：{error}。普通 Pi 写作按实际窗口预算运行。"),
        }
    }

    fn start_pi_calibration(&mut self) {
        // 同一时间只跑一轮 Pi 正文写作：校准会和写作抢同一个本地模型与服务端
        // 并发额度，等写作结束再测试，结果才可信。
        if crate::pi_agent::writing_active() {
            self.pi_calibration_note = "有 Pi 任务正在运行，等它结束后再测试。".into();
            return;
        }
        let chat = match self.config.pi_chat() {
            Ok(chat) => chat,
            Err(error) => {
                self.pi_calibration_note = format!("未启动：{error}");
                return;
            }
        };
        let mut model = match crate::ai_panel::pi_job::pi_model(&chat) {
            Ok(model) => model,
            Err(error) => {
                self.pi_calibration_note = format!("未启动：{error}");
                return;
            }
        };
        let probe_chat = chat.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        // 校准也占全局 Pi 门禁：测试期间不让另一篇稿件开始写作。
        let writing = match crate::pi_agent::begin_writing() {
            Ok(writing) => writing,
            Err(error) => {
                self.pi_calibration_note = format!("未启动：{error}");
                return;
            }
        };
        match std::thread::Builder::new()
            .name("pi-capability-calibration".into())
            .spawn(move || {
                let _writing = writing;
                let mut probe = probe_chat;
                probe.timeout_seconds = probe.timeout_seconds.clamp(1, 3);
                let actual = crate::lmstudio::context::window(&probe);
                model.context_window = actual.tokens.min(u32::MAX as usize).max(1) as u32;
                model.window_source =
                    crate::pi_agent::capability::CapabilityWindowSource::from_window(actual.source);
                model.max_tokens = model
                    .max_tokens
                    .min(model.context_window.saturating_sub(1_024).max(1));
                let checked_window = model.context_window;
                let checked_source = model.window_source;
                let mut result = crate::pi_agent::capability::run_paired_calibration(
                    model,
                    worker_cancel,
                    |_| {},
                );
                if let Ok((profile, _)) = &result {
                    let after = crate::lmstudio::context::window(&probe);
                    let after_window = after.tokens.min(u32::MAX as usize).max(1) as u32;
                    let after_source =
                        crate::pi_agent::capability::CapabilityWindowSource::from_window(
                            after.source,
                        );
                    if after_window != checked_window
                        || after_source != checked_source
                        || profile.identity.as_ref().is_none_or(|identity| {
                            identity.context_window != checked_window
                                || identity.window_source != checked_source
                        })
                    {
                        result = Err("测试期间实际加载窗口发生变化；旧窗口结果已丢弃。".into());
                    }
                }
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                self.pi_calibration_rx = Some(rx);
                self.pi_calibration_cancel = Some(cancel);
                self.pi_calibration_note = "正在运行隔离合成测试…".into();
            }
            Err(error) => self.pi_calibration_note = format!("启动校准后台线程失败：{error}"),
        }
    }

    fn poll_pi_calibration(&mut self) {
        let Some(receiver) = &self.pi_calibration_rx else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("Pi 能力测试后台线程意外退出。".into()))
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
        };
        let Some(result) = result else {
            return;
        };
        let cancelled = self
            .pi_calibration_cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Relaxed));
        self.pi_calibration_rx = None;
        self.pi_calibration_cancel = None;
        if cancelled {
            self.pi_calibration_note = "能力测试已停止，本次结果未保存。".into();
            return;
        }
        match result {
            Ok((mut profile, evidence))
                if self.pi_calibration_profile_matches_current(&profile) =>
            {
                let saved = (|| -> anyhow::Result<()> {
                    // 校准档案与证据都落在槽位目录里：诱饵系统不碰真实系统的 `pi/`。
                    let pi_dir = crate::storage::slot_subdir("pi")?;
                    profile.evidence_digest = crate::pi_agent::capability::save_evidence(
                        &pi_dir.join("calibration-evidence"),
                        &evidence,
                    )
                    .map_err(anyhow::Error::msg)?;
                    let path = pi_dir.join("capability.json");
                    let mut entries = crate::pi_agent::capability::load(&path).unwrap_or_default();
                    entries.retain(|entry| entry.identity != profile.identity);
                    entries.push(profile.clone());
                    if entries.len() > 32 {
                        entries.drain(..entries.len() - 32);
                    }
                    crate::pi_agent::capability::save(&path, &entries).map_err(anyhow::Error::msg)
                })();
                self.pi_calibration_note = match saved {
                    Ok(()) => format!(
                        "校准完成：{} · 窗口 {} token（{}）· {} · 证据 {}",
                        profile.conclusion.completed_at,
                        profile
                            .identity
                            .as_ref()
                            .map(|identity| identity.context_window)
                            .unwrap_or_default(),
                        profile
                            .identity
                            .as_ref()
                            .map(|identity| identity.window_source.description())
                            .unwrap_or("窗口来源未知"),
                        profile.conclusion.recommendation,
                        &profile.evidence_digest[..12]
                    ),
                    Err(error) => {
                        format!("校准结果未保存：{error}。普通 Pi 写作按实际窗口预算运行。")
                    }
                };
            }
            Ok(_) => {
                self.pi_calibration_note =
                    "测试期间 Pi 模型服务或思考设置已更改；本次结果已丢弃。".into()
            }
            Err(error) => {
                self.pi_calibration_note =
                    format!("校准未完成：{error}。普通 Pi 写作按实际窗口预算运行。")
            }
        }
    }

    fn pi_calibration_profile_matches_current(
        &self,
        profile: &crate::pi_agent::capability::CapabilityProfile,
    ) -> bool {
        let (Some(identity), Ok(chat)) = (&profile.identity, self.config.pi_chat()) else {
            return false;
        };
        let Ok(model) = crate::ai_panel::pi_job::pi_model(&chat) else {
            return false;
        };
        // 校准身份必须包含当前有效窗口值。若用户在后台测试期间修改了窗口，
        // 不要用档案旧值重建身份而误把过期结果显示为当前结果。
        if model.context_window != identity.context_window {
            return false;
        }
        crate::pi_agent::capability::CapabilityIdentity::new(
            &model.base_url,
            &model.model_id,
            model.context_window,
            model.window_source,
            model
                .thinking_level
                .as_deref()
                .unwrap_or(if model.reasoning {
                    "enabled"
                } else {
                    "default"
                }),
            &identity.skill_digest,
        )
        .is_ok_and(|current| current == *identity)
    }

    /// 进入模型服务页时统计一次 Pi 工作区占用。失败留标记，不每帧重试。
    fn refresh_pi_workspace_usage(&mut self) {
        if self.pi_workspace_busy
            || self.pi_workspace_usage.is_some()
            || self.pi_workspace_error.is_some()
        {
            return;
        }
        self.pi_workspace_busy = true;
        let sender = self.sender.clone();
        // 统计要遍历工作区目录，放到后台线程，别卡住界面。
        std::thread::spawn(move || {
            let result = crate::pi_agent::workspace_usage();
            let _ = sender.send(crate::app::WorkerResult::PiWorkspaceUsage(result));
        });
    }

    /// 清理旧 Pi 工作区，保留每组最近 3 轮。
    fn start_pi_workspace_prune(&mut self) {
        if self.pi_workspace_busy {
            return;
        }
        self.pi_workspace_busy = true;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = crate::pi_agent::prune_workspaces(
                3,
                std::time::Duration::from_secs(7 * 24 * 60 * 60),
            );
            let _ = sender.send(crate::app::WorkerResult::PiWorkspacePruned(result));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_calibration_discards_an_already_queued_success_without_touching_the_document() {
        use crate::pi_agent::capability::{
            CalibrationEvidence, CapabilityIdentity, CapabilityProfile,
        };
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let _reset = Reset;
        let mut app = crate::app::management_samples::fixture();
        app.config.providers = vec![crate::models::ProviderConfig {
            id: "local".into(),
            name: "测试服务".into(),
            enabled: true,
            base_url: "http://127.0.0.1:12345/v1".into(),
            ..Default::default()
        }];
        app.config.pi_model = crate::models::ModelRef {
            provider_id: "local".into(),
            model: "test-model".into(),
        };
        let model = crate::ai_panel::pi_job::pi_model(&app.config.pi_chat().unwrap()).unwrap();
        app.docs[0].generated_markdown = "测试前正文保持不变".into();
        let profile = CapabilityProfile {
            identity: Some(
                CapabilityIdentity::new(
                    "http://127.0.0.1:12345/v1",
                    "test-model",
                    model.context_window,
                    model.window_source,
                    "default",
                    "test",
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        assert!(app.pi_calibration_profile_matches_current(&profile));
        let evidence = CalibrationEvidence {
            version: 1,
            task: "自造任务".into(),
            materials: vec![],
            knowledge_fact: String::new(),
            manuscript: String::new(),
            runs: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok((profile, evidence))).unwrap();
        app.pi_calibration_rx = Some(rx);
        app.pi_calibration_cancel = Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            true,
        )));
        app.poll_pi_calibration();
        assert!(app.pi_calibration_note.contains("已停止"));
        assert!(!dir.path().join("pi/capability.json").exists());
        assert!(!dir.path().join("pi/calibration-evidence").exists());
        assert_eq!(app.docs[0].generated_markdown, "测试前正文保持不变");
    }

    #[test]
    fn same_numeric_window_from_a_different_source_does_not_match_calibration() {
        use crate::pi_agent::capability::{
            CapabilityIdentity, CapabilityProfile, CapabilityWindowSource,
        };
        let mut app = crate::app::management_samples::fixture();
        app.config.providers = vec![crate::models::ProviderConfig {
            id: "local".into(),
            name: "测试服务".into(),
            enabled: true,
            base_url: "http://127.0.0.1:12345/v1".into(),
            ..Default::default()
        }];
        app.config.pi_model = crate::models::ModelRef {
            provider_id: "local".into(),
            model: "test-model".into(),
        };
        let model = crate::ai_panel::pi_job::pi_model(&app.config.pi_chat().unwrap()).unwrap();
        let other_source = if model.window_source == CapabilityWindowSource::NativeLoaded {
            CapabilityWindowSource::Service
        } else {
            CapabilityWindowSource::NativeLoaded
        };
        let profile = CapabilityProfile {
            identity: Some(
                CapabilityIdentity::new(
                    &model.base_url,
                    &model.model_id,
                    model.context_window,
                    other_source,
                    "default",
                    "test",
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        assert!(!app.pi_calibration_profile_matches_current(&profile));
    }

    #[test]
    fn same_window_source_with_a_different_numeric_window_does_not_match_calibration() {
        use crate::pi_agent::capability::{CapabilityIdentity, CapabilityProfile};
        let mut app = crate::app::management_samples::fixture();
        app.config.providers = vec![crate::models::ProviderConfig {
            id: "local".into(),
            name: "测试服务".into(),
            enabled: true,
            base_url: "http://127.0.0.1:12345/v1".into(),
            ..Default::default()
        }];
        app.config.pi_model = crate::models::ModelRef {
            provider_id: "local".into(),
            model: "test-model".into(),
        };
        let model = crate::ai_panel::pi_job::pi_model(&app.config.pi_chat().unwrap()).unwrap();
        let profile = CapabilityProfile {
            identity: Some(
                CapabilityIdentity::new(
                    &model.base_url,
                    &model.model_id,
                    model.context_window.saturating_add(1),
                    model.window_source,
                    "default",
                    "test",
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        assert!(!app.pi_calibration_profile_matches_current(&profile));
    }

    /// 设置搬家后每个分区都还在菜单里，而且只出现一次。
    #[test]
    fn every_section_is_reachable_from_the_menu() {
        let listed: Vec<AiSection> = MENU_GROUPS
            .iter()
            .flat_map(|(_, sections)| sections.iter().copied())
            .collect();
        assert_eq!(listed.len(), AiSection::ALL.len());
        for section in AiSection::ALL {
            assert_eq!(listed.iter().filter(|item| **item == section).count(), 1);
            assert!(!section.description().is_empty());
        }
    }
}
