/// 窗口标题不展示版本号；软件版本仍以 Cargo.toml 为唯一来源。
pub const APP_TITLE: &str = "公文助手";

#[cfg(test)]
mod tests {
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
}
