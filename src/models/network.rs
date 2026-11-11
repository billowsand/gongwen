use serde::{Deserialize, Serialize};

/// 访问模型服务时走哪条网络通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// 一律直连，连环境变量里的代理也不认。
    Direct,
    /// 跟随系统：先看 `HTTPS_PROXY` / `ALL_PROXY` 等环境变量，没设再读 Windows
    /// 「设置 → 网络 → 代理」（macOS 读系统偏好）。
    #[default]
    System,
    /// 用下面填的代理地址。
    Custom,
}

impl ProxyMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Direct => "直连",
            Self::System => "跟随系统",
            Self::Custom => "自定义代理",
        }
    }

    pub const ALL: [Self; 3] = [Self::Direct, Self::System, Self::Custom];
}

/// 网络代理。起草、复核、知识库的 embedding / rerank 请求都走这一条通道。
///
/// 本机地址（localhost、127.0.0.0/8、::1）不论哪种模式都直连：本地模型服务
/// 绕到代理上再折回来，轻则多一跳，重则被代理规则拦下。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    /// 代理地址，如 `http://127.0.0.1:7890`、`socks5h://127.0.0.1:1080`。
    /// 用户名密码写在地址里：`socks5://user:pass@host:port`。
    pub url: String,
    /// 自定义代理下不走代理的地址，逗号分隔；支持域名（含子域）、IP 与网段，
    /// 如 `intranet.gov.cn, 10.0.0.0/8`。
    pub bypass: String,
}
