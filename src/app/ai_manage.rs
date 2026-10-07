//! AI 管理页：AI 相关的设置都收在这里——模型服务商管理、模型服务（按功能选模型）、
//! 技能、数据接口、工具调试台、润色预设与输出标准。
//!
//! 布局与设置页相同：左侧分区菜单、右侧当前分区、底部常驻保存条。表单行、
//! 小标题、菜单项这些画法直接复用 `app::settings` 的，两页看起来是一套东西。
//! 网络代理是全局的（起草、复核、知识库都走它），留在设置页。
//!
//! 模型分两层管：「模型服务商管理」只管连接身份（地址、密钥、模型清单缓存，
//! 见 `app::provider_settings`）；「模型服务」按功能（文字起草 / 文字复核 /
//! 知识库检索）各自挑一个提供商的模型，不再碰地址密钥。

use super::provider_settings::{model_picker, provider_ref_chip};
use super::settings::{
    DETAIL_MAX_WIDTH, FOOTER_HEIGHT, MENU_COLUMN_WIDTH, MENU_PANEL_MARGIN, MENU_WIDTH,
    setting_continuation, setting_label, setting_row, settings_menu_item, sub_heading,
};
use crate::app::{GongwenApp, warn};
use crate::models::{ModelKind, RerankMode};
use crate::theme;
use eframe::egui;

/// 润色预设分区的正文最大宽度：列表和编辑区并排，比表单类分区要宽。
const PRESETS_MAX_WIDTH: f32 = 1180.0;
/// 服务商卡片需要同时容纳名称、状态和操作区，比普通设置表单稍宽。
const PROVIDERS_MAX_WIDTH: f32 = 1040.0;

/// AI 管理页的分区。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AiSection {
    /// 服务提供商：接口地址、密钥与模型清单缓存。
    #[default]
    Providers,
    /// 按功能配模型：文字起草、文字复核、知识库检索。
    ModelService,
    /// 技能：SKILL.md 的管理。
    Skills,
    /// 数据接口：内网查询接口。
    DataApis,
    /// 工具调试台。
    ToolConsole,
    /// 写法风格档案（16.15 C）。
    Styles,
    /// 润色预设库（侧栏技能的 `{preset}`）。
    Presets,
    /// 内置输出标准（只读）。
    Contract,
}

/// 左侧菜单的分组。
const MENU_GROUPS: [(&str, &[AiSection]); 3] = [
    ("模型", &[AiSection::Providers, AiSection::ModelService]),
    (
        "智能体",
        &[
            AiSection::Skills,
            AiSection::DataApis,
            AiSection::ToolConsole,
        ],
    ),
    (
        "写法",
        &[AiSection::Styles, AiSection::Presets, AiSection::Contract],
    ),
];

impl AiSection {
    #[cfg(test)]
    const ALL: [AiSection; 8] = [
        AiSection::Providers,
        AiSection::ModelService,
        AiSection::Skills,
        AiSection::DataApis,
        AiSection::ToolConsole,
        AiSection::Styles,
        AiSection::Presets,
        AiSection::Contract,
    ];

    /// 菜单里显示的中文名，同时是右栏的标题。
    pub(crate) fn label(self) -> &'static str {
        match self {
            AiSection::Providers => "模型服务商管理",
            AiSection::ModelService => "模型服务",
            AiSection::Skills => "技能",
            AiSection::DataApis => "数据接口",
            AiSection::ToolConsole => "工具调试台",
            AiSection::Styles => "风格",
            AiSection::Presets => "润色预设",
            AiSection::Contract => "输出标准",
        }
    }

    fn icon(self) -> theme::Icon {
        match self {
            AiSection::Providers => theme::Icon::Globe,
            AiSection::ModelService => theme::Icon::PlugZap,
            AiSection::Skills => theme::Icon::WandSparkles,
            AiSection::DataApis => theme::Icon::Braces,
            AiSection::ToolConsole => theme::Icon::Settings,
            AiSection::Styles => theme::Icon::Quote,
            AiSection::Presets => theme::Icon::Edit,
            AiSection::Contract => theme::Icon::Shield,
        }
    }

    fn description(self) -> &'static str {
        match self {
            AiSection::Providers => {
                "管理连接地址与密钥，在「模型服务」中为起草、复核和知识库选择模型。"
            }
            AiSection::ModelService => {
                "每个功能各自挑一个提供商的模型；地址与密钥到「模型服务商管理」里统一维护。\
                 换服务商不用各处改，在这里重选一次即可。"
            }
            AiSection::Skills => {
                "AI 侧栏按技能做事：流程、提示词和能用的工具都写在 SKILL.md 里。内置技能可以复制一份改，\
                 也可以新建自己的；保存时校验，写坏了不影响使用。"
            }
            AiSection::DataApis => {
                "让技能查内网系统（统计、政策库、业务系统、算法服务）的数据。只做查询，不发任何会改变\
                 对方状态的请求；接口地址与密钥只存在本机，不写进技能文件。"
            }
            AiSection::ToolConsole => {
                "选一个工具、填参数、看输出，写技能时用来试工具。对象是当前稿件的副本，改不到正文。"
            }
            AiSection::Styles => {
                "从指定的几篇稿子学出一份写法风格（基调、开头、结构、常用表达、结尾），以后起草、改写时按文种与场合\
                 自动挑一份带上，也可以在侧栏输入框底栏指定。只学写法不学事实；档案只存本机，可导出给同事。"
            }
            AiSection::Presets => {
                "润色、语气等技能里的 {preset}：选了预设，它的指令就拼进提示词。在侧栏输入框底栏的预设标签里选用。"
            }
            AiSection::Contract => {
                "每次让模型写正文都会自动拼上这段输出标准，并声明优先级更高：预设或技能里的要求与它冲突时，一律以它为准。只读。"
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
            AiSection::Presets => PRESETS_MAX_WIDTH,
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
                            AiSection::ToolConsole => self.tool_console_section_ui(ui),
                            AiSection::Styles => self.styles_section_ui(ui),
                            AiSection::Presets => self.ai_presets_section_ui(ui),
                            AiSection::Contract => self.output_contract_ui(ui),
                        }
                        ui.add_space(8.0);
                    });
            });
    }

    /// 底部常驻保存条：润色预设编辑区没应用的内容、数据接口没保存的修改一并落盘。
    fn ai_footer_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        if self.ai_section == AiSection::Skills {
            ui.weak("技能文件在工作区单独校验与保存；切换文件会保留未保存的修改。");
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

    /// 保存 AI 管理页：先应用润色预设的编辑区（名称不合格就停在那里提示），
    /// 再写配置与数据接口。
    pub(crate) fn save_ai_manage(&mut self) {
        if !self.apply_pending_ai_prompt() {
            self.ai_section = AiSection::Presets;
            return;
        }
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
                // 默认关着的那些要标出来，别让人以为它和默认开的几条一样有把握。
                if crate::revise_model::DEFAULT_DISABLED_TASKS.contains(&task.id) {
                    theme::chip(ui, "默认关闭", warn(), theme::surface());
                }
            });
            setting_continuation(ui, |ui| {
                ui.weak(task.criteria);
            });
        }
        setting_continuation(ui, |ui| {
            ui.weak(
                "标「默认关闭」的项在回归集上还有误报未复测。开之前先跑一遍：\
                 cargo test --bin gongwen-assistant revise_cases -- --ignored --nocapture",
            );
        });
    }

    /// 文字复核的埋点面板：这个检查器到底在帮忙还是在添乱。
    ///
    /// 三个数字要一起看：闸门拦截多说明模型不合用；采纳率低说明这类检查本身
    /// 不受用；两者都低才说明它真的有价值。只报一个数会误导人。
    fn revise_metrics_ui(&mut self, ui: &mut egui::Ui) {
        let mut any = false;
        let mut clear = false;
        let mut copied = false;
        for task in &crate::revise_model::TASKS {
            let stat = self.metrics.get(task.id);
            if stat.decisions() == 0 && stat.gate_rejected == 0 {
                continue;
            }
            any = true;
            ui.horizontal_wrapped(|ui| {
                setting_label(ui, task.label, None);
                match stat.adoption() {
                    Some(rate) if stat.decisions() >= crate::metrics::MIN_SAMPLES => {
                        ui.colored_label(
                            if stat.is_underperforming() {
                                warn()
                            } else {
                                theme::text_soft()
                            },
                            format!(
                                "采纳 {:.0}%（采纳 {} / 忽略 {}）",
                                rate * 100.0,
                                stat.accepted,
                                stat.ignored
                            ),
                        );
                    }
                    _ => {
                        ui.weak(format!(
                            "采纳 {} / 忽略 {}（样本不足，暂不计采纳率）",
                            stat.accepted, stat.ignored
                        ));
                    }
                }
                if stat.gate_rejected > 0 {
                    ui.weak(format!("· 闸门拦下 {} 条", stat.gate_rejected));
                }
                if stat.undone > 0 {
                    ui.weak(format!("· 采纳后撤销 {} 次", stat.undone));
                }
            });
            // 拦截原因的分布才是调阈值的依据。只报总数的话，看不出是阈值太紧
            // 还是提示词让模型话太多。
            let reasons: Vec<String> = crate::revise_model::GateReason::ALL
                .into_iter()
                .map(|reason| (reason, self.metrics.gate_reason_count(task.id, reason)))
                .filter(|(_, count)| *count > 0)
                .map(|(reason, count)| format!("{} {count}", reason.label()))
                .collect();
            if !reasons.is_empty() {
                setting_continuation(ui, |ui| {
                    ui.weak(format!("拦截原因：{}", reasons.join("、")));
                });
            }
            if stat.is_underperforming() {
                setting_continuation(ui, |ui| {
                    ui.colored_label(
                        warn(),
                        "这个检查器的建议多数被划掉，建议关掉——留着只会让人习惯性忽略整个建议面板。",
                    );
                });
            }
        }
        if !any {
            ui.weak("还没有复核记录。跑过几轮、逐条处理过之后，这里会显示采纳率。");
            return;
        }
        setting_continuation(ui, |ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::Copy, "复制统计摘要"))
                .on_hover_text("只有编号和计数，不含任何稿件内容，可以拿出内网讨论怎么调阈值")
                .clicked()
            {
                ui.ctx().copy_text(self.metrics.report());
                copied = true;
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::RotateCcw, "清空统计"))
                .on_hover_text("换了模型或改了提示词之后，旧统计不再可比，清掉重新攒")
                .clicked()
            {
                clear = true;
            }
        });
        if copied {
            self.status = "统计摘要已复制到剪贴板（仅计数，不含稿件内容）。".into();
        }
        if clear {
            self.metrics.clear();
            crate::metrics::save(&mut self.metrics);
            self.status = "检查器统计已清空。".into();
        }
    }
}

/// 「上下文窗口」一行（16.15 A.1）：自动 / 常用档位 / 手填；自动时写明现在按多少算、从哪来。
fn context_window_row(
    ui: &mut egui::Ui,
    salt: &str,
    value: &mut u32,
    auto: crate::lmstudio::context::Window,
) {
    use crate::lmstudio::context::tokens_label;
    const PRESETS: [u32; 6] = [8192, 16_384, 32_768, 65_536, 131_072, 204_800];
    setting_row(
        ui,
        "上下文窗口",
        Some(
            "一次请求里输入加输出最多多少 token。自动：先问服务（点「测试连接」时读到），问不到按模型名估，再不行按 32k；服务端报过超长时按它说的算。",
        ),
        |ui| {
            let text = if *value == 0 {
                format!("自动：{}", auto.label())
            } else {
                tokens_label(*value as usize)
            };
            egui::ComboBox::from_id_salt(salt)
                .selected_text(text)
                .width(240.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(value, 0, "自动");
                    for preset in PRESETS {
                        ui.selectable_value(value, preset, tokens_label(preset as usize));
                    }
                });
            if *value != 0 {
                ui.add(
                    egui::DragValue::new(value)
                        .range(2048..=2_000_000)
                        .speed(256)
                        .suffix(" token"),
                )
                .on_hover_text("也可以直接填");
            }
        },
    );
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

    /// 「模型服务」分区：按功能配模型——文字起草、文字复核、知识库检索。
    /// 地址与密钥不在这里填；模型从「模型服务商管理」缓存的清单里挑，
    /// 选择器按提供商分组、可筛选（与字体选择同一套交互）。
    fn model_service_section_ui(&mut self, ui: &mut egui::Ui) {
        let providers = self.config.providers.clone();

        // ── 文字起草 ────────────────────────────────────────────────
        ui.horizontal(|ui| {
            sub_heading(ui, "文字起草", None);
            let mref = self.config.draft_model.clone();
            provider_ref_chip(ui, &self.config, &mref);
        });
        ui.label(
            egui::RichText::new("AI 侧栏的技能都用这里的模型。")
                .size(theme::font_sizes::SMALL)
                .color(theme::text_muted()),
        );
        setting_row(ui, "模型", None, |ui| {
            let (mut filter, mut show_all) = self.picker_state("draft");
            model_picker(
                ui,
                "draft",
                &providers,
                &mut self.config.draft_model,
                None,
                &["chat"],
                ModelKind::Chat,
                &mut filter,
                &mut show_all,
            );
            self.store_picker_state("draft", filter, show_all);
        });
        setting_row(ui, "备用模型（可不选）", None, |ui| {
            let (mut filter, mut show_all) = self.picker_state("draft_backup");
            model_picker(
                ui,
                "draft_backup",
                &providers,
                &mut self.config.draft_backup_model,
                Some("不配置备用"),
                &["chat"],
                ModelKind::Chat,
                &mut filter,
                &mut show_all,
            );
            self.store_picker_state("draft_backup", filter, show_all);
        });
        setting_row(ui, "温度", None, |ui| {
            ui.add(
                egui::Slider::new(&mut self.config.lm_studio.temperature, 0.0..=1.2).step_by(0.05),
            );
        });
        setting_row(
            ui,
            "最大输出 Token",
            Some("每次请求还会按上下文窗口的剩余空间自动收紧，不会因为输入加输出超过窗口被拒"),
            |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.config.lm_studio.max_tokens).range(256..=32768),
                );
            },
        );
        let auto = {
            let mut probe = self.config.draft_chat().unwrap_or_default();
            probe.context_window = 0;
            crate::lmstudio::context::peek_window(&probe)
        };
        context_window_row(
            ui,
            "draft_context_window",
            &mut self.config.lm_studio.context_window,
            auto,
        );
        setting_row(ui, "超时（秒）", None, |ui| {
            ui.add(
                egui::DragValue::new(&mut self.config.lm_studio.timeout_seconds).range(5..=1800),
            );
        });

        // ── 文字复核 ────────────────────────────────────────────────
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            sub_heading(ui, "文字复核", None);
            let mref = self.config.revise_model.model_ref.clone();
            provider_ref_chip(ui, &self.config, &mref);
        });
        ui.label(
            egui::RichText::new(
                "逐句检查语病，结果进「修订建议」，逐条确认后才改正文。与起草模型分开配：\
                 起草要发挥，复核只要稳——Qwen3 4B/8B 一类的小模型温度 0 反而更好使，也快得多。\
                 复核请求会自动带上关闭思考的开关，服务端不认时会自动去掉重试。",
            )
            .size(theme::font_sizes::SMALL)
            .color(theme::text_muted()),
        );
        ui.checkbox(
            &mut self.config.revise_model.enabled,
            "启用文字复核（起草页「审校」分区出现入口）",
        );
        ui.add_enabled_ui(self.config.revise_model.enabled, |ui| {
            setting_row(ui, "模型", Some("留空 = 沿用起草模型"), |ui| {
                let (mut filter, mut show_all) = self.picker_state("revise");
                model_picker(
                    ui,
                    "revise",
                    &providers,
                    &mut self.config.revise_model.model_ref,
                    Some("沿用起草模型"),
                    &["chat"],
                    ModelKind::Chat,
                    &mut filter,
                    &mut show_all,
                );
                self.store_picker_state("revise", filter, show_all);
            });

            setting_row(ui, "备用模型（可不选）", None, |ui| {
                let (mut filter, mut show_all) = self.picker_state("revise_backup");
                model_picker(
                    ui,
                    "revise_backup",
                    &providers,
                    &mut self.config.revise_model.backup_model_ref,
                    Some("不配置备用"),
                    &["chat"],
                    ModelKind::Chat,
                    &mut filter,
                    &mut show_all,
                );
                self.store_picker_state("revise_backup", filter, show_all);
            });
            sub_heading(ui, "送检范围", None);
            setting_row(ui, "单句字数上限", None, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.config.revise_model.max_sentence_chars)
                        .range(40..=400),
                )
                .on_hover_text("超过这个长度的多半是整段没断句，交给小模型只会跑飞，直接跳过");
            });
            setting_row(ui, "单轮送检句数上限", None, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.config.revise_model.max_sentences)
                        .range(10..=1000),
                )
                .on_hover_text("逐句顺序调用，句数越多等得越久");
            });
            setting_row(ui, "超时（秒）", None, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.config.revise_model.timeout_seconds)
                        .range(5..=600),
                );
            });
            let auto = {
                let mut probe = self.config.revise_chat(false).unwrap_or_default();
                probe.context_window = 0;
                crate::lmstudio::context::peek_window(&probe)
            };
            context_window_row(
                ui,
                "revise_context_window",
                &mut self.config.revise_model.context_window,
                auto,
            );

            sub_heading(ui, "检查器", None);
            self.revise_tasks_ui(ui);

            sub_heading(ui, "采纳统计", None);
            self.revise_metrics_ui(ui);
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
                "用 embedding 与 rerank 模型检索知识库，技能起草时据此取证据。\
                 两个模型与起草对话模型相互独立。",
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

#[cfg(test)]
mod tests {
    use super::*;

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
