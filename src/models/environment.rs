//! 环境安全检测配置。只保存策略，不保存设备名或序列号等现场信息。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DevicePolicy {
    pub removable_storage: bool,
    pub portable_media: bool,
    pub human_interface: bool,
    pub imaging: bool,
    pub other: bool,
}

impl DevicePolicy {
    pub(crate) fn any_selected(self) -> bool {
        self.removable_storage
            || self.portable_media
            || self.human_interface
            || self.imaging
            || self.other
    }
}

impl Default for DevicePolicy {
    fn default() -> Self {
        Self {
            removable_storage: true,
            portable_media: true,
            human_interface: false,
            imaging: false,
            other: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct EnvironmentPolicy {
    /// 默认关闭，由用户选择启用外设感知。
    pub enabled: bool,
    pub devices: DevicePolicy,
    /// 严格模式：后端失败、快照缺失或过期均按不安全处理。
    pub fail_closed: bool,
    /// Windows 窗口捕获排除；不是截屏动作检测，其他平台忽略。
    pub capture_protection: bool,
}

impl Default for EnvironmentPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            devices: DevicePolicy::default(),
            fail_closed: true,
            capture_protection: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_legacy_network_setting_is_ignored_and_not_saved() {
        let policy: EnvironmentPolicy = serde_json::from_str(
            r#"{
            "enabled": true, "network": "NoAdapters", "fail_closed": false,
            "capture_protection": true,
            "devices": {"removable_storage": false, "portable_media": false}
        }"#,
        )
        .unwrap();
        assert!(policy.enabled);
        assert!(policy.capture_protection);
        assert!(!policy.fail_closed);
        assert!(!policy.devices.any_selected());
        assert!(
            serde_json::to_value(policy)
                .unwrap()
                .get("network")
                .is_none()
        );
    }
}
