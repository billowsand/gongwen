//! 进程级环境安全监控。平台线程只提交完整快照，界面线程负责策略和锁定。

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

#[cfg_attr(windows, allow(unsafe_code))]
mod capture;
mod policy;

pub(crate) use capture::CaptureGuard;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

pub(crate) use policy::{Violation, evaluate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkInterfaceKind {
    Ethernet,
    Wifi,
    Cellular,
    Vpn,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetworkInterface {
    /// 仅用于本次运行内关联和诊断，不写入安全日志。
    pub id: String,
    pub kind: NetworkInterfaceKind,
    pub loopback: bool,
    pub link_up: bool,
    pub non_loopback_address: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NetworkSnapshot {
    pub interfaces: Vec<NetworkInterface>,
    pub default_route: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceClass {
    RemovableStorage,
    PortableMedia,
    HumanInterface,
    Imaging,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DeviceIdentity {
    pub serial: String,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceInfo {
    /// 平台设备实例标识，仅用于事件归并。
    pub id: String,
    pub class: DeviceClass,
    /// 现有 IS903 导出授权只在这两个字段完整匹配时生效。
    pub identity: Option<DeviceIdentity>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DeviceSnapshot {
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone)]
pub(crate) struct Observed<T> {
    pub value: T,
    pub observed_at: Instant,
    pub healthy: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct EnvironmentSnapshot {
    pub network: Option<Observed<NetworkSnapshot>>,
    pub devices: Option<Observed<DeviceSnapshot>>,
}

impl EnvironmentSnapshot {
    pub(crate) fn summary(&self) -> String {
        let network = self
            .network
            .as_ref()
            .map_or("网卡状态等待采集".to_owned(), |n| {
                if let Some(error) = &n.error {
                    format!("网络监控异常：{error}")
                } else if !n.healthy {
                    "网络监控异常".to_owned()
                } else {
                    let adapters = n.value.interfaces.iter().filter(|i| !i.loopback).count();
                    let linked = n
                        .value
                        .interfaces
                        .iter()
                        .filter(|i| !i.loopback && i.link_up)
                        .count();
                    let wireless = n
                        .value
                        .interfaces
                        .iter()
                        .filter(|i| {
                            !i.loopback
                                && matches!(
                                    i.kind,
                                    crate::environment::NetworkInterfaceKind::Wifi
                                        | crate::environment::NetworkInterfaceKind::Cellular
                                )
                        })
                        .count();
                    format!("非回环接口 {adapters} 个，已连接 {linked} 个，无线 {wireless} 个")
                }
            });
        let devices = self
            .devices
            .as_ref()
            .map_or("设备状态等待采集".to_owned(), |d| {
                if let Some(error) = &d.error {
                    format!("设备监控异常：{error}")
                } else if !d.healthy {
                    "设备监控异常".to_owned()
                } else {
                    format!("受监控外设 {} 个", d.value.devices.len())
                }
            });
        format!("{network}；{devices}")
    }
}

pub(crate) enum MonitorEvent {
    Network(Result<NetworkSnapshot, String>),
    Devices(Result<DeviceSnapshot, String>),
}

pub(crate) struct Monitor {
    receiver: mpsc::Receiver<MonitorEvent>,
    stop: Arc<AtomicBool>,
    snapshot: EnvironmentSnapshot,
}

impl Monitor {
    #[cfg(test)]
    pub(crate) fn disabled_for_test() -> Self {
        let (_sender, receiver) = mpsc::channel();
        Self {
            receiver,
            stop: Arc::new(AtomicBool::new(false)),
            snapshot: EnvironmentSnapshot::default(),
        }
    }

    pub(crate) fn start(ctx: &egui::Context) -> Self {
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let wake = ctx.clone();
        std::thread::spawn(move || {
            platform::run(sender, thread_stop, wake);
        });
        Self {
            receiver,
            stop,
            snapshot: EnvironmentSnapshot::default(),
        }
    }

    /// 每帧非阻塞取空事件。心跳快照也会刷新观测时间，供失效关闭判断。
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.receiver.try_recv() {
            changed = true;
            match event {
                MonitorEvent::Network(Ok(value)) => {
                    self.snapshot.network = Some(Observed {
                        value,
                        observed_at: Instant::now(),
                        healthy: true,
                        error: None,
                    });
                }
                MonitorEvent::Network(Err(error)) => {
                    let value = self
                        .snapshot
                        .network
                        .take()
                        .map_or_else(NetworkSnapshot::default, |old| old.value);
                    self.snapshot.network = Some(Observed {
                        value,
                        observed_at: Instant::now(),
                        healthy: false,
                        error: Some(error),
                    });
                }
                MonitorEvent::Devices(Ok(value)) => {
                    self.snapshot.devices = Some(Observed {
                        value,
                        observed_at: Instant::now(),
                        healthy: true,
                        error: None,
                    });
                }
                MonitorEvent::Devices(Err(error)) => {
                    let value = self
                        .snapshot
                        .devices
                        .take()
                        .map_or_else(DeviceSnapshot::default, |old| old.value);
                    self.snapshot.devices = Some(Observed {
                        value,
                        observed_at: Instant::now(),
                        healthy: false,
                        error: Some(error),
                    });
                }
            }
        }
        changed
    }

    pub(crate) fn snapshot(&self) -> &EnvironmentSnapshot {
        &self.snapshot
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

/// 非桌面平台的编译兜底不伪造安全状态；严格策略会拒绝解锁。
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod platform {
    use super::*;
    pub(crate) fn run(
        sender: mpsc::Sender<MonitorEvent>,
        _stop: Arc<AtomicBool>,
        wake: egui::Context,
    ) {
        let _ = sender.send(MonitorEvent::Network(Err("当前平台不支持网络监控".into())));
        let _ = sender.send(MonitorEvent::Devices(Err("当前平台不支持设备监控".into())));
        wake.request_repaint();
    }
}
