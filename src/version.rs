/// 窗口标题不展示版本号；软件版本仍以 Cargo.toml 为唯一来源。
pub const APP_TITLE: &str = "公文助手";
/// Wayland app_id、桌面文件名与窗口规则共同使用的稳定标识。
pub const APP_ID: &str = "cn.localtools.GongwenAssistant";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_uses_three_numeric_components() {
        let components = env!("CARGO_PKG_VERSION").split('.').collect::<Vec<_>>();
        assert_eq!(components.len(), 3, "版本号必须使用 x.x.x 格式");
        assert!(
            components
                .iter()
                .all(|component| component.parse::<u64>().is_ok()),
            "版本号的每一段都必须是非负整数"
        );
    }

    #[test]
    fn app_id_is_a_stable_reverse_dns_identifier() {
        assert_eq!(APP_ID, "cn.localtools.GongwenAssistant");
        assert!(!APP_ID.contains(char::is_whitespace));

        let desktop = include_str!("../assets/linux/cn.localtools.GongwenAssistant.desktop");
        assert!(desktop.contains(&format!("Icon={APP_ID}")));
        assert!(desktop.contains(&format!("StartupWMClass={APP_ID}")));
    }
}
