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
    InitialNetworkSampleRequired,
    InitialDeviceSampleRequired,
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
            Self::InitialNetworkSampleRequired => "尚未取得有效的网络采样，请稍候重试".into(),
            Self::InitialDeviceSampleRequired => "尚未取得有效的外设采样，请稍候重试".into(),
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
    // 三种例外分别对应：可存在、可常驻链路、可持有自身地址。
    // 只有平台确认的系统服务接口（System）享受全部三项。Offline 下虚拟接口
    // 与 VPN 可存在、可常驻链路，但取得可路由地址即视为联网；NoAdapters 下
    // 虚拟接口与 VPN 仍算网卡存在。默认路由始终单独判定。
    let confirmed_system_service = interface.kind == super::NetworkInterfaceKind::System;
    let persistent_link = matches!(
        interface.kind,
        super::NetworkInterfaceKind::System
            | super::NetworkInterfaceKind::Virtual
            | super::NetworkInterfaceKind::Vpn
    );
    match policy {
        NetworkPolicy::NoAdapters if confirmed_system_service => None,
        NetworkPolicy::NoAdapters => Some(Violation::AdapterPresent(interface.id.clone())),
        // AirDrop / AWDL / LLW / ANPI 等已确认的系统服务接口常驻且可能只有
        // 链路本地地址；它们本身不代表外网可达。真正的默认路由仍由快照的
        // 独立字段判定。
        NetworkPolicy::Offline if confirmed_system_service => None,
        // 虚拟网桥、Hyper-V / WSL 网卡与 VPN 的链路常常常驻为真：没有可路由
        // 地址时不能据此认定联网；一旦取得地址，即使没有默认路由也可能访问
        // 直连网段，必须锁定。
        NetworkPolicy::Offline if persistent_link => {
            if interface.non_loopback_address {
                Some(Violation::NetworkAddress(interface.id.clone()))
            } else {
                None
            }
        }
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

/// 首次启用/修改策略时的准入判定：无论用户是否选择「失效时立即锁定」，
/// 都必须先拿到网络与外设两份健康且未过期的样本。运行期后端故障是否立即
/// 锁定仍由 `fail_closed` 决定，两者不能共用一个开关。
pub(crate) fn initial_admission_violation(
    snapshot: &EnvironmentSnapshot,
    policy: &EnvironmentPolicy,
    trusted_devices: &BTreeSet<DeviceIdentity>,
    capture_protection_healthy: bool,
    now: Instant,
) -> Option<Violation> {
    if !policy.enabled {
        return None;
    }
    if observed_unhealthy(snapshot.network.as_ref(), now, true) {
        return Some(Violation::InitialNetworkSampleRequired);
    }
    if observed_unhealthy(snapshot.devices.as_ref(), now, true) {
        return Some(Violation::InitialDeviceSampleRequired);
    }
    evaluate(
        snapshot,
        policy,
        trusted_devices,
        capture_protection_healthy,
        now,
    )
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
    fn offline_mode_allows_idle_virtual_link_but_rejects_its_address() {
        let policy = EnvironmentPolicy {
            enabled: true,
            ..Default::default()
        };
        let mut bridge = interface("bridge0");
        bridge.kind = super::super::NetworkInterfaceKind::Virtual;
        bridge.link_up = true;
        bridge.non_loopback_address = false;
        let mut state = snapshot(NetworkSnapshot {
            interfaces: vec![bridge.clone()],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            None,
            "常驻但未取得可路由地址的虚拟网桥不单独判为联网"
        );

        bridge.non_loopback_address = true;
        state.network.as_mut().expect("network").value.interfaces = vec![bridge];
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::NetworkAddress("bridge0".into())),
            "有可路由地址的虚拟网桥即使没有默认路由也可能访问直连网段"
        );
    }

    #[test]
    fn no_adapter_mode_counts_virtual_interface_as_adapter() {
        let policy = EnvironmentPolicy {
            enabled: true,
            network: NetworkPolicy::NoAdapters,
            ..Default::default()
        };
        let mut virtual_adapter = interface("vEthernet (WSL)");
        virtual_adapter.kind = super::super::NetworkInterfaceKind::Virtual;
        let state = snapshot(NetworkSnapshot {
            interfaces: vec![virtual_adapter],
            default_route: false,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::AdapterPresent("vEthernet (WSL)".into())),
            "只有平台确认的系统服务接口才享受存在例外"
        );
    }

    #[test]
    fn initial_admission_requires_both_fresh_samples_when_not_fail_closed() {
        let policy = EnvironmentPolicy {
            enabled: true,
            fail_closed: false,
            ..Default::default()
        };
        assert_eq!(
            initial_admission_violation(
                &EnvironmentSnapshot::default(),
                &policy,
                &BTreeSet::new(),
                true,
                Instant::now(),
            ),
            Some(Violation::InitialNetworkSampleRequired),
            "首次启用不能因为关闭失效锁定就跳过初始采样"
        );

        let state = snapshot(NetworkSnapshot::default());
        assert_eq!(
            initial_admission_violation(&state, &policy, &BTreeSet::new(), true, Instant::now(),),
            None
        );

        let mut no_devices = snapshot(NetworkSnapshot::default());
        no_devices.devices = None;
        assert_eq!(
            initial_admission_violation(
                &no_devices,
                &policy,
                &BTreeSet::new(),
                true,
                Instant::now(),
            ),
            Some(Violation::InitialDeviceSampleRequired)
        );

        let mut stale = snapshot(NetworkSnapshot::default());
        stale.network.as_mut().expect("network").observed_at =
            Instant::now() - SNAPSHOT_MAX_AGE - Duration::from_secs(1);
        assert_eq!(
            initial_admission_violation(&stale, &policy, &BTreeSet::new(), true, Instant::now(),),
            Some(Violation::InitialNetworkSampleRequired)
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
