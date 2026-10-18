//! egui 全局样式与重配置。
//!
//! 由 src/theme.rs 拆分而来：本文件是模块 `theme::style`，与其它子模块共享
//! `theme` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::theme::{
    CONTROL_HEIGHT, accent, accent_active, accent_soft, anim, border, border_strong, canvas,
    chrome_radius, current, danger, font_sizes, is_mdex, smartisan, surface, surface_active,
    surface_hover, surface_sunk, text, text_soft, warn,
};
use eframe::egui;
use eframe::egui::{Color32, Margin, Stroke};

pub(crate) fn configure_style(ctx: &egui::Context) {
    // 从 egui 对应明暗的内建 visuals 起步：本函数只覆盖用到的字段，
    // 其余（滚动条、拖拽反馈等）交给 egui 自己那套明色/深色缺省值，
    // 否则深色主题下会残留一批为明色算好的浅底。
    let dark = current().dark;
    let base = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(if dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });
    let mut style = (*ctx.style_of(base)).clone();

    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    // 一行控件的高度。egui 把 `interact_size.y` 当作横向布局的「行带」高度：
    // 每个控件先在行带里居中，再往右排。默认的 18px 是按 egui 自带西文字体和
    // 它那套 1px 按钮内边距算的；我们的中文字体行高更大、按钮内边距也调到了
    // 5pt，按钮、下拉框都比行带高——行带兜不住，矮的那个（标签、纯文字）就被
    // 顶到行的上沿，和右边的下拉框差出好几像素。把行带放到与控件同高，一行里
    // 的标签与控件才真正对在同一条中线上。
    style.spacing.interact_size.y = CONTROL_HEIGHT;
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.indent = 18.0;
    style.spacing.scroll.bar_width = 9.0;
    style.spacing.scroll.floating = false;
    // egui 内建动画（面板展开、折叠标题等）统一走同一档节奏，
    // 与本模块 `anim` 里手写的那些动效对齐。
    style.animation_time = anim::MEDIUM;

    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::new(font_sizes::HEADING, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(font_sizes::BODY, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(font_sizes::BODY, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Small,
            egui::FontId::new(font_sizes::SMALL, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(font_sizes::MONO, egui::FontFamily::Monospace),
        ),
    ]
    .into();

    let visuals = &mut style.visuals;
    visuals.panel_fill = canvas();
    visuals.window_fill = surface();
    visuals.window_stroke = Stroke::new(1.0, border());
    visuals.window_corner_radius = chrome_radius(10);
    visuals.menu_corner_radius = chrome_radius(8);
    visuals.faint_bg_color = surface_sunk();
    visuals.extreme_bg_color = surface();
    visuals.text_edit_bg_color = Some(if smartisan::active() {
        Color32::from_rgb(244, 236, 223)
    } else {
        surface()
    });
    visuals.code_bg_color = surface_sunk();
    visuals.hyperlink_color = accent();
    visuals.warn_fg_color = warn();
    visuals.error_fg_color = danger();
    visuals.selection = egui::style::Selection {
        bg_fill: accent_soft(),
        stroke: Stroke::new(1.0, accent_active()),
    };
    visuals.text_cursor.stroke = Stroke::new(2.0, accent());
    visuals.striped = !smartisan::active();
    visuals.indent_has_left_vline = false;
    // 深色底上 24/255 的黑几乎看不出来，投影要加重才能把浮窗从背景里托起来。
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 2],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 96 } else { 24 }),
    };
    if is_mdex() {
        visuals.window_shadow = egui::epaint::Shadow::NONE;
    }
    visuals.popup_shadow = visuals.window_shadow;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = canvas();
    widgets.noninteractive.weak_bg_fill = canvas();
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, border());
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, text());
    widgets.noninteractive.corner_radius = chrome_radius(7);

    widgets.inactive.bg_fill = surface_sunk();
    widgets.inactive.weak_bg_fill = surface_sunk();
    widgets.inactive.bg_stroke = Stroke::new(1.0, border());
    widgets.inactive.fg_stroke = Stroke::new(1.0, text_soft());
    widgets.inactive.corner_radius = chrome_radius(7);

    widgets.hovered.bg_fill = surface_hover();
    widgets.hovered.weak_bg_fill = surface_hover();
    widgets.hovered.bg_stroke = Stroke::new(1.0, border_strong());
    widgets.hovered.fg_stroke = Stroke::new(1.5, text());
    widgets.hovered.corner_radius = chrome_radius(7);
    widgets.hovered.expansion = 0.0;

    widgets.active.bg_fill = surface_active();
    widgets.active.weak_bg_fill = surface_active();
    widgets.active.bg_stroke = Stroke::new(1.0, accent());
    widgets.active.fg_stroke = Stroke::new(1.5, text());
    widgets.active.corner_radius = chrome_radius(7);
    widgets.active.expansion = 0.0;

    widgets.open.bg_fill = surface_sunk();
    widgets.open.weak_bg_fill = surface_sunk();
    widgets.open.bg_stroke = Stroke::new(1.0, accent());
    widgets.open.fg_stroke = Stroke::new(1.0, text());
    widgets.open.corner_radius = chrome_radius(7);

    ctx.set_style_of(base, style);
}

/// 在 UI 事件处理中切换主题。
///
/// 当前帧的根 `Ui` 在菜单点击前已经按旧主题创建；普通重绘不能丢掉这一次混合
/// 新旧样式的绘制结果。作废当前 pass 后，egui 会在同一帧立即重跑 UI，新的根
/// `Ui`、Markdown 编辑器与滚动区因而从一开始就使用同一套主题。
pub(crate) fn reconfigure_style(ctx: &egui::Context) {
    configure_style(ctx);
    ctx.request_discard("界面主题已切换，按新样式重排当前帧");
}
