//! 环境安全检测配置。只保存策略，不保存网卡、设备名或序列号等现场信息。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum NetworkPolicy {
    /// 不允许存在任何非回环接口，包括禁用的网卡和虚拟接口。
    NoAdapters,
    /// 允许网卡存在，但不允许链路、非回环地址或默认路由出现。
    #[default]
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DevicePolicy {
    pub removable_storage: bool,
    pub portable_media: bool,
    pub human_interface: bool,
    pub imaging: bool,
    pub other: bool,
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
    /// 默认关闭，避免升级后因为用户当前联网而突然锁定。
    pub enabled: bool,
    pub network: NetworkPolicy,
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
            network: NetworkPolicy::default(),
            devices: DevicePolicy::default(),
            fail_closed: true,
            capture_protection: false,
        }
    }
}
