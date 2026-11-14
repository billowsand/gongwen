//! 当前加密工作区批准的 IS903 虚拟光盘设备。
use std::collections::BTreeSet;
use std::io::Write;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use eframe::egui;
use serde::{Deserialize, Serialize};

const FILE: &str = ".export-devices";
const MAX_BYTES: u64 = 16 * 1024;
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Identity {
    serial: String,
    revision: u64,
}

use crate::is903_writer::Target as Device;

impl From<&Device> for Identity {
    fn from(device: &Device) -> Self {
        Self {
            serial: device.serial.clone(),
            revision: device.revision,
        }
    }
}

type Approvals = BTreeSet<Identity>;

fn approvals(key: &crate::vault::KeyHandle) -> Result<Approvals> {
    let path = crate::storage::slot_file_for(key.slot(), FILE)?;
    let sealed = match crate::file_limits::read_bytes(&path, MAX_BYTES, "设备认证记录") {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error.into()),
    };
    ensure!(crate::vault::is_sealed_file(&sealed), "设备认证记录未加密");
    let bytes = crate::vault::open_path_bytes(key, &path, "config", &sealed)?;
    serde_json::from_slice(&bytes).context("设备认证记录损坏")
}

fn save(key: &crate::vault::KeyHandle, approvals: &Approvals) -> Result<()> {
    let current = crate::vault::key_handle().context("当前已锁定")?;
    ensure!(current.same_unlock(key), "工作区已变化");
    let path = crate::storage::slot_file_for(key.slot(), FILE)?;
    let sealed =
        crate::vault::seal_path_bytes(key, &path, "config", &serde_json::to_vec(approvals)?)?;
    ensure!(sealed.len() as u64 <= MAX_BYTES, "设备认证记录已满");
    crate::storage::atomic_write(&path, |file| {
        file.write_all(&sealed)?;
        Ok(())
    })
}

enum Scan {
    Refresh,
    Approve(Device),
}

#[derive(Default)]
pub(super) struct DeviceSettings {
    scan: Option<Receiver<Result<Vec<Device>>>>,
    pending: Option<Scan>,
    last_scan: Option<Instant>,
    devices: Vec<Device>,
    notice: String,
    scan_error: Option<String>,
}

impl DeviceSettings {
    fn start_scan(&mut self, ctx: &egui::Context, scan: Scan) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::is903_writer::scan());
            ctx.request_repaint();
        });
        self.scan = Some(rx);
        self.pending = Some(scan);
    }

    fn poll(&mut self, key: &crate::vault::KeyHandle, allow_change: bool) {
        let Some(receiver) = &self.scan else { return };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(anyhow::anyhow!("设备检测任务已退出")),
        };
        let pending = self.pending.take();
        self.scan = None;
        self.last_scan = Some(Instant::now());
        match result {
            Ok(devices) => {
                self.scan_error = None;
                if let Some(Scan::Approve(expected)) = pending {
                    let result = (|| {
                        ensure!(allow_change, "当前无法修改认证记录");
                        ensure!(
                            devices.contains(&expected) && unique_serial(&devices, &expected),
                            "设备已更换、拔出或序列号有冲突"
                        );
                        let mut approved = approvals(key)?;
                        approved.insert(Identity::from(&expected));
                        save(key, &approved)
                    })();
                    self.notice = match result {
                        Ok(()) => "设备已认证。".into(),
                        Err(error) => format!("认证失败：{error:#}"),
                    };
                }
                self.devices = devices;
            }
            Err(error) => {
                self.devices.clear();
                self.scan_error = Some(format!("检测失败：{error:#}"));
            }
        }
    }

    pub(super) fn ui(&mut self, ui: &mut egui::Ui, allow_change: bool) {
        ui.strong("IS903 设备认证");
        let Some(key) = crate::vault::key_handle() else {
            *self = Self::default();
            ui.weak("请先启用应用锁并解锁。");
            return;
        };
        self.poll(&key, allow_change);
        let scan_due = self
            .last_scan
            .is_none_or(|last| last.elapsed() >= SCAN_INTERVAL);
        let refresh = ui.add_enabled(self.scan.is_none(), egui::Button::new("重新检测"));
        if self.scan.is_none() && (scan_due || refresh.clicked()) {
            self.start_scan(ui.ctx(), Scan::Refresh);
        }
        ui.ctx().request_repaint_after(SCAN_INTERVAL);

        let approved = match approvals(&key) {
            Ok(approved) => approved,
            Err(error) => {
                ui.colored_label(
                    crate::theme::danger(),
                    format!("无法读取认证记录：{error:#}"),
                );
                return;
            }
        };
        if let Some(error) = &self.scan_error {
            ui.colored_label(crate::theme::danger(), error);
        } else if self.devices.is_empty() {
            ui.weak(if self.scan.is_some() {
                "正在检测 IS903 虚拟光盘…"
            } else {
                "未检测到带序列号的 IS903 虚拟光盘。"
            });
        }

        let mut approve = None;
        for device in &self.devices {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("IS903 · {} · {}", device.serial, device.disk));
                if !unique_serial(&self.devices, device) {
                    ui.colored_label(crate::theme::danger(), "序列号冲突");
                } else if approved.contains(&Identity::from(device)) {
                    ui.label("已认证");
                } else {
                    ui.label("未认证");
                    if ui
                        .add_enabled(
                            allow_change && self.scan.is_none(),
                            egui::Button::new("认证此设备"),
                        )
                        .clicked()
                    {
                        approve = Some(device.clone());
                    }
                }
            });
        }
        if let Some(device) = approve {
            self.notice.clear();
            self.start_scan(ui.ctx(), Scan::Approve(device));
        }
        let mut revoke = None;
        for identity in &approved {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "认证记录 · {} · 版本 {}",
                    identity.serial, identity.revision
                ));
                if ui
                    .add_enabled(allow_change, egui::Button::new("撤销认证"))
                    .clicked()
                {
                    revoke = Some(identity.clone());
                }
            });
        }
        if let Some(identity) = revoke {
            self.pending = None;
            let mut approved = approved;
            approved.remove(&identity);
            self.notice = match save(&key, &approved) {
                Ok(()) => "已撤销设备认证。".into(),
                Err(error) => format!("撤销失败：{error:#}"),
            };
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
    }
}

fn unique_serial(devices: &[Device], expected: &Device) -> bool {
    devices
        .iter()
        .filter(|d| d.serial == expected.serial)
        .count()
        == 1
}

/// 环境监控只读取已经批准的 IS903 身份；读取失败时返回空集合，按未授权处理。
/// 这项豁免在策略层仅对移动存储功能有效，不放行同一设备的其他接口。
pub(super) fn environment_approved_identities() -> BTreeSet<crate::environment::DeviceIdentity> {
    crate::vault::key_handle()
        .and_then(|key| approvals(&key).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|identity| crate::environment::DeviceIdentity {
            serial: identity.serial,
            revision: identity.revision,
        })
        .collect()
}

pub(super) fn approved_targets(key: &crate::vault::KeyHandle) -> Result<Vec<Device>> {
    let approvals = approvals(key)?;
    let devices = crate::is903_writer::scan()?;
    Ok(devices
        .iter()
        .filter(|target| {
            unique_serial(&devices, target) && approvals.contains(&Identity::from(*target))
        })
        .cloned()
        .collect())
}

pub(super) fn require_approved(key: &crate::vault::KeyHandle, target: &Device) -> Result<()> {
    ensure!(
        approvals(key)?.contains(&Identity::from(target)),
        "IS903 设备未经认证"
    );
    Ok(())
}
