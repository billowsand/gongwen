//! AI 管理页「智能体」分组的共享状态与小组件。
//!
//! 技能页的状态在 `pi_skill_manager`，数据接口页的状态在 `api_settings`；
//! 这里只放两者的入口和公用的只读代码块。旧工具调试台已移除——Pi 的工具由
//! Pi 执行内核提供，不在这里配置或调试。

use super::api_settings::ApisPage;
use super::pi_skill_manager::PiSkillsPage;
use eframe::egui;

/// 两个分区的状态。纯当次会话，不进配置。
#[derive(Default)]
pub(crate) struct AgentSettings {
    pub(super) skills: PiSkillsPage,
    pub(super) apis: ApisPage,
}

/// 只读代码块，用于展示接口返回、校验详情等。
pub(super) fn code_block(ui: &mut egui::Ui, id: &str, text: &str) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(200.0)
        .show(ui, |ui| {
            let mut text = text.to_string();
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .interactive(false)
                    .desired_width(f32::INFINITY),
            );
        });
}
