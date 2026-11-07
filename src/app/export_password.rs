//! 设置页独立导出密码；不写配置历史，不保存明文，不复用登录口令。
use eframe::egui;
use zeroize::{Zeroize, Zeroizing};

#[derive(Default)]
pub(super) struct PasswordSettings {
    password: Zeroizing<String>,
    confirm: Zeroizing<String>,
    show: bool,
    notice: String,
    saving: Option<std::sync::mpsc::Receiver<anyhow::Result<()>>>,
}

impl PasswordSettings {
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        ui.strong("导出加密");
        ui.label("勾选加密后，Word、PDF 使用打开密码，源码包及份号清单使用加密 7z。修改密码只影响今后的导出。");
        if crate::vault::key_handle().is_none() {
            ui.weak("先启用应用锁并解锁，再单独设置导出密码。");
            return;
        }
        match crate::export::protection::ExportPolicy::load() {
            Ok(mut policy) => {
                let mut enabled = policy.enabled;
                if ui
                    .add_enabled(
                        self.saving.is_none(),
                        egui::Checkbox::new(
                            &mut enabled,
                            "加密导出文档（Word、PDF、源码包及随行单据）",
                        ),
                    )
                    .changed()
                {
                    policy.enabled = enabled;
                    if let Some(handle) = crate::vault::key_handle() {
                        self.notice = match policy.save(&handle) {
                            Ok(()) => if enabled {
                                "已启用加密导出；加密文件导出不再提示风险。"
                            } else {
                                "已关闭导出加密；每次明文导出前会确认安全风险。"
                            }
                            .into(),
                            Err(error) => format!("设置保存失败：{error:#}"),
                        };
                    }
                }
                ui.weak("勾选时加密导出，不弹安全风险确认；不勾选时导出普通文件，每次都需明确确认。打印与剪贴板仍单独确认。");
            }
            Err(error) => {
                ui.colored_label(
                    crate::theme::danger(),
                    format!("导出设置读取失败：{error:#}"),
                );
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
        ui.weak(if configured {
            "已设置独立导出密码。"
        } else {
            "尚未设置密码：选择加密时需先保存密码；选择明文时无需密码。"
        });
        ui.add_enabled_ui(self.saving.is_none(), |ui| {
            ui.horizontal(|ui| {
                ui.label("新导出密码");
                ui.add(egui::TextEdit::singleline(&mut *self.password).password(!self.show).desired_width(260.0));
                if ui.button("随机生成").clicked() {
                    *self.password = crate::export::protection::generated_password();
                    *self.confirm = self.password.to_string();
                    self.show = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("确认密码");
                ui.add(egui::TextEdit::singleline(&mut *self.confirm).password(!self.show).desired_width(260.0));
                ui.checkbox(&mut self.show, "显示输入");
            });
            ui.weak(crate::password_policy::EXPORT_HINT);
            ui.weak("必须与登录密码不同；本地保存的导出密码由应用锁加密保护。接收方得到导出密码不能用它登录本应用；能解锁本机数据的人仍可能取得已保存的导出密码。");
            if ui.button("保存独立导出密码").clicked() {
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
    ensure!(
        current.slot() == handle.slot() && current.dek() == handle.dek(),
        "工作区已变化，未保存导出密码"
    );
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
