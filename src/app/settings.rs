//! 设置页：主题、字体与导出设置。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::settings`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::storage;
use crate::system_fonts;
use crate::theme;
use crate::models::{FontRole, RerankMode, ThemeName};
use eframe::egui;
use crate::app::{warn, GongwenApp, LABEL_WIDTH, field, row_label_with_info, section_heading_with_info};

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

    /// 界面主题：明色配色预设，点选立即生效并保存。公文纸面（预览、导出）按
    /// 红头文件规范固定为白纸黑字，不随主题变化。
    pub(crate) fn theme_settings_ui(&mut self, ui: &mut egui::Ui) {
        section_heading_with_info(
            ui,
            theme::Icon::Palette,
            "界面主题",
            "界面的明色配色预设，点选后立即生效并保存。公文纸面（预览与导出）不受影响，仍按规范为白纸黑字红头。",
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for name in ThemeName::ALL {
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

                    // 主题名与选中标记。
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
                if response.clicked() && self.config.theme != name {
                    self.apply_theme(ui.ctx(), name);
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

                ui.add_space(12.0);
                ui.separator();
                self.theme_settings_ui(ui);

                ui.add_space(12.0);
                ui.separator();
                self.font_settings_ui(ui);

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
