//! 纯策略判定：平台后端不提供“安全”结论，只提供事实快照。

use super::{DeviceClass, DeviceIdentity, EnvironmentSnapshot, NetworkInterface, Observed};
use crate::models::{DevicePolicy, EnvironmentPolicy, NetworkPolicy};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Violation {
    AdapterPresent(String),
    NetworkLink(String),
    NetworkAddress(String),
    DefaultRoute,
    Device(DeviceClass),
    NetworkMonitorUnavailable,
    DeviceMonitorUnavailable,
    CaptureProtectionFailed,
}

impl Violation {
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::AdapterPresent(id) => format!("检测到禁止存在的网络接口：{id}"),
            Self::NetworkLink(id) => format!("网络接口链路已连接：{id}"),
            Self::NetworkAddress(id) => format!("网络接口已取得非回环地址：{id}"),
            Self::DefaultRoute => "检测到默认网络路由".into(),
            Self::Device(DeviceClass::RemovableStorage) => "检测到未授权移动存储设备".into(),
            Self::Device(DeviceClass::PortableMedia) => "检测到 WPD / MTP / PTP 便携设备".into(),
            Self::Device(DeviceClass::HumanInterface) => "检测到未授权人机接口设备".into(),
            Self::Device(DeviceClass::Imaging) => "检测到未授权图像采集设备".into(),
            Self::Device(DeviceClass::Other) => "检测到未授权外部设备".into(),
            Self::NetworkMonitorUnavailable => "网络环境监控失效或快照过期".into(),
            Self::DeviceMonitorUnavailable => "设备环境监控失效或快照过期".into(),
            Self::CaptureProtectionFailed => "窗口捕获排除未生效".into(),
        }
    }
}

fn observed_unhealthy<T>(observed: Option<&Observed<T>>, now: Instant, fail_closed: bool) -> bool {
    fail_closed
        && observed.is_none_or(|value| {
            !value.healthy || now.saturating_duration_since(value.observed_at) > SNAPSHOT_MAX_AGE
        })
}

fn device_selected(policy: DevicePolicy, class: DeviceClass) -> bool {
    match class {
        DeviceClass::RemovableStorage => policy.removable_storage,
        DeviceClass::PortableMedia => policy.portable_media,
        DeviceClass::HumanInterface => policy.human_interface,
        DeviceClass::Imaging => policy.imaging,
        DeviceClass::Other => policy.other,
    }
}

fn adapter_violation(interface: &NetworkInterface, policy: NetworkPolicy) -> Option<Violation> {
    if interface.loopback {
        return None;
    }
    match policy {
        NetworkPolicy::NoAdapters if interface.kind == super::NetworkInterfaceKind::System => None,
        NetworkPolicy::NoAdapters => Some(Violation::AdapterPresent(interface.id.clone())),
        // AirDrop / AWDL / LLW 等系统服务接口常驻且可能只有链路本地地址；
        // 它们本身不代表外网可达。真正的默认路由仍由快照的独立字段判定。
        NetworkPolicy::Offline if interface.kind == super::NetworkInterfaceKind::System => None,
        // VPN 接口的“链路”常常常驻为真，只有取得可路由地址才视为联网。
        NetworkPolicy::Offline
            if interface.kind == super::NetworkInterfaceKind::Vpn
                && interface.non_loopback_address =>
        {
            Some(Violation::NetworkAddress(interface.id.clone()))
        }
        NetworkPolicy::Offline if interface.kind == super::NetworkInterfaceKind::Vpn => None,
        NetworkPolicy::Offline if interface.link_up => {
            Some(Violation::NetworkLink(interface.id.clone()))
        }
        NetworkPolicy::Offline if interface.non_loopback_address => {
            Some(Violation::NetworkAddress(interface.id.clone()))
        }
        NetworkPolicy::Offline => None,
    }
}

pub(crate) fn evaluate(
    snapshot: &EnvironmentSnapshot,
    policy: &EnvironmentPolicy,
    trusted_devices: &BTreeSet<DeviceIdentity>,
    capture_protection_healthy: bool,
    now: Instant,
) -> Option<Violation> {
    if !policy.enabled {
        return None;
    }
    if policy.capture_protection && !capture_protection_healthy {
        return Some(Violation::CaptureProtectionFailed);
    }
    if observed_unhealthy(snapshot.network.as_ref(), now, policy.fail_closed) {
        return Some(Violation::NetworkMonitorUnavailable);
    }
    if observed_unhealthy(snapshot.devices.as_ref(), now, policy.fail_closed) {
        return Some(Violation::DeviceMonitorUnavailable);
    }
    evaluate_observed_risk(snapshot, policy, trusted_devices)
}

pub(crate) fn evaluate_observed_risk(
    snapshot: &EnvironmentSnapshot,
    policy: &EnvironmentPolicy,
    trusted_devices: &BTreeSet<DeviceIdentity>,
) -> Option<Violation> {
    if policy.fail_closed {
        if snapshot.network.as_ref().is_some_and(|item| !item.healthy) {
            return Some(Violation::NetworkMonitorUnavailable);
        }
        if snapshot.devices.as_ref().is_some_and(|item| !item.healthy) {
            return Some(Violation::DeviceMonitorUnavailable);
        }
    }
    if let Some(network) = &snapshot.network {
        if let Some(violation) = network
            .value
            .interfaces
            .iter()
            .find_map(|interface| adapter_violation(interface, policy.network))
        {
            return Some(violation);
        }
        if network.value.default_route {
            return Some(Violation::DefaultRoute);
        }
    }
    if let Some(devices) = &snapshot.devices
        && let Some(device) = devices.value.devices.iter().find(|device| {
            if !device_selected(policy.devices, device.class) {
                return false;
            }
            let trusted_storage = device.class == DeviceClass::RemovableStorage
                && device.identity.as_ref().is_some_and(|identity| {
                    trusted_devices.contains(identity)
                        && devices
                            .value
                            .devices
                            .iter()
                            .filter(|candidate| {
                                candidate.class == DeviceClass::RemovableStorage
                                    && candidate.identity.as_ref() == Some(identity)
                            })
                            .map(|candidate| candidate.id.as_str())
                            .collect::<BTreeSet<_>>()
                            .len()
                            == 1
                });
            !trusted_storage
        })
    {
        return Some(Violation::Device(device.class));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{DeviceInfo, NetworkSnapshot};

    fn interface(id: &str) -> NetworkInterface {
        NetworkInterface {
            id: id.into(),
            kind: super::super::NetworkInterfaceKind::Ethernet,
            loopback: false,
            link_up: false,
            non_loopback_address: false,
        }
    }

    fn snapshot(network: NetworkSnapshot) -> EnvironmentSnapshot {
        let now = Instant::now();
        EnvironmentSnapshot {
            network: Some(Observed {
                value: network,
                observed_at: now,
                healthy: true,
                error: None,
            }),
            devices: Some(Observed {
                value: Default::default(),
                observed_at: now,
                healthy: true,
                error: None,
            }),
        }
    }

    #[test]
    fn disabled_policy_does_not_lock_on_unknown_backends() {
        let policy = EnvironmentPolicy::default();
        assert_eq!(
            evaluate(
                &EnvironmentSnapshot::default(),
                &policy,
                &BTreeSet::new(),
                false,
                Instant::now(),
            ),
            None
        );
    }

    #[test]
    fn no_adapter_mode_rejects_even_disconnected_adapter() {
        let policy = EnvironmentPolicy {
            enabled: true,
            network: NetworkPolicy::NoAdapters,
            ..Default::default()
        };
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![interface("net0")],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::AdapterPresent("net0".into()))
        );
    }

    #[test]
    fn offline_mode_rejects_link_without_address() {
        let policy = EnvironmentPolicy {
            enabled: true,
            network: NetworkPolicy::Offline,
            ..Default::default()
        };
        let mut connected = interface("en0");
        connected.link_up = true;
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![connected],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::NetworkLink("en0".into()))
        );
    }

    #[test]
    fn offline_mode_allows_loopback_and_disconnected_adapter() {
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let mut local = interface("lo0");
        local.loopback = true;
        local.link_up = true;
        local.non_loopback_address = true;
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![local, interface("en0")],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            None
        );
    }

    #[test]
    fn offline_mode_ignores_system_service_interfaces() {
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let mut service = interface("awdl0");
        service.kind = super::super::NetworkInterfaceKind::System;
        service.link_up = true;
        service.non_loopback_address = true;
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![service.clone()],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            None
        );
        let no_adapters = EnvironmentPolicy {
            enabled: true,
            network: NetworkPolicy::NoAdapters,
            ..Default::default()
        };
        assert_eq!(
            evaluate(&state, &no_adapters, &BTreeSet::new(), true, Instant::now()),
            None,
            "系统服务接口不是用户网卡"
        );
        service.kind = super::super::NetworkInterfaceKind::Ethernet;
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![service],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &no_adapters, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::AdapterPresent("awdl0".into()))
        );
    }

    #[test]
    fn offline_mode_requires_a_routable_address_for_vpn() {
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let mut vpn = interface("utun4");
        vpn.kind = super::super::NetworkInterfaceKind::Vpn;
        vpn.link_up = true;
        vpn.non_loopback_address = false;
        let mut state = snapshot(NetworkSnapshot {
            interfaces: vec![vpn.clone()],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            None,
            "只有链路本地地址的常驻 VPN 接口不代表可联网"
        );
        vpn.non_loopback_address = true;
        state.network.as_mut().expect("network").value.interfaces = vec![vpn];
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::NetworkAddress("utun4".into()))
        );
    }

    #[test]
    fn offline_mode_rejects_connected_physical_interface() {
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let mut ethernet = interface("en0");
        ethernet.kind = super::super::NetworkInterfaceKind::Ethernet;
        ethernet.link_up = true;
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![ethernet],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::NetworkLink("en0".into()))
        );
    }

    #[test]
    fn default_route_is_rejected_without_link_details() {
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![],
            default_route: true,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::DefaultRoute)
        );
    }

    #[test]
    fn stale_or_failed_snapshots_fail_closed() {
        let policy = EnvironmentPolicy {
            enabled: true,
            fail_closed: true,
            ..Default::default()
        };
        let mut state = snapshot(NetworkSnapshot::default());
        state.network.as_mut().expect("network").observed_at =
            Instant::now() - SNAPSHOT_MAX_AGE - Duration::from_secs(1);
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::NetworkMonitorUnavailable)
        );
        state.network.as_mut().expect("network").observed_at = Instant::now();
        state.network.as_mut().expect("network").healthy = false;
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::NetworkMonitorUnavailable)
        );
    }

    #[test]
    fn failed_backend_does_not_erase_last_known_violation() {
        let policy = EnvironmentPolicy {
            enabled: true,
            fail_closed: false,
            ..Default::default()
        };
        let mut connected = interface("en0");
        connected.link_up = true;
        let mut state = snapshot(NetworkSnapshot {
            interfaces: vec![connected],
            default_route: false,
        });
        state.network.as_mut().expect("network").healthy = false;
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::NetworkLink("en0".into()))
        );
    }

    #[test]
    fn approved_identity_only_exempts_matching_storage_device() {
        let identity = DeviceIdentity {
            serial: "approved".into(),
            revision: 0x100,
        };
        let trusted = BTreeSet::from([identity.clone()]);
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let mut state = snapshot(NetworkSnapshot::default());
        state.devices.as_mut().expect("devices").value.devices = vec![
            DeviceInfo {
                id: "approved-storage".into(),
                class: DeviceClass::RemovableStorage,
                identity: Some(identity.clone()),
            },
            DeviceInfo {
                id: "same-device-media-function".into(),
                class: DeviceClass::PortableMedia,
                identity: Some(identity),
            },
        ];
        assert_eq!(
            evaluate(&state, &policy, &trusted, true, Instant::now()),
            Some(Violation::Device(DeviceClass::PortableMedia))
        );
        state.devices.as_mut().expect("devices").value.devices.pop();
        assert_eq!(
            evaluate(&state, &policy, &trusted, true, Instant::now()),
            None,
            "唯一的获批 IS903 存储功能不应触发锁定"
        );
    }

    #[test]
    fn duplicate_approved_storage_identities_are_not_trusted() {
        let identity = DeviceIdentity {
            serial: "duplicated".into(),
            revision: 0x100,
        };
        let trusted = BTreeSet::from([identity.clone()]);
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let mut state = snapshot(NetworkSnapshot::default());
        state.devices.as_mut().expect("devices").value.devices = vec![
            DeviceInfo {
                id: "storage-a".into(),
                class: DeviceClass::RemovableStorage,
                identity: Some(identity.clone()),
            },
            DeviceInfo {
                id: "storage-b".into(),
                class: DeviceClass::RemovableStorage,
                identity: Some(identity),
            },
        ];
        assert_eq!(
            evaluate(&state, &policy, &trusted, true, Instant::now()),
            Some(Violation::Device(DeviceClass::RemovableStorage))
        );
    }
}
