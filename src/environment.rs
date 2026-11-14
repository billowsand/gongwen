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

pub(crate) use policy::{Violation, evaluate, evaluate_observed_risk, initial_admission_violation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkInterfaceKind {
    Ethernet,
    Wifi,
    Cellular,
    Vpn,
    /// 平台已确认的常驻系统服务接口（如 macOS AWDL、LLW、ANPI）。
    /// 只有这一类接口享受「可存在 / 可常驻链路 / 可持有自身地址」的例外。
    System,
    /// 虚拟或软件接口（bridge、veth、Hyper-V、WSL 等）。常驻链路本身
    /// 不判为联网，但取得可路由地址必须按普通接口处理。
    Virtual,
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
                    let interfaces = n.value.interfaces.iter().filter(|i| !i.loopback).map(|i| {
                        let link = if i.link_up { "有链路" } else { "无链路" };
                        let address = if i.non_loopback_address { "有非回环地址" } else { "无非回环地址" };
                        format!("{}（{link}，{address}）", i.id)
                    }).collect::<Vec<_>>().join("、");
                    let route = if n.value.default_route { "有默认路由" } else { "无默认路由" };
                    format!("非回环接口 {adapters} 个，已连接 {linked} 个，无线 {wireless} 个，{route}。{interfaces}")
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
                    [
                        (DeviceClass::RemovableStorage, "移动存储"),
                        (DeviceClass::PortableMedia, "便携设备"),
                        (DeviceClass::HumanInterface, "人机接口"),
                        (DeviceClass::Imaging, "图像采集"),
                        (DeviceClass::Other, "其他"),
                    ]
                    .into_iter()
                    .map(|(class, label)| {
                        let count = d
                            .value
                            .devices
                            .iter()
                            .filter(|device| device.class == class)
                            .count();
                        format!("{label} {count} 项")
                    })
                    .collect::<Vec<_>>()
                    .join("、")
                }
            });
        format!("{network}；{devices}")
    }
}

pub(crate) enum MonitorEvent {
    Network(Result<NetworkSnapshot, String>),
    Devices(Result<DeviceSnapshot, String>),
}

pub(crate) type TimedEvent = (Instant, MonitorEvent);

pub(crate) struct Monitor {
    receiver: mpsc::Receiver<TimedEvent>,
    stop: Arc<AtomicBool>,
    snapshot: EnvironmentSnapshot,
}

impl Monitor {
    #[cfg(test)]
    pub(crate) fn disabled_for_test() -> Self {
        Self::test_channel().0
    }

    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, mpsc::Sender<TimedEvent>) {
        let (sender, receiver) = mpsc::channel();
        (
            Self {
                receiver,
                stop: Arc::new(AtomicBool::new(false)),
                snapshot: EnvironmentSnapshot::default(),
            },
            sender,
        )
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

    /// 逐条应用队列里的快照，调用方可在下一条覆盖前记录违规。
    pub(crate) fn poll_next(&mut self) -> bool {
        let Ok((observed_at, event)) = self.receiver.try_recv() else {
            return false;
        };
        match event {
            MonitorEvent::Network(result) => {
                update_observation(&mut self.snapshot.network, result, observed_at)
            }
            MonitorEvent::Devices(result) => {
                update_observation(&mut self.snapshot.devices, result, observed_at)
            }
        }
        true
    }

    pub(crate) fn snapshot(&self) -> &EnvironmentSnapshot {
        &self.snapshot
    }
}

fn update_observation<T: Default>(
    slot: &mut Option<Observed<T>>,
    result: Result<T, String>,
    observed_at: Instant,
) {
    let (value, error) = match result {
        Ok(value) => (value, None),
        Err(error) => (
            slot.take().map_or_else(T::default, |old| old.value),
            Some(error),
        ),
    };
    *slot = Some(Observed {
        value,
        observed_at,
        healthy: error.is_none(),
        error,
    });
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
        sender: mpsc::Sender<TimedEvent>,
        _stop: Arc<AtomicBool>,
        wake: egui::Context,
    ) {
        let _ = sender.send((
            Instant::now(),
            MonitorEvent::Network(Err("当前平台不支持网络监控".into())),
        ));
        let _ = sender.send((
            Instant::now(),
            MonitorEvent::Devices(Err("当前平台不支持设备监控".into())),
        ));
        wake.request_repaint();
    }
}
