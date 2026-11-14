//! 设置页「环境检测」：策略配置与当前采集状态，不展示设备序列号或正文信息。

use super::GongwenApp;
use super::settings::{setting_note, setting_row, sub_heading};
use crate::models::NetworkPolicy;
use crate::storage::{self, DataSlot};
use crate::theme;
use eframe::egui;

impl GongwenApp {
    pub(crate) fn environment_section_ui(&mut self, ui: &mut egui::Ui) {
        if storage::current_slot() == DataSlot::Decoy {
            ui.label(egui::RichText::new("本机未启用环境安全检测。").color(theme::text_soft()));
            return;
        }
        let encrypted = crate::vault::current_db_key_active();
        if !encrypted {
            setting_note(
                ui,
                "先启用应用锁并完成数据加密，再开启环境检测。未加密工作区不能提供锁定后的数据保护。",
            );
            return;
        }

        let summary = self.environment_summary.clone();
        let policy = &mut self.environment_draft;
        setting_row(
            ui,
            "启用环境检测",
            Some("保存后生效。启用期间，风险事件会先遮蔽工作区，再保存并锁定。"),
            |ui| {
                ui.checkbox(&mut policy.enabled, "启用");
            },
        );

        setting_note(ui, &summary);
        setting_note(ui, &self.environment_preview);
        if !self.config.environment.enabled {
            setting_note(
                ui,
                "当前只观察。可调整检测项并查看预检结果，保存通过后才开始锁定。",
            );
        }

        ui.add_space(8.0);
        sub_heading(ui, "网络", None);
        setting_row(
            ui,
            "允许状态",
            Some("两种模式都忽略回环接口；本机模型服务不受影响。"),
            |ui| {
                ui.radio_value(
                    &mut policy.network,
                    NetworkPolicy::Offline,
                    "网卡可存在，但不得联网",
                )
                .on_hover_text("网卡链路连接、取得可路由地址或出现默认路由都会触发锁定；系统服务接口与常驻虚拟链路按各自规则判定。");
                ui.radio_value(
                    &mut policy.network,
                    NetworkPolicy::NoAdapters,
                    "不得存在网卡",
                )
                .on_hover_text(
                    "物理网卡、虚拟网卡、VPN 等存在即锁定；只有确认的系统服务接口除外。",
                );
            },
        );

        ui.add_space(8.0);
        sub_heading(ui, "外设", None);
        setting_row(
            ui,
            "检测类别",
            Some("勾选的设备有可读介质或功能出现即锁定；已认证 IS903 仅豁免其移动存储功能。"),
            |ui| {
                ui.vertical(|ui| {
                    ui.checkbox(&mut policy.devices.removable_storage, "U 盘 / 移动存储");
                    ui.checkbox(&mut policy.devices.portable_media, "WPD / MTP / PTP");
                    ui.checkbox(&mut policy.devices.human_interface, "人机接口设备");
                    ui.checkbox(&mut policy.devices.imaging, "图像采集设备");
                    ui.checkbox(&mut policy.devices.other, "其他外部设备");
                });
            },
        );

        ui.add_space(8.0);
        sub_heading(ui, "托底", None);
        setting_row(
            ui,
            "监控失效",
            Some("后端错误、快照缺失或超过 15 秒未更新时，按环境不安全处理。"),
            |ui| {
                ui.checkbox(&mut policy.fail_closed, "失效时立即锁定");
            },
        );
        #[cfg(windows)]
        setting_row(
            ui,
            "窗口捕获排除",
            Some("要求 Windows 把本窗口排除在支持的捕获路径之外；设置失败或被移除时锁定。"),
            |ui| {
                ui.checkbox(&mut policy.capture_protection, "启用并持续复核");
            },
        );
        setting_note(
            ui,
            "截屏动作没有跨平台完整检测接口；本模块不承诺识别所有截图、录屏、远程共享或拍摄屏幕。",
        );
    }
}

#[cfg(test)]
mod samples {
    use super::*;

    #[test]
    #[ignore = "出环境检测设置样张，手动跑"]
    fn environment_settings_sample() {
        let ctx = egui::Context::default();
        theme::set_current(crate::models::ThemeName::Smartisan);
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        theme::configure_icons(&ctx);
        theme::configure_style(&ctx);
        crate::vault::set_key_handle_for_slot([7; 32], true, DataSlot::Real);
        let mut app = crate::app::management_samples::fixture();
        app.environment_draft.enabled = true;
        app.environment_summary = "非回环接口 1 个，已连接 0 个；受监控外设 0 个".into();
        let size = egui::vec2(1000.0, 760.0);
        let mut canvas = crate::ui_snapshot::Canvas::default();
        for frame in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(frame as f64),
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE.fill(theme::surface()))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                app.environment_section_ui(ui);
                            });
                        });
                },
            );
            if frame < 2 {
                canvas.absorb(&mut output.textures_delta);
            } else {
                output.textures_delta.clear();
                canvas.render(
                    &ctx,
                    output,
                    size,
                    theme::canvas(),
                    std::path::Path::new("tmp/environment-settings.png"),
                );
            }
        }
        crate::vault::clear_key_handle();
        theme::set_current(crate::models::ThemeName::default());
    }
}
