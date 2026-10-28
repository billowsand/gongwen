//! 解析 Omarchy 主题的 `colors.toml`。
//!
//! 文件只有平铺的 `key = "#rrggbb"` 行（外加 `mode = "dark"`），不引入 toml 依赖，
//! 逐行手写解析：空行、`#` 注释、取值不是六位十六进制颜色的行（例如
//! `hyprland_active_border = "rgba(...) 45deg"`）一律跳过，不报错。

use eframe::egui::Color32;
use std::collections::BTreeMap;
use std::path::Path;

/// `colors.toml` 的体积上限。官方主题都不到 1 KiB；取整 1 MB 是为了让
/// `file_limits::too_large` 的「超过 N MB」提示读得通。
const MAX_COLORS_BYTES: u64 = 1024 * 1024;

/// 旧版主题的 `colorN` 键名与新键名的对应关系，与 Omarchy 自己的回退一致。
const LEGACY_KEYS: [(&str, &str); 15] = [
    ("background", "color0"),
    ("red", "color1"),
    ("green", "color2"),
    ("yellow", "color3"),
    ("blue", "color4"),
    ("magenta", "color5"),
    ("cyan", "color6"),
    ("foreground", "color7"),
    ("muted", "color8"),
    ("bright_red", "color9"),
    ("bright_green", "color10"),
    ("bright_yellow", "color11"),
    ("bright_blue", "color12"),
    ("bright_magenta", "color13"),
    ("bright_cyan", "color14"),
];

/// 一份解析好的 Omarchy 配色。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Colors {
    /// `mode` 键：`Some(true)` 深色、`Some(false)` 明色，缺失或写错为 `None`。
    pub(crate) dark: Option<bool>,
    values: BTreeMap<String, Color32>,
}

impl Colors {
    /// 按新键名取色，没有时退到旧版 `colorN` 键名。
    pub(crate) fn get(&self, key: &str) -> Option<Color32> {
        self.values.get(key).copied().or_else(|| {
            LEGACY_KEYS
                .iter()
                .find(|(name, _)| *name == key)
                .and_then(|(_, legacy)| self.values.get(*legacy).copied())
        })
    }
}

/// 解析 `colors.toml` 文本。至少要有 `background` 与 `foreground`（或旧键名
/// `color0` / `color7`），否则无从换算，返回错误说明。
pub(crate) fn parse(text: &str) -> Result<Colors, String> {
    let mut colors = Colors::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let Some(value) = unquote(value.trim()) else {
            continue;
        };
        if key == "mode" {
            colors.dark = match value {
                "dark" => Some(true),
                "light" => Some(false),
                _ => None,
            };
        } else if let Some(color) = parse_hex(value) {
            colors.values.insert(key.to_owned(), color);
        }
    }
    if colors.get("background").is_none() || colors.get("foreground").is_none() {
        return Err("colors.toml 里缺少 background 或 foreground".into());
    }
    Ok(colors)
}

/// 读并解析一个 `colors.toml`；读前先卡体积。
pub(crate) fn read(path: &Path) -> Result<Colors, String> {
    let bytes = crate::file_limits::read_bytes(path, MAX_COLORS_BYTES, "Omarchy 主题配色")
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    parse(&text).map_err(|error| format!("{}：{error}", path.display()))
}

/// 取引号里的值；引号后面允许跟 `# 注释`。没有引号的值原样返回。
fn unquote(value: &str) -> Option<&str> {
    match value.strip_prefix('"') {
        Some(rest) => rest.split_once('"').map(|(inner, _)| inner.trim()),
        None => Some(value.split('#').next().unwrap_or_default().trim()).filter(|v| !v.is_empty()),
    }
}

/// `#rrggbb`（大小写均可）转颜色；其余写法返回 `None`。
fn parse_hex(value: &str) -> Option<Color32> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    Some(Color32::from_rgb(channel(0)?, channel(2)?, channel(4)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors_mode_and_skips_everything_else() {
        let colors = parse(
            "# 注释\nmode = \"light\"\n\naccent = \"#7AA2F7\"  # 行尾注释\n\
             background = \"#1a1b26\"\nforeground = \"#a9b1d6\"\n\
             hyprland_active_border = \"rgba(26a269ee) rgba(2ec27eee) 45deg\"\n\
             broken = \"#12345\"\nnot a pair\n",
        )
        .expect("应能解析");
        assert_eq!(colors.dark, Some(false));
        assert_eq!(
            colors.get("accent"),
            Some(Color32::from_rgb(0x7A, 0xA2, 0xF7))
        );
        assert_eq!(colors.get("hyprland_active_border"), None);
        assert_eq!(colors.get("broken"), None);
    }

    #[test]
    fn falls_back_to_legacy_color_numbers() {
        let colors = parse("color0 = \"#000000\"\ncolor7 = \"#ffffff\"\ncolor1 = \"#ff0000\"\n")
            .expect("旧键名也应能解析");
        assert_eq!(colors.get("background"), Some(Color32::BLACK));
        assert_eq!(colors.get("foreground"), Some(Color32::WHITE));
        assert_eq!(colors.get("red"), Some(Color32::from_rgb(0xFF, 0, 0)));
        assert_eq!(colors.dark, None);
    }

    #[test]
    fn rejects_a_file_without_background_or_foreground() {
        assert!(parse("accent = \"#ffffff\"\n").is_err());
        assert!(parse("background = \"#ffffff\"\n").is_err());
    }

    #[test]
    fn refuses_to_read_an_oversized_file() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("colors.toml");
        std::fs::write(&path, "#".repeat(MAX_COLORS_BYTES as usize + 1)).expect("写入");
        assert!(read(&path).is_err());
    }
}
