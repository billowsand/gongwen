//! 从起草页选择交付介质。后台仅将内存中的成品交给选定的刻录后端。
use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Result, ensure};
use eframe::egui;

use super::GongwenApp;
use super::output_guard::OutputAction;
use crate::draft_page::{DocKey, ExportKind};
use crate::modal::{self, Dismiss};
use crate::models::ExportSelection;
use crate::vault::KeyHandle;

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Medium {
    Is903(crate::is903_writer::Target),
    Optical(crate::optical_burn::Drive),
}

impl Medium {
    fn label(&self) -> String {
        match self {
            Self::Is903(target) => format!("IS903 虚拟光盘 · {} · {}", target.serial, target.disk),
            Self::Optical(drive) => format!("实体光盘 · {}", drive.label()),
        }
    }

    pub(crate) fn burn(&self, key: &KeyHandle, image: &[u8]) -> Result<()> {
        let authorize = || require_workspace(key);
        authorize()?;
        match self {
            Self::Is903(target) => crate::is903_writer::write_iso(target, image, || {
                authorize()?;
                super::export_devices::require_approved(key, target)
            }),
            Self::Optical(drive) => crate::optical_burn::burn(drive, image, authorize),
        }
    }
}

pub(crate) fn require_workspace(key: &KeyHandle) -> Result<()> {
    ensure!(!crate::storage::writes_blocked(), "工作区正在锁定或切换");
    let current = crate::vault::key_handle().context("工作区已锁定")?;
    ensure!(current.same_unlock(key), "工作区已变化");
    Ok(())
}

#[derive(Default)]
struct MediaScan {
    devices: Vec<Medium>,
    notices: Vec<String>,
}

pub(super) struct Chooser {
    key: KeyHandle,
    document: DocKey,
    selection: ExportSelection,
    open: Option<ExportKind>,
    copy: Option<OutputAction>,
    chosen: Option<Medium>,
    scan: Option<Receiver<MediaScan>>,
    media: MediaScan,
}

fn start_scan(ctx: &egui::Context, key: Option<KeyHandle>) -> Receiver<MediaScan> {
    let ctx = ctx.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut scan = MediaScan::default();
        match key
            .as_ref()
            .context("工作区已锁定")
            .and_then(super::export_devices::approved_targets)
        {
            Ok(targets) if targets.is_empty() => scan
                .notices
                .push("未检测到已认证的 IS903。请在「设置 → 导出格式」认证设备。".into()),
            Ok(targets) => scan.devices.extend(targets.into_iter().map(Medium::Is903)),
            Err(error) => scan.notices.push(format!("IS903：{error:#}")),
        }
        match crate::optical_burn::scan() {
            Ok(drives) if drives.is_empty() => {
                scan.notices.push("未检测到可刻录的空白光盘。".into())
            }
            Ok(drives) => scan.devices.extend(drives.into_iter().map(Medium::Optical)),
            Err(error) => scan.notices.push(format!("实体光盘：{error:#}")),
        }
        let _ = sender.send(scan);
        ctx.request_repaint();
    });
    receiver
}

impl GongwenApp {
    pub(super) fn choose_draft_delivery(
        &mut self,
        ctx: &egui::Context,
        document: DocKey,
        selection: ExportSelection,
        open: Option<ExportKind>,
        copy: Option<OutputAction>,
    ) {
        if self.media_export.is_some() || self.pending_output.is_some() {
            self.status = "请先处理当前的导出请求。".into();
            return;
        }
        let Some(key) = crate::vault::key_handle() else {
            self.status = "当前已锁定，已取消导出。".into();
            return;
        };
        self.media_export = Some(Chooser {
            scan: Some(start_scan(ctx, crate::vault::key_handle())),
            key,
            document,
            selection,
            open,
            copy,
            chosen: None,
            media: MediaScan::default(),
        });
    }

    pub(super) fn media_export_window(&mut self, ctx: &egui::Context) {
        let Some(chooser) = self.media_export.as_mut() else {
            return;
        };
        if require_workspace(&chooser.key).is_err() {
            self.media_export = None;
            self.status = "工作区已变化，已取消导出。".into();
            return;
        }
        if let Some(scan) = &chooser.scan {
            match scan.try_recv() {
                Ok(media) => {
                    chooser.media = media;
                    chooser.scan = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    chooser.media.notices.push("介质检测任务已退出。".into());
                    chooser.scan = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut cancel = false;
        let mut encrypted = false;
        let mut confirmed = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("document_delivery"),
            "选择交付方式",
            540.0,
            Dismiss::EscOnly,
            |ui| {
                encrypted = ui.button("导出密码加密文件…").clicked();
                ui.separator();
                ui.strong("刻录明文");
                ui.weak("获得介质的人可以读取并复制其内容。写入 IS903 会覆盖原光盘内容。刻录完成前请保持所选介质插入。");
                ui.horizontal(|ui| {
                    if chooser.scan.is_some() {
                        ui.spinner();
                        ui.label("正在检测介质…");
                    } else if ui.button("重新检测").clicked() {
                        chooser.chosen = None;
                        chooser.media = MediaScan::default();
                        chooser.scan = Some(start_scan(ctx, crate::vault::key_handle()));
                    }
                });
                for notice in &chooser.media.notices {
                    ui.weak(notice);
                }
                for medium in &chooser.media.devices {
                    let selected = chooser.chosen.as_ref() == Some(medium);
                    if ui.selectable_label(selected, medium.label()).clicked() {
                        chooser.chosen = Some(medium.clone());
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    cancel = ui.button("取消").clicked();
                    confirmed = ui
                        .add_enabled(chooser.chosen.is_some(), egui::Button::new("确认刻录"))
                        .clicked();
                });
            },
        );
        if cancel || response.dismissed {
            self.media_export = None;
            self.status = "已取消导出。".into();
        } else if encrypted || confirmed {
            let Some(chooser) = self.media_export.take() else {
                return;
            };
            if encrypted {
                self.request_document_output("导出当前稿件", move |app| {
                    if let Some(copy) = chooser.copy {
                        copy(app);
                    } else if let Some(index) = app.doc_index_of_key(chooser.document) {
                        app.draft_page_at(index).start_confirmed_export(
                            chooser.selection,
                            chooser.open,
                            false,
                        );
                    }
                });
            } else if let Some(target) = chooser.chosen
                && let Some(index) = self.doc_index_of_key(chooser.document)
            {
                if self.any_busy() {
                    self.status = "请等待当前任务结束后再刻录。".into();
                    return;
                }
                self.draft_page_at(index).start_media_export(
                    chooser.selection,
                    target,
                    chooser.key,
                    ctx.clone(),
                );
            }
        }
    }
}
