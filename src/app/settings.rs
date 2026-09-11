//! 设置页：左侧分区主菜单 + 右侧当前分区，平板式的两栏布局。
//!
//! 分区的名字、图标与说明都挂在 [`SettingsSection`] 上，菜单项和右栏标题共用
//! 同一份；新增一个分区只要加一个枚举成员、在 `MENU_GROUPS` 里归组，
//! 再在 `settings_detail_ui` 的 match 里接上画法。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::settings`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{GongwenApp, warn};
use crate::models::{
    BoldStyle, FontRole, HeadingNumbering, ListNumbering, PaperMode, RerankMode, ThemeName,
};
use crate::storage;
use crate::system_fonts;
use crate::theme;
use eframe::egui;

/// 左栏主菜单的宽度。容得下最长的分区名「标题与列表编号」加图标和选中底色的
/// 内边距，再宽就只是白占正文的地方。
const MENU_WIDTH: f32 = 188.0;
/// 左栏连同沉底面板左右内边距一起占掉的宽度。
const MENU_COLUMN_WIDTH: f32 = MENU_WIDTH + 2.0 * MENU_PANEL_MARGIN as f32;
/// 左栏沉底面板的左右内边距。
const MENU_PANEL_MARGIN: i8 = 6;
/// 右栏正文的最大宽度。设置项都是「标签 + 控件」的窄表单，铺满超宽屏只会让
/// 标签和控件隔着半个屏幕，反而更难读。
const DETAIL_MAX_WIDTH: f32 = 760.0;
/// 底部操作条的高度。两栏先把这块高度让出来，「保存设置」才不会被内容顶出视区。
const FOOTER_HEIGHT: f32 = 52.0;
/// 设置页表单行的标签列宽。比起草页的窄标签宽，容得下「单轮送检句数上限」这类
/// 完整的设置项名称，不再被截断成半截。
const SETTING_LABEL_WIDTH: f32 = 132.0;
/// 表单行控件列的高度，和标签一起决定一行的基线。
const SETTING_ROW_HEIGHT: f32 = 22.0;

/// 设置页的分区：左侧主菜单的一项对应右侧一屏设置项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SettingsSection {
    /// 本地模型服务（起草模型的 OpenAI 兼容接口）。
    #[default]
    ModelService,
    /// AI 文字复核（小模型逐句检查）。
    ReviseModel,
    /// 知识库（检索增强起草）。
    Knowledge,
    /// 输出与录入（输出目录、编辑器选项）。
    Output,
    /// 界面主题与公文纸面。
    Theme,
    /// 界面字体与编译字体。
    Font,
    /// 标题与列表编号样式。
    Numbering,
    /// 导出格式。
    Export,
    /// 保存与现场。
    Persistence,
    /// 密级与保密期限规则。
    Security,
    /// 上手指引。
    Guide,
}

/// 左侧主菜单的分组。分组名只是小字标题，不可点击——十几项平铺成一列时，
/// 找一项要从头扫到尾；按「配什么」分成几摞，眼睛一次只在一摞里找。
const MENU_GROUPS: [(&str, &[SettingsSection]); 5] = [
    (
        "智能助手",
        &[
            SettingsSection::ModelService,
            SettingsSection::ReviseModel,
            SettingsSection::Knowledge,
        ],
    ),
    ("外观", &[SettingsSection::Theme, SettingsSection::Font]),
    (
        "公文排版",
        &[SettingsSection::Numbering, SettingsSection::Security],
    ),
    (
        "文件与现场",
        &[
            SettingsSection::Output,
            SettingsSection::Export,
            SettingsSection::Persistence,
        ],
    ),
    ("帮助", &[SettingsSection::Guide]),
];

impl SettingsSection {
    /// 左侧主菜单里显示的中文名，同时是右栏的标题。
    pub(crate) fn label(self) -> &'static str {
        match self {
            SettingsSection::ModelService => "本地模型服务",
            SettingsSection::ReviseModel => "AI 文字复核",
            SettingsSection::Knowledge => "知识库",
            SettingsSection::Output => "输出与录入",
            SettingsSection::Theme => "界面主题",
            SettingsSection::Font => "字体",
            SettingsSection::Numbering => "标题与列表编号",
            SettingsSection::Export => "导出格式",
            SettingsSection::Persistence => "保存与现场",
            SettingsSection::Security => "密级规则",
            SettingsSection::Guide => "上手指引",
        }
    }

    /// 菜单项与右栏标题共用的图标。
    fn icon(self) -> theme::Icon {
        match self {
            SettingsSection::ModelService => theme::Icon::PlugZap,
            SettingsSection::ReviseModel => theme::Icon::Sparkles,
            SettingsSection::Knowledge => theme::Icon::Library,
            SettingsSection::Output => theme::Icon::Folder,
            SettingsSection::Theme => theme::Icon::Palette,
            SettingsSection::Font => theme::Icon::Type,
            SettingsSection::Numbering => theme::Icon::ListOrdered,
            SettingsSection::Export => theme::Icon::FileDown,
            SettingsSection::Persistence => theme::Icon::Save,
            SettingsSection::Security => theme::Icon::Shield,
            SettingsSection::Guide => theme::Icon::Book,
        }
    }

    /// 右栏标题下的说明。原先这些话只挂在标题的悬停提示里——没人会去悬停一个
    /// 看起来没有交互的标题，等于没写；这里改成常显的小字。
    fn description(self) -> &'static str {
        match self {
            SettingsSection::ModelService => {
                "应用调用本机 OpenAI 兼容接口，如 LM Studio（http://127.0.0.1:1234/v1）或 \
                 Ollama（http://127.0.0.1:11434/v1）。正文不会主动发送到互联网。"
            }
            SettingsSection::ReviseModel => {
                "逐句检查语病，结果进「修订建议」，逐条确认后才改正文。与起草模型分开配：\
                 起草要发挥，复核只要稳——Qwen3 4B/8B 一类的小模型温度 0 反而更好使，也快得多。\
                 地址和密钥留空表示沿用起草模型的。"
            }
            SettingsSection::Knowledge => {
                "用本地模型服务的 embedding 与 rerank 模型检索历史公文，起草时调出相似稿件作参考。\
                 两个模型与起草对话模型相互独立。"
            }
            SettingsSection::Output => {
                "导出 TeX 时会自动检测 XeLaTeX 或 Tectonic；检测到后编译 PDF 并清理中间文件。"
            }
            SettingsSection::Theme => {
                "界面配色与屏幕上的纸色，点选后立即生效并保存。导出的公文不受影响，\
                 仍按规范为白纸黑字红头。"
            }
            SettingsSection::Font => "界面、Markdown 编辑器与公文编译三处字体分别设置，互不牵连。",
            SettingsSection::Numbering => {
                "各级标题与列表项的编号样式。导出的 TeX/PDF、Word 与界面预览、实时排版编辑器\
                 统一使用，保证预览所见即导出所得。"
            }
            SettingsSection::Export => {
                "这里的选择对所有稿件生效；起草页的「导出」按钮按这里勾选的格式产出。"
            }
            SettingsSection::Persistence => {
                "新建的稿件在第一次真正改动时自动入库；下次启动会恢复本次打开的标签。"
            }
            SettingsSection::Security => {
                "默认取自《保守国家秘密法》第十五条：绝密级不超过三十年、机密级不超过二十年、\
                 秘密级不超过十年。本单位口径不同的，直接改下面三个上限。"
            }
            SettingsSection::Guide => "第一次用这个程序，按下面的顺序走一遍就能出第一份稿子。",
        }
    }
}

/// 设置页表单行的标签。固定列宽让同一屏里的控件左边缘对齐；说明挂在标签的
/// 悬停提示上，不额外占行。
fn setting_label(ui: &mut egui::Ui, label: &str, tip: Option<&str>) {
    let response = ui.add_sized(
        [SETTING_LABEL_WIDTH, SETTING_ROW_HEIGHT],
        egui::Label::new(label).wrap_mode(egui::TextWrapMode::Extend),
    );
    if let Some(tip) = tip {
        response.on_hover_text(tip.to_owned());
    }
}

/// 一行设置项：左列标签，右列控件。
fn setting_row<R>(
    ui: &mut egui::Ui,
    label: &str,
    tip: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        setting_label(ui, label, tip);
        add(ui)
    })
    .inner
}

/// 续行：左列留白，右列接着排。用于挂在某一设置项下面的说明、按钮和统计。
///
/// 内容排在一个**限定了宽度的子 Ui** 里，而不是直接 `horizontal_wrapped` 加空格：
/// 后者换行后会退回整行的左边缘，长说明的第二行就跑到标签列底下，和上一行对不齐。
fn setting_continuation<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let indent = SETTING_LABEL_WIDTH + ui.spacing().item_spacing.x;
    let width = (ui.available_width() - indent).max(160.0);
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
            add,
        )
        .inner
    })
    .inner
}

/// 一行文本输入设置项。
fn setting_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    setting_row(ui, label, None, |ui| {
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
    });
}

/// 分区内的小标题。比右栏大标题低一级，用来把一屏里的设置项再分成几摞；
/// 带 `tip` 时补充说明挂在标题的悬停提示上。
fn sub_heading(ui: &mut egui::Ui, text: &str, tip: Option<&str>) {
    ui.add_space(14.0);
    let response = ui.label(egui::RichText::new(text).strong().color(theme::text_soft()));
    if let Some(tip) = tip {
        response.on_hover_text(tip.to_owned());
    }
    ui.add_space(6.0);
}

/// 左栏的一个分区条目。整行可点：平板上的设置菜单点哪儿都能进，
/// 只让文字那几个像素可点是桌面端才有的坏习惯。
fn settings_menu_item(
    ui: &mut egui::Ui,
    selected: bool,
    section: SettingsSection,
) -> egui::Response {
    let width = ui.available_width();
    ui.add(
        egui::Button::new((
            section
                .icon()
                .image()
                .fit_to_exact_size(egui::vec2(16.0, 16.0)),
            section.label(),
            // 末尾放一个可伸张的空原子，图标和文字才会靠左，不会被居中到行中间。
            egui::Atom::grow(),
        ))
        .image_tint_follows_text_color(true)
        .selected(selected)
        .frame_when_inactive(selected)
        .corner_radius(egui::CornerRadius::same(7))
        .min_size(egui::vec2(width, 30.0)),
    )
}

/// 上手指引分区。原先这段话跟着「保存设置」钉在每一屏的底部，无论在配模型还是
/// 调字号都要滚过一遍；它只在头一次用的时候有用，所以单独成一项。
fn guide_section_ui(ui: &mut egui::Ui) {
    for (index, step) in [
        "在「本地模型服务」里填好接口地址——LM Studio 先启动 Local Server，Ollama 先执行 ollama serve。",
        "点「测试连接 / 刷新模型」，从下拉里选一个中文指令模型。",
        "在「标准词库」里维护单位全称、常见错写和联系人电话。",
        "为每类模板保存默认的发文单位、联系人和呈报领导。",
        "生成草稿 → 在右侧改稿 → 处理审校提示 → 导出签发稿。",
    ]
    .iter()
    .enumerate()
    {
        ui.horizontal_top(|ui| {
            ui.add_sized(
                [22.0, SETTING_ROW_HEIGHT],
                egui::Label::new(
                    egui::RichText::new(format!("{}.", index + 1)).color(theme::accent()),
                ),
            );
            ui.label(egui::RichText::new(*step).color(theme::text_soft()));
        });
        ui.add_space(4.0);
    }
}

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
        setting_label(ui, label, None);
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
        setting_continuation(ui, |ui| {
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
        sub_heading(
            ui,
            "配色预设",
            Some("深色主题借鉴常见终端配色方案。点选后立即生效并保存。"),
        );

        let mut pending = None;
        for (title, dark) in [("浅色", false), ("深色", true)] {
            ui.add_space(4.0);
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

        sub_heading(
            ui,
            "公文纸面",
            Some(
                "只改屏幕上预览的纸色，方便深色主题下长时间盯屏。导出的 DOCX、TeX 与 PDF 一律仍是白纸黑字红头，不受此项影响。",
            ),
        );
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

    /// 字体设置：界面与 Markdown 源码编辑器字体可分别个性化，五个编译位置也可分别换成本机字体。
    pub(crate) fn font_settings_ui(&mut self, ui: &mut egui::Ui) {
        // 界面字体和编译字体共用本机字体列表；进入字体设置后只扫描一次。
        if !self.system_fonts_scanned && !self.system_fonts_busy {
            self.start_system_font_scan();
        }
        let before = self.config.fonts.clone();

        sub_heading(
            ui,
            "界面与编辑器",
            Some(
                "应用窗口、菜单与列表使用的字体。Windows 默认使用微软雅黑，Linux 默认使用 Noto Sans SC；可在此选择其他本机字体，字体文件失效时自动回退系统默认。",
            ),
        );
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
        // Markdown 源码编辑器单独一项：留空即跟随界面字体。
        if let Some(text) = {
            let filter = self.font_filter.entry("editor").or_default();
            font_choice_row(
                ui,
                "editor",
                "编辑器字体",
                "跟随界面字体",
                "只影响 Markdown 源码编辑模式的字体，不影响界面、公文预览和导出字体",
                &mut self.config.fonts.editor_font,
                &self.system_fonts,
                filter,
            )
        } {
            message = Some(text);
        }
        // 字号紧挨着字体：同一件事（编辑器看着舒不舒服）不该拆到两个分区里找。
        setting_row(ui, "编辑器字号", None, |ui| {
            ui.add(
                egui::DragValue::new(&mut self.config.editor_font_size)
                    .range(
                        crate::models::EDITOR_FONT_SIZE_MIN..=crate::models::EDITOR_FONT_SIZE_MAX,
                    )
                    .speed(0.1)
                    .suffix(" px"),
            )
            .on_hover_text(
                "Markdown 源码编辑器的字号；编辑器里也可用 Ctrl+滚轮或 Ctrl± 调整，Ctrl+0 复位",
            );
        });
        setting_continuation(ui, |ui| {
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

        sub_heading(
            ui,
            "公文编译字体",
            Some(
                "默认使用随应用分发的内置字体：标题方正小标宋、一级标题黑体、二级标题楷体、正文仿宋、页码宋体。改用本机字体后，内置 Tectonic 按文件加载所选字体，导出的 TeX 拿到别的机器上编译时按字体名加载。只列出 ttf 与 otf。字体集合（ttc，例如 simsun.ttc）一个文件里装着多个字面，按文件加载必须额外指定序号，内置 Tectonic 上没有验证过，因此不在可选范围内。",
            ),
        );
        ui.checkbox(&mut self.config.fonts.use_system_fonts, "使用本机字体编译")
            .on_hover_text("不勾选时下面的选择仍然保留，只是不生效，方便和内置版式来回对照");

        // 加粗排法不受上面的本机字体开关约束：它决定的是「怎么加粗」，
        // 预览、Word 与 TeX 三端同时生效。
        ui.add_space(8.0);
        setting_row(ui, "加粗文字", None, |ui| {
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
        sub_heading(ui, "标题编号", None);
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
            setting_row(ui, label, None, |ui| {
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

        sub_heading(ui, "列表编号", None);
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
            setting_row(ui, label, None, |ui| {
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
        setting_continuation(ui, |ui| {
            ui.weak("会议议程的事项编号固定为“1. 2. 3.”，不受列表编号设置影响。");
        });
    }

    /// 设置页：左侧分区主菜单 + 右侧当前分区内容的平板式两栏布局，
    /// 底部一条常驻操作条。
    ///
    /// 两栏的高度先扣掉操作条再分配，是因为**两个滚动区会把父容器吃光**：
    /// 右栏内容一长，`ScrollArea` 就把可用高度全占了，「保存设置」被顶到视区
    /// 之外，改完设置反而找不到保存的地方。
    pub(crate) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let body_height = (ui.available_height() - FOOTER_HEIGHT).max(240.0);
        let body_width = ui.available_width();
        ui.allocate_ui(egui::vec2(body_width, body_height), |ui| {
            ui.horizontal_top(|ui| {
                // 两栏都要显式换回自上而下的布局：`Frame::show` 沿用父 Ui 的方向，
                // 直接在 horizontal 里画框，框里的菜单会横着排成一条。
                ui.allocate_ui_with_layout(
                    egui::vec2(MENU_COLUMN_WIDTH, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.settings_menu_ui(ui),
                );
                ui.add_space(10.0);
                let detail_width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(detail_width, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.settings_detail_ui(ui),
                );
            });
        });
        self.settings_footer_ui(ui);
    }

    /// 左栏主菜单：沉底面板上按组竖排的分区列表，整行可点，选中项常显底色。
    fn settings_menu_ui(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::surface_sunk())
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(MENU_PANEL_MARGIN, 8))
            .show(ui, |ui| {
                ui.set_width(MENU_WIDTH);
                egui::ScrollArea::vertical()
                    .id_salt("settings_menu_scroll")
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
                                let selected = self.settings_section == *section;
                                if settings_menu_item(ui, selected, *section).clicked() {
                                    self.settings_section = *section;
                                }
                            }
                        }
                    });
            });
    }

    /// 右栏：当前分区的标题、说明与设置项。滚动位置按分区各自记忆，
    /// 来回切换不会丢掉看到一半的位置。
    fn settings_detail_ui(&mut self, ui: &mut egui::Ui) {
        let section = self.settings_section;
        theme::card()
            .inner_margin(egui::Margin::symmetric(18, 14))
            .corner_radius(egui::CornerRadius::same(10))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(format!("settings_scroll_{section:?}"))
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.set_max_width(DETAIL_MAX_WIDTH.min(ui.available_width()));
                        ui.horizontal(|ui| {
                            ui.add(
                                section
                                    .icon()
                                    .image()
                                    .tint(theme::accent())
                                    .fit_to_exact_size(egui::vec2(20.0, 20.0)),
                            );
                            ui.heading(section.label());
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
                        match section {
                            SettingsSection::ModelService => self.model_service_section_ui(ui),
                            SettingsSection::ReviseModel => self.revise_model_section_ui(ui),
                            SettingsSection::Knowledge => self.knowledge_section_ui(ui),
                            SettingsSection::Output => self.output_section_ui(ui),
                            SettingsSection::Theme => self.theme_settings_ui(ui),
                            SettingsSection::Font => self.font_settings_ui(ui),
                            SettingsSection::Numbering => self.numbering_settings_ui(ui),
                            SettingsSection::Export => self.export_section_ui(ui),
                            SettingsSection::Persistence => self.persistence_section_ui(ui),
                            SettingsSection::Security => self.security_section_ui(ui),
                            SettingsSection::Guide => guide_section_ui(ui),
                        }
                        ui.add_space(8.0);
                    });
            });
    }

    /// 底部常驻操作条：无论在哪个分区，保存都在同一个位置。
    fn settings_footer_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if theme::primary_icon_button(ui, theme::Icon::Save, "保存设置").clicked() {
                self.persist();
            }
            ui.add_space(8.0);
            ui.weak("主题、纸面与字体改完立即生效；其余各项要保存后才写入配置文件。");
        });
    }

    /// 本地模型服务分区：起草模型的接口地址、模型与生成参数。
    fn model_service_section_ui(&mut self, ui: &mut egui::Ui) {
        sub_heading(ui, "接口", None);
        setting_field(
            ui,
            "接口地址",
            &mut self.config.lm_studio.base_url,
            "包含 /v1",
        );
        setting_row(ui, "模型", None, |ui| {
            if self.models.is_empty() {
                ui.text_edit_singleline(&mut self.config.lm_studio.model);
            } else {
                egui::ComboBox::from_id_salt("model_selector")
                    .selected_text(if self.config.lm_studio.model.is_empty() {
                        "请选择模型"
                    } else {
                        &self.config.lm_studio.model
                    })
                    .width(300.0)
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
                    theme::icon_text_button(theme::Icon::PlugZap, "测试连接 / 刷新模型"),
                )
                .clicked()
            {
                self.start_model_probe();
            }
        });
        setting_field(
            ui,
            "API Key",
            &mut self.config.lm_studio.api_key,
            "本地服务通常可留空",
        );

        sub_heading(ui, "生成参数", None);
        setting_row(ui, "温度", None, |ui| {
            ui.add(
                egui::Slider::new(&mut self.config.lm_studio.temperature, 0.0..=1.2).step_by(0.05),
            );
        });
        setting_row(ui, "最大输出 Token", None, |ui| {
            ui.add(egui::DragValue::new(&mut self.config.lm_studio.max_tokens).range(256..=32768));
        });
        setting_row(ui, "超时（秒）", None, |ui| {
            ui.add(
                egui::DragValue::new(&mut self.config.lm_studio.timeout_seconds).range(5..=1800),
            );
        });
    }

    /// AI 文字复核分区：复核小模型的接口、参数、检查器开关与采纳统计。
    fn revise_model_section_ui(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(
            &mut self.config.revise_model.enabled,
            "启用文字复核（起草页「审校」分区出现入口）",
        );
        ui.add_enabled_ui(self.config.revise_model.enabled, |ui| {
            sub_heading(
                ui,
                "接口",
                Some(
                    "复核请求会自动带上关闭思考的开关（Qwen3 一类的模型开着 thinking 会把输出预算全花在推理上），服务端不认时会自动去掉重试；若报错提示只拿到思考过程，再到服务端关闭或改用非思考模型。",
                ),
            );
            setting_field(
                ui,
                "接口地址",
                &mut self.config.revise_model.base_url,
                "留空沿用起草模型的地址",
            );
            setting_row(ui, "模型", None, |ui| {
                if self.models.is_empty() {
                    ui.text_edit_singleline(&mut self.config.revise_model.model);
                } else {
                    egui::ComboBox::from_id_salt("revise_model_selector")
                        .selected_text(if self.config.revise_model.model.is_empty() {
                            "请选择模型"
                        } else {
                            &self.config.revise_model.model
                        })
                        .width(300.0)
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
            setting_field(
                ui,
                "API Key",
                &mut self.config.revise_model.api_key,
                "留空沿用起草模型的密钥",
            );

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

            sub_heading(ui, "检查器", None);
            self.revise_tasks_ui(ui);

            sub_heading(ui, "采纳统计", None);
            self.revise_metrics_ui(ui);
        });
    }

    /// 知识库分区：embedding 与 rerank 模型的接口与检索方式。
    fn knowledge_section_ui(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.config.rag.enabled, "启用知识库检索增强")
            .on_hover_text("关闭后，起草页的“参考知识库”开关不生效");
        sub_heading(ui, "Embedding 模型", None);
        setting_field(
            ui,
            "接口地址",
            &mut self.config.rag.embedding.base_url,
            "包含 /v1",
        );
        setting_row(ui, "模型", None, |ui| {
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
        setting_field(
            ui,
            "API Key",
            &mut self.config.rag.embedding.api_key,
            "本地服务通常可留空",
        );

        sub_heading(
            ui,
            "重排（可选，用于精排检索结果）",
            Some("rerank 响应字段名等进阶项可在 config.json 的 rag.rerank 节调整，适配不同服务。"),
        );
        let rerank_hint = match self.config.rag.rerank.mode {
            RerankMode::None => "直接按混合召回的融合分取前 N 条。够用，只是排序不如重排精准。",
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
                        ui.selectable_value(&mut self.config.rag.rerank.mode, mode, mode.label());
                    }
                });
        });
        if self.config.rag.rerank.mode == RerankMode::Api {
            setting_field(
                ui,
                "接口地址",
                &mut self.config.rag.rerank.base_url,
                "包含 /v1",
            );
            setting_row(ui, "端点路径", None, |ui| {
                ui.text_edit_singleline(&mut self.config.rag.rerank.path)
                    .on_hover_text("拼在接口地址后，默认 rerank；不同服务路径可能不同");
            });
            setting_row(ui, "模型", None, |ui| {
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
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            // 允许清空：rerank 可选。
                            if ui
                                .selectable_label(
                                    self.config.rag.rerank.model.is_empty(),
                                    "（不使用）",
                                )
                                .clicked()
                            {
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
            setting_field(
                ui,
                "API Key",
                &mut self.config.rag.rerank.api_key,
                "本地服务通常可留空",
            );
        }
        if self.config.rag.rerank.mode != RerankMode::None {
            setting_continuation(ui, |ui| {
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
                setting_continuation(ui, |ui| {
                    ui.colored_label(if ok { theme::accent() } else { warn() }, message);
                });
            }
        }
    }

    /// 输出与录入分区：输出目录、字段录入方式与编辑器选项。
    fn output_section_ui(&mut self, ui: &mut egui::Ui) {
        setting_row(ui, "输出目录", None, |ui| {
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

        sub_heading(ui, "录入与编辑器", None);
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
        ui.add_space(4.0);
        ui.weak("编辑器的字体与字号在「字体」一节设置。");
    }

    /// 导出格式分区：勾选起草页「导出」按钮产出的格式。
    fn export_section_ui(&mut self, ui: &mut egui::Ui) {
        // Word 导出尚未达到当前 LaTeX 链路的成熟度，入口保留但暂不允许启用。
        self.config.export.docx = false;
        setting_row(ui, "格式", None, |ui| {
            ui.checkbox(&mut self.config.export.markdown, "Markdown");
            ui.add_enabled(
                false,
                egui::Checkbox::new(&mut self.config.export.docx, "Word"),
            )
            .on_disabled_hover_text("Word 导出仍在完善，当前请使用 LaTeX/PDF");
            ui.checkbox(&mut self.config.export.tex, "LaTeX");
        });
        if !self.config.export.any() {
            setting_continuation(ui, |ui| {
                ui.colored_label(warn(), "未勾选任何导出格式，起草页的导出按钮不会产生文件。");
            });
        }
        ui.add_space(6.0);
        ui.checkbox(
            &mut self.config.auto_export,
            "AI 起草或优化完成后自动导出一次",
        )
        .on_hover_text("不勾选则只出稿，什么时候导出完全由你决定");
        ui.checkbox(&mut self.config.export.overwrite, "覆盖同名文件")
            .on_hover_text("不勾选时每次导出都会生成“标题-2、标题-3”这样的新文件");
    }

    /// 保存与现场分区：自动保存到稿件库的开关。
    fn persistence_section_ui(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.config.auto_save, "自动保存到稿件库")
            .on_hover_text(
                "每 2 分钟以及切换标签、关闭窗口前，把改动静默写回稿件库。
自动保存不会提交版本——版本链什么时候留痕，始终由你决定。",
            );
    }

    /// 密级规则分区：三级密级的保密期限上限与「长期」标注开关。
    fn security_section_ui(&mut self, ui: &mut egui::Ui) {
        setting_row(ui, "秘密级上限（年）", None, |ui| {
            ui.add(
                egui::DragValue::new(&mut self.config.security_rules.secret_max_years)
                    .range(1..=100),
            );
        });
        setting_row(ui, "机密级上限（年）", None, |ui| {
            ui.add(
                egui::DragValue::new(&mut self.config.security_rules.confidential_max_years)
                    .range(1..=100),
            );
        });
        setting_row(ui, "绝密级上限（年）", None, |ui| {
            ui.add(
                egui::DragValue::new(&mut self.config.security_rules.top_secret_max_years)
                    .range(1..=100),
            );
        });
        ui.add_space(6.0);
        ui.checkbox(
            &mut self.config.security_rules.allow_long_term,
            "期限无法确定时允许标注“长期”",
        );
    }
}
