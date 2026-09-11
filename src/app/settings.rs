//! 设置页：主题、字体与导出设置。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::settings`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{
    GongwenApp, LABEL_WIDTH, field, row_label_with_info, section_heading_with_info, warn,
};
use crate::models::{
    BoldStyle, FontRole, HeadingNumbering, ListNumbering, PaperMode, RerankMode, ThemeName,
};
use crate::storage;
use crate::system_fonts;
use crate::theme;
use eframe::egui;

/// 一个位置的字体选择行：下拉选本机字体，或浏览一个字体文件。
/// 返回需要显示在状态栏的提示（选了不支持的文件时给出）。
///
/// `key` 只用于下拉框的 id，`label` 是行标题，`default_label` 是未选择时显示的
/// 完整名称（例如“系统默认（微软雅黑）”或“内置（仿宋）”），`hint` 是悬停说明。
#[allow(clippy::too_many_arguments)] // Shared form helper; call sites keep these options explicit.
fn font_choice_row(
    ui: &mut egui::Ui,
    key: &str,
    label: &str,
    default_label: &str,
    hint: &str,
    choice: &mut crate::models::FontChoice,
    available: &[system_fonts::SystemFont],
    filter: &mut String,
) -> Option<String> {
    let mut message = None;
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(label));
        let selected = if choice.is_set() {
            choice.label().to_string()
        } else {
            default_label.to_string()
        };
        egui::ComboBox::from_id_salt(format!("font_role_{key}"))
            .selected_text(selected)
            .width(300.0)
            .show_ui(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(filter)
                        .hint_text("输入字体名筛选")
                        .desired_width(280.0),
                );
                ui.separator();
                if ui
                    .selectable_label(!choice.is_set(), default_label)
                    .clicked()
                {
                    *choice = crate::models::FontChoice::default();
                }
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        let needle = filter.trim().to_lowercase();
                        let mut shown = 0usize;
                        for font in available.iter().filter(|font| {
                            needle.is_empty()
                                || font.display.to_lowercase().contains(&needle)
                                || font.family.to_lowercase().contains(&needle)
                        }) {
                            shown += 1;
                            // 中文名和英文名不一致时两个都显示：写进 TeX 的是英文名。
                            let label = if font.display == font.family {
                                font.family.clone()
                            } else {
                                format!("{}（{}）", font.display, font.family)
                            };
                            let picked = choice.family == font.family;
                            if ui.selectable_label(picked, label).clicked() {
                                *choice = font.to_choice();
                            }
                        }
                        if shown == 0 {
                            ui.weak("没有匹配的字体。");
                        }
                    });
            })
            .response
            .on_hover_text(hint);
        if theme::icon_button(ui, theme::Icon::Folder, "浏览字体文件").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("字体文件", system_fonts::SUPPORTED_EXTENSIONS)
                .pick_file()
        {
            match system_fonts::read_font(&path) {
                Some(font) => *choice = font.to_choice(),
                None => {
                    message = Some(format!(
                        "无法把「{}」用作{label}：只支持 ttf 与 otf，字体集合（ttc）需要额外指定字面序号，暂不支持。",
                        path.display(),
                    ));
                }
            }
        }
        if choice.is_set() && theme::icon_button(ui, theme::Icon::RotateCcw, "恢复默认字体").clicked()
        {
            *choice = crate::models::FontChoice::default();
        }
    });
    if choice.is_set() {
        ui.horizontal(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
            ui.weak(choice.path.clone());
        });
    }
    message
}

impl GongwenApp {
    /// 检查器开关。
    ///
    /// 逐项可关，是因为**一个爱误报的检查器会把整个功能连坐关掉**：用户不会
    /// 去分辨是哪一项在捣乱，只会不再打开这个面板。给了开关，坏的那项可以
    /// 单独摘掉，好的留下。
    fn revise_tasks_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("启用的检查器"));
            ui.weak("每多开一项，一轮复核的调用次数就多一倍");
        });
        for task in &crate::revise_model::TASKS {
            let mut on = self.config.revise_model.task_enabled(task.id);
            ui.horizontal_wrapped(|ui| {
                ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                if ui.checkbox(&mut on, task.label).changed() {
                    self.config.revise_model.set_task_enabled(task.id, on);
                }
                // 默认关着的那些要标出来，别让人以为它和默认开的几条一样有把握。
                if crate::revise_model::DEFAULT_DISABLED_TASKS.contains(&task.id) {
                    theme::chip(ui, "默认关闭", warn(), theme::surface());
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                ui.weak(task.criteria);
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
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
                ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(task.label));
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
                ui.horizontal_wrapped(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                    ui.weak(format!("拦截原因：{}", reasons.join("、")));
                });
            }
            if stat.is_underperforming() {
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
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
        ui.horizontal_wrapped(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
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

    /// 切换主题并立即生效：写入配置、刷新全局样式与窗口图标、保存。
    /// 设置页的主题卡片与应用菜单的「外观主题」子菜单共用这一入口。
    pub(crate) fn apply_theme(&mut self, ctx: &egui::Context, name: ThemeName) {
        self.config.theme = name;
        theme::set_current(name);
        theme::configure_style(ctx);
        theme::apply_app_icon(ctx, name);
        self.status = format!("界面主题已切换为「{}」。", theme::by_name(name).label);
        let _ = storage::save(&self.config);
    }

    /// 切换纸面明暗并立即生效。只影响屏幕预览，导出结果不变。
    pub(crate) fn apply_paper_mode(&mut self, mode: PaperMode) {
        self.config.paper = mode;
        theme::set_current_paper(mode);
        self.status = format!("公文纸面已切换为「{}」。", mode.label());
        let _ = storage::save(&self.config);
    }

    /// 画一张主题预览卡；返回值为卡片的点击响应。
    fn theme_card(&self, ui: &mut egui::Ui, name: ThemeName) -> egui::Response {
        let palette = theme::by_name(name);
        let selected = name == self.config.theme;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(168.0, 76.0), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            // 卡片底与描边：选中时用强调色加粗描边。
            painter.rect_filled(rect, 10.0, palette.surface);
            let stroke = if selected {
                egui::Stroke::new(2.0, palette.accent)
            } else {
                egui::Stroke::new(1.0, palette.border_strong)
            };
            painter.rect_stroke(rect, 10.0, stroke, egui::StrokeKind::Inside);

            // 色板行：画布底、沉底、强调淡底、强调色四枚色块。
            let swatch = 16.0;
            let gap = 6.0;
            let start = egui::pos2(rect.left() + 12.0, rect.top() + 12.0);
            for (index, color) in [
                palette.canvas,
                palette.surface_sunk,
                palette.accent_soft,
                palette.accent,
            ]
            .iter()
            .enumerate()
            {
                let min = start + egui::vec2(index as f32 * (swatch + gap), 0.0);
                let max = min + egui::vec2(swatch, swatch);
                painter.rect_filled(egui::Rect::from_min_max(min, max), 4.0, *color);
            }

            // 主题名与选中标记。卡片自带配色，文字一律用该主题自己的前景色。
            painter.text(
                egui::pos2(rect.left() + 12.0, rect.top() + 42.0),
                egui::Align2::LEFT_TOP,
                palette.label,
                egui::FontId::proportional(14.0),
                palette.text,
            );
            if selected {
                painter.text(
                    egui::pos2(rect.right() - 12.0, rect.top() + 10.0),
                    egui::Align2::RIGHT_TOP,
                    "✓ 当前",
                    egui::FontId::proportional(12.0),
                    palette.accent,
                );
            }
        }
        response
    }

    /// 界面主题：配色预设按明暗分两组多行展示，点选立即生效并保存。
    /// 公文纸面单独一项，且只作用于屏幕——导出永远是白纸黑字红头。
    pub(crate) fn theme_settings_ui(&mut self, ui: &mut egui::Ui) {
        section_heading_with_info(
            ui,
            theme::Icon::Palette,
            "界面主题",
            "界面配色预设，点选后立即生效并保存。深色主题借鉴常见终端配色方案。导出的公文不受影响，仍按规范为白纸黑字红头。",
        );

        let mut pending = None;
        for (title, dark) in [("浅色", false), ("深色", true)] {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(title)
                    .size(theme::font_sizes::SMALL)
                    .color(theme::text_muted()),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for name in ThemeName::ALL
                    .into_iter()
                    .filter(|name| theme::by_name(*name).dark == dark)
                {
                    if self.theme_card(ui, name).clicked() && self.config.theme != name {
                        pending = Some(name);
                    }
                }
            });
        }
        if let Some(name) = pending {
            self.apply_theme(ui.ctx(), name);
        }

        ui.add_space(14.0);
        section_heading_with_info(
            ui,
            theme::Icon::FileTypeDoc,
            "公文纸面",
            "只改屏幕上预览的纸色，方便深色主题下长时间盯屏。导出的 DOCX、TeX 与 PDF 一律仍是白纸黑字红头，不受此项影响。",
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for mode in PaperMode::ALL {
                let selected = mode == self.config.paper;
                if ui
                    .add(theme::menu_selectable_item(selected, mode.label()))
                    .on_hover_text(match mode {
                        PaperMode::Follow => "明色主题配白纸，深色主题配深色纸",
                        PaperMode::Light => "始终白纸黑字，与打印稿一致",
                        PaperMode::Dark => "始终深色纸，长时间盯屏更省眼",
                    })
                    .clicked()
                    && !selected
                {
                    self.apply_paper_mode(mode);
                }
            }
        });
    }

    /// 字体设置：界面字体可个性化，五个编译位置也可分别换成本机字体。
    pub(crate) fn font_settings_ui(&mut self, ui: &mut egui::Ui) {
        // 界面字体和编译字体共用本机字体列表；进入字体设置后只扫描一次。
        if !self.system_fonts_scanned && !self.system_fonts_busy {
            self.start_system_font_scan();
        }
        let before = self.config.fonts.clone();

        section_heading_with_info(
            ui,
            theme::Icon::Type,
            "界面字体",
            "应用窗口、菜单与列表使用的字体。Windows 默认使用微软雅黑，Linux 默认使用 Noto Sans SC；可在此选择其他本机字体，字体文件失效时自动回退系统默认。",
        );
        ui.add_space(4.0);
        let default_ui_label = format!("系统默认（{}）", theme::default_ui_font_label());
        let mut message = {
            let filter = self.font_filter.entry("ui").or_default();
            font_choice_row(
                ui,
                "ui",
                "界面字体",
                &default_ui_label,
                "只影响应用窗口、菜单与列表，不影响公文预览和导出字体",
                &mut self.config.fonts.ui_font,
                &self.system_fonts,
                filter,
            )
        };
        ui.horizontal(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
            if ui
                .add_enabled(
                    !self.system_fonts_busy,
                    theme::icon_text_button(theme::Icon::Refresh, "重新扫描本机字体"),
                )
                .clicked()
            {
                self.start_system_font_scan();
            }
            if self.system_fonts_busy {
                ui.weak("正在扫描…");
            } else {
                ui.weak(format!("已收录 {} 个字体", self.system_fonts.len()));
            }
        });
        ui.add_space(12.0);

        section_heading_with_info(
            ui,
            theme::Icon::Tex,
            "编译字体",
            "默认使用随应用分发的内置字体：标题方正小标宋、一级标题黑体、二级标题楷体、正文仿宋、页码宋体。改用本机字体后，内置 Tectonic 按文件加载所选字体，导出的 TeX 拿到别的机器上编译时按字体名加载。只列出 ttf 与 otf。字体集合（ttc，例如 simsun.ttc）一个文件里装着多个字面，按文件加载必须额外指定序号，内置 Tectonic 上没有验证过，因此不在可选范围内。",
        );
        ui.add_space(4.0);
        ui.checkbox(&mut self.config.fonts.use_system_fonts, "使用本机字体编译")
            .on_hover_text("不勾选时下面的选择仍然保留，只是不生效，方便和内置版式来回对照");

        // 加粗排法不受上面的本机字体开关约束：它决定的是「怎么加粗」，
        // 预览、Word 与 TeX 三端同时生效。
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [LABEL_WIDTH, 20.0],
                egui::Label::new(egui::RichText::new("加粗文字")),
            );
            for style in BoldStyle::ALL {
                ui.selectable_value(&mut self.config.fonts.bold_style, style, style.label())
                    .on_hover_text(style.hint());
            }
        });

        if self.config.fonts.use_system_fonts {
            ui.add_space(4.0);
            for role in FontRole::ALL {
                let filter = self.font_filter.entry(role.key()).or_default();
                let default_label = format!("内置（{}）", role.bundled_label());
                if let Some(text) = font_choice_row(
                    ui,
                    role.key(),
                    role.label(),
                    &default_label,
                    role.hint(),
                    self.config.fonts.choice_mut(role),
                    &self.system_fonts,
                    filter,
                ) {
                    message = Some(text);
                }
            }
        }

        if let Some(text) = message {
            self.status = text;
        }
        if self.config.fonts != before {
            // 预览也跟着换，否则屏幕上的版式和编译出来的 PDF 对不上。
            theme::configure_fonts(ui.ctx(), &self.config.fonts);
        }
    }

    /// 标题与列表编号：各级标题与一、二级列表分别选择编号样式。
    ///
    /// 选择立即写入配置并在保存时生效；预览、实时排版编辑器与导出的 TeX/PDF、
    /// Word 共用同一套选择，保证所见即所得。
    pub(crate) fn numbering_settings_ui(&mut self, ui: &mut egui::Ui) {
        section_heading_with_info(
            ui,
            theme::Icon::ListOrdered,
            "标题与列表编号",
            "设置公文各级标题与列表项的编号样式。导出的 TeX/PDF、Word 与界面预览、实时排版编辑器统一使用，保证预览所见即导出所得。",
        );
        ui.add_space(4.0);
        ui.label(egui::RichText::new("标题编号").strong());
        for (key, label, style) in [
            (
                "heading1",
                "一级标题（##）",
                &mut self.config.numbering.heading1,
            ),
            (
                "heading2",
                "二级标题（###）",
                &mut self.config.numbering.heading2,
            ),
            (
                "heading3",
                "三级标题（####）",
                &mut self.config.numbering.heading3,
            ),
            (
                "heading4",
                "四级标题（#####）",
                &mut self.config.numbering.heading4,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(label));
                egui::ComboBox::from_id_salt(format!("heading_numbering_{key}"))
                    .selected_text(style.label())
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for option in HeadingNumbering::ALL {
                            ui.selectable_value(style, option, option.label());
                        }
                    });
            });
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new("列表编号").strong());
        for (key, label, style) in [
            (
                "list1",
                "一级列表（段内）",
                &mut self.config.numbering.list1,
            ),
            (
                "list2",
                "二级列表（独立）",
                &mut self.config.numbering.list2,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(label));
                egui::ComboBox::from_id_salt(format!("list_numbering_{key}"))
                    .selected_text(style.label())
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for option in ListNumbering::ALL {
                            ui.selectable_value(style, option, option.label());
                        }
                    });
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
            ui.weak("会议议程的事项编号固定为“1. 2. 3.”，不受列表编号设置影响。");
        });
    }

    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("settings_scroll")
            .show(ui, |ui| {
                ui.add_space(4.0);
                section_heading_with_info(
                    ui,
                    theme::Icon::PlugZap,
                    "本地模型服务设置",
                    "应用调用本机 OpenAI 兼容接口，如 LM Studio（http://127.0.0.1:1234/v1）或 Ollama（http://127.0.0.1:11434/v1）。正文不会主动发送到互联网。",
                );
                ui.add_space(8.0);
                field(
                    ui,
                    "接口地址",
                    &mut self.config.lm_studio.base_url,
                    "包含 /v1",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("模型"));
                    if self.models.is_empty() {
                        ui.text_edit_singleline(&mut self.config.lm_studio.model);
                    } else {
                        egui::ComboBox::from_id_salt("model_selector")
                            .selected_text(if self.config.lm_studio.model.is_empty() {
                                "请选择模型"
                            } else {
                                &self.config.lm_studio.model
                            })
                            .width(420.0)
                            .show_ui(ui, |ui| {
                                for model in &self.models {
                                    ui.selectable_value(
                                        &mut self.config.lm_studio.model,
                                        model.clone(),
                                        model,
                                    );
                                }
                            });
                    }
                    if ui
                        .add_enabled(
                            !self.busy,
                            theme::icon_text_button(
                                theme::Icon::PlugZap,
                                "测试连接 / 刷新模型",
                            ),
                        )
                        .clicked()
                    {
                        self.start_model_probe();
                    }
                });
                field(
                    ui,
                    "API Key",
                    &mut self.config.lm_studio.api_key,
                    "本地服务通常可留空",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("温度"));
                    ui.add(
                        egui::Slider::new(&mut self.config.lm_studio.temperature, 0.0..=1.2)
                            .step_by(0.05),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("最大输出 Token"));
                    ui.add(
                        egui::DragValue::new(&mut self.config.lm_studio.max_tokens)
                            .range(256..=32768),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("超时（秒）"));
                    ui.add(
                        egui::DragValue::new(&mut self.config.lm_studio.timeout_seconds)
                            .range(5..=1800),
                    );
                });

                ui.add_space(12.0);
                ui.separator();
                section_heading_with_info(
                    ui,
                    theme::Icon::Sparkles,
                    "AI 文字复核（小模型逐句检查）",
                    "逐句检查语病，结果进「修订建议」，逐条确认后才改正文。与上面的起草模型分开配：起草要发挥，复核只要稳——Qwen3 4B/8B 一类的小模型温度 0 反而更好使，也快得多。地址和密钥留空表示沿用起草模型的。\n复核请求会自动带上关闭思考的开关（Qwen3 一类的模型开着 thinking 会把输出预算全花在推理上），服务端不认时会自动去掉重试；若报错提示只拿到思考过程，再到服务端关闭或改用非思考模型。",
                );
                ui.add_space(8.0);
                ui.checkbox(
                    &mut self.config.revise_model.enabled,
                    "启用文字复核（起草页「审校」分区出现入口）",
                );
                ui.add_enabled_ui(self.config.revise_model.enabled, |ui| {
                    field(
                        ui,
                        "接口地址",
                        &mut self.config.revise_model.base_url,
                        "留空沿用起草模型的地址",
                    );
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("模型"));
                        if self.models.is_empty() {
                            ui.text_edit_singleline(&mut self.config.revise_model.model);
                        } else {
                            egui::ComboBox::from_id_salt("revise_model_selector")
                                .selected_text(if self.config.revise_model.model.is_empty() {
                                    "请选择模型"
                                } else {
                                    &self.config.revise_model.model
                                })
                                .width(420.0)
                                .show_ui(ui, |ui| {
                                    for model in &self.models {
                                        ui.selectable_value(
                                            &mut self.config.revise_model.model,
                                            model.clone(),
                                            model,
                                        );
                                    }
                                });
                        }
                    });
                    field(
                        ui,
                        "API Key",
                        &mut self.config.revise_model.api_key,
                        "留空沿用起草模型的密钥",
                    );
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("单句字数上限"));
                        ui.add(
                            egui::DragValue::new(
                                &mut self.config.revise_model.max_sentence_chars,
                            )
                            .range(40..=400),
                        )
                        .on_hover_text("超过这个长度的多半是整段没断句，交给小模型只会跑飞，直接跳过");
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("单轮送检句数上限"));
                        ui.add(
                            egui::DragValue::new(&mut self.config.revise_model.max_sentences)
                                .range(10..=1000),
                        )
                        .on_hover_text("逐句顺序调用，句数越多等得越久");
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("超时（秒）"));
                        ui.add(
                            egui::DragValue::new(&mut self.config.revise_model.timeout_seconds)
                                .range(5..=600),
                        );
                    });
                    ui.add_space(8.0);
                    self.revise_tasks_ui(ui);
                    ui.add_space(8.0);
                    self.revise_metrics_ui(ui);
                });

                ui.add_space(12.0);
                ui.separator();
                section_heading_with_info(
                    ui,
                    theme::Icon::Library,
                    "知识库（检索增强起草）",
                    "用本地模型服务（LM Studio / Ollama 等 OpenAI 兼容服务）的 embedding 与 rerank 模型检索历史公文，起草时调出相似稿件作参考。两个模型与上面的对话模型相互独立。",
                );
                ui.add_space(4.0);
                ui.checkbox(&mut self.config.rag.enabled, "启用知识库检索增强")
                    .on_hover_text("关闭后，起草页的“参考知识库”开关不生效");
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Embedding 模型").strong());
                field(
                    ui,
                    "接口地址",
                    &mut self.config.rag.embedding.base_url,
                    "包含 /v1",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("模型"));
                    if self.embedding_models.is_empty() {
                        ui.text_edit_singleline(&mut self.config.rag.embedding.model)
                            .on_hover_text("可手填模型名，或点右侧按钮从服务读取");
                    } else {
                        egui::ComboBox::from_id_salt("embedding_model_selector")
                            .selected_text(if self.config.rag.embedding.model.is_empty() {
                                "请选择模型"
                            } else {
                                &self.config.rag.embedding.model
                            })
                            .width(360.0)
                            .show_ui(ui, |ui| {
                                for model in &self.embedding_models {
                                    ui.selectable_value(
                                        &mut self.config.rag.embedding.model,
                                        model.clone(),
                                        model,
                                    );
                                }
                            });
                    }
                    if ui
                        .add_enabled(
                            !self.embedding_probe_busy,
                            theme::icon_text_button(theme::Icon::PlugZap, "测试连接 / 刷新模型"),
                        )
                        .clicked()
                    {
                        self.start_embedding_probe();
                    }
                });
                field(
                    ui,
                    "API Key",
                    &mut self.config.rag.embedding.api_key,
                    "本地服务通常可留空",
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("重排（可选，用于精排检索结果）").strong())
                    .on_hover_text(
                        "rerank 响应字段名等进阶项可在 config.json 的 rag.rerank 节调整，适配不同服务。",
                    );
                ui.horizontal(|ui| {
                    row_label_with_info(
                        ui,
                        "重排方式",
                        match self.config.rag.rerank.mode {
                            RerankMode::None => "直接按混合召回的融合分取前 N 条。够用，只是排序不如重排精准。",
                            RerankMode::Api => "需要能提供 rerank 接口的服务（Jina / Cohere / TEI / Infinity 等）。注意：LM Studio 与 Ollama 目前均不提供该专用接口。",
                            RerankMode::Llm => "复用上面的对话模型给候选片段打分，不必另起服务。代价是每次检索多一次模型调用（低温短输出，通常几秒）。",
                        },
                    );
                    egui::ComboBox::from_id_salt("rerank_mode_selector")
                        .selected_text(self.config.rag.rerank.mode.label())
                        .width(360.0)
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
                    field(
                        ui,
                        "接口地址",
                        &mut self.config.rag.rerank.base_url,
                        "包含 /v1",
                    );
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("端点路径"));
                        ui.text_edit_singleline(&mut self.config.rag.rerank.path)
                            .on_hover_text("拼在接口地址后，默认 rerank；不同服务路径可能不同");
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("模型"));
                        if self.rerank_models.is_empty() {
                            ui.text_edit_singleline(&mut self.config.rag.rerank.model)
                                .on_hover_text("留空则跳过重排；可手填或点右侧按钮从服务读取");
                        } else {
                            egui::ComboBox::from_id_salt("rerank_model_selector")
                                .selected_text(if self.config.rag.rerank.model.is_empty() {
                                    "请选择（留空跳过重排）"
                                } else {
                                    &self.config.rag.rerank.model
                                })
                                .width(360.0)
                                .show_ui(ui, |ui| {
                                    // 允许清空：rerank 可选。
                                    if ui.selectable_label(self.config.rag.rerank.model.is_empty(), "（不使用）").clicked() {
                                        self.config.rag.rerank.model = String::new();
                                    }
                                    for model in &self.rerank_models {
                                        ui.selectable_value(
                                            &mut self.config.rag.rerank.model,
                                            model.clone(),
                                            model,
                                        );
                                    }
                                });
                        }
                        if ui
                            .add_enabled(
                                !self.rerank_probe_busy,
                                theme::icon_text_button(theme::Icon::PlugZap, "测试连接 / 刷新模型"),
                            )
                            .clicked()
                        {
                            self.start_rerank_probe();
                        }
                    });
                }
                if self.config.rag.rerank.mode == RerankMode::Api {
                    field(
                        ui,
                        "API Key",
                        &mut self.config.rag.rerank.api_key,
                        "本地服务通常可留空",
                    );
                }
                if self.config.rag.rerank.mode != RerankMode::None {
                    ui.horizontal(|ui| {
                        ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                        if ui
                            .add_enabled(
                                !self.rerank_probe_busy,
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
                        ui.horizontal_wrapped(|ui| {
                            ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new(""));
                            ui.colored_label(if ok { theme::accent() } else { warn() }, message);
                        });
                    }
                }
                ui.add_space(12.0);
                ui.separator();
                section_heading_with_info(
                    ui,
                    theme::Icon::Folder,
                    "输出与录入",
                    "导出 TeX 时会自动检测 XeLaTeX 或 Tectonic；检测到后编译 PDF 并清理中间文件。",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("输出目录"));
                    ui.text_edit_singleline(&mut self.config.output_dir);
                    if theme::icon_button(ui, theme::Icon::Folder, "选择输出目录").clicked()
                        && let Some(path) = rfd::FileDialog::new().pick_folder()
                    {
                        self.config.output_dir = path.display().to_string();
                    }
                    if theme::icon_button(ui, theme::Icon::Open, "打开输出目录").clicked() {
                        self.draft_page().open_output_dir();
                    }
                });
                ui.checkbox(
                    &mut self.config.allow_free_text,
                    "允许在标准词库之外手工填写单位、联系人等字段",
                )
                .on_hover_text("取消勾选后，起草页这些字段只能从词库中选，杜绝临时手写造成的名称错误");
                ui.checkbox(
                    &mut self.config.show_editor_line_numbers,
                    "Markdown 源码与实时排版模式显示行号",
                )
                .on_hover_text("行号只用于定位，不会写入稿件或导出文件");
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("编辑器字号"));
                    ui.add(
                        egui::DragValue::new(&mut self.config.editor_font_size)
                            .range(
                                crate::models::EDITOR_FONT_SIZE_MIN
                                    ..=crate::models::EDITOR_FONT_SIZE_MAX,
                            )
                            .speed(0.1)
                            .suffix(" px"),
                    )
                    .on_hover_text(
                        "Markdown 源码编辑器的字号；编辑器里也可用 Ctrl+滚轮或 Ctrl± 调整，Ctrl+0 复位",
                    );
                });

                ui.add_space(12.0);
                ui.separator();
                self.theme_settings_ui(ui);

                ui.add_space(12.0);
                ui.separator();
                self.font_settings_ui(ui);

                ui.add_space(12.0);
                ui.separator();
                self.numbering_settings_ui(ui);

                ui.add_space(12.0);
                ui.separator();
                section_heading_with_info(
                    ui,
                    theme::Icon::FileDown,
                    "导出格式",
                    "这里的选择对所有稿件生效；起草页的“导出”按钮按这里勾选的格式产出。",
                );
                // Word 导出尚未达到当前 LaTeX 链路的成熟度，入口保留但暂不允许启用。
                self.config.export.docx = false;
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_WIDTH, 20.0], egui::Label::new("格式"));
                    ui.checkbox(&mut self.config.export.markdown, "Markdown");
                    ui.add_enabled(false, egui::Checkbox::new(&mut self.config.export.docx, "Word"))
                        .on_disabled_hover_text("Word 导出仍在完善，当前请使用 LaTeX/PDF");
                    ui.checkbox(&mut self.config.export.tex, "LaTeX");
                });
                if !self.config.export.any() {
                    ui.colored_label(warn(), "未勾选任何导出格式，起草页的导出按钮不会产生文件。");
                }
                ui.checkbox(&mut self.config.auto_export, "AI 起草或优化完成后自动导出一次")
                    .on_hover_text("不勾选则只出稿，什么时候导出完全由你决定");
                ui.checkbox(&mut self.config.export.overwrite, "覆盖同名文件")
                    .on_hover_text("不勾选时每次导出都会生成“标题-2、标题-3”这样的新文件");

                ui.add_space(12.0);
                ui.separator();
                section_heading_with_info(
                    ui,
                    theme::Icon::Save,
                    "保存与现场",
                    "新建的稿件在第一次真正改动时自动入库；下次启动会恢复本次打开的标签。",
                );
                ui.checkbox(&mut self.config.auto_save, "自动保存到稿件库")
                    .on_hover_text(
                        "每 2 分钟以及切换标签、关闭窗口前，把改动静默写回稿件库。
自动保存不会提交版本——版本链什么时候留痕，始终由你决定。",
                    );

                ui.add_space(12.0);
                ui.separator();
                section_heading_with_info(
                    ui,
                    theme::Icon::Shield,
                    "密级与保密期限规则",
                    "默认取自《保守国家秘密法》第十五条：绝密级不超过三十年、机密级不超过二十年、秘密级不超过十年。本单位口径不同的，直接改下面三个上限。",
                );
                egui::Grid::new("security_rules_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("秘密级上限（年）");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.config.security_rules.secret_max_years,
                            )
                            .range(1..=100),
                        );
                        ui.end_row();
                        ui.label("机密级上限（年）");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.config.security_rules.confidential_max_years,
                            )
                            .range(1..=100),
                        );
                        ui.end_row();
                        ui.label("绝密级上限（年）");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.config.security_rules.top_secret_max_years,
                            )
                            .range(1..=100),
                        );
                        ui.end_row();
                    });
                ui.checkbox(
                    &mut self.config.security_rules.allow_long_term,
                    "期限无法确定时允许标注“长期”",
                );

                ui.add_space(12.0);
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存设置").clicked() {
                    self.persist();
                }
                ui.separator();
                section_heading_with_info(
                    ui,
                    theme::Icon::Sparkles,
                    "建议流程",
                    "1. 在本地模型服务中加载中文指令模型并启动服务（LM Studio 启动 Local Server；Ollama 执行 ollama serve）。\n2. 刷新模型并选择模型。\n3. 在“标准词库”维护全称、常见错写和联系人电话。\n4. 为每类模板保存默认单位、联系人和呈报领导。\n5. 生成草稿 → 在右侧改稿 → 处理审校提示 → 导出签发稿。",
                );
                ui.add_space(8.0);
            });
    }
}
