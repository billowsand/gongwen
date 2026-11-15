//! 纯策略判定：平台后端不提供“安全”结论，只提供事实快照。

use super::{DeviceClass, DeviceIdentity, EnvironmentSnapshot, Observed};
use crate::models::{DevicePolicy, EnvironmentPolicy};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Violation {
    Device(DeviceClass),
    DeviceMonitorUnavailable,
    InitialDeviceSampleRequired,
    CaptureProtectionFailed,
}

impl Violation {
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Device(DeviceClass::RemovableStorage) => "检测到未授权移动存储设备".into(),
            Self::Device(DeviceClass::PortableMedia) => "检测到 WPD / MTP / PTP 便携设备".into(),
            Self::Device(DeviceClass::HumanInterface) => "检测到未授权人机接口设备".into(),
            Self::Device(DeviceClass::Imaging) => "检测到未授权图像采集设备".into(),
            Self::Device(DeviceClass::Other) => "检测到未授权外部设备".into(),
            Self::DeviceMonitorUnavailable => "设备环境监控失效或快照过期".into(),
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
    if policy.devices.any_selected()
        && observed_unhealthy(snapshot.devices.as_ref(), now, policy.fail_closed)
    {
        return Some(Violation::DeviceMonitorUnavailable);
    }
    evaluate_observed_risk(snapshot, policy, trusted_devices)
}

/// 首次启用/修改策略时的准入判定：无论用户是否选择「失效时立即锁定」，
/// 都必须先拿到健康且未过期的外设样本。运行期后端故障是否立即
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
    if policy.fail_closed
        && policy.devices.any_selected()
        && snapshot.devices.as_ref().is_some_and(|item| !item.healthy)
    {
        return Some(Violation::DeviceMonitorUnavailable);
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
    use crate::environment::DeviceInfo;

    fn snapshot() -> EnvironmentSnapshot {
        EnvironmentSnapshot {
            devices: Some(Observed {
                value: Default::default(),
                observed_at: Instant::now(),
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
    fn no_peripheral_categories_means_device_health_does_not_lock_at_runtime() {
        let policy: EnvironmentPolicy = serde_json::from_str(
            r#"{
            "enabled": true, "network": "Offline",
            "devices": {"removable_storage": false, "portable_media": false}
        }"#,
        )
        .unwrap();
        let missing = EnvironmentSnapshot::default();
        assert_eq!(
            evaluate(&missing, &policy, &BTreeSet::new(), true, Instant::now()),
            None
        );
    }

    #[test]
    fn initial_admission_requires_fresh_device_sample_when_not_fail_closed() {
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
            Some(Violation::InitialDeviceSampleRequired),
            "首次启用不能因为关闭失效锁定就跳过初始采样"
        );

        let state = snapshot();
        assert_eq!(
            initial_admission_violation(&state, &policy, &BTreeSet::new(), true, Instant::now(),),
            None
        );

        let mut stale = snapshot();
        stale.devices.as_mut().expect("devices").observed_at =
            Instant::now() - SNAPSHOT_MAX_AGE - Duration::from_secs(1);
        assert_eq!(
            initial_admission_violation(&stale, &policy, &BTreeSet::new(), true, Instant::now(),),
            Some(Violation::InitialDeviceSampleRequired)
        );
    }

    #[test]
    fn stale_or_failed_snapshots_fail_closed() {
        let policy = EnvironmentPolicy {
            enabled: true,
            fail_closed: true,
            ..Default::default()
        };
        let mut state = snapshot();
        state.devices.as_mut().expect("devices").observed_at =
            Instant::now() - SNAPSHOT_MAX_AGE - Duration::from_secs(1);
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::DeviceMonitorUnavailable)
        );
        state.devices.as_mut().expect("devices").observed_at = Instant::now();
        state.devices.as_mut().expect("devices").healthy = false;
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::DeviceMonitorUnavailable)
        );
    }

    #[test]
    fn failed_backend_does_not_erase_last_known_violation() {
        let policy = EnvironmentPolicy {
            enabled: true,
            fail_closed: false,
            ..Default::default()
        };
        let mut state = snapshot();
        let devices = state.devices.as_mut().unwrap();
        devices.healthy = false;
        devices.value.devices.push(DeviceInfo {
            id: "storage".into(),
            class: DeviceClass::RemovableStorage,
            identity: None,
        });
        assert_eq!(
            evaluate(&state, &policy, &BTreeSet::new(), true, Instant::now()),
            Some(Violation::Device(DeviceClass::RemovableStorage))
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
        let mut state = snapshot();
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
        let mut state = snapshot();
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
