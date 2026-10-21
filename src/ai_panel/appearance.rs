//! 便签主题的侧栏纸面与任务入口；只负责呈现，不发起模型调用。

use crate::theme;
use eframe::egui;

pub(super) fn background() -> egui::Color32 {
    if theme::smartisan::active() {
        egui::Color32::from_rgb(250, 246, 235)
    } else {
        theme::canvas()
    }
}

pub(super) fn composer_fill() -> egui::Color32 {
    if theme::smartisan::active() {
        egui::Color32::from_rgb(255, 252, 245)
    } else {
        theme::surface()
    }
}

pub(super) fn header_button(ui: &mut egui::Ui, icon: theme::Icon, label: &str) -> egui::Response {
    if !theme::smartisan::active() {
        return theme::titlebar_icon_button(ui, true, icon, label);
    }
    let response = ui.add(
        egui::Button::image(icon.image_sized(16.0))
            .image_tint_follows_text_color(true)
            .frame_when_inactive(false)
            .corner_radius(4)
            .min_size(egui::vec2(26.0, 26.0)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response.on_hover_text(label)
}

/// 入口只填入输入框，保留发送前的人工编辑机会；已有输入时也不覆盖。
pub(super) fn welcome(ui: &mut egui::Ui, has_text: bool, can_fill: bool) -> Option<&'static str> {
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("ai_paper_welcome")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // 常态卡片保持纸色，悬停与按下继续沿用主题的交互反馈。
            ui.visuals_mut().widgets.inactive.weak_bg_fill = theme::surface();
            ui.add_space((ui.available_height() * 0.08).clamp(12.0, 48.0));
            ui.vertical_centered(|ui| {
                ui.add(
                    theme::Icon::Sparkles
                        .image_sized(32.0)
                        .tint(theme::smartisan::INDEX_MARK),
                );
                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new("今天想写些什么？")
                        .size(theme::font_sizes::HEADING)
                        .strong()
                        .color(theme::text()),
                );
                ui.add_space(6.0);
                ui.label(egui::RichText::new("从起草、修改或审核开始").color(theme::text_soft()));
            });
            ui.add_space(26.0);
            for (icon, label, prompt, needs_text) in [
                (theme::Icon::Edit, "起草一份通知", "起草一份通知：", false),
                (
                    theme::Icon::WandSparkles,
                    "润色当前文稿",
                    "润色当前文稿，保留原有事实和行文要素。",
                    true,
                ),
                (
                    theme::Icon::Search,
                    "检查文字与依据",
                    "签发前帮我审一下，检查文字表述和引用依据。",
                    true,
                ),
            ] {
                let enabled = can_fill && (!needs_text || has_text);
                let response = ui
                    .add_enabled(
                        enabled,
                        egui::Button::image_and_text(icon.image_sized(18.0), label)
                            .image_tint_follows_text_color(true)
                            .right_text(theme::Icon::ChevronRight.image_sized(14.0))
                            .min_size(egui::vec2(ui.available_width(), 48.0))
                            .corner_radius(5)
                            .stroke(egui::Stroke::new(1.0, theme::border())),
                    )
                    .on_disabled_hover_text(if !can_fill {
                        "输入框已有内容，请先编辑或发送当前要求"
                    } else {
                        "先打开或写入一篇文稿"
                    });
                if response.clicked() {
                    picked = Some(prompt);
                }
                ui.add_space(8.0);
            }
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("/ 选择技能 · @ 引用材料")
                        .small()
                        .color(theme::text_muted()),
                );
            });
        });
    picked
}
