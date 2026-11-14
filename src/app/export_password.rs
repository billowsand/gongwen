//! 设置页独立导出密码；不写配置历史，不保存明文，不复用登录口令。
use super::settings::{setting_continuation, setting_row};
use eframe::egui;
use zeroize::{Zeroize, Zeroizing};

#[derive(Default)]
pub(super) struct PasswordSettings {
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    show: bool,
    notice: String,
    saving: Option<std::sync::mpsc::Receiver<anyhow::Result<()>>>,
    /// 「重置损坏的导出设置」需要点两次，避免误删还能用的密码文件。
    reset_confirm: bool,
}

impl PasswordSettings {
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        ui.strong("导出加密");

        if crate::vault::key_handle().is_none() {
            ui.weak("先启用应用锁并解锁，再单独设置导出密码。");
            return;
        }
        match crate::export::protection::ExportPolicy::load() {
            Ok(_) => {
                ui.weak("加密工作区导出到普通文件时，每次须输入独立导出密码。");
            }
            Err(error) => {
                // fail-closed：解不开就不导出。但用户必须有一条恢复路径，
                // 否则只能手工去配置目录删文件（而且不知道该删哪个）。
                ui.colored_label(
                    crate::theme::danger(),
                    format!("导出设置读取失败：{error:#}"),
                );

                if self.reset_confirm {
                    ui.colored_label(
                        crate::theme::danger(),
                        "重置后应用内无法再预览旧加密 PDF，导出前需要重新设置导出密码。",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("确认重置导出设置").clicked() {
                            let result = crate::vault::key_handle().map_or_else(
                                || Err(anyhow::anyhow!("当前已锁定，未重置")),
                                |handle| {
                                    let path = crate::storage::slot_file_for(
                                        handle.slot(),
                                        crate::export::protection::PASSWORD_FILE,
                                    )?;
                                    crate::vault::erase_file_checked(&path)
                                },
                            );
                            self.reset_confirm = false;
                            self.notice = match result {
                                Ok(()) => "已重置导出设置，请设置新的导出密码。".into(),
                                Err(error) => format!("重置失败：{error:#}"),
                            };
                        }
                        if ui.button("取消").clicked() {
                            self.reset_confirm = false;
                        }
                    });
                } else if ui.button("重置导出设置…")
                    .on_hover_text("重置会删除已保存的导出密码设置。旧加密文件仍可在系统阅读器中用原密码打开。")
                    .clicked()
                {
                    self.reset_confirm = true;
                }
                if !self.notice.is_empty() {
                    ui.label(&self.notice);
                }
                return;
            }
        }
        if let Some(rx) = &self.saving {
            match rx.try_recv() {
                Ok(result) => {
                    self.saving = None;
                    self.notice = match result {
                        Ok(()) => "导出密码已单独保存。请妥善保管，旧文件仍使用旧密码。".into(),
                        Err(error) => format!("保存失败：{error:#}"),
                    };
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.saving = None;
                    self.notice = "密码保存任务中断，请重试。".into();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        let configured =
            crate::export::protection::ExportPolicy::load().is_ok_and(|p| !p.password.is_empty());
        ui.add_enabled_ui(self.saving.is_none(), |ui| {
            let password_tip = concat!(
                "必须与登录密码不同。已保存的导出密码由应用锁加密保护；能解锁本机数据的人仍可能取得它。\n",
                "接收方可用导出密码打开文件，登录本应用仍需登录密码。",
            );
            setting_row(ui, "新导出密码", Some(crate::password_policy::EXPORT_HINT), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut *self.password)
                        .password(!self.show)
                        .desired_width(260.0),
                )
                .on_hover_text(crate::password_policy::EXPORT_HINT);
                if ui
                    .button("随机生成")
                    .on_hover_text("按音节拼读的三个词，含大小写字母、数字与符号，便于口述与记忆")
                    .clicked()
                {
                    *self.password = crate::export::protection::generated_password();
                    *self.confirm = self.password.to_string();
                    self.show = true;
                }
            });
            setting_row(ui, "确认密码", Some("再次输入新导出密码。"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut *self.confirm)
                        .password(!self.show)
                        .desired_width(260.0),
                );
                ui.checkbox(&mut self.show, "显示输入");
            });
            let save = setting_continuation(ui, |ui| {
                let save = ui.button("保存导出密码").on_hover_text(password_tip).clicked();
                ui.weak(if configured {
                    "已设置密码"
                } else {
                    "尚未设置密码"
                })
                .on_hover_text(if configured {
                    "导出加密文件时，请输入此密码。"
                } else {
                    "加密导出前需要先保存密码。"
                });
                save
            });
            if save {
                if self.password != self.confirm {
                    self.notice = "两次输入的导出密码不一致。".into();
                } else if let Err(error) = crate::export::protection::validate_password(&self.password) {
                    self.notice = error.to_string();
                } else if let Some(handle) = crate::vault::key_handle() {
                    let password = Zeroizing::new(std::mem::take(&mut *self.password));
                    self.confirm.zeroize();
                    self.show = false;
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.saving = Some(rx);
                    self.notice = "正在校验并保存…".into();
                    std::thread::spawn(move || {
                        let _ = tx.send(save_password(&password, &handle));
                    });
                }
            }
        });
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
    }
}

fn save_password(password: &str, handle: &crate::vault::KeyHandle) -> anyhow::Result<()> {
    use anyhow::ensure;
    crate::export::protection::validate_password(password)?;
    // 对两个验证器只做只读验证，不切换槽位，也不返回哪一个命中。
    ensure!(
        !crate::vault::Vault::load()?.accepts_password(password),
        "导出密码不能与登录密码相同，请另设一个密码"
    );
    let current =
        crate::vault::key_handle().ok_or_else(|| anyhow::anyhow!("当前已锁定，未保存导出密码"))?;
    ensure!(current.same_unlock(handle), "工作区已变化，未保存导出密码");
    let mut policy = crate::export::protection::ExportPolicy::load()?;
    policy.password = password.to_owned();
    policy.save(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    const LOGIN: &str = "Harbor!Stone8Pine";
    const EXPORT: &str = "Jade!River7Cloud";

    #[test]
    fn export_password_is_independent_sealed_and_cannot_become_login_password() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().into()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::vault::clear_key_handle();
                crate::storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        let (_, unlocked) = crate::vault::Vault::create(LOGIN, None, false).unwrap();
        crate::vault::set_key_handle(*unlocked.unwrap().dek, true);
        let handle = crate::vault::key_handle().unwrap();
        assert!(save_password(LOGIN, &handle).is_err());
        let path = dir.path().join(crate::export::protection::PASSWORD_FILE);
        assert!(!path.exists());
        save_password(EXPORT, &handle).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(crate::vault::is_sealed_file(&raw));
        assert!(!raw.windows(EXPORT.len()).any(|p| p == EXPORT.as_bytes()));
        assert!(
            crate::export::protection::Protection::current()
                .unwrap()
                .unwrap()
                .matches_password(EXPORT)
        );
        assert!(
            crate::vault::Vault::load()
                .unwrap()
                .set_password(crate::storage::DataSlot::Real, EXPORT, Some(handle.dek()))
                .is_err()
        );
        assert!(crate::export::protection::validate_password(&"汉".repeat(45)).is_err());
    }
}
