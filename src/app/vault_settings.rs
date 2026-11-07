//! 设置页「应用锁」分区：启用 / 停用、改主密码、设置与清除第二密码、
//! 闲置自动锁定时限。设计见 docs/security-lock-design.md。
//!
//! 两条硬约束：
//! - 界面**不显示**「是否设置了第二密码」——锁文件里那个槽是真是假看不出来，
//!   这里也只能提供「设置（覆盖）」与「清除」两个盲操作；
//! - 诱饵系统（第二密码进入的空白系统）不显示本分区：它的「设置页」里
//!   没有应用锁这一项才符合「另一台机器」的样子。

use super::GongwenApp;
use super::settings::{setting_note, setting_row};
use crate::storage::{self, DataSlot};
use crate::theme;
use crate::vault::{Probe, Vault};
use eframe::egui;

/// 分区里的一句话提示：成功绿、失败红，样式与设置页弱化小字一致。
fn setting_note_colored(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.label(
        egui::RichText::new(text)
            .size(theme::font_sizes::SMALL)
            .color(color),
    );
}

/// 设置页「应用锁」分区的输入草稿与会话内提示。
#[derive(Default)]
pub(crate) struct VaultPageState {
    /// 启用表单：主密码与确认。
    enable_real: String,
    enable_real_confirm: String,
    /// 启用表单：要不要同时设第二密码。
    with_decoy: bool,
    enable_decoy: String,
    enable_decoy_confirm: String,
    /// 已启用后的修改表单。
    change_real: String,
    change_real_confirm: String,
    decoy_new: String,
    decoy_new_confirm: String,
    /// 停用 / 清除第二密码的行内二次确认。
    disable_confirm: bool,
    clear_decoy_confirm: bool,
    /// 操作结果：一句话提示（成功或失败文案都走这里，失败另标红）。
    notice: Option<(String, bool)>,
}

/// 闲置自动锁定的候选项（分钟；0 = 永不）。
const AUTO_LOCK_OPTIONS: [(u32, &str); 5] = [
    (0, "永不"),
    (5, "5 分钟"),
    (15, "15 分钟"),
    (30, "30 分钟"),
    (60, "1 小时"),
];

impl GongwenApp {
    pub(crate) fn vault_section_ui(&mut self, ui: &mut egui::Ui) {
        if storage::current_slot() == DataSlot::Decoy {
            // 诱饵系统：当作「这台机器没启用过应用锁」。
            ui.label(egui::RichText::new("本机未启用应用锁。").color(theme::text_soft()));
            return;
        }
        let probe = Vault::probe();
        match probe {
            Probe::Locked => self.vault_enabled_ui(ui),
            Probe::Absent | Probe::Skipped => self.vault_disabled_ui(ui),
            Probe::Broken(reason) => {
                ui.label(
                    egui::RichText::new(format!("锁设置已损坏：{reason}。请到锁屏清除后重启。"))
                        .color(theme::danger()),
                );
            }
        }
    }

    /// 未启用：引导启用，表单与首次启动的锁屏一致。
    fn vault_disabled_ui(&mut self, ui: &mut egui::Ui) {
        setting_row(
            ui,
            "主密码",
            Some(crate::password_policy::LOCK_HINT),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.vault_page.enable_real)
                        .password(true)
                        .desired_width(220.0),
                );
            },
        );
        setting_row(ui, "确认主密码", None, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.vault_page.enable_real_confirm)
                    .password(true)
                    .desired_width(220.0),
            );
        });
        ui.add_space(4.0);
        setting_row(
            ui,
            "第二密码",
            Some("可选。输入第二密码进入的是空白系统；设不设，磁盘上看不出区别。"),
            |ui| {
                ui.checkbox(&mut self.vault_page.with_decoy, "同时设置第二密码")
                    .on_hover_text(
                        "两套密码都能正常进入，界面没有区别；两套数据完全独立、互不越界。",
                    );
            },
        );
        if self.vault_page.with_decoy {
            setting_row(ui, "第二密码", None, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.vault_page.enable_decoy)
                        .password(true)
                        .desired_width(220.0),
                );
            });
            setting_row(ui, "确认第二密码", None, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.vault_page.enable_decoy_confirm)
                        .password(true)
                        .desired_width(220.0),
                );
            });
        }
        ui.add_space(8.0);
        self.vault_notice(ui);
        ui.add_space(4.0);
        setting_row(ui, "", None, |ui| {
            if ui
                .button("启用密码保护")
                .on_hover_text("启用后立即迁移加密现有数据，启动与闲置超时都要输入密码。")
                .clicked()
            {
                self.vault_apply_enable();
            }
        });
    }

    /// 已启用：改密、第二密码、自动锁定、停用。
    fn vault_enabled_ui(&mut self, ui: &mut egui::Ui) {
        setting_note(ui, "已启用。启动时要输入密码；菜单里也可以随时手动锁定。");
        ui.add_space(10.0);

        if crate::vault::disable_marker_present() {
            setting_note_colored(
                ui,
                "已申请停用：锁定后输入一次密码完成解密；重启未完成前可在此取消。",
                theme::text_soft(),
            );
            ui.add_space(4.0);
            setting_row(ui, "", None, |ui| {
                if ui.button("取消停用").clicked() {
                    crate::vault::cancel_disable();
                    self.notice_vault("已取消停用，密码保护保持启用。".into());
                }
            });
            ui.add_space(10.0);
        }

        setting_row(
            ui,
            "闲置自动锁定",
            Some("没有任何键盘 / 鼠标输入超过这个时长就退回锁屏。"),
            |ui| {
                let current = self.config.auto_lock_minutes;
                let label = AUTO_LOCK_OPTIONS
                    .iter()
                    .find(|(minutes, _)| *minutes == current)
                    .map_or("15 分钟", |(_, label)| label);
                egui::ComboBox::from_id_salt("vault_auto_lock_minutes")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (minutes, label) in AUTO_LOCK_OPTIONS {
                            if ui.selectable_label(current == minutes, label).clicked()
                                && current != minutes
                            {
                                self.config.auto_lock_minutes = minutes;
                                self.notice_vault(format!("闲置 {label} 后自动锁定"));
                            }
                        }
                    });
            },
        );
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        setting_row(
            ui,
            "修改主密码",
            Some("立即生效；忘记密码无法恢复数据。"),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.vault_page.change_real)
                        .password(true)
                        .hint_text("新主密码")
                        .desired_width(200.0),
                );
            },
        );
        setting_row(ui, "", None, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.vault_page.change_real_confirm)
                    .password(true)
                    .hint_text("再输一遍")
                    .desired_width(200.0),
            );
            if ui.button("修改主密码").clicked() {
                self.vault_apply_change_real();
            }
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        setting_row(
            ui,
            "第二密码",
            Some("输入第二密码进入的是空白系统；这里看不出当前是否已设置。"),
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.vault_page.decoy_new)
                        .password(true)
                        .hint_text("新第二密码")
                        .desired_width(200.0),
                );
            },
        );
        setting_row(ui, "", None, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.vault_page.decoy_new_confirm)
                    .password(true)
                    .hint_text("再输一遍")
                    .desired_width(200.0),
            );
            if ui
                .button("设置第二密码")
                .on_hover_text("设置或覆盖；与主密码不能相同。会重置第二密码系统的数据。")
                .clicked()
            {
                self.vault_apply_set_decoy();
            }
        });
        setting_row(ui, "", None, |ui| {
            if self.vault_page.clear_decoy_confirm {
                if ui
                    .button("确认清除")
                    .on_hover_text("清除后该密码立即失效，第二密码系统的数据一并删除。")
                    .clicked()
                {
                    self.vault_apply_clear_decoy();
                }
                if ui.button("取消").clicked() {
                    self.vault_page.clear_decoy_confirm = false;
                }
            } else if ui
                .button("清除第二密码")
                .on_hover_text("无论之前是否设置过，都把第二密码槽位置成空白。")
                .clicked()
            {
                self.vault_page.clear_decoy_confirm = true;
            }
        });
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        self.vault_notice(ui);
        ui.add_space(4.0);
        setting_row(
            ui,
            "停用密码保护",
            Some("锁定后输入一次密码，把数据解密回明文，回到无密码状态。"),
            |ui| {
                if self.vault_page.disable_confirm {
                    if ui.button("确认停用").clicked() {
                        self.vault_apply_disable();
                    }
                    if ui.button("取消").clicked() {
                        self.vault_page.disable_confirm = false;
                    }
                } else if ui.button("停用密码保护…").clicked() {
                    self.vault_page.disable_confirm = true;
                }
            },
        );
    }

    fn vault_notice(&mut self, ui: &mut egui::Ui) {
        if let Some((text, ok)) = self.vault_page.notice.clone() {
            let color = if ok {
                theme::success()
            } else {
                theme::danger()
            };
            setting_note_colored(ui, &text, color);
        }
    }

    fn notice_vault(&mut self, text: String) {
        self.vault_page.notice = Some((text, true));
    }

    fn fail_vault(&mut self, error: impl std::fmt::Display) {
        self.vault_page.notice = Some((error.to_string(), false));
    }

    /// 刚在设置页启用了密码保护：立即把磁盘上的明文数据迁成密文。
    ///
    /// 迁移要换掉稿件库文件（SQLCipher 重新导出），工作区里的连接必须先关掉，
    /// 所以这条请求由**外壳**执行（见 `AppShell::run`）：先存盘 → 丢弃工作区 →
    /// 后台迁移 → 重建工作区。推迟到下次解锁等于让磁盘上的正文多敞开一段时间，
    /// 与「拷走配置目录就能翻看正文」的威胁模型直接冲突。
    fn vault_apply_enable(&mut self) {
        self.vault_apply_enable_inner();
    }

    /// 测试用入口：填好主/第二密码后走“启用密码保护”。
    #[cfg(test)]
    pub(crate) fn vault_enable_with_for_test(&mut self, real: &str, decoy: Option<&str>) {
        self.vault_page.enable_real = real.to_string();
        self.vault_page.enable_real_confirm = real.to_string();
        self.vault_page.with_decoy = decoy.is_some();
        if let Some(decoy) = decoy {
            self.vault_page.enable_decoy = decoy.to_string();
            self.vault_page.enable_decoy_confirm = decoy.to_string();
        }
        self.vault_apply_enable_inner();
    }

    fn vault_apply_enable_inner(&mut self) {
        let page = &mut self.vault_page;
        if page.enable_real != page.enable_real_confirm {
            page.notice = Some(("两次输入的主密码不一致。".into(), false));
            return;
        }
        if page.with_decoy && page.enable_decoy != page.enable_decoy_confirm {
            page.notice = Some(("两次输入的第二密码不一致。".into(), false));
            return;
        }
        // 密码强度先在本地校验一遍，给出具体是哪一条不满足。
        if let Err(error) = crate::password_policy::validate_lock(&page.enable_real) {
            self.fail_vault(format!("主密码不合要求：{error}"));
            return;
        }
        if page.with_decoy
            && let Err(error) = crate::password_policy::validate_lock(&page.enable_decoy)
        {
            self.fail_vault(format!("第二密码不合要求：{error}"));
            return;
        }
        let real = page.enable_real.clone();
        let decoy = page
            .with_decoy
            .then(|| page.enable_decoy.clone())
            .filter(|password| !password.is_empty());
        match Vault::create(&real, decoy.as_deref(), false) {
            Ok((_, Some(unlocked))) => {
                // 全新 DEK 意味着旧诱饵数据从此不可读。**先立锁成功、再擦旧文件**：
                // 密码不合规或写盘失败时不能先把旧诱饵数据毁掉。
                if decoy.is_some() {
                    storage::wipe_slot_files(DataSlot::Decoy);
                }
                // 本会话继续按明文开库（磁盘数据还没迁）；配置类文件从下一次
                // 保存起就密封。数据迁移安排在下次解锁，由闸门完成。
                crate::vault::set_key_handle_for_slot(*unlocked.dek, false, DataSlot::Real);
                self.vault_locked = true;
                let page = &mut self.vault_page;
                page.enable_real.clear();
                page.enable_real_confirm.clear();
                page.enable_decoy.clear();
                page.enable_decoy_confirm.clear();
                page.notice = Some(("密码保护已启用；正在把现有数据加密到磁盘。".into(), true));
                // 立刻加密，不等下次解锁。
                self.request_enable_encrypt();
            }
            Ok((_, None)) => self.fail_vault("锁设置保存失败，请重试"),
            Err(error) => self.fail_vault(format!("{error:#}")),
        }
    }

    fn vault_apply_change_real(&mut self) {
        let page = &mut self.vault_page;
        if page.change_real != page.change_real_confirm {
            page.notice = Some(("两次输入的主密码不一致。".into(), false));
            return;
        }
        // 新密码按同一套强度规则校验（改密不能成为绕过强度要求的口子）。
        if let Err(error) = crate::password_policy::validate_lock(&page.change_real) {
            self.fail_vault(format!("新主密码不合要求：{error}"));
            return;
        }
        let password = page.change_real.clone();
        // 改密只换包裹：把当前 DEK 用新密码裹回去，数据文件一个都不动。
        let Some(handle) = crate::vault::key_handle() else {
            self.fail_vault("当前状态无法修改密码，请重启应用后再试");
            return;
        };
        let current_dek = *handle.dek();
        match Vault::load()
            .and_then(|mut vault| vault.set_password(DataSlot::Real, &password, Some(&current_dek)))
        {
            Ok(_) => {
                let page = &mut self.vault_page;
                page.change_real.clear();
                page.change_real_confirm.clear();
                page.notice = Some(("主密码已修改。".into(), true));
            }
            Err(error) => self.fail_vault(format!("{error:#}")),
        }
    }

    fn vault_apply_set_decoy(&mut self) {
        let page = &mut self.vault_page;
        if page.decoy_new != page.decoy_new_confirm {
            page.notice = Some(("两次输入的第二密码不一致。".into(), false));
            return;
        }
        // 第二密码同样按强度规则校验：它也是一把真钥匙，弱密码等于白设。
        if let Err(error) = crate::password_policy::validate_lock(&page.decoy_new) {
            self.fail_vault(format!("第二密码不合要求：{error}"));
            return;
        }
        let password = page.decoy_new.clone();
        // 第二密码总是配一把全新 DEK——等于重置第二密码系统。先设置成功再擦
        // 旧诱饵数据：密码校验失败或写盘失败时不能毁掉原有诱饵系统。
        match Vault::load()
            .and_then(|mut vault| vault.set_password(DataSlot::Decoy, &password, None))
        {
            Ok(_) => {
                storage::wipe_slot_files(DataSlot::Decoy);
                let page = &mut self.vault_page;
                page.decoy_new.clear();
                page.decoy_new_confirm.clear();
                page.notice = Some(("第二密码已设置。".into(), true));
            }
            Err(error) => self.fail_vault(format!("{error:#}")),
        }
    }

    fn vault_apply_clear_decoy(&mut self) {
        match Vault::load().and_then(|mut vault| vault.clear_decoy()) {
            Ok(()) => {
                self.vault_page.clear_decoy_confirm = false;
                self.notice_vault("第二密码已清除，诱饵系统的数据文件已删除。".into());
            }
            Err(error) => self.fail_vault(format!("{error:#}")),
        }
    }

    fn vault_apply_disable(&mut self) {
        // 停用 = 解密回明文：先标记，再锁定；锁屏输入一次密码完成数据库解密，
        // 之后删锁文件、写「暂不设置」标记，回到无密码状态。
        match crate::vault::request_disable() {
            Ok(()) => {
                self.vault_page.disable_confirm = false;
                self.notice_vault("已申请停用：应用即将锁定，输入一次密码完成解密。".into());
                self.request_lock();
            }
            Err(error) => self.fail_vault(format!("{error:#}")),
        }
    }
}
